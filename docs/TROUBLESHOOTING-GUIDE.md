# LamBoot Troubleshooting Guide

**Version:** 0.16.5
**Updated:** 2026-06-08

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

2. **`/boot` on an unreadable filesystem**: As of v0.16.0 LamBoot reads ext4, btrfs, XFS, exFAT, ZFS (single-disk/mirror/RAIDZ1 boot pools), and FAT natively in-binary, read-only — no firmware driver required — so a separate ext4/btrfs/xfs `/boot` is mounted directly. The bundled EfiFs `*.efi` drivers ship only as an inert fallback (the xfs/zfs ones are skipped at boot because the native readers cover those filesystems). If `/boot` is on an unsupported layout (e.g. an encrypted ZFS pool, RAIDZ2/3, or a multi-vdev pool), LamBoot rejects it cleanly rather than guessing.
   ```bash
   ls /boot/efi/EFI/LamBoot/drivers/
   ```
   Fix: confirm `/boot`'s filesystem is one of the supported readers; check `boot.log` for the mount path the native backend took.

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

### Named UEFI boot entry vanishes after reboot (Proxmox / OVMF, ESP on a separate disk)

**Symptoms**: LamBoot boots once, but after a reboot the firmware loses the `LamBoot` `Boot####` entry and boots GRUB/systemd-boot or the firmware fallback instead. Common on Proxmox/QEMU OVMF when the ESP lives on a disk that is not in the VM's boot index.

**What happened**: OVMF prunes a named NVRAM `Boot####` entry that points at a disk outside the VM's `boot:` order on the next reboot. As of v0.16.1 the installer warns about this (`esp_on_separate_disk` trust event) when it detects the ESP and the OS root on different disks.

**Fixes**:

1. **Make the ESP disk boot-indexed on the host** (the actionable fix):
   ```bash
   # On the Proxmox host — add the ESP's disk to the VM boot order:
   qm set VMID --boot order=scsi0   # use the disk that carries the ESP
   ```
2. **Or install via the removable-media fallback** so the firmware loads LamBoot from `\EFI\BOOT\BOOTX64.EFI` regardless of named entries:
   ```bash
   sudo lamboot-install --update --fallback
   ```
3. **Bootloader-side self-install** (v0.16.3) recreates the `LamBoot` `Boot####` entry from the UEFI environment on each boot, but it cannot override a firmware that prunes the entry on a non-boot-indexed disk — the host-side `qm set --boot` / `--fallback` fix above is still required there.

### BIOS-installed RHEL/Rocky/Alma (MBR /boot) shows no entries

**Symptoms**: A BIOS-installed RHEL-family system converted to UEFI shows "No bootable entries found" even though `/boot` exists.

**What happened**: Before v0.16.3, partition discovery consumed only GPT entries, so a legacy-MBR (`msdos`) disk with XFS `/boot` as a primary partition yielded zero discovered partitions.

**Fix**: Update to v0.16.3 or later. `scan_discoverable_partitions` is now a three-source enumerator — GPT, MBR (via `PartitionInfo.mbr_partition_record()`), and BlockIO-only logical partitions, de-duplicated by handle — so MBR `/boot` is discovered and mounted by the native readers unchanged. Check `boot.log` for the discovered partition count.

### Newly installed kernel shows `X` (error) on XFS `/boot`

**Symptoms**: After a kernel upgrade on a RHEL-family system with an XFS `/boot` (surfaced by a RHEL 9.7 -> 9.8 upgrade), the new kernel's entry shows a red `X` and won't boot, while the old kernel still boots.

**What happened**: This was a host-side tooling bug in `lamboot-kernel-hook` (the bootloader binary was unaffected). Its placement set omitted `xfs`, so the upgrade wrote the new BLS entry to the ESP instead of in place on the XFS `/boot`, and the `linux` field referenced kernel-install's staging path rather than the bootable `/boot` kernel copy.

**Fix**: Update lamboot-tools to v0.16.4 or later, then reinstall the affected kernel (e.g. `dnf reinstall kernel-core-<version>`) so the hook rewrites the entry in place. Placement parity between the hook and `lamboot-install` is now build-enforced by `tools/check-fs-coverage-parity.py`.

### Boot-from-ISO: no ISO entries appear

**Symptoms**: You enabled boot-from-ISO but no ISO entries show in the menu.

Boot-from-ISO is **opt-in and off by default**. Both gates live in `policy.toml`:

```toml
[boot-from-iso]
enabled = true   # boot a distro .iso from a mounted volume (default false)
optical = true   # boot from a physical CD/DVD/BD (default false)
```

**Causes**:

1. **Gate not enabled**: With `enabled = false` (the default), file-hosted ISOs are never scanned. Set `enabled = true`. For a physical disc, also set `optical = true`.
2. **ISO not in a scanned directory**: File-hosted ISO discovery scans `/isos` and `/boot/isos` for `*.iso` on each mounted volume (ext4/btrfs/xfs/FAT, including the ESP). Move or symlink the image there.
3. **Distro not yet live-validated**: Only Arch 2026.05 and Fedora 44 are live-ISO-booted end to end in this release. The other families (ubuntu-casper, debian-live, opensuse, alpine, and derivatives) are recipe/table-validated but not yet live-booted, so treat them as experimental.
4. **Kernel won't load**: A distribution kernel is a Linux EFI-stub PE. Since v0.16.1 LamBoot's native loader reads the EFI-stub PE layout directly (the hand-rolled no_std PE/COFF reader accepts the DOS-stub-less image that the old goblin-based loader rejected); if the native load fails, the ISO path falls back to firmware `LoadImage`. If that fails too, check `boot.log` for the load error.

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
| `Loaded N filesystem driver(s)` | Drivers | Fallback EfiFs drivers; xfs/zfs skipped (native readers cover them) |
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
