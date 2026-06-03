//! Layer: 7 — Orchestration.

use alloc::{boxed::Box, string::String, vec::Vec};

use uefi::{
    boot::LoadImageSource, prelude::*, proto::loaded_image::LoadedImage, CString16, Handle, Result,
};

use crate::{
    boot_types::{BootEntry, EntryKind},
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
    /// Secure Boot posture observed at verify time. Carried (rather than
    /// re-detected) so `choose_load_path` can route on the same reading
    /// `verify_kernel_bytes` made — under `ActiveDirect` the native loader
    /// performs no cryptographic check, so those bytes must take the
    /// firmware path where the `db` signature is actually verified.
    sb_state: SecureBootState,
}

/// Measure the kernel into PCR 4 and record an HONEST trust event: only emit
/// `kernel_measured` when the PCR was actually extended; otherwise emit
/// `kernel_measurement_skipped` so the trust log never claims a measurement
/// that did not happen (TPM absent / TCG2 error). Keeps measured-boot evidence
/// truthful in the no-TPM case.
fn measure_kernel_logged(tpm: &TpmContext, trust_log: &mut TrustLog, data: &[u8], path: &str) {
    if tpm.measure_kernel(data) {
        trust_log.record(
            TrustEvent::new("kernel_measured")
                .with_path(path)
                .with_note("pcr=4"),
        );
    } else {
        trust_log.record(
            TrustEvent::new("kernel_measurement_skipped")
                .with_path(path)
                .with_note("pcr=4 reason=no_tpm_or_tcg2_error"),
        );
    }
}

/// Measure the cmdline into PCR 12 and record the matching honest event.
fn measure_cmdline_logged(tpm: &TpmContext, trust_log: &mut TrustLog, options: &str, path: &str) {
    if tpm.measure_cmdline(options) {
        trust_log.record(
            TrustEvent::new("cmdline_measured")
                .with_path(path)
                .with_note("pcr=12"),
        );
    } else {
        trust_log.record(
            TrustEvent::new("cmdline_measurement_skipped")
                .with_path(path)
                .with_note("pcr=12 reason=no_tpm_or_tcg2_error"),
        );
    }
}

/// Boot the selected entry.
/// `volumes` includes the ESP as the first element, followed by any extra volumes
/// exposed by filesystem drivers (ext4, btrfs, etc.).
#[expect(
    clippy::too_many_arguments,
    reason = "boot dispatch; sb_state is threaded from run_bootloader so the verify path shares the one Secure Boot detection"
)]
pub(crate) fn boot_entry(
    current_image: Handle,
    volumes: &mut [Volume],
    entry: BootEntry,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    // v0.11.0 diagnostics: trail audit.log breadcrumbs through boot_entry
    // so a freeze inside can be located by reading audit.log post-mortem.
    // v0.11.0: log::info! at post-selection depth kills next FAT op
    // on ASUS G10AJ. Replaced with audit-log emission, which is
    // unbounded in this depth band. log::info! re-enabled would
    // freeze the boot before reaching kernel handoff.
    if let Some(esp) = volumes.first_mut() {
        crate::diag::append_fmt(
            esp,
            format_args!("boot_entry: name={} id={}\n", entry.name, entry.id),
        );
    }

    let (esp_slot, rest) = volumes.split_first_mut().ok_or(Status::ABORTED)?;
    crate::diag::append(esp_slot, "BOOTENTRY3 after_split\n");

    match entry.kind {
        EntryKind::Chainload { path } => {
            chainload_efi(current_image, esp_slot, &path, tpm, trust_log, quirks)
        }
        EntryKind::Uki { path, options } => boot_uki(
            current_image,
            esp_slot,
            &path,
            &options,
            tpm,
            policy,
            trust_log,
            quirks,
            sb_state,
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
            quirks,
            sb_state,
        ),
    }
}

