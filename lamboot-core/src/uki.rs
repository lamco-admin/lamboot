//! UKI (Unified Kernel Image) PE section parser.
//!
//! Parses PE/COFF sections from UKI .efi files to extract metadata:
//!   - `.osrel` — OS identification (os-release key=value format)
//!   - `.cmdline` — Embedded kernel command line
//!   - `.uname` — Kernel version string
//!
//! No external crate dependency. No unsafe code. ~150 lines.

use alloc::{string::String, vec::Vec};

/// Section names as 8-byte null-padded arrays for comparison
const SECTION_OSREL: [u8; 8] = *b".osrel\0\0";
const SECTION_CMDLINE: [u8; 8] = *b".cmdline";
const SECTION_UNAME: [u8; 8] = *b".uname\0\0";

/// Metadata extracted from a UKI PE binary
#[derive(Debug, Clone)]
pub(crate) struct UkiMetadata {
    /// Display name from .osrel PRETTY_NAME
    pub os_name: Option<String>,
    /// Version from .osrel VERSION_ID (available for host-side report consumption)
    #[expect(
        dead_code,
        reason = "included in boot report via BootContext; PRETTY_NAME covers display"
    )]
    pub os_version: Option<String>,
    /// Kernel version from .uname
    pub kernel_version: Option<String>,
    /// Embedded command line from .cmdline
    pub cmdline: Option<String>,
    /// Machine type from COFF header (0x8664 = x86_64, 0xAA64 = aarch64)
    #[expect(
        dead_code,
        reason = "UEFI firmware rejects wrong-arch binaries at LoadImage; kept for future diagnostics"
    )]
    pub machine: u16,
}

/// Location of a PE section within a file
struct SectionLoc {
    name: [u8; 8],
    offset: u64,
    size: usize,
}

/// Parse PE headers from a small buffer to find section locations.
/// Returns (machine_type, section_locations) or None if not a valid PE.
fn parse_pe_section_table(header: &[u8]) -> Option<(u16, Vec<SectionLoc>)> {
    if header.len() < 64 || header[0..2] != [0x4D, 0x5A] {
        return None;
    }

    let pe_offset = read_u32_le(header, 0x3C) as usize;
    if pe_offset + 4 + 20 > header.len() {
        return None;
    }
    if header[pe_offset..pe_offset + 4] != [0x50, 0x45, 0x00, 0x00] {
        return None;
    }

    let coff_offset = pe_offset + 4;
    let machine = read_u16_le(header, coff_offset);
    let num_sections = read_u16_le(header, coff_offset + 2) as usize;
    let opt_header_size = read_u16_le(header, coff_offset + 16) as usize;
    let section_table = coff_offset + 20 + opt_header_size;

    if section_table + (num_sections * 40) > header.len() {
        return None;
    }

    let mut sections = Vec::new();
    for i in 0..num_sections {
        let entry = section_table + (i * 40);
        let mut name = [0u8; 8];
        name.copy_from_slice(&header[entry..entry + 8]);
        let raw_size = read_u32_le(header, entry + 16) as usize;
        let raw_offset = read_u32_le(header, entry + 20) as u64;
        sections.push(SectionLoc {
            name,
            offset: raw_offset,
            size: raw_size,
        });
    }

    Some((machine, sections))
}

