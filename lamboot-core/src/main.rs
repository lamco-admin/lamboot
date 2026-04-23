#![no_main]
#![no_std]

extern crate alloc;

mod acpi;
mod autodiscovery;
mod bls;
mod boot;
mod bootlog;
mod console;
mod discovery;
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
mod policy;
mod preflight;
mod report;
mod secure;
mod security_override;
mod smbios;
mod telemetry;
mod tpm;
mod trust_log;
mod uki;

use alloc::{format, string::String};

use log::info;
use uefi::{prelude::*, Result};

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
    // decision to \loader\boot-trust.log for host-side audit. See
    // docs/analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md §9.
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
    let policy = policy::load_policy(&mut esp).unwrap_or_else(|e| {
        log::warn!("Failed to load policy, using defaults: {e:?}");
        policy::Policy::default()
    });

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
    let driver_count = drivers::load_drivers(image, &mut esp, &tpm, &mut trust_log);
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

    // Phase 6.5: Scan discoverable partitions and mount XBOOTLDR
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

    // Phase 7: Discover boot entries (BLS first, then ESP fallback)
    bootlog.info(Some(&mut esp), "Discovering boot entries...");
    info!("Discovering boot entries...");
    let t = telemetry::timestamp_ms();
    let mut entries = discovery::discover_all_entries(&mut esp, &mut extra_volumes, &policy);
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
        entry.preflight = Some(preflight::run_preflight(
            &entry.kind,
            entry.icon,
            &mut esp,
            &mut extra_volumes,
            sb_state,
            driver_count,
        ));
    }

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
            if let Ok(content) = esp.read_to_string_str(&format!("{entries_dir}/{bls_file}")) {
                tpm.measure_bls_entry(content.as_bytes(), &selection.id);
                if let Some(bls_entry) = bls::BlsEntry::parse(bls_file, &content) {
                    if bls_entry.tries_left.is_some() {
                        bls::decrement_boot_count(&mut esp, &bls_entry);
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
        let result = boot::boot_entry(image, &mut all_volumes, selection, &tpm);

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
                 delegations to original={delegate_calls}, ShimLock handle not found={shimlock_not_found}; \
                 see docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21.md"
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
