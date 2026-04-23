//! EFI filesystem driver loading.
//!
//! Loads .efi driver binaries from `\EFI\LamBoot\drivers\` on the ESP,
//! then reconnects all controllers so the firmware binds the new drivers
//! to block devices — making ext4/btrfs/ntfs partitions accessible via
//! SimpleFileSystem protocol.

use alloc::format;

use uefi::{boot::LoadImageSource, prelude::*, proto::media::fs::SimpleFileSystem};

use crate::{fs::Volume, security_override, tpm::TpmContext, trust_log};

const DRIVERS_DIR: &str = "/EFI/LamBoot/drivers";

/// Case-insensitive file extension check for no_std environments
fn has_extension_ignore_case(filename: &str, ext: &str) -> bool {
    let Some(dot_pos) = filename.rfind('.') else {
        return false;
    };
    filename[dot_pos + 1..].eq_ignore_ascii_case(ext)
}

/// Load all EFI drivers from the drivers directory on the ESP.
/// Returns the number of successfully loaded drivers.
/// Failures are logged but non-fatal — missing directory is normal (ESP-only mode).
pub(crate) fn load_drivers(
    image: Handle,
    esp: &mut Volume,
    tpm: &TpmContext,
    trust: &mut trust_log::TrustLog,
) -> usize {
    let Ok(filenames) = esp.read_dir_str(DRIVERS_DIR) else {
        log::info!("No drivers directory found (ESP-only mode)");
        return 0;
    };

    let driver_files: alloc::vec::Vec<_> = filenames
        .into_iter()
        .filter(|name| has_extension_ignore_case(name, "efi"))
        .collect();

    if driver_files.is_empty() {
        log::info!("No EFI drivers found in {DRIVERS_DIR}");
        return 0;
    }

    let mut loaded = 0;

    for filename in &driver_files {
        let path = format!("{DRIVERS_DIR}/{filename}");
        log::info!("Loading driver: {path}");

        match load_single_driver(image, esp, &path, tpm) {
            Ok(()) => {
                loaded += 1;
                log::info!("Driver loaded successfully: {filename}");
                trust.record(
                    trust_log::TrustEvent::new("driver_loaded")
                        .with_path(&path)
                        .with_verified_via("security_override")
                        .with_status(Status::SUCCESS),
                );
            }
            Err(e) => {
                log::warn!("Failed to load driver {filename}: {e:?}");
                trust.record(
                    trust_log::TrustEvent::new("driver_rejected")
                        .with_path(&path)
                        .with_verified_via("rejected")
                        .with_status(e.status()),
                );
            }
        }
    }

    if loaded > 0 {
        // Reconnect all controllers so firmware binds new drivers to devices
        reconnect_all_controllers();
        log::info!("Loaded {loaded} driver(s), reconnected controllers");
    }

    loaded
}

/// Load and start a single EFI driver binary
fn load_single_driver(
    parent_image: Handle,
    esp: &mut Volume,
    path: &str,
    tpm: &TpmContext,
) -> uefi::Result {
    // Read the driver file into memory
    let driver_data = esp.read_str(path)?;

    // Measure driver into TPM PCR 4 before execution
    tpm.measure_driver(&driver_data, path);

    // Under shim < v16 (Ubuntu 25.10, Debian 13, current Fedora), shim does
    // NOT hook BS->LoadImage. If we call load_image on a MOK-signed driver,
    // firmware's native SB check runs against db only and returns ACCESS_DENIED.
    // Install a temporary SecurityArchProtocol override that routes firmware's
    // image-authentication callbacks through ShimLock::Verify for the duration
    // of this LoadImage call. See src/security_override.rs for rationale and
    // systemd/systemd#38624 for the canonical precedent.
    //
    // The guard uninstalls on drop, keeping the override window as narrow as
    // possible.
    let _sb_override = security_override::install_security_override();

    // Load the driver image
    let driver_handle = uefi::boot::load_image(
        parent_image,
        LoadImageSource::FromBuffer {
            buffer: &driver_data,
            file_path: None,
        },
    )?;

    // Start the driver — it will register its DriverBinding protocol
    uefi::boot::start_image(driver_handle)?;

    Ok(())
}

/// Reconnect all controllers in the system.
/// This triggers the firmware to match newly loaded drivers to devices,
/// causing new SimpleFileSystem handles to appear for supported partitions.
fn reconnect_all_controllers() {
    let Ok(handles) = uefi::boot::find_handles::<SimpleFileSystem>() else {
        return;
    };

    // ConnectController with recursive=true on each handle
    for handle in handles {
        let _ = uefi::boot::connect_controller(handle, None, None, true);
    }

    // Also try to connect any block device handles that don't have
    // SimpleFileSystem yet — they might now match a loaded driver.
    // find_handles with a broad protocol type catches most devices.
    if let Ok(all_handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::AllHandles) {
        for &handle in all_handles.iter() {
            let _ = uefi::boot::connect_controller(handle, None, None, true);
        }
    }
}
