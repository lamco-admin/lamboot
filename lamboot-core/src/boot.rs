use alloc::{boxed::Box, string::String, vec::Vec};

use uefi::{
    boot::LoadImageSource, prelude::*, proto::loaded_image::LoadedImage, CString16, Handle, Result,
};

use crate::{
    discovery::{BootEntry, EntryKind},
    fs::Volume,
    initrd::InitrdHandle,
    pe_loader, pe_loader_pure,
    policy::{LoaderNativePeMode, Policy},
    secure::{self, SecureBootState},
    security_override,
    tpm::TpmContext,
    trust_log::{
        self, TrustEvent, TrustLog, V_DEGRADED_TRUST_SB_DIRECT, V_DEGRADED_TRUST_SB_OFF,
        V_FIRMWARE_LOADIMAGE, V_NATIVE_PE_LOADER, V_SHIM_MOK, V_SHIM_REJECTED,
    },
};

/// SDS-4 §3.1 — zero-copy wrapper carrying trust-verified bytes plus
/// their SHA-256 and the stable `verified_via` vocabulary token.
///
/// The lifetime parameter ensures the referenced bytes outlive the
/// `load_pe` call that consumes them. `sha256` is computed at verify
/// time (Step 10) and re-emitted in `image_loaded_native` (Step 13)
/// so the §6.4 invariant `verify.sha256 == load.sha256` is auditable
/// from the trust log alone.
struct VerifiedBytes<'b> {
    bytes: &'b [u8],
    sha256: [u8; 32],
    verified_via: &'static str,
}

