#![no_main]
#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec::Vec};

use log::info;
use uefi::{
    prelude::*,
    proto::pci::{root_bridge::PciRootBridgeIo, PciIoAddress},
};

const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI");

    info!("PCI Device Inventory v0.1.0");
    info!("===========================");
    info!("");

    let devices = scan_pci_devices();

    if devices.is_empty() {
        info!("No PCI devices found.");
    } else {
        info!("Found {} PCI device(s):", devices.len());
        info!("");
        info!(
            "{:<8} {:<6} {:<6} {:<24} {}",
            "BDF", "VID", "DID", "Class", "Type"
        );
        info!("{}", "-".repeat(60));

        for dev in &devices {
            let dev_type = if dev.vendor_id == VIRTIO_VENDOR_ID {
                "VirtIO (emulated)"
            } else if dev.vendor_id == 0x8086 {
                "Intel"
            } else if dev.vendor_id == 0x1022 {
                "AMD"
            } else if dev.vendor_id == 0x10DE {
                "NVIDIA (passthrough?)"
            } else if dev.vendor_id == 0x1002 {
                "AMD GPU (passthrough?)"
            } else {
                "Hardware"
            };

            info!(
                "{:<8} {:04X}  {:04X}  {:<24} {}",
                dev.bdf_string(),
                dev.vendor_id,
                dev.device_id,
                dev.class_string(),
                dev_type
            );
        }

        // Summary
        info!("");
        let virtio_count = devices
            .iter()
            .filter(|d| d.vendor_id == VIRTIO_VENDOR_ID)
            .count();
        let passthrough_count = devices
            .iter()
            .filter(|d| {
                d.vendor_id != VIRTIO_VENDOR_ID && d.vendor_id != 0x8086 && d.vendor_id != 0x1B36
            })
            .count();
        info!("VirtIO emulated: {virtio_count}");
        if passthrough_count > 0 {
            info!("Possible passthrough: {passthrough_count}");
        }
    }

    // Write report to ESP
    write_report(&devices);

    info!("");
    info!("Press any key to return to LamBoot...");
    uefi::system::with_stdin(|stdin| loop {
        if stdin.read_key().ok().flatten().is_some() {
            break;
        }
        uefi::boot::stall(core::time::Duration::from_millis(10));
    });

    Status::SUCCESS
}

struct PciDevice {
    bus: u8,
    device: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    class_code: u8,
    subclass: u8,
    prog_if: u8,
}

impl PciDevice {
    fn bdf_string(&self) -> String {
        format!("{:02X}:{:02X}.{}", self.bus, self.device, self.function)
    }

    fn class_string(&self) -> &'static str {
        match (self.class_code, self.subclass) {
            (0x00, _) => "Unclassified",
            (0x01, 0x00) => "SCSI Controller",
            (0x01, 0x01) => "IDE Controller",
            (0x01, 0x06) => "SATA Controller",
            (0x01, 0x08) => "NVMe Controller",
            (0x01, _) => "Mass Storage",
            (0x02, 0x00) => "Ethernet Controller",
            (0x02, _) => "Network Controller",
            (0x03, 0x00) => "VGA Controller",
            (0x03, 0x02) => "3D Controller",
            (0x03, _) => "Display Controller",
            (0x04, 0x00) => "Video Device",
            (0x04, 0x01 | 0x03) => "Audio Device",
            (0x04, _) => "Multimedia",
            (0x05, _) => "Memory Controller",
            (0x06, 0x00) => "Host Bridge",
            (0x06, 0x01) => "ISA Bridge",
            (0x06, 0x04) => "PCI-PCI Bridge",
            (0x06, _) => "Bridge Device",
            (0x07, _) => "Communication",
            (0x08, _) => "System Peripheral",
            (0x09, _) => "Input Device",
            (0x0C, 0x03) => "USB Controller",
            (0x0C, _) => "Serial Bus",
            (0x0D, _) => "Wireless",
            (0x10, _) => "Encryption",
            (0x11, _) => "Signal Processing",
            _ => "Other",
        }
    }
}

