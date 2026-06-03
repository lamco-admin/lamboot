//! Layer: 1 — Firmware Boundary.
//!
//! Per-firmware quirk detection.
//!
//! Some firmware implementations have bugs that LamBoot needs to work
//! around. Rather than degrading behavior for every system, we identify
//! the specific firmware at boot via SMBIOS, match against a data-driven
//! list of known quirks, and enable only the workarounds needed for the
//! current hardware.
//!
//! Discovered during pve2 / ASUS G10AJ debugging session (2026-05-26).
//! Three firmware-coupling bugs surfaced on that one piece of 2014
//! workstation hardware that do not appear on QEMU/OVMF, modern
//! Supermicro/Dell, or any other system tested.
//!
//! Step 1 of the gating rollout (this commit): detection + trust event
//! emission only. The boolean flags are not yet consumed by code paths.
//! Subsequent commits will gate the hardcoded workarounds in
//! `verify_kernel_bytes` (shim skip) and boot.rs (log::info! removal)
//! behind these flags.

use alloc::string::String;

use crate::smbios::SystemInfo;

/// Set of workarounds the boot path can consult. All default to false —
/// conservative posture is "this firmware works normally."
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FirmwareQuirks {
    /// Post-selection log:: emissions and trust_log.flush hang the
    /// firmware FAT driver on this hardware. Observed on ASUS G10AJ
    /// (night-2) and reproducibly confirmed via force-off override
    /// (2026-05-27): with the quirk forcibly disabled, pre-selection
    /// `trust_log.record` log::info! emissions succeed (10+ events,
    /// dual-emitted to ConOut + flushed to spec path), but the FIRST
    /// post-selection `trust_log.record(entry_selected)` hangs the
    /// next FAT op. Reducing cumulative FAT writes (verbose=false,
    /// 3 BLS entries instead of 14) did NOT prevent the hang —
    /// cumulative count is not the cause.
    ///
    /// Exact mechanism unknown. Behavior is reproducibly real. The
    /// hang is somewhere in the post-selection sequence
    ///   record(entry_selected) → record(boot_attempt) → flush
    /// Narrowing further would require 4 per-behavior override flags
    /// and 4 boot iterations; deferred as marginal value.
    ///
    /// Workaround applied when this flag is true:
    ///   - TrustLog::record is push-only (no log:: macro emission)
    ///   - trust.log path is `/EFI/LamBoot/reports/trust.log`
    ///     instead of BLS-spec `/loader/boot-trust.log`
    ///   - info!("Booting: ...") replaced with audit-log write
    ///   - 5 post-selection trust_log.flush calls are skipped
    ///     (main pre-handoff, boot_linux x3, native_load_and_start
    ///     pre-handoff)
    pub conout_fat_coupling: bool,
}

/// Static match list of known quirky firmware. Match on SMBIOS Mfr +
/// Product is conservative — variants of the same model (different BIOS
/// revisions) all carry the same quirks observed to date.
struct KnownQuirky {
    manufacturer_eq_case_insensitive: &'static str,
    product_starts_with: &'static str,
    quirks: FirmwareQuirks,
}

const KNOWN_QUIRKY_SYSTEMS: &[KnownQuirky] = &[KnownQuirky {
    // pve2 — 2014 workstation, original OEM firmware.
    //
    // shim_verify_unusable was REMOVED on 2026-05-27 after the
    // diagnostics_force_shim_verify probe proved shim::Verify completes
    // successfully on the 16MB Linux kernel with verified_via=shim_mok.
    //
    // no_partition_info_protocol was REMOVED on 2026-05-27 — the flag
    // was never consumed by any code branch. The BlockIO fallback at
    // main.rs:494 runs based on `discovered_partitions.is_empty()`,
    // which is independent of the flag and self-healing across firmware
    // variants. The `lvm_probe_pass_start source=blockio_fallback`
    // trust event already carries the same auditable signal.
    manufacturer_eq_case_insensitive: "ASUSTeK COMPUTER INC.",
    product_starts_with: "G10AJ",
    quirks: FirmwareQuirks {
        conout_fat_coupling: true,
    },
}];

impl FirmwareQuirks {
    /// Detect quirks for the running firmware via SMBIOS. Returns
    /// `FirmwareQuirks::default()` (all false) when no match — modern
    /// firmware default is "behave normally."
    pub(crate) fn detect(sys_info: &SystemInfo) -> Self {
        let (Some(mfr), Some(product)) = (
            sys_info.manufacturer.as_deref(),
            sys_info.product_name.as_deref(),
        ) else {
            return Self::default();
        };

        for q in KNOWN_QUIRKY_SYSTEMS {
            if mfr.eq_ignore_ascii_case(q.manufacturer_eq_case_insensitive)
                && product.starts_with(q.product_starts_with)
            {
                return q.quirks;
            }
        }

        Self::default()
    }

    /// Compact human-readable summary for trust-log emission.
    pub(crate) fn summary(self) -> String {
        let mut parts: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();
        if self.conout_fat_coupling {
            parts.push("conout_fat_coupling=true");
        }
        if parts.is_empty() {
            String::from("none")
        } else {
            parts.join(" ")
        }
    }
}
