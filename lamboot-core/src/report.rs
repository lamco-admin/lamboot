//! Layer: 5 — Trust & Audit.

use alloc::{format, string::String};

use uefi::Result;

use crate::{
    acpi::IommuInfo, boot_types::BootEntry, fs::Volume, fs_backend::PathBuf, fs_writer::EspWriter,
    smbios::SystemInfo,
};

const REPORTS_DIR: &str = "/EFI/LamBoot/reports";
const BOOT_REPORT: &str = "/EFI/LamBoot/reports/boot.json";
const AUDIT_LOG: &str = "/EFI/LamBoot/reports/audit.log";
const ERROR_REPORT: &str = "/EFI/LamBoot/reports/error.json";

/// Boot context carried from early phases into the report
pub(crate) struct BootContext {
    pub sys_info: SystemInfo,
    pub fleet_id: Option<String>,
    pub os_name: Option<String>,
    pub hypervisor: Option<String>,
    pub iommu: IommuInfo,
    pub vmid: Option<String>,
    /// Operator-assigned role for this VM, populated from the
    /// `[roles]` table in /etc/lamboot/fleet.toml via the
    /// lamboot-hookscript fw_cfg blob. Empty for hosts not running
    /// the v0.8.4+ hookscript.
    pub role: Option<String>,
    /// Proxmox node name that started this VM, populated from
    /// `hostname` in the lamboot-hookscript fw_cfg JSON. Helps an
    /// operator correlate a guest's boot.json with a specific PVE
    /// node in clusters. Empty on non-Proxmox / legacy hosts.
    pub host_node: Option<String>,
    pub telemetry_json: String,
    /// Secure Boot posture, for the boot.json `verified_via` field (#5).
    pub report_sb: crate::trust_log_pure::ReportSbState,
    /// Whether `[loader].native_pe = "never"` is in effect, for the
    /// boot.json `loader` field (#5).
    pub native_pe_never: bool,
}

/// Get current timestamp as ISO 8601 string from UEFI RTC
fn get_timestamp() -> String {
    match uefi::runtime::get_time() {
        Ok(time) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            time.year(),
            time.month(),
            time.day(),
            time.hour(),
            time.minute(),
            time.second(),
        ),
        Err(_) => String::from("unknown"),
    }
}

fn writer_for(esp: &mut Volume) -> Result<EspWriter<'_>> {
    EspWriter::new(esp).ok_or_else(|| uefi::Error::from(uefi::Status::UNSUPPORTED))
}

fn path(s: &str) -> Result<PathBuf> {
    PathBuf::from_str(s).map_err(|_| uefi::Error::from(uefi::Status::INVALID_PARAMETER))
}

