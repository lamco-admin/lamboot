//! Boot health monitoring via UEFI NVRAM variables.
//!
//! Implements a state machine for crash detection and recovery:
//!   Fresh → Booting → BootedOK (success) or CrashLoop (failure)
//!
//! All variables use vendor GUID 4C414D42-4F4F-5400-0000-000000000001
//! ("LAMBOOT") and are readable from the Proxmox host via OVMF_VARS.fd.

use uefi::{
    guid,
    prelude::*,
    runtime,
    runtime::{ResetType, VariableAttributes, VariableVendor},
    CStr16, Result,
};

/// LamBoot vendor GUID: 4C414D42-4F4F-5400-0000-000000000001
const LAMBOOT_VENDOR: &VariableVendor =
    &VariableVendor(guid!("4c414d42-4f4f-5400-0000-000000000001"));

/// Boot Loader Interface vendor GUID (systemd-boot compatible)
const LOADER_VENDOR: &VariableVendor =
    &VariableVendor(guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"));

// LamBoot-specific variables
const STATE_VAR: &CStr16 = cstr16!("LamBootState");
const CRASH_COUNTER_VAR: &CStr16 = cstr16!("LamBootCrashCount");
const LAST_ENTRY_VAR: &CStr16 = cstr16!("LamBootLastEntry");
const TIMESTAMP_VAR: &CStr16 = cstr16!("LamBootTimestamp");
const VERSION_VAR: &CStr16 = cstr16!("LamBootVersion");

// Boot Loader Interface variables (systemd-boot compatible)
const LOADER_INFO_VAR: &CStr16 = cstr16!("LoaderInfo");
const LOADER_ENTRY_SELECTED_VAR: &CStr16 = cstr16!("LoaderEntrySelected");
const LOADER_BOOT_COUNT_PATH_VAR: &CStr16 = cstr16!("LoaderBootCountPath");

const ATTRS: VariableAttributes = VariableAttributes::NON_VOLATILE
    .union(VariableAttributes::BOOTSERVICE_ACCESS)
    .union(VariableAttributes::RUNTIME_ACCESS);

/// LamBoot version packed as u32: major << 16 | minor << 8 | patch
const LAMBOOT_VERSION: u32 = 2 << 8; // 0.2.0

/// Boot state machine values
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootState {
    Fresh = 0,
    Booting = 1,
    BootedOK = 2,
    CrashLoop = 3,
}

impl BootState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Booting,
            2 => Self::BootedOK,
            3 => Self::CrashLoop,
            _ => Self::Fresh,
        }
    }
}

/// Read the current boot state from NVRAM
pub(crate) fn get_state() -> BootState {
    let mut buf = [0u8; 1];
    match runtime::get_variable(STATE_VAR, LAMBOOT_VENDOR, &mut buf) {
        Ok((data, _)) if !data.is_empty() => BootState::from_u8(data[0]),
        _ => BootState::Fresh,
    }
}

/// Write boot state to NVRAM
fn set_state(state: BootState) -> Result {
    runtime::set_variable(STATE_VAR, LAMBOOT_VENDOR, ATTRS, &[state as u8])?;
    Ok(())
}

/// Get the current crash counter
pub(crate) fn get_crash_counter() -> u8 {
    let mut buf = [0u8; 1];
    match runtime::get_variable(CRASH_COUNTER_VAR, LAMBOOT_VENDOR, &mut buf) {
        Ok((data, _)) if !data.is_empty() => data[0],
        _ => 0,
    }
}

/// Write the current EFI timestamp to NVRAM
fn write_timestamp() -> Result {
    match uefi::runtime::get_time() {
        Ok(time) => {
            // Pack as 8 bytes: year(2) month(1) day(1) hour(1) min(1) sec(1) pad(1)
            let bytes = [
                (time.year() & 0xFF) as u8,
                (time.year() >> 8) as u8,
                time.month(),
                time.day(),
                time.hour(),
                time.minute(),
                time.second(),
                0, // padding
            ];
            runtime::set_variable(TIMESTAMP_VAR, LAMBOOT_VENDOR, ATTRS, &bytes)?;
        }
        Err(_) => {
            // No RTC available — write zeros
            runtime::set_variable(TIMESTAMP_VAR, LAMBOOT_VENDOR, ATTRS, &[0u8; 8])?;
        }
    }
    Ok(())
}

