//! Boot Loader Specification (BLS) Type 1 entry parser.
//!
//! Parses `/loader/entries/*.conf` files per the UAPI Group specification.
//! Supports all 14 fields, multi-value initrd/options, boot counting (+N[-M]),
//! and UAPI.10 version comparison for sorting.

use alloc::{format, string::String, vec::Vec};
use core::cmp::Ordering;

use crate::{
    discovery::{BootEntry, EntryKind, Icon},
    fs::Volume,
    policy::Policy,
};

/// A parsed BLS Type 1 entry
#[derive(Debug, Clone)]
pub(crate) struct BlsEntry {
    /// Entry ID (filename without .conf and boot count suffix)
    pub id: String,
    /// Full filename (used by boot counting rename)
    pub filename: String,
    // BLS fields
    pub title: Option<String>,
    pub version: Option<String>,
    pub machine_id: Option<String>,
    pub sort_key: Option<String>,
    pub linux: Option<String>,
    pub initrd: Vec<String>,
    pub efi: Option<String>,
    pub options: Vec<String>,
    pub architecture: Option<String>,
    // Boot counting
    pub tries_left: Option<u32>,
    pub tries_done: Option<u32>,
}

/// Boot count state derived from tries_left
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootCountState {
    /// No counters — entry is known good
    Good,
    /// tries_left > 0 — being assessed
    Indeterminate,
    /// tries_left == 0 — entry failed assessment
    Bad,
}

impl BlsEntry {
    /// Parse a BLS entry from file contents
    pub(crate) fn parse(filename: &str, content: &str) -> Option<Self> {
        let mut entry = BlsEntry {
            id: String::new(),
            filename: String::from(filename),
            title: None,
            version: None,
            machine_id: None,
            sort_key: None,
            linux: None,
            initrd: Vec::new(),
            efi: None,
            options: Vec::new(),
            architecture: None,
            tries_left: None,
            tries_done: None,
        };

        // Parse boot counting suffix from filename
        // Format: <base>+<tries_left>[-<tries_done>].conf
        let name_without_ext = filename.trim_end_matches(".conf");
        if let Some(plus_pos) = name_without_ext.rfind('+') {
            entry.id = String::from(&name_without_ext[..plus_pos]);
            let count_part = &name_without_ext[plus_pos + 1..];
            if let Some(dash_pos) = count_part.find('-') {
                entry.tries_left = count_part[..dash_pos].parse().ok();
                entry.tries_done = count_part[dash_pos + 1..].parse().ok();
            } else {
                entry.tries_left = count_part.parse().ok();
                entry.tries_done = Some(0);
            }
        } else {
            entry.id = String::from(name_without_ext);
        }

        // Parse fields
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Split on first whitespace
            let (key, value) = if let Some(pos) = line.find(|c: char| c.is_ascii_whitespace()) {
                let k = &line[..pos];
                let v = line[pos..].trim_start();
                (k, v)
            } else {
                continue;
            };

            match key {
                "title" => entry.title = Some(String::from(value)),
                "version" => entry.version = Some(String::from(value)),
                "machine-id" => entry.machine_id = Some(String::from(value)),
                "sort-key" => entry.sort_key = Some(String::from(value)),
                "linux" => entry.linux = Some(normalize_path(value)),
                "efi" => entry.efi = Some(normalize_path(value)),
                "initrd" => {
                    // initrd can have multiple space-separated values on one line
                    for part in value.split_whitespace() {
                        entry.initrd.push(normalize_path(part));
                    }
                }
                "options" => entry.options.push(String::from(value)),
                "architecture" => entry.architecture = Some(String::from(value)),
                _ => {} // Unknown keys silently ignored per spec
            }
        }

        // Validation: at least linux or efi must be present
        if entry.linux.is_none() && entry.efi.is_none() {
            return None;
        }

