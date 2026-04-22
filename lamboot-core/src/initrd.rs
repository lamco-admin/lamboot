//! Linux initrd loading via EFI LoadFile2 protocol.
//!
//! Implements the LINUX_EFI_INITRD_MEDIA_GUID mechanism used by the Linux
//! EFI stub (since kernel 5.7) to discover initrd. The bootloader registers
//! a LoadFile2 protocol provider on a handle with a VenMedia device path.
//! The kernel calls back with a two-phase buffer protocol:
//!   1. First call with NULL buffer → returns size (BUFFER_TOO_SMALL)
//!   2. Second call with allocated buffer → copies initrd data (SUCCESS)

use alloc::{boxed::Box, vec::Vec};
use core::ffi::c_void;

use uefi::{guid, prelude::*, Guid, Result};

/// LINUX_EFI_INITRD_MEDIA_GUID — identifies the initrd LoadFile2 provider
const LINUX_INITRD_MEDIA_GUID: Guid = guid!("5568e427-68fc-4f3d-ac74-ca555231cc68");

/// EFI_LOAD_FILE2_PROTOCOL_GUID
const LOAD_FILE2_GUID: Guid = guid!("4006c0c1-fcb3-403e-996d-4a6c8724e06d");

/// EFI_DEVICE_PATH_PROTOCOL_GUID
const DEVICE_PATH_GUID: Guid = guid!("09576e91-6d3f-11d2-8e39-00a0c969723b");

