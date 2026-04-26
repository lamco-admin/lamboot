//! SDS-4 PR-4 — host unit tests for the pure trust-log logic.
//!
//! The module lives at `lamboot-core/src/trust_log_pure.rs` and is included
//! verbatim into this test crate via `#[path]`. Coverage:
//!
//!   * §6.1 event schema: JSON field order is stable across events, all
//!     mandatory fields appear, optional fields (`verifier_tag`, `note`)
//!     are elided when empty, JSON-special characters in strings are
//!     escaped correctly.
//!   * §6.2 event ordering: sequence numbers start at 0 and monotonically
//!     increase across record() + flush boundaries; accumulation across
//!     multiple serialize_merged() calls preserves earlier events.
//!   * §6.3 stable vocabulary: every constant is listed in
//!     `ALL_VERIFIED_VIA`; token strings are exactly the documented
//!     values (renames would be a major-version break).
//!   * §6.4 invariant substrate: the schema exposes a sha256 field that
//!     carries through the builder-pattern.
//!
//! The Volume-dependent flush-to-ESP path (which serializes via the same
//! pure helpers then invokes `EspWriter`) is out of scope for host tests
//! and is exercised by the QEMU harness in `run-qemu-ext4-backend-test.sh`.

use lamboot_fs_tests::trust_log_pure::{
    serialize_events, TrustEvent, TrustLog, ALL_VERIFIED_VIA, V_DEGRADED_TRUST_SB_DIRECT,
    V_DEGRADED_TRUST_SB_OFF, V_FIRMWARE_DB_FALLBACK, V_FIRMWARE_LOADIMAGE, V_NATIVE_PE_LOADER,
    V_SHIM_MOK, V_SHIM_REJECTED,
};

// ---------------------------------------------------------------------------
// §6.3 stable vocabulary tokens
//
// Renaming or removing one of these values is a major-version trust-log
// break per SDS-4 §6.3. Pin the exact strings here so a refactor can't
// silently drift them.
// ---------------------------------------------------------------------------

#[test]
fn vocabulary_tokens_are_exact_strings() {
    assert_eq!(V_SHIM_MOK, "shim_mok");
    assert_eq!(V_FIRMWARE_DB_FALLBACK, "firmware_db_fallback");
    assert_eq!(V_DEGRADED_TRUST_SB_OFF, "degraded_trust_sb_off");
    assert_eq!(V_DEGRADED_TRUST_SB_DIRECT, "degraded_trust_sb_direct");
    assert_eq!(V_SHIM_REJECTED, "shim_rejected");
    assert_eq!(V_NATIVE_PE_LOADER, "native_pe_loader");
    assert_eq!(V_FIRMWARE_LOADIMAGE, "firmware_loadimage");
}

#[test]
fn all_verified_via_lists_every_constant() {
    // Every individual V_* constant must appear in the catalog. Adding a
    // new constant means updating ALL_VERIFIED_VIA; this test catches the
    // omission.
    let catalog: Vec<&str> = ALL_VERIFIED_VIA.iter().copied().collect();
    for expected in [
        V_SHIM_MOK,
        V_FIRMWARE_DB_FALLBACK,
        V_DEGRADED_TRUST_SB_OFF,
        V_DEGRADED_TRUST_SB_DIRECT,
        V_SHIM_REJECTED,
        V_NATIVE_PE_LOADER,
        V_FIRMWARE_LOADIMAGE,
    ] {
        assert!(
            catalog.contains(&expected),
            "ALL_VERIFIED_VIA missing {expected}"
        );
    }
}

#[test]
fn vocabulary_tokens_are_unique() {
    // If two constants ever collide, audit consumers matching on the
    // token string can't tell which path the event came from.
    let mut seen: Vec<&str> = Vec::new();
    for token in ALL_VERIFIED_VIA {
        assert!(
            !seen.contains(token),
            "duplicate verified_via token: {token}"
        );
        seen.push(token);
    }
}

