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
    fs::Volume,
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
    _icon: Icon,
    esp: &mut Volume,
    extra_volumes: &mut [Volume],
    sb_state: SecureBootState,
    _driver_count: usize,
) -> PreflightResult {
    // `_icon` and `_driver_count` are retained in the signature to keep
    // call sites unchanged but are currently unused — former consumer
    // `check_fs_driver` was removed (see note below).
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

            // check_fs_driver was removed in SDS-2+SDS-6 era — under native
            // ext4-view backend the `driver_count == 0` heuristic is a
            // false positive, and `check_file_exists_any` above already
            // answers the reachability question definitively.
        }
    }

    PreflightResult::from_checks(checks)
}

/// Check if a file exists on any volume (ESP first, then extras).
/// Targeted lookup only — no directory enumeration.
fn check_file_exists_any(
    esp: &mut Volume,
    extra_volumes: &mut [Volume],
    path: &str,
) -> CheckResult {
    if file_exists_any(esp, extra_volumes, path) {
        ok()
    } else {
        error(&format!("File not found: {path}"))
    }
}

fn file_exists_any(esp: &mut Volume, extra_volumes: &mut [Volume], path: &str) -> bool {
    if exists_with_boot_fallback(esp, path).is_some() {
        return true;
    }
    extra_volumes
        .iter_mut()
        .any(|vol| exists_with_boot_fallback(vol, path).is_some())
}

/// Find the volume that has a specific file. Returns ESP if no volume has it.
fn find_volume_with_file<'a>(
    esp: &'a mut Volume,
    extra_volumes: &'a mut [Volume],
    path: &str,
) -> &'a mut Volume {
    if exists_with_boot_fallback(esp, path).is_some() {
        return esp;
    }
    for vol in extra_volumes.iter_mut() {
        if exists_with_boot_fallback(vol, path).is_some() {
            return vol;
        }
    }
    esp // fallback to ESP
}

/// Check whether a volume has `path` either as-written OR under an implicit
/// `\boot\` prefix. Needed because Debian's kernel-install occasionally
/// generates BLS entries like `linux /vmlinuz-X` (no `/boot/` prefix) even
/// when the actual kernel file is at `/boot/vmlinuz-X` on the root
/// filesystem. The cross-distro layouts we target:
///
///   Debian / Ubuntu single-root:  kernel at `/boot/vmlinuz-X`
///   Fedora separate /boot (XBOOTLDR): kernel at `/vmlinuz-X` on /boot volume
///
/// Trying the path as-written covers the XBOOTLDR case; the `\boot\`
/// fallback covers the Debian-single-root misgeneration case without
/// affecting correctly-prefixed entries. First match wins.
pub(crate) fn exists_with_boot_fallback(
    vol: &mut Volume,
    path: &str,
) -> Option<alloc::string::String> {
    use alloc::string::ToString as _;
    if vol.exists_str(path) {
        return Some(path.to_string());
    }
    // Only try the boot-prefixed fallback if the path doesn't already
    // have one — avoid turning `\boot\vmlinuz` into `\boot\boot\vmlinuz`.
    let lower = path.to_lowercase();
    if lower.starts_with("\\boot\\") || lower.starts_with("/boot/") {
        return None;
    }
    // Build the \boot\-prefixed form. Path always starts with \ after
    // bls_parse::normalize_path, but be defensive.
    let prefixed = if path.starts_with('\\') || path.starts_with('/') {
        format!("\\boot{path}")
    } else {
        format!("\\boot\\{path}")
    };
    if vol.exists_str(&prefixed) {
        return Some(prefixed);
    }
    None
}

fn check_pe_header(esp: &mut Volume, path: &str) -> CheckResult {
    // Only read the first 256 bytes — enough for MZ magic, PE offset, and signature
    let Ok(data) = esp.read_at_str(path, 0, 256) else {
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

fn check_secure_boot(vol: &mut Volume, path: &str, sb_state: SecureBootState) -> CheckResult {
    if sb_state == SecureBootState::Disabled {
        return ok();
    }

    let Ok(data) = vol.read_str(path) else {
        return warning("Cannot read file for Secure Boot check");
    };

    // Under shim+MOK, ShimLock::Verify is the authoritative runtime check
    // (it covers both MOK-enrolled certs AND firmware `db`). Calling only
    // `verify_image` here — which walks firmware `db` only — false-warns
    // on every stock distro kernel (Canonical, Red Hat, Debian signing
    // certs are in MOK, not in `db`). Mirror the runtime trust decision
    // so the menu matches what boot.rs will actually do.
    if sb_state == SecureBootState::ActiveWithShim && crate::security_override::shim_validate(&data)
    {
        return ok();
    }

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

// check_fs_driver was removed. Historical rationale: under v0.8.3 this
// warned "you probably forgot --with-drivers" when `driver_count == 0`
// and the kernel path wasn't on the ESP. Under v0.9.x SDS-2 compiles the
// native ext4-view backend in unconditionally, and SDS-6 Auto mode
// deliberately skips loading the legacy `ext4_x64.efi` driver — so
// `driver_count == 0` is the *expected* state on any ext4-rooted distro,
// not a problem. The reachability question is already answered correctly
// by `check_file_exists_any` (which probes the mounted volumes,
// including native backends). Keeping this check would have fired a
// warning ! on every Ubuntu/Debian/Fedora BLS entry under v0.9.x.