        Some(entry)
    }

    /// Get the boot count state
    pub(crate) fn boot_count_state(&self) -> BootCountState {
        match self.tries_left {
            None => BootCountState::Good,
            Some(0) => BootCountState::Bad,
            Some(_) => BootCountState::Indeterminate,
        }
    }

    /// Get combined options as a single string, stripping GRUB $variable tokens
    pub(crate) fn combined_options(&self) -> String {
        strip_grub_variables(&self.options.join(" "))
    }

    /// Get initrd paths with GRUB $variable tokens removed
    pub(crate) fn cleaned_initrd(&self) -> Vec<String> {
        self.initrd
            .iter()
            .filter(|s| !s.starts_with('$'))
            .cloned()
            .collect()
    }

    /// Convert to a BootEntry for the menu system
    pub(crate) fn to_boot_entry(&self) -> BootEntry {
        let name = self.title.clone().unwrap_or_else(|| {
            if let Some(ref ver) = self.version {
                format!("Linux {ver}")
            } else {
                self.id.clone()
            }
        });

        let kind = if let Some(ref efi_path) = self.efi {
            EntryKind::Chainload {
                path: efi_path.clone(),
            }
        } else if let Some(ref linux_path) = self.linux {
            EntryKind::LinuxLegacy {
                kernel_path: linux_path.clone(),
                initrd_paths: self.cleaned_initrd(),
                options: self.combined_options(),
            }
        } else {
            // Should not reach here due to validation
            return BootEntry {
                id: self.id.clone(),
                name,
                kind: EntryKind::Chainload {
                    path: String::new(),
                },
                icon: Icon::Linux,
                bls_filename: None,
                preflight: None,
            };
        };

        BootEntry {
            id: format!("bls-{}", self.id),
            name,
            kind,
            icon: Icon::Linux,
            bls_filename: Some(self.filename.clone()),
            preflight: None,
        }
    }
}

