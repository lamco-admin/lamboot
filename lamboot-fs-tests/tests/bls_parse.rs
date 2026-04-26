//! SDS-5 PR-2: host-side unit tests for the pure BLS parser.
//!
//! The parser lives at `lamboot-core/src/bls_parse.rs` and is included
//! verbatim into this test crate via `#[path = "..."]` — no duplication,
//! no drift risk. Tests validate the behaviors specified in
//! SPEC-BLS-MULTI-FS §9.1 that can be exercised without a UEFI Volume:
//!
//!   * Parse well-formed Fedora-style, EndeavourOS-style, Pop!_OS-style
//!     .conf files and check every field lands in the expected slot.
//!   * Parse boot-counter filename suffixes (+N / +N-M).
//!   * Reject .conf files lacking both `linux` and `efi`.
//!   * Tolerate GRUB `$variable` tokens (stripped from options+initrd).
//!   * Unknown fields silently ignored.
//!   * UAPI.10 version comparison via `version_compare`.
//!   * BLS sort order via `bls_sort_compare` (bad-last, sort-key groups,
//!     filename fallback).
//!
//! Scan-level behaviors (per-volume trust events, multi-volume entry
//! indexing, policy filter, architecture filter) require a real
//! `Volume` and are covered by the QEMU harness in PR-4. This test
//! crate cannot link against `lamboot-core` (UEFI-only) so those
//! cases are out of scope here by architecture.

use lamboot_fs_tests::bls_parse::{
    bls_sort_compare, count_digits_in_filename, has_extension_ignore_case, is_native_architecture,
    version_compare, BlsEntry, BootCountState,
};

// ---------------------------------------------------------------------------
// 1. Single-entry parse — well-formed Fedora-style .conf
// ---------------------------------------------------------------------------

#[test]
fn parse_fedora_style_entry_populates_every_field() {
    let filename = "4a9f6c93f81e4736982abea00129918b-6.19.12-200.fc43.x86_64.conf";
    let content = "\
title Fedora Linux (6.19.12-200.fc43.x86_64) 43 (Sway Atomic)
version 6.19.12-200.fc43.x86_64
linux /vmlinuz-6.19.12-200.fc43.x86_64
initrd /initramfs-6.19.12-200.fc43.x86_64.img
options root=UUID=4a9f6c93-f81e-4736-982a-bea00129918b ro rootflags=subvol=root rd.luks.uuid=luks-xyz rhgb quiet
machine-id 4a9f6c93f81e4736982abea00129918b
";

    let entry = BlsEntry::parse(filename, content).expect("parse must succeed");

    assert_eq!(
        entry.id,
        "4a9f6c93f81e4736982abea00129918b-6.19.12-200.fc43.x86_64"
    );
    assert_eq!(entry.filename, filename);
    assert_eq!(
        entry.title.as_deref(),
        Some("Fedora Linux (6.19.12-200.fc43.x86_64) 43 (Sway Atomic)")
    );
    assert_eq!(entry.version.as_deref(), Some("6.19.12-200.fc43.x86_64"));
    // normalize_path converts / to \ for EFI path consumption
    assert_eq!(
        entry.linux.as_deref(),
        Some("\\vmlinuz-6.19.12-200.fc43.x86_64")
    );
    assert_eq!(
        entry.initrd,
        vec!["\\initramfs-6.19.12-200.fc43.x86_64.img"]
    );
    assert_eq!(
        entry.machine_id.as_deref(),
        Some("4a9f6c93f81e4736982abea00129918b")
    );
    assert_eq!(entry.options.len(), 1);
    assert!(entry.options[0].contains("root=UUID="));
    assert_eq!(entry.boot_count_state(), BootCountState::Good);
}

// ---------------------------------------------------------------------------
// 2. Multi-value initrd — one line with multiple paths
// ---------------------------------------------------------------------------

#[test]
fn parse_multi_value_initrd_splits_on_whitespace() {
    let content = "\
title Test
linux /vmlinuz
initrd /amd-ucode.img /initramfs.img
";
    let entry = BlsEntry::parse("test.conf", content).unwrap();
    assert_eq!(
        entry.initrd,
        vec!["\\amd-ucode.img", "\\initramfs.img"],
        "initrd with multiple whitespace-separated values must split",
    );
}