#[test]
fn vocabulary_tokens_are_non_empty_and_snake_case() {
    // Discipline: empty strings or whitespace would be a footgun for
    // downstream matchers. Snake-case is the documented convention.
    for token in ALL_VERIFIED_VIA {
        assert!(!token.is_empty(), "empty vocabulary token");
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "non-snake_case token: {token}"
        );
    }
}

// ---------------------------------------------------------------------------
// §6.1 JSON schema — field order + mandatory fields
// ---------------------------------------------------------------------------

// A stand-in for production's `uefi::Status` — its Debug impl produces a
// bare token without quotes (e.g. `SUCCESS` or `ACCESS_DENIED`). The
// TrustEvent::with_status builder takes `impl Debug` and writes
// `format!("{s:?}")`; passing a plain `&str` would add extra quotes (the
// string's own Debug representation), which is not how callers actually
// use it.
#[derive(Debug)]
#[allow(non_camel_case_types)]
enum FakeStatus {
    SUCCESS,
}

#[test]
fn full_event_emits_all_mandatory_fields_in_order() {
    // Minimal: the documented order is seq, event, path, size, sha256,
    // verified_via, status. Field presence is mandatory for each; their
    // appearance order is part of the contract consumers rely on for
    // line-diff-based reference testing.
    let ev = TrustEvent::new("image_verified")
        .with_path("/EFI/Linux/fedora.efi")
        .with_size(42)
        .with_sha256("ab01")
        .with_verified_via(V_SHIM_MOK)
        .with_status(FakeStatus::SUCCESS);
    let json = serialize_events(std::slice::from_ref(&ev), 0);

    // Strip the trailing newline for easier positional assertions.
    let line = json.trim_end();

    let expected_order = [
        r#""seq":"#,
        r#""event":"image_verified""#,
        r#""path":"/EFI/Linux/fedora.efi""#,
        r#""size":42"#,
        r#""sha256":"ab01""#,
        r#""verified_via":"shim_mok""#,
        r#""status":"SUCCESS""#,
    ];
    let mut cursor = 0;
    for token in expected_order {
        let idx = line[cursor..]
            .find(token)
            .unwrap_or_else(|| panic!("token {token} not found after position {cursor} in {line}"));
        cursor += idx + token.len();
    }
}

