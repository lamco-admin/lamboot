//! Pure BLS Type 1 parser — no UEFI, no Volume, no I/O.
//!
//! This module holds the byte-level parsing logic that can run on any
//! target. It's kept separate from `bls.rs` so host tests
//! (`lamboot-fs-tests`) can include it verbatim via `#[path]` and
//! exercise §9.1 of SPEC-BLS-MULTI-FS without a UEFI toolchain.
//!
//! The Volume-dependent side (scan, boot-counter rename, conversion to
//! the menu's `BootEntry`) lives in `bls.rs`. The split is a pure-
//! function extraction; no behavior change.

use alloc::{string::String, vec::Vec};
use core::cmp::Ordering;

use crate::fs_types::BackendTag;

/// A parsed BLS Type 1 entry.
///
/// `PartialEq` is **manual** (not derived) so it excludes
/// `source_volume_index` and `source_backend_tag`. Two entries are
/// equal when their *content* matches, regardless of which volume
/// each was discovered on — this is the dedup key mandated by
/// SPEC-BLS-MULTI-FS §5 (v1.1 amendment: parsed-entry equality
/// replaces v1.0's SHA-256 content hash).
#[derive(Debug, Clone, Eq)]
pub struct BlsEntry {
    /// Entry ID (filename without `.conf` and boot-count suffix).
    pub id: String,
    /// Full filename (used by boot-counting rename).
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
    /// SDS-5: which volume this entry was discovered on. Index into
    /// the `volumes` slice passed to `discover_all_entries`. Set by
    /// `scan_volume_for_bls` in `bls.rs`.
    pub source_volume_index: usize,
    /// SDS-5: backend tag of the source volume.
    pub source_backend_tag: BackendTag,
}

impl PartialEq for BlsEntry {
    /// Content-equality: every parsed field except the source-volume
    /// back-references. Two entries discovered on different volumes
    /// with identical `.conf` contents are considered equal for the
    /// purpose of dedup (spec §5 rule 1). Same filename + different
    /// content triggers rule 2 (both kept + event) — the dedup
    /// orchestrator compares `filename` and then calls this for the
    /// content discriminator.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.filename == other.filename
            && self.title == other.title
            && self.version == other.version
            && self.machine_id == other.machine_id
            && self.sort_key == other.sort_key
            && self.linux == other.linux
            && self.initrd == other.initrd
            && self.efi == other.efi
            && self.options == other.options
            && self.architecture == other.architecture
            && self.tries_left == other.tries_left
            && self.tries_done == other.tries_done
        // source_volume_index + source_backend_tag intentionally excluded
    }
}

/// Boot-count state derived from `tries_left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootCountState {
    /// No counters — entry is known good.
    Good,
    /// `tries_left > 0` — being assessed.
    Indeterminate,
    /// `tries_left == 0` — entry failed assessment.
    Bad,
}

impl BlsEntry {
    /// Parse a BLS entry from a `.conf` filename + contents.
    ///
    /// Returns `None` if neither `linux` nor `efi` is set (spec-required).
    /// Unknown fields are silently ignored per spec.
    pub fn parse(filename: &str, content: &str) -> Option<Self> {
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
            // Source fields defaulted here; `scan_volume_for_bls` fills
            // them with the real volume index + backend tag.
            source_volume_index: 0,
            source_backend_tag: "",
        };

        // Parse boot-counting suffix from filename:
        //   <base>+<tries_left>[-<tries_done>].conf
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

        // Parse fields.
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Split on first whitespace.
            let Some(pos) = line.find(|c: char| c.is_ascii_whitespace()) else {
                continue;
            };
            let key = &line[..pos];
            let value = line[pos..].trim_start();