/// Chainload another EFI application (Windows Boot Manager, GRUB, EFI fallback,
/// a diagnostic module, …).
///
/// The image is measured into PCR 4 and a trust event is recorded before
/// `start_image`, so a chainloaded binary does not silently break the
/// measured-boot chain or leave a hole in the audit log. Authenticity is still
/// enforced by `BS->LoadImage` (firmware `db`/Security2, plus ShimLock via the
/// SecurityOverride when shim is present) inside `load_efi_image_from_buffer`;
/// this adds the missing measurement + audit record that the kernel/UKI paths
/// already emit.
fn chainload_efi(
    current_image: Handle,
    esp: &mut Volume,
    path: &str,
    tpm: &TpmContext,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
) -> Result<Status> {
    log::info!("Chainloading: {path}");
    let image_data = esp.read_str(path)?;
    log::info!("Read {} bytes", image_data.len());

    // Measure into PCR 4 (boot-code) before load, mirroring the kernel path so
    // the measured-boot chain stays unbroken across a chainload. Record the
    // event honestly: only claim a measurement if the PCR was actually
    // extended.
    let sha256 = pe_loader_pure::sha256_of(&image_data);
    if tpm.measure_kernel(&image_data) {
        trust_log.record(
            TrustEvent::new("chainload_measured")
                .with_path(path)
                .with_note("pcr=4"),
        );
    } else {
        trust_log.record(
            TrustEvent::new("chainload_measurement_skipped")
                .with_path(path)
                .with_note("pcr=4 reason=no_tpm_or_tcg2_error"),
        );
    }

    let image_handle = load_efi_image_from_buffer(current_image, &image_data, None)?;

    // The binary passed firmware authentication (LoadImage above would have
    // failed otherwise). Record it for audit before handing off control.
    trust_log.record(
        TrustEvent::new("image_chainloaded")
            .with_path(path)
            .with_sha256(&pe_loader_pure::hex_encode_32(&sha256))
            .with_verified_via(V_FIRMWARE_LOADIMAGE)
            .with_status(Status::SUCCESS),
    );
    // Gate the FAT flush on the conout_fat_coupling quirk like every other
    // pre-handoff flush: on ASUS G10AJ-class firmware an ungated write here can
    // hang the FAT driver during a chainload (a recovery/diagnostic path). The
    // event is still recorded in-memory; it flushes on the next gated boundary
    // or, for a chainloaded tool that returns, when the menu loop flushes.
    if !quirks.conout_fat_coupling {
        trust_log.flush(esp);
    }

    reconnect_console_drivers();
    log::info!("Starting chainloaded image...");
    crate::diag::flush(esp);
    uefi::boot::start_image(image_handle)?;

    // Child image returned — caller should re-enter the menu
    log::info!("Chainloaded image returned");
    Ok(Status::SUCCESS)
}