fn scan_pci_devices() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    let Ok(handles) = uefi::boot::find_handles::<PciRootBridgeIo>() else {
        info!("No PCI Root Bridge found");
        return devices;
    };

    for handle in handles {
        // SAFETY: GetProtocol avoids disconnecting OVMF's PCI bus driver
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
            // Skip empty buses
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

                    // Read vendor/device ID (register 0)
                    let Ok(reg0) = pci_root.pci().read_one::<u32>(addr) else {
                        continue;
                    };

                    let vendor_id = (reg0 & 0xFFFF) as u16;
                    let device_id = ((reg0 >> 16) & 0xFFFF) as u16;

                    // 0xFFFF = no device present
                    if vendor_id == 0xFFFF {
                        if function == 0 {
                            break; // No device at this slot, skip remaining functions
                        }
                        continue;
                    }

                    // Read class code (register 2)
                    let addr2 = PciIoAddress::new(bus, device, function).with_register(8);
                    let reg2 = pci_root.pci().read_one::<u32>(addr2).unwrap_or(0);
                    let class_code = ((reg2 >> 24) & 0xFF) as u8;
                    let subclass = ((reg2 >> 16) & 0xFF) as u8;
                    let prog_if = ((reg2 >> 8) & 0xFF) as u8;

                    devices.push(PciDevice {
                        bus,
                        device,
                        function,
                        vendor_id,
                        device_id,
                        class_code,
                        subclass,
                        prog_if,
                    });

                    // Check if multi-function device (header type bit 7)
                    if function == 0 {
                        let addr3 = PciIoAddress::new(bus, device, 0).with_register(0x0C);
                        let reg3 = pci_root.pci().read_one::<u32>(addr3).unwrap_or(0);
                        let header_type = ((reg3 >> 16) & 0xFF) as u8;
                        if header_type & 0x80 == 0 {
                            break; // Single-function device, skip functions 1-7
                        }
                    }
                }
            }
        }
    }

    devices
}

fn write_report(devices: &[PciDevice]) {
    // Build JSON report
    let mut json = String::from("{\n  \"devices\": [\n");
    for (i, dev) in devices.iter().enumerate() {
        if i > 0 {
            json.push_str(",\n");
        }
        use core::fmt::Write;
        let dev_type = if dev.vendor_id == VIRTIO_VENDOR_ID {
            "virtio"
        } else {
            "hardware"
        };
        let _ = write!(
            json,
            "    {{\"bdf\":\"{}\",\"vendor\":\"0x{:04X}\",\"device\":\"0x{:04X}\",\"class\":\"{}\",\"prog_if\":\"0x{:02X}\",\"type\":\"{}\"}}",
            dev.bdf_string(), dev.vendor_id, dev.device_id, dev.class_string(), dev.prog_if, dev_type
        );
    }
    json.push_str("\n  ]\n}\n");

    // Try to write to ESP
    let image = uefi::boot::image_handle();
    if let Ok(loaded) =
        uefi::boot::open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(image)
    {
        if let Some(device) = loaded.device() {
            if let Ok(mut fs) = uefi::boot::open_protocol_exclusive::<
                uefi::proto::media::fs::SimpleFileSystem,
            >(device)
            {
                if let Ok(mut root) = fs.open_volume() {
                    use uefi::proto::media::file::{File, FileAttribute, FileMode};
                    let path =
                        uefi::CString16::try_from("\\EFI\\LamBoot\\reports\\pci-inventory.json");
                    if let Ok(path) = path {
                        if let Ok(file) =
                            root.open(&path, FileMode::CreateReadWrite, FileAttribute::empty())
                        {
                            if let Some(mut regular) = file.into_regular_file() {
                                let _ = regular.write(json.as_bytes());
                                info!(
                                    "Report written to \\EFI\\LamBoot\\reports\\pci-inventory.json"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
