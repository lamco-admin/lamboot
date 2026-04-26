//! Pure helpers for discovery — host-testable, no UEFI dependencies.
//!
//! Lives split from `discovery.rs` so it can be `#[path]`-included into
//! `lamboot-fs-tests` for unit testing without dragging in `Volume`,
//! `Policy`, `TrustLog`, or `BackendTag`. Same authoritative-source
//! pattern as `bls_parse.rs`, `pe_loader_pure.rs`, `trust_log_pure.rs`.
//!
//! This module is `no_std`-clean. It depends only on `alloc::string::*`.

extern crate alloc;
use alloc::string::{String, ToString};

/// Recognize directory names of the shape `<distro>-<uuid-ish>` used by
/// the systemd-boot discoverable-EFI pattern (Pop!_OS kernelstub layout
/// + Pop!_OS Recovery + Garuda variants + custom systemd-boot installs).
///
/// `Pop_OS-fc9ac3db-cac4-48bd-b437-cd47301f1c7f` → true
/// `Recovery-79EB-58C6` → true
/// `Linux` / `BOOT` / `fedora` / `systemd` → false (no dash)
/// `LamBoot` / `Microsoft` → false (no dash)
/// `foo-bar` → false (suffix not hex-shaped)
/// `-prefix-only` → false (empty distro name)
/// `name-zzz` → false (suffix not hex)
pub fn looks_like_distro_uuid_dir(dirname: &str) -> bool {
    let Some(dash_pos) = dirname.find('-') else {
        return false;
    };
    if dash_pos == 0 {
        return false;
    }
    let (name, suffix) = (&dirname[..dash_pos], &dirname[dash_pos + 1..]);
    if name.is_empty() || suffix.len() < 4 {
        return false;
    }
    let Some(first) = name.chars().next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    // Suffix: hex digits + dashes only. At least 4 hex digits total.
    if !suffix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    suffix.chars().filter(char::is_ascii_hexdigit).count() >= 4
}

/// Render a user-facing display name from a distro-UUID directory.
///
/// `Pop_OS-fc9ac3db-…` → `Pop!_OS` (System76 escapes the `!` as `_`)
/// `Recovery-79EB-58C6` → `Recovery`
/// `Garuda-aabbccdd-…` → `Garuda`
/// `name-only-no-dash` → `name-only-no-dash` (degenerate; caller should
///   only invoke after `looks_like_distro_uuid_dir` returns true)
pub fn pretty_name_from_distro_uuid_dir(dirname: &str) -> String {
    let prefix = dirname.find('-').map_or(dirname, |i| &dirname[..i]);
    if prefix == "Pop_OS" {
        return String::from("Pop!_OS");
    }
    prefix.to_string()
}

/// Convert forward-slash path to UEFI backslash path. Used when emitting
/// `EntryKind` paths (which are firmware-style backslashes everywhere
/// else in the bootloader's discovery layer).
pub fn forward_to_backslash(path: &str) -> String {
    path.replace('/', "\\")
}
