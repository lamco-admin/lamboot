use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    bls,
    bls_parse::BlsEntry,
    fs::Volume,
    fs_types::{BackendTag, FileKind},
    policy::Policy,
    trust_log::{TrustEvent, TrustLog},
    uki,
};

#[derive(Debug, Clone)]
pub(crate) struct BootEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub icon: Icon,
    /// BLS source info for boot counting (None for non-BLS entries)
    pub bls_filename: Option<String>,
    /// Preflight validation results (None if not yet run)
    pub preflight: Option<crate::preflight::PreflightResult>,
    /// SDS-5: back-reference into the `volumes` slice passed to
    /// `discover_all_entries`. Consumed by the boot layer (SDS-3+) to
    /// pick the correct `Volume` as byte source for the kernel read.
    /// For pre-SDS-3 boot paths that still read from ESP, a non-zero
    /// value surfaces as a "kernel not on ESP" preflight warning.
    pub source_volume_index: usize,
    /// SDS-5: backend identifier of the volume this entry came from,
    /// used for trust-log annotation + menu UI grouping.
    pub source_backend_tag: BackendTag,
}

#[derive(Debug, Clone)]
pub(crate) enum EntryKind {
    Chainload {
        path: String,
    },
    Uki {
        path: String,
        options: String,
    },
    LinuxLegacy {
        kernel_path: String,
        initrd_paths: Vec<String>,
        options: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Windows,
    Linux,
    Efi,
    #[expect(dead_code, reason = "used when recovery partition discovery is added")]
    Recovery,
    #[expect(dead_code, reason = "used when network boot support is added")]
    Network,
    Tools,
}

/// Discover all bootable entries across all mounted volumes.
///
/// SDS-5 signature: takes the full volume slice (ESP at index 0 by
/// mount convention, extras 1..n), emits per-volume events into the
/// trust log.
///
/// Discovery priority:
///   1. BLS entries from `/loader/entries/*.conf` on **every** volume
///      that has that directory (spec §2: ESP, XBOOTLDR, ext4 /boot).
///   2. ESP-only scanners: Windows Boot Manager, UKIs under `\EFI\Linux\`,
///      chainloadable other loaders (GRUB, rEFInd, EFI Fallback),
///      diagnostic modules.
///
/// Removed in SDS-5: the v0.8.3 `discover_linux_legacy` heuristic
/// (`\EFI\fedora\vmlinuz*` and friends) — superseded by proper BLS
/// discovery on every volume. Tightens trust story (fewer ambiguous
/// paths) per spec §8.1.
pub(crate) fn discover_all_entries(
    volumes: &mut [Volume],
    policy: &Policy,
    trust_log: &mut TrustLog,
    self_image_sha256: Option<[u8; 32]>,
) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    // Phase 1: scan every volume for BLS entries at /loader/entries/.
    // Volume ordering by construction (SDS-1 mount order): index 0 = ESP,
    // 1..n = extras (XBOOTLDR, ext4 roots). The BLS spec allows entries
    // on ESP or XBOOTLDR; Fedora places them on ext4 /boot. We scan all
    // three uniformly via the FsBackend trait — same code path for FAT
    // and ext4.
    let mut all_bls: Vec<BlsEntry> = Vec::new();
    for (volume_index, volume) in volumes.iter_mut().enumerate() {
        let bls_entries = bls::scan_volume_for_bls(volume, volume_index, policy, trust_log);
        all_bls.extend(bls_entries);
    }

    // Phase 1.5: parsed-entry dedup. Spec §5 (v1.1 amendment):
    //   Rule 1: same filename AND same parsed content -> keep first
    //           (ESP wins by iteration order; silent dedup).
    //   Rule 2: same filename, different parsed content -> keep BOTH
    //           and emit `bls_duplicate_filename` so operators see
    //           the anomaly. Spec-compliant-location preference is
    //           a UI concern (menu ordering), not a dedup one.
    //   Rule 3: same machine-id, different filenames -> both kept
    //           (handled by default — no dedup on machine-id alone).
    all_bls = dedup_bls_entries(all_bls, trust_log);

    let bls_count = all_bls.len();
    if bls_count > 0 {
        log::info!("Found {bls_count} BLS entries across all volumes");
    }
    for bls_entry in all_bls {
        entries.push(bls_entry.to_boot_entry());
    }

