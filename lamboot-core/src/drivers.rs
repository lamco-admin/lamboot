//! EFI filesystem driver loading.
//!
//! Loads .efi driver binaries from `\EFI\LamBoot\drivers\` on the ESP,
//! then reconnects all controllers so the firmware binds the new drivers
//! to block devices — making ext4/btrfs/ntfs partitions accessible via
//! SimpleFileSystem protocol.
//!
//! SDS-6 (v0.9.0+) gates loading per-driver against native-backend
//! coverage. When LamBoot compiles in a native backend for a filesystem
//! (ext4/ext2 via `Ext4Backend`, FAT via UEFI-native `SimpleFileSystem`),
//! loading the corresponding UEFI FS driver is redundant and
//! security-wise undesirable — each `LoadImage`+`StartImage` of a UEFI
//! FS driver triggers shim 15.8 to uninstall `ShimLock`, the root cause
//! of the v0.8.3 Secure Boot failure mode. Default `Auto` mode skips
//! natively-covered drivers; `Always` restores v0.8.3 behavior for
//! debugging; `Never` blocks all driver loads for security-conscious
//! installs.

use alloc::format;

use uefi::{boot::LoadImageSource, prelude::*, proto::media::fs::SimpleFileSystem};

use crate::{fs::Volume, policy::DriversLegacyMode, security_override, tpm::TpmContext, trust_log};

const DRIVERS_DIR: &str = "/EFI/LamBoot/drivers";

/// Is the filesystem that this driver filename targets already covered
/// by a native-backend compiled into LamBoot?
///
/// Post-SDS-2, `Ext4Backend` is unconditionally compiled in, so ext4
/// and ext2 drivers are always natively covered. Post-SDS-1, FAT is
/// UEFI-native (no third-party driver ever needed). When v1.1+ adds
/// a compile-time-gated Btrfs backend, this match gets a `#[cfg]` arm.
///
/// Filename matching is case-insensitive and tolerates both the
/// `_x64.efi` (x86_64) and `_aa64.efi` (aarch64) suffixes.
fn filesystem_natively_covered(driver_filename: &str) -> bool {
    let lower = driver_filename.to_lowercase();
    // SDS-2: ext4-view covers ext2 + ext3 + ext4 via one backend.
    if lower.starts_with("ext4_") || lower.starts_with("ext2_") || lower.starts_with("ext3_") {
        return true;
    }
    // FAT is always UEFI-native; no driver needed (no fat_x64.efi ships
    // in our tarball today, but guard anyway).
    if lower.starts_with("fat_") || lower.starts_with("vfat_") {
        return true;
    }
    // btrfs/xfs/zfs/ntfs/f2fs/iso9660 — no native backend in v0.9.x.
    false
}

/// Case-insensitive file extension check for no_std environments
fn has_extension_ignore_case(filename: &str, ext: &str) -> bool {
    let Some(dot_pos) = filename.rfind('.') else {
        return false;
    };
    filename[dot_pos + 1..].eq_ignore_ascii_case(ext)
}

/// Load EFI filesystem drivers from the drivers directory on the ESP,
/// gated by SDS-6 policy + per-driver native-backend coverage.
///
/// Returns the number of successfully loaded drivers. Skipped drivers
/// (native coverage, or policy=Never) do NOT count toward the return
/// value but DO emit per-skip trust events so the operator can see
/// exactly which paths fired.
///
/// Failures are logged but non-fatal — missing directory is normal
/// (ESP-only mode, install with `--with-drivers-legacy=none`).
pub(crate) fn load_drivers(
    image: Handle,
    esp: &mut Volume,
    tpm: &TpmContext,
    trust: &mut trust_log::TrustLog,
    policy_mode: DriversLegacyMode,
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
    let mut skipped_covered = 0;
    let mut skipped_policy = 0;

    for filename in &driver_files {
        let path = format!("{DRIVERS_DIR}/{filename}");

        // SDS-6 gating decision. Three modes:
        match policy_mode {
            DriversLegacyMode::Never => {
                skipped_policy += 1;
                log::info!("Skipping {filename}: policy legacy_uefi_drivers=\"never\"");
                trust.record(
                    trust_log::TrustEvent::new("legacy_driver_skipped_policy")
                        .with_path(&path)
                        .with_note("policy=never"),
                );
                continue;
            }
            DriversLegacyMode::Auto => {
                if filesystem_natively_covered(filename) {
                    skipped_covered += 1;
                    log::info!("Skipping {filename}: native backend covers this filesystem");
                    trust.record(
                        trust_log::TrustEvent::new("native_backend_preferred")
                            .with_path(&path)
                            .with_note(
                                "ext4/ext2 handled by ext4-view (SDS-2); skipping legacy driver",
                            ),
                    );
                    trust.record(
                        trust_log::TrustEvent::new("legacy_driver_skipped_covered")
                            .with_path(&path)
                            .with_note("mode=auto native_backend=compiled_in"),
                    );
                    continue;
                }
            }
            DriversLegacyMode::Always => {
                // Fall through — v0.8.3 behavior. Emit a note so the
                // trust log distinguishes "no native coverage" from
                // "operator asked for legacy anyway".
                if filesystem_natively_covered(filename) {
                    trust.record(
                        trust_log::TrustEvent::new("legacy_driver_redundant")
                            .with_path(&path)
                            .with_note(
                                "mode=always; native backend covers this filesystem but driver loaded anyway",
                            ),
                    );
                }
            }
        }

        log::info!("Loading driver: {path}");

        match load_single_driver(image, esp, &path, tpm) {
            Ok(()) => {
                loaded += 1;
                log::info!("Driver loaded successfully: {filename}");
                trust.record(
                    trust_log::TrustEvent::new("legacy_driver_loaded")
                        .with_path(&path)
                        .with_verified_via("security_override")
                        .with_status(Status::SUCCESS),
                );
            }
            Err(e) => {
                log::warn!("Failed to load driver {filename}: {e:?}");
                trust.record(
                    trust_log::TrustEvent::new("legacy_driver_rejected")
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
    if skipped_covered > 0 || skipped_policy > 0 {
        log::info!(
            "Driver load summary: {loaded} loaded, {skipped_covered} skipped (native-covered), {skipped_policy} skipped (policy)",
        );
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
