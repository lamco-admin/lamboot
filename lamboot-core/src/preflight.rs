//! Preflight validation engine.
//!
//! Validates boot entries before presenting them in the menu.
//! Checks file existence, PE headers, initrd sanity, Secure Boot
//! compatibility, and filesystem driver availability.
//!
//! Core principle: NEVER block boot. All results are advisory.

use alloc::{format, string::String, vec::Vec};

use crate::{
    discovery::{EntryKind, Icon},
    fs::EspVolume,
    secure::SecureBootState,
};

/// Aggregate preflight result for a boot entry
#[derive(Debug, Clone)]
pub(crate) struct PreflightResult {
    pub status: PreflightStatus,
    pub checks: Vec<CheckResult>,
}

impl PreflightResult {
    pub(crate) fn from_checks(checks: Vec<CheckResult>) -> Self {
        let status = if checks.iter().any(|c| c.severity == Severity::Error) {
            PreflightStatus::Error
        } else if checks.iter().any(|c| c.severity == Severity::Warning) {
            PreflightStatus::Warning
        } else {
            PreflightStatus::Ok
        };
        Self { status, checks }
    }

    pub(crate) fn first_issue(&self) -> Option<&CheckResult> {
        self.checks.iter().find(|c| c.severity != Severity::Ok)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PreflightStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct CheckResult {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Ok,
    Warning,
    Error,
}

fn ok() -> CheckResult {
    CheckResult {
        severity: Severity::Ok,
        message: String::new(),
    }
}

fn warning(msg: &str) -> CheckResult {
    CheckResult {
        severity: Severity::Warning,
        message: String::from(msg),
    }
}

fn error(msg: &str) -> CheckResult {
    CheckResult {
        severity: Severity::Error,
        message: String::from(msg),
    }
}

/// Run all applicable preflight checks for a boot entry.
/// Searches ESP first, then extra volumes for file existence (targeted
/// lookups only — no directory enumeration on large filesystems).
pub(crate) fn run_preflight(
    kind: &EntryKind,
    icon: Icon,
    esp: &mut EspVolume,
    extra_volumes: &mut [EspVolume],
    sb_state: SecureBootState,
    driver_count: usize,
) -> PreflightResult {
    let mut checks = Vec::new();

    match kind {
        EntryKind::Chainload { path } | EntryKind::Uki { path, .. } => {
            let file_ok = check_file_exists_any(esp, extra_volumes, path);
            let passed = file_ok.severity == Severity::Ok;
            checks.push(file_ok);

            if passed {
                // PE check needs to read from whichever volume has the file
                let vol = find_volume_with_file(esp, extra_volumes, path);
                checks.push(check_pe_header(vol, path));
                if sb_state != SecureBootState::Disabled {
                    checks.push(check_secure_boot(vol, path, sb_state));
                }
            }
        }

        EntryKind::LinuxLegacy {
            kernel_path,
            initrd_paths,
            options,
        } => {
            let file_ok = check_file_exists_any(esp, extra_volumes, kernel_path);
            let passed = file_ok.severity == Severity::Ok;
            checks.push(file_ok);

            if passed {
                let vol = find_volume_with_file(esp, extra_volumes, kernel_path);
                checks.push(check_pe_header(vol, kernel_path));
                if sb_state != SecureBootState::Disabled {
                    checks.push(check_secure_boot(vol, kernel_path, sb_state));
                }
            }

            // Initrd checks — search all volumes
            for ipath in initrd_paths {
                if !file_exists_any(esp, extra_volumes, ipath) {
                    checks.push(warning(&format!("Initrd not found: {ipath}")));
                }
            }

            // root= check
            checks.push(check_root_param(options));

            // Filesystem driver check
            if icon != Icon::Tools {
                checks.push(check_fs_driver(kernel_path, driver_count));
            }
        }
    }

    PreflightResult::from_checks(checks)
}

/// Check if a file exists on any volume (ESP first, then extras).
/// Targeted lookup only — no directory enumeration.
fn check_file_exists_any(
    esp: &mut EspVolume,
    extra_volumes: &mut [EspVolume],
    path: &str,
) -> CheckResult {
    if file_exists_any(esp, extra_volumes, path) {
        ok()
    } else {
        error(&format!("File not found: {path}"))
    }
}

fn file_exists_any(esp: &mut EspVolume, extra_volumes: &mut [EspVolume], path: &str) -> bool {
    if esp.exists(path) {
        return true;
    }
    extra_volumes.iter_mut().any(|vol| vol.exists(path))
}

/// Find the volume that has a specific file. Returns ESP if no volume has it.
fn find_volume_with_file<'a>(
    esp: &'a mut EspVolume,
    extra_volumes: &'a mut [EspVolume],
    path: &str,
) -> &'a mut EspVolume {
    if esp.exists(path) {
        return esp;
    }
    for vol in extra_volumes.iter_mut() {
        if vol.exists(path) {
            return vol;
        }
    }
    esp // fallback to ESP
}