    // Phase 2: ESP-only scanners. These paths are UEFI conventions (Windows
    // boot manager, UKIs under \EFI\Linux\, chainload targets) that only
    // live on the FAT ESP by firmware contract. Tag each entry as
    // source_volume_index = 0 (ESP).
    if !volumes.is_empty() {
        let esp = &mut volumes[0];
        let esp_tag = esp.backend_tag();
        let mut push_esp = |mut new_entries: Vec<BootEntry>| {
            for e in &mut new_entries {
                e.source_volume_index = 0;
                e.source_backend_tag = esp_tag;
            }
            entries.extend(new_entries);
        };
        push_esp(discover_windows(esp, policy));
        push_esp(discover_linux_uki(esp, policy));
        push_esp(discover_systemd_boot_dir_style(esp, policy));
        push_esp(discover_other_loaders(esp, policy, self_image_sha256));
        push_esp(discover_tools(esp, policy));
    }

    // Deduplicate: same filename BLS-vs-UKI collisions, etc. SDS-5 spec
    // §5 (amendment: parsed-entry equality, not SHA-256 content).
    dedup_entries(&mut entries);

    entries
}

/// SDS-5 §5 dedup for BLS entries before they become `BootEntry`s.
///
/// Linear scan (O(n²) on ~10-20 entries — negligible). For each
/// incoming entry, look for an earlier-seen entry with the same
/// (case-insensitive) filename. If content matches, drop silently
/// (keep first occurrence). If content differs, keep both and emit
/// `bls_duplicate_filename` so the operator can see that multiple
/// conflicting .conf files exist under the same name.
fn dedup_bls_entries(entries: Vec<BlsEntry>, trust_log: &mut TrustLog) -> Vec<BlsEntry> {
    let mut deduped: Vec<BlsEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let existing = deduped
            .iter()
            .find(|e| e.filename.eq_ignore_ascii_case(&entry.filename));
        match existing {
            Some(e) if *e == entry => {
                // Rule 1: same filename + same content -> silent dedup.
                log::debug!(
                    "bls dedup: {} on volume {} (backend={}) matches earlier copy on volume {} (backend={})",
                    entry.filename,
                    entry.source_volume_index,
                    entry.source_backend_tag,
                    e.source_volume_index,
                    e.source_backend_tag,
                );
            }
            Some(e) => {
                // Rule 2: same filename + different content -> keep both
                // AND log. Operator decides; LamBoot surfaces, doesn't hide.
                trust_log.record(
                    TrustEvent::new("bls_duplicate_filename")
                        .with_path(&entry.filename)
                        .with_note(&format!(
                            "volume_a={} backend_a={} volume_b={} backend_b={}",
                            e.source_volume_index,
                            e.source_backend_tag,
                            entry.source_volume_index,
                            entry.source_backend_tag,
                        )),
                );
                deduped.push(entry);
            }
            None => {
                deduped.push(entry);
            }
        }
    }
    deduped
}

