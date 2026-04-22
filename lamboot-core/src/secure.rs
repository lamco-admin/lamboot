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

/// Detect the current Secure Boot state
pub(crate) fn detect_secure_boot() -> SecureBootState {
    // Read the SecureBoot EFI variable
    let mut buf = [0u8; 1];
    let sb_enabled = match runtime::get_variable(SECURE_BOOT_VAR, EFI_GLOBAL_VARIABLE, &mut buf) {
        Ok((data, _)) if !data.is_empty() => data[0] == 1,
        _ => false,
    };

    if !sb_enabled {
        return SecureBootState::Disabled;
    }

    // Check if shim is loaded (ShimLock protocol present)
    if uefi::boot::get_handle_for_protocol::<ShimLock>().is_ok() {
        SecureBootState::ActiveWithShim
    } else {
        SecureBootState::ActiveDirect
    }
}

/// Verify a PE image buffer against shim's embedded certificate.
/// Returns Ok(()) if verification succeeds or Secure Boot is disabled.
/// Returns Err if verification fails.
pub(crate) fn verify_image(image_data: &[u8]) -> uefi::Result {
    let state = detect_secure_boot();

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

/// Log the Secure Boot state at startup
pub(crate) fn log_secure_boot_state() {
    let state = detect_secure_boot();
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
