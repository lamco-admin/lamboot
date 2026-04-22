use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::{bls, fs::EspVolume, policy::Policy, uki};

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

/// Discover all bootable entries across all available volumes.
///
/// Discovery priority:
///   1. BLS entries from /loader/entries/*.conf (on all volumes)
///   2. Custom entries from \EFI\LamBoot\entries\*.toml (future)
///   3. ESP scanning: Windows, UKI, GRUB, rEFInd, tools (fallback)
pub(crate) fn discover_all_entries(
    esp: &mut EspVolume,
    _extra_volumes: &mut [EspVolume],
    policy: &Policy,
) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    // Phase 1: BLS entries from ESP only.
    // Extra volumes (driver-exposed partitions) are NOT scanned for BLS —
    // traversing large ext4/btrfs filesystems hangs under UEFI's I/O model.
    // XBOOTLDR entries come via the ESP's /loader/entries since Fedora/systemd
    // write BLS entries to the ESP regardless of XBOOTLDR presence.
    let bls_entries = bls::discover_bls_entries(esp, policy);
    let bls_count = bls_entries.len();
    for bls_entry in bls_entries {
        entries.push(bls_entry.to_boot_entry());
    }

    if bls_count > 0 {
        log::info!("Found {bls_count} BLS entries");
    }

    // Phase 2: ESP fallback scanning (always runs — catches Windows, tools, etc.)
    entries.extend(discover_windows(esp, policy));
    entries.extend(discover_linux_uki(esp, policy));
    entries.extend(discover_other_loaders(esp, policy));
    entries.extend(discover_tools(esp, policy));

    // Only use legacy distro scanning if no BLS entries were found
    if bls_count == 0 {
        entries.extend(discover_linux_legacy(esp, policy));
    }

    // Deduplicate: if a BLS entry and an ESP scan found the same path, keep the BLS one
    dedup_entries(&mut entries);

    entries
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
fn discover_windows(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let windows_path = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi";
    if esp.exists(windows_path) && policy.allowed(windows_path) {
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
        });
    }
    entries
}

/// Discover Linux Unified Kernel Images (UKI)
/// Discover Linux Unified Kernel Images (UKI) with PE section parsing.
/// Extracts .osrel (OS name/version), .cmdline (embedded command line),
/// and .uname (kernel version) for rich menu entries.
fn discover_linux_uki(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let uki_paths = esp.glob("\\EFI\\Linux\\*.efi");

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

/// Discover legacy Linux installations (vmlinuz on ESP — fallback only)
fn discover_linux_legacy(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let distro_paths = [
        "\\EFI\\fedora",
        "\\EFI\\ubuntu",
        "\\EFI\\debian",
        "\\EFI\\arch",
        "\\EFI\\opensuse",
    ];

    for distro_path in &distro_paths {
        let kernel_pattern = format!("{distro_path}\\vmlinuz*");
        let kernels = esp.glob(&kernel_pattern);

        for kernel_path in kernels {
            if !policy.allowed(&kernel_path) {
                continue;
            }
            let initrd_pattern = kernel_path.replace("vmlinuz", "initrd");
            let initrd_paths = if esp.exists(&initrd_pattern) {
                alloc::vec![initrd_pattern]
            } else {
                Vec::new()
            };

            let distro_name = extract_distro_name(distro_path);
            log::info!("Found Linux kernel: {kernel_path}");

            entries.push(BootEntry {
                id: format!("linux-legacy-{}", entries.len()),
                name: format!("Linux - {distro_name}"),
                kind: EntryKind::LinuxLegacy {
                    kernel_path: kernel_path.clone(),
                    initrd_paths,
                    options: String::from("root=/dev/sda2 rw quiet"),
                },
                icon: Icon::Linux,
                bls_filename: None,
                preflight: None,
            });
        }
    }
    entries
}

/// Discover other bootloaders for chainloading
fn discover_other_loaders(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
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
        if esp.exists(path) && policy.allowed(path) {
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
            });
        }
    }
    entries
}

/// Discover diagnostic tools and recovery options.
/// Reads manifest.toml for module metadata if available.
/// Skipped entirely if `modules.enabled = false` in policy.
fn discover_tools(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();

    if !policy.modules_enabled {
        return entries;
    }

    // Try to load module manifest for friendly names
    let manifest = load_module_manifest(esp);

    let module_paths = esp.glob("\\EFI\\LamBoot\\modules\\*.efi");

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
        });
    }
    entries
}

/// Parse module manifest for friendly names.
/// Returns Vec<(module_id, display_name)>.
fn load_module_manifest(esp: &mut EspVolume) -> Vec<(String, String)> {
    let manifest_path = "\\EFI\\LamBoot\\modules\\manifest.toml";
    let Ok(content) = esp.read_to_string(manifest_path) else {
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

fn extract_distro_name(path: &str) -> String {
    path.rsplit('\\').next().map_or_else(
        || String::from("Linux"),
        |s| {
            let mut s = s.to_string();
            if !s.is_empty() {
                s[0..1].make_ascii_uppercase();
            }
            s
        },
    )
}