/// Discover BLS Type 1 entries on a volume
pub(crate) fn discover_bls_entries(esp: &mut Volume, policy: &Policy) -> Vec<BlsEntry> {
    let entries_dir = "/loader/entries";

    let Ok(filenames) = esp.read_dir_str(entries_dir) else {
        return Vec::new();
    };

    let mut entries: Vec<BlsEntry> = Vec::new();

    for filename in &filenames {
        // Must end with .conf
        if !has_extension_ignore_case(filename, "conf") {
            continue;
        }
        // Reject filenames starting with . or auto-
        if filename.starts_with('.') || filename.starts_with("auto-") {
            continue;
        }

        let path = format!("{entries_dir}/{filename}");

        let Ok(content) = esp.read_to_string_str(&path) else {
            continue;
        };

        if let Some(mut entry) = BlsEntry::parse(filename, &content) {
            // Filter by architecture — accept entries matching our target
            if let Some(ref arch) = entry.architecture {
                if !is_native_architecture(arch) {
                    continue;
                }
            }

            // Check policy allowlist/denylist
            if let Some(ref linux_path) = entry.linux {
                if !policy.allowed(linux_path) {
                    continue;
                }
            }

            // Auto-discover initrd if entry has none
            if entry.initrd.is_empty() {
                if let Some(ref linux_path) = entry.linux {
                    let discovered = crate::autodiscovery::find_initrd(linux_path, esp);
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
    }

    // Sort entries per BLS spec
    entries.sort_by(bls_sort_compare);

    entries
}

/// BLS sort comparison per specification:
/// 1. Bad entries (tries_left==0) sort last
/// 2. Entries with sort-key sort before those without
/// 3. Compare sort-key, then machine-id, then version (descending)
/// 4. Filename fallback (descending version order)
fn bls_sort_compare(a: &BlsEntry, b: &BlsEntry) -> Ordering {
    // Bad entries last
    let a_bad = a.boot_count_state() == BootCountState::Bad;
    let b_bad = b.boot_count_state() == BootCountState::Bad;
    if a_bad != b_bad {
        return if a_bad {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }

    // sort-key presence: entries WITH sort-key come first
    let a_has_sk = a.sort_key.is_some();
    let b_has_sk = b.sort_key.is_some();
    if a_has_sk != b_has_sk {
        return if a_has_sk {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    // Both have sort-key: compare sort-key → machine-id → version (desc)
    if a_has_sk && b_has_sk {
        let sk_cmp = cmp_opt_str(a.sort_key.as_ref(), b.sort_key.as_ref());
        if sk_cmp != Ordering::Equal {
            return sk_cmp;
        }

        let mid_cmp = cmp_opt_str(a.machine_id.as_ref(), b.machine_id.as_ref());
        if mid_cmp != Ordering::Equal {
            return mid_cmp;
        }

        // Version: descending (newer first)
        let ver_cmp = version_compare(
            a.version.as_deref().unwrap_or(""),
            b.version.as_deref().unwrap_or(""),
        );
        if ver_cmp != Ordering::Equal {
            return ver_cmp.reverse();
        }
    }

    // Filename fallback.
    // If BOTH entries have version metadata, treat the filename as a version-like
    // identifier and sort descending (newer version-like names first).
    // If either is missing a version, there's nothing version-y to reverse —
    // use plain ascending alphabetical order so predictable names like
    // "Pop_OS-current" beat "Recovery-79EB-58C6". Reversing in the no-version
    // case produced the Pop!_OS recovery-as-default bug on v0.8.3 (task #51).
    if a.version.is_some() && b.version.is_some() {
        version_compare(&a.id, &b.id).reverse()
    } else {
        a.id.cmp(&b.id)
    }
}

/// Compare optional strings (None sorts lower than Some)
fn cmp_opt_str(a: Option<&String>, b: Option<&String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater, // None sorts after
        (Some(_), None) => Ordering::Less,    // Some sorts before
        (Some(a), Some(b)) => a.cmp(b),
    }
}

/// UAPI.10 Version Format Comparison
///
/// Compares two version strings according to the UAPI Group specification.
/// Used for BLS entry sorting and boot counting.
pub(crate) fn version_compare(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut ai = 0;
    let mut bi = 0;

    loop {
        // Skip non-alphanumeric, non-special characters
        while ai < a.len() && !is_version_char(a[ai]) {
            ai += 1;
        }
        while bi < b.len() && !is_version_char(b[bi]) {
            bi += 1;
        }

        // Tilde: sorts lower than everything (pre-release)
        let a_tilde = ai < a.len() && a[ai] == b'~';
        let b_tilde = bi < b.len() && b[bi] == b'~';
        if a_tilde || b_tilde {
            if !a_tilde {
                return Ordering::Greater;
            }
            if !b_tilde {
                return Ordering::Less;
            }
            ai += 1;
            bi += 1;
            continue;
        }

        // End of string check
        let a_end = ai >= a.len();
        let b_end = bi >= b.len();
        if a_end && b_end {
            return Ordering::Equal;
        }
        if a_end {
            return Ordering::Less;
        }
        if b_end {
            return Ordering::Greater;
        }

        // Dash: sorts lower
        let a_dash = a[ai] == b'-';
        let b_dash = b[bi] == b'-';
        if a_dash || b_dash {
            if !a_dash {
                return Ordering::Greater;
            }
            if !b_dash {
                return Ordering::Less;
            }
            ai += 1;
            bi += 1;
            continue;
        }

        // Caret: sorts lower (post-release)
        let a_caret = a[ai] == b'^';
        let b_caret = b[bi] == b'^';
        if a_caret || b_caret {
            if !a_caret {
                return Ordering::Greater;
            }
            if !b_caret {
                return Ordering::Less;
            }
            ai += 1;
            bi += 1;
            continue;
        }

        // Dot: sorts lower
        let a_dot = a[ai] == b'.';
        let b_dot = b[bi] == b'.';
        if a_dot || b_dot {
            if !a_dot {
                return Ordering::Greater;
            }
            if !b_dot {
                return Ordering::Less;
            }
            ai += 1;
            bi += 1;
            continue;
        }

        // Numeric comparison
        if a[ai].is_ascii_digit() || b[bi].is_ascii_digit() {
            // Skip leading zeros
            while ai < a.len() && a[ai] == b'0' {
                ai += 1;
            }
            while bi < b.len() && b[bi] == b'0' {
                bi += 1;
            }

            // Extract numeric spans
            let a_start = ai;
            let b_start = bi;
            while ai < a.len() && a[ai].is_ascii_digit() {
                ai += 1;
            }
            while bi < b.len() && b[bi].is_ascii_digit() {
                bi += 1;
            }
            let a_len = ai - a_start;
            let b_len = bi - b_start;

            // Longer number is bigger
            if a_len != b_len {
                return a_len.cmp(&b_len);
            }

            // Same length: compare digit-by-digit
            for i in 0..a_len {
                let cmp = a[a_start + i].cmp(&b[b_start + i]);
                if cmp != Ordering::Equal {
                    return cmp;
                }
            }
            continue;
        }

        // Alphabetic comparison (uppercase sorts LOWER than lowercase)
        if a[ai].is_ascii_alphabetic() || b[bi].is_ascii_alphabetic() {
            let a_start = ai;
            let b_start = bi;
            while ai < a.len() && a[ai].is_ascii_alphabetic() {
                ai += 1;
            }
            while bi < b.len() && b[bi].is_ascii_alphabetic() {
                bi += 1;
            }

            let a_span = &a[a_start..ai];
            let b_span = &b[b_start..bi];

            let cmp = a_span.cmp(b_span);
            if cmp != Ordering::Equal {
                return cmp;
            }
            continue;
        }

        // Should not reach here, but advance to prevent infinite loop
        ai += 1;
        bi += 1;
    }
}

/// Check if a byte is a version-relevant character
fn is_version_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'~' || b == b'^' || b == b'-' || b == b'.'
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
        return None; // Already bad, nothing to decrement
    }

    let tries_done = entry.tries_done.unwrap_or(0);
    let new_left = tries_left - 1;
    let new_done = tries_done.saturating_add(1);

    // Preserve digit width for padding (e.g., +03 stays 2 digits)
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

/// Count the number of digits after a marker character in the filename
fn count_digits_in_filename(filename: &str, marker: char) -> usize {
    let name = filename.trim_end_matches(".conf");
    if let Some(pos) = name.rfind(marker) {
        let after = &name[pos + 1..];
        // Count digits until a non-digit (e.g., the '-' separator)
        after.chars().take_while(char::is_ascii_digit).count()
    } else {
        0
    }
}

/// Check if an architecture string matches the current build target
fn is_native_architecture(arch: &str) -> bool {
    let arch_lower = arch.to_lowercase();
    #[cfg(target_arch = "x86_64")]
    {
        arch_lower == "x64" || arch_lower == "x86_64" || arch_lower == "x86-64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_lower == "aa64" || arch_lower == "aarch64" || arch_lower == "arm64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        false
    }
}

/// Case-insensitive file extension check for no_std environments
fn has_extension_ignore_case(filename: &str, ext: &str) -> bool {
    let Some(dot_pos) = filename.rfind('.') else {
        return false;
    };
    filename[dot_pos + 1..].eq_ignore_ascii_case(ext)
}

/// Convert forward slashes to backslashes for EFI path compatibility
/// Remove GRUB `$variable` tokens from a string.
/// Fedora BLS entries contain `$tuned_initrd`, `$grub_users`, etc.
/// that are meaningful to GRUB but not to LamBoot.
fn strip_grub_variables(s: &str) -> String {
    s.split_whitespace()
        .filter(|token| !token.starts_with('$'))
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_path(path: &str) -> String {
    let mut s = path.replace('/', "\\");
    if !s.starts_with('\\') {
        s.insert(0, '\\');
    }
    s
}
