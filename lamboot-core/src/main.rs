//! Layer: 7 — Orchestration.

#![no_main]
#![no_std]

extern crate alloc;

mod acpi;
mod autodiscovery;
mod block_source;
mod bls;
mod bls_parse;
mod boot;
mod boot_entry;
mod boot_entry_pure;
mod boot_route_pure;
mod boot_types;
mod bootlog;
mod console;
mod diag;
mod discovery;
mod discovery_pure;
mod distro_iso;
mod drivers;
mod firmware_quirks;
mod fs;
mod fs_backend;
mod fs_backend_btrfs;
mod fs_backend_exfat;
mod fs_backend_ext4;
mod fs_backend_fat;
mod fs_backend_fat_ro;
mod fs_backend_lamfold;
mod fs_backend_lvm;
mod fs_backend_lvm_btrfs;
mod fs_backend_lvm_dispatch;
mod fs_backend_lvm_fat;
mod fs_backend_xfs;
mod fs_backend_zfs;
mod fs_shared;
mod fs_types;
mod fs_writer;
mod fw_cfg;
mod fw_cfg_config;
mod gui;
mod health;
mod hypervisor;
mod initrd;
mod input;
mod loopback_cfg;
mod partition_classify_pure;
mod partitions;
mod pe_loader;
mod pe_loader_pure;
mod policy;
mod preflight;
mod read_limit_pure;
mod report;
mod sb_classify_pure;
mod secure;
mod security_override;
mod smbios;
mod telemetry;
mod tpm;
mod trust_log;
mod trust_log_pure;
mod uki;
mod version;

use alloc::{format, string::String, vec::Vec};

use log::info;
use uefi::{prelude::*, Result};

use crate::fs::Volume;

const WATCHDOG_TIMEOUT_SECONDS: usize = 300; // 5 minutes

/// Build information embedded at compile time.
/// Re-export from [`crate::version`] so existing call sites keep working;
/// new code should reach into [`crate::version`] directly.
pub(crate) use crate::version::VERSION;

#[cfg(target_arch = "x86_64")]
pub(crate) const BUILD_TARGET: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
pub(crate) const BUILD_TARGET: &str = "aarch64";

