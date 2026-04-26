use alloc::{string::String, vec::Vec};

use uefi::Result;

use crate::fs::Volume;

// Defaults aligned with examples/policy.toml so a fresh install (or a
// delete-policy-and-reboot) matches documented behavior.
const DEFAULT_TIMEOUT_MS: u32 = 5000;
const DEFAULT_CRASH_THRESHOLD: u8 = 3;

// Compiled-in ceilings for security-critical fields. ESP-loaded policies
// more permissive than these get clamped at load time. Attacker with
// offline ESP write access cannot weaken LamBoot below this floor.
const MAX_CRASH_THRESHOLD: u8 = 10;
const MAX_WATCHDOG_GRACE_SECONDS: u32 = 300;
// Build-time hardening knobs. Flip these to `true` in a downstream
// hardened build to force the corresponding runtime posture regardless
// of what ESP policy.toml says. Default distribution leaves them off so
// SB-off and measured-boot-off remain valid operator choices.
const FLOOR_REQUIRE_SECURE_BOOT: bool = false;
const FLOOR_REQUIRE_MEASURED_BOOT: bool = false;

#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "policy configuration is inherently boolean flags"
)]
pub(crate) struct Policy {
    pub version: u32,
    pub default_timeout_ms: u32,
    pub default_entry: Option<String>,
    pub secure_boot_required: bool,
    pub measured_boot: bool,
    pub fallback_order: Vec<String>,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub ui_theme: String,
    pub ui_font_size: FontSize,
    pub watchdog_enabled: bool,
    pub watchdog_grace_seconds: u32,
    pub crash_threshold: u8,
    pub modules_enabled: bool,
    /// SDS-6: legacy UEFI filesystem driver load policy. Default Auto.
    /// Consumed by `drivers::load_drivers` to gate per-driver loading.
    pub drivers_legacy: DriversLegacyMode,
    /// SDS-3: native PE loader policy. Default Auto.
    /// Consumed by `boot.rs` to gate native-vs-firmware load path.
    pub loader_native_pe: LoaderNativePeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontSize {
    Small,
    Medium,
    Large,
}

/// Legacy UEFI-filesystem-driver load policy (SDS-6).
///
/// Controls whether `drivers::load_drivers` loads `.efi` filesystem
/// drivers from `\EFI\LamBoot\drivers\`. As native backends cover
/// more filesystems, the default `Auto` mode eliminates more driver
/// loads — each load is a surface that interacts with shim 15.8's
/// ShimLock-uninstall behavior, so skipping is a security win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriversLegacyMode {
    /// Load drivers only for filesystems LamBoot does NOT natively
    /// support. Default. Currently skips `ext4_x64.efi` and
    /// `ext2_x64.efi` because `Ext4Backend` (SDS-2) covers ext2/3/4.
    Auto,
    /// Load every driver present in the drivers directory, unconditional.
    /// v0.8.3 behavior. Useful during transitional testing or if the
    /// native backend hits an edge case and the operator wants to fall
    /// back to the UEFI driver temporarily.
    Always,
    /// Load no drivers. Skip all, regardless of coverage. Security-
    /// conscious posture for systems whose filesystems are all natively
    /// covered — asserts LamBoot will never `LoadImage` a third-party
    /// UEFI binary at boot.
    Never,
}

impl DriversLegacyMode {
    /// Parse a policy.toml string value. Case-insensitive. Unknown
    /// values fall back to `Auto` with a log warning.
    pub(crate) fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auto" => Self::Auto,
            "always" => Self::Always,
            "never" => Self::Never,
            other => {
                log::warn!(
                    "policy.toml [drivers].legacy_uefi_drivers = {other:?} not recognized \
                     (valid: \"auto\" | \"always\" | \"never\"); defaulting to \"auto\""
                );
                Self::Auto
            }
        }
    }
}

/// Native PE loader policy (SDS-3 v1.1 amendment #2).
///
/// Controls whether `boot.rs` routes image loads through LamBoot's
/// own `pe_loader::load_pe` or falls back to `uefi::boot::load_image`.
/// Native path bypasses firmware's Security2Arch hook — the root
/// cause of the v0.8.3 shim-15.8 ShimLock-uninstall failure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoaderNativePeMode {
    /// Use native PE loader when ShimLock::Verify verified the bytes
    /// pre-load. Default for v0.9.0 release. Fall back to firmware
    /// load_image if verification couldn't happen (no shim + SB off,
    /// for instance — in which case the firmware path is safe too).
    Auto,
    /// Always use native loader. Fail boot if byte verification
    /// wasn't performed. For security-conscious operators who want
    /// to assert LamBoot's own PE loader is the only image-load path.
    Always,
    /// Fall back entirely to `uefi::boot::load_image`. v0.8.3
    /// behavior. Available as a one-keystroke rollback if the native
    /// loader ever hits an unexpected firmware quirk during v0.9.0
    /// release validation.
    Never,
}

