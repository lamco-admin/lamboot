# LamBoot Troubleshooting Guide

**Version:** 0.8.3
**Updated:** 2026-04-21

---

## First Steps

Before debugging, check the boot log:

```bash
cat /boot/efi/EFI/LamBoot/reports/boot.log
```

The boot log records every phase with timestamps. During early boot (before the menu), it writes to the ESP in real-time — so even if LamBoot crashes, you'll see how far it got.

For automated diagnostics, use [lamboot-diagnose](LAMBOOT-TOOLS-OVERVIEW.md):

```bash
sudo lamboot-diagnose
```

---

## Common Problems

### LamBoot doesn't appear after reboot

**Symptoms**: System boots directly into GRUB, systemd-boot, or the OS.

**Causes and fixes**:

1. **Not in boot order**: Run `efibootmgr` to check. LamBoot should appear as a boot entry.
   ```bash
   sudo efibootmgr
   # Look for: Boot0002* LamBoot
   ```
   Fix: `sudo lamboot-install --set-default` to move LamBoot to first in the boot order.

2. **Other bootloader is first**: The UEFI boot order has another bootloader before LamBoot.
   Fix: Use `efibootmgr -o` to reorder, or `lamboot-install --set-default`.

3. **Binary not on ESP**: Verify the file exists:
   ```bash
   ls -la /boot/efi/EFI/LamBoot/lambootx64.efi
   ```
   Fix: Re-run `lamboot-install`.

### Menu appears but no boot entries

**Symptoms**: LamBoot menu shows "No bootable entries found" or only system actions.

**Causes**:

1. **No BLS entries on ESP**: LamBoot reads `\loader\entries\*.conf` on the ESP only.
   ```bash
   ls /boot/efi/loader/entries/
   ```
   Fix: Run `lamboot-install` to generate BLS entries, or ensure your distro writes them to the ESP.

2. **Filesystem driver missing**: If `/boot` is on ext4 or btrfs and separate from the ESP, LamBoot needs a filesystem driver.
   ```bash
   ls /boot/efi/EFI/LamBoot/drivers/
   ```
   Fix: `lamboot-install --with-drivers` to install filesystem drivers.

3. **Policy filtering**: The `allowlist` or `denylist` in `policy.toml` may be hiding entries. Check the policy file.

### Entry shows `!` (warning) or `X` (error) indicator

**Symptoms**: An exclamation mark or X appears on the right side of a boot entry.

These are **advisory only** — they never block booting. You can still select and boot the entry.

| Indicator | Meaning | Common cause |
|-----------|---------|--------------|
| `!` | Warning — potential issue detected | Missing initrd, no `root=` parameter, architecture mismatch |
| `X` | Error — file not found | Kernel or EFI binary missing from ESP |

To see the specific message, select the entry — the footer shows the preflight detail.

Fix: Check that the kernel and initrd paths in the BLS entry point to files that exist on the ESP or an accessible volume.

### Crash loop detected

**Symptoms**: Boot log shows "Crash loop detected", auto-boot may be disabled or a fallback entry is selected.

**What happened**: LamBoot incremented the crash counter on each boot attempt, and `lamboot-mark-success.service` never ran to clear it (because the OS never fully booted).

**Fixes**:

1. **Select a known-good kernel**: Use the menu to manually select an older kernel version.
2. **Reset the counter**: Boot into a working kernel, then:
   ```bash
   # The mark-success service resets it automatically
   sudo systemctl status lamboot-mark-success.service
   ```
3. **Adjust the threshold**: In `policy.toml`, increase `crash_threshold`:
   ```toml
   [security]
   crash_threshold = 5
   ```
4. **After VM snapshot restore**: The crash counter should auto-reset via vmgenid detection. If it doesn't, the vmgenid device may not be configured on the VM.

### Black screen after selecting an entry

**Symptoms**: Screen goes black after choosing a boot entry. System may or may not be booting.

**Causes**:

1. **Kernel is booting normally**: Many Linux kernels clear the screen during early init. Wait 10-20 seconds for the OS to start, then try SSH.
2. **Console driver state**: LamBoot reconnects the UEFI GraphicsConsole driver before handing off. If this fails, text output may be invisible. The kernel should still boot correctly — check via SSH or serial console.

### Module appears to hang

**Symptoms**: After selecting a diagnostic module (diag-shell, pci-inventory), the "Booting: ..." screen stays and nothing happens.

**Causes**:

1. **GraphicsConsole not reconnected**: This was a known bug (fixed in v0.2.0). LamBoot's GUI opens the GOP protocol exclusively, which disconnects the UEFI text console driver. After the fix, `connect_controller` is called before starting any child image.
2. **PCI scan on large bus**: The PCI inventory scans all 256 PCI buses. On systems with many bridges, this can take several seconds. Wait for it to complete.

### Auto-boot selects wrong entry

**Symptoms**: The auto-boot timer selects an entry you didn't expect.

**Fix**: Set `default_entry` in `policy.toml`:
```toml
default_entry = "bls-fedora-6.19.9"
```

