//! Layer: 4 — boot-entry identity & NVRAM self-install.
//!
//! A self-contained firmware/NVRAM mechanism (like `health`): it depends on no
//! other lamboot-core module, so callers in the orchestration layer
//! (`discovery`, `boot`, `main`) own policy-gating and trust-log auditing of its
//! results.
//!
//! Two concerns that both need the running image's own UEFI identity:
//!
//!   * **Self-loop detection** (`booted_via_removable_fallback`) — when LamBoot
//!     was launched from the firmware removable-media path
//!     `\EFI\BOOT\BOOTX64.EFI`, offering that same path back as a chainload
//!     "EFI Fallback" entry re-invokes LamBoot itself. The pre-existing hash
//!     guard in `discovery::discover_other_loaders` is ineffective here: it
//!     compares the on-disk `BOOTX64.EFI` bytes against the **in-memory,
//!     relocated** running image (firmware applies base relocations at load), so
//!     the two hashes never match for a firmware-loaded fallback. Comparing the
//!     *path* we were loaded from is the reliable signal.
//!
//!   * **NVRAM self-install** (`ensure_named_boot_entry`) — the durable,
//!     OS-independent pathway for the persistent `LamBoot` boot entry. LamBoot
//!     runs in the UEFI environment with unmediated Runtime-Services access, so
//!     it can create its own `Boot####` + `BootOrder` entry directly — no
//!     `efibootmgr`, no OS, and on RHEL no SELinux confinement in the way. (Boot
//!     variables are non-authenticated, so no Secure Boot keys are needed.)

use alloc::{string::String, vec::Vec};

use uefi::{
    cstr16,
    proto::{
        device_path::{
            build::{self, DevicePathBuilder},
            media::FilePath,
            DevicePath, DeviceType,
        },
        loaded_image::LoadedImage,
    },
    runtime::{self, VariableAttributes, VariableVendor},
    CStr16, CString16,
};

use crate::boot_entry_pure;

/// EFI global variable namespace — home of `Boot####` and `BootOrder`.
const GLOBAL: &VariableVendor = &VariableVendor::GLOBAL_VARIABLE;

/// `BootOrder` variable name.
const BOOT_ORDER: &CStr16 = cstr16!("BootOrder");

/// Non-volatile, readable at boot- and runtime — the standard posture for a
/// persistent boot entry.
const ATTRS: VariableAttributes = VariableAttributes::NON_VOLATILE
    .union(VariableAttributes::BOOTSERVICE_ACCESS)
    .union(VariableAttributes::RUNTIME_ACCESS);

/// Upper bound for the `Boot####` idempotency scan / free-slot search. 256 slots
/// is far more than any real firmware uses and keeps the per-boot scan bounded.
const BOOT_SCAN_MAX: u16 = 0x00FF;

/// Outcome of [`ensure_named_boot_entry`], for the caller to audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfInstall {
    /// Created `Boot####` (number) and front-loaded `BootOrder`.
    Created(u16),
    /// An entry with the same description already existed at `Boot####`.
    AlreadyPresent(u16),
    /// No `LoadedImage` device / device-path; cannot derive our location.
    NoDevicePath,
    /// A firmware variable read/write or device-path build failed.
    Failed,
}

/// The file path LamBoot was loaded from, as a backslash path string
/// (e.g. `\EFI\BOOT\BOOTX64.EFI`), or `None` if the firmware exposes no
/// `LoadedImage` / device-path for the running image.
///
/// Concatenates every `MEDIA_FILEPATH` node in the loaded-image device path —
/// firmware may split a path across several FilePath nodes (one per segment).
pub(crate) fn running_image_file_path() -> Option<String> {
    let image = uefi::boot::image_handle();
    let li = uefi::boot::open_protocol_exclusive::<LoadedImage>(image).ok()?;
    let dp: &DevicePath = li.file_path()?;

    let mut out = String::new();
    for node in dp.node_iter() {
        let Ok(file): Result<&FilePath, _> = node.try_into() else {
            continue;
        };
        // path_name() is UTF-16, possibly unaligned, NUL-terminated per node.
        let path_name = file.path_name();
        for unit in &path_name {
            if unit == 0 {
                continue;
            }
            if let Some(c) = char::from_u32(u32::from(unit)) {
                out.push(c);
            }
        }
    }

    (!out.is_empty()).then_some(out)
}

/// True when LamBoot was launched from the firmware removable-media fallback
/// path `\EFI\BOOT\BOOTX64.EFI`. Offering that path back as a chainload entry
/// would be a self-loop, so discovery must suppress the "EFI Fallback"
/// synthetic in that case. Matches case-insensitively and tolerates either
/// separator (some firmware reports `/`-style FilePath nodes).
pub(crate) fn booted_via_removable_fallback() -> bool {
    running_image_file_path().is_some_and(|p| {
        let up = p.to_ascii_uppercase();
        up.ends_with("\\EFI\\BOOT\\BOOTX64.EFI") || up.ends_with("/EFI/BOOT/BOOTX64.EFI")
    })
}