impl LoaderNativePeMode {
    pub(crate) fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auto" => Self::Auto,
            "always" => Self::Always,
            "never" => Self::Never,
            other => {
                log::warn!(
                    "policy.toml [loader].native_pe = {other:?} not recognized \
                     (valid: \"auto\" | \"always\" | \"never\"); defaulting to \"auto\""
                );
                Self::Auto
            }
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 1,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            default_entry: None,
            // SB-required stays opt-in since SB-off is a valid homelab
            // posture; measured_boot defaults on because it's free
            // when a TPM exists and a no-op when it doesn't.
            secure_boot_required: false,
            measured_boot: true,
            fallback_order: Vec::new(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            ui_theme: String::from("default"),
            ui_font_size: FontSize::Medium,
            watchdog_enabled: true,
            watchdog_grace_seconds: 15,
            crash_threshold: DEFAULT_CRASH_THRESHOLD,
            modules_enabled: true,
            drivers_legacy: DriversLegacyMode::Auto,
            loader_native_pe: LoaderNativePeMode::Auto,
        }
    }
}

impl Policy {
    /// Check if an EFI path is allowed by policy
    pub(crate) fn allowed(&self, path: &str) -> bool {
        // If no allowlist, everything is allowed (unless denied)
        if self.allowlist.is_empty() {
            return !self.is_denied(path);
        }

        // Check allowlist
        let allowed = self.allowlist.iter().any(|pattern| {
            if pattern.contains('*') {
                path_matches_pattern(path, pattern)
            } else {
                path.eq_ignore_ascii_case(pattern)
            }
        });

        allowed && !self.is_denied(path)
    }

    fn is_denied(&self, path: &str) -> bool {
        self.denylist.iter().any(|pattern| {
            if pattern.contains('*') {
                path_matches_pattern(path, pattern)
            } else {
                path.eq_ignore_ascii_case(pattern)
            }
        })
    }
}

/// Record emitted by [`apply_policy_floor`] for each clamped field.
/// Consumed by the trust-log writer so offline audits can see that the
/// ESP policy.toml tried to weaken a security-critical field.
#[derive(Debug, Clone)]
pub(crate) struct PolicyClampReport {
    pub field: &'static str,
    pub esp_value: String,
    pub floor_value: String,
}

/// Load policy from ESP and apply the compiled-in security floor.
/// Returns the clamped policy plus a report of any fields that were
/// adjusted (empty if the ESP policy was within bounds).
pub(crate) fn load_policy(esp: &mut Volume) -> Result<(Policy, Vec<PolicyClampReport>)> {
    let content = esp.read_to_string_str("/EFI/LamBoot/policy.toml")?;
    let mut policy = parse_policy_toml(&content);
    let clamps = apply_policy_floor(&mut policy);
    Ok((policy, clamps))
}

/// Apply compiled-in security ceilings to an in-memory policy. ESP
/// policies more permissive than the floor get silently adjusted here.
/// Each adjustment produces a [`PolicyClampReport`] for trust-log emission.
fn apply_policy_floor(policy: &mut Policy) -> Vec<PolicyClampReport> {
    let mut reports = Vec::new();

    if policy.crash_threshold > MAX_CRASH_THRESHOLD {
        reports.push(PolicyClampReport {
            field: "crash_threshold",
            esp_value: alloc::format!("{}", policy.crash_threshold),
            floor_value: alloc::format!("{MAX_CRASH_THRESHOLD}"),
        });
        policy.crash_threshold = MAX_CRASH_THRESHOLD;
    }

    if policy.watchdog_grace_seconds > MAX_WATCHDOG_GRACE_SECONDS {
        reports.push(PolicyClampReport {
            field: "watchdog_grace_seconds",
            esp_value: alloc::format!("{}", policy.watchdog_grace_seconds),
            floor_value: alloc::format!("{MAX_WATCHDOG_GRACE_SECONDS}"),
        });
        policy.watchdog_grace_seconds = MAX_WATCHDOG_GRACE_SECONDS;
    }

    if FLOOR_REQUIRE_SECURE_BOOT && !policy.secure_boot_required {
        reports.push(PolicyClampReport {
            field: "secure_boot_required",
            esp_value: String::from("false"),
            floor_value: String::from("true"),
        });
        policy.secure_boot_required = true;
    }

    if FLOOR_REQUIRE_MEASURED_BOOT && !policy.measured_boot {
        reports.push(PolicyClampReport {
            field: "measured_boot",
            esp_value: String::from("false"),
            floor_value: String::from("true"),
        });
        policy.measured_boot = true;
    }

    reports
}

