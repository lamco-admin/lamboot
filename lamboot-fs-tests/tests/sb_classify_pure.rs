//! Host unit tests for the Secure Boot read classification + retry.
//!
//! The module lives at `lamboot-core/src/sb_classify_pure.rs` and is included
//! verbatim via `#[path]`. `secure::read_secure_boot_enabled` maps one
//! `runtime::get_variable` call onto `SbReadOutcome` and delegates here, so this
//! is the authoritative test of the fail-closed/retry contract — driven by a
//! stateful closure, no UEFI needed.
//!
//! Dual contract: flexibility — a clean off read, an absent/unsupported
//! variable, and a single transient blip on an SB-off box must all yield "off"
//! so a good unsigned kernel is not wrongly refused; security — an oversized
//! variable (`BufferTooSmall`) and a persistently indeterminate read must fail
//! closed to "on" so a flaky/perturbed read never becomes a free verification
//! skip. Only `0x01` is "on".
//!
//! The fixed-length read sequences pin the read COUNT: the closure panics if
//! `classify_secure_boot` reads more times than the contract allows, so a test
//! that passes also proves "decisive outcomes do not retry" and "indeterminate
//! retries exactly once".

use lamboot_fs_tests::sb_classify_pure::{classify_secure_boot, SbReadOutcome};

/// A closure feeding a fixed sequence of reads; panics if consumed past the end
/// (i.e. if classify_secure_boot performed more reads than the sequence allows).
fn reads(seq: &[SbReadOutcome]) -> impl FnMut() -> SbReadOutcome + '_ {
    let mut it = seq.iter().copied();
    move || {
        it.next()
            .expect("classify_secure_boot read more times than expected")
    }
}

// ---- clean reads decide on the first read (no retry) ----

#[test]
fn clean_value_1_is_on() {
    assert!(classify_secure_boot(reads(&[SbReadOutcome::Value(Some(
        1
    ))])));
}

#[test]
fn clean_value_0_is_off() {
    assert!(!classify_secure_boot(reads(&[SbReadOutcome::Value(Some(
        0
    ))])));
}

#[test]
fn nonspec_byte_is_off() {
    // Pins the exact `== 1` comparison: any clean byte other than 0x01 is off.
    assert!(!classify_secure_boot(reads(&[SbReadOutcome::Value(Some(
        2
    ))])));
}

#[test]
fn not_found_is_off() {
    assert!(!classify_secure_boot(reads(&[SbReadOutcome::NotFound])));
}

#[test]
fn unsupported_is_off() {
    assert!(!classify_secure_boot(reads(&[SbReadOutcome::Unsupported])));
}

#[test]
fn buffer_too_small_is_on_decisively_no_retry() {
    // Length-1 sequence: if classify retried, the closure would be consumed past
    // the end and panic. Passing proves BufferTooSmall decides immediately.
    assert!(classify_secure_boot(reads(&[
        SbReadOutcome::BufferTooSmall
    ])));
}

// ---- indeterminate reads retry exactly once, then honor / fail closed ----

#[test]
fn empty_then_clean_off_retries_to_off() {
    assert!(!classify_secure_boot(reads(&[
        SbReadOutcome::Value(None),
        SbReadOutcome::Value(Some(0)),
    ])));
}

#[test]
fn transient_then_clean_on_retries_to_on() {
    assert!(classify_secure_boot(reads(&[
        SbReadOutcome::TransientError,
        SbReadOutcome::Value(Some(1)),
    ])));
}

#[test]
fn transient_twice_fails_closed_on() {
    // Length-2: the retry is exhausted after the second read; a third read would
    // panic, proving exactly one retry before failing closed (on).
    assert!(classify_secure_boot(reads(&[
        SbReadOutcome::TransientError,
        SbReadOutcome::TransientError,
    ])));
}

#[test]
fn empty_twice_fails_closed_on() {
    // An empty payload is indeterminate, not "off" — two empties fail closed.
    assert!(classify_secure_boot(reads(&[
        SbReadOutcome::Value(None),
        SbReadOutcome::Value(None),
    ])));
}

#[test]
fn buffer_too_small_first_wins_over_poisoned_second() {
    // BufferTooSmall is decisive on read 0; the off-value sentinel on read 1
    // must never be reached (length-1 sequence would panic if it were).
    assert!(classify_secure_boot(reads(&[
        SbReadOutcome::BufferTooSmall
    ])));
}
