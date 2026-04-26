//! Host-side unit tests for the pure helpers in
//! `lamboot-core/src/discovery_pure.rs`. The Volume-dependent scanner
//! (`discover_systemd_boot_dir_style`) is exercised by the QEMU /
//! fleet harness on VM 124 (Pop!_OS) — see
//! `docs/POPOS-AUTO-DISCOVERY-FINDINGS-2026-04-25.md` for the original
//! VM 124 finding and `docs/analysis/POPOS-UPSTREAM-DEEP-DIVE-2026-04-25.md`
//! for the strategic context.
//!
//! The pure helpers cover the *recognizer* and *renderer* layers:
//!   * `looks_like_distro_uuid_dir` — true/false on directory-name shapes
//!   * `pretty_name_from_distro_uuid_dir` — display rendering
//!   * `forward_to_backslash` — path-style conversion

use lamboot_fs_tests::discovery_pure::{
    forward_to_backslash, looks_like_distro_uuid_dir, pretty_name_from_distro_uuid_dir,
};

// ---------------------------------------------------------------------------
// looks_like_distro_uuid_dir — accept the cases we MUST recognize
// ---------------------------------------------------------------------------

#[test]
fn pop_os_full_uuid_layout_is_recognized() {
    // Pop!_OS canonical kernelstub layout: \EFI\Pop_OS-<root-fs-uuid>\
    assert!(looks_like_distro_uuid_dir(
        "Pop_OS-fc9ac3db-cac4-48bd-b437-cd47301f1c7f"
    ));
}

#[test]
fn pop_os_recovery_short_uuid_is_recognized() {
    // Pop!_OS Recovery uses a shorter ID: \EFI\Recovery-<short-id>\
    assert!(looks_like_distro_uuid_dir("Recovery-79EB-58C6"));
}

#[test]
fn garuda_style_full_uuid_recognized() {
    // Garuda variants and custom systemd-boot installs use the same
    // <name>-<uuid> shape — the scanner must pick them up generically,
    // not via a hard-coded distro list.
    assert!(looks_like_distro_uuid_dir(
        "Garuda-aabbccdd-eeff-0011-2233-445566778899"
    ));
}

#[test]
fn lowercase_distro_name_is_recognized() {
    assert!(looks_like_distro_uuid_dir(
        "ubuntu-12345678-90ab-cdef-1234-567890abcdef"
    ));
}

#[test]
fn underscore_in_distro_name_is_recognized() {
    // System76's `Pop_OS` uses underscore — must be permitted in
    // the name portion (before the dash).
    assert!(looks_like_distro_uuid_dir("My_Distro-abcd1234"));
}

// ---------------------------------------------------------------------------
// looks_like_distro_uuid_dir — reject the cases we MUST NOT pick up
// ---------------------------------------------------------------------------

#[test]
fn linux_dir_rejected() {
    // /EFI/Linux/ is the UKI scanner's domain, not ours
    assert!(!looks_like_distro_uuid_dir("Linux"));
}

#[test]
fn boot_dir_rejected() {
    // /EFI/BOOT/ is the firmware-fallback synthetic
    assert!(!looks_like_distro_uuid_dir("BOOT"));
}

#[test]
fn fedora_dir_rejected_no_dash() {
    // /EFI/fedora/ has no dash → not our pattern
    assert!(!looks_like_distro_uuid_dir("fedora"));
}

#[test]
fn lamboot_dir_rejected_no_dash() {
    // /EFI/LamBoot/ — our own canonical install — must never be
    // mistaken for a kernel directory.
    assert!(!looks_like_distro_uuid_dir("LamBoot"));
}

#[test]
fn systemd_dir_rejected_no_dash() {
    // /EFI/systemd/ — sd-boot binary lives here, not a kernel dir.
    assert!(!looks_like_distro_uuid_dir("systemd"));
}

#[test]
fn microsoft_dir_rejected_no_dash() {
    // /EFI/Microsoft/ — Windows Boot Manager dir.
    assert!(!looks_like_distro_uuid_dir("Microsoft"));
}

#[test]
fn dash_prefix_rejected() {
    // Empty distro name on the left of the dash.
    assert!(!looks_like_distro_uuid_dir("-fc9ac3db"));
}

#[test]
fn suffix_too_short_rejected() {
    // Suffix has fewer than 4 hex digits.
    assert!(!looks_like_distro_uuid_dir("foo-aa"));
}

#[test]
fn non_hex_suffix_rejected() {
    // Suffix contains non-hex letters → not UUID-shaped.
    assert!(!looks_like_distro_uuid_dir("foo-zzzzzzzz"));
}

#[test]
fn name_starting_with_digit_rejected() {
    // Distro names start with a letter.
    assert!(!looks_like_distro_uuid_dir(
        "1Pop_OS-fc9ac3db-cac4-48bd-b437-cd47301f1c7f"
    ));
}

#[test]
fn name_with_dot_rejected() {
    // Dots aren't allowed in the name (would be unusual + ambiguous).
    assert!(!looks_like_distro_uuid_dir("foo.bar-aabbccdd"));
}

#[test]
fn empty_string_rejected() {
    assert!(!looks_like_distro_uuid_dir(""));
}

// ---------------------------------------------------------------------------
// pretty_name_from_distro_uuid_dir — display rendering
// ---------------------------------------------------------------------------

#[test]
fn pop_os_renders_with_bang() {
    // System76 escapes `!` as `_` in the directory name; we render
    // the user-facing display correctly.
    assert_eq!(
        pretty_name_from_distro_uuid_dir("Pop_OS-fc9ac3db-cac4-48bd-b437-cd47301f1c7f"),
        "Pop!_OS"
    );
}

#[test]
fn recovery_renders_plain() {
    assert_eq!(
        pretty_name_from_distro_uuid_dir("Recovery-79EB-58C6"),
        "Recovery"
    );
}

#[test]
fn garuda_renders_plain() {
    assert_eq!(
        pretty_name_from_distro_uuid_dir("Garuda-aabbccdd-eeff-0011-2233-445566778899"),
        "Garuda"
    );
}

#[test]
fn ubuntu_renders_plain() {
    assert_eq!(
        pretty_name_from_distro_uuid_dir("ubuntu-12345678-90ab-cdef-1234-567890abcdef"),
        "ubuntu"
    );
}

// ---------------------------------------------------------------------------
// forward_to_backslash — path-style conversion
// ---------------------------------------------------------------------------

#[test]
fn forward_to_backslash_basic() {
    assert_eq!(
        forward_to_backslash("/EFI/Pop_OS-foo/vmlinuz.efi"),
        "\\EFI\\Pop_OS-foo\\vmlinuz.efi"
    );
}

#[test]
fn forward_to_backslash_already_backslash() {
    // Idempotent on backslash input.
    assert_eq!(forward_to_backslash("\\EFI\\foo\\bar"), "\\EFI\\foo\\bar");
}

#[test]
fn forward_to_backslash_empty() {
    assert_eq!(forward_to_backslash(""), "");
}
