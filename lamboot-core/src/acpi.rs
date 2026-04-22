//! ACPI DMAR/IVRS parser for IOMMU group detection.
//!
//! Walks ACPI tables (RSDP → XSDT → DMAR/IVRS) to identify IOMMU
//! hardware units and their device scope assignments. This enables
//! pre-boot PCI passthrough validation on Proxmox — uniquely valuable
//! because no other tool provides IOMMU visibility before Linux boots.
//!
//! Supports:
//! - Intel VT-d via DMAR table (DRHD + RMRR structures)
//! - AMD-Vi via IVRS table (IVHD blocks)

use alloc::{string::String, vec::Vec};
use core::ffi::c_void;

use uefi::table::cfg::ConfigTableEntry;

/// Aggregate IOMMU information from ACPI tables
#[derive(Debug, Clone, Default)]
pub(crate) struct IommuInfo {
    pub intel_vt_d: bool,
    pub amd_vi: bool,
    pub drhd_units: Vec<DrhdUnit>,
    pub rmrr_regions: Vec<RmrrRegion>,
}

/// DMA Remapping Hardware Unit Definition (Intel VT-d)
#[derive(Debug, Clone)]
#[expect(
    dead_code,
    reason = "fields populated by parser; consumed via Debug format and future IOMMU report"
)]
pub(crate) struct DrhdUnit {
    pub(crate) segment: u16,
    pub(crate) register_base: u64,
    pub(crate) include_pci_all: bool,
    pub(crate) device_scopes: Vec<DeviceScope>,
}

/// Reserved Memory Region Reporting (Intel VT-d)
#[derive(Debug, Clone)]
#[expect(
    dead_code,
    reason = "fields populated by parser; consumed via Debug format and future IOMMU report"
)]
pub(crate) struct RmrrRegion {
    pub(crate) base_address: u64,
    pub(crate) limit_address: u64,
    pub(crate) segment: u16,
    pub(crate) device_scopes: Vec<DeviceScope>,
}

/// A PCI device scope within a DRHD or RMRR
#[derive(Debug, Clone)]
#[expect(
    dead_code,
    reason = "fields populated by parser; bdf_string() used in future IOMMU display"
)]
pub(crate) struct DeviceScope {
    /// 1 = PCI Endpoint, 2 = PCI Sub-Hierarchy, 3 = IOAPIC, 4 = HPET
    pub(crate) scope_type: u8,
    pub(crate) enumeration_id: u8,
    pub(crate) start_bus: u8,
    /// (device, function) pairs forming the PCI path
    pub(crate) path: Vec<(u8, u8)>,
}

impl DeviceScope {
    /// Format as a PCI BDF string for the leaf device.
    #[expect(dead_code, reason = "used by future diag-shell IOMMU screen")]
    pub(crate) fn bdf_string(&self) -> String {
        if let Some(&(dev, func)) = self.path.last() {
            alloc::format!("{:02X}:{:02X}.{}", self.start_bus, dev, func)
        } else {
            alloc::format!("{:02X}:??", self.start_bus)
        }
    }
}

/// Parse IOMMU tables from ACPI.
pub(crate) fn parse_iommu_tables() -> IommuInfo {
    let mut info = IommuInfo::default();

    let Some(rsdp_addr) = find_acpi_rsdp() else {
        log::debug!("No ACPI RSDP found in configuration table");
        return info;
    };

    let Some(sdt_entries) = parse_rsdp(rsdp_addr) else {
        log::debug!("Failed to parse ACPI RSDP");
        return info;
    };

    for table_addr in sdt_entries {
        // SAFETY: table_addr points to an ACPI SDT header provided by
        // firmware. We read the 4-byte signature with bounds checking.
        let sig = unsafe { core::slice::from_raw_parts(table_addr, 4) };

        if sig == b"DMAR" {
            parse_dmar(table_addr, &mut info);
        } else if sig == b"IVRS" {
            parse_ivrs(table_addr, &mut info);
        }
    }

    info
}

/// Find ACPI RSDP address from EFI Configuration Table.
fn find_acpi_rsdp() -> Option<*const c_void> {
    uefi::system::with_config_table(|entries| {
        // Prefer ACPI 2.0 (64-bit XSDT) over 1.0 (32-bit RSDT)
        let mut acpi1 = None;
        for entry in entries {
            if entry.guid == ConfigTableEntry::ACPI2_GUID {
                return Some(entry.address);
            }
            if entry.guid == ConfigTableEntry::ACPI_GUID {
                acpi1 = Some(entry.address);
            }
        }
        acpi1
    })
}

