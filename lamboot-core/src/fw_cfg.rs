//! QEMU fw_cfg device access for host→guest data injection.
//!
//! The fw_cfg device provides a key-value store accessible from the guest
//! via I/O ports (x86) or MMIO (ARM). Used for:
//! - VM Generation ID detection (vmgenid crash counter reset)
//! - Arbitrary host-injected data blobs (fleet config, boot overrides)
//!
//! Host-side injection:
//!   qm set <vmid> --args '-fw_cfg name=opt/lamboot/config,file=/path/to/config.json'

use alloc::{string::String, vec::Vec};

/// Check if QEMU fw_cfg device is present.
#[cfg(target_arch = "x86_64")]
pub(crate) fn fw_cfg_available() -> bool {
    let mut sig = [0u8; 4];
    fw_cfg_select(0x0000);
    for byte in &mut sig {
        *byte = fw_cfg_read_byte();
    }
    &sig == b"QEMU"
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn fw_cfg_available() -> bool {
    false
}

/// Read the VM Generation ID (16 bytes) from fw_cfg.
/// Returns None if not on QEMU or vmgenid not configured.
#[cfg(target_arch = "x86_64")]
pub(crate) fn read_vmgenid() -> Option<[u8; 16]> {
    if !fw_cfg_available() {
        return None;
    }
    read_file_raw("etc/vmgenid_guid").and_then(|data| {
        if data.len() >= 16 {
            let mut guid = [0u8; 16];
            guid.copy_from_slice(&data[..16]);
            Some(guid)
        } else {
            None
        }
    })
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn read_vmgenid() -> Option<[u8; 16]> {
    None
}

/// Read an arbitrary fw_cfg file by name.
/// Returns the file contents as a byte vector, or None if not found.
///
/// Use for host-injected data:
///   qm set <vmid> --args '-fw_cfg name=opt/lamboot/config,file=/path/to/config.json'
///
/// Then read with: `fw_cfg_read_file("opt/lamboot/config")`
#[cfg(target_arch = "x86_64")]
pub(crate) fn read_file(name: &str) -> Option<Vec<u8>> {
    if !fw_cfg_available() {
        return None;
    }
    read_file_raw(name)
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn read_file(_name: &str) -> Option<Vec<u8>> {
    None
}

/// Read an arbitrary fw_cfg file as a UTF-8 string.
#[cfg(target_arch = "x86_64")]
pub(crate) fn read_file_string(name: &str) -> Option<String> {
    let data = read_file(name)?;
    String::from_utf8(data).ok()
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn read_file_string(_name: &str) -> Option<String> {
    None
}

/// List available fw_cfg file names.
#[cfg(target_arch = "x86_64")]
#[expect(dead_code, reason = "utility for diag-shell fw_cfg browser screen")]
pub(crate) fn list_files() -> Vec<(String, u32)> {
    let mut files = Vec::new();

    if !fw_cfg_available() {
        return files;
    }

    fw_cfg_select(0x0019); // FW_CFG_FILE_DIR
    let count = fw_cfg_read_u32_be();
    if count == 0 || count > 1000 {
        return files;
    }

    for _ in 0..count {
        let size = fw_cfg_read_u32_be();
        let _select = fw_cfg_read_u16_be();
        let _reserved = fw_cfg_read_u16_be();
        let mut name_buf = [0u8; 56];
        for byte in &mut name_buf {
            *byte = fw_cfg_read_byte();
        }

        let name_str = core::str::from_utf8(&name_buf).unwrap_or("");
        let name = name_str.trim_end_matches('\0');
        if !name.is_empty() {
            files.push((String::from(name), size));
        }
    }

    files
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn list_files() -> Vec<(String, u32)> {
    Vec::new()
}

// ============================================================================
// Internal: fw_cfg file reading via directory scan
// ============================================================================

#[cfg(target_arch = "x86_64")]
fn read_file_raw(target_name: &str) -> Option<Vec<u8>> {
    fw_cfg_select(0x0019); // FW_CFG_FILE_DIR

    let count = fw_cfg_read_u32_be();
    if count == 0 || count > 1000 {
        return None;
    }

    for _ in 0..count {
        let size = fw_cfg_read_u32_be();
        let select = fw_cfg_read_u16_be();
        let _reserved = fw_cfg_read_u16_be();
        let mut name_buf = [0u8; 56];
        for byte in &mut name_buf {
            *byte = fw_cfg_read_byte();
        }

        let name_str = core::str::from_utf8(&name_buf).unwrap_or("");
        let name = name_str.trim_end_matches('\0');

        if name == target_name {
            // Found — read the data
            fw_cfg_select(select);
            let mut data = Vec::with_capacity(size as usize);
            for _ in 0..size {
                data.push(fw_cfg_read_byte());
            }
            return Some(data);
        }
    }

    None
}

// ============================================================================
// I/O port access (x86_64 only)
// ============================================================================

/// Write to fw_cfg selector register (I/O port 0x510)
#[cfg(target_arch = "x86_64")]
fn fw_cfg_select(selector: u16) {
    // SAFETY: I/O port 0x510 is the fw_cfg selector register on QEMU.
    // Writing to it selects a configuration item.
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") 0x510u16,
            in("ax") selector,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Read one byte from fw_cfg data register (I/O port 0x511)
#[cfg(target_arch = "x86_64")]
fn fw_cfg_read_byte() -> u8 {
    let value: u8;
    // SAFETY: I/O port 0x511 is the fw_cfg data register on QEMU.
    // Each read returns the next byte of the selected item.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") 0x511u16,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

#[cfg(target_arch = "x86_64")]
fn fw_cfg_read_u16_be() -> u16 {
    let hi = fw_cfg_read_byte() as u16;
    let lo = fw_cfg_read_byte() as u16;
    (hi << 8) | lo
}

#[cfg(target_arch = "x86_64")]
fn fw_cfg_read_u32_be() -> u32 {
    let b3 = fw_cfg_read_byte() as u32;
    let b2 = fw_cfg_read_byte() as u32;
    let b1 = fw_cfg_read_byte() as u32;
    let b0 = fw_cfg_read_byte() as u32;
    (b3 << 24) | (b2 << 16) | (b1 << 8) | b0
}