/// Boot a Unified Kernel Image (UKI)
#[expect(
    clippy::too_many_arguments,
    reason = "orchestration; passing FirmwareQuirks alongside policy is intentional"
)]
fn boot_uki(
    current_image: Handle,
    esp: &mut Volume,
    path: &str,
    options: &str,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    log::info!("Booting UKI: {path} with options: {options}");
    let backend_tag = esp.backend_tag();
    let image_data = esp.read_str(path)?;

    // Measure into TPM before any load decision — same PCR/semantics
    // whether we take the native or firmware path. SDS-4 §6.1 Step 11:
    // kernel_measured records the PCR 4 extend (or measurement_skipped if
    // no TPM, so the log stays honest).
    measure_kernel_logged(tpm, trust_log, &image_data, path);
    // Step 12: measure cmdline (UKI cmdline comes from the binary's
    // .cmdline section; we still measure it since it's what the
    // kernel will see).
    if !options.is_empty() {
        measure_cmdline_logged(tpm, trust_log, options, path);
    }

    // SDS-4 Step 10: verify + capture SHA-256 + verified_via.
    let Ok(verified) = verify_kernel_bytes(&image_data, path, trust_log, esp, sb_state) else {
        log::error!("UKI verification failed, returning to menu: {path}");
        return Err(Status::SECURITY_VIOLATION.into());
    };

    match choose_load_path(policy.loader_native_pe, verified.sb_state) {
        LoadPath::Native => native_load_and_start(
            &verified,
            options,
            backend_tag,
            path,
            current_image,
            trust_log,
            esp,
            quirks,
        ),
        LoadPath::Firmware => firmware_load_and_start(
            current_image,
            &image_data,
            options,
            policy.loader_native_pe,
            trust_log,
            path,
            esp,
        ),
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
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    crate::diag::append_fmt(
        esp,
        format_args!(
            "boot_linux: kernel={} initrds={} options={}\n",
            kernel_path,
            initrd_paths.len(),
            options
        ),
    );

    trust_log.record(
        TrustEvent::new("kernel_load_phase")
            .with_path(kernel_path)
            .with_note("phase=resolve_volume"),
    );
    crate::diag::append(esp, "BOOTLINUX3 after_first_record\n");
    if !quirks.conout_fat_coupling {
        trust_log.flush(esp);
    }
    crate::diag::append(esp, "BOOTLINUX4 after_first_flush\n");

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
    crate::diag::append(esp, "BL5 check_esp_for_kernel\n");
    let (kernel_bytes, backend_tag) = if let Some(resolved) =
        crate::preflight::exists_with_boot_fallback(esp, kernel_path)
    {
        crate::diag::append_fmt(
            esp,
            format_args!("BL5_esp kernel_found_on_esp: {resolved}\n"),
        );
        let tag = esp.backend_tag();
        trust_log.record(
            TrustEvent::new("kernel_load_phase")
                .with_path(&resolved)
                .with_note(&alloc::format!(
                    "phase=read_bytes_start volume_index=0 backend={tag}"
                )),
        );
        crate::diag::append(esp, "BL6_esp before_esp_read\n");
        let bytes = esp.read_str(&resolved)?;
        crate::diag::append_fmt(
            esp,
            format_args!("BL7_esp after_esp_read size={}\n", bytes.len()),
        );
        trust_log.record(
            TrustEvent::new("kernel_load_phase")
                .with_path(&resolved)
                .with_note(&alloc::format!(
                    "phase=read_bytes_done size={}",
                    bytes.len()
                )),
        );
        (bytes, tag)
    } else {
        crate::diag::append(esp, "BL5_extras not_on_esp_scanning_extras\n");
        let mut found: Option<(usize, String)> = None;
        for (i, v) in extras.iter_mut().enumerate() {
            if let Some(resolved) = crate::preflight::exists_with_boot_fallback(v, kernel_path) {
                found = Some((i, resolved));
                break;
            }
        }
        crate::diag::append(esp, "BL5b after_extras_scan\n");
        let Some((extra_idx, resolved)) = found else {
            crate::diag::append(esp, "BL5_err kernel_not_found\n");
            trust_log.record(
                TrustEvent::new("kernel_load_phase")
                    .with_path(kernel_path)
                    .with_note("phase=resolve_failed"),
            );
            if !quirks.conout_fat_coupling {
                trust_log.flush(esp);
            }
            return Err(Status::NOT_FOUND.into());
        };
        crate::diag::append_fmt(
            esp,
            format_args!("BL5c resolved extras[{extra_idx}]: {resolved}\n"),
        );
        let vol = &mut extras[extra_idx];
        let tag = vol.backend_tag();
        let volume_index = extra_idx + 1;
        trust_log.record(
            TrustEvent::new("kernel_load_phase")
                .with_path(&resolved)
                .with_note(&alloc::format!(
                    "phase=read_bytes_start volume_index={volume_index} backend={tag}"
                )),
        );
        crate::diag::append(esp, "BL6 before_extras_read\n");
        let bytes = vol.read_str(&resolved)?;
        crate::diag::append_fmt(
            esp,
            format_args!("BL7 after_extras_read size={}\n", bytes.len()),
        );
        trust_log.record(
            TrustEvent::new("kernel_load_phase")
                .with_path(&resolved)
                .with_note(&alloc::format!(
                    "phase=read_bytes_done size={}",
                    bytes.len()
                )),
        );
        (bytes, tag)
    };

    crate::diag::append(esp, "BL8 bytes_acquired\n");
    trust_log.record(
        TrustEvent::new("kernel_load_phase")
            .with_path(kernel_path)
            .with_note(&alloc::format!(
                "phase=bytes_acquired total_size={} backend={backend_tag}",
                kernel_bytes.len()
            )),
    );
    if !quirks.conout_fat_coupling {
        trust_log.flush(esp);
    }
    crate::diag::append(esp, "BL9 before_tpm_measure_kernel\n");

    measure_kernel_logged(tpm, trust_log, &kernel_bytes, kernel_path);
    crate::diag::append(esp, "BL10 after_tpm_measure_kernel\n");

    if !options.is_empty() {
        measure_cmdline_logged(tpm, trust_log, options, kernel_path);
        crate::diag::append(esp, "BL11 after_tpm_measure_cmdline\n");
    }
    crate::diag::append(esp, "BL12 before_verify_kernel_bytes\n");

    let Ok(verified) = verify_kernel_bytes(&kernel_bytes, kernel_path, trust_log, esp, sb_state)
    else {
        crate::diag::append(esp, "BL13_err verify_failed\n");
        return Err(Status::SECURITY_VIOLATION.into());
    };
    crate::diag::append(esp, "BL13 after_verify_kernel_bytes\n");

    let _initrd_handle = if initrd_paths.is_empty() {
        crate::diag::append(esp, "BL14 no_initrds\n");
        None
    } else {
        crate::diag::append_fmt(
            esp,
            format_args!("BL14 initrds_count={}\n", initrd_paths.len()),
        );
        let mut combined = Vec::new();
        for (idx, path) in initrd_paths.iter().enumerate() {
            crate::diag::append_fmt(esp, format_args!("BL14_initrd[{idx}] start: {path}\n"));
            let data = if let Some(resolved) =
                crate::preflight::exists_with_boot_fallback(esp, path)
            {
                crate::diag::append(esp, "BL14_initrd_on_esp\n");
                esp.read_str(&resolved)?
            } else {
                crate::diag::append(esp, "BL14_initrd_scan_extras\n");
                let mut found_idx: Option<(usize, String)> = None;
                for (i, v) in extras.iter_mut().enumerate() {
                    if let Some(resolved) = crate::preflight::exists_with_boot_fallback(v, path) {
                        found_idx = Some((i, resolved));
                        break;
                    }
                }
                if let Some((i, resolved)) = found_idx {
                    crate::diag::append_fmt(
                        esp,
                        format_args!("BL14_initrd_extras[{i}]: {resolved} before_read\n"),
                    );
                    let d = extras[i].read_str(&resolved)?;
                    crate::diag::append_fmt(
                        esp,
                        format_args!("BL14_initrd_after_read size={}\n", d.len()),
                    );
                    d
                } else {
                    crate::diag::append_fmt(esp, format_args!("BL14_initrd_NOT_FOUND: {path}\n"));
                    continue;
                }
            };
            crate::diag::append_fmt(
                esp,
                format_args!("BL14_initrd[{idx}] {} bytes\n", data.len()),
            );
            combined.extend_from_slice(&data);
        }
        crate::diag::append_fmt(
            esp,
            format_args!(
                "BL15 total_initrd={} bytes before_register\n",
                combined.len()
            ),
        );
        let r = InitrdHandle::register(combined, Some(esp));
        crate::diag::append_fmt(
            esp,
            format_args!("BL16 after_initrd_register ok={}\n", r.is_ok()),
        );
        r.ok()
    };

    crate::diag::append(esp, "BL17 before_choose_load_path\n");
    let path = choose_load_path(policy.loader_native_pe, verified.sb_state);
    crate::diag::append_fmt(esp, format_args!("BL18 load_path={path:?}\n"));
    match path {
        LoadPath::Native => {
            crate::diag::append(esp, "BL19 calling_native_load_and_start\n");
            native_load_and_start(
                &verified,
                options,
                backend_tag,
                kernel_path,
                current_image,
                trust_log,
                esp,
                quirks,
            )
        }
        LoadPath::Firmware => {
            crate::diag::append(esp, "BL19 calling_firmware_load_and_start\n");
            firmware_load_and_start(
                current_image,
                &kernel_bytes,
                options,
                policy.loader_native_pe,
                trust_log,
                kernel_path,
                esp,
            )
        }
    }
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

    // Measure kernel image into TPM PCR 4 (best-effort; this helper records no
    // trust event of its own, so the measure outcome is intentionally ignored).
    let _ = tpm.measure_kernel(&image_data);

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

/// Decide the load path from policy + the Secure Boot posture the bytes
/// were verified under.
///
/// The native loader performs no cryptographic check of its own — its trust
/// contract is "the caller already verified these bytes." That contract holds
/// only when verification actually happened:
///   - `ActiveWithShim`: ShimLock::Verify ran and accepted (callers reach here
///     only on the accepted branch), so native is safe.
///   - `Disabled`: Secure Boot is off — there is nothing to verify and no
///     security guarantee to uphold, so native is fine.
///   - `ActiveDirect`: SB on, no shim. Nothing verified the kernel against
///     firmware `db` yet. The native loader would execute it unauthenticated,
///     so we MUST take the firmware path, where `BS->LoadImage` runs the real
///     `db` check (the SecurityOverride hook delegates to firmware when no shim
///     is present). `Always` cannot override this — there is no shim to ask and
///     skipping `db` would be a silent Secure Boot bypass.
fn choose_load_path(mode: LoaderNativePeMode, sb_state: SecureBootState) -> LoadPath {
    use crate::boot_route_pure::{decide_load_route, LoadRoute, NativePeMode, SbPosture};

    // Adapt the real enums into the pure mirror and delegate; the 9-row truth
    // table (incl. the Auto+ActiveDirect C1 must-reject) lives in
    // boot_route_pure and is host-tested there.
    let mode = match mode {
        LoaderNativePeMode::Auto => NativePeMode::Auto,
        LoaderNativePeMode::Always => NativePeMode::Always,
        LoaderNativePeMode::Never => NativePeMode::Never,
    };
    let posture = match sb_state {
        SecureBootState::Disabled => SbPosture::Disabled,
        SecureBootState::ActiveWithShim => SbPosture::ActiveWithShim,
        SecureBootState::ActiveDirect => SbPosture::ActiveDirect,
    };
    match decide_load_route(mode, posture) {
        LoadRoute::Native => LoadPath::Native,
        LoadRoute::Firmware => LoadPath::Firmware,
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
    esp_for_diag: &mut Volume,
    sb_state: SecureBootState,
) -> core::result::Result<VerifiedBytes<'b>, ()> {
    crate::diag::append(esp_for_diag, "VK1 enter\n");
    let sha256 = pe_loader_pure::sha256_of(bytes);
    crate::diag::append(esp_for_diag, "VK2 after_sha256\n");
    let sha_hex = pe_loader_pure::hex_encode_32(&sha256);
    crate::diag::append(esp_for_diag, "VK3 after_hex_encode\n");

    // sb_state was detected once at the top of run_bootloader (so the report and
    // trust log share one posture), BUT it was captured BEFORE load_drivers. On
    // shim < 15.8 the ShimLock protocol is uninstalled after first use, so a
    // legacy FS driver (XFS/ZFS/NTFS /boot) loaded since then can have consumed
    // it. Re-probe here and degrade ActiveWithShim -> ActiveDirect if shim has
    // vanished: verifying against a ShimLock that is no longer present would
    // hard-reject a kernel that should fall through to the firmware-db path
    // (v0.12.0 detected fresh at this site; this restores that resilience).
    let sb_state = if sb_state == SecureBootState::ActiveWithShim && !secure::shim_lock_present() {
        SecureBootState::ActiveDirect
    } else {
        sb_state
    };
    crate::diag::append_fmt(esp_for_diag, format_args!("VK4 secure_boot={sb_state:?}\n"));

    // Honest status: only a path where a signature was actually validated
    // records "SUCCESS". The two accepted-but-unverified postures carry a
    // distinct status so a SIEM keying on status=SUCCESS cannot mistake a
    // degraded boot for a verified one (the verified_via token already encodes
    // the degraded posture; this keeps the status field consistent with it).
    //   - Disabled: SB off, nothing to verify -> SKIPPED.
    //   - ActiveDirect: no shim to ask; LamBoot performs no check here, the
    //     firmware db check runs later on the forced firmware LoadImage path
    //     (see choose_load_path) -> DEFERRED.
    let (verified_via, status_str, accepted) = match sb_state {
        SecureBootState::Disabled => (V_DEGRADED_TRUST_SB_OFF, "SKIPPED", true),
        SecureBootState::ActiveDirect => (V_DEGRADED_TRUST_SB_DIRECT, "DEFERRED", true),
        SecureBootState::ActiveWithShim => {
            crate::diag::append(esp_for_diag, "VK5 before_shim_verify\n");
            let r = secure::verify_image(bytes, sb_state);
            crate::diag::append(esp_for_diag, "VK6 after_shim_verify\n");
            match r {
                Ok(()) => (V_SHIM_MOK, "SUCCESS", true),
                Err(_) => (V_SHIM_REJECTED, "REJECTED", false),
            }
        }
    };
    crate::diag::append_fmt(
        esp_for_diag,
        format_args!("VK7 verified_via={verified_via} status={status_str} accepted={accepted}\n"),
    );

    trust_log.record(
        TrustEvent::new("image_verified")
            .with_path(path_for_log)
            .with_sha256(&sha_hex)
            .with_verified_via(verified_via)
            .with_status(status_str),
    );
    crate::diag::append(esp_for_diag, "VK8 after_record_image_verified\n");

    if accepted {
        Ok(VerifiedBytes {
            bytes,
            sha256,
            verified_via,
            sb_state,
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
/// changed under us) and is treated as a security-critical failure:
/// the load is refused (fail-closed) and the boot returns to the menu
/// with a recorded trust event, rather than panicking/bricking — a
/// brick is an unnecessarily harsh way to fail closed.
// Orchestration function — ESP pass-through is how we persist
// image_verified + image_loaded_native to disk before ExitBootServices.
#[expect(
    clippy::too_many_arguments,
    reason = "orchestration; FirmwareQuirks routes the conout-coupling flush gate"
)]
fn native_load_and_start(
    verified: &VerifiedBytes<'_>,
    options: &str,
    backend_tag: &'static str,
    path_for_log: &str,
    _current_image: Handle,
    trust_log: &mut TrustLog,
    esp_for_flush: &mut Volume,
    quirks: crate::firmware_quirks::FirmwareQuirks,
) -> Result<Status> {
    crate::diag::append(esp_for_flush, "N1 enter\n");
    let bytes = verified.bytes;

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
    crate::diag::append(esp_for_flush, "N2 after_options_encode\n");

    crate::diag::append(esp_for_flush, "N3 before_pe_loader_load_pe\n");
    let loaded = match pe_loader::load_pe(bytes, options_box, backend_tag, Some(esp_for_flush)) {
        Ok(l) => {
            crate::diag::append(esp_for_flush, "N4 after_pe_loader_load_pe_ok\n");
            l
        }
        Err(e) => {
            crate::diag::append_fmt(
                esp_for_flush,
                format_args!("N4_err pe_loader_failed: {}\n", e.as_log_token()),
            );
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

    // SDS-4 §6.4: fail closed on a verify/load SHA-256 mismatch (a TOCTOU
    // signal), but return to the menu rather than panic-bricking the boot.
    if *loaded.sha256() != verified.sha256 {
        crate::diag::append(esp_for_flush, "N5_err sha256_invariant_violation\n");
        trust_log.record(
            TrustEvent::new("image_load_failed")
                .with_path(path_for_log)
                .with_verified_via("native_pe_loader")
                .with_note("reason=sds4_6.4_verify_load_sha256_mismatch"),
        );
        if !quirks.conout_fat_coupling {
            trust_log.flush(esp_for_flush);
        }
        return Err(Status::SECURITY_VIOLATION.into());
    }
    crate::diag::append(esp_for_flush, "N5 after_sha256_invariant\n");

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
    crate::diag::append(esp_for_flush, "N6 after_record_image_loaded_native\n");

    if !quirks.conout_fat_coupling {
        trust_log.flush(esp_for_flush);
    }
    crate::diag::append(esp_for_flush, "N7 after_or_skipped_trust_log_flush\n");

    reconnect_console_drivers();
    crate::diag::append(esp_for_flush, "N8 after_reconnect_console_drivers\n");

    // Single pre-handoff flush of coalesced breadcrumbs (no-op unless
    // coalescing). This is the last point we control before the kernel
    // ExitBootServices-es, so it captures the whole post-selection trail in
    // one FAT write on coupling-prone firmware.
    crate::diag::flush(esp_for_flush);

    // SAFETY: pe_loader validated PE; verified.bytes came from the
    // verify_kernel_bytes path above.
    let status = unsafe { pe_loader::start_image(loaded) };
    // If we get here, the kernel returned (unusual — normally ExitBootServices makes this unreachable).
    crate::diag::append_fmt(
        esp_for_flush,
        format_args!("N9 start_image_returned status={status:?}\n"),
    );
    Ok(status)
}

/// Firmware load path: the v0.8.3 load_image + start_image sequence,
/// preserved verbatim so operators have a one-flag rollback path.
///
/// Emits two trust events:
/// - `legacy_loadimage_used` before the LoadImage call — records that the
///   firmware path was taken (and why), so an operator running with
///   `[loader].native_pe = "never"` still sees an audit entry instead of
///   silence. Without this, the trust log went dead after `boot_attempt`
///   on the Never path because the verify/load events live only on the
///   native path. (v0.9.1 sprint finding §7.P3.)
/// - `image_loaded_firmware` after a successful load — same as before.
///
/// The trust log is flushed BEFORE `start_image` so events persist when
/// the kernel ExitBootServices-es. Without this flush, the events above
/// were lost in memory and never reached `\loader\boot-trust.log`.
fn firmware_load_and_start(
    current_image: Handle,
    bytes: &[u8],
    options: &str,
    mode: LoaderNativePeMode,
    trust_log: &mut TrustLog,
    path_for_log: &str,
    esp_for_flush: &mut Volume,
) -> Result<Status> {
    log::info!(
        "Firmware image load: {path_for_log} ({} bytes via BS->LoadImage)",
        bytes.len()
    );

    trust_log.record(
        TrustEvent::new("legacy_loadimage_used")
            .with_path(path_for_log)
            .with_verified_via(V_FIRMWARE_LOADIMAGE)
            .with_note(&alloc::format!(
                "size={} policy={}",
                bytes.len(),
                mode.as_log_token(),
            )),
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

    // Mirror the native path's flush (line ~617). Trust events emitted
    // here would otherwise vanish when the kernel ExitBootServices-es.
    trust_log.flush(esp_for_flush);

    reconnect_console_drivers();
    crate::diag::flush(esp_for_flush);
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
