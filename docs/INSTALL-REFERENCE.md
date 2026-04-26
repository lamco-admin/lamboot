# lamboot-install Reference

**Version:** 0.8.3
**Updated:** 2026-04-21

---

## Synopsis

```
lamboot-install [OPTIONS]
```

Install, update, or remove the LamBoot UEFI bootloader on the local system.

---

## SDS-6 Driver Install Policy

From v0.9.0, `lamboot-install` gates legacy UEFI filesystem driver
deployment against a new policy knob. The default (`auto`) installs
drivers only for filesystems LamBoot does NOT natively support. For
stock ext4 `/boot` installs, no third-party `.efi` driver is written
to the ESP at all.

| Flag | Effect |
|------|--------|
| `--with-drivers-legacy=auto` (default) | Install a driver only if `/boot`'s filesystem has no native LamBoot backend. ext2/3/4 skipped (native via SDS-2). btrfs/xfs/ntfs/zfs/f2fs/iso9660 installed when applicable. |
| `--with-drivers-legacy=all` | v0.8.3 behavior — install every driver applicable to the running architecture, regardless of native coverage. Useful for debugging. |
| `--with-drivers-legacy=none` | Install no legacy drivers. Fails gracefully at boot if `/boot` is not natively covered. Use only after confirming `/boot` is ext2/3/4 or vfat. |
| `--with-drivers` | Alias for `--with-drivers-legacy=all`. Retained for v0.8.x script compatibility. |

On `--update`, lamboot-install also **prunes** previously-installed
`ext4_x64.efi` / `ext2_x64.efi` / `ext3_x64.efi` (and the `-signed`
and aarch64 variants) from the ESP in Auto mode — they are no longer
needed and their presence has security implications (each would load
at boot under `--with-drivers-legacy=all`, triggering shim 15.8's
ShimLock-uninstall). To keep them around for A/B testing, run
`lamboot-install --update --with-drivers-legacy=all`.

See `docs/specs/SPEC-UEFI-FSDRV-DEPRECATION.md` and
`docs/CONFIGURATION-GUIDE.md` `[drivers]` section.

## Options

| Flag | Description |
|------|-------------|
| `--esp PATH` | Override ESP mount point detection |
| `--no-make-default` | Do **not** make LamBoot the default boot entry. (Default behavior is to promote LamBoot to first in BootOrder, matching `grub-install`, `bootctl install`, `refind-install`.) The previous default is always preserved as the next entry in BootOrder. |
| `--set-default` / `--make-default` | Explicitly make LamBoot default. This is now the install default; these flags are retained for backward compatibility and explicit scripts. |
| `--fallback` | Also install as `\EFI\BOOT\BOOTX64.EFI` (fallback boot path) |
| `--no-efi-entry` | Don't create a UEFI boot entry (file copy only) |
| `--no-bls` | Don't generate BLS entries (use existing ones) |
| `--with-drivers` | Install filesystem drivers (auto-detected by default) |
| `--with-modules` | Install diagnostic modules |
| `--update` | Update existing installation (preserve config) |
| `--remove` | Remove LamBoot installation |
| `--keep-entries` | With `--remove`: keep generated BLS entries |
| `--dry-run` | Show what would happen without doing anything |
| `--force` | Skip safety checks |
| `--quiet` | Minimal output |
| `--verbose` | Detailed output |
| `--version` | Print version and exit |
| `--help`, `-h` | Print usage and exit |

---

## Exit Codes

| Code | Name | Meaning |
|------|------|---------|
| 0 | `EXIT_OK` | Success |
| 1 | `EXIT_ERROR` | Fatal error — installation failed |
| 2 | `EXIT_PARTIAL` | Partial success — some steps completed, some failed |
| 3 | `EXIT_NOOP` | Nothing to do — already up-to-date (with `--update`) |

---

## Installation Phases

The installer runs 8 phases in sequence:

### Phase 1: Environment Detection