/// Ensure a persistent UEFI `Boot####` entry named `description` exists,
/// pointing at `install_file_path` (e.g. `\EFI\LamBoot\lambootx64.efi`) on the
/// partition LamBoot was loaded from.
///
/// **Idempotent and conservative.** It scans existing `Boot####` slots; if one
/// already carries `description`, it returns `AlreadyPresent` and writes
/// nothing (so re-running every boot — and coexisting with the OS-side
/// `efibootmgr` pathway, which keys on the same exact description — never
/// duplicates or churns NVRAM). Only when absent does it create the entry at
/// the lowest free slot and **front-load** `BootOrder`, leaving every other
/// entry untouched. `Boot####`/`BootOrder` are non-authenticated, so no Secure
/// Boot keys are involved.
pub(crate) fn ensure_named_boot_entry(install_file_path: &str, description: &str) -> SelfInstall {
    // The partition handle LamBoot was loaded from.
    let image = uefi::boot::image_handle();
    let Ok(loaded) = uefi::boot::open_protocol_exclusive::<LoadedImage>(image) else {
        return SelfInstall::Failed;
    };
    let Some(device_handle) = loaded.device() else {
        return SelfInstall::NoDevicePath;
    };
    drop(loaded);

    let Ok(part_path) = uefi::boot::open_protocol_exclusive::<DevicePath>(device_handle) else {
        return SelfInstall::NoDevicePath;
    };

    // Build the load-option device path: the partition's nodes (minus its End
    // terminator) + a FilePath node for our binary. `finalize` re-appends End.
    let Ok(file_node_path) = CString16::try_from(install_file_path) else {
        return SelfInstall::Failed;
    };
    let mut dp_buf = Vec::new();
    let mut builder = DevicePathBuilder::with_vec(&mut dp_buf);
    for node in part_path.node_iter() {
        if node.device_type() == DeviceType::END {
            continue;
        }
        builder = match builder.push(&node) {
            Ok(b) => b,
            Err(_) => return SelfInstall::Failed,
        };
    }
    builder = match builder.push(&build::media::FilePath {
        path_name: &file_node_path,
    }) {
        Ok(b) => b,
        Err(_) => return SelfInstall::Failed,
    };
    let Ok(full_path) = builder.finalize() else {
        return SelfInstall::Failed;
    };
    let dp_bytes = full_path.as_bytes();

    // Idempotency scan: collect used slot numbers and any existing same-named entry.
    let mut used: Vec<u16> = Vec::new();
    let mut existing: Option<u16> = None;
    for num in 0u16..=BOOT_SCAN_MAX {
        let Ok(name) = CString16::try_from(boot_entry_pure::boot_var_name(num).as_str()) else {
            continue;
        };
        if let Ok((data, _)) = runtime::get_variable_boxed(&name, GLOBAL) {
            used.push(num);
            if existing.is_none()
                && boot_entry_pure::parse_load_option_description(&data).as_deref()
                    == Some(description)
            {
                existing = Some(num);
            }
        }
    }
    if let Some(num) = existing {
        return SelfInstall::AlreadyPresent(num);
    }

    let Some(num) = boot_entry_pure::first_free_boot_number(&used, BOOT_SCAN_MAX) else {
        return SelfInstall::Failed;
    };

    let option = boot_entry_pure::encode_load_option(
        boot_entry_pure::LOAD_OPTION_ACTIVE,
        description,
        dp_bytes,
        &[],
    );
    let Ok(boot_name) = CString16::try_from(boot_entry_pure::boot_var_name(num).as_str()) else {
        return SelfInstall::Failed;
    };
    if runtime::set_variable(&boot_name, GLOBAL, ATTRS, &option).is_err() {
        return SelfInstall::Failed;
    }

    // Front-load BootOrder. Non-fatal on failure: the entry exists and remains
    // selectable from the firmware boot menu even if the order write is refused.
    let order_bytes = runtime::get_variable_boxed(BOOT_ORDER, GLOBAL)
        .map(|(data, _)| data.to_vec())
        .unwrap_or_default();
    let order = boot_entry_pure::decode_boot_order(&order_bytes);
    let new_order = boot_entry_pure::boot_order_with_front(&order, num);
    let _ = runtime::set_variable(
        BOOT_ORDER,
        GLOBAL,
        ATTRS,
        &boot_entry_pure::encode_boot_order(&new_order),
    );

    SelfInstall::Created(num)
}