/// Deduplicate entries — matches on kernel filename (not full path) so that
/// the same kernel discovered from ESP (`\EFI\Linux\vmlinuz-6.19.9...`) and
/// from /boot via ext4 driver (`\vmlinuz-6.19.9...`) is recognized as one entry.
/// BLS entries (prefixed "bls-") take precedence since they appear first.
fn dedup_entries(entries: &mut Vec<BootEntry>) {
    let mut seen: Vec<String> = Vec::new();
    entries.retain(|entry| {
        let key = match &entry.kind {
            EntryKind::Chainload { path } | EntryKind::Uki { path, .. } => {
                extract_filename_lower(path)
            }
            EntryKind::LinuxLegacy {
                ref kernel_path, ..
            } => extract_filename_lower(kernel_path),
        };
        if seen.iter().any(|s| s == &key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

fn extract_filename_lower(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_lowercase()
}

/// Discover Windows Boot Manager
fn discover_windows(esp: &mut Volume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let windows_path = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi";
    if esp.exists_str(windows_path) && policy.allowed(windows_path) {
        log::info!("Found Windows Boot Manager");
        entries.push(BootEntry {
            id: String::from("windows"),
            name: String::from("Windows Boot Manager"),
            kind: EntryKind::Chainload {
                path: String::from(windows_path),
            },
            icon: Icon::Windows,
            bls_filename: None,
            preflight: None,
            source_volume_index: 0,
            source_backend_tag: "",
        });
    }
    entries
}

/// Discover Linux Unified Kernel Images (UKI)
/// Discover Linux Unified Kernel Images (UKI) with PE section parsing.
/// Extracts .osrel (OS name/version), .cmdline (embedded command line),
/// and .uname (kernel version) for rich menu entries.
fn discover_linux_uki(esp: &mut Volume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let uki_paths = esp.glob("/EFI/Linux/*.efi");

    for path in uki_paths {
        if !policy.allowed(&path) {
            continue;
        }

        // Parse PE section headers with targeted seeks (avoids loading
        // the entire UKI binary, which can be 60-100MB on Fedora)
        let metadata = uki::read_uki_metadata(esp, &path);

        // Build display name from parsed metadata or fall back to filename
        let name = match &metadata {
            Some(m) if m.os_name.is_some() => {
                let os = m.os_name.as_deref().unwrap_or("Linux");
                match &m.kernel_version {
                    Some(kv) => format!("{os} ({kv})"),
                    None => String::from(os),
                }
            }
            _ => {
                let fname = extract_filename(&path).unwrap_or_else(|| String::from("Linux"));
                format!("Linux ({fname})")
            }
        };

        // Use embedded cmdline or empty (NOT hardcoded "quiet splash")
        let options = metadata
            .as_ref()
            .and_then(|m| m.cmdline.clone())
            .unwrap_or_default();

        log::info!("Found Linux UKI: {path} -> {name}");

        entries.push(BootEntry {
            id: format!("uki-{}", entries.len()),
            name,
            kind: EntryKind::Uki {
                path: path.clone(),
                options,
            },
            icon: Icon::Linux,
            bls_filename: None,
            preflight: None,
            source_volume_index: 0,
            source_backend_tag: "",
        });
    }

    // Sort UKIs by kernel version (newest first) using existing UAPI.10 comparison
    entries.sort_by(|a, b| {
        let av = extract_uki_version(a);
        let bv = extract_uki_version(b);
        bls::version_compare(&av, &bv).reverse()
    });

    entries
}

/// Extract a version string from a UKI BootEntry name for sorting.
/// Looks for the version in parentheses: "Fedora Linux 43 (6.12.0-200...)" → "6.12.0-200..."
fn extract_uki_version(entry: &BootEntry) -> String {
    if let Some(start) = entry.name.rfind('(') {
        if let Some(end) = entry.name.rfind(')') {
            if start < end {
                return String::from(&entry.name[start + 1..end]);
            }
        }
    }
    entry.id.clone()
}

// `discover_linux_legacy` removed in SDS-5 per spec §8.1.
// Rationale: the v0.8.3 distro-path heuristic (`\EFI\fedora\vmlinuz*`,
// `\EFI\ubuntu\vmlinuz*`, etc.) was a discovery hack predating proper
// BLS support. v0.9.x scans BLS entries on every mounted volume —
// including ext4 /boot/loader/entries/ where Fedora actually stores
// its kernels. The legacy heuristic is superseded and was a source of
// ambiguous trust decisions (same kernel potentially discovered via
// two paths). Removed for a tighter trust story.

/// Discover other bootloaders for chainloading.
///
/// `self_image_sha256` is the SHA-256 of the running LamBoot image
/// (computed by `main.rs` from `LoadedImageProtocol` of the current
/// image_handle). When `\EFI\BOOT\BOOTX64.EFI` exists and its hash
/// matches, the synthetic "EFI Fallback" entry is **skipped** —
/// otherwise chainloading the fallback path would self-loop back
/// into LamBoot. This case arises when operators use the toolkit's
/// belt-and-suspenders pattern (`repair.fallback.replace_with_lamboot`)
/// to write LamBoot's signed binary to BOOTX64.EFI.
fn discover_other_loaders(
    esp: &mut Volume,
    policy: &Policy,
    self_image_sha256: Option<[u8; 32]>,
) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    let loaders = [
        ("\\EFI\\grub\\grubx64.efi", "grub", "GRUB Bootloader"),
        (
            "\\EFI\\refind\\refind_x64.efi",
            "refind",
            "rEFInd Boot Manager",
        ),
        ("\\EFI\\BOOT\\BOOTX64.EFI", "fallback", "EFI Fallback"),
    ];

    for (path, id, name) in &loaders {
        if !(esp.exists_str(path) && policy.allowed(path)) {
            continue;
        }
        // Self-loop guard for the BOOTX64.EFI fallback synthetic.
        // Only meaningful for that one path; GRUB/rEFInd are different
        // binaries by definition.
        if *path == "\\EFI\\BOOT\\BOOTX64.EFI" {
            if let Some(self_hash) = self_image_sha256 {
                if let Ok(bytes) = esp.read(
                    crate::fs_types::PathBuf::from_str("/EFI/BOOT/BOOTX64.EFI")
                        .unwrap()
                        .as_path(),
                ) {
                    let bootx_hash = crate::pe_loader_pure::sha256_of(&bytes);
                    if bootx_hash == self_hash {
                        log::debug!(
                            "discover_other_loaders: skipping EFI Fallback synthetic — \\EFI\\BOOT\\BOOTX64.EFI hash matches running image (self-loop guard)"
                        );
                        continue;
                    }
                }
            }
        }
        log::info!("Found {name}");
        entries.push(BootEntry {
            id: String::from(*id),
            name: String::from(*name),
            kind: EntryKind::Chainload {
                path: String::from(*path),
            },
            icon: Icon::Efi,
            bls_filename: None,
            preflight: None,
            source_volume_index: 0,
            source_backend_tag: "",
        });
    }
    entries
}

/// Discover Linux distros that lay out kernels using systemd-boot's
/// "discoverable EFI" directory pattern: `/EFI/<name>-<uuid>/vmlinuz.efi`
/// alongside sibling `cmdline` (text) and `initrd.img` files.
///
/// Pop!_OS (kernelstub-managed) is the canonical example —
/// `\EFI\Pop_OS-<root-uuid>\vmlinuz.efi`. Pop!_OS Recovery uses
/// `\EFI\Recovery-<short-uuid>\vmlinuz.efi`. Other distros adopting
/// the same pattern (Garuda variants, custom systemd-boot installs)
/// are picked up automatically.
///
/// Distinguishes itself from `discover_linux_uki` (which globs
/// `/EFI/Linux/*.efi` for unified kernel images with PE sections) and
/// from the v0.8.3 `discover_linux_legacy` heuristic (removed in
/// SDS-5) by requiring a UUID-shaped suffix on the directory name.
/// This avoids collision with bootloader vendor directories
/// (`/EFI/grub/`, `/EFI/systemd/`, `/EFI/fedora/`, etc.) which lack
/// the dash-suffix pattern.
fn discover_systemd_boot_dir_style(esp: &mut Volume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    let Ok(efi_dir) = crate::fs_types::PathBuf::from_str("/EFI") else {
        return entries;
    };

    let Ok(dirs) = esp.read_dir(efi_dir.as_path()) else {
        return entries;
    };

    for entry in dirs {
        if entry.kind != FileKind::Directory {
            continue;
        }
        if !crate::discovery_pure::looks_like_distro_uuid_dir(&entry.name) {
            continue;
        }

        // Three sibling files we care about: vmlinuz.efi, cmdline, initrd.img
        // Per Pop!_OS layout. `vmlinuz-previous.efi` exists for the
        // last-known-good kernel; we surface that as a separate entry.
        let dir_path = format!("/EFI/{}", entry.name);
        let kernel_path = format!("{dir_path}/vmlinuz.efi");
        let cmdline_path = format!("{dir_path}/cmdline");
        let initrd_path = format!("{dir_path}/initrd.img");
        let prev_kernel_path = format!("{dir_path}/vmlinuz-previous.efi");
        let prev_initrd_path = format!("{dir_path}/initrd.img-previous");

        if !esp.exists_str(&kernel_path) {
            continue;
        }
        // Convert forward slashes to backslashes for the EntryKind path
        // (UEFI-style paths used everywhere else in BootEntry).
        let kernel_efi = crate::discovery_pure::forward_to_backslash(&kernel_path);
        let initrd_efi = crate::discovery_pure::forward_to_backslash(&initrd_path);
        let prev_kernel_efi = crate::discovery_pure::forward_to_backslash(&prev_kernel_path);
        let prev_initrd_efi = crate::discovery_pure::forward_to_backslash(&prev_initrd_path);

        if !policy.allowed(&kernel_efi) {
            continue;
        }

        let cmdline = esp
            .read_to_string_str(&cmdline_path)
            .unwrap_or_default()
            .trim()
            .to_string();
        let initrd_paths = if esp.exists_str(&initrd_path) {
            alloc::vec![initrd_efi]
        } else {
            Vec::new()
        };

        let display_name = crate::discovery_pure::pretty_name_from_distro_uuid_dir(&entry.name);
        let id = format!("sdboot-{}", entry.name);

        log::info!("Found systemd-boot-dir-style entry: {display_name} ({kernel_efi})");
        entries.push(BootEntry {
            id,
            name: display_name.clone(),
            kind: EntryKind::LinuxLegacy {
                kernel_path: kernel_efi,
                initrd_paths,
                options: cmdline.clone(),
            },
            icon: Icon::Linux,
            bls_filename: None,
            preflight: None,
            source_volume_index: 0,
            source_backend_tag: "",
        });

        // Surface vmlinuz-previous.efi as a sibling "last-known-good" entry
        // when present. Pop!_OS keeps this so users can recover from a bad
        // kernel update; LamBoot honors that semantic.
        if esp.exists_str(&prev_kernel_path) && policy.allowed(&prev_kernel_efi) {
            let prev_initrd = if esp.exists_str(&prev_initrd_path) {
                alloc::vec![prev_initrd_efi]
            } else {
                Vec::new()
            };
            entries.push(BootEntry {
                id: format!("sdboot-{}-previous", entry.name),
                name: format!("{display_name} (previous kernel)"),
                kind: EntryKind::LinuxLegacy {
                    kernel_path: prev_kernel_efi,
                    initrd_paths: prev_initrd,
                    options: cmdline,
                },
                icon: Icon::Linux,
                bls_filename: None,
                preflight: None,
                source_volume_index: 0,
                source_backend_tag: "",
            });
        }
    }

    entries
}

// Pure helpers `looks_like_distro_uuid_dir`,
// `pretty_name_from_distro_uuid_dir`, and `forward_to_backslash` live
// in `discovery_pure.rs` and are re-exported by lib.rs into the
// lamboot-fs-tests host-test crate via `#[path]` (matches the
// bls_parse / pe_loader_pure / trust_log_pure pattern). See
// `discovery_pure.rs` for behavior.

/// Discover diagnostic tools and recovery options.
/// Reads manifest.toml for module metadata if available.
/// Skipped entirely if `modules.enabled = false` in policy.
fn discover_tools(esp: &mut Volume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    if !policy.modules_enabled {
        return entries;
    }

    // Try to load module manifest for friendly names
    let manifest = load_module_manifest(esp);

    let module_paths = esp.glob("/EFI/LamBoot/modules/*.efi");

    for path in module_paths {
        if !policy.allowed(&path) {
            continue;
        }
        let filename = extract_filename(&path).unwrap_or_else(|| String::from("tool"));

        // Look up friendly name from manifest
        let display_name = manifest
            .iter()
            .find(|(id, _)| id == &filename)
            .map_or_else(|| format!("Tool: {filename}"), |(_, name)| name.clone());

        log::info!("Found diagnostic module: {display_name} ({path})");
        entries.push(BootEntry {
            id: format!("tool-{filename}"),
            name: display_name,
            kind: EntryKind::Chainload { path },
            icon: Icon::Tools,
            bls_filename: None,
            preflight: None,
            source_volume_index: 0,
            source_backend_tag: "",
        });
    }
    entries
}

/// Parse module manifest for friendly names.
/// Returns Vec<(module_id, display_name)>.
fn load_module_manifest(esp: &mut Volume) -> Vec<(String, String)> {
    let manifest_path = "\\EFI\\LamBoot\\modules\\manifest.toml";
    let Ok(content) = esp.read_to_string_str(manifest_path) else {
        return Vec::new();
    };

    let mut modules = Vec::new();
    let mut current_module_id: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse [modules.xxx] section headers
        if line.starts_with("[modules.") {
            if let Some(end) = line.rfind(']') {
                let id = &line[9..end]; // skip "[modules."
                current_module_id = Some(String::from(id));
            }
            continue;
        }

        // Parse name = "..." within a module section
        if let Some(ref module_id) = current_module_id {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "name" {
                    let name = value.trim().trim_matches('"');
                    modules.push((module_id.clone(), String::from(name)));
                }
            }
        }
    }

    modules
}

fn extract_filename(path: &str) -> Option<String> {
    path.rsplit('\\')
        .next()
        .map(|s| s.trim_end_matches(".efi").to_string())
}

// `extract_distro_name` removed in SDS-5 along with `discover_linux_legacy`.