- Detects system architecture (x86_64 or aarch64)
- Finds the ESP mount point (auto-detection via `findmnt` and `lsblk`, or `--esp` override)
- Identifies the Linux distribution (Fedora, Debian, Arch, openSUSE, etc.)
- Checks for existing LamBoot installation
- Verifies minimum ESP free space (2 MB)

### Phase 2: Filesystem Driver Assessment

- Checks if `/boot` is on a separate partition with a non-FAT filesystem
- Determines which filesystem driver(s) are needed (ext4, btrfs)
- Drivers are auto-installed unless `--with-drivers` is explicitly specified for forced install

### Phase 3: Boot Entry Discovery

- Inventories existing BLS entries at `/boot/efi/loader/entries/`
- Discovers installed kernels in `/boot`
- Reads current kernel command line from `/proc/cmdline`

### Phase 4: File Installation

- Creates the directory structure: `EFI/LamBoot/{drivers,modules,reports}`
- Copies the LamBoot binary (`lambootx64.efi` or `lambootaa64.efi`)
- Installs filesystem drivers to `EFI/LamBoot/drivers/`
- Installs diagnostic modules and manifest to `EFI/LamBoot/modules/` (if `--with-modules`)
- Copies `policy.toml` — **only if not already present** (new defaults saved as `policy.toml.new`)
- All copies use atomic rename (write to `.tmp`, then `mv`) for crash safety
- Files are tracked in `.install-manifest` for clean removal

### Phase 5: BLS Entry Generation

- If the distro provides native BLS entries (Fedora), this phase is skipped
- Otherwise, generates BLS `.conf` files for each kernel in `/boot`
- Includes title, version, linux path, initrd path(s), and kernel options
- Microcode initrd auto-prepended (Intel or AMD ucode)

#### Where LamBoot writes its own BLS entries (v0.9.0+)

Post-SDS-5, `lamboot-install` **always** writes generated entries to
`$ESP/loader/entries/` on the FAT ESP — regardless of whether your
distro keeps its existing entries on the ESP (Debian, Ubuntu with
BLS, Pop!_OS, EndeavourOS, Arch with systemd-boot) or on ext4
`/boot/loader/entries/` (Fedora, openSUSE Tumbleweed).

**Why:**

- **Spec-compliant location.** ESP is always a valid BLS location
  per the UAPI Boot Loader Specification.
- **Writable at runtime.** LamBoot's boot-counter rename
  (`entry+3-0.conf` → `entry+2-1.conf`) works only on FAT because
  `ext4-view` is read-only by design. Entries we generate on the
  ESP therefore support `systemd-bless-boot` integration; entries
  the distro left on ext4 do not.
- **Fast boot reads.** FAT backend is in-firmware; no native ext4
  parser needed.
- **Coexistence.** Runtime LamBoot scans **every** mounted volume
  for `/loader/entries/*.conf`, so distro entries on ext4 coexist
  with `lamboot-install`-generated entries on ESP — both appear in
  the menu, grouped by source volume.

**What `lamboot-install` does NOT do:**

- Does not copy distro BLS entries from ext4 to ESP (that would
  duplicate state and fight `kernel-install`).
