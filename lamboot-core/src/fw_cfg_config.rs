//! Layer: 0 — Platform Introspection.
//!
//! Parse the host-supplied JSON blob exposed via fw_cfg at
//! `opt/lamboot/config`.
//!
//! Format is the lamboot-hookscript v0.8.4+ per-VM JSON, schema_version="v1":
//! a flat top-level object whose string-valued keys we care about are
//! `fleet_id`, `vmid`, `role`, and `hostname`. Other fields (`schema_version`,
//! `written_by`, `written_at`, `tags_at_setup`) are ignored.
//!
//! We hand-roll the parse rather than pull in serde_json/serde_json_core
//! because the input shape is fixed and tiny (a few hundred bytes,
//! always object-of-string-pairs at top level), and adding a JSON crate
//! to the no_std UEFI bootloader brings more risk than 60 lines of careful
//! scanning. The parser is intentionally strict on string values only —
//! `tags_at_setup: []` parses as "no value" for the key, which is fine
//! because we never need its value here.
//!
//! Spec source: `tools/lamboot-hookscript.pl` `write_per_vm_json()`.

use alloc::string::String;

/// Fields extracted from the fw_cfg JSON blob. All fields are Option
/// because the host may suppress any of them via `[hookscript]` toggles
/// in `/etc/lamboot/fleet.toml`.
#[derive(Default, Debug)]
pub(crate) struct FwCfgConfig {
    pub fleet_id: Option<String>,
    pub vmid: Option<String>,
    pub role: Option<String>,
    pub host_node: Option<String>,
}

impl FwCfgConfig {
    /// Parse a JSON object of the v0.8.4 hookscript shape. Returns a
    /// possibly-empty FwCfgConfig — never errors, on parse failure we
    /// simply yield None-ish fields and let SMBIOS-based fallback take
    /// over downstream.
    pub(crate) fn parse(input: &str) -> Self {
        let mut out = FwCfgConfig::default();
        out.fleet_id = extract_string_field(input, "fleet_id");
        out.vmid = extract_string_field(input, "vmid");
        out.role = extract_string_field(input, "role");
        // The hookscript field is "hostname"; we expose it as host_node
        // to disambiguate from the GUEST's hostname inside the VM.
        out.host_node = extract_string_field(input, "hostname");
        out
    }
}

/// Extract the string value associated with `key` from a flat JSON object.
/// Returns None if the key is missing, the value is not a string, or the
/// value is the empty string. Handles common JSON escapes (\\ \" \n \r \t).
fn extract_string_field(input: &str, key: &str) -> Option<String> {
    // Search for the key as `"<key>"` to avoid matching substrings of
    // other keys (e.g. searching for "vmid" must not match "old_vmid").
    let needle_a = alloc::format!("\"{key}\"");
    let key_pos = input.find(&needle_a)?;
    let after_key = &input[key_pos + needle_a.len()..];

    // Skip whitespace + the colon that separates key from value.
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];

    // Skip whitespace before the opening quote. If the next non-whitespace
    // char isn't '"', this isn't a string-valued field (could be array,
    // object, number, null). We treat that as "no value".
    let trimmed = after_colon.trim_start();
    let mut chars = trimmed.chars();
    if chars.next() != Some('"') {
        return None;
    }

    // Read until the closing quote, honoring backslash escapes. We accept
    // \" \\ \/ \n \r \t \b \f and the Unicode escape \uXXXX. Anything
    // else after a backslash falls through as that literal character —
    // good enough for the simple identifier-shaped values the hookscript
    // writes (no embedded newlines, no Unicode, no quotes).
    let mut out = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            match c {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\x08'),
                'f' => out.push('\x0c'),
                // Treat \uXXXX as literal "uXXXX" — the hookscript never
                // emits non-ASCII identifier values, so seeing one is
                // already a sign of corrupted input. Don't crash; just
                // pass through.
                other => out.push(other),
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            // Closing quote — done.
            if out.is_empty() {
                return None;
            }
            return Some(out);
        } else {
            out.push(c);
        }
    }

    // Reached end without closing quote — malformed input.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "schema_version": "v1",
  "vmid": "364",
  "hostname": "pve2",
  "fleet_id": "lamco-pve2",
  "role": "lamboot-archinstall-target",
  "written_by": "lamboot-hookscript 0.8.4",
  "written_at": "2026-05-29T01:12:12Z",
  "tags_at_setup": []
}"#;

    #[test]
    fn parses_full_v1_blob() {
        let cfg = FwCfgConfig::parse(SAMPLE);
        assert_eq!(cfg.fleet_id.as_deref(), Some("lamco-pve2"));
        assert_eq!(cfg.vmid.as_deref(), Some("364"));
        assert_eq!(cfg.role.as_deref(), Some("lamboot-archinstall-target"));
        assert_eq!(cfg.host_node.as_deref(), Some("pve2"));
    }

    #[test]
    fn missing_keys_yield_none() {
        let input = r#"{"schema_version":"v1"}"#;
        let cfg = FwCfgConfig::parse(input);
        assert!(cfg.fleet_id.is_none());
        assert!(cfg.vmid.is_none());
        assert!(cfg.role.is_none());
        assert!(cfg.host_node.is_none());
    }

    #[test]
    fn empty_string_value_yields_none() {
        let input = r#"{"fleet_id":"","vmid":"x"}"#;
        let cfg = FwCfgConfig::parse(input);
        assert!(cfg.fleet_id.is_none());
        assert_eq!(cfg.vmid.as_deref(), Some("x"));
    }

    #[test]
    fn array_value_yields_none() {
        let input = r#"{"tags_at_setup": [], "vmid": "5"}"#;
        let cfg = FwCfgConfig::parse(input);
        assert_eq!(cfg.vmid.as_deref(), Some("5"));
    }

    #[test]
    fn escaped_quote_handled() {
        let input = r#"{"role": "has \"quote\" in it"}"#;
        let cfg = FwCfgConfig::parse(input);
        assert_eq!(cfg.role.as_deref(), Some(r#"has "quote" in it"#));
    }

    #[test]
    fn malformed_blob_silently_returns_empty() {
        let cfg = FwCfgConfig::parse("not json");
        assert!(cfg.fleet_id.is_none());
    }

    #[test]
    fn substring_collision_avoided() {
        // "old_vmid" should NOT match when searching for "vmid"
        let input = r#"{"old_vmid": "999", "vmid": "364"}"#;
        let cfg = FwCfgConfig::parse(input);
        assert_eq!(cfg.vmid.as_deref(), Some("364"));
    }
}