#[entry]
fn efi_main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::ABORTED;
    }

    let image = uefi::boot::image_handle();

    info!("LamBoot v{VERSION} ({BUILD_TARGET}) starting...");

    // Set watchdog timer to prevent hangs. Best-effort: quirky firmware can
    // reject this (UNSUPPORTED / INVALID_PARAMETER / DEVICE_ERROR), and a
    // missing watchdog is no reason to refuse to boot — degrade, don't panic.
    if let Err(e) = uefi::boot::set_watchdog_timer(WATCHDOG_TIMEOUT_SECONDS, 0x10000, None) {
        log::warn!("set_watchdog_timer failed ({e:?}) — continuing without watchdog");
    }

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

    // Phase 1: Health assessment (infallible — NVRAM persistence is best-effort)
    let t = telemetry::timestamp_ms();
    let crash_count = health::assess_boot_health();
    telemetry.record("health", t);

    // Phase 2: Security checks
    // Detect the Secure Boot posture ONCE for the whole boot. The SecureBoot
    // variable and the ShimLock protocol presence are stable for a boot, so
    // this single value is threaded into the startup log, the boot_start trust
    // event, preflight, the boot.json report, and the kernel verify path —
    // they can no longer disagree (the prior code re-detected at each site).
    let sb_state = secure::detect_secure_boot();
    secure::log_secure_boot_state(sb_state);
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
    // SMBIOS-derived defaults (pre-0.8.4 hookscript pattern). These are
    // kept as fallback for legacy hosts that still inject metadata via
    // `qm set --smbios=...`. The v0.8.4+ hookscript writes fw_cfg JSON
    // instead; we read it below in Phase 2.8 and let those values
    // override the SMBIOS-derived ones.
    let mut fleet_id = smbios::get_lamboot_oem_value(&oem_strings, "fleet-id").map(String::from);
    let mut vmid = smbios::get_lamboot_oem_value(&oem_strings, "vmid").map(String::from);
    let mut role: Option<String> = None;
    let mut host_node: Option<String> = None;

    // Phase 2.5b: per-firmware quirk detection. Match SMBIOS Mfr+Product
    // against the known-quirky-systems table. On vanilla modern systems
    // this is all-false; on quirky firmware (e.g. ASUS G10AJ) flags
    // enable workarounds. Step 1 of the gating rollout: detection +
    // trust-event emission only. Subsequent commits gate the hardcoded
    // workarounds behind these flags.
    let firmware_quirks = firmware_quirks::FirmwareQuirks::detect(&sys_info);

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

    // Phase 2.8: fw_cfg data channel (host-injected config). v0.11.16:
    // parse the JSON blob written by lamboot-hookscript.pl (v0.8.4+) into
    // structured fields and let those override the SMBIOS-derived
    // values above. SMBIOS path stays as fallback for legacy hosts.
    if fw_cfg::fw_cfg_available() {
        if let Some(config) = fw_cfg::read_file_string("opt/lamboot/config") {
            info!("fw_cfg config: {} bytes from host", config.len());
            let parsed = fw_cfg_config::FwCfgConfig::parse(&config);
            if parsed.fleet_id.is_some() {
                fleet_id = parsed.fleet_id;
            }
            if parsed.vmid.is_some() {
                vmid = parsed.vmid;
            }
            // role + host_node have no SMBIOS equivalent — fw_cfg is the
            // only source. Empty values stay None.
            role = parsed.role;
            host_node = parsed.host_node;
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
    // Quirk gates on the trust log:
    //   conout_fat_coupling -> push-only record (no log:: macro inside
    //     record, which hangs the firmware FAT driver on G10AJ class);
    //   conout_fat_coupling -> use the /EFI/LamBoot/reports/trust.log
    //     path (known-safe on G10AJ) instead of the BLS-spec
    //     `\loader\boot-trust.log`. On modern firmware the spec path
    //     and dual-emission via the log facade both work.
    trust_log.set_quiet(firmware_quirks.conout_fat_coupling);
    if firmware_quirks.conout_fat_coupling {
        trust_log.set_log_path("/EFI/LamBoot/reports/trust.log");
        // On this firmware ANY log:: emission goes to ConOut, and a ConOut
        // write stalls the next FAT operation (the conout↔FAT coupling). The
        // hot paths were hand-gated, but per-site gating is whack-a-mole: the
        // v0.11.15 LVM-FS dispatch refactor added ungated log::info!/log::warn!
        // in the LV-open path (fs_backend_lvm_dispatch.rs), which fires on
        // every boot of an LVM root and re-triggered the freeze on pve2.
        // Suppress the log facade globally here — after quirk detection — so no
        // macro, present or future, can emit ConOut on coupling-prone firmware.
        // The auditable record is unaffected: trust.log/audit.log use
        // EspWriter::true_append, which does not go through the log facade.
        log::set_max_level(log::LevelFilter::Off);
    }
    trust_log.record(trust_log::TrustEvent::new("boot_start").with_note(&format!(
        "version={VERSION} arch={BUILD_TARGET} sb={sb_state:?} crash_counter={crash_count}"
    )));
    // Auditable record of which firmware-quirk workarounds will apply
    // for this boot. Empty summary = "none" = modern firmware default.
    trust_log.record(
        trust_log::TrustEvent::new("firmware_quirks_applied").with_note(&firmware_quirks.summary()),
    );

    // v0.11.0 diag: input subsystem surface. Count of protocol handles
    // that the menu's InputManager would try to use. Surfaces "no
    // keyboard handle" / "no pointer handle" cases that would render
    // the menu non-interactive on real hardware (observed on pve2:
    // arrow keys don't change selection, mouse doesn't move).
    {
        use uefi::proto::{
            console::{pointer::Pointer, text::Input},
            usb::io::UsbIo,
        };
        let n_text_input = uefi::boot::find_handles::<Input>().map_or(0, |v| v.len());
        let n_pointer = uefi::boot::find_handles::<Pointer>().map_or(0, |v| v.len());
        let n_usb_io = uefi::boot::find_handles::<UsbIo>().map_or(0, |v| v.len());
        trust_log.record(
            trust_log::TrustEvent::new("input_subsystem").with_note(&format!(
                "simple_text_input={n_text_input} simple_pointer={n_pointer} usb_io={n_usb_io}"
            )),
        );
    }

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
    // v0.11.0: arm gated diag breadcrumbs from policy. Default off — one
    // atomic load per call site when off, FAT write when on.
    diag::set_enabled(policy.diagnostics_verbose);
    // On conout_fat_coupling firmware, route verbose breadcrumbs through a RAM
    // buffer flushed once before handoff instead of per-call FAT writes, so
    // enabling diagnostics cannot add FAT-write churn to the post-selection
    // band on hardware whose FAT driver couples with ConOut.
    diag::set_coalesce(firmware_quirks.conout_fat_coupling);

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

    // Measure boot config into TPM PCR 5 (best-effort; no paired trust event,
    // so the measure outcome is intentionally ignored here).
    if let Ok(config_data) = esp.read_str("/EFI/LamBoot/policy.toml") {
        let _ = tpm.measure_config(&config_data);
    }
    if let Ok(manifest_data) = esp.read_str("/EFI/LamBoot/modules/manifest.toml") {
        let _ = tpm.measure_config(&manifest_data);
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
                        partitions::FsType::ExFat => "exfat",
                        partitions::FsType::Unknown => "unknown",
                    },
                    fs_info.uuid.as_deref().unwrap_or("none"),
                );
            }
        }
    }
    match partitions::mount_xbootldr(&discovered_partitions) {
        Some(xbootldr) => {
            // Provenance: record WHICH backend read the XBOOTLDR /boot — the
            // native FatRo reader (`fatfs-ro@…`) when the firmware refused the
            // volume, or the firmware FAT backend (`fat`) when it served it.
            // Same `volume_mounted` event the ext4/btrfs/LVM mounts emit, so a
            // host-side audit can trace the kernel source uniformly instead of
            // inferring it. `role=xbootldr` distinguishes it from a root mount.
            trust_log.record(
                trust_log::TrustEvent::new("volume_mounted").with_note(&format!(
                    "backend={} fs_uuid={} partition_guid={} index={} role=xbootldr",
                    xbootldr.identity().backend_tag,
                    xbootldr
                        .identity()
                        .fs_uuid
                        .as_ref()
                        .map_or("none".into(), |u| format!("{u}")),
                    xbootldr
                        .identity()
                        .partition_guid
                        .map_or("none".into(), |g| format!("{g}")),
                    xbootldr.identity().index,
                )),
            );
            info!("Mounted XBOOTLDR /boot: {}", xbootldr.identity().describe());
            extra_volumes.push(xbootldr);
        }
        None => {
            // An XBOOTLDR partition that exists but cannot be mounted by either
            // the firmware or FatRo is an auditable condition, not a silent
            // skip — record it so a `/boot` LamBoot can't read is visible in
            // the trust log rather than only inferable from a missing entry.
            if discovered_partitions
                .iter()
                .any(|p| p.partition_type == partitions::PartType::Xbootldr)
            {
                trust_log.record(trust_log::TrustEvent::new("xbootldr_mount_failed").with_note(
                    "XBOOTLDR partition present but unmountable (firmware refused and FatRo failed)",
                ));
            }
        }
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

    // Path A: Mount btrfs partitions natively via lambutter. Same
    // architectural slot as the ext4 loop above; replaces the legacy
    // EfiFs `btrfs_x64.efi` driver path (GRUB-derived, GPLv3, untrusted
    // for Microsoft shim-review). lambutter resolves the Snapper-managed
    // default-subvol redirect (OID-6 DIR_ITEM "default") at open time so
    // openSUSE Tumbleweed / Leap reads land in the active snapshot.
    for part in &discovered_partitions {
        let Some(fs_info) = partitions::probe_superblock(part.handle) else {
            continue;
        };
        if fs_info.fs_type != partitions::FsType::Btrfs {
            continue;
        }
        match fs_backend_btrfs::BtrfsBackend::new(part.handle, &fs_info) {
            Ok(backend) => {
                use fs_backend::FsBackend as _;
                let label = backend.label().map(alloc::string::String::from);
                let fs_uuid = backend.uuid();
                let identity = fs::VolumeIdentity {
                    partition_guid: Some(part.unique_guid),
                    fs_uuid,
                    label,
                    index: (extra_volumes.len() as u32) + 1,
                    backend_tag: fs_backend_btrfs::BTRFS_BACKEND_TAG,
                };
                let volume = fs::Volume::from_backend(identity, alloc::boxed::Box::new(backend));
                info!(
                    "Mounted native btrfs volume: {}",
                    volume.identity().describe()
                );
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
                    "Skipping btrfs partition {:?}: {} ({})",
                    part.partition_type,
                    e,
                    e.as_log_token(),
                );
            }
        }
    }

    // Path B: Mount XFS partitions natively via lamxfs. Same architectural slot
    // as the ext4/btrfs loops above; closes the #1 RHEL-family de-bundling gap
    // (XFS is the default /boot + / on RHEL/Rocky/Alma/Oracle/Amazon), replacing
    // the GPLv3 EfiFs `xfs_x64.efi` driver path that is untrusted for Microsoft
    // shim-review. lamxfs holds the partition's BlockIO exclusive for the
    // backend's lifetime, so the Step-4 media loop below skips it (no
    // double-mount), exactly as the ext4/btrfs loops do.
    for part in &discovered_partitions {
        let Some(fs_info) = partitions::probe_superblock(part.handle) else {
            continue;
        };
        if fs_info.fs_type != partitions::FsType::Xfs {
            continue;
        }
        match fs_backend_xfs::XfsBackend::new(part.handle) {
            Ok(backend) => {
                use fs_backend::FsBackend as _;
                let label = backend.label().map(alloc::string::String::from);
                let fs_uuid = backend.uuid();
                let identity = fs::VolumeIdentity {
                    partition_guid: Some(part.unique_guid),
                    fs_uuid,
                    label,
                    index: (extra_volumes.len() as u32) + 1,
                    backend_tag: fs_backend_xfs::XFS_BACKEND_TAG,
                };
                let volume = fs::Volume::from_backend(identity, alloc::boxed::Box::new(backend));
                info!(
                    "Mounted native XFS volume: {}",
                    volume.identity().describe()
                );
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
                    "Skipping XFS partition {:?}: {} ({})",
                    part.partition_type,
                    e,
                    e.as_log_token(),
                );
            }
        }
    }

    // Mount exFAT partitions natively via lamexfat. exFAT is removable/utility
    // media (USB stick, SD card) carrying a kernel + initrd or a boot image —
    // never the ESP and never a Linux root, so it does not overlap the loops
    // above. Same exclusive-BlockIO discipline: the Step-4 media loop skips it.
    for part in &discovered_partitions {
        let Some(fs_info) = partitions::probe_superblock(part.handle) else {
            continue;
        };
        if fs_info.fs_type != partitions::FsType::ExFat {
            continue;
        }
        match fs_backend_exfat::ExFatBackend::new(part.handle) {
            Ok(backend) => {
                use fs_backend::FsBackend as _;
                let label = backend.label().map(alloc::string::String::from);
                let fs_uuid = backend.uuid();
                let identity = fs::VolumeIdentity {
                    partition_guid: Some(part.unique_guid),
                    fs_uuid,
                    label,
                    index: (extra_volumes.len() as u32) + 1,
                    backend_tag: fs_backend_exfat::EXFAT_BACKEND_TAG,
                };
                let volume = fs::Volume::from_backend(identity, alloc::boxed::Box::new(backend));
                info!(
                    "Mounted native exFAT volume: {}",
                    volume.identity().describe()
                );
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
                    "Skipping exFAT partition {:?}: {} ({})",
                    part.partition_type,
                    e,
                    e.as_log_token(),
                );
            }
        }
    }

    // Mount ZFS boot pools natively via lamzfs. A pool spans one or more
    // whole-partition members (mirror / raidz), so the ZFS-labeled partitions are
    // grouped by pool GUID, each pool is imported once, and every dataset it
    // holds (each a separate ZPL filesystem) becomes its own Volume. Same
    // exclusive-BlockIO discipline as the loops above: a member's BlockIO is held
    // for the pool's lifetime, so the Step-4 media loop skips it. ZFS members are
    // never the ESP and never read by the ext4/btrfs/xfs backends, so there is no
    // overlap.
    {
        type ZfsMember =
            lamzfs::PoolMember<block_source::SourceReader<block_source::BlockIoSource>>;
        // pool GUID -> (members held open, a representative partition GUID).
        let mut by_pool: alloc::collections::BTreeMap<
            u64,
            (alloc::vec::Vec<ZfsMember>, uefi::Guid),
        > = alloc::collections::BTreeMap::new();
        for part in &discovered_partitions {
            let Some(fs_info) = partitions::probe_superblock(part.handle) else {
                continue;
            };
            if fs_info.fs_type != partitions::FsType::Zfs {
                continue;
            }
            let Ok(source) = block_source::BlockIoSource::open(part.handle) else {
                continue; // unreadable or already held
            };
            let reader = block_source::SourceReader::new(source);
            let device_size_bytes = reader.byte_len();
            let mut member = lamzfs::PoolMember {
                reader,
                device_size_bytes,
            };
            match lamzfs::peek_pool_id(&mut member) {
                Ok(id) => {
                    by_pool
                        .entry(id.guid)
                        .or_insert_with(|| (alloc::vec::Vec::new(), part.unique_guid))
                        .0
                        .push(member);
                }
                Err(e) => log::warn!(
                    "Skipping ZFS partition {:?}: peek failed ({})",
                    part.partition_type,
                    e.token(),
                ),
            }
        }
        for (pool_guid, (members, rep_guid)) in by_pool {
            let member_count = members.len();
            match fs_backend_zfs::ZfsBackend::mount_pool(members) {
                Ok(backends) => {
                    for backend in backends {
                        use fs_backend::FsBackend as _;
                        let label = backend.label().map(alloc::string::String::from);
                        let identity = fs::VolumeIdentity {
                            partition_guid: Some(rep_guid),
                            fs_uuid: None,
                            label,
                            index: (extra_volumes.len() as u32) + 1,
                            backend_tag: fs_backend_zfs::ZFS_BACKEND_TAG,
                        };
                        let volume =
                            fs::Volume::from_backend(identity, alloc::boxed::Box::new(backend));
                        info!(
                            "Mounted native ZFS volume: {}",
                            volume.identity().describe()
                        );
                        trust_log.record(trust_log::TrustEvent::new("volume_mounted").with_note(
                            &format!(
                                "backend={} pool_guid={pool_guid:#018x} members={member_count} index={}",
                                volume.identity().backend_tag,
                                volume.identity().index,
                            ),
                        ));
                        extra_volumes.push(volume);
                    }
                }
                Err(e) => log::warn!(
                    "Skipping ZFS pool {pool_guid:#018x} ({member_count} member(s)): {e} ({})",
                    e.as_log_token(),
                ),
            }
        }
    }

    // Step 4 (SPEC-LAMFOLD-INTEGRATION §5): standalone media filesystems
    // (erofs / squashfs / iso / cramfs / romfs / udf) as read-only roots or
    // /boot, via the lamfold stack. Runs after the ext4/btrfs loops, which hold
    // their partitions' BlockIO *exclusive* — so `BlockIoSource::open` here fails
    // on those handles and the partition is skipped. `dispatch_media` mounts ONLY
    // the media variants (ext4/btrfs/FAT → `Unsupported`), so there is never a
    // duplicate Volume. The ESP is already excluded by
    // `scan_discoverable_partitions`, so exclusive-opening these handles is safe
    // (the same thing the ext4 loop's `probe_superblock` does).
    for part in &discovered_partitions {
        let Ok(source) = block_source::BlockIoSource::open(part.handle) else {
            continue; // held by an ext4/btrfs/FatRo mount above, or unreadable
        };
        match fs_backend_lvm_dispatch::dispatch_media_fs_over_source(source) {
            Ok(backend) => {
                // `backend` is `Box<dyn FsBackend>`, so its trait methods resolve
                // through the trait object — no `use FsBackend` needed here.
                let identity = fs::VolumeIdentity {
                    partition_guid: Some(part.unique_guid),
                    fs_uuid: backend.uuid(),
                    label: backend.label().map(alloc::string::String::from),
                    index: (extra_volumes.len() as u32) + 1,
                    backend_tag: backend.tag(),
                };
                let volume = fs::Volume::from_backend(identity, backend);
                info!(
                    "Mounted native media volume: {}",
                    volume.identity().describe()
                );
                trust_log.record(
                    trust_log::TrustEvent::new("volume_mounted").with_note(&format!(
                        "backend={} fs_uuid={} partition_guid={} index={} role=media",
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
                // Not a media filesystem (ext4/btrfs/FAT/unknown) — expected for
                // most partitions, so debug-level to keep the boot log clean.
                log::debug!(
                    "partition {:?} is not a lamfold media filesystem: {} ({})",
                    part.partition_type,
                    e,
                    e.as_log_token(),
                );
            }
        }
    }

    // SDS-LVM (v0.11.0): mount ext4 filesystems that live on a logical
    // volume. Canonical case is Proxmox VE — installer puts /boot under
    // `pve-root` (linear LV on a single PV). Coexistence path: GRUB
    // chainloads `lambootx64.efi`, Lamboot then needs to read
    // /boot/loader/entries/*.conf from the LV.
    //
    // Strategy: any partition whose first 512 bytes have the LABELONE
    // signature is a PV candidate. We parse the VG and attempt to mount
    // every LV whose name appears in a curated allow-list
    // (`root`, `pve-root`, `system`, `ubuntu-lv`, `centos-root`, etc.).
    // Failures are logged and skipped — non-Proxmox LVs that aren't
    // ext4 produce one INFO line each rather than aborting boot.
    const LV_NAME_CANDIDATES: &[&str] = &[
        "root",      // Proxmox (vg "pve"), most generic distros
        "pve-root",  // some older Proxmox docs
        "system",    // Ubuntu Mint server installer default
        "ubuntu-lv", // Ubuntu desktop installer default
        "rootlv",    // RHEL/CentOS
    ];
    // v0.11.0: LVM probe candidate set. Primary path uses GPT-typed
    // partitions (scan_discoverable_partitions). On firmware that
    // doesn't install PartitionInfo on every partition (observed on
    // ASUS G10AJ / pve2: only the ESP has PartitionInfo, sda3 LVM
    // doesn't), the GPT-typed scan comes back empty. Fall back to
    // a non-exclusive BlockIO walk that probes each logical-partition
    // handle for the LABELONE signature directly. The fallback uses
    // OpenProtocolAttributes::GetProtocol (non-disturbing) and only
    // returns handles that already passed the signature check —
    // safe to subsequently exclusive-open in LvmExt4Backend because
    // raw LVM partitions have no firmware FS driver attached.
    //
    // First iteration of this fallback used open_protocol_exclusive
    // which disconnected the ESP's SFS driver and killed the boot.
    // That bug + the fix is documented in partitions::scan_block_io_partitions_for_lvm.
    let lvm_candidate_handles: Vec<(uefi::Handle, &'static str, uefi::data_types::Guid)> = {
        let mut out: Vec<(uefi::Handle, &'static str, uefi::data_types::Guid)> = Vec::new();
        if discovered_partitions.is_empty() {
            for handle in partitions::scan_block_io_partitions_for_lvm() {
                out.push((handle, "blockio_fallback", uefi::data_types::Guid::ZERO));
            }
        } else {
            for part in &discovered_partitions {
                out.push((part.handle, "gpt_typed", part.unique_guid));
            }
        }
        out
    };

    // v0.11.0 diagnostic: emit a trust event summarizing the LVM-probe
    // pass so post-boot inspection of the trust log can answer "did
    // LamBoot's LVM detection even try?" without console access.
    trust_log.record(
        trust_log::TrustEvent::new("lvm_probe_pass_start").with_note(&format!(
            "discovered_partitions={} candidate_handles={} source={}",
            discovered_partitions.len(),
            lvm_candidate_handles.len(),
            if discovered_partitions.is_empty() {
                "blockio_fallback"
            } else {
                "gpt_typed"
            },
        )),
    );
    let mut lvm_pv_count = 0usize;
    for (handle, source, unique_guid) in &lvm_candidate_handles {
        let probe_hit = partitions::probe_lvm_pv(*handle);
        trust_log.record(
            trust_log::TrustEvent::new("lvm_pv_probe").with_note(&format!(
                "source={source} partition_guid={unique_guid} signature_found={probe_hit}",
            )),
        );
        if !probe_hit {
            continue;
        }
        lvm_pv_count += 1;
        info!("LVM PV signature detected (source={source})");
        for lv_name in LV_NAME_CANDIDATES {
            // v0.11.14 — single dispatch via fs_backend_lvm_dispatch:
            // one PV/VG/LV parse, one superblock probe, one backend
            // construction. Replaces v0.11.13's cascading try-ext4-
            // then-try-btrfs which doubled the LVM metadata parse on
            // the fallback path and didn't scale to a third backend.
            //
            // Adding a new LVM-FS backend (e.g. LvmXfsBackend) lives
            // entirely inside fs_backend_lvm_dispatch — see its
            // "Adding a new LVM-FS backend" comment.
            match fs_backend_lvm_dispatch::open_lvm_lv_backend(*handle, lv_name) {
                Ok(dispatch) => {
                    let identity = fs::VolumeIdentity {
                        partition_guid: Some(*unique_guid),
                        fs_uuid: dispatch.fs_uuid,
                        label: dispatch.label.clone(),
                        index: (extra_volumes.len() as u32) + 1,
                        backend_tag: dispatch.backend_tag,
                    };
                    let vg_lv = dispatch.vg_lv.clone();
                    let backend_tag = dispatch.backend_tag;
                    let volume = fs::Volume::from_backend(identity, dispatch.backend);
                    info!(
                        "Mounted native LVM volume: {} (backend={} lv={})",
                        volume.identity().describe(),
                        backend_tag,
                        vg_lv,
                    );
                    trust_log.record(trust_log::TrustEvent::new("volume_mounted").with_note(
                        &format!(
                            "backend={} fs_uuid={} partition_guid={} lv={} index={}",
                            backend_tag,
                            volume
                                .identity()
                                .fs_uuid
                                .as_ref()
                                .map_or("none".into(), |u| format!("{u}")),
                            volume
                                .identity()
                                .partition_guid
                                .map_or("none".into(), |g| format!("{g}")),
                            vg_lv,
                            volume.identity().index,
                        ),
                    ));
                    extra_volumes.push(volume);
                    // Stop at the first successful LV mount per PV
                    // — we don't want to mount /boot twice if both
                    // `root` and a custom rename happen to exist.
                    break;
                }
                Err(e) => {
                    let token = e.as_log_token();
                    log::info!("LV '{lv_name}' on candidate (source={source}): {e} ({token})");
                    trust_log.record(trust_log::TrustEvent::new("lvm_lv_open_failed").with_note(
                        &format!(
                            "lv_name={lv_name} source={source} partition_guid={unique_guid} error_token={token}",
                        ),
                    ));
                }
            }
        }
    }
    trust_log.record(
        trust_log::TrustEvent::new("lvm_probe_pass_end")
            .with_note(&format!("pv_signatures_found={lvm_pv_count}")),
    );

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
    let (mut entries, cohort_notice) = {
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
        // v0.10 Bug 22 / Option I: extract the cohort-split notice
        // (None when not applicable). Passed into the menu UI to
        // render as a footer line.
        let notice = result.cohort_split.map(|c| c.notice);
        (result.entries, notice)
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

    // (b) Ensure the persistent named "LamBoot" NVRAM entry exists — the
    // OS-independent boot-entry pathway (no efibootmgr, no SELinux in the way).
    // Idempotent: a no-op when the entry already exists, so it runs every boot
    // but writes NVRAM only on first install. Only attempted when the canonical
    // install binary is actually present on this ESP, so we never point an entry
    // at a missing file (e.g. a pure removable-media boot with no \EFI\LamBoot).
    #[cfg(target_arch = "x86_64")]
    const LAMBOOT_INSTALL_PATH: &str = "\\EFI\\LamBoot\\lambootx64.efi";
    #[cfg(target_arch = "aarch64")]
    const LAMBOOT_INSTALL_PATH: &str = "\\EFI\\LamBoot\\lambootaa64.efi";
    if policy.self_install_boot_entry && esp.exists_str(LAMBOOT_INSTALL_PATH) {
        match boot_entry::ensure_named_boot_entry(LAMBOOT_INSTALL_PATH, "LamBoot") {
            boot_entry::SelfInstall::Created(num) => {
                info!("Self-installed UEFI boot entry Boot{num:04X} 'LamBoot'");
                trust_log.record(
                    trust_log::TrustEvent::new("boot_entry_self_installed").with_note(&format!(
                        "num={num:04X} path={LAMBOOT_INSTALL_PATH} status=created"
                    )),
                );
            }
            boot_entry::SelfInstall::AlreadyPresent(num) => {
                trust_log.record(
                    trust_log::TrustEvent::new("boot_entry_self_installed")
                        .with_note(&format!("num={num:04X} status=already_present")),
                );
            }
            boot_entry::SelfInstall::NoDevicePath => {
                trust_log.record(
                    trust_log::TrustEvent::new("boot_entry_self_install_skipped")
                        .with_note("reason=no_loaded_image_device_path"),
                );
            }
            boot_entry::SelfInstall::Failed => {
                trust_log.record(
                    trust_log::TrustEvent::new("boot_entry_self_install_failed")
                        .with_note("reason=nvram_or_devicepath_error"),
                );
            }
        }
    }

    let bootable_count = entries
        .iter()
        .filter(|e| e.icon != boot_types::Icon::Tools)
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
    // Checks file existence across ALL volumes (targeted lookups, not dir scans).
    // sb_state was detected once in Phase 2 and is reused here (and below for
    // the report + boot_entry) — single source of truth.
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
            if check.severity != boot_types::Severity::Ok {
                let sev = match check.severity {
                    boot_types::Severity::Warning => "warning",
                    boot_types::Severity::Error => "error",
                    boot_types::Severity::Ok => "ok", // unreachable
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

    // vmid was resolved earlier in Phase 2.8 — fw_cfg-derived value (if
    // present) already overrode any SMBIOS-derived fallback. The variable
    // is in scope here; no second read needed.

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

        // In a crash loop with no operator-configured fallback, show the menu
        // with a SHORT timeout (not the normal one) so an attended operator can
        // still intervene at the threshold while a headless host proceeds.
        const CRASH_RECOVERY_TIMEOUT_MS: u32 = 8000;
        let menu_result = if in_crash_loop {
            if let Some(entry) = select_fallback(&entries, &policy) {
                Ok(entry)
            } else if let Some(entry) = select_crash_recovery_entry(&entries) {
                // No operator-configured fallback. Earlier this auto-booted the
                // recovery entry with NO menu — which on a deterministically-bad
                // entry re-picks it every boot with no operator escape. Instead
                // show the menu defaulting to the recovery entry on a short
                // timeout: an attended operator can pick a different entry or a
                // tool, and a headless host still boots the recovery pick after
                // the timeout (the crash counter, now reset only on a *successful*
                // boot, keeps accumulating across genuinely-failing recoveries).
                log::warn!(
                    "Crash loop, no fallback_order — short-timeout menu, default = recovery entry: {} ({})",
                    entry.name,
                    entry.id,
                );
                let mut recovery_policy = policy.clone();
                recovery_policy.default_timeout_ms = CRASH_RECOVERY_TIMEOUT_MS;
                recovery_policy.default_entry = Some(entry.id.clone());
                recovery_policy.default_pattern = None;
                run_menu_with_fallback(
                    &entries,
                    &recovery_policy,
                    &gui_sys_info,
                    cohort_notice.as_deref(),
                )
            } else {
                // Nothing bootable at all — fall through to the menu so a human
                // (if present) can act; there is no entry to auto-boot.
                run_menu_with_fallback(&entries, &policy, &gui_sys_info, cohort_notice.as_deref())
            }
        } else {
            run_menu_with_fallback(&entries, &policy, &gui_sys_info, cohort_notice.as_deref())
        };

        let selection = match menu_result {
            Ok(entry) => entry,
            Err(e) => {
                // A menu RUN failure (GUI run error, or text-menu failure)
                // must not strand an unattended/fleet host in firmware setup.
                // Boot a deterministic recovery entry non-interactively first;
                // only fall back to firmware setup when nothing is bootable.
                // This cannot spin: a failed kernel boot returns out of
                // run_bootloader (it is not a tool, so the loop does not
                // re-enter the menu), and the on-disk crash counter makes the
                // NEXT boot's crash-loop detection take over.
                log::error!("Menu failed: {e}.");
                let Some(entry) = select_crash_recovery_entry(&entries) else {
                    log::error!(
                        "No bootable entry after menu failure; rebooting to firmware setup."
                    );
                    health::reboot_to_firmware_setup();
                };
                log::warn!(
                    "Booting recovery entry non-interactively after menu failure: {} ({})",
                    entry.name,
                    entry.id,
                );
                entry
            }
        };

        let is_tool = selection.icon == boot_types::Icon::Tools;

        bootlog.info(
            None,
            &format!("Selected: {} ({})", selection.name, selection.id),
        );
        bootlog.flush(&mut esp);

        let _ = health::record_boot_entry(&selection.id);
        let report_sb = match sb_state {
            secure::SecureBootState::Disabled => trust_log_pure::ReportSbState::Off,
            secure::SecureBootState::ActiveWithShim => trust_log_pure::ReportSbState::Shim,
            secure::SecureBootState::ActiveDirect => trust_log_pure::ReportSbState::Direct,
        };
        let boot_ctx = report::BootContext {
            sys_info: sys_info.clone(),
            fleet_id: fleet_id.clone(),
            os_name: None,
            hypervisor: hypervisor.name.clone(),
            iommu: iommu.clone(),
            vmid: vmid.clone(),
            role: role.clone(),
            host_node: host_node.clone(),
            telemetry_json: telemetry.to_json(),
            report_sb,
            native_pe_never: matches!(policy.loader_native_pe, policy::LoaderNativePeMode::Never),
        };
        report::write_boot_report(&mut esp, &selection, &boot_ctx)?;

        // v0.11.0: BLS file measurement + TPM extend. Diagnostic LAMDIAG
        // audit-log writes removed — they served their purpose narrowing
        // the freeze and were themselves contributing FAT file-handle
        // opens to the firmware threshold. The boot-counter `ro_skipped`
        // event remains in the trust log (recorded but not flushed
        // here; the flush below at line ~854 batches it).
        if let Some(ref bls_file) = selection.bls_filename {
            let entries_dir = "/loader/entries";
            let vol_idx = selection.source_volume_index;
            let bls_path = format!("{entries_dir}/{bls_file}");
            let mut ro_skipped_backend: Option<alloc::string::String> = None;

            let source_volume: &mut fs::Volume = if vol_idx == 0 {
                &mut esp
            } else if vol_idx - 1 < extra_volumes.len() {
                &mut extra_volumes[vol_idx - 1]
            } else {
                &mut esp
            };
            let source_backend = alloc::string::String::from(source_volume.backend_tag());

            if let Ok(content) = source_volume.read_to_string_str(&bls_path) {
                tpm.measure_bls_entry(content.as_bytes(), &selection.id);
                if let Some(bls_entry) = bls::BlsEntry::parse(bls_file, &content) {
                    if bls_entry.tries_left.is_some() {
                        if source_backend == fs_backend_fat::FatBackend::TAG {
                            bls::decrement_boot_count(source_volume, &bls_entry);
                        } else {
                            ro_skipped_backend = Some(source_backend.clone());
                        }
                    }
                }
            }
            if let Some(backend) = ro_skipped_backend {
                trust_log.record(
                    trust_log::TrustEvent::new("boot_counter_skipped_ro")
                        .with_path(bls_file)
                        .with_note(&format!("vol_idx={vol_idx} backend={backend}")),
                );
            }
        }

        // Bracketing diagnostic: append-before-tpm. If this line appears
        // in audit.log but the next one doesn't, TPM is the freeze.
        crate::diag::append(&mut esp, "LAMDIAG2 before_cmdline_tpm\n");

        if let boot_types::EntryKind::LinuxLegacy { ref options, .. }
        | boot_types::EntryKind::Uki { ref options, .. } = selection.kind
        {
            let _ = tpm.measure_cmdline(options);
        }

        // Bracketing diagnostic: append-after-tpm.
        crate::diag::append(&mut esp, "LAMDIAG2 after_cmdline_tpm_before_flush\n");

        // v0.11.0 EXACT MATCH to bf790ae7 known-good build: 3 tight back-
        // to-back audit-log writes immediately after LAMDIAG2 are required
        // for the post-selection FAT cadence on ASUS G10AJ to remain
        // operational. Their absence in the night-3 cleanup attempt
        // caused the firmware to freeze at the next FAT op.
        crate::diag::append(&mut esp, "LAMDIAG3a tight_post_diag2\n");
        crate::diag::append(&mut esp, "LAMDIAG3b tight_5\n");
        crate::diag::append(&mut esp, "LAMDIAG3c tight_6\n");

        // info!("Booting: ...") hangs the next FAT op on quirky firmware
        // (ASUS G10AJ class). On modern firmware the info! is restored
        // so operators see what's being booted on the console.
        if firmware_quirks.conout_fat_coupling {
            crate::diag::append_fmt(&mut esp, format_args!("Booting: {}\n", selection.name));
        } else {
            info!("Booting: {}", selection.name);
        }
        crate::diag::append(&mut esp, "LAMDIAG3d after_audit_replacement\n");

        let selection_name = selection.name.clone();
        let selection_id = selection.id.clone();
        crate::diag::append(&mut esp, "LAMDIAG3e after_clones\n");

        // SDS-4 §6.1 Step 8: entry_selected + boot_attempt.
        // record() is push-only (its log:: macro was removed in v0.11.0
        // — kills the firmware FAT driver on ASUS G10AJ).
        trust_log.record(
            trust_log::TrustEvent::new("entry_selected")
                .with_path(&selection_name)
                .with_note(&alloc::format!("id={selection_id}")),
        );
        crate::diag::append(&mut esp, "LAMDIAG3f after_first_record\n");

        trust_log.record(
            trust_log::TrustEvent::new("boot_attempt")
                .with_path(&selection_name)
                .with_note(&alloc::format!(
                    "id={} kind={:?}",
                    selection_id,
                    selection.kind
                )),
        );
        crate::diag::append(&mut esp, "LAMDIAG4 after_intervening_code\n");

        let mut all_volumes = alloc::vec![esp];
        all_volumes.append(&mut extra_volumes);

        // Pre-handoff trust_log.flush: gated by conout_fat_coupling
        // quirk. On quirky firmware the FAT op after this flush hangs
        // (ASUS G10AJ). On modern firmware the flush runs so
        // entry_selected + boot_attempt events land in trust.log
        // before kernel handoff.
        crate::diag::append(&mut all_volumes[0], "PREFLUSH before_trustlog_flush\n");
        if !firmware_quirks.conout_fat_coupling {
            trust_log.flush(&mut all_volumes[0]);
        }
        crate::diag::append(&mut all_volumes[0], "POSTFLUSH after_handoff_flush\n");

        let result = boot::boot_entry(
            image,
            &mut all_volumes,
            selection,
            &tpm,
            &policy,
            &mut trust_log,
            firmware_quirks,
            sb_state,
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

        // Kernel/UKI boot — if we get here, the image RETURNED (unusual: a
        // successful handoff via start_image does not return). Mark success
        // ONLY on an Ok return. On Err the boot failed, so we must NOT reset
        // the crash counter — leaving it intact lets the next boot's
        // assess_boot_health() accumulate it so crash-loop detection engages
        // after the threshold. (Resetting it here is what would turn a
        // repeatedly-failing boot — including the menu-failure recovery path
        // above — into an unbreakable cross-boot loop.)
        if result.is_ok() {
            let _ = health::mark_boot_success();
        }
        return result;
    }
}

/// Try to find a configured fallback entry.
fn select_fallback(
    entries: &[boot_types::BootEntry],
    policy: &policy::Policy,
) -> Option<boot_types::BootEntry> {
    for fallback_id in &policy.fallback_order {
        if let Some(entry) = entries.iter().find(|e| &e.id == fallback_id) {
            log::info!("Using fallback entry: {} ({})", entry.name, entry.id);
            return Some(entry.clone());
        }
    }
    log::warn!("No fallback entry configured");
    None
}

/// Deterministic last-resort recovery pick for a crash loop with no configured
/// `fallback_order`: the first bootable (non-tool) entry. Used to keep a
/// headless host from hanging in the interactive menu at the crash threshold.
fn select_crash_recovery_entry(entries: &[boot_types::BootEntry]) -> Option<boot_types::BootEntry> {
    // Prefer a non-tool entry that PASSED preflight (no Error), so a
    // non-interactive recovery boot does not deterministically pick a
    // known-broken entry (e.g. kernel/initrd missing → boot_entry returns
    // NOT_FOUND). Fall back to any non-tool entry when none are preflight-clean
    // (preflight is advisory and may be absent / all-Error).
    entries
        .iter()
        .find(|e| {
            e.icon != boot_types::Icon::Tools
                && !matches!(
                    e.preflight.as_ref().map(|pf| pf.status),
                    Some(boot_types::PreflightStatus::Error)
                )
        })
        .or_else(|| entries.iter().find(|e| e.icon != boot_types::Icon::Tools))
        .cloned()
}

/// Present the boot menu, preferring the GOP GUI but falling back to the text
/// console menu when GUI init fails. Previously a `GuiContext::new()?` on a
/// graphics-init error escaped `run_bootloader` entirely and dropped to
/// firmware with no boot attempt; on firmware where the GUI reliably fails that
/// is a reboot loop. The text menu the code already supports is the correct
/// degraded path.
fn run_menu_with_fallback(
    entries: &[boot_types::BootEntry],
    policy: &policy::Policy,
    gui_sys_info: &gui::SystemInfoDisplay,
    cohort_notice: Option<&str>,
) -> uefi::Result<boot_types::BootEntry> {
    if console::has_graphics() {
        match gui::GuiContext::new() {
            Ok(mut gui_ctx) => {
                gui_ctx.set_system_info(gui_sys_info.clone());
                gui_ctx.set_cohort_notice(cohort_notice);
                return gui_ctx.run_menu(entries, policy);
            }
            Err(e) => {
                log::warn!("GUI init failed ({e:?}) — falling back to text menu");
            }
        }
    }
    let mut text_menu = console::TextMenu::new();
    text_menu.set_cohort_notice(cohort_notice);
    text_menu.run_menu(entries, policy)
}