- Does not rewrite distro entries (LamBoot's read-only stance).
- Does not install BLS entries to `/boot/loader/entries/` even if
  `/boot` is on FAT — always at `/loader/entries/` on the ESP.

**Boot-counter limitation on ext4 entries.** Systems using
Fedora-style ext4 `/boot/loader/entries/` cannot have those
entries participate in `systemd-bless-boot` counter decrement.
The security gain from a read-only `/boot` mount outweighs the
loss of counter-based automatic fallback for most users. Three
workarounds if you need the counter:

1. Keep `/boot` on FAT (effectively kernel-on-ESP flow).
2. Use UKIs on the ESP — same effect.
3. Migrate to `lamboot-install`'s generated ESP entries and
   disable `kernel-install` on the distro side.

See `docs/specs/SPEC-BLS-MULTI-FS.md` §6 for the full policy.

### Phase 6: UEFI Boot Entry

- Creates a UEFI boot entry via `efibootmgr` (if not already present)
- **Default behavior (v0.8.3+):** LamBoot is promoted to the **first** position in BootOrder, matching `grub-install` / `bootctl install` / `refind-install`. The previous default is preserved as the next entry so there's a one-keystroke fallback.
- `--no-make-default`: preserves the previous default and appends LamBoot to the **end** (the old pre-v0.8.3 behavior — useful for trial installs alongside an existing loader)
- `--set-default` / `--make-default`: explicit form of the current default
- `--fallback`: copies to `\EFI\BOOT\BOOTX64.EFI` (backs up existing)
- `--no-efi-entry`: skips boot entry creation entirely

### Phase 7: Systemd Integration

- Installs `lamboot-mark-success.service` — runs on boot, sets LamBootState=BootedOK and resets crash counter
- Installs `90-lamboot.install` — kernel-install plugin that generates BLS entries for new kernels
- Enables the mark-success service via systemctl
- Skipped if systemd is not present

### Phase 8: Verification

- Verifies the binary exists on the ESP with correct size
- Checks filesystem drivers are present (applicable when
  `--with-drivers-legacy` retained any; see §"SDS-6 Driver Install
  Policy" above)
- Verifies the UEFI boot entry exists
- Validates each BLS entry: checks kernel and initrd file existence
  across **both** `$ESP/loader/entries/` and `/boot/loader/entries/`
  (Fedora-layout systems keep BLS on a separate ext4 `/boot`
  partition, so ESP-only scanning would false-warn "empty menu" even
  when entries are present — see commit a586677)
- Reports any issues found

---

## Requirements

| Requirement | Usage |
|-------------|-------|
| bash 4.0+ | Script execution (associative arrays) |
| GNU coreutils or uutils | File operations (cp, mv, stat, sha256sum, etc.) |
| util-linux | `findmnt`, `lsblk`, `mount`, `mountpoint` |
| efibootmgr | UEFI boot entry management |
| systemctl _(optional)_ | Systemd service enable/disable |
| file _(optional)_ | Arch Linux kernel version detection |
| grep, sed, awk _(optional)_ | POSIX-compatible versions sufficient |

---

## Distro-Specific Behavior

### Fedora

- Native BLS entries present → Phase 5 skipped
- Uses `/boot/efi` as default ESP mount
- kernel-install already generates BLS entries

### Debian / Ubuntu

- No native BLS entries → Phase 5 generates them
- `/boot` is often on ext4 → ext4 driver auto-installed
- Kernel naming: `vmlinuz-VERSION`, `initrd.img-VERSION`

### Arch Linux

- Kernel naming: `vmlinuz-linux`, `initramfs-linux.img`
- Uses `file` command for kernel version detection (if available)
- May use `/efi` instead of `/boot/efi`

### openSUSE

- May have `/boot` on btrfs → btrfs driver auto-installed
- BLS entries generated if not present

---

## Configuration Preservation

When running `--update`:
- The existing `policy.toml` is **never overwritten**
- New defaults are written to `policy.toml.new` for reference
- All other files are updated based on SHA256 comparison
- If a file is unchanged, it is skipped (exit code 3 if nothing changed)

When running `--remove`:
- All files tracked in `.install-manifest` are removed
- The UEFI boot entry is deleted
- Systemd services are disabled and removed
- The ESP directory structure is cleaned up (empty dirs removed)
- BLS entries are removed unless `--keep-entries` is specified

---

## ESP Detection

The installer finds the ESP using this priority:

1. `--esp PATH` flag (explicit override)
2. `findmnt --target /boot/efi` (standard mount point)
3. `findmnt --target /efi` (alternative mount point)
4. `lsblk` scan for partitions with ESP type GUID (`c12a7328-f81f-11d2-ba4b-00a0c93ec93b`)

The detected ESP must have at least 2 MB free space.

---

## See Also

- [User Guide](USER-GUIDE.md) — getting started, daily use
- [Configuration Guide](CONFIGURATION-GUIDE.md) — policy.toml reference
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — common problems
- [LamBoot Tools](LAMBOOT-TOOLS-OVERVIEW.md) — companion diagnostic toolkit
