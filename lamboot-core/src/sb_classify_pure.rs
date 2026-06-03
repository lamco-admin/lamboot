//! Layer: 0 (pure) — Secure Boot read classification.
//!
//! The error-classification + retry core of `secure::read_secure_boot_enabled`,
//! split out so `lamboot-fs-tests` can include it verbatim via `#[path]` and
//! drive every firmware-read outcome (clean on/off, a non-spec byte, absent
//! variable, oversized variable, transient errors, and the single-retry
//! recovery) on the host with a stateful closure — no UEFI needed.
//!
//! Fail-closed contract: a genuinely indeterminate read (empty payload or a
//! transient error) must never silently downgrade to the no-verification
//! `Disabled` posture, or a flaky/attacker-perturbed read becomes a free
//! verification skip. BUT it must not over-fail-closed: a `NotFound`/
//! `Unsupported` firmware has no Secure Boot at all (→ off), and a single
//! transient blip on an SB-off box must retry and then honor the real value
//! rather than wrongly refuse an unsigned kernel. So: clean value decides;
//! absence/unsupported → off; oversized (`BufferTooSmall`) → on; indeterminate
//! → retry once, then fail closed (on).

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

/// One firmware read of the `SecureBoot` variable, classified into the pure
/// vocabulary `secure.rs` maps `runtime::get_variable` results onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbReadOutcome {
    /// A clean read: `Some(byte)` for the 1-byte value, `None` for an empty
    /// (indeterminate) payload.
    Value(Option<u8>),
    /// The variable does not exist (`NOT_FOUND`) — no Secure Boot.
    NotFound,
    /// Firmware does not implement Secure Boot variables (`UNSUPPORTED`).
    Unsupported,
    /// The variable exists but is larger than the 1-byte buffer
    /// (`BUFFER_TOO_SMALL`) — SB is configured.
    BufferTooSmall,
    /// Any other transient/indeterminate firmware error.
    TransientError,
}

/// Classify Secure Boot enablement from repeated reads, retrying once on an
/// indeterminate result before failing closed (on).
///
/// `next` performs one firmware read; it is called at most twice (the initial
/// read plus a single retry on an indeterminate outcome). Only `0x01` is "on";
/// any other clean byte is "off" (the exact `== 1` comparison is pinned here).
pub fn classify_secure_boot(mut next: impl FnMut() -> SbReadOutcome) -> bool {
    let mut attempt = 0u8;
    loop {
        match next() {
            SbReadOutcome::Value(Some(byte)) => return byte == 1,
            SbReadOutcome::NotFound | SbReadOutcome::Unsupported => return false,
            SbReadOutcome::BufferTooSmall => return true,
            SbReadOutcome::Value(None) | SbReadOutcome::TransientError => {
                if attempt == 0 {
                    attempt += 1;
                    continue;
                }
                return true;
            }
        }
    }
}