// ---------------------------------------------------------------------------
// 3. Multiple `options` lines — concatenate per BLS spec
// ---------------------------------------------------------------------------

#[test]
fn parse_multiple_options_lines_preserved_as_vec() {
    let content = "\
title Test
linux /vmlinuz
options foo=bar
options baz=qux
";
    let entry = BlsEntry::parse("test.conf", content).unwrap();
    assert_eq!(entry.options, vec!["foo=bar", "baz=qux"]);
    assert_eq!(entry.combined_options(), "foo=bar baz=qux");
}

// ---------------------------------------------------------------------------
// 4. GRUB $variables are stripped from combined_options + cleaned_initrd
// ---------------------------------------------------------------------------

#[test]
fn parse_fedora_grub_variables_stripped_from_options_and_initrd() {
    let content = "\
title Fedora
linux /vmlinuz
initrd /initramfs.img $tuned_initrd
options ro $grub_users rhgb quiet
";
    let entry = BlsEntry::parse("fedora.conf", content).unwrap();

    // Raw initrd vec retains the $variable with the normalize_path
    // backslash prefix — that's the raw parse. SDS-5 PR-2 surfaced
    // that v0.8.3's cleaned_initrd only filtered the bare `$` form
    // and missed `\$` (which is what parse actually produces because
    // normalize_path prepends `\`). cleaned_initrd now handles both.
    assert_eq!(entry.initrd, vec!["\\initramfs.img", "\\$tuned_initrd"]);

    // cleaned_initrd drops the variable in both forms.
    assert_eq!(entry.cleaned_initrd(), vec!["\\initramfs.img"]);

    // combined_options drops GRUB $vars.
    let combined = entry.combined_options();
    assert!(combined.contains("ro"));
    assert!(combined.contains("rhgb"));
    assert!(combined.contains("quiet"));
    assert!(
        !combined.contains("$grub_users"),
        "combined_options leaked $grub_users: {combined}"
    );
}