/// Set Boot Loader Interface variables (systemd-boot compatible)
fn set_loader_info() -> Result {
    // LoaderInfo — identifies the bootloader
    let info = b"L\0a\0m\0B\0o\0o\0t\0 \x000\0.\x002\0.\x000\0\0\0";
    runtime::set_variable(LOADER_INFO_VAR, LOADER_VENDOR, ATTRS, info)?;

    // LamBootVersion — machine-readable version
    runtime::set_variable(
        VERSION_VAR,
        LAMBOOT_VENDOR,
        ATTRS,
        &LAMBOOT_VERSION.to_le_bytes(),
    )?;

    Ok(())
}

/// Assess boot health on startup. Returns the crash counter value.
///
/// State machine:
/// - Previous state Booting → crash detected → increment counter
/// - Previous state BootedOK/Fresh → success → reset counter
/// - Sets state to Booting, writes timestamp, sets loader info
pub(crate) fn assess_boot_health() -> Result<u8> {
    // vmgenid check: if VM generation changed (snapshot restore, template clone),
    // reset state to Fresh to avoid false crash loop detection
    if let Some(current_genid) = crate::partitions::read_vmgenid() {
        let stored_genid = get_vmgenid();
        if stored_genid.is_none() || stored_genid.as_ref() != Some(&current_genid) {
            log::info!("VM generation ID changed — resetting boot state");
            set_state(BootState::Fresh)?;
            runtime::set_variable(CRASH_COUNTER_VAR, LAMBOOT_VENDOR, ATTRS, &[0])?;
            set_vmgenid(&current_genid)?;
            let _ = write_timestamp();
            let _ = set_loader_info();
            return Ok(0);
        }
    }

    let prev_state = get_state();
    let mut counter = get_crash_counter();

    match prev_state {
        BootState::Booting => {
            counter = counter.saturating_add(1);
            log::warn!("Previous boot did not complete. Crash counter: {counter}");
        }
        BootState::BootedOK | BootState::Fresh => {
            counter = 0;
        }
        BootState::CrashLoop => {
            log::error!("System is in crash loop state. Counter: {counter}");
        }
    }

    // Write updated counter
    runtime::set_variable(CRASH_COUNTER_VAR, LAMBOOT_VENDOR, ATTRS, &[counter])?;

    // Transition to Booting state
    set_state(BootState::Booting)?;

    // Record timestamp and loader info
    let _ = write_timestamp();
    let _ = set_loader_info();

    log::info!("Boot health: prev={prev_state:?}, crash_counter={counter}");
    Ok(counter)
}

/// Check if we're in a crash loop based on the threshold
pub(crate) fn is_crash_loop(threshold: u8) -> bool {
    get_crash_counter() >= threshold
}

/// Record which entry we're about to boot
pub(crate) fn record_boot_entry(entry_id: &str) -> Result {
    let bytes = entry_id.as_bytes();
    let len = bytes.len().min(128);
    runtime::set_variable(LAST_ENTRY_VAR, LAMBOOT_VENDOR, ATTRS, &bytes[..len])?;

    // Also set systemd-boot compatible LoaderEntrySelected (UTF-16)
    let mut utf16_buf = alloc::vec::Vec::with_capacity((len + 1) * 2);
    for &b in &bytes[..len] {
        utf16_buf.push(b);
        utf16_buf.push(0);
    }
    utf16_buf.push(0); // null terminator
    utf16_buf.push(0);
    let _ = runtime::set_variable(LOADER_ENTRY_SELECTED_VAR, LOADER_VENDOR, ATTRS, &utf16_buf);

    Ok(())
}

/// Set the LoaderBootCountPath variable (for systemd-bless-boot compatibility)
pub(crate) fn set_boot_count_path(path: &str) -> Result {
    let bytes = path.as_bytes();
    let len = bytes.len().min(256);
    let mut utf16_buf = alloc::vec::Vec::with_capacity((len + 1) * 2);
    for &b in &bytes[..len] {
        utf16_buf.push(b);
        utf16_buf.push(0);
    }
    utf16_buf.push(0);
    utf16_buf.push(0);
    runtime::set_variable(LOADER_BOOT_COUNT_PATH_VAR, LOADER_VENDOR, ATTRS, &utf16_buf)?;
    Ok(())
}

