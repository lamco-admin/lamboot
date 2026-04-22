use alloc::{format, string::String};

use uefi::Result;

use crate::{acpi::IommuInfo, discovery::BootEntry, fs::EspVolume, smbios::SystemInfo};

/// Boot context carried from early phases into the report
pub(crate) struct BootContext {
    pub sys_info: SystemInfo,
    pub fleet_id: Option<String>,
    pub os_name: Option<String>,
    pub hypervisor: Option<String>,
    pub iommu: IommuInfo,
    pub vmid: Option<String>,
    pub telemetry_json: String,
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

/// Write a boot report in JSON format
pub(crate) fn write_boot_report(
    esp: &mut EspVolume,
    entry: &BootEntry,
    ctx: &BootContext,
) -> Result {
    let _ = esp.ensure_dir("\\EFI\\LamBoot\\reports");

    let sys_manufacturer = ctx.sys_info.manufacturer.as_deref().unwrap_or("");
    let sys_product = ctx.sys_info.product_name.as_deref().unwrap_or("");
    let fleet = ctx.fleet_id.as_deref().unwrap_or("");
    let os = ctx.os_name.as_deref().unwrap_or("");
    let hv = ctx.hypervisor.as_deref().unwrap_or("");
    let vmid = ctx.vmid.as_deref().unwrap_or("");
    let iommu_type = if ctx.iommu.intel_vt_d {
        "Intel VT-d"
    } else if ctx.iommu.amd_vi {
        "AMD-Vi"
    } else {
        ""
    };

    // Create simple JSON manually (no_std compatible)
    let json = format!(
        r#"{{
  "lamboot_version": "{}",
  "lamboot_arch": "{}",
  "timestamp": "{}",
  "entry_id": "{}",
  "entry_name": "{}",
  "entry_type": "{}",
  "path": "{}",
  "system_manufacturer": "{}",
  "system_product": "{}",
  "fleet_id": "{}",
  "vmid": "{}",
  "os_name": "{}",
  "hypervisor": "{}",
  "iommu": "{}",
  "iommu_units": {},
  "boot_timing_ms": {}
}}
"#,
        crate::VERSION,
        crate::BUILD_TARGET,
        get_timestamp(),
        entry.id,
        entry.name,
        match entry.kind {
            crate::discovery::EntryKind::Chainload { .. } => "chainload",
            crate::discovery::EntryKind::Uki { .. } => "uki",
            crate::discovery::EntryKind::LinuxLegacy { .. } => "linux_legacy",
        },
        match &entry.kind {
            crate::discovery::EntryKind::Chainload { path }
            | crate::discovery::EntryKind::Uki { path, .. } => path,
            crate::discovery::EntryKind::LinuxLegacy { kernel_path, .. } => kernel_path,
        },
        sys_manufacturer,
        sys_product,
        fleet,
        vmid,
        os,
        hv,
        iommu_type,
        ctx.iommu.drhd_units.len(),
        ctx.telemetry_json,
    );

    esp.write_file("\\EFI\\LamBoot\\reports\\boot.json", json.as_bytes())?;

    // Also append to audit log
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

/// Append a line to the audit log
pub(crate) fn append_audit_log(esp: &mut EspVolume, message: &str) -> Result {
    let log_path = "\\EFI\\LamBoot\\reports\\audit.log";

    // Read existing log
    let mut log = esp.read_to_string(log_path).unwrap_or_default();

    // Limit log size (keep last 10KB)
    if log.len() > 10240 {
        let keep_from = log.len() - 8192;
        log = String::from(&log[keep_from..]);
    }

    // Append new message
    log.push_str(message);

    // Write back
    esp.write_file(log_path, log.as_bytes())?;

    Ok(())
}

/// Write an error report
#[expect(
    dead_code,
    reason = "will be called from error paths once error reporting is integrated"
)]
pub(crate) fn write_error_report(esp: &mut EspVolume, error: &str) -> Result {
    let json = format!(
        r#"{{
  "timestamp": "{}",
  "error": "{}"
}}
"#,
        get_timestamp(),
        error.replace('"', "'")
    );

    esp.write_file("\\EFI\\LamBoot\\reports\\error.json", json.as_bytes())?;
    append_audit_log(esp, &format!("[{}] ERROR: {}\n", get_timestamp(), error))?;

    Ok(())
}
