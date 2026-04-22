//! SMBIOS table reading for VM identification and fleet management.
//!
//! Reads SMBIOS Type 1 (System Information) and Type 11 (OEM Strings)
//! from the firmware configuration table. On Proxmox/QEMU VMs, these
//! contain VM identity (manufacturer, product, serial, UUID) and
//! custom fleet tags injected via `args: -smbios type=11,value=key=val`.

use alloc::{string::String, vec::Vec};
use core::ffi::c_void;

use uefi::table::cfg::ConfigTableEntry;

/// System information from SMBIOS Type 1
#[derive(Debug, Clone, Default)]
pub(crate) struct SystemInfo {
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub version: Option<String>,
    pub serial_number: Option<String>,
    pub family: Option<String>,
}

/// OEM strings from SMBIOS Type 11
/// On Proxmox, these can contain fleet identity tags like:
///   lamboot.fleet-id=cluster-01
///   lamboot.monitor=http://10.0.0.1:9090
pub(crate) type OemStrings = Vec<String>;

/// Read SMBIOS tables and extract system info + OEM strings
pub(crate) fn read_smbios() -> (SystemInfo, OemStrings) {
    let mut sys_info = SystemInfo::default();
    let mut oem_strings = Vec::new();

    // Find SMBIOS entry point in EFI Configuration Table
    let table_addr = uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == ConfigTableEntry::SMBIOS_GUID
                || entry.guid == ConfigTableEntry::SMBIOS3_GUID
            {
                return Some(entry.address);
            }
        }
        None
    });

    let Some(entry_point) = table_addr else {
        log::debug!("No SMBIOS table found in configuration table");
        return (sys_info, oem_strings);
    };

    // Parse SMBIOS entry point to find the structure table address
    // SMBIOS 2.x entry point: signature "_SM_" at offset 0
    // SMBIOS 3.x entry point: signature "_SM3_" at offset 0
    let (table_addr, table_length) = parse_entry_point(entry_point);
    let Some(table_addr) = table_addr else {
        log::warn!("Failed to parse SMBIOS entry point");
        return (sys_info, oem_strings);
    };

    // Walk the SMBIOS structure table
    walk_smbios_structures(table_addr, table_length, &mut sys_info, &mut oem_strings);

    if sys_info.manufacturer.is_some() {
        log::info!(
            "SMBIOS: {} {} ({})",
            sys_info.manufacturer.as_deref().unwrap_or("?"),
            sys_info.product_name.as_deref().unwrap_or("?"),
            sys_info.serial_number.as_deref().unwrap_or("?"),
        );
    }
    if !oem_strings.is_empty() {
        log::info!("SMBIOS OEM strings: {}", oem_strings.len());
        for s in &oem_strings {
            log::info!("  OEM: {s}");
        }
    }

    (sys_info, oem_strings)
}

/// Get a specific LamBoot OEM string value by key prefix.
/// Looks for "lamboot.KEY=VALUE" in OEM strings.
pub(crate) fn get_lamboot_oem_value<'a>(oem_strings: &'a [String], key: &str) -> Option<&'a str> {
    let prefix = alloc::format!("lamboot.{key}=");
    for s in oem_strings {
        if let Some(val) = s.strip_prefix(&prefix) {
            return Some(val);
        }
    }
    None
}

/// Parse SMBIOS entry point structure to find the table address
fn parse_entry_point(entry_point: *const c_void) -> (Option<*const u8>, usize) {
    // SAFETY: entry_point comes from the EFI Configuration Table,
    // which the firmware guarantees points to valid SMBIOS data.
    let bytes = unsafe { core::slice::from_raw_parts(entry_point.cast::<u8>(), 32) };

    // Check for SMBIOS 2.x: "_SM_" signature
    if bytes.len() >= 24 && &bytes[0..4] == b"_SM_" {
        // Structure table address at offset 0x18 (4 bytes, LE)
        let addr = u32::from_le_bytes([bytes[0x18], bytes[0x19], bytes[0x1A], bytes[0x1B]]);
        // Structure table length at offset 0x16 (2 bytes, LE)
        let length = u16::from_le_bytes([bytes[0x16], bytes[0x17]]) as usize;
        return (Some(addr as *const u8), length);
    }

    // Check for SMBIOS 3.x: "_SM3_" signature
    if bytes.len() >= 24 && &bytes[0..5] == b"_SM3_" {
        // Structure table address at offset 0x10 (8 bytes, LE)
        let addr = u64::from_le_bytes([
            bytes[0x10],
            bytes[0x11],
            bytes[0x12],
            bytes[0x13],
            bytes[0x14],
            bytes[0x15],
            bytes[0x16],
            bytes[0x17],
        ]);
        // Max structure table length at offset 0x0C (4 bytes, LE)
        let length =
            u32::from_le_bytes([bytes[0x0C], bytes[0x0D], bytes[0x0E], bytes[0x0F]]) as usize;
        return (Some(addr as *const u8), length);
    }

    (None, 0)
}