/// Mark boot as successful — state → BootedOK, counter → 0
pub(crate) fn mark_boot_success() -> Result {
    set_state(BootState::BootedOK)?;
    runtime::set_variable(CRASH_COUNTER_VAR, LAMBOOT_VENDOR, ATTRS, &[0])?;
    let _ = write_timestamp();
    log::info!("Boot marked successful, counter cleared");
    Ok(())
}

// --- vmgenid NVRAM Storage ---

const VMGENID_VAR: &CStr16 = cstr16!("LamBootVmGenId");

fn get_vmgenid() -> Option<[u8; 16]> {
    let mut buf = [0u8; 16];
    match runtime::get_variable(VMGENID_VAR, LAMBOOT_VENDOR, &mut buf) {
        Ok((data, _)) if data.len() == 16 => {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(data);
            Some(arr)
        }
        _ => None,
    }
}

fn set_vmgenid(genid: &[u8; 16]) -> Result {
    runtime::set_variable(VMGENID_VAR, LAMBOOT_VENDOR, ATTRS, genid)?;
    Ok(())
}

// --- System Recovery Actions ---

/// EFI Global Variable GUID (for OsIndications)
const EFI_GLOBAL_VARIABLE: &VariableVendor =
    &VariableVendor(guid!("8be4df61-93ca-11d2-aa0d-00e098032b8c"));

const OS_INDICATIONS_VAR: &CStr16 = cstr16!("OsIndications");
const OS_INDICATIONS_SUPPORTED_VAR: &CStr16 = cstr16!("OsIndicationsSupported");

/// Bit 0 of OsIndications: request firmware UI on next boot
const EFI_OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 1;

/// Reboot into UEFI firmware setup (BIOS menu).
/// Sets OsIndications variable to request firmware UI, then cold resets.
/// This function does not return.
pub(crate) fn reboot_to_firmware_setup() -> ! {
    // Check if firmware supports boot-to-FW-UI
    let mut buf = [0u8; 8];
    let supported =
        match runtime::get_variable(OS_INDICATIONS_SUPPORTED_VAR, EFI_GLOBAL_VARIABLE, &mut buf) {
            Ok((data, _)) if data.len() >= 8 => {
                u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))
            }
            Ok((data, _)) if !data.is_empty() => {
                // Handle shorter-than-expected variable
                let mut arr = [0u8; 8];
                arr[..data.len()].copy_from_slice(data);
                u64::from_le_bytes(arr)
            }
            _ => 0,
        };

    if supported & EFI_OS_INDICATIONS_BOOT_TO_FW_UI != 0 {
        // Read current OsIndications value (may have other bits set)
        let current = match runtime::get_variable(OS_INDICATIONS_VAR, EFI_GLOBAL_VARIABLE, &mut buf)
        {
            Ok((data, _)) if !data.is_empty() => {
                let mut arr = [0u8; 8];
                arr[..data.len().min(8)].copy_from_slice(&data[..data.len().min(8)]);
                u64::from_le_bytes(arr)
            }
            _ => 0,
        };

        // Set the boot-to-FW-UI bit
        let new_value = current | EFI_OS_INDICATIONS_BOOT_TO_FW_UI;
        let _ = runtime::set_variable(
            OS_INDICATIONS_VAR,
            EFI_GLOBAL_VARIABLE,
            ATTRS,
            &new_value.to_le_bytes(),
        );
        log::info!("OsIndications set for firmware setup, resetting...");
    } else {
        log::warn!("Firmware does not support boot-to-FW-UI, performing cold reboot instead");
    }

    runtime::reset(ResetType::COLD, Status::SUCCESS, None)
}

/// Cold reboot the system. This function does not return.
pub(crate) fn reboot() -> ! {
    log::info!("Cold reboot requested");
    runtime::reset(ResetType::COLD, Status::SUCCESS, None)
}
