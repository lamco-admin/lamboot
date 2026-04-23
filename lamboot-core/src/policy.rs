use alloc::{string::String, vec::Vec};

use uefi::Result;

use crate::fs::Volume;

const DEFAULT_TIMEOUT_MS: u32 = 4000;
const DEFAULT_CRASH_THRESHOLD: u8 = 2;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontSize {
    Small,
    Medium,
    Large,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 1,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            default_entry: None,
            secure_boot_required: false,
            measured_boot: false,
            fallback_order: Vec::new(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            ui_theme: String::from("default"),
            ui_font_size: FontSize::Medium,
            watchdog_enabled: true,
            watchdog_grace_seconds: 15,
            crash_threshold: DEFAULT_CRASH_THRESHOLD,
            modules_enabled: true,
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

/// Load policy from ESP
pub(crate) fn load_policy(esp: &mut Volume) -> Result<Policy> {
    let content = esp.read_to_string_str("/EFI/LamBoot/policy.toml")?;
    Ok(parse_policy_toml(&content))
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
