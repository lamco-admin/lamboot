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
        TrustEvent, TrustLog, V_DEGRADED_TRUST_SB_DIRECT, V_DEGRADED_TRUST_SB_OFF,
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
        EntryKind::Iso {
            source: crate::boot_types::IsoSource::File { iso_path },
        } => boot_iso(
            esp_slot,
            rest,
            entry.source_volume_index,
            &iso_path,
            current_image,
            tpm,
            policy,
            trust_log,
            quirks,
            sb_state,
        ),
        EntryKind::Iso {
            source: crate::boot_types::IsoSource::Optical { handle, label },
        } => boot_iso_optical(
            esp_slot,
            handle,
            &label,
            current_image,
            tpm,
            policy,
            trust_log,
            quirks,
            sb_state,
        ),
    }
}

/// Boot-from-ISO (SPEC-BOOT-FROM-ISO). Promotes the holding volume's backend to
/// a shared handle, opens a loopback [`FileBlockSource`] over the `.iso`, and
/// dispatches it as an ISO9660 volume (the M0/M1 path). Reading the ISO off the
/// ESP is safe — `FileBlockSource` reads through the holding FAT backend's cached
/// SFS, never a fresh exclusive open.
///
/// **Path A1 (preferred for a file-hosted ISO):** parse the distro's own
/// `/boot/grub/loopback.cfg` — it carries the kernel/initrd paths and the exact
/// iso-find token (`${iso_path}` substituted). **Path A2 (fallback):** if there
/// is no usable `loopback.cfg`, fingerprint the distro family
/// ([`crate::distro_iso`]) and render its `file_cmdline`. Either way the shared
/// [`iso_boot_resolved`] tail runs the same measure → verify → initrd-register →
/// PE-load handoff as `boot_linux`, bytes sourced from the ISO. Path B (El Torito
/// chainload, M4) is not yet wired — it depends on the lamfat FAT-over-source arm.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors boot_linux; the boot params are threaded from boot_entry for the shared verify/load path"
)]
fn boot_iso(
    esp: &mut Volume,
    rest: &mut [Volume],
    holding_index: usize,
    iso_path: &str,
    current_image: Handle,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    // The `.iso` lives on the volume named by `source_volume_index` (0 = ESP).
    let holding: &mut Volume = if holding_index == 0 {
        esp
    } else {
        rest.get_mut(holding_index - 1)
            .ok_or_else(|| uefi::Error::from(Status::ABORTED))?
    };

    let shared = holding.share_backend();
    let path = crate::fs_backend::PathBuf::from_str(iso_path)
        .map_err(|_| uefi::Error::from(Status::INVALID_PARAMETER))?;
    let src = crate::block_source::FileBlockSource::new(shared, path).map_err(uefi::Error::from)?;
    let backend = crate::fs_backend_lvm_dispatch::dispatch_fs_over_source(src, None)
        .map_err(uefi::Error::from)?;

    let tag = backend.tag();
    let mut iso_vol = crate::fs::Volume::from_backend(
        crate::fs::VolumeIdentity {
            partition_guid: None,
            fs_uuid: None,
            label: None,
            index: u32::MAX,
            backend_tag: tag,
        },
        backend,
    );
    log::info!("Mounted ISO {iso_path}: {}", iso_vol.identity().describe());
    trust_log.record(TrustEvent::new("iso_mounted").with_note(&alloc::format!(
        "iso_path={iso_path} backend={tag} holding_index={holding_index}"
    )));

    // Path A1 (file case preferred): the distro's own /boot/grub/loopback.cfg
    // names the kernel, initrd, and the exact iso-find token (with `${iso_path}`
    // substituted). When present it is the most faithful resolver for a
    // file-hosted ISO, so it is tried first.
    const LOOPBACK_CFG: &str = "/boot/grub/loopback.cfg";
    if let Ok(cfg_bytes) = iso_vol.read_str(LOOPBACK_CFG) {
        let cfg = String::from_utf8_lossy(&cfg_bytes);
        if let Some(entry) = crate::loopback_cfg::parse_first_menuentry(&cfg, iso_path) {
            log::info!(
                "ISO {iso_path}: Path A1 (loopback.cfg) kernel={}",
                entry.kernel
            );
            let initrd_paths = [entry.initrd];
            return iso_boot_resolved(
                iso_vol,
                tag,
                iso_path,
                &entry.kernel,
                &initrd_paths,
                &entry.options,
                esp,
                current_image,
                tpm,
                policy,
                trust_log,
                quirks,
                sb_state,
            );
        }
        log::warn!(
            "ISO {iso_path}: {LOOPBACK_CFG} present but no usable menuentry — trying Path A2"
        );
    }

    // Path A2 (fallback): fingerprint the distro family and use its table entry.
    // For a file-hosted ISO this renders the `file_cmdline` ({iso} = the path).
    let Some(recipe) = crate::distro_iso::fingerprint(|p| iso_vol.exists_str(p)) else {
        log::warn!(
            "ISO {iso_path}: no usable loopback.cfg and unrecognized distro layout — Path B (chainload) not yet wired"
        );
        return Err(uefi::Error::from(Status::UNSUPPORTED));
    };
    let label = String::from(iso_vol.label().unwrap_or(""));
    let options = crate::distro_iso::render_cmdline(recipe.file_cmdline, iso_path, &label);
    let Some((kernel_path, initrd_paths)) = resolve_recipe(&mut iso_vol, recipe) else {
        log::warn!(
            "ISO {iso_path}: distro={} recognized but no kernel at the known paths",
            recipe.family
        );
        return Err(uefi::Error::from(Status::UNSUPPORTED));
    };
    log::info!("ISO {iso_path}: Path A2 distro={} (file)", recipe.family);
    trust_log.record(
        TrustEvent::new("iso_path_a2_distro").with_note(&alloc::format!(
            "family={} source=file iso={iso_path}",
            recipe.family
        )),
    );
    iso_boot_resolved(
        iso_vol,
        tag,
        iso_path,
        &kernel_path,
        &initrd_paths,
        &options,
        esp,
        current_image,
        tpm,
        policy,
        trust_log,
        quirks,
        sb_state,
    )
}