#[test]
fn cleaned_initrd_filters_all_grub_var_forms() {
    // Explicit coverage for the three shapes a $variable can take
    // after parse: bare `$`, `\$` (normalize_path result), `/$` (on
    // systems that keep forward-slash paths, e.g. ext4-sourced).
    let content = "\
title X
linux /vmlinuz
initrd /real1.img $bare /real2.img \\$backslash /$forward
";
    // The parse layer normalizes to backslash, so `$bare` ends up as
    // `\$bare`, `/$forward` ends up as `\$forward`, and the already-
    // backslashed `\$backslash` stays `\$backslash`.
    let entry = BlsEntry::parse("x.conf", content).unwrap();
    let cleaned = entry.cleaned_initrd();
    assert!(cleaned.iter().any(|s| s == "\\real1.img"));
    assert!(cleaned.iter().any(|s| s == "\\real2.img"));
    assert!(
        !cleaned.iter().any(|s| s.contains("bare")),
        "cleaned leaked $bare: {cleaned:?}"
    );
    assert!(
        !cleaned.iter().any(|s| s.contains("backslash")),
        "cleaned leaked \\$backslash: {cleaned:?}"
    );
    assert!(
        !cleaned.iter().any(|s| s.contains("forward")),
        "cleaned leaked /$forward: {cleaned:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Unknown fields silently ignored (spec §4.2)
// ---------------------------------------------------------------------------

#[test]
fn parse_unknown_fields_silently_ignored() {
    let content = "\
title Test
linux /vmlinuz
fuzzbucket hello
devicetree not-yet-supported
";
    let entry =
        BlsEntry::parse("test.conf", content).expect("unknown fields must not reject parse");
    assert_eq!(entry.title.as_deref(), Some("Test"));
    assert_eq!(entry.linux.as_deref(), Some("\\vmlinuz"));
}

// ---------------------------------------------------------------------------
// 6. No linux + no efi → parse returns None
// ---------------------------------------------------------------------------

#[test]
fn parse_rejects_entry_lacking_both_linux_and_efi() {
    let content = "\
title Test
version 1.0
options nothing-actionable
";
    assert!(
        BlsEntry::parse("empty.conf", content).is_none(),
        "spec-required: at least one of linux|efi must be set"
    );
}

// ---------------------------------------------------------------------------
// 7. Comments + blank lines tolerated
// ---------------------------------------------------------------------------

#[test]
fn parse_tolerates_comments_and_blank_lines() {
    let content = "\
# This is a comment
title Test

# Another comment
linux /vmlinuz


options ro
";
    let entry = BlsEntry::parse("test.conf", content).unwrap();
    assert_eq!(entry.title.as_deref(), Some("Test"));
    assert_eq!(entry.options, vec!["ro"]);
}

// ---------------------------------------------------------------------------
// 8. Boot-counter filename suffix parsing — +N form
// ---------------------------------------------------------------------------

#[test]
fn parse_boot_counter_suffix_plus_N() {
    let content = "title X\nlinux /vmlinuz\n";
    let entry = BlsEntry::parse("system+3.conf", content).unwrap();
    assert_eq!(entry.id, "system");
    assert_eq!(entry.tries_left, Some(3));
    assert_eq!(entry.tries_done, Some(0));
    assert_eq!(entry.boot_count_state(), BootCountState::Indeterminate);
}

// ---------------------------------------------------------------------------
// 9. Boot-counter filename suffix parsing — +N-M form
// ---------------------------------------------------------------------------

#[test]
fn parse_boot_counter_suffix_plus_N_M() {
    let content = "title X\nlinux /vmlinuz\n";
    let entry = BlsEntry::parse("system+2-1.conf", content).unwrap();
    assert_eq!(entry.id, "system");
    assert_eq!(entry.tries_left, Some(2));
    assert_eq!(entry.tries_done, Some(1));
    assert_eq!(entry.boot_count_state(), BootCountState::Indeterminate);
}

#[test]
fn parse_boot_counter_tries_left_zero_is_bad() {
    let content = "title X\nlinux /vmlinuz\n";
    let entry = BlsEntry::parse("system+0-3.conf", content).unwrap();
    assert_eq!(entry.tries_left, Some(0));
    assert_eq!(entry.boot_count_state(), BootCountState::Bad);
}

// ---------------------------------------------------------------------------
// 10. count_digits_in_filename preserves zero-padding width
// ---------------------------------------------------------------------------

#[test]
fn count_digits_preserves_padding() {
    assert_eq!(count_digits_in_filename("entry+03-02.conf", '+'), 2);
    assert_eq!(count_digits_in_filename("entry+03-02.conf", '-'), 2);
    assert_eq!(count_digits_in_filename("entry+3.conf", '+'), 1);
    assert_eq!(count_digits_in_filename("entry+3.conf", '-'), 0);
    assert_eq!(count_digits_in_filename("no-marker.conf", '+'), 0);
}

// ---------------------------------------------------------------------------
// 11. UAPI.10 version comparison
// ---------------------------------------------------------------------------

#[test]
fn version_compare_numeric_and_tilde_and_dash_rules() {
    use core::cmp::Ordering;

    // Numeric: longer wins
    assert_eq!(version_compare("1.10", "1.9"), Ordering::Greater);
    // Same-length numeric: digit-by-digit
    assert_eq!(version_compare("1.10", "1.11"), Ordering::Less);
    // Leading zeros stripped: 005 == 5
    assert_eq!(version_compare("005", "5"), Ordering::Equal);
    // Tilde: pre-release sorts before non-tilde
    assert_eq!(version_compare("1.0~rc1", "1.0"), Ordering::Less);
    // Dash: sorts lower when both strings still have chars to compare
    assert_eq!(version_compare("1.0-alpha", "1.0.alpha"), Ordering::Less);
    // Alphabetic: uppercase < lowercase
    assert_eq!(version_compare("1.0A", "1.0a"), Ordering::Less);
    // Equal
    assert_eq!(
        version_compare("6.19.12-200.fc43", "6.19.12-200.fc43"),
        Ordering::Equal
    );
    // End-of-string before special char: longer string wins
    // (spec-ambiguous for caret; the implementation resolves via EOS first)
    assert_eq!(version_compare("1.0^post", "1.0"), Ordering::Greater);
    assert_eq!(version_compare("1.0-beta", "1.0"), Ordering::Greater);
}

// ---------------------------------------------------------------------------
// 12. BLS sort — bad entries last
// ---------------------------------------------------------------------------

#[test]
fn bls_sort_places_bad_entries_last() {
    let good = BlsEntry::parse("good.conf", "title G\nlinux /v\n").unwrap();
    let bad = BlsEntry::parse("bad+0-3.conf", "title B\nlinux /v\n").unwrap();
    let mut v = vec![bad.clone(), good.clone()];
    v.sort_by(bls_sort_compare);
    assert_eq!(v[0].id, "good");
    assert_eq!(v[1].id, "bad");
}

// ---------------------------------------------------------------------------
// 13. BLS sort — sort-key presence wins over absence
// ---------------------------------------------------------------------------

#[test]
fn bls_sort_prefers_entries_with_sort_key() {
    let with_sk = BlsEntry::parse("a.conf", "title A\nsort-key zzzz\nlinux /v\n").unwrap();
    let without_sk = BlsEntry::parse("b.conf", "title B\nlinux /v\n").unwrap();
    let mut v = vec![without_sk, with_sk];
    v.sort_by(bls_sort_compare);
    assert!(
        v[0].sort_key.is_some(),
        "entry with sort-key must come first"
    );
}

// ---------------------------------------------------------------------------
// 14. BLS sort — version descending (newer first) when both have version
// ---------------------------------------------------------------------------

#[test]
fn bls_sort_version_descending_when_both_have_version() {
    // When neither has sort-key, sort falls back to version_compare
    // on the filename-derived ID (descending). For realistic Fedora-
    // shaped filenames like `{machine-id}-{version}.conf`, embedding
    // the version into the ID makes the sort do what operators expect.
    let old = BlsEntry::parse("fedora-6.18.conf", "title Old\nversion 6.18\nlinux /v\n").unwrap();
    let new = BlsEntry::parse("fedora-6.19.conf", "title New\nversion 6.19\nlinux /v\n").unwrap();
    let mut v = vec![old, new];
    v.sort_by(bls_sort_compare);
    assert_eq!(
        v[0].id, "fedora-6.19",
        "newer version in filename must sort first"
    );
}

// ---------------------------------------------------------------------------
// 15. BLS sort — filename ascending when either lacks version (#51 fix)
// ---------------------------------------------------------------------------

#[test]
fn bls_sort_filename_ascending_when_missing_version() {
    // Pop!_OS bug: Pop_OS-current vs Recovery-79EB-58C6. Neither had a
    // version field, so the old code reverse-sorted filename and picked
    // Recovery first. Fix: ascending alphabetical when either lacks version.
    let pop_os = BlsEntry::parse("Pop_OS-current.conf", "title Pop_OS\nlinux /v\n").unwrap();
    let recovery =
        BlsEntry::parse("Recovery-79EB-58C6.conf", "title Recovery\nlinux /v\n").unwrap();
    let mut v = vec![recovery, pop_os];
    v.sort_by(bls_sort_compare);
    assert_eq!(
        v[0].id, "Pop_OS-current",
        "#51 fix: Pop_OS must sort before Recovery"
    );
}

// ---------------------------------------------------------------------------
// 16. has_extension_ignore_case — case-insensitive extension match
// ---------------------------------------------------------------------------

#[test]
fn has_extension_ignore_case_accepts_mixed_case() {
    assert!(has_extension_ignore_case("entry.conf", "conf"));
    assert!(has_extension_ignore_case("entry.CONF", "conf"));
    assert!(has_extension_ignore_case("entry.Conf", "conf"));
    assert!(!has_extension_ignore_case("entry.confx", "conf"));
    assert!(!has_extension_ignore_case("entry", "conf"));
}

// ---------------------------------------------------------------------------
// 17. is_native_architecture — x86_64 variants all match on x86_64 host
// ---------------------------------------------------------------------------

#[test]
fn is_native_architecture_x86_64_variants() {
    // lamboot-fs-tests compiles with target_arch = "x86_64" by default,
    // matching how x86_64 UEFI builds see it.
    assert!(is_native_architecture("x64"));
    assert!(is_native_architecture("x86_64"));
    assert!(is_native_architecture("x86-64"));
    assert!(is_native_architecture("X64")); // case-insensitive
    assert!(!is_native_architecture("aa64"));
    assert!(!is_native_architecture("riscv64"));
}

// ---------------------------------------------------------------------------
// 18. Filename without plus → no boot counter
// ---------------------------------------------------------------------------

#[test]
fn parse_filename_without_plus_has_no_boot_counter() {
    let entry = BlsEntry::parse("plain-entry.conf", "title X\nlinux /vmlinuz\n").unwrap();
    assert_eq!(entry.id, "plain-entry");
    assert_eq!(entry.tries_left, None);
    assert_eq!(entry.tries_done, None);
    assert_eq!(entry.boot_count_state(), BootCountState::Good);
}

// ---------------------------------------------------------------------------
// 19. source_volume_index/tag default to 0/"" at parse time
// ---------------------------------------------------------------------------

#[test]
fn parse_defaults_source_fields_to_zero_and_empty() {
    let entry = BlsEntry::parse("x.conf", "title X\nlinux /v\n").unwrap();
    assert_eq!(entry.source_volume_index, 0);
    assert_eq!(entry.source_backend_tag, "");
    // The real values get set by scan_volume_for_bls before the entry
    // reaches discover_all_entries' output vec. Tested via QEMU in PR-4.
}

// ---------------------------------------------------------------------------
// 20. Equal-by-content check — PR-3 dedup key foundation
// ---------------------------------------------------------------------------

#[test]
fn parsed_entries_with_identical_content_are_equal() {
    let content = "title X\nlinux /vmlinuz\noptions ro\n";
    let a = BlsEntry::parse("x.conf", content).unwrap();
    let b = BlsEntry::parse("x.conf", content).unwrap();
    assert_eq!(
        a, b,
        "identical .conf bytes parse to structurally equal BlsEntry"
    );
}

#[test]
fn parsed_entries_with_different_options_differ() {
    let a = BlsEntry::parse("x.conf", "title X\nlinux /v\noptions ro\n").unwrap();
    let b = BlsEntry::parse("x.conf", "title X\nlinux /v\noptions rw\n").unwrap();
    assert_ne!(
        a, b,
        "different options parse to different BlsEntry — dedup key discriminates"
    );
}

#[test]
fn parsed_entry_equality_ignores_source_volume_fields() {
    // SDS-5 PR-3 dedup key: content-equality must IGNORE source_volume_index
    // and source_backend_tag. Two entries discovered on different volumes
    // with byte-identical .conf contents are considered equal for dedup.
    let content = "title X\nlinux /vmlinuz\noptions ro\n";
    let mut a = BlsEntry::parse("x.conf", content).unwrap();
    let mut b = BlsEntry::parse("x.conf", content).unwrap();

    a.source_volume_index = 0;
    a.source_backend_tag = "fat";
    b.source_volume_index = 1;
    b.source_backend_tag = "ext4-view@0.9.3";

    assert_eq!(
        a, b,
        "content-equality must ignore source_volume_index / source_backend_tag"
    );
}

#[test]
fn parsed_entry_equality_catches_options_change_across_sources() {
    // Spec §5 rule 2: same filename + different parsed content triggers
    // bls_duplicate_filename. Content difference must surface via ==/!=
    // even when only `options` differs and source fields are set.
    let mut a = BlsEntry::parse("x.conf", "title X\nlinux /v\noptions ro\n").unwrap();
    let mut b = BlsEntry::parse("x.conf", "title X\nlinux /v\noptions rw\n").unwrap();

    a.source_volume_index = 0;
    a.source_backend_tag = "fat";
    b.source_volume_index = 1;
    b.source_backend_tag = "ext4-view@0.9.3";

    assert_ne!(
        a, b,
        "content difference must not be masked by source-field differences"
    );
}