Without `default_entry`, LamBoot selects the first boot entry by sort order (highest version, per UAPI.10 sorting).

---

## Recovery Procedures

### F2 — Firmware Setup

Press **F2** from the LamBoot menu (or select "Firmware Setup" from the right column) to reboot into the UEFI/BIOS setup. From there you can change the boot order, disable Secure Boot, or access the UEFI Shell.

### F12 — Cold Reboot

Press **F12** for an immediate system reset.

### Removing LamBoot from the Proxmox Host

If the VM is stuck in LamBoot and can't boot:

```bash
# On the Proxmox host:
qm stop VMID
lvchange -ay pve/vm-VMID-disk-N
qemu-nbd --connect=/dev/nbd0 -f raw /dev/pve/vm-VMID-disk-N
mount /dev/nbd0p1 /tmp/esp

# Remove LamBoot
rm -rf /tmp/esp/EFI/LamBoot

# Or fix a specific file
# cp fixed-lambootx64.efi /tmp/esp/EFI/LamBoot/lambootx64.efi

umount /tmp/esp
qemu-nbd -d /dev/nbd0
qm start VMID
```

### Resetting the Crash Counter

From inside the booted OS:

```bash
# The mark-success service handles this automatically
sudo systemctl start lamboot-mark-success.service

# Or manually delete the NVRAM variables
sudo efivar -d -n 4c414d42-4f4f-5400-0000-000000000001-LamBootCrashCount
sudo efivar -d -n 4c414d42-4f4f-5400-0000-000000000001-LamBootState
```

### Booting with a Different Bootloader

If LamBoot is first in the boot order and you need to skip it:

1. Enter UEFI firmware setup (press the firmware's key during POST — usually Del, F2, or F10)
2. Change the boot order to put your other bootloader first
3. Or use the one-time boot menu (usually F8 or F11 during POST)

---

## Boot Report Reference

### boot.json Fields

| Field | Description |
|-------|-------------|
| `lamboot_version` | LamBoot binary version |
| `lamboot_arch` | Architecture (x86_64 or aarch64) |
| `timestamp` | ISO 8601 timestamp from UEFI RTC |
| `entry_id` | Selected entry's identifier |
| `entry_name` | Selected entry's display name |
| `entry_type` | `linux_legacy`, `uki`, or `chainload` |
| `path` | Kernel or EFI binary path |
| `system_manufacturer` | SMBIOS manufacturer (e.g., "QEMU") |
| `system_product` | SMBIOS product name |
| `fleet_id` | Fleet identifier from OEM strings |
| `vmid` | VM ID from OEM strings |
| `os_name` | Detected OS name (when available) |
| `hypervisor` | Detected hypervisor (KVM, Hyper-V, etc.) |
| `iommu` | IOMMU type (Intel VT-d or AMD-Vi) |
| `iommu_units` | Number of IOMMU hardware units |
| `boot_timing_ms` | Per-phase timing: health, drivers, discovery, total |

### boot.log Phases

| Log message | Phase | What's happening |
|-------------|-------|-----------------|
| `Crash counter: N` | Health | Boot health assessment |
| `System: ...` | SMBIOS | Hardware identification |
| `Hypervisor: ...` | Detection | CPUID-based hypervisor detection |
| `Loaded N filesystem driver(s)` | Drivers | ext4/btrfs driver loading |
| `Discovering boot entries...` | Discovery | BLS and ESP scanning |
| `Found N boot entries` | Discovery | Entry count summary |
| `Boot init: N ms` | Telemetry | Total init time |
| `Selected: ...` | Menu | User or auto-boot selection |

---

## Using lamboot-tools for Diagnostics

The [lamboot-tools](LAMBOOT-TOOLS-OVERVIEW.md) companion package provides automated diagnostic and repair tools:

| Situation | Tool | Command |
|-----------|------|---------|
| General boot health check | lamboot-diagnose | `sudo lamboot-diagnose` |
| ESP filesystem issues | lamboot-esp | `sudo lamboot-esp check` |
| Backup before changes | lamboot-backup | `sudo lamboot-backup save` |
| Automated boot repair | lamboot-repair | `sudo lamboot-repair` |
| Offline VM repair from host | lamboot-repair | `sudo lamboot-repair --offline /dev/pve/vm-201-disk-1` |
| Migrating from GRUB | lamboot-migrate | `sudo lamboot-migrate to-lamboot` |
| BIOS to UEFI conversion | lamboot-migrate | `sudo lamboot-migrate to-uefi` |

---

## See Also

- [User Guide](USER-GUIDE.md) — getting started, daily use
- [Configuration Guide](CONFIGURATION-GUIDE.md) — policy.toml reference
- [Proxmox Guide](PROXMOX-GUIDE.md) — VM-specific deployment and monitoring
- [LamBoot Tools](LAMBOOT-TOOLS-OVERVIEW.md) — diagnostic and repair toolkit
- [Security Guide](SECURITY-GUIDE.md) — Secure Boot, TPM, crash loop details
