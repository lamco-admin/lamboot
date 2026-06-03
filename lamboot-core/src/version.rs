//! Layer: 5 (cross-cutting) — Trust & Audit.
//!
//! Single source of truth for LamBoot identity and version strings.
//!
//! Every site that emits a product name or version — trust log, boot log,
//! GUI footer, UEFI `LoaderInfo` variable, JSON reports — must read from
//! this module. Cargo's `CARGO_PKG_VERSION*` env vars are the upstream
//! source; this module formats them for each consumer.
//!
//! Adding a new version-bearing emission site? Use one of the items here.
//! Never write a literal version like `"0.2.0"` or `"LamBoot 0.9.1"`
//! anywhere else — it will go stale and the trust log + the actual
//! shipping binary will disagree. (Bug 20: the LoaderInfo CHAR16 string
//! got stuck at `"LamBoot 0.2.0"` for six minor releases because it was
//! a literal byte sequence in `health.rs`.)

use alloc::vec::Vec;

/// Product name. Used in trust log headers, the GUI title, and the
/// `LoaderInfo` UEFI variable prefix.
pub(crate) const NAME: &str = "LamBoot";

/// Semver version string sourced from `Cargo.toml` at compile time.
/// Format: `MAJOR.MINOR.PATCH` (e.g. `"0.9.2"`).
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Packed `u32` form of the version for the `LamBootVersion` NVRAM
/// variable: `(major << 16) | (minor << 8) | patch`. Built at compile
/// time from the three split env vars so it can never disagree with
/// `VERSION`. Two-digit components are accepted (e.g. `0.10.0`); any
/// non-numeric component falls back to `0` rather than failing the build.
pub(crate) const VERSION_PACKED_U32: u32 = {
    let major = parse_u32_or_zero(env!("CARGO_PKG_VERSION_MAJOR"));
    let minor = parse_u32_or_zero(env!("CARGO_PKG_VERSION_MINOR"));
    let patch = parse_u32_or_zero(env!("CARGO_PKG_VERSION_PATCH"));
    (major << 16) | (minor << 8) | patch
};

const fn parse_u32_or_zero(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut n: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            return 0;
        }
        n = n * 10 + (b - b'0') as u32;
        i += 1;
    }
    n
}

/// `"<NAME> <VERSION>"` as a UTF-16LE null-terminated byte sequence,
/// the format the UEFI `LoaderInfo` variable requires (CHAR16). Built
/// at runtime once per boot from the const `NAME` and `VERSION` so it
/// can never diverge from them.
///
/// systemd-boot's `bootctl status` reads this variable to print the
/// "Current Boot Loader" line. Vendor GUID is the systemd-boot Boot
/// Loader Interface GUID, not LamBoot's vendor GUID.
pub(crate) fn loader_info_utf16le() -> Vec<u8> {
    let s = alloc::format!("{NAME} {VERSION}");
    let mut bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    // CHAR16 null terminator
    bytes.push(0);
    bytes.push(0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_version_matches_string() {
        // Round-trip: unpack u32 → format → compare to VERSION
        let major = (VERSION_PACKED_U32 >> 16) & 0xff;
        let minor = (VERSION_PACKED_U32 >> 8) & 0xff;
        let patch = VERSION_PACKED_U32 & 0xff;
        let reconstructed = std::format!("{major}.{minor}.{patch}");
        assert_eq!(reconstructed, VERSION);
    }

    #[test]
    fn loader_info_bytes_have_null_terminator() {
        let bytes = loader_info_utf16le();
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
    }

    #[test]
    fn loader_info_starts_with_name_in_utf16() {
        let bytes = loader_info_utf16le();
        // "L" = 0x4c, in UTF-16LE that's [0x4c, 0x00]
        assert_eq!(&bytes[..2], &[b'L', 0]);
    }

    #[test]
    fn parse_u32_handles_two_digits() {
        assert_eq!(parse_u32_or_zero("10"), 10);
        assert_eq!(parse_u32_or_zero("99"), 99);
        assert_eq!(parse_u32_or_zero("0"), 0);
    }

    #[test]
    fn parse_u32_returns_zero_on_non_numeric() {
        assert_eq!(parse_u32_or_zero("1-pre"), 0);
        assert_eq!(parse_u32_or_zero(""), 0);
    }
}
