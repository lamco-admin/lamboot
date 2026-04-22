# LamBoot Diagnostic Modules

**Version:** 0.8.3
**Updated:** 2026-04-21

---

## Overview

LamBoot can load standalone UEFI diagnostic applications (modules) from `\EFI\LamBoot\modules\` on the ESP. Modules appear in the right column of the boot menu under "Tools & Actions."

Modules are separate EFI binaries — they run independently from LamBoot and return to the LamBoot menu when they exit. You can run multiple modules in succession without rebooting.

To disable module discovery entirely, set `modules.enabled = false` in `policy.toml`.

---

## Available Modules

| Module | Filename | Description | Status |
|--------|----------|-------------|--------|
| Diagnostic Shell | `diag-shell.efi` | Interactive 5-screen diagnostic tool | Functional |
| PCI Device Inventory | `pci-inventory.efi` | PCI bus scan with VirtIO/passthrough detection | Functional |
| Quick Memory Test | `mem-quick.efi` | Memory stats and basic integrity tests | Functional |
| NVMe Health Check | `nvme-diag.efi` | NVMe S.M.A.R.T. diagnostics | Stub (not yet implemented) |

---

## Diagnostic Shell

The diagnostic shell provides an interactive text-mode menu with five information screens. Navigate by pressing the number key for each option.

### Menu

```
========================================
  LamBoot Diagnostic Shell v0.1.0
========================================

  1) Boot Variables (LamBoot state)
  2) PCI Device Inventory
  3) Memory Map
  4) System Information (SMBIOS)
  5) Firmware Information

  Q) Return to LamBoot
```

### Screen 1: Boot Variables

Displays LamBoot's NVRAM state:

- **LamBootState**: Current boot state (Fresh, Booting, BootedOK, CrashLoop)
- **LamBootCrashCount**: Number of consecutive failed boots
- **LamBootLastEntry**: ID of the last booted entry
- **LamBootTimestamp**: Time of last boot attempt
- **LoaderInfo**: Boot Loader Interface identification string

### Screen 2: PCI Device Inventory

Scans the PCI bus and displays all devices in a table:

```
BDF      VID    DID    Class
--------------------------------------
00:00.0  8086   29C0   Host Bridge
00:01.0  1AF4   1050   VGA Controller    [virtio]
00:1F.2  8086   2922   SATA Controller
```

Each device shows bus/device/function address, vendor/device IDs, device class, and special markers for VirtIO emulated devices.

### Screen 3: Memory Map

Reads the UEFI memory map and reports:

- **Total memory**: All memory reported by firmware (MB)
- **Conventional**: Free memory available to the OS (MB)
- **Used by firmware**: Memory reserved by UEFI runtime and ACPI (MB)

### Screen 4: System Information (SMBIOS)

Parses SMBIOS tables from firmware memory:

- **BIOS Information** (Type 0): Vendor, version, release date
- **System Information** (Type 1): Manufacturer, product name, version, serial number
- **OEM Strings** (Type 11): Host-injected strings (includes `lamboot.vmid=` etc.)

### Screen 5: Firmware Information

Displays UEFI firmware details:

- **Firmware Vendor**: e.g., "EDK II" (OVMF)
- **Firmware Revision**: Build number
- **UEFI Revision**: Specification version (e.g., 2.7.0)
- **Architecture**: x86_64 or aarch64
- **Secure Boot**: Enabled or disabled
- **Setup Mode**: User mode or setup mode (keys not enrolled)
- **Configuration Tables**: ACPI, SMBIOS entries found in firmware

---

## PCI Device Inventory

A dedicated PCI scanner that produces both on-screen output and a JSON report.

### On-Screen Output

Displays a table of all PCI devices with:
- BDF (Bus:Device.Function) address
- Vendor and device IDs
- Device class name
- Device type hints: VirtIO (emulated), Intel, AMD, NVIDIA (passthrough?), AMD GPU (passthrough?)
- Summary: VirtIO emulated count, possible passthrough count

### JSON Report

Writes `/boot/efi/EFI/LamBoot/reports/pci-inventory.json`:

```json
{
  "devices": [
    {"bdf":"00:00.0","vendor":"0x8086","device":"0x29C0","class":"Host Bridge","prog_if":"0x00","type":"hardware"},
    {"bdf":"00:01.0","vendor":"0x1AF4","device":"0x1050","class":"VGA Controller","prog_if":"0x00","type":"virtio"}
  ]
}
```

Each device includes the `prog_if` (Programming Interface) byte for detailed class identification.

---

## Quick Memory Test

Reads the UEFI memory map and runs two basic integrity tests:

1. **Walking Ones Test**: Writes and verifies each bit position in a 4 KB buffer
2. **Address Pattern Test**: Writes address-dependent values and verifies

Reports total memory, conventional (free) memory, and test results.

This is a fast test (seconds) designed for quick sanity checks, not exhaustive memory testing.

---

## NVMe Health Check

**Status: Planned — not yet implemented.**

The NVMe diagnostic module is a stub. When implemented, it will read NVMe S.M.A.R.T. data (temperature, wear level, error counts) via the NVMe protocol.

---

## Module Manifest

The file `\EFI\LamBoot\modules\manifest.toml` maps module filenames to friendly display names:

```toml
[modules.diag-shell]
name = "Diagnostic Shell"

[modules.pci-inventory]
name = "PCI Device Inventory"
```

Without a manifest, modules appear as "Tool: _filename_" in the menu. The section ID must match the `.efi` filename without extension.

---

## Adding Custom Modules

You can add your own diagnostic modules to `\EFI\LamBoot\modules\`:

### Requirements

- Standalone UEFI application (PE binary for the target architecture)
- Returns `Status::SUCCESS` on exit (LamBoot returns to the menu on success)
- Uses text-mode ConOut for output (LamBoot reconnects the GraphicsConsole driver before starting modules)

### Building a Custom Module (Rust)

A minimal module:

```rust
#![no_main]
#![no_std]

extern crate alloc;

use uefi::prelude::*;

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI");

    // Your diagnostic code here
    log::info!("My custom diagnostic module");

    // Wait for key press
    uefi::system::with_stdin(|stdin| loop {
        if stdin.read_key().ok().flatten().is_some() { break; }
        uefi::boot::stall(core::time::Duration::from_millis(10));
    });

    Status::SUCCESS
}
```

Add a Cargo.toml with the same dependencies as the existing modules and build with `cargo build --target x86_64-unknown-uefi --release`.

### Deployment

1. Copy the `.efi` file to `\EFI\LamBoot\modules\` on the ESP
2. Optionally add a manifest entry for a friendly name
3. Reboot — the module appears automatically in the right column

---

## See Also

- [User Guide](USER-GUIDE.md) — how to use modules from the boot menu
- [Architecture Guide](ARCHITECTURE.md) — module loading internals
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — module hang issues