/// Boot-from-ISO via a **physical optical drive** (SPEC-BOOT-FROM-ISO scenario
/// d). The `BlockIO` handle *is* the byte source — there is no holding
/// filesystem — so it dispatches as an ISO9660 volume directly through
/// [`BlockIoSource`](crate::block_source::BlockIoSource). A disc has no
/// `${iso_path}`, so `loopback.cfg`'s iso-find token would be wrong for it; the
/// resolver is Path A2 only, rendering the distro recipe's `media_cmdline` (the
/// kernel self-locates on the medium by its volume label).
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors boot_iso; the boot params are threaded from boot_entry for the shared verify/load path"
)]
fn boot_iso_optical(
    esp: &mut Volume,
    handle: Handle,
    label: &str,
    current_image: Handle,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    // Optical discs are held BY_DRIVER by the firmware ISO9660 FS driver, so the
    // raw BlockIO must be opened non-exclusively (an exclusive open is
    // ACCESS_DENIED) — see BlockIoSource::open_shared.
    let src = crate::block_source::BlockIoSource::open_shared(handle).map_err(uefi::Error::from)?;
    let backend = crate::fs_backend_lvm_dispatch::dispatch_fs_over_source(src, None)
        .map_err(uefi::Error::from)?;
    let tag = backend.tag();
    let mut iso_vol = crate::fs::Volume::from_backend(
        crate::fs::VolumeIdentity {
            partition_guid: None,
            fs_uuid: None,
            label: None,
            index: u32::MAX,
            backend_tag: tag,
        },
        backend,
    );
    log::info!(
        "Mounted optical disc [{label}]: {}",
        iso_vol.identity().describe()
    );
    trust_log.record(
        TrustEvent::new("iso_optical_mounted")
            .with_note(&alloc::format!("label={label} backend={tag}")),
    );

    let Some(recipe) = crate::distro_iso::fingerprint(|p| iso_vol.exists_str(p)) else {
        log::warn!(
            "Optical [{label}]: unrecognized distro layout — Path B (chainload) not yet wired"
        );
        return Err(uefi::Error::from(Status::UNSUPPORTED));
    };
    let options = crate::distro_iso::render_cmdline(recipe.media_cmdline, "", label);
    let Some((kernel_path, initrd_paths)) = resolve_recipe(&mut iso_vol, recipe) else {
        log::warn!(
            "Optical [{label}]: distro={} recognized but no kernel at the known paths",
            recipe.family
        );
        return Err(uefi::Error::from(Status::UNSUPPORTED));
    };
    log::info!(
        "Optical [{label}]: Path A2 distro={} (media)",
        recipe.family
    );
    trust_log.record(
        TrustEvent::new("iso_path_a2_distro").with_note(&alloc::format!(
            "family={} source=optical label={label}",
            recipe.family
        )),
    );
    iso_boot_resolved(
        iso_vol,
        tag,
        label,
        &kernel_path,
        &initrd_paths,
        &options,
        esp,
        current_image,
        tpm,
        policy,
        trust_log,
        quirks,
        sb_state,
    )
}

