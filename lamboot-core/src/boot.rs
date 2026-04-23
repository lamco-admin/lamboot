use alloc::{boxed::Box, string::String, vec::Vec};

use uefi::{
    boot::LoadImageSource, prelude::*, proto::loaded_image::LoadedImage, CString16, Handle, Result,
};

use crate::{
    discovery::{BootEntry, EntryKind},
    fs::Volume,
    initrd::InitrdHandle,
    security_override,
    tpm::TpmContext,
};

/// Boot the selected entry.
/// `volumes` includes the ESP as the first element, followed by any extra volumes
/// exposed by filesystem drivers (ext4, btrfs, etc.).
pub(crate) fn boot_entry(
    current_image: Handle,
    volumes: &mut [Volume],
    entry: BootEntry,
    tpm: &TpmContext,
) -> Result<Status> {
    log::info!("Booting entry: {} ({})", entry.name, entry.id);

    match entry.kind {
        EntryKind::Chainload { path } => chainload_efi(current_image, &mut volumes[0], &path),
        EntryKind::Uki { path, options } => {
            boot_uki(current_image, &mut volumes[0], &path, &options, tpm)
        }
        EntryKind::LinuxLegacy {
            kernel_path,
            initrd_paths,
            options,
        } => boot_linux(
            current_image,
            volumes,
            &kernel_path,
            &initrd_paths,
            &options,
            tpm,
        ),
    }
}

/// Chainload another EFI application
fn chainload_efi(current_image: Handle, esp: &mut Volume, path: &str) -> Result<Status> {
    log::info!("Chainloading: {path}");
    let image_data = esp.read_str(path)?;
    log::info!("Read {} bytes", image_data.len());
    let image_handle = load_efi_image_from_buffer(current_image, &image_data, None)?;

    reconnect_console_drivers();
    log::info!("Starting chainloaded image...");
    uefi::boot::start_image(image_handle)?;

    // Child image returned — caller should re-enter the menu
    log::info!("Chainloaded image returned");
    Ok(Status::SUCCESS)
}

/// Boot a Unified Kernel Image (UKI)
fn boot_uki(
    current_image: Handle,
    esp: &mut Volume,
    path: &str,
    options: &str,
    tpm: &TpmContext,
) -> Result<Status> {
    log::info!("Booting UKI: {path} with options: {options}");
    let image_handle = load_efi_image(current_image, esp, path, tpm)?;

    if !options.is_empty() {
        set_load_options(image_handle, options)?;
    }

    reconnect_console_drivers();
    log::info!("Starting UKI...");
    uefi::boot::start_image(image_handle)?;

    Ok(Status::SUCCESS)
}

/// Boot Linux kernel with initrd via LoadFile2 protocol.
///
/// This is the proper mechanism since Linux kernel 5.7:
/// 1. Load kernel as an EFI image (works for EFI stub kernels)
/// 2. Register initrd via LoadFile2 protocol (LINUX_EFI_INITRD_MEDIA_GUID)
/// 3. Set kernel command line via load options
/// 4. Start the kernel image
/// 5. Kernel's EFI stub discovers initrd via LoadFile2 callback
///
/// Multiple initrd files (e.g. microcode + main initramfs) are concatenated
/// into a single buffer — the kernel splits them apart internally.
fn boot_linux(
    current_image: Handle,
    volumes: &mut [Volume],
    kernel_path: &str,
    initrd_paths: &[String],
    options: &str,
    tpm: &TpmContext,
) -> Result<Status> {
    log::info!(
        "Booting Linux: kernel={}, initrds={}, options={}",
        kernel_path,
        initrd_paths.len(),
        options
    );

    // Find which volume has the kernel — search ESP first, then extra volumes.
    // BLS entries on the ESP may reference paths on ext4/btrfs partitions
    // exposed by filesystem drivers.
    let Some(kernel_vol) = find_volume_for_path(volumes, kernel_path) else {
        log::error!(
            "Kernel not found on any mounted volume: {kernel_path}. \
             This usually means the filesystem driver for the kernel's \
             partition failed to load, or the kernel path is wrong for \
             this BLS entry."
        );
        return Err(Status::NOT_FOUND.into());
    };
    log::info!("Kernel found on volume {kernel_vol}");
    let vol = &mut volumes[kernel_vol];

    // Load the kernel image
    let image_handle = load_efi_image(current_image, vol, kernel_path, tpm)?;

    // Load and concatenate all initrd files, register via LoadFile2.
    // Initrds are expected on the same volume as the kernel.
    let _initrd_handle = if initrd_paths.is_empty() {
        None
    } else {
        // Re-borrow the volume (may have been released by load_efi_image)
        let vol = &mut volumes[kernel_vol];
        let mut combined = Vec::new();
        for path in initrd_paths {
            log::info!("Loading initrd: {path}");
            let data = vol.read_str(path)?;
            log::info!("  {} bytes", data.len());
            combined.extend_from_slice(&data);
        }
        log::info!("Total initrd size: {} bytes", combined.len());
        match InitrdHandle::register(combined) {
            Ok(handle) => Some(handle),
            Err(e) => {
                log::warn!("Failed to register initrd via LoadFile2: {e:?}");
                None
            }
        }
    };

    // Set kernel command line
    if !options.is_empty() {
        set_load_options(image_handle, options)?;
    }

    // Start the kernel — it will discover initrd via LoadFile2
    reconnect_console_drivers();
    log::info!("Starting Linux kernel...");
    uefi::boot::start_image(image_handle)?;

    // _initrd_handle drops here, unregistering the LoadFile2 protocol
    // (only reached if kernel returns, which normally doesn't happen)
    Ok(Status::SUCCESS)
}

