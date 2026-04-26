//! Boot Loader Specification (BLS) Type 1 discovery — volume side.
//!
//! The pure byte-level parser lives in `bls_parse.rs` so host tests
//! can include it verbatim (SDS-5 §9.1). This file holds the
//! Volume-dependent code:
//!
//!   * `scan_volume_for_bls` — walks a `Volume`'s `/loader/entries/`
//!     directory via the `FsBackend` trait, parses each `.conf` via
//!     `BlsEntry::parse`, emits per-volume trust events, returns the
//!     sorted `Vec<BlsEntry>` (SDS-5 §4.1).
//!   * `to_boot_entry` — converts a parsed `BlsEntry` into the
//!     menu-system `BootEntry`, preserving `source_volume_index` and
//!     `source_backend_tag` for downstream use.
//!   * `decrement_boot_count` — renames the `.conf` file on FAT
//!     (`entry+3-0.conf` → `entry+2-1.conf`) via `EspWriter`. Skips
//!     silently on non-FAT backends; SDS-5 PR-3 wires the
//!     `boot_counter_skipped_ro` event.

use alloc::{format, string::String, vec::Vec};

// Re-export the pure parser types so other modules in lamboot-core keep
// `use crate::bls::{BlsEntry, version_compare};` working. `BootCountState`
// is not currently used outside `bls_parse` itself but stays exported
// from there for host tests.
pub(crate) use crate::bls_parse::{version_compare, BlsEntry};
use crate::{
    bls_parse::{
        bls_sort_compare, count_digits_in_filename, has_extension_ignore_case,
        is_native_architecture,
    },
    discovery::{BootEntry, EntryKind, Icon},
    fs::Volume,
    policy::Policy,
    trust_log::{TrustEvent, TrustLog},
};

/// Convert a parsed `BlsEntry` into the menu's `BootEntry`. Preserves
/// source fields so the boot layer can pick the correct volume.
pub(crate) fn to_boot_entry(entry: &BlsEntry) -> BootEntry {
    let name = entry.title.clone().unwrap_or_else(|| {
        if let Some(ref ver) = entry.version {
            format!("Linux {ver}")
        } else {
            entry.id.clone()
        }
    });

    let kind = if let Some(ref efi_path) = entry.efi {
        EntryKind::Chainload {
            path: efi_path.clone(),
        }
    } else if let Some(ref linux_path) = entry.linux {
        EntryKind::LinuxLegacy {
            kernel_path: linux_path.clone(),
            initrd_paths: entry.cleaned_initrd(),
            options: entry.combined_options(),
        }
    } else {
        // Should not reach here due to parse-time validation.
        return BootEntry {
            id: entry.id.clone(),
            name,
            kind: EntryKind::Chainload {
                path: String::new(),
            },
            icon: Icon::Linux,
            bls_filename: None,
            preflight: None,
            source_volume_index: entry.source_volume_index,
            source_backend_tag: entry.source_backend_tag,
        };
    };

    BootEntry {
        id: format!("bls-{}", entry.id),
        name,
        kind,
        icon: Icon::Linux,
        bls_filename: Some(entry.filename.clone()),
        preflight: None,
        source_volume_index: entry.source_volume_index,
        source_backend_tag: entry.source_backend_tag,
    }
}