/// Walk SMBIOS structures and extract Type 1 and Type 11 data
fn walk_smbios_structures(
    table_start: *const u8,
    table_length: usize,
    sys_info: &mut SystemInfo,
    oem_strings: &mut Vec<String>,
) {
    let mut offset: usize = 0;

    // SAFETY: table_start points to the SMBIOS structure table,
    // validated by the firmware's Configuration Table entry.
    // We bounds-check against table_length on every access.
    let table = unsafe { core::slice::from_raw_parts(table_start, table_length) };

    while offset + 4 <= table_length {
        let struct_type = table[offset];
        let struct_length = table[offset + 1] as usize;

        if struct_type == 127 {
            break; // End-of-table marker
        }

        if struct_length < 4 || offset + struct_length > table_length {
            break;
        }

        // Collect strings that follow the formatted area
        // Strings are null-terminated, the string section ends with double-null
        let strings_start = offset + struct_length;
        let strings = collect_strings(table, strings_start, table_length);
        let strings_end = find_strings_end(table, strings_start, table_length);

        match struct_type {
            1 => {
                // Type 1: System Information
                // String indices at offsets: 4=manufacturer, 5=product, 6=version, 7=serial, 8=sku_number, 26=family
                if struct_length > 4 {
                    sys_info.manufacturer = get_string(&strings, table[offset + 4]);
                }
                if struct_length > 5 {
                    sys_info.product_name = get_string(&strings, table[offset + 5]);
                }
                if struct_length > 6 {
                    sys_info.version = get_string(&strings, table[offset + 6]);
                }
                if struct_length > 7 {
                    sys_info.serial_number = get_string(&strings, table[offset + 7]);
                }
                if struct_length > 26 {
                    sys_info.family = get_string(&strings, table[offset + 26]);
                }
            }
            11 if struct_length > 4 => {
                // Type 11: OEM Strings — count at offset 4
                let count = table[offset + 4] as usize;
                for s in strings.iter().take(count.min(strings.len())) {
                    oem_strings.push(s.clone());
                }
            }
            _ => {}
        }

        // Advance past the formatted area + string section
        offset = strings_end;
    }
}

/// Collect null-terminated strings from the unformatted area after a structure
fn collect_strings(table: &[u8], start: usize, max_len: usize) -> Vec<String> {
    let mut strings = Vec::new();
    let mut pos = start;

    while pos < max_len {
        if table[pos] == 0 {
            break; // End of strings (double-null)
        }

        // Find end of this string
        let str_start = pos;
        while pos < max_len && table[pos] != 0 {
            pos += 1;
        }

        if let Ok(s) = core::str::from_utf8(&table[str_start..pos]) {
            strings.push(String::from(s));
        }

        if pos < max_len {
            pos += 1; // Skip null terminator
        }
    }

    strings
}

/// Find the end of the strings section (past the double-null terminator)
fn find_strings_end(table: &[u8], start: usize, max_len: usize) -> usize {
    let mut pos = start;

    // If first byte is 0, the structure has no strings — skip the double null
    if pos < max_len && table[pos] == 0 {
        return pos + 2;
    }

    while pos + 1 < max_len {
        if table[pos] == 0 && table[pos + 1] == 0 {
            return pos + 2; // Past double-null
        }
        pos += 1;
    }

    max_len // Reached end of table
}

/// Get a string by 1-based index from the collected strings
fn get_string(strings: &[String], index: u8) -> Option<String> {
    if index == 0 {
        return None;
    }
    strings.get((index - 1) as usize).cloned()
}
