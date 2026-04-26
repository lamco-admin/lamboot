//! SecurityArchProtocol override for shim+MOK *driver* loads.
//!
//! # Scope as of v0.9.x (SDS-4 native trust chain)
//!
//! This module's override is now scoped to **legacy UEFI filesystem-driver
//! `LoadImage` calls only**. The kernel-load path no longer routes through
//! here:
//!
//! - Kernels are verified via [`ShimLock::Verify`] directly on the in-memory
//!   bytes in [`crate::boot::verify_kernel_bytes`], then handed to
//!   [`crate::pe_loader`] for a native-PE load. `BS->LoadImage` is not
//!   invoked on kernels under the native path, so no firmware `db` check
//!   runs and no `SecurityArch` delegation is needed.
//! - Drivers (legacy UEFI FS drivers wired via
//!   [`crate::drivers::load_drivers`]) still go through `BS->LoadImage`.
//!   Those *do* hit the firmware security hooks and therefore still
//!   require this override when signed with MOK-only certs.
//!
//! The long-term intent per SDS-6 is to retire the legacy UEFI FS driver
//! path entirely (ext4 is already natively read by `ext4-view`). Once the
//! last driver under `\EFI\LamBoot\drivers\` is removed, this module
//! becomes dead code; until then, keep it correct and minimal.
//!
//! # Background (why the override exists at all)
//!
//! Under shim < v16 (Ubuntu 25.10 ships 15.8, Debian 13, Fedora current as of
//! 2026-04), shim does NOT hook `BS->LoadImage`. When LamBoot calls LoadImage
//! on a MOK-signed driver, firmware's native Secure Boot check runs, sees a
//! signature issued by a cert that isn't in firmware `db` (our cert is only
//! in shim's MOK list), and returns `EFI_ACCESS_DENIED`. This is exactly
//! [systemd/systemd#38624](https://github.com/systemd/systemd/issues/38624).
//!
//! The canonical fix, implemented by systemd-boot in
//! `src/boot/secure-boot.c:install_security_override`, is to temporarily
//! replace firmware's `EFI_SECURITY_ARCH_PROTOCOL->FileAuthenticationState` and
//! `EFI_SECURITY2_ARCH_PROTOCOL->FileAuthentication` with our own hooks that
//! call ShimLock::Verify on the image buffer. If ShimLock approves (cert is in
//! MOK), return SUCCESS; otherwise delegate to the original firmware hook.
//!
//! This is labeled a "hack" in sd-boot's own source comments but is the
//! industry-standard pattern for this exact problem. Target lifetime of the
//! override is a single `LoadImage` call; install just before, uninstall just
//! after. For kernel loads this mechanism is obsolete under SDS-4 — the
//! native path verifies once on bytes and loads via `pe_loader`, never
//! touching `BS->LoadImage`.
//!
//! **Critical invariant:** only one override may be active at a time. Nested
//! installs are rejected. Callers MUST uninstall before installing again.

#![expect(
    unsafe_code,
    reason = "Security protocols are accessed via raw pointers per UEFI spec; \
              mutating firmware protocol tables is inherently unsafe."
)]

use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use uefi::{
    cstr16, guid,
    prelude::*,
    proto::shim::ShimLock,
    runtime::{self, VariableAttributes, VariableVendor},
    Guid,
};

// ============================================================================
// Protocol GUIDs
// ============================================================================

/// EFI_SECURITY_ARCH_PROTOCOL_GUID — UEFI PI spec, firmware's image
/// authentication service (legacy, device-path-based).
const SECURITY_ARCH_PROTOCOL_GUID: Guid = guid!("a46423e3-4617-49f1-b9ff-d1bfa9115839");

/// EFI_SECURITY2_ARCH_PROTOCOL_GUID — UEFI PI spec, newer variant that
/// takes an in-memory buffer. This is the one actually called by firmware
/// for LoadImage(FromBuffer), so it's the path we must override for
/// buffer-based loads.
const SECURITY2_ARCH_PROTOCOL_GUID: Guid = guid!("94ab2f58-1438-4ef1-9152-18941a3a0e68");

// ============================================================================
// Protocol structures (repr(C), matching UEFI PI spec exactly)
// ============================================================================

