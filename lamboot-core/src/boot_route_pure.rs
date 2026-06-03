//! Layer: 4 (pure) — Load-route decision.
//!
//! The native-loader-vs-firmware-`LoadImage` routing decision, split out so
//! `lamboot-fs-tests` can include it verbatim via `#[path]` and exercise the
//! full 9-row truth table (policy `native_pe` × Secure Boot posture) on the
//! host. `boot.rs` adapts its real `LoaderNativePeMode` / `SecureBootState` into
//! these mirror enums and calls [`decide_load_route`], so the security-relevant
//! routing lives — and is tested — in exactly one place. The Volume/firmware-
//! bound load implementations themselves stay in `boot.rs`.
//!
//! The single must-reject row is `Auto` + `ActiveDirect` (Secure Boot on, no
//! shim, operator did not assert trust): the native loader performs no
//! cryptographic check, so routing must fall to firmware `BS->LoadImage` where
//! the real `db` check runs. `Always` is an explicit operator trust assertion
//! and is honored even there; `Never` is the rollback escape hatch and always
//! routes firmware.

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

/// Mirror of `policy::LoaderNativePeMode` (the operator's `native_pe` setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePeMode {
    Auto,
    Always,
    Never,
}

/// Mirror of `secure::SecureBootState` (the posture the bytes were verified under).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbPosture {
    Disabled,
    ActiveWithShim,
    ActiveDirect,
}

/// The chosen image-load path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRoute {
    /// LamBoot's native pe_loader (bypasses `BS->LoadImage`).
    Native,
    /// `uefi::boot::load_image` — the firmware path that runs the `db` check.
    Firmware,
}

/// Decide the load route from policy + the Secure Boot posture.
///
/// `Auto` is the only mode that inspects the posture: under `ActiveDirect`
/// (SB on, no shim, nothing verified the kernel) it must route firmware so the
/// firmware `db` check runs, rather than executing the kernel unauthenticated
/// through the native loader. Every other `Auto` posture (SB off, or
/// shim-verified) is safe for the native loader.
pub fn decide_load_route(mode: NativePeMode, posture: SbPosture) -> LoadRoute {
    match mode {
        NativePeMode::Never => LoadRoute::Firmware,
        NativePeMode::Always => LoadRoute::Native,
        NativePeMode::Auto => {
            if matches!(posture, SbPosture::ActiveDirect) {
                LoadRoute::Firmware
            } else {
                LoadRoute::Native
            }
        }
    }
}