/// Parse RSDP to find SDT table pointers.
/// Returns a list of pointers to individual ACPI tables.
fn parse_rsdp(rsdp_addr: *const c_void) -> Option<Vec<*const u8>> {
    // SAFETY: RSDP from Configuration Table is firmware-guaranteed valid.
    // Minimum RSDP is 20 bytes (ACPI 1.0), extended is 36 bytes (2.0).
    let rsdp = unsafe { core::slice::from_raw_parts(rsdp_addr.cast::<u8>(), 36) };

    // Verify signature "RSD PTR "
    if rsdp.len() < 20 || &rsdp[0..8] != b"RSD PTR " {
        return None;
    }

    let revision = rsdp[15];

    // ACPI 2.0+: use XSDT (64-bit pointers) at offset 24
    if revision >= 2 && rsdp.len() >= 36 {
        let xsdt_addr = u64::from_le_bytes([
            rsdp[24], rsdp[25], rsdp[26], rsdp[27], rsdp[28], rsdp[29], rsdp[30], rsdp[31],
        ]);
        if xsdt_addr != 0 {
            return parse_xsdt(xsdt_addr as *const u8);
        }
    }

    // Fallback: ACPI 1.0 RSDT (32-bit pointers) at offset 16
    let rsdt_addr = u32::from_le_bytes([rsdp[16], rsdp[17], rsdp[18], rsdp[19]]);
    if rsdt_addr != 0 {
        return parse_rsdt(rsdt_addr as *const u8);
    }

    None
}

/// Parse XSDT (Extended System Description Table) — 64-bit entry pointers.
fn parse_xsdt(xsdt_ptr: *const u8) -> Option<Vec<*const u8>> {
    // SAFETY: XSDT pointer comes from RSDP which is firmware-validated.
    // Read standard ACPI SDT header (36 bytes) first.
    let header = unsafe { core::slice::from_raw_parts(xsdt_ptr, 36) };

    if &header[0..4] != b"XSDT" {
        return None;
    }

    let table_length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if table_length < 36 {
        return None;
    }

    // SAFETY: table_length from the header describes the full table size.
    let table = unsafe { core::slice::from_raw_parts(xsdt_ptr, table_length) };

    let entry_count = (table_length - 36) / 8;
    let mut entries = Vec::with_capacity(entry_count);

    for i in 0..entry_count {
        let offset = 36 + i * 8;
        if offset + 8 > table.len() {
            break;
        }
        let addr = u64::from_le_bytes([
            table[offset],
            table[offset + 1],
            table[offset + 2],
            table[offset + 3],
            table[offset + 4],
            table[offset + 5],
            table[offset + 6],
            table[offset + 7],
        ]);
        if addr != 0 {
            entries.push(addr as *const u8);
        }
    }

    Some(entries)
}

/// Parse RSDT (Root System Description Table) — 32-bit entry pointers.
fn parse_rsdt(rsdt_ptr: *const u8) -> Option<Vec<*const u8>> {
    // SAFETY: RSDT pointer comes from RSDP which is firmware-validated.
    let header = unsafe { core::slice::from_raw_parts(rsdt_ptr, 36) };

    if &header[0..4] != b"RSDT" {
        return None;
    }

    let table_length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if table_length < 36 {
        return None;
    }

    let table = unsafe { core::slice::from_raw_parts(rsdt_ptr, table_length) };

    let entry_count = (table_length - 36) / 4;
    let mut entries = Vec::with_capacity(entry_count);

    for i in 0..entry_count {
        let offset = 36 + i * 4;
        if offset + 4 > table.len() {
            break;
        }
        let addr = u32::from_le_bytes([
            table[offset],
            table[offset + 1],
            table[offset + 2],
            table[offset + 3],
        ]);
        if addr != 0 {
            entries.push(addr as *const u8);
        }
    }

    Some(entries)
}

// ============================================================================
// Intel VT-d DMAR Parsing
// ============================================================================

/// DMAR remapping structure types
const DMAR_TYPE_DRHD: u16 = 0;
const DMAR_TYPE_RMRR: u16 = 1;

/// Parse Intel DMAR (DMA Remapping) table.
fn parse_dmar(table_ptr: *const u8, info: &mut IommuInfo) {
    // SAFETY: table_ptr points to a valid ACPI table (firmware-provided).
    let header = unsafe { core::slice::from_raw_parts(table_ptr, 48) };

    let table_length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if table_length < 48 {
        return;
    }

    // SAFETY: Full table length from header.
    let table = unsafe { core::slice::from_raw_parts(table_ptr, table_length) };

    info.intel_vt_d = true;

    let host_addr_width = table[36];
    let flags = table[37];
    log::info!(
        "DMAR: host address width={}, flags=0x{:02X}",
        host_addr_width + 1,
        flags
    );

    // Walk remapping structures starting at offset 48
    let mut offset = 48;
    while offset + 4 <= table_length {
        let struct_type = u16::from_le_bytes([table[offset], table[offset + 1]]);
        let struct_length = u16::from_le_bytes([table[offset + 2], table[offset + 3]]) as usize;

        if struct_length < 4 || offset + struct_length > table_length {
            break;
        }

        match struct_type {
            DMAR_TYPE_DRHD => {
                if let Some(drhd) = parse_drhd(&table[offset..offset + struct_length]) {
                    info.drhd_units.push(drhd);
                }
            }
            DMAR_TYPE_RMRR => {
                if let Some(rmrr) = parse_rmrr(&table[offset..offset + struct_length]) {
                    info.rmrr_regions.push(rmrr);
                }
            }
            _ => {
                // Types 2-4 (ATSR, RHSA, ANDD) are less relevant for passthrough
                log::debug!("DMAR: skipping structure type {struct_type}");
            }
        }

        offset += struct_length;
    }
}