/// Read UKI metadata from a file using targeted seeks.
/// Reads only the PE headers (~4KB) and the small metadata sections,
/// avoiding loading the entire UKI binary (which can be 60-100MB).
pub(crate) fn read_uki_metadata(esp: &mut crate::fs::EspVolume, path: &str) -> Option<UkiMetadata> {
    // Pass 1: read PE headers (4KB is enough for DOS + PE + section table)
    let header = esp.read_file_at(path, 0, 4096).ok()?;
    let (machine, sections) = parse_pe_section_table(&header)?;

    // Pass 2: read only the small metadata sections we need
    let mut osrel_data: Option<Vec<u8>> = None;
    let mut cmdline_data: Option<Vec<u8>> = None;
    let mut uname_data: Option<Vec<u8>> = None;

    for sec in &sections {
        // Skip sections larger than 64KB (metadata sections are tiny;
        // .linux and .initrd are megabytes)
        if sec.size > 65536 {
            continue;
        }

        if section_name_eq(&sec.name, SECTION_OSREL) {
            osrel_data = esp.read_file_at(path, sec.offset, sec.size).ok();
        } else if section_name_eq(&sec.name, SECTION_CMDLINE) {
            cmdline_data = esp.read_file_at(path, sec.offset, sec.size).ok();
        } else if section_name_eq(&sec.name, SECTION_UNAME) {
            uname_data = esp.read_file_at(path, sec.offset, sec.size).ok();
        }
    }

    Some(build_metadata(
        machine,
        osrel_data.as_deref(),
        cmdline_data.as_deref(),
        uname_data.as_deref(),
    ))
}

/// Assemble UkiMetadata from raw section data
fn build_metadata(
    machine: u16,
    osrel_data: Option<&[u8]>,
    cmdline_data: Option<&[u8]>,
    uname_data: Option<&[u8]>,
) -> UkiMetadata {
    let (os_name, os_version) = if let Some(osrel) = osrel_data {
        let info = parse_os_release(osrel);
        (info.display_name(), info.display_version())
    } else {
        (None, None)
    };

    let cmdline = cmdline_data
        .and_then(|d| core::str::from_utf8(d).ok())
        .map(|s| String::from(s.trim_end_matches('\0').trim()));

    let kernel_version = uname_data
        .and_then(|d| core::str::from_utf8(d).ok())
        .map(|s| String::from(s.trim_end_matches('\0').trim()));

    UkiMetadata {
        os_name,
        os_version,
        kernel_version,
        cmdline,
        machine,
    }
}

// --- os-release parser ---

struct OsReleaseInfo {
    pretty_name: Option<String>,
    name: Option<String>,
    id: Option<String>,
    version_id: Option<String>,
}

impl OsReleaseInfo {
    /// Best display name: PRETTY_NAME > NAME > ID
    fn display_name(&self) -> Option<String> {
        self.pretty_name
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.id.clone())
    }

    /// Best version: VERSION_ID
    fn display_version(&self) -> Option<String> {
        self.version_id.clone()
    }
}

/// Parse os-release text and return (display_name, id).
/// Public wrapper for use by autodiscovery module.
pub(crate) fn parse_os_release_text(text: &str) -> (Option<String>, Option<String>) {
    let info = parse_os_release(text.as_bytes());
    (info.display_name(), info.id)
}

/// Parse os-release format text (key=value, # comments, quoted values)
fn parse_os_release(data: &[u8]) -> OsReleaseInfo {
    let mut info = OsReleaseInfo {
        pretty_name: None,
        name: None,
        id: None,
        version_id: None,
    };

    let text = if let Ok(s) = core::str::from_utf8(data) {
        s
    } else {
        // Lossy: try up to the first invalid byte
        let valid_len = data
            .iter()
            .position(|&b| b > 0x7E && b != b'\n' && b != b'\r' && b != b'\t')
            .unwrap_or(data.len());
        let Ok(s) = core::str::from_utf8(&data[..valid_len]) else {
            return info;
        };
        s
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        // Strip matching outer quotes
        let value = value.trim();
        let value = if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };

        match key {
            "PRETTY_NAME" => info.pretty_name = Some(String::from(value)),
            "NAME" => info.name = Some(String::from(value)),
            "ID" => info.id = Some(String::from(value)),
            "VERSION_ID" => info.version_id = Some(String::from(value)),
            _ => {}
        }
    }

    info
}

// --- Helper functions ---

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn section_name_eq(name: &[u8], target: [u8; 8]) -> bool {
    name.len() >= 8 && name[..8] == target
}