            match key {
                "title" => entry.title = Some(String::from(value)),
                "version" => entry.version = Some(String::from(value)),
                "machine-id" => entry.machine_id = Some(String::from(value)),
                "sort-key" => entry.sort_key = Some(String::from(value)),
                "linux" => entry.linux = Some(normalize_path(value)),
                "efi" => entry.efi = Some(normalize_path(value)),
                "initrd" => {
                    // `initrd` may list multiple space-separated values on one line.
                    for part in value.split_whitespace() {
                        entry.initrd.push(normalize_path(part));
                    }
                }
                "options" => entry.options.push(String::from(value)),
                "architecture" => entry.architecture = Some(String::from(value)),
                _ => {} // Unknown keys silently ignored per spec.
            }
        }

        // Validation: at least `linux` or `efi` must be present.
        if entry.linux.is_none() && entry.efi.is_none() {
            return None;
        }

        Some(entry)
    }

    /// Get the boot-count state.
    pub fn boot_count_state(&self) -> BootCountState {
        match self.tries_left {
            None => BootCountState::Good,
            Some(0) => BootCountState::Bad,
            Some(_) => BootCountState::Indeterminate,
        }
    }

    /// Get combined options as a single string, stripping GRUB
    /// `$variable` tokens (Fedora entries contain `$tuned_initrd` etc.
    /// that are meaningful to GRUB but not LamBoot).
    pub fn combined_options(&self) -> String {
        strip_grub_variables(&self.options.join(" "))
    }

    /// Get initrd paths with GRUB `$variable` tokens removed.
    /// Tokens can appear as bare `$tuned_initrd` or — after
    /// `normalize_path` has prefixed a backslash — as `\$tuned_initrd`.
    /// Both forms drop out. Surfaced by SDS-5 PR-2 host tests
    /// (pre-existing v0.8.3 bug where only the bare form was filtered).
    pub fn cleaned_initrd(&self) -> Vec<String> {
        self.initrd
            .iter()
            .filter(|s| !s.starts_with('$') && !s.starts_with("\\$") && !s.starts_with("/$"))
            .cloned()
            .collect()
    }
}

