#![no_main]
#![no_std]

extern crate alloc;

mod acpi;
mod autodiscovery;
mod bls;
mod bls_parse;
mod boot;
mod bootlog;
mod console;
mod discovery;
mod discovery_pure;
mod drivers;
mod fs;
mod fs_backend;
mod fs_backend_ext4;
mod fs_backend_fat;
mod fs_types;
mod fs_writer;
mod fw_cfg;
mod gui;
mod health;
mod hypervisor;
mod initrd;
mod input;
mod partitions;
mod pe_loader;
mod pe_loader_pure;
mod policy;
mod preflight;
mod report;
mod secure;
mod security_override;
mod smbios;
mod telemetry;
mod tpm;
mod trust_log;
mod trust_log_pure;
mod uki;

use alloc::{format, string::String, vec::Vec};

use log::info;
use uefi::{prelude::*, Result};

use crate::fs::Volume;

const WATCHDOG_TIMEOUT_SECONDS: usize = 300; // 5 minutes

/// Build information embedded at compile time
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(target_arch = "x86_64")]
pub(crate) const BUILD_TARGET: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
pub(crate) const BUILD_TARGET: &str = "aarch64";

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI helpers");

    let image = uefi::boot::image_handle();

    info!("LamBoot v{VERSION} ({BUILD_TARGET}) starting...");

    // Set watchdog timer to prevent hangs
    uefi::boot::set_watchdog_timer(WATCHDOG_TIMEOUT_SECONDS, 0x10000, None)
        .expect("Failed to set watchdog timer");

    match run_bootloader(image) {
        Ok(status) => status,
        Err(e) => {
            log::error!("LamBoot failed: {e}");
            e.status()
        }
    }
}

