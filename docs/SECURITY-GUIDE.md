# LamBoot Security Guide

**Version:** 0.16.5
**Updated:** 2026-06-06

> **Note:** This document is retained for historical reference. The authoritative security documentation is:
> - [`SECURITY-MODEL.md`](SECURITY-MODEL.md) — honest threat model (what LamBoot defends vs. what it doesn't)
> - [`SECURE-BOOT-DEPLOYMENT.md`](SECURE-BOOT-DEPLOYMENT.md) — four Secure Boot deployment configurations
> - [`KEY-GENERATION.md`](KEY-GENERATION.md) — production PK/KEK/db key hierarchy
> - [`MOK-ENROLLMENT-GUIDE.md`](MOK-ENROLLMENT-GUIDE.md) — MokManager walkthrough
>
> The content below is still accurate but less complete than the threat-model doc.

---

## Overview

LamBoot provides layered security features that integrate with the UEFI platform:

- **Secure Boot** — verification of loaded images via ShimLock protocol
- **TPM 2.0 Measured Boot** — extending PCR registers with boot component hashes
- **Crash Loop Detection** — NVRAM-based state machine preventing boot loops
- **Boot Counting** — BLS entry assessment protocol for kernel update safety
- **Policy Controls** — path allowlist/denylist for entry filtering
- **Signing Pipeline** — tools for creating signed binaries and enrolling keys

All security features degrade gracefully — they never block boot when hardware or configuration is absent.

---

## Secure Boot

### Detection Modes

LamBoot detects three Secure Boot states:

| State | Description |
|-------|-------------|
| **Disabled** | Secure Boot is off or not supported. All images load freely. |
| **Active with Shim** | Secure Boot is on, LamBoot was loaded via shim. ShimLock protocol is available for image verification. |
| **Active Direct** | Secure Boot is on, LamBoot was loaded directly (self-signed or enrolled in firmware). |

The current state is displayed in the GUI header ("SB: on" or "SB: off") and recorded in `boot.json`.

### Image Verification

When Secure Boot is active and shim is present, LamBoot calls the ShimLock `Verify` protocol on every loaded image (kernels, UKIs, chainloaded applications). If verification fails, the result is logged but boot is **not blocked** — this is advisory behavior.

Since v0.16.1, the native PE loader that loads the verified kernel bytes is a hand-rolled `no_std` PE/COFF reader that parses only the load-bearing header fields from fixed offsets; the third-party `goblin` PE parser (and its transitive `plain`/`scroll` crates) was dropped from `lamboot-core`, removing a third-party parser from the boot trust and attack surface.

Since v0.16.0, LamBoot reads its on-disk filesystems with native, in-binary, memory-safe Rust readers — ext4/ext2/ext3, btrfs, and FAT, joined by read-only XFS, exFAT, and ZFS (single-disk/mirror/RAIDZ1 unencrypted boot pools) and a read-only media stack (EROFS, ISO 9660, SquashFS, cramfs, romfs, UDF). All are read-only by construction. The practical security effect is that a default install loads no GPLv3 EfiFs `*.efi` filesystem driver for these filesystems; see `SECURITY-MODEL.md` for the full driver-trust story.

The `secure_boot_required` policy option logs a warning when Secure Boot is disabled but does not enforce it.

---

## TPM 2.0 Measured Boot

LamBoot measures boot components into TPM Platform Configuration Registers (PCRs) using the TCG2 protocol and the Linux TPM PCR Registry (UAPI Group) allocation.

### PCR Allocation

| PCR | Contents | Event Type | Description |
|-----|----------|------------|-------------|
| **4** | Kernel image | `EV_EFI_BOOT_SERVICES_APPLICATION` | PE binary data of the kernel or UKI being booted |
| **5** | Boot configuration | `EV_EFI_VARIABLE_BOOT` | policy.toml, manifest.toml, and BLS entry file contents |
| **5** | Filesystem drivers | `EV_EFI_BOOT_SERVICES_DRIVER` | Each loaded firmware filesystem driver (.efi). On a default install these are typically absent: native in-binary readers (ext4/ext2/ext3, btrfs, FAT, XFS, exFAT, ZFS) cover the common filesystems, so their legacy drivers are skipped in the default `[drivers] legacy_uefi_drivers = "auto"` mode and nothing is measured into this slot for them |
| **5** | Diagnostic modules | `EV_EFI_BOOT_SERVICES_APPLICATION` | Each loaded module (.efi) |
| **12** | Kernel command line | `EV_IPL` | Command line options as UTF-16 LE |

### When TPM is Not Present

If no TPM 2.0 is detected (common in VMs without emulated TPM), all measurement calls are silently skipped. Boot proceeds normally. The boot log will show:

```
TPM 2.0: not available, measurements skipped
```

### Adding a TPM to a Proxmox VM

```bash
qm set VMID --tpmstate0 local-lvm:1,version=v2.0
```

After adding a TPM, LamBoot will automatically detect it on the next boot and begin measuring.

---

## Bootloader-Side NVRAM Self-Install

Since v0.16.3, LamBoot ensures its own UEFI boot entry exists directly from the firmware environment, without depending on `efibootmgr` or an OS-side service. At boot it checks for a labeled `Boot####` entry pointing at `\EFI\LamBoot\lambootx64.efi` and, if absent, creates it and front-loads `BootOrder`.

This operation writes only the **non-authenticated** UEFI variables `Boot####` and `BootOrder`. It does **not** touch the signed Secure Boot key stores (`PK`, `KEK`, `db`) or any authenticated variable, so it requires no platform keys and has no effect on the Secure Boot trust chain. It is idempotent — keyed on the exact `LamBoot` description — so it never duplicates or churns NVRAM, and it coexists with the OS-side `efibootmgr` pathway.

It is gated by a policy option (default **on**):

```toml
[boot-entry]
self_install = true   # set false to manage UEFI boot order externally
```

## Crash Loop Detection

### State Machine

LamBoot tracks boot health via NVRAM variables under a custom vendor GUID (`4C414D42-4F4F-5400-0000-000000000001`):

```
Fresh ─── (first boot) ──→ Booting ─── (OS success) ──→ BootedOK
                              │                              │
                              │ (crash/power loss)           │ (next boot)
                              ↓                              ↓
                           Booting ──── (count >= threshold) → CrashLoop
```

### NVRAM Variables

| Variable | Type | Description |
|----------|------|-------------|
| `LamBootState` | u8 | 0=Fresh, 1=Booting, 2=BootedOK, 3=CrashLoop |
| `LamBootCrashCount` | u8 | Incremented on each boot attempt, cleared on success |
| `LamBootLastEntry` | UTF-8 | Entry ID of the last attempted boot |
| `LamBootTimestamp` | 8 bytes | Packed date/time of last boot |
| `LamBootVersion` | u32 LE | Bootloader version (major<<16 \| minor<<8 \| patch) |
| `LamBootTelemetry` | binary | Compact per-phase boot timing |
| `LamBootVmGenId` | 16 bytes | Stored VM Generation ID for snapshot detection |

### How It Works

1. LamBoot reads `LamBootCrashCount` and increments it
2. Sets `LamBootState = Booting`
3. After the OS boots successfully, `lamboot-mark-success.service` sets `LamBootState = BootedOK` and resets the counter to 0
4. If the counter reaches `crash_threshold` (default: 2), LamBoot enters CrashLoop mode:
   - Auto-boot is disabled
   - `fallback_order` entries are tried in sequence
   - If no fallback matches, the menu is shown

### VM Snapshot Awareness

On QEMU/KVM, LamBoot reads the VM Generation ID via fw_cfg (`etc/vmgenid_guid`). If the ID changes (snapshot restore), the crash counter is reset to 0 and the state is set to Fresh. This prevents false crash loop detection after rollback.

---

## Boot Counting (BLS Assessment)

LamBoot implements the BLS boot counting protocol for safe kernel updates:

### Filename Convention

```
entry+3.conf      # 3 tries remaining, being assessed
entry+2-1.conf    # 2 tries remaining, 1 attempt made
entry+0-3.conf    # 0 tries remaining, 3 attempts made → FAILED
entry.conf        # No counters → known good
```

### Behavior

1. When booting an entry with `+N` suffix, LamBoot decrements N (renames the file on the ESP)
2. If `N` reaches 0, the entry is considered failed and sorted to the bottom of the menu
3. The OS-side service (e.g., `systemd-bless-boot.service` or `lamboot-mark-success.service`) removes the counter on successful boot, marking the entry as known good

---

## Policy Security Controls

### Path Allowlist/Denylist

Control which EFI paths appear in the boot menu:

```toml
[security]
# Only show entries from specific directories
allowlist = ["\\EFI\\fedora\\*", "\\EFI\\Linux\\*"]

# Block a specific binary
denylist = ["\\EFI\\Linux\\vmlinuz-6.18.0-broken"]
```

- If `allowlist` is empty, all paths are allowed (unless denied)
- `denylist` overrides `allowlist`
- Matching is case-insensitive
- `*` wildcards supported at start, end, or both positions

### Secure Boot Required

```toml
[security]
secure_boot_required = true
```

When enabled, LamBoot logs a warning if Secure Boot is disabled. This is **advisory only** — it does not prevent boot. Future versions may enforce this.

---

## Signing Pipeline

LamBoot includes a signing script for Secure Boot compatibility.

### Usage

```bash
# Generate keys and sign all binaries
./tools/sign-lamboot.sh

# Sign with existing keys
./tools/sign-lamboot.sh --key-dir /path/to/keys
```

### What It Does

1. **Generates keys** (if not provided): PK (Platform Key), KEK (Key Exchange Key), db (Signature Database) — RSA 2048
2. **Adds SBAT section**: Appends Secure Boot Advanced Targeting metadata from `dist/sbat.csv`
3. **Signs binaries**: Signs `lambootx64.efi`, all module `.efi` files, and all driver `.efi` files with `sbsign`

### Key Enrollment

**OVMF (Proxmox/QEMU)**:
1. Boot into UEFI firmware setup (F2)
2. Navigate to Device Manager → Secure Boot Configuration
3. Enroll the `db.auth` file from a USB drive or the ESP

**Physical hardware**:
```bash
sudo mokutil --import /path/to/db.der
# Reboot and accept in MOK Manager
```

---

## See Also

- [Configuration Guide](CONFIGURATION-GUIDE.md) — policy.toml security options
- [Architecture Guide](ARCHITECTURE.md) — internal design details
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — crash loop recovery
- [User Guide](USER-GUIDE.md) — getting started