/// BLS sort comparison per specification:
///   1. Bad entries (`tries_left == 0`) sort last.
///   2. Entries with `sort-key` sort before those without.
///   3. Compare `sort-key`, then `machine-id`, then `version` (descending).
///   4. Filename fallback (descending version order when both have version).
pub fn bls_sort_compare(a: &BlsEntry, b: &BlsEntry) -> Ordering {
    // Bad entries last.
    let a_bad = a.boot_count_state() == BootCountState::Bad;
    let b_bad = b.boot_count_state() == BootCountState::Bad;
    if a_bad != b_bad {
        return if a_bad {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }

    // sort-key presence: entries WITH sort-key come first.
    let a_has_sk = a.sort_key.is_some();
    let b_has_sk = b.sort_key.is_some();
    if a_has_sk != b_has_sk {
        return if a_has_sk {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    // Both have sort-key: compare sort-key → machine-id → version (desc).
    if a_has_sk && b_has_sk {
        let sk_cmp = cmp_opt_str(a.sort_key.as_ref(), b.sort_key.as_ref());
        if sk_cmp != Ordering::Equal {
            return sk_cmp;
        }

        let mid_cmp = cmp_opt_str(a.machine_id.as_ref(), b.machine_id.as_ref());
        if mid_cmp != Ordering::Equal {
            return mid_cmp;
        }

        // Version: descending (newer first).
        let ver_cmp = version_compare(
            a.version.as_deref().unwrap_or(""),
            b.version.as_deref().unwrap_or(""),
        );
        if ver_cmp != Ordering::Equal {
            return ver_cmp.reverse();
        }
    }

    // Filename fallback.
    // If BOTH entries have version metadata, treat the filename as a
    // version-like identifier and sort descending (newer version-like
    // names first). If either is missing a version, there's nothing
    // version-y to reverse — use plain ascending alphabetical order
    // so predictable names like "Pop_OS-current" beat
    // "Recovery-79EB-58C6". Reversing in the no-version case produced
    // the Pop!_OS recovery-as-default bug on v0.8.3 (task #51).
    if a.version.is_some() && b.version.is_some() {
        version_compare(&a.id, &b.id).reverse()
    } else {
        a.id.cmp(&b.id)
    }
}

/// Compare optional strings (`None` sorts after `Some`).
fn cmp_opt_str(a: Option<&String>, b: Option<&String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => a.cmp(b),
    }
}

/// UAPI.10 version-format comparison.
///
/// Compares two version strings according to the UAPI Group
/// specification. Used for BLS entry sorting and boot counting.
pub fn version_compare(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut ai = 0;
    let mut bi = 0;

    loop {
        // Skip non-alphanumeric, non-special characters.
        while ai < a.len() && !is_version_char(a[ai]) {
            ai += 1;
        }
        while bi < b.len() && !is_version_char(b[bi]) {
            bi += 1;
        }

        // Tilde: sorts lower than everything (pre-release).
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

        // End of string check.
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

        // Dash: sorts lower.
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

        // Caret: sorts lower (post-release).
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

        // Dot: sorts lower.
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

        // Numeric comparison.
        if a[ai].is_ascii_digit() || b[bi].is_ascii_digit() {
            // Skip leading zeros.
            while ai < a.len() && a[ai] == b'0' {
                ai += 1;
            }
            while bi < b.len() && b[bi] == b'0' {
                bi += 1;
            }

            // Extract numeric spans.
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

            // Longer number is bigger.
            if a_len != b_len {
                return a_len.cmp(&b_len);
            }

            // Same length: compare digit-by-digit.
            for i in 0..a_len {
                let cmp = a[a_start + i].cmp(&b[b_start + i]);
                if cmp != Ordering::Equal {
                    return cmp;
                }
            }
            continue;
        }

        // Alphabetic comparison (uppercase sorts LOWER than lowercase).
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

        // Should not reach here, but advance to prevent an infinite loop.
        ai += 1;
        bi += 1;
    }
}

/// Check if a byte is a version-relevant character.
fn is_version_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'~' || b == b'^' || b == b'-' || b == b'.'
}

/// Count digits after a marker character in the filename (preserves
/// zero-padding width when generating the next boot-counter name).
pub fn count_digits_in_filename(filename: &str, marker: char) -> usize {
    let name = filename.trim_end_matches(".conf");
    if let Some(pos) = name.rfind(marker) {
        let after = &name[pos + 1..];
        // Count digits until a non-digit (e.g., the '-' separator).
        after.chars().take_while(char::is_ascii_digit).count()
    } else {
        0
    }
}

/// Check if a BLS `architecture` field matches the current build target.
pub fn is_native_architecture(arch: &str) -> bool {
    let arch_lower = arch.to_lowercase();
    #[cfg(target_arch = "x86_64")]
    {
        arch_lower == "x64" || arch_lower == "x86_64" || arch_lower == "x86-64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_lower == "aa64" || arch_lower == "aarch64" || arch_lower == "arm64"
    }
    // Host test builds (x86_64 on Linux) see target_arch = "x86_64",
    // so the first arm above covers the lamboot-fs-tests use case.
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = arch_lower;
        false
    }
}

/// Case-insensitive file-extension check for `no_std` environments.
pub fn has_extension_ignore_case(filename: &str, ext: &str) -> bool {
    let Some(dot_pos) = filename.rfind('.') else {
        return false;
    };
    filename[dot_pos + 1..].eq_ignore_ascii_case(ext)
}

/// Remove GRUB `$variable` tokens from a string. Fedora BLS entries
/// contain `$tuned_initrd`, `$grub_users`, etc. that are meaningful
/// to GRUB but not to LamBoot.
fn strip_grub_variables(s: &str) -> String {
    s.split_whitespace()
        .filter(|token| !token.starts_with('$'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert forward slashes to backslashes for EFI path compatibility.
fn normalize_path(path: &str) -> String {
    let mut s = path.replace('/', "\\");
    if !s.starts_with('\\') {
        s.insert(0, '\\');
    }
    s
}