fn run_bootloader(image: Handle) -> Result<Status> {
    let mut telemetry = telemetry::BootTelemetry::new();
    let mut bootlog = bootlog::BootLog::new();

    // Phase 1: Health assessment
    let t = telemetry::timestamp_ms();
    let crash_count = health::assess_boot_health()?;
    telemetry.record("health", t);

    // Phase 2: Security checks
    secure::log_secure_boot_state();
    let tpm = tpm::TpmContext::new();

    // Phase 2.5: SMBIOS reading (VM identity, fleet tags)
    let (sys_info, oem_strings) = smbios::read_smbios();
    if let Some(ref mfr) = sys_info.manufacturer {
        info!(
            "System: {} {}",
            mfr,
            sys_info.product_name.as_deref().unwrap_or("")
        );
    }
    if !oem_strings.is_empty() {
        info!("SMBIOS OEM strings: {}", oem_strings.len());
    }
    let fleet_id = smbios::get_lamboot_oem_value(&oem_strings, "fleet-id").map(String::from);

    // Phase 2.6: Hypervisor detection
    let hypervisor = hypervisor::detect_hypervisor();
    if hypervisor.present {
        info!(
            "Hypervisor: {}",
            hypervisor.name.as_deref().unwrap_or("unknown")
        );
    }

    // Phase 2.7: IOMMU detection (ACPI DMAR/IVRS)
    let iommu = acpi::parse_iommu_tables();
    if iommu.intel_vt_d {
        info!(
            "IOMMU: Intel VT-d ({} DRHD units, {} RMRR regions)",
            iommu.drhd_units.len(),
            iommu.rmrr_regions.len()
        );
    } else if iommu.amd_vi {
        info!("IOMMU: AMD-Vi ({} IVHD blocks)", iommu.drhd_units.len());
    }

    // Phase 2.8: fw_cfg data channel (host-injected config)
    if fw_cfg::fw_cfg_available() {
        if let Some(config) = fw_cfg::read_file_string("opt/lamboot/config") {
            info!("fw_cfg config: {} bytes from host", config.len());
        }
    }

    // Phase 3: Mount ESP
    info!("Mounting ESP filesystem...");
    let mut esp = fs::mount_esp(image).map_err(|e| {
        log::error!("ESP mount failed: {e}");
        uefi::Error::from(e)
    })?;

    // Trust-evidence log (v0.8.3 initial version) — records every trust
    // decision to \loader\boot-trust.log for host-side audit.
    // Path G (v0.9.x) extends with crypto-signed entries and richer context.
    let mut trust_log = trust_log::TrustLog::new();
    trust_log.record(trust_log::TrustEvent::new("boot_start").with_note(&format!(
        "version={VERSION} arch={BUILD_TARGET} sb={:?} crash_counter={crash_count}",
        secure::detect_secure_boot()
    )));

    // Initialize persistent boot log (write-through during early boot)
    bootlog.start(&mut esp, VERSION, BUILD_TARGET);
    bootlog.info(Some(&mut esp), &format!("Crash counter: {crash_count}"));
    bootlog.info(
        Some(&mut esp),
        &format!(
            "System: {} {}",
            sys_info.manufacturer.as_deref().unwrap_or("unknown"),
            sys_info.product_name.as_deref().unwrap_or("")
        ),
    );
    if hypervisor.present {
        bootlog.info(
            Some(&mut esp),
            &format!(
                "Hypervisor: {}",
                hypervisor.name.as_deref().unwrap_or("unknown")
            ),
        );
    }

    // Phase 4: Load policy
    info!("Loading boot policy...");
    let policy_load = policy::load_policy(&mut esp);
    // SDS-4 Step 5: emit policy_loaded OR policy_invalid trust event.
    // The auditable record of what policy drove this boot lives here.
    match &policy_load {
        Ok((_, _)) => {
            trust_log.record(
                trust_log::TrustEvent::new("policy_loaded")
                    .with_path("/EFI/LamBoot/policy.toml")
                    .with_status(Status::SUCCESS),
            );
        }
        Err(e) => {
            trust_log.record(
                trust_log::TrustEvent::new("policy_invalid")
                    .with_path("/EFI/LamBoot/policy.toml")
                    .with_status(e.status())
                    .with_note("falling back to compiled-in defaults"),
            );
        }
    }
    let (policy, clamps) = policy_load.unwrap_or_else(|e| {
        log::warn!("Failed to load policy, using defaults: {e:?}");
        (policy::Policy::default(), alloc::vec::Vec::new())
    });
    // Surface any compiled-in floor clamps so an operator inspecting the
    // log can see that the ESP policy.toml tried to weaken a
    // security-critical field. `lamboot-inspect verify` picks these up.
    for clamp in &clamps {
        log::warn!(
            "policy.toml {} = {} clamped to floor {}",
            clamp.field,
            clamp.esp_value,
            clamp.floor_value,
        );
    }

    // Measure boot config into TPM PCR 5
    if let Ok(config_data) = esp.read_str("/EFI/LamBoot/policy.toml") {
        tpm.measure_config(&config_data);
    }
    if let Ok(manifest_data) = esp.read_str("/EFI/LamBoot/modules/manifest.toml") {
        tpm.measure_config(&manifest_data);
    }

    // Phase 5: Load filesystem drivers (ext4, etc.)
    // Before any StartImage, ask shim to retain its ShimLock protocol so we
    // can continue validating kernels after driver loads (shim #444 / sd-boot
    // precedent — shim 15.8+ reads ShimRetainProtocol variable).
    security_override::request_shim_retain_protocol();
    info!("Loading filesystem drivers...");
    let t = telemetry::timestamp_ms();
    // SDS-6: policy.drivers_legacy controls per-driver gating.
    // Default "auto" skips drivers whose filesystem is natively covered
    // (ext4/ext2 via SDS-2's Ext4Backend). "always" preserves v0.8.3
    // load-everything behavior. "never" blocks all legacy FS drivers.
    let driver_count =
        drivers::load_drivers(image, &mut esp, &tpm, &mut trust_log, policy.drivers_legacy);
    telemetry.record("drivers", t);
    // Defer trust_log flush until the boot_attempt record is queued —
    // a single flush before handoff captures the full boot's decisions.
    if driver_count > 0 {
        info!("Loaded {driver_count} filesystem driver(s)");
        bootlog.info(
            Some(&mut esp),
            &format!("Loaded {driver_count} filesystem driver(s)"),
        );
    }

    // Phase 6: Enumerate all volumes (ESP + any new ones from loaded drivers)
    let mut extra_volumes = fs::enumerate_volumes();
    info!("Found {} additional volume(s)", extra_volumes.len());

    // Phase 6.5: Scan discoverable partitions, mount XBOOTLDR (FAT), and
    // mount ext4 partitions via the SDS-2 native ext4 backend.
    let discovered_partitions = partitions::scan_discoverable_partitions();
    if !discovered_partitions.is_empty() {
        info!(
            "Found {} discoverable partition(s)",
            discovered_partitions.len()
        );
        // Probe filesystem superblocks for UUID/label identification
        for part in &discovered_partitions {
            if let Some(fs_info) = partitions::probe_superblock(part.handle) {
                info!(
                    "Partition {:?}: {} UUID={}",
                    part.partition_type,
                    match fs_info.fs_type {
                        partitions::FsType::Ext4 => "ext4",
                        partitions::FsType::Btrfs => "btrfs",
                        partitions::FsType::Xfs => "xfs",
                        partitions::FsType::F2fs => "f2fs",
                        partitions::FsType::Zfs => "zfs",
                        partitions::FsType::Unknown => "unknown",
                    },
                    fs_info.uuid.as_deref().unwrap_or("none"),
                );
            }
        }
    }
    if let Some(xbootldr) = partitions::mount_xbootldr(&discovered_partitions) {
        extra_volumes.push(xbootldr);
        info!("XBOOTLDR partition added to volume list");
    }

    // SDS-2: Mount ext4 partitions natively (no UEFI FS driver, no
    // shim-uninstall path). The canonical use case is reading /boot from
    // a Linux root partition when the distro installs kernels on ext4
    // rather than the ESP. Extended to all FsType::Ext4 volumes so future
    // SDS-5 BLS-multi-FS discovery sees them without additional work.
    for part in &discovered_partitions {
        let Some(fs_info) = partitions::probe_superblock(part.handle) else {
            continue;
        };
        if fs_info.fs_type != partitions::FsType::Ext4 {
            continue;
        }
        match fs_backend_ext4::Ext4Backend::new(part.handle, fs_info) {
            Ok(backend) => {
                // Pull identity via the FsBackend trait before we box
                // the backend (after boxing we'd still have access, but
                // fetching here keeps the Volume construction call
                // below concise).
                use fs_backend::FsBackend as _;
                let label = backend.label().map(alloc::string::String::from);
                let fs_uuid = backend.uuid();
                let identity = fs::VolumeIdentity {
                    partition_guid: Some(part.unique_guid),
                    fs_uuid,
                    label,
                    index: (extra_volumes.len() as u32) + 1,
                    backend_tag: fs_backend_ext4::EXT4_BACKEND_TAG,
                };
                let volume = fs::Volume::from_backend(identity, alloc::boxed::Box::new(backend));
                info!(
                    "Mounted native ext4 volume: {}",
                    volume.identity().describe()
                );
                // SDS-4 §6.1 Step 6: volume_mounted trust event with
                // backend tag + fs_uuid so audit consumers can trace
                // which volume sourced which kernel bytes later.
                trust_log.record(
                    trust_log::TrustEvent::new("volume_mounted").with_note(&format!(
                        "backend={} fs_uuid={} partition_guid={} index={}",
                        volume.identity().backend_tag,
                        volume
                            .identity()
                            .fs_uuid
                            .as_ref()
                            .map_or("none".into(), |u| format!("{u}")),
                        volume
                            .identity()
                            .partition_guid
                            .map_or("none".into(), |g| format!("{g}")),
                        volume.identity().index,
                    )),
                );
                extra_volumes.push(volume);
            }
            Err(e) => {
                log::warn!(
                    "Skipping ext4 partition {:?}: {} ({})",
                    part.partition_type,
                    e,
                    e.as_log_token(),
                );
            }
        }
    }

    // Phase 7: Discover boot entries (BLS across all volumes per SDS-5,
    // then ESP-only fallback for Windows/UKI/other-loaders/tools).
    bootlog.info(Some(&mut esp), "Discovering boot entries...");
    info!("Discovering boot entries...");
    let t = telemetry::timestamp_ms();

    // Compute SHA-256 of the running LamBoot image so the discovery
    // layer can recognize and skip the synthetic "EFI Fallback" entry
    // when `\EFI\BOOT\BOOTX64.EFI` is itself a copy of LamBoot (the
    // toolkit's belt-and-suspenders pattern installs LamBoot there for
    // firmware-fallback boot). Without this guard, that path produces
    // a chainload self-loop. See discovery.rs::discover_other_loaders.
    let self_image_sha256: Option<[u8; 32]> = {
        match uefi::boot::open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(image) {
            Ok(li) => {
                let info = li.info();
                let base = info.0.cast::<u8>();
                let size = info.1 as usize;
                if base.is_null() || size == 0 {
                    None
                } else {
                    // SAFETY: LoadedImageProtocol guarantees image_base()
                    // points to a valid mapped image of length image_size().
                    let bytes = unsafe { core::slice::from_raw_parts(base, size) };
                    Some(pe_loader_pure::sha256_of(bytes))
                }
            }
            Err(_) => None,
        }
    };

    // SDS-5 signature takes a unified &mut [Volume]; merge esp + extras,
    // call, then split back. `volumes[0]` is always ESP by construction;
    // this invariant is relied on by the Phase-2 ESP-only scanners.
    let mut entries = {
        let mut all_volumes: Vec<Volume> = alloc::vec![esp];
        all_volumes.append(&mut extra_volumes);
        let result = discovery::discover_all_entries(
            &mut all_volumes,
            &policy,
            &mut trust_log,
            self_image_sha256,
        );
        // Split back (preserve order: esp stays index 0, extras 1..n)
        esp = all_volumes.remove(0);
        extra_volumes = all_volumes;
        result
    };
    // SDS-4 §6.1 Step 7: entries_discovered summary event. Audit
    // consumers use this as the pivot between discovery phase and
    // user-selection phase.
    trust_log.record(
        trust_log::TrustEvent::new("entries_discovered").with_note(&format!(
            "total={} extra_volumes={}",
            entries.len(),
            extra_volumes.len()
        )),
    );
    telemetry.record("discovery", t);

    let bootable_count = entries
        .iter()
        .filter(|e| e.icon != discovery::Icon::Tools)
        .count();
    if bootable_count == 0 {
        log::warn!("No bootable entries found — showing recovery menu");
        bootlog.warn(Some(&mut esp), "No bootable entries found");
    } else {
        info!(
            "Found {} boot entries ({} total with tools)",
            bootable_count,
            entries.len()
        );
        bootlog.info(
            Some(&mut esp),
            &format!(
                "Found {bootable_count} boot entries ({} total with tools)",
                entries.len()
            ),
        );
    }

    // Phase 7.2: OS identification
    // NOTE: Extra volume scanning (identify_os, resolve_kernel_path) is disabled.
    // The ext4/btrfs drivers hang when traversing large root filesystems under
    // UEFI's single-threaded I/O. BLS entries from the ESP are sufficient for
    // Fedora/Debian — they include full kernel paths and distro names.
    // TODO: re-enable with a volume-type filter (only scan XBOOTLDR, not root)

    // Phase 7.3: Auto root= generation from discoverable partitions
    if !discovered_partitions.is_empty() {
        partitions::auto_append_root(&mut entries, &discovered_partitions);
    }

    // Phase 7.5: Preflight validation
    // Checks file existence across ALL volumes (targeted lookups, not dir scans)
    let sb_state = secure::detect_secure_boot();
    for entry in &mut entries {
        let pf = preflight::run_preflight(
            &entry.kind,
            entry.icon,
            &mut esp,
            &mut extra_volumes,
            sb_state,
            driver_count,
        );
        // Temporary instrumentation: emit a trust-log event for every
        // non-Ok preflight result so the status-bar `!` / `X` markers
        // are explainable from the on-disk log alone. Matches the
        // diagnostic pattern used to isolate the Opaque-partition
        // hang on VM 120. Remove once preflight false-positives on
        // Fedora-layout distros are fully resolved.
        for check in &pf.checks {
            if check.severity != preflight::Severity::Ok {
                let sev = match check.severity {
                    preflight::Severity::Warning => "warning",
                    preflight::Severity::Error => "error",
                    preflight::Severity::Ok => "ok", // unreachable
                };
                trust_log.record(trust_log::TrustEvent::new("preflight_issue").with_note(
                    &format!(
                        "entry_id={} severity={} msg={}",
                        entry.id, sev, check.message,
                    ),
                ));
            }
        }
        entry.preflight = Some(pf);
    }
    trust_log.flush(&mut esp);

    // Phase 8: Crash loop check
    let mut in_crash_loop = health::is_crash_loop(policy.crash_threshold);
    if in_crash_loop {
        log::warn!(
            "Crash loop detected! Counter {} >= threshold {}. Selecting fallback.",
            crash_count,
            policy.crash_threshold,
        );
    }

    // Store telemetry before entering menu
    let init_ms = telemetry.total_ms();
    info!("Boot init: {init_ms} ms");
    bootlog.info(Some(&mut esp), &format!("Boot init: {init_ms} ms"));
    telemetry.store_variable();

    // Read VMID for GUI display (if set via Proxmox SMBIOS OEM strings)
    let vmid = smbios::get_lamboot_oem_value(&oem_strings, "vmid").map(String::from);

    // Switch boot log to buffered mode (menu is interactive, crashes unlikely)
    bootlog.set_buffered();

    // Phase 9+10: Menu → boot loop.
    // Chainloaded tools (diag-shell, etc.) return to this loop after exiting.
    // Kernel/UKI boots don't return (they take over the machine).
    loop {
        let gui_sys_info = gui::SystemInfoDisplay {
            vmid: vmid.clone(),
            hypervisor: hypervisor.name.clone(),
            system_product: sys_info.product_name.clone(),
            secure_boot: !matches!(sb_state, secure::SecureBootState::Disabled),
            driver_count,
        };

        let menu_result = if in_crash_loop {
            if let Some(entry) = select_fallback(&entries, &policy) {
                Ok(entry)
            } else if console::has_graphics() {
                let mut gui_ctx = gui::GuiContext::new()?;
                gui_ctx.set_system_info(gui_sys_info);
                gui_ctx.run_menu(&entries, &policy)
            } else {
                let mut text_menu = console::TextMenu::new();
                text_menu.run_menu(&entries, &policy)
            }
        } else if console::has_graphics() {
            let mut gui_ctx = gui::GuiContext::new()?;
            gui_ctx.set_system_info(gui_sys_info);
            gui_ctx.run_menu(&entries, &policy)
        } else {
            let mut text_menu = console::TextMenu::new();
            text_menu.run_menu(&entries, &policy)
        };

        let selection = match menu_result {
            Ok(entry) => entry,
            Err(e) => {
                log::error!("Menu failed: {e}. Rebooting to firmware setup.");
                health::reboot_to_firmware_setup();
            }
        };

        let is_tool = selection.icon == discovery::Icon::Tools;

        bootlog.info(
            None,
            &format!("Selected: {} ({})", selection.name, selection.id),
        );
        bootlog.flush(&mut esp);

        let _ = health::record_boot_entry(&selection.id);
        let boot_ctx = report::BootContext {
            sys_info: sys_info.clone(),
            fleet_id: fleet_id.clone(),
            os_name: None,
            hypervisor: hypervisor.name.clone(),
            iommu: iommu.clone(),
            vmid: vmid.clone(),
            telemetry_json: telemetry.to_json(),
        };
        report::write_boot_report(&mut esp, &selection, &boot_ctx)?;

        if let Some(ref bls_file) = selection.bls_filename {
            let entries_dir = "/loader/entries";
            // SDS-5 PR-3: route to the SOURCE volume, not always ESP.
            // BLS entries on ext4 /boot have bls_filename too, but the
            // .conf file lives on ext4 — reading from ESP would silently
            // fail (NotFound), losing the measurement + rename opportunity.
            let vol_idx = selection.source_volume_index;
            let source_volume: &mut fs::Volume = if vol_idx == 0 {
                &mut esp
            } else if vol_idx - 1 < extra_volumes.len() {
                &mut extra_volumes[vol_idx - 1]
            } else {
                // Stale index — shouldn't happen given the split back
                // in phase 7 preserves order. Fall back to ESP so the
                // measurement path still fires.
                log::warn!(
                    "bls source_volume_index {vol_idx} out of range (extra_volumes.len()={}) — measuring from ESP",
                    extra_volumes.len()
                );
                &mut esp
            };
            let source_backend = source_volume.backend_tag();
            if let Ok(content) =
                source_volume.read_to_string_str(&format!("{entries_dir}/{bls_file}"))
            {
                tpm.measure_bls_entry(content.as_bytes(), &selection.id);
                if let Some(bls_entry) = bls::BlsEntry::parse(bls_file, &content) {
                    if bls_entry.tries_left.is_some() {
                        // SDS-5 PR-3: counter rename requires FAT. On RO
                        // ext4, emit boot_counter_skipped_ro instead of
                        // attempting (which would fail cryptically inside
                        // EspWriter::new). Operator sees the skip + can
                        // migrate to UKI or ESP-BLS if they want counters.
                        if source_backend == fs_backend_fat::FatBackend::TAG {
                            bls::decrement_boot_count(source_volume, &bls_entry);
                        } else {
                            trust_log.record(
                                trust_log::TrustEvent::new("boot_counter_skipped_ro")
                                    .with_path(bls_file)
                                    .with_note(&format!(
                                        "volume_index={vol_idx} backend={source_backend} \
                                         tries_left={}",
                                        bls_entry.tries_left.unwrap_or(0)
                                    )),
                            );
                            log::warn!(
                                "Boot counter for {bls_file} not decremented — source \
                                 volume is read-only ({source_backend}). \
                                 systemd-bless-boot will not work for this entry. \
                                 See SPEC-BLS-MULTI-FS §6.2 for workarounds."
                            );
                        }
                    }
                }
            }
        }

        if let discovery::EntryKind::LinuxLegacy { ref options, .. }
        | discovery::EntryKind::Uki { ref options, .. } = selection.kind
        {
            tpm.measure_cmdline(options);
        }

        info!("Booting: {}", selection.name);
        let selection_name = selection.name.clone();
        let selection_id = selection.id.clone();
        // SDS-4 §6.1 Step 8: entry_selected (audit of the final choice
        // regardless of whether user picked or menu timed out).
        trust_log.record(
            trust_log::TrustEvent::new("entry_selected")
                .with_path(&selection_name)
                .with_note(&format!("id={selection_id}")),
        );
        trust_log.record(
            trust_log::TrustEvent::new("boot_attempt")
                .with_path(&selection_name)
                .with_note(&format!("id={} kind={:?}", selection_id, selection.kind)),
        );
        let mut all_volumes = alloc::vec![esp];
        all_volumes.append(&mut extra_volumes);
        // Flush before handoff so the log reflects everything up to this point
        // even if the child image never returns.
        trust_log.flush(&mut all_volumes[0]);
        let result = boot::boot_entry(
            image,
            &mut all_volumes,
            selection,
            &tpm,
            &policy,
            &mut trust_log,
        );

        // If boot_entry returned an Err (kernel load rejected, missing file,
        // etc.) capture it to the trust log so the next boot's diagnostics
        // can read the rejection reason from \loader\boot-trust.log.
        // The most common case is SecurityOverride→firmware-DB rejecting a
        // MOK-chained distro kernel when shim wasn't in the boot chain; this
        // event makes that failure mode visible on the ESP rather than lost
        // when control returns to firmware.
        if let Err(ref e) = result {
            let status = e.status();
            let (hook2_calls, shim_calls, shim_true, delegate_calls, shimlock_not_found) =
                security_override::diagnostic_counters();
            let note = format!(
                "status hint: ACCESS_DENIED/SECURITY_VIOLATION typically means kernel rejected by firmware DB after shim_validate returned false; \
                 hook_security2 invocations={hook2_calls}, shim_validate calls={shim_calls} (true={shim_true}), \
                 delegations to original={delegate_calls}, ShimLock handle not found={shimlock_not_found}"
            );
            trust_log.record(
                trust_log::TrustEvent::new("kernel_load_failed")
                    .with_path(&selection_name)
                    .with_status(status)
                    .with_note(&note),
            );
            trust_log.flush(&mut all_volumes[0]);
        }

        if is_tool {
            // Tool returned — reclaim volumes and loop back to menu
            esp = all_volumes.swap_remove(0);
            extra_volumes = all_volumes;
            info!("Tool returned, re-entering menu");
            // After returning from a tool, don't auto-boot again
            in_crash_loop = false;
            continue;
        }

        // Kernel/UKI boot — if we get here, the image returned (unusual)
        let _ = health::mark_boot_success();
        return result;
    }
}

/// Try to find a configured fallback entry.
fn select_fallback(
    entries: &[discovery::BootEntry],
    policy: &policy::Policy,
) -> Option<discovery::BootEntry> {
    for fallback_id in &policy.fallback_order {
        if let Some(entry) = entries.iter().find(|e| &e.id == fallback_id) {
            log::info!("Using fallback entry: {} ({})", entry.name, entry.id);
            return Some(entry.clone());
        }
    }
    log::warn!("No fallback entry configured");
    None
}
