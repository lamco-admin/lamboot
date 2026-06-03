//! Layer: 0 — Platform Introspection.
//!
//! Secure Boot awareness and shim integration.
//!
//! LamBoot supports two Secure Boot modes:
//!
//! 1. **Shim 16.1+**: Shim overrides the SystemTable's LoadImage/StartImage,
//!    so standard boot services handle verification transparently. No special
//!    code needed — just use `boot::load_image()` as normal.
//!
//! 2. **Legacy shim (<16.1)**: Use the ShimLock protocol to verify images
//!    before loading them.
//!
//! 3. **Direct signing**: Binary signed with sbsign against db/MOK keys.
//!
//! Detection flow:
//!   - Read SecureBoot EFI variable → determine if SB is active
//!   - Check for ShimLock protocol → determine if shim is loaded
//!   - If SB active but no shim → we were loaded directly (self-signed)

use uefi::{guid, prelude::*, proto::shim::ShimLock, runtime, runtime::VariableVendor, CStr16};

/// EFI Global Variable GUID
const EFI_GLOBAL_VARIABLE: &VariableVendor =
    &VariableVendor(guid!("8be4df61-93ca-11d2-aa0d-00e098032b8c"));

const SECURE_BOOT_VAR: &CStr16 = cstr16!("SecureBoot");

/// Secure Boot state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecureBootState {
    /// Secure Boot is disabled or not supported
    Disabled,
    /// Secure Boot is active, loaded via shim (ShimLock protocol present)
    ActiveWithShim,
    /// Secure Boot is active, loaded directly (self-signed)
    ActiveDirect,
}

/// Detect the current Secure Boot state.
///
/// Reads the `SecureBoot` EFI variable and fails CLOSED on anything other than
/// a clean read or a genuine absence. A read error must never silently downgrade
/// to the no-verification `Disabled` posture: that would let a flaky firmware
/// read — or an oversized/torn variable returning `BUFFER_TOO_SMALL` against the
/// 1-byte buffer — become a free verification skip.
pub(crate) fn detect_secure_boot() -> SecureBootState {
    if !read_secure_boot_enabled() {
        return SecureBootState::Disabled;
    }

    // Check if shim is loaded (ShimLock protocol present)
    if uefi::boot::get_handle_for_protocol::<ShimLock>().is_ok() {
        SecureBootState::ActiveWithShim
    } else {
        SecureBootState::ActiveDirect
    }
}

/// Whether shim's `ShimLock` protocol is currently installed.
///
/// Re-probed at kernel-verify time (not only at the early single detection)
/// because shim **< 15.8** uninstalls `ShimLock` after its first use: a legacy
/// FS driver loaded for an XFS/ZFS/NTFS `/boot` between the early detection and
/// the kernel verify can consume it. Verifying against a vanished `ShimLock`
/// would hard-reject a kernel that should degrade to the firmware-`db` path.
pub(crate) fn shim_lock_present() -> bool {
    uefi::boot::get_handle_for_protocol::<ShimLock>().is_ok()
}

/// Read the SecureBoot variable into a yes/no.
///
/// Fails closed (assume ON) on a genuinely indeterminate read, so a flaky or
/// attacker-perturbed read on a real SB box never becomes a free verification
/// skip. BUT it must NOT over-fail-closed: a `NOT_FOUND`/`UNSUPPORTED` firmware
/// has no Secure Boot at all, and a single transient blip on an SB-OFF box must
/// not wrongly force verification and refuse a perfectly good unsigned kernel.
/// So: absence/unsupported → off; the variable existing-but-oversized
/// (`BUFFER_TOO_SMALL`) → on; an indeterminate transient error → retry once,
/// then fail closed.
fn read_secure_boot_enabled() -> bool {
    use crate::sb_classify_pure::{classify_secure_boot, SbReadOutcome};

    // One firmware read → pure outcome; the retry/fail-closed classification
    // lives in sb_classify_pure and is host-tested there.
    classify_secure_boot(|| {
        let mut buf = [0u8; 1];
        match runtime::get_variable(SECURE_BOOT_VAR, EFI_GLOBAL_VARIABLE, &mut buf) {
            Ok((data, _)) if !data.is_empty() => SbReadOutcome::Value(Some(data[0])),
            Ok(_) => SbReadOutcome::Value(None),
            Err(e) if e.status() == Status::NOT_FOUND => SbReadOutcome::NotFound,
            Err(e) if e.status() == Status::UNSUPPORTED => SbReadOutcome::Unsupported,
            Err(e) if e.status() == Status::BUFFER_TOO_SMALL => SbReadOutcome::BufferTooSmall,
            Err(_) => SbReadOutcome::TransientError,
        }
    })
}

/// Verify a PE image buffer against shim's embedded certificate.
/// Returns Ok(()) if verification succeeds or Secure Boot is disabled.
/// Returns Err if verification fails.
///
/// Takes the already-detected `state` rather than re-reading the SecureBoot
/// variable, so every consumer in one boot acts on a single source of truth
/// (the variable is stable for a boot; re-detecting only risked the verify
/// path, the report, and the trust log disagreeing).
pub(crate) fn verify_image(image_data: &[u8], state: SecureBootState) -> uefi::Result {
    match state {
        SecureBootState::Disabled => {
            // No verification needed
            Ok(())
        }
        SecureBootState::ActiveWithShim => {
            // Use ShimLock protocol to verify
            let handle = uefi::boot::get_handle_for_protocol::<ShimLock>()?;
            let shim_lock = uefi::boot::open_protocol_exclusive::<ShimLock>(handle)?;
            shim_lock.verify(image_data)?;
            log::info!("Image verified via ShimLock ({} bytes)", image_data.len());
            Ok(())
        }
        SecureBootState::ActiveDirect => {
            // We were loaded directly with valid signature — firmware already verified us.
            // For images we load via LoadImage, the firmware will verify them too.
            // No additional verification needed from our side.
            log::info!("Secure Boot active (direct), firmware handles verification");
            Ok(())
        }
    }
}

/// Log the Secure Boot state at startup. Takes the already-detected `state`
/// (see `verify_image`) rather than re-reading the variable.
pub(crate) fn log_secure_boot_state(state: SecureBootState) {
    match state {
        SecureBootState::Disabled => {
            log::info!("Secure Boot: disabled");
        }
        SecureBootState::ActiveWithShim => {
            log::info!("Secure Boot: active (via shim)");
        }
        SecureBootState::ActiveDirect => {
            log::info!("Secure Boot: active (direct signing)");
        }
    }
}