#[test]
fn optional_fields_are_elided_when_empty() {
    // `verifier_tag` and `note` are emitted only when non-empty so that
    // the log stays compact for events that don't carry them.
    let ev = TrustEvent::new("boot_start").with_verified_via(V_SHIM_MOK);
    let json = serialize_events(std::slice::from_ref(&ev), 0);
    assert!(!json.contains("verifier_tag"), "empty verifier_tag leaked");
    assert!(!json.contains(r#""note""#), "empty note leaked");
}

#[test]
fn optional_fields_are_included_when_set() {
    let ev = TrustEvent::new("image_verified")
        .with_verified_via(V_SHIM_MOK)
        .with_verifier_tag("shim-15.8")
        .with_note("MOK chain");
    let json = serialize_events(std::slice::from_ref(&ev), 0);
    assert!(json.contains(r#""verifier_tag":"shim-15.8""#));
    assert!(json.contains(r#""note":"MOK chain""#));

    // Schema position: verifier_tag precedes note when both are present.
    let vt = json.find("verifier_tag").expect("verifier_tag absent");
    let note = json.find(r#""note""#).expect("note absent");
    assert!(vt < note, "verifier_tag must appear before note");
}

#[test]
fn each_event_is_a_single_json_line() {
    let events = vec![
        TrustEvent::new("boot_start").with_verified_via(V_SHIM_MOK),
        TrustEvent::new("image_verified").with_verified_via(V_SHIM_MOK),
    ];
    let json = serialize_events(&events, 0);
    // Count newlines — one per event, each ending with \n.
    let newlines = json.chars().filter(|c| *c == '\n').count();
    assert_eq!(newlines, events.len());
    // No bare '\n' inside a JSON line (we escape them in push_kv).
    for line in json.lines() {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "malformed line: {line}"
        );
    }
}

#[test]
fn json_special_characters_in_path_are_escaped() {
    // A path containing a quote or backslash must be JSON-escaped so
    // that a downstream consumer's json.loads() doesn't throw.
    let ev = TrustEvent::new("boot_attempt")
        .with_path(r#"C:\weird"path\with"quotes"#)
        .with_verified_via(V_SHIM_MOK);
    let json = serialize_events(std::slice::from_ref(&ev), 0);

    // The literal raw string must not appear — it contains unescaped
    // quotes that would break the JSON. Instead the escaped form does.
    assert!(json.contains(r#"C:\\weird\"path\\with\"quotes"#));
}

#[test]
fn control_characters_are_unicode_escaped() {
    // Field values with embedded newlines or control bytes land via
    // \uXXXX so a line-oriented parser can still split reliably.
    let ev = TrustEvent::new("boot_attempt")
        .with_note("line1\nline2\x07bell")
        .with_verified_via(V_SHIM_MOK);
    let json = serialize_events(std::slice::from_ref(&ev), 0);
    let line = json.trim_end();
    assert!(line.contains(r"line1\nline2"));
    assert!(line.contains(r"\u0007"), "bell byte not unicode-escaped");
    // And there's still exactly one physical line.
    assert_eq!(json.chars().filter(|c| *c == '\n').count(), 1);
}

// ---------------------------------------------------------------------------
// §6.2 event ordering — sequence numbers + accumulation across flushes
// ---------------------------------------------------------------------------

#[test]
fn sequence_numbers_start_at_zero_and_increase() {
    let mut log = TrustLog::new();
    log.record(TrustEvent::new("boot_start").with_verified_via(V_SHIM_MOK));
    log.record(TrustEvent::new("driver_loaded").with_verified_via(V_SHIM_MOK));
    log.record(TrustEvent::new("image_verified").with_verified_via(V_SHIM_MOK));
    let json = log.serialize_merged();
    // Expect seq:0, seq:1, seq:2 appearing in order.
    let i0 = json.find(r#""seq":0"#).expect("seq 0 missing");
    let i1 = json.find(r#""seq":1"#).expect("seq 1 missing");
    let i2 = json.find(r#""seq":2"#).expect("seq 2 missing");
    assert!(i0 < i1 && i1 < i2, "sequence out of order");
}

#[test]
fn accumulation_preserves_events_across_flushes() {
    // The accumulator is designed so that if flush() is called multiple
    // times, each call rewrites the cumulative log from the start (the
    // ESP writer overwrites; append-at-EOF is Path G). That means an
    // earlier event must still appear in a later serialization.
    let mut log = TrustLog::new();
    log.record(TrustEvent::new("boot_start").with_verified_via(V_SHIM_MOK));
    let first = log.serialize_merged();
    assert!(first.contains("boot_start"));

    log.record(TrustEvent::new("image_verified").with_verified_via(V_SHIM_MOK));
    let second = log.serialize_merged();
    assert!(
        second.contains("boot_start"),
        "earlier event lost after flush"
    );
    assert!(second.contains("image_verified"));

    // Second serialization must have both seq 0 and seq 1.
    assert!(second.contains(r#""seq":0"#));
    assert!(second.contains(r#""seq":1"#));
}

#[test]
fn pending_and_committed_views_are_disjoint_after_flush() {
    let mut log = TrustLog::new();
    log.record(TrustEvent::new("boot_start").with_verified_via(V_SHIM_MOK));
    assert_eq!(log.pending_events().len(), 1);
    assert_eq!(log.committed_events().len(), 0);

    let _ = log.serialize_merged();
    assert_eq!(log.pending_events().len(), 0);
    assert_eq!(log.committed_events().len(), 1);

    log.record(TrustEvent::new("image_verified").with_verified_via(V_SHIM_MOK));
    assert_eq!(log.pending_events().len(), 1);
    assert_eq!(log.committed_events().len(), 1);
}

#[test]
fn empty_log_serializes_to_empty_string() {
    let log = TrustLog::new();
    let json = serialize_events(log.committed_events(), 0);
    assert!(json.is_empty(), "empty log must not emit anything");
}

#[test]
fn image_verified_precedes_image_loaded_native_in_recorded_order() {
    // SDS-4 §7.2 Step 10 (image_verified) must always precede Step 13
    // (image_loaded_native) in the trust log. Callers that record in the
    // wrong order would break the §6.4 audit invariant from a log
    // consumer's perspective.
    let mut log = TrustLog::new();
    log.record(
        TrustEvent::new("image_verified")
            .with_sha256("deadbeef")
            .with_verified_via(V_SHIM_MOK),
    );
    log.record(
        TrustEvent::new("image_loaded_native")
            .with_sha256("deadbeef")
            .with_verified_via(V_NATIVE_PE_LOADER),
    );
    let json = log.serialize_merged();
    let iv = json
        .find(r#""event":"image_verified""#)
        .expect("image_verified missing");
    let il = json
        .find(r#""event":"image_loaded_native""#)
        .expect("image_loaded_native missing");
    assert!(iv < il, "image_verified must precede image_loaded_native");
}

#[test]
fn sha256_round_trips_from_verified_to_loaded() {
    // The §6.4 invariant is that verify and load carry the same digest.
    // The schema supports this (both events emit a sha256 field) and the
    // pure layer preserves the string verbatim. Runtime enforcement is
    // in boot.rs (`assert_eq!` against pe_loader.sha256()); this test
    // only checks the log-layer substrate.
    let digest_hex = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    let events = vec![
        TrustEvent::new("image_verified")
            .with_sha256(digest_hex)
            .with_verified_via(V_SHIM_MOK),
        TrustEvent::new("image_loaded_native")
            .with_sha256(digest_hex)
            .with_verified_via(V_NATIVE_PE_LOADER),
    ];
    let json = serialize_events(&events, 0);
    // The digest must appear exactly twice — once per event.
    let occurrences = json.matches(digest_hex).count();
    assert_eq!(occurrences, 2, "sha256 must appear in both events");
}

// ---------------------------------------------------------------------------
// Builder-pattern behavior (defensive regression tests)
// ---------------------------------------------------------------------------

#[test]
fn default_trust_event_has_empty_fields() {
    let ev = TrustEvent::new("whatever");
    assert_eq!(ev.event, "whatever");
    assert_eq!(ev.path, "");
    assert_eq!(ev.size, 0);
    assert_eq!(ev.sha256_hex, "");
    assert_eq!(ev.verified_via, "");
    assert_eq!(ev.status, "");
    assert_eq!(ev.note, "");
    assert_eq!(ev.verifier_tag, "");
}

#[test]
fn builders_are_independent() {
    // Each `with_*` call overwrites only its own field; the rest stay
    // at their previous value.
    let ev = TrustEvent::new("x")
        .with_path("a")
        .with_note("b")
        .with_verifier_tag("c")
        .with_sha256("d")
        .with_verified_via(V_SHIM_MOK)
        .with_size(99);
    assert_eq!(ev.path, "a");
    assert_eq!(ev.note, "b");
    assert_eq!(ev.verifier_tag, "c");
    assert_eq!(ev.sha256_hex, "d");
    assert_eq!(ev.verified_via, V_SHIM_MOK);
    assert_eq!(ev.size, 99);
}

#[test]
fn default_trust_log_is_empty_and_accumulator_ready() {
    let log = TrustLog::default();
    assert!(log.pending_events().is_empty());
    assert!(log.committed_events().is_empty());
}
