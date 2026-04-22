#![no_main]
#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use core::time::Duration;

use uefi::{
    prelude::*,
    proto::pci::{root_bridge::PciRootBridgeIo, PciIoAddress},
    runtime,
    table::cfg::ConfigTableEntry,
};

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI");

    loop {
        show_menu();
        match wait_for_key() {
            '1' => show_boot_variables(),
            '2' => show_pci_devices(),
            '3' => show_memory_map(),
            '4' => show_system_info(),
            '5' => show_firmware_info(),
            'q' | 'Q' | '\x1B' => {
                println("Returning to LamBoot...");
                return Status::SUCCESS;
            }
            _ => {}
        }
        println("");
        println("Press any key to continue...");
        wait_for_key();
    }
}

fn show_menu() {
    clear_screen();
    println("========================================");
    println("  LamBoot Diagnostic Shell v0.1.0");
    println("========================================");
    println("");
    println("  1) Boot Variables (LamBoot state)");
    println("  2) PCI Device Inventory");
    println("  3) Memory Map");
    println("  4) System Information (SMBIOS)");
    println("  5) Firmware Information");
    println("");
    println("  Q) Return to LamBoot");
    println("");
    println("Select: ");
}

// ============================================================================
// 1. Boot Variables
// ============================================================================

fn show_boot_variables() {
    clear_screen();
    println("=== LamBoot Boot Variables ===");
    println("");

    let lamboot_guid = uefi::guid!("4c414d42-4f4f-5400-0000-000000000001");
    let vendor = runtime::VariableVendor(lamboot_guid);

    // LamBootState
    let mut buf = [0u8; 1];
    match runtime::get_variable(cstr16!("LamBootState"), &vendor, &mut buf) {
        Ok((data, _)) if !data.is_empty() => {
            let state = match data[0] {
                0 => "Fresh",
                1 => "Booting",
                2 => "BootedOK",
                3 => "CrashLoop",
                _ => "Unknown",
            };
            println(&format!("  LamBootState:      {state} ({})", data[0]));
        }
        _ => println("  LamBootState:      (not set)"),
    }

    // LamBootCrashCount
    match runtime::get_variable(cstr16!("LamBootCrashCount"), &vendor, &mut buf) {
        Ok((data, _)) if !data.is_empty() => {
            println(&format!("  LamBootCrashCount: {}", data[0]));
        }
        _ => println("  LamBootCrashCount: (not set)"),
    }

    // LamBootLastEntry
    let mut entry_buf = [0u8; 128];
    match runtime::get_variable(cstr16!("LamBootLastEntry"), &vendor, &mut entry_buf) {
        Ok((data, _)) if !data.is_empty() => {
            if let Ok(s) = core::str::from_utf8(data) {
                println(&format!(
                    "  LamBootLastEntry:  {}",
                    s.trim_end_matches('\0')
                ));
            }
        }
        _ => println("  LamBootLastEntry:  (not set)"),
    }

    // LamBootTimestamp
    let mut ts_buf = [0u8; 8];
    match runtime::get_variable(cstr16!("LamBootTimestamp"), &vendor, &mut ts_buf) {
        Ok((data, _)) if data.len() >= 7 => {
            let year = u16::from_le_bytes([data[0], data[1]]);
            println(&format!(
                "  LamBootTimestamp:  {:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                year, data[2], data[3], data[4], data[5], data[6]
            ));
        }
        _ => println("  LamBootTimestamp:  (not set)"),
    }

    // Boot Loader Interface
    println("");
    println("=== Boot Loader Interface ===");
    let loader_guid = uefi::guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f");
    let loader_vendor = runtime::VariableVendor(loader_guid);

    let mut info_buf = [0u8; 64];
    match runtime::get_variable(cstr16!("LoaderInfo"), &loader_vendor, &mut info_buf) {
        Ok((data, _)) if data.len() >= 2 => {
            // UTF-16 string
            let chars: Vec<char> = data
                .chunks(2)
                .filter_map(|c| {
                    if c.len() == 2 {
                        let ch = u16::from_le_bytes([c[0], c[1]]);
                        if ch != 0 {
                            char::from_u32(ch as u32)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();
            let s: String = chars.into_iter().collect();
            println(&format!("  LoaderInfo:        {s}"));
        }
        _ => println("  LoaderInfo:        (not set)"),
    }
}

// ============================================================================
// 2. PCI Devices
// ============================================================================

fn show_pci_devices() {
    clear_screen();
    println("=== PCI Device Inventory ===");
    println("");

    let Ok(handles) = uefi::boot::find_handles::<PciRootBridgeIo>() else {
        println("No PCI Root Bridge found.");
        return;
    };

    println(&format!(
        "{:<8} {:<6} {:<6} {}",
        "BDF", "VID", "DID", "Class"
    ));
    println("--------------------------------------");

    let mut count = 0;
    for handle in handles {
        // SAFETY: GetProtocol avoids disconnecting OVMF's PCI bus driver
        // (open_protocol_exclusive would disconnect it, breaking PCI access)
        let Ok(mut pci_root) = (unsafe {
            uefi::boot::open_protocol::<PciRootBridgeIo>(
                uefi::boot::OpenProtocolParams {
                    handle,
                    agent: uefi::boot::image_handle(),
                    controller: None,
                },
                uefi::boot::OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            continue;
        };

        for bus in 0..=255u8 {
            // Quick check: if device 0 on this bus doesn't exist, skip the bus
            let bus_check = PciIoAddress::new(bus, 0, 0);
            if pci_root
                .pci()
                .read_one::<u32>(bus_check)
                .map_or(true, |v| v & 0xFFFF == 0xFFFF)
            {
                continue;
            }

            for device in 0..32u8 {
                for function in 0..8u8 {
                    let addr = PciIoAddress::new(bus, device, function);
                    let Ok(reg0) = pci_root.pci().read_one::<u32>(addr) else {
                        continue;
                    };

                    let vid = (reg0 & 0xFFFF) as u16;
                    if vid == 0xFFFF {
                        if function == 0 {
                            break;
                        }
                        continue;
                    }
                    let did = ((reg0 >> 16) & 0xFFFF) as u16;

                    let addr2 = PciIoAddress::new(bus, device, function).with_register(8);
                    let reg2 = pci_root.pci().read_one::<u32>(addr2).unwrap_or(0);
                    let class = ((reg2 >> 24) & 0xFF) as u8;
                    let subclass = ((reg2 >> 16) & 0xFF) as u8;

                    let class_name = pci_class_name(class, subclass);
                    let marker = if vid == 0x1AF4 { " [virtio]" } else { "" };

                    println(&format!(
                        "{bus:02X}:{device:02X}.{function}  {vid:04X}  {did:04X}  {class_name}{marker}"
                    ));
                    count += 1;

                    if function == 0 {
                        let addr3 = PciIoAddress::new(bus, device, 0).with_register(0x0C);
                        let reg3 = pci_root.pci().read_one::<u32>(addr3).unwrap_or(0);
                        if ((reg3 >> 16) & 0x80) == 0 {
                            break;
                        }
                    }
                }
            }
        }
    }

    println("");
    println(&format!("Total: {count} devices"));
}

fn pci_class_name(class: u8, subclass: u8) -> &'static str {
    match (class, subclass) {
        (0x01, 0x00) => "SCSI",
        (0x01, 0x06) => "SATA",
        (0x01, 0x08) => "NVMe",
        (0x01, _) => "Storage",
        (0x02, _) => "Network",
        (0x03, 0x00) => "VGA",
        (0x03, _) => "Display",
        (0x04, _) => "Multimedia",
        (0x06, 0x00) => "Host Bridge",
        (0x06, 0x01) => "ISA Bridge",
        (0x06, 0x04) => "PCI Bridge",
        (0x06, _) => "Bridge",
        (0x08, _) => "System",
        (0x0C, 0x03) => "USB",
        (0x0C, _) => "Serial Bus",
        _ => "Other",
    }
}

// ============================================================================
// 3. Memory Map
// ============================================================================

fn show_memory_map() {
    clear_screen();
    println("=== UEFI Memory Map ===");
    println("");

    use uefi::mem::memory_map::MemoryMap;
    let mmap = uefi::boot::memory_map(uefi::boot::MemoryType::LOADER_DATA);
    match mmap {
        Ok(map) => {
            let mut total: u64 = 0;
            let mut conventional: u64 = 0;
            let mut regions = 0u32;

            for desc in map.entries() {
                let size = desc.page_count * 4096;
                total += size;
                if desc.ty == uefi::boot::MemoryType::CONVENTIONAL {
                    conventional += size;
                    regions += 1;
                }
            }

            println(&format!(
                "  Total memory:       {} MB",
                total / (1024 * 1024)
            ));
            println(&format!(
                "  Conventional:       {} MB ({regions} regions)",
                conventional / (1024 * 1024)
            ));
            println(&format!(
                "  Used by firmware:   {} MB",
                (total - conventional) / (1024 * 1024)
            ));
        }
        Err(e) => println(&format!("  Failed to get memory map: {e:?}")),
    }
}

// ============================================================================
// 4. System Info (SMBIOS)
// ============================================================================

fn show_system_info() {
    clear_screen();
    println("=== System Information (SMBIOS) ===");
    println("");

    let smbios_addr = uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == ConfigTableEntry::SMBIOS3_GUID {
                return Some((entry.address, true));
            }
            if entry.guid == ConfigTableEntry::SMBIOS_GUID {
                return Some((entry.address, false));
            }
        }
        None
    });

    let Some((table_ptr, is_v3)) = smbios_addr else {
        println("  SMBIOS table: not found");
        return;
    };

    println(&format!(
        "  SMBIOS table: found ({})",
        if is_v3 { "3.x 64-bit" } else { "2.x" }
    ));
    println("");

    // Parse entry point to find structure table
    // SAFETY: table_ptr is from the firmware's configuration table
    let ep = table_ptr.cast::<u8>();
    let (struct_addr, struct_len) = unsafe {
        if is_v3 {
            // SMBIOS 3.x: struct table address at offset 16 (8 bytes)
            let addr = core::ptr::read_unaligned(ep.add(16).cast::<u64>()) as usize;
            let len = core::ptr::read_unaligned(ep.add(12).cast::<u32>()) as usize;
            (addr, len)
        } else {
            // SMBIOS 2.x: struct table address at offset 24 (4 bytes)
            let addr = core::ptr::read_unaligned(ep.add(24).cast::<u32>()) as usize;
            let len = core::ptr::read_unaligned(ep.add(22).cast::<u16>()) as usize;
            (addr, len)
        }
    };

    if struct_addr == 0 || struct_len == 0 {
        println("  Structure table: invalid");
        return;
    }

    // Walk SMBIOS structures looking for Type 1 (System) and Type 11 (OEM Strings)
    // SAFETY: struct_addr points to the SMBIOS structure table in firmware memory
    let table = unsafe { core::slice::from_raw_parts(struct_addr as *const u8, struct_len) };
    let mut offset = 0;

    while offset + 4 <= struct_len {
        let stype = table[offset];
        let slen = table[offset + 1] as usize;

        if slen < 4 || offset + slen > struct_len {
            break;
        }

        // Collect strings (null-terminated after the structure)
        let strings_start = offset + slen;
        let mut strings: Vec<&str> = Vec::new();
        let mut pos = strings_start;
        loop {
            if pos >= struct_len {
                break;
            }
            if table[pos] == 0 {
                pos += 1;
                break;
            }
            let str_start = pos;
            while pos < struct_len && table[pos] != 0 {
                pos += 1;
            }
            if let Ok(s) = core::str::from_utf8(&table[str_start..pos]) {
                strings.push(s);
            }
            if pos < struct_len {
                pos += 1; // skip null
            }
        }

        match stype {
            0 if slen >= 18 => {
                // BIOS Information
                let str_idx = |i: usize| -> &str {
                    if i > 0 && i <= strings.len() {
                        strings[i - 1]
                    } else {
                        "(none)"
                    }
                };
                println("  --- BIOS ---");
                println(&format!(
                    "  Vendor:    {}",
                    str_idx(table[offset + 4] as usize)
                ));
                println(&format!(
                    "  Version:   {}",
                    str_idx(table[offset + 5] as usize)
                ));
                println(&format!(
                    "  Date:      {}",
                    str_idx(table[offset + 8] as usize)
                ));
                println("");
            }
            1 if slen >= 27 => {
                // System Information
                let str_idx = |i: usize| -> &str {
                    if i > 0 && i <= strings.len() {
                        strings[i - 1]
                    } else {
                        "(none)"
                    }
                };
                println("  --- System ---");
                println(&format!(
                    "  Manufacturer: {}",
                    str_idx(table[offset + 4] as usize)
                ));
                println(&format!(
                    "  Product:      {}",
                    str_idx(table[offset + 5] as usize)
                ));
                println(&format!(
                    "  Version:      {}",
                    str_idx(table[offset + 6] as usize)
                ));
                println(&format!(
                    "  Serial:       {}",
                    str_idx(table[offset + 7] as usize)
                ));
                println("");
            }
            11 => {
                // OEM Strings
                let count = if slen >= 5 {
                    table[offset + 4] as usize
                } else {
                    0
                };
                if count > 0 {
                    println("  --- OEM Strings ---");
                    for (i, s) in strings.iter().enumerate().take(count) {
                        println(&format!("  [{i}] {s}"));
                    }
                    println("");
                }
            }
            127 => break, // End-of-Table
            _ => {}
        }

        offset = pos;
    }
}

// ============================================================================
// 5. Firmware Info
// ============================================================================

fn show_firmware_info() {
    clear_screen();
    println("=== Firmware Information ===");
    println("");

    // Firmware vendor + revision from system table
    let fw_vendor = uefi::system::firmware_vendor();
    let fw_rev = uefi::system::firmware_revision();
    println(&format!("  Firmware Vendor:   {fw_vendor}"));
    println(&format!("  Firmware Revision: {fw_rev:#010X}"));
    println(&format!(
        "  UEFI Revision:     {}",
        uefi::system::uefi_revision()
    ));

    // Architecture
    #[cfg(target_arch = "x86_64")]
    println("  Architecture:      x86_64");
    #[cfg(target_arch = "aarch64")]
    println("  Architecture:      aarch64");

    // Secure Boot state
    println("");
    let sb_var = cstr16!("SecureBoot");
    let efi_global = runtime::VariableVendor(uefi::guid!("8be4df61-93ca-11d2-aa0d-00e098032b8c"));
    let mut buf = [0u8; 1];
    match runtime::get_variable(sb_var, &efi_global, &mut buf) {
        Ok((data, _)) if !data.is_empty() => {
            let state = if data[0] == 1 { "ENABLED" } else { "disabled" };
            println(&format!("  Secure Boot:       {state}"));
        }
        _ => println("  Secure Boot:       unknown"),
    }

    // Setup mode
    let setup_var = cstr16!("SetupMode");
    match runtime::get_variable(setup_var, &efi_global, &mut buf) {
        Ok((data, _)) if !data.is_empty() => {
            let mode = if data[0] == 1 {
                "setup mode (keys not enrolled)"
            } else {
                "user mode"
            };
            println(&format!("  Setup Mode:        {mode}"));
        }
        _ => {}
    }

    // Config tables
    println("");
    println("  --- Configuration Tables ---");
    uefi::system::with_config_table(|entries| {
        println(&format!("  Total entries: {}", entries.len()));
        for entry in entries {
            let name = if entry.guid == ConfigTableEntry::ACPI2_GUID {
                "ACPI 2.0 RSDP"
            } else if entry.guid == ConfigTableEntry::ACPI_GUID {
                "ACPI 1.0 RSDP"
            } else if entry.guid == ConfigTableEntry::SMBIOS_GUID {
                "SMBIOS 2.x"
            } else if entry.guid == ConfigTableEntry::SMBIOS3_GUID {
                "SMBIOS 3.x"
            } else {
                continue;
            };
            println(&format!("  {name}"));
        }
    });
}

// ============================================================================
// Console Helpers
// ============================================================================

fn println(msg: &str) {
    uefi::system::with_stdout(|stdout| {
        let buf = uefi::CString16::try_from(msg).unwrap_or_default();
        let _ = stdout.output_string(&buf);
        let _ = stdout.output_string(cstr16!("\r\n"));
    });
}

fn clear_screen() {
    uefi::system::with_stdout(|stdout| {
        let _ = stdout.clear();
    });
}

fn wait_for_key() -> char {
    loop {
        let result = uefi::system::with_stdin(|stdin| stdin.read_key().ok().flatten());
        if let Some(key) = result {
            match key {
                uefi::proto::console::text::Key::Printable(c) => return c.into(),
                uefi::proto::console::text::Key::Special(sc) => {
                    use uefi::proto::console::text::ScanCode;
                    if sc == ScanCode::ESCAPE {
                        return '\x1B';
                    }
                }
            }
        }
        uefi::boot::stall(Duration::from_millis(10));
    }
}