/// Escape a string for safe inclusion inside a JSON `"..."` string literal.
///
/// Required because UEFI-style paths (`\EFI\LamBoot\lambootx64.efi`,
/// `\boot\vmlinuz-linux`) contain bare backslashes which JSON parsers
/// interpret as escape introducers — `\b` becomes a backspace, `\v` is
/// invalid, etc. Without this, `boot.json`'s `"path"` field is invalid
/// JSON and downstream consumers (lamboot-inspect, lamboot-pve-monitor)
/// either choke or silently corrupt the value.
///
/// Covers the JSON spec's required escapes: `\\`, `\"`, `\b`, `\f`,
/// `\n`, `\r`, `\t`, and `\uXXXX` for the remaining C0 controls.
fn json_escape(s: &str) -> String {
    use core::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                // String impls core::fmt::Write so this never errors
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Write a boot report in JSON format
pub(crate) fn write_boot_report(esp: &mut Volume, entry: &BootEntry, ctx: &BootContext) -> Result {
    let reports_dir = path(REPORTS_DIR)?;
    let boot_report = path(BOOT_REPORT)?;
    {
        let mut w = writer_for(esp)?;
        let _ = w.ensure_dir(reports_dir.as_path());
    }

    let sys_manufacturer = ctx.sys_info.manufacturer.as_deref().unwrap_or("");
    let sys_product = ctx.sys_info.product_name.as_deref().unwrap_or("");
    let fleet = ctx.fleet_id.as_deref().unwrap_or("");
    let os = ctx.os_name.as_deref().unwrap_or("");
    let hv = ctx.hypervisor.as_deref().unwrap_or("");
    let vmid = ctx.vmid.as_deref().unwrap_or("");
    let role = ctx.role.as_deref().unwrap_or("");
    let host_node = ctx.host_node.as_deref().unwrap_or("");
    let iommu_type = if ctx.iommu.intel_vt_d {
        "Intel VT-d"
    } else if ctx.iommu.amd_vi {
        "AMD-Vi"
    } else {
        ""
    };

    // #5: surface the trust posture in the summary report so a consumer can
    // tell native-PE from firmware-LoadImage without parsing the trust log.
    // Chainload targets are firmware-loaded and not verified by LamBoot, so
    // their verified_via is empty (not applicable) rather than implying a
    // kernel-trust decision that never happened.
    //
    // IMPORTANT: this boot.json `verified_via` is a *pre-boot posture* field.
    // The report is written at selection time, BEFORE the kernel is read or
    // verified (verification happens later in boot.rs, after which LamBoot
    // hands off and never returns to rewrite this file). It records HOW the
    // image will be verified under the detected Secure Boot posture, not a
    // post-hoc cryptographic result. The authoritative per-image outcome —
    // SUCCESS / SKIPPED / DEFERRED / REJECTED — lives in the trust log's
    // `image_verified` event (\loader\boot-trust.log). `ctx.report_sb` is
    // derived from the same single `sb_state` the verify path uses (threaded
    // from run_bootloader), so the posture here cannot disagree with it.
    let is_chainload = matches!(entry.kind, crate::boot_types::EntryKind::Chainload { .. });
    let verified_via = if is_chainload {
        ""
    } else {
        crate::trust_log_pure::report_verified_via(ctx.report_sb)
    };
    let loader = crate::trust_log_pure::report_loader(ctx.native_pe_never, is_chainload);

    let json = format!(
        r#"{{
  "lamboot_version": "{}",
  "lamboot_arch": "{}",
  "timestamp": "{}",
  "entry_id": "{}",
  "entry_name": "{}",
  "entry_type": "{}",
  "path": "{}",
  "verified_via": "{}",
  "loader": "{}",
  "system_manufacturer": "{}",
  "system_product": "{}",
  "fleet_id": "{}",
  "vmid": "{}",
  "role": "{}",
  "host_node": "{}",
  "os_name": "{}",
  "hypervisor": "{}",
  "iommu": "{}",
  "iommu_units": {},
  "boot_timing_ms": {}
}}
"#,
        json_escape(crate::VERSION),
        json_escape(crate::BUILD_TARGET),
        json_escape(&get_timestamp()),
        json_escape(&entry.id),
        json_escape(&entry.name),
        match entry.kind {
            crate::boot_types::EntryKind::Chainload { .. } => "chainload",
            crate::boot_types::EntryKind::Uki { .. } => "uki",
            crate::boot_types::EntryKind::LinuxLegacy { .. } => "linux_legacy",
            crate::boot_types::EntryKind::Iso { .. } => "iso",
        },
        json_escape(match &entry.kind {
            crate::boot_types::EntryKind::Chainload { path }
            | crate::boot_types::EntryKind::Uki { path, .. } => path,
            crate::boot_types::EntryKind::LinuxLegacy { kernel_path, .. } => kernel_path,
            crate::boot_types::EntryKind::Iso {
                source: crate::boot_types::IsoSource::File { iso_path },
            } => iso_path,
            crate::boot_types::EntryKind::Iso {
                source: crate::boot_types::IsoSource::Optical { label, .. },
            } => label,
        }),
        json_escape(verified_via),
        json_escape(loader),
        json_escape(sys_manufacturer),
        json_escape(sys_product),
        json_escape(fleet),
        json_escape(vmid),
        json_escape(role),
        json_escape(host_node),
        json_escape(os),
        json_escape(hv),
        json_escape(iommu_type),
        ctx.iommu.drhd_units.len(),
        ctx.telemetry_json,
    );

    {
        let mut w = writer_for(esp)?;
        w.write(boot_report.as_path(), json.as_bytes())?;
    }

    append_audit_log(
        esp,
        &format!(
            "[{}] Boot: {} ({})\n",
            get_timestamp(),
            entry.name,
            entry.id
        ),
    )?;

    Ok(())
}

/// Append a line to the audit log via true positional append.
///
/// v0.11.0: switched from read-modify-write (which re-wrote the full
/// audit.log file on every line) to `EspWriter::true_append` (writes
/// only `message` bytes at EOF). Same root-cause fix as
/// `TrustLog::flush` — the read-modify-write pattern hit the firmware
/// FAT-driver overwrite-count hang on real Proxmox hosts.
///
/// The previous 10240/8192 rolling-truncate behavior is dropped here:
/// (a) it required reading the whole file every call, defeating the
/// append; (b) operator-side rotation (logrotate from Linux, or a
/// once-per-boot truncate from trust_log init) is the right layer.
/// audit.log will now grow unbounded until rotated externally.
pub(crate) fn append_audit_log(esp: &mut Volume, message: &str) -> Result {
    let audit = path(AUDIT_LOG)?;
    let mut w = writer_for(esp)?;
    w.true_append(audit.as_path(), message.as_bytes())?;
    Ok(())
}

/// Write an error report
#[expect(
    dead_code,
    reason = "will be called from error paths once error reporting is integrated"
)]
pub(crate) fn write_error_report(esp: &mut Volume, error: &str) -> Result {
    let json = format!(
        r#"{{
  "timestamp": "{}",
  "error": "{}"
}}
"#,
        get_timestamp(),
        error.replace('"', "'")
    );

    let err_path = path(ERROR_REPORT)?;
    {
        let mut w = writer_for(esp)?;
        w.write(err_path.as_path(), json.as_bytes())?;
    }
    append_audit_log(esp, &format!("[{}] ERROR: {}\n", get_timestamp(), error))?;

    Ok(())
}
