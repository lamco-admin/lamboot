# LamBoot Tools — Companion Toolkit

**Version:** 0.8.3
**Updated:** 2026-04-21

---

## Overview

LamBoot Tools is a companion toolkit of five bash CLI utilities for boot diagnostics, maintenance, backup, repair, and migration. The tools operate on running systems or offline VM disks, making them suitable for both bare-metal administration and Proxmox fleet management.

All tools are in the [lamboot-tools](https://github.com/lamco-admin/lamboot-tools-dev) repository.

---

## When to Use Which Tool

| Situation | Tool | Command |
|-----------|------|---------|
| Something seems wrong with boot | `lamboot-diagnose` | `sudo lamboot-diagnose` |
| ESP filesystem corruption or space issues | `lamboot-esp` | `sudo lamboot-esp check` |
| Before making changes to boot config | `lamboot-backup` | `sudo lamboot-backup save` |
| System won't boot, need automated repair | `lamboot-repair` | `sudo lamboot-repair` |
| VM won't boot, repair from Proxmox host | `lamboot-repair` | `sudo lamboot-repair --offline /dev/pve/vm-201-disk-1` |
| Migrating from BIOS/MBR to UEFI/GPT | `lamboot-migrate` | `sudo lamboot-migrate to-uefi` |
| Replacing GRUB with LamBoot | `lamboot-migrate` | `sudo lamboot-migrate to-lamboot` |
| Check current boot configuration | `lamboot-migrate` | `sudo lamboot-migrate status` |
| List what's on the ESP | `lamboot-esp` | `sudo lamboot-esp inventory` |
| Clean stale files from ESP | `lamboot-esp` | `sudo lamboot-esp clean` |

---

## lamboot-diagnose

Comprehensive UEFI boot diagnostic scanner. Checks the full boot chain and reports issues with actionable recommendations.

### Usage

```
lamboot-diagnose [OPTIONS]
```

### What It Checks

- **Partition table**: GPT vs MBR, ESP partition type GUID
- **ESP health**: Mount state, filesystem type, free space, permissions
- **Boot entries**: UEFI boot variables, boot order, LamBoot entry presence
- **Bootloader files**: Binary existence and size on ESP
- **Kernels**: Installed kernels, matching initrds, valid BLS entries
- **Secure Boot**: Status, enrolled keys, MOK state
- **fstab**: ESP mount configuration
- **VM-specific**: OVMF NVRAM variables, LamBoot health state (crash counter, boot state)

### Options

| Flag | Description |
|------|-------------|
| `--json` | Output in JSON format (for scripting and monitoring) |
| `--verbose` | Show all checks including passing ones |
| `--quiet` | Only show failures and warnings |

### Requirements

bash 4.0+, findmnt, lsblk, blkid. Optional: efibootmgr, dosfstools, virt-fw-vars.

---

## lamboot-esp

EFI System Partition health check and management tool. Scans the ESP for filesystem integrity, space usage, and bootloader inventory.

### Usage

```
lamboot-esp [OPTIONS] [check|inventory|clean]
```

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `check` | (default) Full ESP health scan — integrity, space, permissions, fallback path |
| `inventory` | List all files on the ESP with sizes and bootloader identification |
| `clean` | Identify and optionally remove stale/orphaned files (dry-run by default) |

### What It Reports

- Filesystem integrity (via `fsck.fat` if available)
- Space usage and free space warnings
- Bootloader inventory (LamBoot, GRUB, systemd-boot, Windows, rEFInd, shim)
- Stale or orphaned files (old kernels, abandoned bootloaders)
- Fallback path (`\EFI\BOOT\BOOTX64.EFI`) presence and status
- Directory permissions

### Requirements

bash 4.0+, findmnt, lsblk, blkid. Optional: dosfstools (for `fsck.fat`).

---

## lamboot-backup

UEFI boot configuration backup and restore. Exports the complete boot configuration to a JSON file for safekeeping before making changes.

### Usage

```
lamboot-backup save [FILE]         # Save boot config (default: lamboot-backup.json)
lamboot-backup restore FILE        # Restore boot entries from backup
lamboot-backup show [FILE]         # Display backup contents
```

### What It Saves

- All UEFI boot entries (paths, labels, attributes)
- Boot order
- Boot timeout
- Secure Boot state
- LamBoot-specific variables (crash counter, state, last entry)
- Timestamp and system identifier

### Offline VM Support

With `kernel-bootcfg` or `virt-fw-vars`, backups can be taken from offline VM NVRAM images without booting the VM.

### Requirements

bash 4.0+, efibootmgr. Optional: kernel-bootcfg (for offline operations).

---

## lamboot-repair

Boot repair tool for online and offline systems. Follows a structured Diagnose → Plan → Show → Confirm → Execute → Verify workflow — it never makes changes without showing you the plan first.

### Usage

```
lamboot-repair [OPTIONS]                     # Online repair (running system)
lamboot-repair --offline DISK [OPTIONS]      # Offline repair (VM disk from host)
```

### What It Can Fix

- Missing or corrupted LamBoot binary on ESP
- Missing UEFI boot entry
- Missing BLS entries
- Missing filesystem drivers
- ESP filesystem errors
- Boot order issues
- Missing initrd references
- Broken fallback path

### Offline Mode

Repair a VM that won't boot, from the Proxmox host:

```bash
sudo lamboot-repair --offline /dev/pve/vm-201-disk-1
```

This uses `qemu-nbd` to mount the VM's disk, inspects the ESP and NVRAM, generates a repair plan, and executes it — all without booting the VM.

### Options

| Flag | Description |
|------|-------------|
| `--offline DISK` | Repair an offline VM disk image |
| `--dry-run` | Show repair plan without executing |
| `--force` | Skip confirmation prompts |
| `--quiet` | Minimal output |
| `--verbose` | Detailed diagnostics |

### Requirements

**Online**: bash 4.0+, efibootmgr, findmnt, lsblk, blkid.
**Offline**: qemu-nbd (or losetup), virt-fw-vars, mount. Optional: dosfstools, sgdisk.

---

## lamboot-migrate

BIOS→UEFI migration and cross-bootloader migration tool. Handles the complete conversion from legacy BIOS/MBR to modern UEFI/GPT, including partition table conversion, ESP creation, and bootloader installation.

### Usage

```
lamboot-migrate to-uefi [OPTIONS]       # BIOS/MBR → UEFI/GPT conversion
lamboot-migrate to-lamboot [OPTIONS]    # Install LamBoot alongside/replacing current bootloader
lamboot-migrate status                  # Show current boot configuration
```

### to-uefi: BIOS→UEFI Migration

The first automated end-to-end BIOS→UEFI migration tool for Linux. Steps:

1. **Safety checks**: Verifies current system state, checks for data risks
2. **MBR→GPT conversion**: Converts partition table via `sgdisk` (non-destructive)
3. **ESP creation**: Creates and formats an EFI System Partition
4. **Bootloader installation**: Installs LamBoot to the new ESP
5. **fstab update**: Adds ESP mount to `/etc/fstab`
6. **Verification**: Confirms everything is ready for UEFI boot

**Recommended**: Run from live media when converting the boot disk. Always have a backup.

### to-lamboot: Cross-Bootloader Migration

Install LamBoot alongside or replacing an existing bootloader (GRUB, systemd-boot):

1. Installs LamBoot binary and drivers to ESP
2. Generates BLS entries from existing kernel/initrd pairs
3. Creates UEFI boot entry
4. Preserves existing bootloader as a chainload option

### status: Boot Configuration Report

Shows current system boot configuration:

- Boot mode (UEFI or BIOS/Legacy)
- Partition table type (GPT or MBR)
- ESP presence and mount point
- Installed bootloaders
- Kernel versions

### Requirements

bash 4.0+, sgdisk (gdisk package), mkfs.vfat (dosfstools), efibootmgr. Recommended: run from live media for `to-uefi`.

---

## Installation

```bash
# Clone the repository
git clone https://github.com/lamco-admin/lamboot-tools-dev.git

# Tools are standalone bash scripts — copy to your PATH
sudo cp lamboot-tools-dev/tools/lamboot-* /usr/local/bin/
```

Or install via the LamBoot installer:
```bash
sudo lamboot-install --with-modules  # installs tools alongside LamBoot
```

---

## See Also

- [User Guide](USER-GUIDE.md) — LamBoot bootloader usage
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — when to use each tool
- [Install Reference](INSTALL-REFERENCE.md) — lamboot-install documentation
- [Proxmox Guide](PROXMOX-GUIDE.md) — offline repair for VMs