/// Search all volumes for a file path. Returns the index of the volume
/// where the file exists, or None if absent from every volume. Callers must
/// handle the None case — a misleading fallback to volume 0 hid real bugs
/// where kernel-on-ext4 lookups silently redirected to the FAT ESP and failed
/// later with an opaque NOT_FOUND from read_to_vec.
fn find_volume_for_path(volumes: &mut [Volume], path: &str) -> Option<usize> {
    for (i, vol) in volumes.iter_mut().enumerate() {
        if vol.exists_str(path) {
            return Some(i);
        }
    }
    None
}

/// Load an EFI image from ESP with Secure Boot verification and TPM measurement
fn load_efi_image(
    parent_image: Handle,
    esp: &mut Volume,
    path: &str,
    tpm: &TpmContext,
) -> Result<Handle> {
    let image_data = esp.read_str(path)?;

    // Measure kernel image into TPM PCR 4
    tpm.measure_kernel(&image_data);

    load_efi_image_from_buffer(parent_image, &image_data, Some(path))
}

/// Load an EFI image from an in-memory buffer.
///
/// Under shim < v16, firmware's native `BS->LoadImage` rejects MOK-signed
/// images (including UKIs signed by distro keys enrolled only in MOK, and
/// user-signed kernels) with EFI_ACCESS_DENIED. The SecurityOverride guard
/// temporarily replaces firmware's image-authentication protocols with hooks
/// that delegate to ShimLock::Verify — mirroring systemd-boot's pattern. The
/// guard uninstalls on drop, keeping the override scoped to this single
/// LoadImage call. A no-op when SB is disabled or shim isn't present.
fn load_efi_image_from_buffer(
    parent_image: Handle,
    image_data: &[u8],
    _file_path: Option<&str>,
) -> Result<Handle> {
    log::info!("Loading image from memory: {} bytes", image_data.len());

    let _sb_override = security_override::install_security_override();

    let image_handle = uefi::boot::load_image(
        parent_image,
        LoadImageSource::FromBuffer {
            buffer: image_data,
            file_path: None,
        },
    )?;

    Ok(image_handle)
}

/// Set load options (kernel command line) for a loaded image.
///
/// The CString16 is intentionally leaked — UEFI's LoadedImage stores a pointer
/// to the options buffer, so it must remain valid through `start_image`. The
/// leaked memory is reclaimed when the firmware calls `ExitBootServices`.
fn set_load_options(image_handle: Handle, options: &str) -> Result {
    let options_wide = CString16::try_from(options).map_err(|_| Status::INVALID_PARAMETER)?;

    // Leak into a stable heap allocation that outlives this function
    let leaked: &'static [uefi::Char16] =
        Box::leak(options_wide.as_slice_with_nul().to_vec().into_boxed_slice());
    let options_ptr = leaked.as_ptr().cast::<u8>();
    let options_size = (leaked.len() * 2) as u32;

    let mut loaded_image = uefi::boot::open_protocol_exclusive::<LoadedImage>(image_handle)?;

    // SAFETY: `options_ptr` points to leaked heap memory that remains valid
    // through `start_image` and until `ExitBootServices` reclaims the pool.
    // `options_size` is the exact byte length of the UTF-16 buffer with NUL.
    unsafe {
        loaded_image.set_load_options(options_ptr, options_size);
    }

    log::info!("Set load options: {options}");
    Ok(())
}

/// Reconnect the GraphicsConsole driver to the GOP handle.
///
/// The GUI opens GOP with `open_protocol_exclusive` which disconnects
/// OVMF's GraphicsConsole driver. Dropping the exclusive handle releases
/// the lock but does NOT reconnect the driver. Without reconnection,
/// ConOut text rendering is invisible — child images appear to hang.
fn reconnect_console_drivers() {
    if let Ok(gop_handle) =
        uefi::boot::get_handle_for_protocol::<uefi::proto::console::gop::GraphicsOutput>()
    {
        let _ = uefi::boot::connect_controller(gop_handle, None, None, true);
    }
}