/// LoadFile2 protocol function table (must match UEFI spec layout)
#[repr(C)]
struct LoadFile2Proto {
    load_file: unsafe extern "efiapi" fn(
        this: *mut LoadFile2Proto,
        file_path: *const c_void,
        boot_policy: u8,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> uefi::Status,
}

/// Our initrd provider — holds the protocol vtable and the data pointer
#[repr(C)]
struct InitrdProvider {
    proto: LoadFile2Proto,
    data_ptr: *const u8,
    data_len: usize,
}

/// VenMedia device path node for LINUX_EFI_INITRD_MEDIA_GUID
/// Layout: [Type(1) SubType(1) Length(2)] [GUID(16)] [EndType(1) EndSubType(1) EndLength(2)]
#[repr(C, packed)]
struct InitrdDevicePath {
    // VenMedia node
    media_type: u8,        // 0x04 = MEDIA_DEVICE_PATH
    media_subtype: u8,     // 0x03 = MEDIA_VENDOR_DP
    media_length: [u8; 2], // 20 = 4 + 16
    vendor_guid: [u8; 16],
    // End node
    end_type: u8,        // 0x7F = END_DEVICE_PATH
    end_subtype: u8,     // 0xFF = END_ENTIRE
    end_length: [u8; 2], // 4
}

/// Handle to a registered initrd LoadFile2 provider.
/// Drop uninstalls the protocols and frees memory.
pub(crate) struct InitrdHandle {
    uefi_handle: Handle,
    provider_ptr: *mut InitrdProvider,
    devpath_ptr: *mut InitrdDevicePath,
    data_ptr: *mut [u8],
}

impl InitrdHandle {
    /// Register an initrd with the UEFI firmware via LoadFile2 protocol.
    ///
    /// The initrd data must remain valid until this handle is dropped.
    /// Uses Box::leak() internally to ensure stable memory addresses.
    pub(crate) fn register(initrd_data: Vec<u8>) -> Result<Self> {
        // Leak the data so it stays at a stable address
        let data_boxed: Box<[u8]> = initrd_data.into_boxed_slice();
        let data_len = data_boxed.len();
        let data_ptr = Box::into_raw(data_boxed);
        let data_slice_ptr = unsafe { (*data_ptr).as_ptr() };

        // Build the device path
        let guid_bytes = LINUX_INITRD_MEDIA_GUID.to_bytes();
        let devpath = Box::new(InitrdDevicePath {
            media_type: 0x04,
            media_subtype: 0x03,
            media_length: 20u16.to_le_bytes(),
            vendor_guid: guid_bytes,
            end_type: 0x7F,
            end_subtype: 0xFF,
            end_length: 4u16.to_le_bytes(),
        });
        let devpath_ptr = Box::into_raw(devpath);

        // Build the protocol provider
        let provider = Box::new(InitrdProvider {
            proto: LoadFile2Proto {
                load_file: initrd_load_file_callback,
            },
            data_ptr: data_slice_ptr,
            data_len,
        });
        let provider_ptr = Box::into_raw(provider);

        // Install DevicePathProtocol on a new handle
        let uefi_handle = unsafe {
            uefi::boot::install_protocol_interface(
                None,
                &DEVICE_PATH_GUID,
                devpath_ptr as *const c_void,
            )?
        };

        // Install LoadFile2Protocol on the same handle
        unsafe {
            uefi::boot::install_protocol_interface(
                Some(uefi_handle),
                &LOAD_FILE2_GUID,
                provider_ptr as *const c_void,
            )?;
        }

        log::info!("Registered initrd LoadFile2 provider ({data_len} bytes)");

        Ok(Self {
            uefi_handle,
            provider_ptr,
            devpath_ptr,
            data_ptr,
        })
    }
}

impl Drop for InitrdHandle {
    fn drop(&mut self) {
        // Uninstall in reverse order
        unsafe {
            let _ = uefi::boot::uninstall_protocol_interface(
                self.uefi_handle,
                &LOAD_FILE2_GUID,
                self.provider_ptr as *const c_void,
            );
            let _ = uefi::boot::uninstall_protocol_interface(
                self.uefi_handle,
                &DEVICE_PATH_GUID,
                self.devpath_ptr as *const c_void,
            );

            // Reclaim leaked memory
            drop(Box::from_raw(self.provider_ptr));
            drop(Box::from_raw(self.devpath_ptr));
            drop(Box::from_raw(self.data_ptr));
        }
        log::info!("Unregistered initrd LoadFile2 provider");
    }
}

/// LoadFile2 callback — called by the Linux kernel's EFI stub.
///
/// Two-phase protocol:
///   - buffer is NULL or too small → set buffer_size, return BUFFER_TOO_SMALL
///   - buffer is large enough → copy data, return SUCCESS
unsafe extern "efiapi" fn initrd_load_file_callback(
    this: *mut LoadFile2Proto,
    _file_path: *const c_void,
    boot_policy: u8,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> uefi::Status {
    // LoadFile2 must reject boot_policy = TRUE
    if boot_policy != 0 {
        return uefi::Status::UNSUPPORTED;
    }

    if this.is_null() || buffer_size.is_null() {
        return uefi::Status::INVALID_PARAMETER;
    }

    // SAFETY: `this` was verified non-null above. The pointer originates from
    // `Box::into_raw(provider)` in `register()`, where `InitrdProvider` is
    // #[repr(C)] with `proto` as the first field, so the cast is valid.
    let provider = unsafe { &*(this as *const InitrdProvider) };

    if provider.data_ptr.is_null() || provider.data_len == 0 {
        return uefi::Status::NOT_FOUND;
    }

    let required = provider.data_len;

    // Phase 1: size query — caller passes NULL buffer or too-small size
    // SAFETY: `buffer_size` was verified non-null above. The kernel's EFI stub
    // guarantees it points to a valid, writable usize.
    if buffer.is_null() || unsafe { *buffer_size } < required {
        unsafe { *buffer_size = required };
        return uefi::Status::BUFFER_TOO_SMALL;
    }

    // Phase 2: copy initrd data into kernel-provided buffer
    // SAFETY: The kernel allocated `buffer` with at least `*buffer_size` bytes
    // (verified >= required above). `provider.data_ptr` points to the leaked
    // Box<[u8]> from `register()` which remains valid until `InitrdHandle::drop`.
    unsafe {
        core::ptr::copy_nonoverlapping(provider.data_ptr, buffer.cast::<u8>(), required);
        *buffer_size = required;
    }
    uefi::Status::SUCCESS
}