/// Scan a single volume for BLS Type 1 entries at `/loader/entries/`.
///
/// SDS-5 §4.1 per-volume scanner. Uses `Volume`'s FsBackend-uniform
/// API — works identically on FAT (ESP / XBOOTLDR) and ext4 (Fedora
/// /boot). Emits per-volume trust events for operator visibility:
///
///   * `bls_entry_read_failed` — a `.conf` file's bytes could not be read
///   * `bls_entry_invalid` — parse failure (malformed `.conf`)
///   * `bls_entries_found` — terminal event with count + backend tag
///
/// Volumes without `/loader/entries/` return an empty `Vec` silently —
/// that's the common case (most disk volumes have nothing to offer).
pub(crate) fn scan_volume_for_bls(
    volume: &mut Volume,
    volume_index: usize,
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Vec<BlsEntry> {
    let entries_dir = "/loader/entries";
    let backend_tag = volume.backend_tag();

    // `/loader/entries/` not present on this volume — silent: most
    // volumes legitimately have no BLS directory. Genuine I/O errors
    // (as opposed to NotFound) surface as Err too; distinguishing
    // them requires an `exists()` probe first, which we skip on the
    // common-empty path for perf (§10 budget).
    let Ok(filenames) = volume.read_dir_str(entries_dir) else {
        return Vec::new();
    };

    let mut entries: Vec<BlsEntry> = Vec::new();

    for filename in &filenames {
        // Must end with `.conf`.
        if !has_extension_ignore_case(filename, "conf") {
            continue;
        }
        // Reject hidden (`.`) + auto-generated (`auto-*`) filenames.
        if filename.starts_with('.') || filename.starts_with("auto-") {
            continue;
        }

        let path = format!("{entries_dir}/{filename}");

        let content = match volume.read_to_string_str(&path) {
            Ok(s) => s,
            Err(e) => {
                trust_log.record(
                    TrustEvent::new("bls_entry_read_failed")
                        .with_path(&path)
                        .with_note(&format!(
                            "volume_index={volume_index} backend={backend_tag} err={e}"
                        )),
                );
                continue;
            }
        };

        match BlsEntry::parse(filename, &content) {
            Some(mut entry) => {
                // Tag the source so downstream (boot layer, trust log,
                // menu UI) can pick the correct volume / group entries.
                entry.source_volume_index = volume_index;
                entry.source_backend_tag = backend_tag;

                // Filter by architecture — accept entries matching our target.
                if let Some(ref arch) = entry.architecture {
                    if !is_native_architecture(arch) {
                        continue;
                    }
                }

                // Check policy allowlist/denylist.
                if let Some(ref linux_path) = entry.linux {
                    if !policy.allowed(linux_path) {
                        continue;
                    }
                }

                // Auto-discover initrd if entry has none. Pre-SDS-5 this
                // always probed the ESP; now we probe the SAME volume the
                // entry came from (Fedora's initramfs lives next to its
                // kernel on ext4, not on ESP).
                if entry.initrd.is_empty() {
                    if let Some(ref linux_path) = entry.linux {
                        let discovered = crate::autodiscovery::find_initrd(linux_path, volume);
                        if !discovered.is_empty() {
                            log::info!(
                                "Auto-discovered {} initrd(s) for {}",
                                discovered.len(),
                                entry.id
                            );
                            entry.initrd = discovered;
                        }
                    }
                }

                entries.push(entry);
            }
            None => {
                trust_log.record(
                    TrustEvent::new("bls_entry_invalid")
                        .with_path(&path)
                        .with_note(&format!(
                            "volume_index={volume_index} backend={backend_tag} \
                             parse=missing_linux_or_efi"
                        )),
                );
            }
        }
    }

    // Terminal trust event: count + backend identity so an operator
    // reading the log knows which backend produced which entries.
    trust_log.record(TrustEvent::new("bls_entries_found").with_note(&format!(
        "volume_index={volume_index} backend={backend_tag} count={}",
        entries.len()
    )));

    // Sort entries per BLS spec.
    entries.sort_by(bls_sort_compare);

    entries
}

/// Provide the `.to_boot_entry()` method on `BlsEntry` used by the
/// discovery pipeline. Delegates to the free function so `bls_parse`
/// stays free of `BootEntry` / UEFI dependencies.
impl BlsEntry {
    pub(crate) fn to_boot_entry(&self) -> BootEntry {
        to_boot_entry(self)
    }
}

/// Decrement the boot counter for a BLS entry.
/// Renames the .conf file on the FAT filesystem: +3 → +2-1, +2-1 → +1-2, etc.
/// Returns the new filename and sets the LoaderBootCountPath EFI variable.
pub(crate) fn decrement_boot_count(
    esp: &mut crate::fs::Volume,
    entry: &BlsEntry,
) -> Option<String> {
    let tries_left = entry.tries_left?;
    if tries_left == 0 {
        return None; // Already bad, nothing to decrement.
    }

    let tries_done = entry.tries_done.unwrap_or(0);
    let new_left = tries_left - 1;
    let new_done = tries_done.saturating_add(1);

    // Preserve digit width for padding (e.g., +03 stays 2 digits).
    let left_width = count_digits_in_filename(&entry.filename, '+');
    let done_width = count_digits_in_filename(&entry.filename, '-');

    let new_suffix = if done_width > 0 {
        format!(
            "+{:0>width_l$}-{:0>width_d$}",
            new_left,
            new_done,
            width_l = left_width.max(1),
            width_d = done_width.max(1)
        )
    } else {
        format!("+{:0>width$}", new_left, width = left_width.max(1))
    };

    let new_filename = format!("{}{}.conf", entry.id, new_suffix);

    let entries_dir_str = "/loader/entries";
    let Ok(entries_dir) = crate::fs_backend::PathBuf::from_str(entries_dir_str) else {
        log::warn!("decrement_boot_count: entries_dir path canonicalization failed");
        return None;
    };
    let Some(mut writer) = crate::fs_writer::EspWriter::new(esp) else {
        log::warn!(
            "decrement_boot_count skipped: target volume is not FAT (boot-count \
             rename cannot be performed on non-FAT backends per SDS-1)"
        );
        return None;
    };
    match writer.rename(entries_dir.as_path(), &entry.filename, &new_filename) {
        Ok(()) => {
            log::info!("Boot count: {} → {}", entry.filename, new_filename);
            // Set LoaderBootCountPath for systemd-bless-boot compatibility.
            // Keep the backslash form since the variable is consumed by
            // systemd-bless-boot, which expects UEFI-style paths.
            let boot_count_path = format!("\\loader\\entries\\{new_filename}");
            let _ = crate::health::set_boot_count_path(&boot_count_path);
            Some(new_filename)
        }
        Err(e) => {
            log::warn!(
                "Failed to rename {} → {}: {}",
                entry.filename,
                new_filename,
                e
            );
            None
        }
    }
}