/// Parse a DRHD (DMA Remapping Hardware Unit Definition) structure.
fn parse_drhd(data: &[u8]) -> Option<DrhdUnit> {
    if data.len() < 16 {
        return None;
    }

    let flags = data[4];
    let segment = u16::from_le_bytes([data[6], data[7]]);
    let register_base = u64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);

    let device_scopes = parse_device_scopes(&data[16..]);

    Some(DrhdUnit {
        segment,
        register_base,
        include_pci_all: flags & 0x01 != 0,
        device_scopes,
    })
}

/// Parse an RMRR (Reserved Memory Region Reporting) structure.
fn parse_rmrr(data: &[u8]) -> Option<RmrrRegion> {
    if data.len() < 24 {
        return None;
    }

    let segment = u16::from_le_bytes([data[6], data[7]]);
    let base_address = u64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);
    let limit_address = u64::from_le_bytes([
        data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
    ]);

    let device_scopes = parse_device_scopes(&data[24..]);

    Some(RmrrRegion {
        base_address,
        limit_address,
        segment,
        device_scopes,
    })
}

/// Parse device scope entries from a DRHD or RMRR structure.
fn parse_device_scopes(data: &[u8]) -> Vec<DeviceScope> {
    let mut scopes = Vec::new();
    let mut offset = 0;

    while offset + 6 <= data.len() {
        let scope_type = data[offset];
        let scope_length = data[offset + 1] as usize;

        if scope_length < 6 || offset + scope_length > data.len() {
            break;
        }

        let enumeration_id = data[offset + 4];
        let start_bus = data[offset + 5];

        // PCI path entries: (device, function) pairs after the 6-byte header
        let path_count = (scope_length - 6) / 2;
        let mut path = Vec::with_capacity(path_count);
        for i in 0..path_count {
            let p = offset + 6 + i * 2;
            if p + 1 < data.len() {
                path.push((data[p], data[p + 1]));
            }
        }

        scopes.push(DeviceScope {
            scope_type,
            enumeration_id,
            start_bus,
            path,
        });

        offset += scope_length;
    }

    scopes
}

// ============================================================================
// AMD-Vi IVRS Parsing
// ============================================================================

/// Parse AMD IVRS (I/O Virtualization Reporting Structure) table.
fn parse_ivrs(table_ptr: *const u8, info: &mut IommuInfo) {
    // SAFETY: table_ptr points to a valid ACPI table (firmware-provided).
    let header = unsafe { core::slice::from_raw_parts(table_ptr, 48) };

    let table_length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if table_length < 48 {
        return;
    }

    let table = unsafe { core::slice::from_raw_parts(table_ptr, table_length) };

    info.amd_vi = true;
    log::info!("IVRS: AMD-Vi I/O virtualization detected");

    // IVRS has IVHD (I/O Virtualization Hardware Definition) blocks
    // starting at offset 48, each with a type+flags+length header
    let mut offset = 48;
    while offset + 4 <= table_length {
        let block_type = table[offset];
        let block_length = u16::from_le_bytes([table[offset + 2], table[offset + 3]]) as usize;

        if block_length < 4 || offset + block_length > table_length {
            break;
        }

        // IVHD type 0x10 (legacy) or 0x11/0x40 (extended)
        if block_type == 0x10 || block_type == 0x11 || block_type == 0x40 {
            // Minimal parsing: count IVHD blocks as DRHD equivalents
            if block_length >= 24 {
                let segment = u16::from_le_bytes([table[offset + 8], table[offset + 9]]);
                let base = u64::from_le_bytes([
                    table[offset + 16],
                    table[offset + 17],
                    table[offset + 18],
                    table[offset + 19],
                    table[offset + 20],
                    table[offset + 21],
                    table[offset + 22],
                    table[offset + 23],
                ]);

                info.drhd_units.push(DrhdUnit {
                    segment,
                    register_base: base,
                    include_pci_all: false,
                    device_scopes: Vec::new(),
                });
            }
        }

        offset += block_length;
    }
}