type EfiFileAuthenticationState = unsafe extern "efiapi" fn(
    this: *const SecurityArchProtocol,
    authentication_status: u32,
    file: *const c_void, // EFI_DEVICE_PATH_PROTOCOL
) -> Status;

type EfiFileAuthentication = unsafe extern "efiapi" fn(
    this: *const Security2ArchProtocol,
    device_path: *const c_void, // EFI_DEVICE_PATH_PROTOCOL
    file_buffer: *mut c_void,
    file_size: usize,
    boot_policy: bool,
) -> Status;

#[repr(C)]
struct SecurityArchProtocol {
    file_authentication_state: EfiFileAuthenticationState,
}

#[repr(C)]
struct Security2ArchProtocol {
    file_authentication: EfiFileAuthentication,
}

// ============================================================================
// Override state — stored in statics because firmware callbacks are extern "efiapi"
// and cannot capture environment.
// ============================================================================

static OVERRIDE_ACTIVE: AtomicBool = AtomicBool::new(false);

// Per-load diagnostics accumulated in hook_security2 so the trust log can
// report what the hook actually did. These are UEFI-callback-safe (atomic
// store), readable from main task context via the getters below.
static HOOK2_CALLS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static SHIM_VALIDATE_CALLS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static SHIM_VALIDATE_TRUE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static DELEGATE_CALLS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static SHIMLOCK_NOT_FOUND: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Diagnostic snapshot: (total hook2 invocations, shim_validate calls,
/// shim_validate returning true, delegations to original, ShimLock lookup failures).
pub(crate) fn diagnostic_counters() -> (usize, usize, usize, usize, usize) {
    (
        HOOK2_CALLS.load(Ordering::Relaxed),
        SHIM_VALIDATE_CALLS.load(Ordering::Relaxed),
        SHIM_VALIDATE_TRUE.load(Ordering::Relaxed),
        DELEGATE_CALLS.load(Ordering::Relaxed),
        SHIMLOCK_NOT_FOUND.load(Ordering::Relaxed),
    )
}

/// SAFETY invariant: all access to these statics is serialized by OVERRIDE_ACTIVE.
/// install_security_override uses compare_exchange to take exclusive ownership;
/// hooks and uninstall run only when override is active.
static mut SECURITY: *mut SecurityArchProtocol = ptr::null_mut();
static mut SECURITY2: *mut Security2ArchProtocol = ptr::null_mut();
static mut ORIGINAL_HOOK: Option<EfiFileAuthenticationState> = None;
static mut ORIGINAL_HOOK2: Option<EfiFileAuthentication> = None;

// ============================================================================
// Our replacement hooks — called by firmware during LoadImage
// ============================================================================

/// Hook installed in EFI_SECURITY_ARCH_PROTOCOL. Called when firmware needs to
/// authenticate a file by device path (no buffer). We cannot call ShimLock::Verify
/// without reading the file ourselves first; for simplicity we delegate to the
/// original hook. The SECURITY2 path below is what firmware actually uses for
/// LoadImage(FromBuffer).
unsafe extern "efiapi" fn hook_security(
    this: *const SecurityArchProtocol,
    auth_status: u32,
    file: *const c_void,
) -> Status {
    // SAFETY: ORIGINAL_HOOK is set during install_security_override and
    // cleared during uninstall, both under OVERRIDE_ACTIVE serialization.
    let original = unsafe { ORIGINAL_HOOK };
    match original {
        Some(f) => unsafe { f(this, auth_status, file) },
        None => Status::SECURITY_VIOLATION,
    }
}

/// Hook installed in EFI_SECURITY2_ARCH_PROTOCOL. This is the one firmware
/// calls for LoadImage with a source buffer. Verifies the buffer via shim's
/// ShimLock protocol; on success returns EFI_SUCCESS (firmware proceeds with
/// load); on shim rejection delegates to original firmware check (which will
/// normally return EFI_ACCESS_DENIED since our cert isn't in db).
unsafe extern "efiapi" fn hook_security2(
    this: *const Security2ArchProtocol,
    device_path: *const c_void,
    file_buffer: *mut c_void,
    file_size: usize,
    boot_policy: bool,
) -> Status {
    HOOK2_CALLS.fetch_add(1, Ordering::Relaxed);
    if !file_buffer.is_null() && file_size > 0 {
        // SAFETY: firmware passes a pointer to the image buffer with its size.
        // We treat it as an immutable slice for ShimLock::Verify.
        let buf = unsafe { core::slice::from_raw_parts(file_buffer.cast::<u8>(), file_size) };
        SHIM_VALIDATE_CALLS.fetch_add(1, Ordering::Relaxed);
        if shim_validate(buf) {
            SHIM_VALIDATE_TRUE.fetch_add(1, Ordering::Relaxed);
            return Status::SUCCESS;
        }
    }
    DELEGATE_CALLS.fetch_add(1, Ordering::Relaxed);
    // Delegate to original firmware security check.
    // SAFETY: ORIGINAL_HOOK2 is set during install_security_override and
    // cleared during uninstall, both under OVERRIDE_ACTIVE serialization.
    let original = unsafe { ORIGINAL_HOOK2 };
    match original {
        Some(f) => unsafe { f(this, device_path, file_buffer, file_size, boot_policy) },
        None => Status::SECURITY_VIOLATION,
    }
}

/// Call ShimLock::Verify on a buffer. Returns true iff shim trusts the image.
/// Exposed crate-visible so preflight can mirror the runtime trust decision
/// without re-implementing the ShimLock dance (otherwise preflight would
/// false-warn "Not signed for Secure Boot" on MOK-chained distro kernels
/// whose certs live in MOK rather than firmware `db`).
pub(crate) fn shim_validate(buffer: &[u8]) -> bool {
    let Ok(handle) = uefi::boot::get_handle_for_protocol::<ShimLock>() else {
        SHIMLOCK_NOT_FOUND.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    let Ok(shim_lock) = uefi::boot::open_protocol_exclusive::<ShimLock>(handle) else {
        return false;
    };
    shim_lock.verify(buffer).is_ok()
}

/// Ask shim to keep its ShimLock protocol installed across StartImage calls.
///
/// Shim 15.8+ reads a `ShimRetainProtocol` EFI variable under the SHIM_LOCK
/// vendor GUID when deciding whether to uninstall its security protocols on
/// child-image exit. Without this, shim uninstalls ShimLock after the first
/// successful `StartImage()` returned by a child (e.g. our ext4 driver),
/// rendering subsequent `ShimLock::Verify` calls on kernel images unable to
/// validate — even for distro-signed kernels that shim would otherwise trust.
///
/// Reference: [rhboot/shim#444](https://github.com/rhboot/shim/issues/444),
/// systemd-boot's `shim_retain_protocol()` in `src/boot/shim.c:127`.
///
/// Call this once at LamBoot startup, before any driver or kernel load.
/// Safe no-op if shim isn't present.
pub(crate) fn request_shim_retain_protocol() {
    if uefi::boot::get_handle_for_protocol::<ShimLock>().is_err() {
        // No shim in this boot — nothing to retain.
        return;
    }
    let shim_lock_vendor = VariableVendor(guid!("605dab50-e046-4300-abb6-3dd810dd8b23"));
    let var_name = cstr16!("ShimRetainProtocol");
    let value = [1u8];
    let attrs = VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS
        | VariableAttributes::NON_VOLATILE;
    match runtime::set_variable(var_name, &shim_lock_vendor, attrs, &value) {
        Ok(()) => log::info!("Requested shim to retain ShimLock protocol (ShimRetainProtocol=1)"),
        Err(e) => log::warn!("Failed to set ShimRetainProtocol: {e:?}"),
    }
}

// ============================================================================
// Public API
// ============================================================================

/// RAII guard — uninstalls the security override when dropped.
///
/// Scope this over a single `uefi::boot::load_image` call for best practice.
pub(crate) struct SecurityOverrideGuard {
    _private: (),
}

impl Drop for SecurityOverrideGuard {
    fn drop(&mut self) {
        uninstall_security_override();
    }
}

/// Install a temporary security override that routes UEFI LoadImage signature
/// checks through ShimLock::Verify (and delegates on shim rejection).
///
/// Returns a guard that uninstalls on drop. If no SecurityArch protocols are
/// present (rare, or SB is disabled), returns None — callers should treat this
/// as "no override needed, proceed with plain LoadImage".
///
/// Fails silently if an override is already active (returns None). Only one
/// override may be in flight at a time.
pub(crate) fn install_security_override() -> Option<SecurityOverrideGuard> {
    // Take exclusive ownership of the override state
    if OVERRIDE_ACTIVE
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        log::warn!("security_override: already active, refusing nested install");
        return None;
    }

    // Locate SECURITY2 first (the path actually used by firmware for buffer loads)
    let security2 = locate_security2_protocol();
    let security = locate_security_protocol();

    if security2.is_null() && security.is_null() {
        // Neither protocol available — SB likely off or firmware doesn't expose
        // them. Release ownership and let caller proceed.
        OVERRIDE_ACTIVE.store(false, Ordering::SeqCst);
        log::debug!("security_override: no SecurityArch protocols found, skipping");
        return None;
    }

    // SAFETY: we hold exclusive ownership via OVERRIDE_ACTIVE. Install hooks.
    unsafe {
        SECURITY = security;
        SECURITY2 = security2;

        if !security.is_null() {
            ORIGINAL_HOOK = Some((*security).file_authentication_state);
            (*security).file_authentication_state = hook_security;
        }
        if !security2.is_null() {
            ORIGINAL_HOOK2 = Some((*security2).file_authentication);
            (*security2).file_authentication = hook_security2;
        }
    }

    log::debug!(
        "security_override installed (security={}, security2={})",
        !security.is_null(),
        !security2.is_null()
    );
    Some(SecurityOverrideGuard { _private: () })
}

fn uninstall_security_override() {
    if !OVERRIDE_ACTIVE.load(Ordering::SeqCst) {
        return;
    }

    // SAFETY: we own the statics while OVERRIDE_ACTIVE is true. Restore hooks
    // in reverse order of install.
    unsafe {
        if !SECURITY2.is_null() {
            if let Some(orig) = ORIGINAL_HOOK2 {
                (*SECURITY2).file_authentication = orig;
            }
            ORIGINAL_HOOK2 = None;
            SECURITY2 = ptr::null_mut();
        }
        if !SECURITY.is_null() {
            if let Some(orig) = ORIGINAL_HOOK {
                (*SECURITY).file_authentication_state = orig;
            }
            ORIGINAL_HOOK = None;
            SECURITY = ptr::null_mut();
        }
    }

    OVERRIDE_ACTIVE.store(false, Ordering::SeqCst);
    log::debug!("security_override uninstalled");
}

// ============================================================================
// Protocol location helpers — use raw boot services because uefi-rs does not
// expose the SecurityArch protocols.
// ============================================================================

fn locate_security_protocol() -> *mut SecurityArchProtocol {
    locate_raw_protocol(&SECURITY_ARCH_PROTOCOL_GUID).cast()
}

fn locate_security2_protocol() -> *mut Security2ArchProtocol {
    locate_raw_protocol(&SECURITY2_ARCH_PROTOCOL_GUID).cast()
}

/// Thin wrapper around `BS->LocateProtocol` for protocols uefi-rs doesn't expose.
/// Returns null if the protocol isn't present.
fn locate_raw_protocol(guid: &Guid) -> *mut c_void {
    // uefi-rs 0.37's `uefi::boot::locate_protocol` via raw system table access:
    // we go through uefi_raw for the actual LocateProtocol call.
    let Some(systab) = uefi::table::system_table_raw() else {
        return ptr::null_mut();
    };
    // SAFETY: system_table_raw returns a valid pointer during boot services.
    let bs = unsafe { (*systab.as_ptr()).boot_services };
    if bs.is_null() {
        return ptr::null_mut();
    }

    let mut interface: *mut c_void = ptr::null_mut();
    // uefi::Guid and uefi_raw::Guid are bit-compatible layouts; the uefi crate
    // uses a transparent newtype around the raw type.
    let raw_guid_ptr: *const uefi_raw::Guid = core::ptr::from_ref::<Guid>(guid).cast();
    // SAFETY: calling LocateProtocol per UEFI spec. Passing null registration.
    let status =
        unsafe { ((*bs).locate_protocol)(raw_guid_ptr, ptr::null_mut(), &raw mut interface) };
    if status.is_success() {
        interface
    } else {
        ptr::null_mut()
    }
}