fn check_pe_header(esp: &mut EspVolume, path: &str) -> CheckResult {
    // Only read the first 256 bytes — enough for MZ magic, PE offset, and signature
    let Ok(data) = esp.read_file_at(path, 0, 256) else {
        return warning("Cannot read file for PE validation");
    };

    if data.len() < 64 {
        return warning("File too small for PE binary");
    }

    if data[0..2] != [0x4D, 0x5A] {
        return warning("Invalid PE header (no MZ signature)");
    }

    let pe_offset = u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
    if pe_offset + 4 > data.len() {
        return warning("PE header offset out of bounds");
    }

    if data[pe_offset..pe_offset + 4] != [0x50, 0x45, 0x00, 0x00] {
        return warning("Invalid PE signature");
    }

    // Architecture check
    let machine = u16::from_le_bytes([data[pe_offset + 4], data[pe_offset + 5]]);

    #[cfg(target_arch = "x86_64")]
    const NATIVE: u16 = 0x8664;
    #[cfg(target_arch = "aarch64")]
    const NATIVE: u16 = 0xAA64;

    if machine != NATIVE {
        let found = match machine {
            0x8664 => "x86_64",
            0xAA64 => "aarch64",
            _ => "unknown",
        };
        return warning(&format!("Architecture mismatch: found {found}"));
    }

    ok()
}

fn check_secure_boot(esp: &mut EspVolume, path: &str, sb_state: SecureBootState) -> CheckResult {
    if sb_state == SecureBootState::Disabled {
        return ok();
    }

    let Ok(data) = esp.read_to_vec(path) else {
        return warning("Cannot read file for Secure Boot check");
    };

    match crate::secure::verify_image(&data) {
        Ok(()) => ok(),
        Err(_) => warning("Not signed for Secure Boot"),
    }
}

fn check_root_param(options: &str) -> CheckResult {
    // Extract root= value
    let root = options
        .split_whitespace()
        .find(|t| t.starts_with("root="))
        .map(|t| &t[5..]);

    let Some(root_spec) = root else {
        // No root= is acceptable (discoverable partitions, or embedded in initrd)
        return ok();
    };

    // Basic format validation for UUID
    if let Some(uuid) = root_spec.strip_prefix("UUID=") {
        let parts: Vec<&str> = uuid.split('-').collect();
        if parts.len() != 5 {
            return warning(&format!("Malformed UUID in root=: {uuid}"));
        }
    }

    ok()
}

fn check_fs_driver(kernel_path: &str, driver_count: usize) -> CheckResult {
    // If kernel path suggests it's NOT on the ESP (no \EFI\ prefix)
    // and no filesystem drivers were loaded, warn
    let p = kernel_path.to_uppercase();
    if driver_count == 0 && !p.starts_with("\\EFI\\") && !p.starts_with("\\LOADER\\") {
        return warning("Path may require a filesystem driver (none loaded)");
    }
    ok()
}