/// Parse policy from TOML string with section awareness
fn parse_policy_toml(toml: &str) -> Policy {
    let mut policy = Policy::default();
    let mut current_section = String::new();

    for line in toml.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse section headers: [section_name]
        if line.starts_with('[') {
            if let Some(end) = line.rfind(']') {
                current_section = String::from(line[1..end].trim());
            }
            continue;
        }

        // Parse key = value
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();

            // Build qualified key: "section.key" or just "key" for root
            let qualified = if current_section.is_empty() {
                String::from(key)
            } else {
                alloc::format!("{current_section}.{key}")
            };

            apply_config_value(&mut policy, &qualified, value);
        }
    }

    policy
}

/// Apply a single configuration value to the policy
fn apply_config_value(policy: &mut Policy, qualified_key: &str, raw_value: &str) {
    // Strip quotes from string values
    let value = raw_value.trim_matches('"');

    match qualified_key {
        // Root-level keys
        "version" => {
            if let Ok(v) = value.parse() {
                policy.version = v;
            }
        }
        "default_timeout_ms" => {
            if let Ok(v) = value.parse() {
                policy.default_timeout_ms = v;
            }
        }
        "default_entry" => {
            policy.default_entry = Some(String::from(value));
        }

        // [security] section
        "security.secure_boot_required" => {
            policy.secure_boot_required = value == "true";
        }
        "security.measured_boot" => {
            policy.measured_boot = value == "true";
        }
        "security.crash_threshold" => {
            if let Ok(v) = value.parse() {
                policy.crash_threshold = v;
            }
        }
        "security.fallback_order" => {
            policy.fallback_order = parse_toml_array(raw_value);
        }
        "security.allowlist" => {
            policy.allowlist = parse_toml_array(raw_value);
        }
        "security.denylist" => {
            policy.denylist = parse_toml_array(raw_value);
        }

        // [ui] section
        "ui.theme" => {
            policy.ui_theme = String::from(value);
        }
        "ui.font_size" => {
            policy.ui_font_size = match value {
                "small" => FontSize::Small,
                "large" => FontSize::Large,
                _ => FontSize::Medium,
            };
        }

        // [watchdog] section
        "watchdog.enabled" => {
            policy.watchdog_enabled = value == "true";
        }
        "watchdog.grace_seconds" => {
            if let Ok(v) = value.parse() {
                policy.watchdog_grace_seconds = v;
            }
        }

        // [modules] section
        "modules.enabled" => {
            policy.modules_enabled = value == "true";
        }

        // [drivers] section — SDS-6
        "drivers.legacy_uefi_drivers" => {
            policy.drivers_legacy = DriversLegacyMode::parse(value);
        }

        // [loader] section — SDS-3
        "loader.native_pe" => {
            policy.loader_native_pe = LoaderNativePeMode::parse(value);
        }

        _ => {
            log::debug!("Unknown config key: {qualified_key}");
        }
    }
}

/// Parse a TOML inline array: `["value1", "value2", "value3"]`
fn parse_toml_array(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    // Strip outer brackets
    let inner = if raw.starts_with('[') && raw.ends_with(']') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };

    inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\''))
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Simple pattern matching for paths
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let path_lower = path.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    if pattern_lower.contains('*') {
        if pattern_lower.starts_with('*') && pattern_lower.ends_with('*') {
            let middle = &pattern_lower[1..pattern_lower.len() - 1];
            return path_lower.contains(middle);
        }

        if let Some(suffix) = pattern_lower.strip_prefix('*') {
            return path_lower.ends_with(suffix);
        }

        if pattern_lower.ends_with('*') {
            let prefix = &pattern_lower[..pattern_lower.len() - 1];
            return path_lower.starts_with(prefix);
        }
    }

    path_lower == pattern_lower
}