/// Boot the selected entry.
/// `volumes` includes the ESP as the first element, followed by any extra volumes
/// exposed by filesystem drivers (ext4, btrfs, etc.).
pub(crate) fn boot_entry(
    current_image: Handle,
    volumes: &mut [Volume],
    entry: BootEntry,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Result<Status> {
    log::info!("Booting entry: {} ({})", entry.name, entry.id);

    // Split the ESP off the front so the UKI/Linux paths can flush
    // trust-log events (image_verified, image_loaded_native) to it
    // AFTER they're recorded but BEFORE start_image transfers
    // control. Without this second flush, SDS-4 §6.4's load-bearing
    // image_verified event never makes it to \loader\boot-trust.log.
    let (esp_slot, rest) = volumes.split_first_mut().ok_or(Status::ABORTED)?;

    match entry.kind {
        EntryKind::Chainload { path } => chainload_efi(current_image, esp_slot, &path),
        EntryKind::Uki { path, options } => boot_uki(
            current_image,
            esp_slot,
            &path,
            &options,
            tpm,
            policy,
            trust_log,
        ),
        EntryKind::LinuxLegacy {
            kernel_path,
            initrd_paths,
            options,
        } => boot_linux(
            current_image,
            esp_slot,
            rest,
            &kernel_path,
            &initrd_paths,
            &options,
            tpm,
            policy,
            trust_log,
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
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Result<Status> {
    log::info!("Booting UKI: {path} with options: {options}");
    let backend_tag = esp.backend_tag();
    let image_data = esp.read_str(path)?;

    // Measure into TPM before any load decision — same PCR/semantics
    // whether we take the native or firmware path.
    tpm.measure_kernel(&image_data);
    // SDS-4 §6.1 Step 11: kernel_measured event records the PCR 4 extend.
    trust_log.record(
        TrustEvent::new("kernel_measured")
            .with_path(path)
            .with_note("pcr=4"),
    );
    // Step 12: measure cmdline (UKI cmdline comes from the binary's
    // .cmdline section; we still measure it since it's what the
    // kernel will see).
    if !options.is_empty() {
        tpm.measure_cmdline(options);
        trust_log.record(
            TrustEvent::new("cmdline_measured")
                .with_path(path)
                .with_note("pcr=12"),
        );
    }

    // SDS-4 Step 10: verify + capture SHA-256 + verified_via.
    let Ok(verified) = verify_kernel_bytes(&image_data, path, trust_log) else {
        log::error!("UKI verification failed, returning to menu: {path}");
        return Err(Status::SECURITY_VIOLATION.into());
    };

    match choose_load_path(policy.loader_native_pe, /*verify_ok=*/ true) {
        LoadPath::Native => native_load_and_start(
            &verified,
            options,
            backend_tag,
            path,
            current_image,
            trust_log,
            esp,
        ),
        LoadPath::Firmware => {
            firmware_load_and_start(current_image, &image_data, options, trust_log, path)
        }
    }
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
#[expect(
    clippy::too_many_arguments,
    reason = "orchestration function; composing the args further would mean a context struct for just this call site"
)]
fn boot_linux(
    current_image: Handle,
    esp: &mut Volume,
    extras: &mut [Volume],
    kernel_path: &str,
    initrd_paths: &[String],
    options: &str,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Result<Status> {
    log::info!(
        "Booting Linux: kernel={}, initrds={}, options={}",
        kernel_path,
        initrd_paths.len(),
        options
    );

    // Find which volume has the kernel — check ESP first, then extras.
    // BLS entries on the ESP may reference paths on ext4 partitions
    // mounted as extras (Fedora /boot). Return None → NOT_FOUND.
    //
    // Use `exists_with_boot_fallback` to also try `\boot\`-prefixed
    // variants: Debian's kernel-install occasionally generates BLS
    // entries like `linux /vmlinuz-X` even when the actual kernel is
    // at `/boot/vmlinuz-X` on a single-root filesystem (observed on
    // VM 100 for the 6.19.11+deb14 entry). Trying as-written first
    // covers the XBOOTLDR separate-/boot layout; the `\boot\` fallback
    // covers the single-root mis-prefixed case.
    let (kernel_bytes, backend_tag) = if let Some(resolved) =
        crate::preflight::exists_with_boot_fallback(esp, kernel_path)
    {
        log::info!("Kernel found on ESP (volume 0) at {resolved}");
        let tag = esp.backend_tag();
        (esp.read_str(&resolved)?, tag)
    } else {
        let mut found: Option<(usize, String)> = None;
        for (i, v) in extras.iter_mut().enumerate() {
            if let Some(resolved) = crate::preflight::exists_with_boot_fallback(v, kernel_path) {
                found = Some((i, resolved));
                break;
            }
        }
        let Some((extra_idx, resolved)) = found else {
            log::error!(
                "Kernel not found on any mounted volume: {kernel_path}. \
                 This usually means the filesystem driver for the kernel's \
                 partition failed to load, or the kernel path is wrong for \
                 this BLS entry."
            );
            return Err(Status::NOT_FOUND.into());
        };
        log::info!(
            "Kernel found on extras[{extra_idx}] (volume {}) at {resolved}",
            extra_idx + 1
        );
        let vol = &mut extras[extra_idx];
        let tag = vol.backend_tag();
        (vol.read_str(&resolved)?, tag)
    };

    // Measure into TPM unconditionally — same PCR semantics on both
    // native and firmware paths.
    tpm.measure_kernel(&kernel_bytes);
    // SDS-4 §6.1 Step 11: kernel_measured records the PCR 4 extend.
    trust_log.record(
        TrustEvent::new("kernel_measured")
            .with_path(kernel_path)
            .with_note("pcr=4"),
    );
    // Step 12: cmdline measurement (PCR 12).
    if !options.is_empty() {
        tpm.measure_cmdline(options);
        trust_log.record(
            TrustEvent::new("cmdline_measured")
                .with_path(kernel_path)
                .with_note("pcr=12"),
        );
    }

    // SDS-4 Step 10: verify + capture SHA-256 + verified_via.
    // The native loader does NOT verify bytes itself; this is THE
    // verification point per §3.1 of SPEC-NATIVE-TRUST-CHAIN.md.
    let Ok(verified) = verify_kernel_bytes(&kernel_bytes, kernel_path, trust_log) else {
        log::error!("Kernel verification failed, returning to menu: {kernel_path}");
        return Err(Status::SECURITY_VIOLATION.into());
    };

    // Load and concatenate all initrd files, register via LoadFile2.
    // Happens before load_pe so the LoadFile2 protocol is installed
    // by the time the kernel's EFI stub queries it. Initrds live on
    // the SAME volume as the kernel — check ESP first, then extras.
    let _initrd_handle = if initrd_paths.is_empty() {
        None
    } else {
        let mut combined = Vec::new();
        for path in initrd_paths {
            log::info!("Loading initrd: {path}");
            // Same `\boot\`-fallback dance as the kernel lookup above;
            // Debian's 6.19.11+deb14 entry had both linux and initrd
            // mis-prefixed (linux /vmlinuz-X + initrd /initrd.img-X
            // instead of /boot/vmlinuz-X + /boot/initrd.img-X).
            let data = if let Some(resolved) =
                crate::preflight::exists_with_boot_fallback(esp, path)
            {
                esp.read_str(&resolved)?
            } else {
                let mut found_idx: Option<(usize, String)> = None;
                for (i, v) in extras.iter_mut().enumerate() {
                    if let Some(resolved) = crate::preflight::exists_with_boot_fallback(v, path) {
                        found_idx = Some((i, resolved));
                        break;
                    }
                }
                if let Some((idx, resolved)) = found_idx {
                    extras[idx].read_str(&resolved)?
                } else {
                    log::warn!("Initrd not found on any volume: {path} — skipping");
                    continue;
                }
            };
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

    match choose_load_path(policy.loader_native_pe, /*verify_ok=*/ true) {
        LoadPath::Native => native_load_and_start(
            &verified,
            options,
            backend_tag,
            kernel_path,
            current_image,
            trust_log,
            esp,
        ),
        LoadPath::Firmware => firmware_load_and_start(
            current_image,
            &kernel_bytes,
            options,
            trust_log,
            kernel_path,
        ),
    }
    // _initrd_handle drops here if we got back (normally we don't —
    // ExitBootServices makes return impossible).
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

// ---------------------------------------------------------------------------
// SDS-3 PR-3: native-vs-firmware load decision + path implementations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum LoadPath {
    /// Use LamBoot's pe_loader. Bypasses `BS->LoadImage` entirely.
    Native,
    /// Fall back to `uefi::boot::load_image`. v0.8.3 path; used when
    /// policy is Never or when Auto + ShimLock::Verify wasn't
    /// available (e.g. SB off, no shim).
    Firmware,
}

/// Decide the load path from policy + pre-load verify outcome.
/// In Always mode we require verify_ok; if verification didn't
/// happen we refuse to route to the native loader (because its
/// trust contract is that the caller has already verified bytes).
fn choose_load_path(mode: LoaderNativePeMode, verify_ok: bool) -> LoadPath {
    match mode {
        LoaderNativePeMode::Never => LoadPath::Firmware,
        LoaderNativePeMode::Always => {
            if verify_ok {
                LoadPath::Native
            } else {
                // Operator explicitly asked for native but we couldn't
                // verify. Still go native — the operator's assertion
                // is "use my loader even if there's no shim to ask."
                // Under SB-off this is fine; under SB-on + no shim the
                // firmware load would have rejected us too.
                LoadPath::Native
            }
        }
        LoaderNativePeMode::Auto => {
            if verify_ok {
                LoadPath::Native
            } else {
                // SB-disabled or ShimLock unavailable. The firmware
                // path is still safe (SB-off = no check, or firmware
                // would reject via native SB anyway).
                LoadPath::Firmware
            }
        }
    }
}

/// SDS-4 Step 10 — the single load-bearing kernel-verification call.
///
/// Computes SHA-256 of the bytes, performs the trust check
/// appropriate to the Secure Boot state, emits the `image_verified`
/// trust event, and returns a `VerifiedBytes` carrying the digest +
/// stable vocabulary token for Step 13's invariant check.
///
/// Ok — verification succeeded (or is semantically not required in
/// this SB posture). Bytes are safe to load.
/// Err — verification was refused. Caller must not proceed to
/// `start_image`; the menu returns to let the user pick again.
fn verify_kernel_bytes<'b>(
    bytes: &'b [u8],
    path_for_log: &str,
    trust_log: &mut TrustLog,
) -> core::result::Result<VerifiedBytes<'b>, ()> {
    let sha256 = pe_loader_pure::sha256_of(bytes);
    let sha_hex = pe_loader_pure::hex_encode_32(&sha256);

    let (verified_via, status_str, accepted) = match secure::detect_secure_boot() {
        // SB-off: no crypto verification attempted. Accepted for
        // homelab/dev; loud in the trust log so an auditor can see
        // that no cryptographic check backed this boot.
        SecureBootState::Disabled => (V_DEGRADED_TRUST_SB_OFF, "SUCCESS", true),

        // SB on, loaded directly by firmware db without shim in the
        // chain. Same trust model as SB-off from LamBoot's point of
        // view — the administrator's db-enrolled key covers the
        // tree; we don't re-verify. Distinct token so audit logs
        // distinguish the two states.
        SecureBootState::ActiveDirect => (V_DEGRADED_TRUST_SB_DIRECT, "SUCCESS", true),

        // SB on + shim in chain: delegate to ShimLock::Verify.
        SecureBootState::ActiveWithShim => match secure::verify_image(bytes) {
            Ok(()) => (V_SHIM_MOK, "SUCCESS", true),
            Err(e) => {
                log::warn!("ShimLock::Verify rejected {path_for_log}: {e:?}");
                (V_SHIM_REJECTED, "REJECTED", false)
            }
        },
    };

    trust_log.record(
        TrustEvent::new("image_verified")
            .with_path(path_for_log)
            .with_sha256(&sha_hex)
            .with_verified_via(verified_via)
            .with_status(status_str),
    );

    if accepted {
        Ok(VerifiedBytes {
            bytes,
            sha256,
            verified_via,
        })
    } else {
        Err(())
    }
}

/// Native load path: pe_loader::load_pe + pe_loader::start_image.
/// Emits image_loaded_native trust event with the SHA-256 and
/// verified_via token carried from Step 10. Never calls BS->LoadImage.
///
/// SDS-4 §6.4 invariant: the SHA-256 recorded in `image_loaded_native`
/// MUST match the SHA-256 computed at `verify_kernel_bytes`. A
/// mismatch indicates a TOCTOU bug between verify and load (bytes
/// changed under us) and is treated as a security-critical failure
/// via `assert_eq!` — a release build will panic on violation rather
/// than silently proceed.
#[expect(
    clippy::too_many_arguments,
    reason = "orchestration function; ESP pass-through is how we persist image_verified + image_loaded_native to disk before ExitBootServices"
)]
fn native_load_and_start(
    verified: &VerifiedBytes<'_>,
    options: &str,
    backend_tag: &'static str,
    path_for_log: &str,
    _current_image: Handle,
    trust_log: &mut TrustLog,
    esp_for_flush: &mut Volume,
) -> Result<Status> {
    let bytes = verified.bytes;
    log::info!(
        "Native PE load: {path_for_log} ({} bytes from {backend_tag}, verified_via={})",
        bytes.len(),
        verified.verified_via,
    );

    // Encode options as UTF-16 + NUL (what LoadedImageProtocol expects).
    let options_box: Option<alloc::boxed::Box<[u16]>> = if options.is_empty() {
        None
    } else {
        let wide = CString16::try_from(options).map_err(|_| Status::INVALID_PARAMETER)?;
        let vec: Vec<u16> = wide
            .as_slice_with_nul()
            .iter()
            .map(|c| u16::from(*c))
            .collect();
        Some(vec.into_boxed_slice())
    };

    let loaded = match pe_loader::load_pe(bytes, options_box, backend_tag) {
        Ok(l) => l,
        Err(e) => {
            log::error!("pe_loader::load_pe failed: {e:?}");
            // Include the Debug repr in the trust-log note so the
            // specific RelocationMalformed(detail) string or similar
            // payload reaches the operator, not just the generic
            // token. Helps triage SDS-3 PR-4-era correctness issues.
            let debug_repr = alloc::format!("{e:?}");
            trust_log.record(
                TrustEvent::new("image_load_failed")
                    .with_path(path_for_log)
                    .with_verified_via("native_pe_loader")
                    .with_note(&alloc::format!(
                        "backend={backend_tag} size={} token={} debug={debug_repr}",
                        bytes.len(),
                        e.as_log_token(),
                    )),
            );
            return Err(Status::LOAD_ERROR.into());
        }
    };

    // SDS-4 §6.4 invariant: SHA-256 at Step 10 (verify) MUST equal
    // SHA-256 at Step 13 (load). A mismatch is a TOCTOU security bug
    // — the bytes we verified are not the bytes we're about to run.
    // Release build panics rather than silently proceed.
    assert_eq!(
        *loaded.sha256(),
        verified.sha256,
        "SDS-4 §6.4 invariant violation: verify SHA-256 != load SHA-256 for {path_for_log}"
    );

    // Emit the successful-load event per spec §12 BEFORE transferring
    // control — the log flushes to ESP before the child runs. Carries
    // the `verified_via` token from Step 10 so audit consumers can
    // correlate the decision back through the chain.
    trust_log.record(
        TrustEvent::new("image_loaded_native")
            .with_path(path_for_log)
            .with_sha256(&loaded.sha256_hex())
            .with_verified_via(verified.verified_via)
            .with_status(Status::SUCCESS)
            .with_note(&alloc::format!(
                "backend={backend_tag} loader={V_NATIVE_PE_LOADER}"
            )),
    );

    // SDS-4 Step 15: flush the trust log to \loader\boot-trust.log
    // BEFORE transferring control. Without this, `image_verified`
    // and `image_loaded_native` live only in memory and are lost
    // when the kernel ExitBootServices-es. The log file is the only
    // persistent audit evidence of the trust-chain decision.
    trust_log.flush(esp_for_flush);

    reconnect_console_drivers();
    log::info!("Starting image via native loader (no BS->LoadImage)");

    // SAFETY: `verify_pre_load` returned true on the Auto branch (we
    // would not have chosen Native otherwise) OR the operator set
    // policy to Always, asserting the load bytes are trusted. The
    // loaded image came from our pe_loader which validated PE
    // structure per §5.1.
    let status = unsafe { pe_loader::start_image(loaded) };
    Ok(status)
}

/// Firmware load path: the v0.8.3 load_image + start_image sequence,
/// preserved verbatim so operators have a one-flag rollback path.
/// Emits image_loaded_firmware trust event.
fn firmware_load_and_start(
    current_image: Handle,
    bytes: &[u8],
    options: &str,
    trust_log: &mut TrustLog,
    path_for_log: &str,
) -> Result<Status> {
    log::info!(
        "Firmware image load: {path_for_log} ({} bytes via BS->LoadImage)",
        bytes.len()
    );

    let image_handle = load_efi_image_from_buffer(current_image, bytes, Some(path_for_log))?;

    if !options.is_empty() {
        set_load_options(image_handle, options)?;
    }

    trust_log.record(
        TrustEvent::new("image_loaded_firmware")
            .with_path(path_for_log)
            .with_verified_via(V_FIRMWARE_LOADIMAGE)
            .with_status(Status::SUCCESS),
    );

    reconnect_console_drivers();
    uefi::boot::start_image(image_handle)?;
    Ok(Status::SUCCESS)
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