/// Resolve a distro recipe's kernel + initrd candidates against the mounted ISO.
/// Kernel: the **first** candidate that resolves (exact path or prefix glob).
/// Initrd: **every** candidate that resolves, in order (microcode images before
/// the main initramfs). `None` if no kernel candidate resolves.
fn resolve_recipe(
    iso_vol: &mut Volume,
    recipe: &crate::distro_iso::DistroRecipe,
) -> Option<(String, Vec<String>)> {
    let kernel = recipe
        .kernel
        .iter()
        .copied()
        .find_map(|c| resolve_candidate(iso_vol, c))?;
    let initrd = recipe
        .initrd
        .iter()
        .copied()
        .filter_map(|c| resolve_candidate(iso_vol, c))
        .collect();
    Some((kernel, initrd))
}

/// Resolve one recipe candidate against the mounted ISO: an exact path is
/// returned if it exists; a trailing-`*` prefix glob is resolved against its
/// directory listing (newest match wins). `None` if nothing matches.
fn resolve_candidate(iso_vol: &mut Volume, candidate: &str) -> Option<String> {
    if let Some((dir, prefix)) = crate::distro_iso::split_glob(candidate) {
        let names = iso_vol.read_dir_str(dir).ok()?;
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let pick = crate::distro_iso::glob_pick(&refs, prefix)?;
        Some(alloc::format!("{dir}/{pick}"))
    } else if iso_vol.exists_str(candidate) {
        Some(String::from(candidate))
    } else {
        None
    }
}

