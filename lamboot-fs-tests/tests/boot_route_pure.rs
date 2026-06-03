//! Host unit tests for the native-vs-firmware load-route decision.
//!
//! The module lives at `lamboot-core/src/boot_route_pure.rs` and is included
//! verbatim into this test crate via `#[path]`. `boot::choose_load_path` adapts
//! the real `LoaderNativePeMode` / `SecureBootState` into these mirror enums and
//! delegates here, so this is the authoritative test of the 9-row truth table
//! (policy `native_pe` × Secure Boot posture).
//!
//! Dual contract: the flexibility rows assert that every legitimate deployment
//! (SB-off homelab, shim-verified enterprise, the operator `always`/`never`
//! escape hatches incl. on a Config-4 db-direct box) takes the route the docs
//! promise; the single security row is `Auto` + `ActiveDirect` (the C1 fix),
//! which MUST route firmware so the firmware `db` check runs rather than
//! executing an unauthenticated kernel through the native loader.

use lamboot_fs_tests::boot_route_pure::{decide_load_route, LoadRoute, NativePeMode, SbPosture};

// ---- Never: rollback escape hatch — always firmware, every posture ----

#[test]
fn never_disabled_routes_firmware() {
    assert_eq!(
        decide_load_route(NativePeMode::Never, SbPosture::Disabled),
        LoadRoute::Firmware
    );
}

#[test]
fn never_shim_routes_firmware() {
    assert_eq!(
        decide_load_route(NativePeMode::Never, SbPosture::ActiveWithShim),
        LoadRoute::Firmware
    );
}

#[test]
fn never_direct_routes_firmware() {
    assert_eq!(
        decide_load_route(NativePeMode::Never, SbPosture::ActiveDirect),
        LoadRoute::Firmware
    );
}

// ---- Always: explicit operator trust assertion — always native ----

#[test]
fn always_disabled_routes_native() {
    assert_eq!(
        decide_load_route(NativePeMode::Always, SbPosture::Disabled),
        LoadRoute::Native
    );
}

#[test]
fn always_shim_routes_native() {
    assert_eq!(
        decide_load_route(NativePeMode::Always, SbPosture::ActiveWithShim),
        LoadRoute::Native
    );
}

#[test]
fn always_direct_routes_native_operator_escape() {
    // Config-4 db-direct deployment where the operator EXPLICITLY set
    // native_pe="always". The C1 fix must NOT clobber this — only Auto routes
    // ActiveDirect to firmware.
    assert_eq!(
        decide_load_route(NativePeMode::Always, SbPosture::ActiveDirect),
        LoadRoute::Native
    );
}

// ---- Auto (default): native unless db-direct with no verifier ----

#[test]
fn auto_disabled_routes_native() {
    assert_eq!(
        decide_load_route(NativePeMode::Auto, SbPosture::Disabled),
        LoadRoute::Native
    );
}

#[test]
fn auto_shim_routes_native() {
    assert_eq!(
        decide_load_route(NativePeMode::Auto, SbPosture::ActiveWithShim),
        LoadRoute::Native
    );
}

#[test]
fn auto_direct_routes_firmware_c1_security_row() {
    // SB on, no shim, operator did NOT assert trust: the native loader performs
    // no cryptographic check, so routing native would execute the kernel
    // unauthenticated — a silent Secure Boot bypass. MUST route firmware.
    assert_eq!(
        decide_load_route(NativePeMode::Auto, SbPosture::ActiveDirect),
        LoadRoute::Firmware
    );
}
