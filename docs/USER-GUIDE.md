# LamBoot User Guide

**Version:** 0.8.3
**Updated:** 2026-04-21

---

## What is LamBoot?

LamBoot is a memory-safe UEFI bootloader written in Rust. It discovers and boots Linux kernels, Unified Kernel Images (UKIs), and other EFI applications from BLS (Boot Loader Specification) entries on the ESP. It provides a graphical boot menu, crash loop recovery, TPM measured boot, and built-in diagnostic tools — all in a single compact binary.

LamBoot is designed for both bare-metal Linux systems and Proxmox/QEMU/KVM virtual machine fleets.

### Key Features

- **Graphical two-column boot menu** with mouse and keyboard support
- **BLS Type 1 entry discovery** with UAPI.10 version sorting and boot counting
- **Unified Kernel Image (UKI) support** with PE section metadata extraction
- **Preflight validation** — advisory checks on every entry before you boot (see [Preflight Validation](#preflight-validation))
- **Crash loop detection** with automatic fallback and VM snapshot awareness
- **TPM 2.0 measured boot** — kernel, config, and command line measured into PCRs
- **Persistent boot logging** — write-through during early boot, readable after the OS starts
- **Loadable diagnostic modules** — PCI inventory, memory test, interactive diagnostic shell
- **Filesystem drivers** — loads ext4/btrfs drivers for non-FAT /boot partitions
- **Proxmox integration** — VMID display, hookscript monitoring, fleet operations
- **Secure Boot compatible** — ShimLock verification, SBAT, signing pipeline

### Boot Architecture (10 Phases)

LamBoot executes a structured 10-phase boot sequence. For the full technical description, see the [Architecture Guide](ARCHITECTURE.md). In summary:

1. **Health assessment** — check crash counter, detect VM snapshot restore
2. **Security & hardware detection** — Secure Boot, TPM, SMBIOS, hypervisor, IOMMU
3. **Mount ESP** — locate and mount the EFI System Partition
4. **Load policy** — read `policy.toml` configuration
5. **Load drivers** — ext4, btrfs, and other filesystem drivers
6. **Enumerate volumes** — discover additional partitions (XBOOTLDR, etc.)
7. **Discover boot entries** — BLS entries, UKIs, Windows, other loaders, diagnostic tools
8. **Preflight validation** — check each entry for potential issues
9. **Interactive menu** — display the graphical boot menu, wait for selection
10. **Boot** — record selection, measure into TPM, load kernel and initrd, hand off

---

## Requirements

- **Firmware**: UEFI 2.0+ (OVMF for VMs, any UEFI firmware for bare metal)
- **Architecture**: x86_64 (primary), aarch64 (secondary)
- **ESP**: EFI System Partition (FAT32), typically mounted at `/boot/efi`
- **Guest OS**: Any Linux distribution with UEFI boot support. LamBoot uses the Boot Loader Specification (BLS) for entry discovery, which is supported by systemd-based distributions natively and can be set up on any Linux system.
- **Installer requirements**: bash 4.0+, GNU coreutils or uutils, util-linux, efibootmgr

LamBoot has been tested on Fedora 43, Debian 13 (forky/sid), Ubuntu 24.04, Arch Linux, and openSUSE Tumbleweed. Any UEFI-capable Linux distribution should work — the installer auto-detects the distro and adapts its behavior (BLS entry generation, driver selection, initrd paths).

---

## Quick Start

### Install from Release

Download the latest release archive and run the installer:

```bash
tar xf lamboot-0.2.0-x86_64.tar.gz
cd lamboot-0.2.0

# Install to the local system (requires root)
sudo ./lamboot-install

# Verify
sudo efibootmgr | grep LamBoot
```

### Build from Source

If building from source:

```bash
# Requires Rust nightly toolchain
rustup default nightly
rustup target add x86_64-unknown-uefi

# Build all components
./build.sh

# Install from the build output
sudo tools/lamboot-install

# Or update an existing installation
sudo tools/lamboot-install --update
```

### First Boot

1. Reboot the system
2. LamBoot appears if it's in the UEFI boot order (use `--set-default` to make it first)
3. The boot menu shows discovered kernels on the left, tools on the right
4. After the auto-boot timeout (default: 5 seconds), the first kernel boots automatically

---

## The Boot Menu

LamBoot uses a two-column graphical menu at 1024x768 resolution:

### Header

- **Left**: Lamco sheep logo, "LamBoot" title, version (e.g., `v0.2.0 (x86_64)`)
- **Right**: System identity information (when available):
  - **VM ID**: Shown as `VM 201` when configured via SMBIOS OEM strings (see [Configuration Guide](CONFIGURATION-GUIDE.md)). On Proxmox, this is auto-injected by the hookscript. On other hypervisors, set via QEMU `-smbios type=11,value=lamboot.vmid=201` or equivalent.
  - **Hypervisor**: Auto-detected via CPUID (KVM, Hyper-V, VMware, Xen, etc.). On bare metal, this line is absent.
  - **Build info**: Secure Boot state, loaded filesystem driver count

### Left Column — Boot Entries

Linux kernels, UKIs, and EFI loaders discovered from BLS entries on the ESP. Each entry shows:

- **Type badge**: `Linux`, `EFI`, `Win`
- **Name**: Entry title from BLS `title` field (e.g., "Fedora Linux 43 (6.19.9)")
- **Subtitle**: Entry type ("Linux kernel + initrd", "Unified Kernel Image", "EFI bootloader")
- **Preflight indicator**: `!` (warning) or `X` (error) if validation found issues

### Right Column — Tools & Actions

- **Diagnostic modules**: Modules deployed to `\EFI\LamBoot\modules\` (diag-shell, pci-inventory, etc.)
- **System actions**: "Firmware Setup [F2]" and "Reboot [F12]"

### Footer

- **Left**: Status message (auto-boot countdown, preflight warnings, navigation hints)
- **Right**: Keyboard shortcut reference

---

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Up / Down | Navigate entries within a column |
| Left / Right | Switch between boot entries and tools columns |
| Enter | Boot the selected entry |
| Escape | Cancel auto-boot countdown |
| F1 | Help screen |
| F2 | Reboot to firmware setup (UEFI menu) |
| F12 | Cold reboot |

Mouse is also supported: hover to highlight, click to boot. Mouse movement does **not** cancel the auto-boot timer — only keystrokes and clicks do.

---

## Auto-Boot

By default, LamBoot waits 5 seconds, then boots the first kernel entry. This behavior is controlled by `policy.toml`:

- **`default_timeout_ms`**: Delay in milliseconds (0 = wait forever, no auto-boot)
- **`default_entry`**: Entry ID to auto-boot (e.g., `bls-fedora-6.19.9`)

Press **Escape** or any key to cancel the auto-boot countdown and select manually.

If crash loop detection triggers (see [Crash Loop Recovery](#crash-loop-recovery)), auto-boot may be overridden to select a fallback entry.

---

## Booting a Kernel

When you select a boot entry, LamBoot:

1. **Records the selection** in NVRAM (`LamBootLastEntry`)
2. **Sets the boot state** to `Booting`
3. **Writes the boot report** to `\EFI\LamBoot\reports\boot.json`
4. **Appends to the audit log** at `\EFI\LamBoot\reports\audit.log`
5. **Flushes the boot log** to `\EFI\LamBoot\reports\boot.log`
6. **Measures into TPM** (if present): kernel image → PCR 4, config → PCR 5, cmdline → PCR 12
7. **Decrements the boot counter** (if using BLS boot counting)
8. **Loads and starts the kernel** via UEFI LoadImage/StartImage

For Linux kernels with initrd, LamBoot registers the initrd via the LoadFile2 protocol (`LINUX_EFI_INITRD_MEDIA_GUID`), which is the standard mechanism since Linux kernel 5.7. Multiple initrds (e.g., microcode + main initramfs) are concatenated automatically.

---

## Using Diagnostic Tools

LamBoot can load diagnostic modules from `\EFI\LamBoot\modules\` on the ESP. These appear in the right column of the boot menu.

### Available Modules

| Module | Description |
|--------|-------------|
| **Diagnostic Shell** | Interactive 5-screen diagnostic tool (boot vars, PCI, memory, SMBIOS, firmware) |
| **PCI Device Inventory** | Scans PCI bus, detects VirtIO/passthrough devices, writes JSON report |
| **Quick Memory Test** | Reports memory stats and runs walking-ones + address pattern tests |
| **NVMe Health Check** | Planned — not yet implemented |

### Using Modules

1. Navigate to the right column (press **Right** arrow)
2. Select a module and press **Enter**
3. The module runs in text mode
4. When done, press **Q** or the indicated key to return to the LamBoot menu

Modules return to the LamBoot menu without rebooting — you can run multiple tools in succession.

---

## Boot Reports

LamBoot writes diagnostic information to the ESP after every boot:

### `boot.json`

JSON file with the complete boot context:

```json
{
  "lamboot_version": "0.2.0",
  "lamboot_arch": "x86_64",
  "timestamp": "2026-04-05T02:01:49",
  "entry_id": "bls-fedora-6.19.9",
  "entry_name": "Fedora Linux 43 (6.19.9)",
  "entry_type": "linux_legacy",
  "path": "\\EFI\\Linux\\vmlinuz-6.19.9-200.fc43.x86_64",
  "system_manufacturer": "QEMU",
  "system_product": "Standard PC (Q35 + ICH9, 2009)",
  "fleet_id": "",
  "vmid": "201",
  "os_name": "",
  "hypervisor": "KVM",
  "iommu": "",
  "iommu_units": 0,
  "boot_timing_ms": {"health":0,"drivers":1000,"discovery":0,"total":11000}
}
```

### `boot.log`

Full boot trace with timestamps. Write-through during early boot (survives crashes), buffered during menu phase:

```
=== LamBoot 0.2.0 (x86_64) boot log ===
=== 2026-04-05T02:01:49 ===

[2026-04-05T02:01:49] INFO: Crash counter: 0
[2026-04-05T02:01:49] INFO: System: QEMU Standard PC (Q35 + ICH9, 2009)
[2026-04-05T02:01:49] INFO: Hypervisor: KVM
[2026-04-05T02:01:49] INFO: Loaded 4 filesystem driver(s)
[2026-04-05T02:01:49] INFO: Discovering boot entries...
[2026-04-05T02:01:49] INFO: Found 3 boot entries (7 total with tools)
[2026-04-05T02:01:49] INFO: Boot init: 0 ms
[2026-04-05T02:02:50] INFO: Selected: Fedora Linux 43 (6.19.9) (bls-fedora-6.19.9)
```

### `audit.log`

Rolling log (10 KB max) of every boot with entry name and timestamp:

```
[2026-04-05T02:01:49] Boot: Fedora Linux 43 (6.19.9) (bls-fedora-6.19.9)
```

All reports are at `/boot/efi/EFI/LamBoot/reports/` (accessible from the booted OS).

---

## Crash Loop Recovery

LamBoot tracks boot success via NVRAM variables:

1. On each boot attempt, the **crash counter** increments and the state is set to `Booting`
2. After the OS starts successfully, `lamboot-mark-success.service` sets the state to `BootedOK` and resets the counter
3. If the counter reaches the **crash threshold** (default: 3), LamBoot enters crash loop mode:
   - Auto-boot is disabled
   - If `fallback_order` is configured, the first available fallback entry is selected
   - Otherwise, the menu is shown for manual selection

The crash threshold is configurable in `policy.toml` under `security.crash_threshold`.

### VM Snapshot Awareness

On QEMU/KVM, LamBoot reads the VM Generation ID (`vmgenid`). If the ID changes (indicating a snapshot restore), the crash counter is automatically reset to prevent false crash loop detection after rollback.

---

## Updating LamBoot

```bash
sudo tools/lamboot-install --update
```

The update:
- Replaces the binary only if the new one is different (SHA256 check)
- Preserves the existing `policy.toml` (new defaults saved as `policy.toml.new`)
- Preserves boot entries and ESP layout
- Updates the install manifest

---

## Removing LamBoot

```bash
sudo tools/lamboot-install --remove
```

This removes:
- The LamBoot binary from the ESP
- The UEFI boot entry
- All LamBoot files (drivers, modules, reports, policy)
- Systemd integration (mark-success service, kernel-install plugin)

The ESP itself and other bootloaders are not affected.

---

## Preflight Validation

Before displaying the menu, LamBoot validates each boot entry:

| Check | What it validates | Indicator |
|-------|-------------------|-----------|
| File exists | Kernel/EFI binary present on ESP | `X` (error) |
| PE header | Valid MZ signature and PE format | `!` (warning) |
| Architecture | PE machine type matches current arch | `!` (warning) |
| Secure Boot | Image signed (when SB is enabled) | `!` (warning) |
| Initrd exists | Referenced initrd files present | `!` (warning) |
| Root parameter | `root=` present in kernel options | `!` (warning) |
| Filesystem driver | Driver available for non-FAT partitions | `!` (warning) |

Preflight is **advisory only** — it never blocks booting. A warning indicates a potential issue but you can still select and boot the entry.

---

## ESP Layout

After installation, the ESP contains:

```
EFI/LamBoot/
├── lambootx64.efi          # Main bootloader binary
├── policy.toml             # Boot policy configuration
├── .install-manifest       # Tracks installed files
├── drivers/
│   ├── ext4_x64.efi        # Filesystem drivers
│   ├── btrfs_x64.efi
│   └── ...
├── modules/
│   ├── manifest.toml       # Module display names
│   ├── diag-shell.efi
│   ├── pci-inventory.efi
│   └── ...
└── reports/
    ├── boot.json           # Last boot report
    ├── boot.log            # Boot trace log
    ├── audit.log           # Boot history
    └── pci-inventory.json  # PCI scan results (from module)
```

---

## See Also

- [Configuration Guide](CONFIGURATION-GUIDE.md) — policy.toml reference, SMBIOS OEM strings, advanced options
- [Proxmox Guide](PROXMOX-GUIDE.md) — VM fleet deployment, hookscript, monitoring
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — common problems and recovery
- [Security Guide](SECURITY-GUIDE.md) — Secure Boot, TPM, signing
- [Diagnostic Modules](DIAGNOSTIC-MODULES.md) — detailed module reference
- [Install Reference](INSTALL-REFERENCE.md) — lamboot-install complete CLI reference
- [LamBoot Tools](LAMBOOT-TOOLS-OVERVIEW.md) — companion diagnostic and repair toolkit