/// Shared boot-from-ISO tail: read the kernel + initrd component(s) off the
/// mounted ISO, drop the ISO volume (releasing its `Rc` clone of any shared
/// holding backend) before the handoff touches `esp`, then run the same
/// measure → verify → initrd-register → PE-load handoff as `boot_linux` with the
/// bytes sourced from the ISO. Every initrd component is concatenated in order
/// (microcode first); the kernel consumes the concatenation as one initrd.
#[expect(
    clippy::too_many_arguments,
    reason = "shared tail of the file + optical ISO boot paths; threads the same boot params as boot_linux"
)]
fn iso_boot_resolved(
    mut iso_vol: Volume,
    iso_tag: crate::fs_types::BackendTag,
    label_for_log: &str,
    kernel_path: &str,
    initrd_paths: &[String],
    options: &str,
    esp: &mut Volume,
    current_image: Handle,
    tpm: &TpmContext,
    policy: &Policy,
    trust_log: &mut TrustLog,
    quirks: crate::firmware_quirks::FirmwareQuirks,
    sb_state: SecureBootState,
) -> Result<Status> {
    let kernel = iso_vol.read_str(kernel_path).map_err(uefi::Error::from)?;
    let mut initrd = Vec::new();
    for p in initrd_paths {
        match iso_vol.read_str(p) {
            Ok(mut bytes) => initrd.append(&mut bytes),
            Err(_) => log::warn!("ISO [{label_for_log}]: initrd component {p} absent, skipping"),
        }
    }
    log::info!(
        "ISO Path A resolved [{label_for_log}]: kernel={kernel_path} ({} B), initrd={} comp ({} B), cmdline=[{options}]",
        kernel.len(),
        initrd_paths.len(),
        initrd.len()
    );
    trust_log.record(
        TrustEvent::new("iso_path_a_resolved").with_note(&alloc::format!(
            "kernel={kernel_path} kbytes={} ibytes={} label={label_for_log}",
            kernel.len(),
            initrd.len()
        )),
    );

    // Done reading the ISO — drop the loopback/optical volume so its `Rc` clone
    // of any shared holding backend is released before the handoff touches `esp`.
    drop(iso_vol);

    measure_kernel_logged(tpm, trust_log, &kernel, kernel_path);
    if !options.is_empty() {
        measure_cmdline_logged(tpm, trust_log, options, kernel_path);
    }
    let Ok(verified) = verify_kernel_bytes(&kernel, kernel_path, trust_log, esp, sb_state) else {
        return Err(Status::SECURITY_VIOLATION.into());
    };
    // The live system locates its own root via the cmdline (iso-find token or
    // CDLABEL); LamBoot only registers this kernel's own initrd. Skip
    // registration when no initrd component resolved.
    let _initrd_handle = if initrd.is_empty() {
        None
    } else {
        InitrdHandle::register(initrd, Some(esp)).ok()
    };
    let firmware_load = |trust_log: &mut TrustLog, esp: &mut Volume| {
        firmware_load_and_start(
            current_image,
            &kernel,
            options,
            policy.loader_native_pe,
            trust_log,
            kernel_path,
            esp,
        )
    };
    match choose_load_path(policy.loader_native_pe, verified.sb_state) {
        // A distro ISO kernel is a real Linux EFI-stub PE (sections .setup /
        // .compat / .text / .data), which LamBoot's native loader does not load
        // (LOAD_ERROR) — unlike LamBoot's own modules. The native path is still
        // attempted (it honors an operator's SB-bypass intent and works for a
        // PE-stub UKI on an ISO), but on a load failure we fall back to firmware
        // `BS->LoadImage`, which loads the stub correctly. This keeps the default
        // `Auto` policy able to boot an ISO under SB-off, where the route is
        // Native. boot_linux keeps its native-only contract; the fallback is
        // scoped to the ISO path, whose kernels are always foreign distro images.
        LoadPath::Native => match native_load_and_start(
            &verified,
            options,
            iso_tag,
            kernel_path,
            current_image,
            trust_log,
            esp,
            quirks,
        ) {
            Err(_) => {
                log::warn!(
                    "ISO [{label_for_log}]: native PE load failed for {kernel_path} (Linux EFI stub) — falling back to firmware LoadImage"
                );
                trust_log.record(
                    TrustEvent::new("iso_native_load_fallback")
                        .with_path(kernel_path)
                        .with_note("reason=native_pe_load_error route=firmware"),
                );
                firmware_load(trust_log, esp)
            }
            ok => ok,
        },
        LoadPath::Firmware => firmware_load(trust_log, esp),
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
            match native_load_and_start(
                &verified,
                options,
                backend_tag,
                kernel_path,
                current_image,
                trust_log,
                esp,
                quirks,
            ) {
                Err(_) => {
                    // Defense-in-depth (v0.16.1): a native-PE parse/load failure
                    // on a Linux kernel must NOT be terminal. The native loader
                    // is preferred (it honors the SB-bypass intent and measured
                    // boot), but a parser regression or an exotic header should
                    // degrade to firmware BS->LoadImage — which loads the EFI
                    // stub correctly — rather than leaving the system unbootable.
                    // The ISO path already had this fallback; extending it here
                    // is what would have turned the goblin-0.9.3 regression that
                    // powered archie off into a (logged) firmware-path boot.
                    // firmware_load_and_start carries the SecurityOverride guard,
                    // so this is safe under SB+MOK as well as SB-off.
                    crate::diag::append(esp, "BL19b native_failed_fallback_firmware\n");
                    log::warn!(
                        "native PE load failed for {kernel_path} — falling back to firmware LoadImage"
                    );
                    trust_log.record(
                        TrustEvent::new("kernel_native_load_fallback")
                            .with_path(kernel_path)
                            .with_note("reason=native_pe_load_error route=firmware"),
                    );
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
                ok => ok,
            }
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
