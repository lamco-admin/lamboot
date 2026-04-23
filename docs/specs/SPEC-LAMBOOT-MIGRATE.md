# SPEC-LAMBOOT-MIGRATE: BIOS→UEFI Conversion and Cross-Bootloader Migration Tool

**SDS number:** SDS-7 (v0.9.x companion; independent of SDS-1..6)
**Version:** 1.0 (spec-level target)
**Date:** 2026-04-22
**Status:** Ready for implementation review — gap-close target for an existing v0.1.0 tool
**Existing implementation:** `lamboot-migrate` v0.1.0 lives at
`~/lamboot-tools-dev/tools/lamboot-migrate` (626 lines, bash,
committed 2026-04-05). It implements `to-uefi`, `to-lamboot`, and
`status` subcommands. **This SDS describes the v1.0 target, not a
green-field design.** The existing implementation is authoritative
for flows already working; SDS-7 bundles the gap-close deliverables
(verify + rollback subcommands, 10-phase pipeline formalization,
three explicit Proxmox methods, top-5 failure guardrails, distro
recipes). See §14 for the deviation/reconciliation table.
**Target repo:** `lamco-admin/lamboot-tools-dev` (private) → future
`lamco-admin/lamboot-tools` (public). NOT `lamboot-dev`. The tool
is host-side, not bootloader-internal.
**Depends on:** none of SDS-1..6. `lamboot-migrate` is a host-side bash tool, architecturally independent of the bootloader. It CAN be delivered before or alongside v0.9.0. Per founder direction: high-priority for pre-v0.9.0 bundling.
**Blocks:** expanded fleet-test coverage (converting SeaBIOS VMs to OVMF unlocks ~14 test VMs currently outside the UEFI-only LamBoot-testable set — see `docs/FLEET-TEST-PLAN-2026-04-21.md` reality-check).
**External dependencies:** `sgdisk` (gptfdisk), `mkfs.vfat` (dosfstools), `efibootmgr`, `blkid`, `lsblk`, `rsync` (for backup), `qm` / `virsh` (optional, per-hypervisor helpers). All broadly available in distro repos.

---

## 1. Overview

`lamboot-migrate` is a single-binary host-side CLI that converts an existing legacy-BIOS Linux install to UEFI, either in-place on physical hardware or via one of three documented flows on Proxmox/libvirt VMs. It is also the canonical tool for migrating an existing UEFI install FROM GRUB/rEFInd/systemd-boot TO LamBoot (the `to-lamboot` subcommand referenced in `docs/TROUBLESHOOTING-GUIDE.md`).

The tool is **NOT** a replacement for `lamboot-install`. `lamboot-install` installs LamBoot on an already-UEFI system. `lamboot-migrate` brings a non-UEFI system up to the point where `lamboot-install` can run, then hands off.

### 1.1 Motivation

- **Fleet coverage unlock.** Per `docs/FLEET-TEST-PLAN-2026-04-21.md`, 14 of LamBoot's 18 distro-testable VMs are currently BIOS/SeaBIOS — outside LamBoot's testable set until converted. A robust `lamboot-migrate` turns them on-line.
- **User path from legacy installs.** A 2010-era Ubuntu VM on MBR+BIOS exists on many Proxmox servers. Users want to get those to UEFI without reinstalling.
- **Cross-bootloader migration.** Moving a UEFI install from GRUB to LamBoot is currently manual (`lamboot-install --set-default` after trial boot). `lamboot-migrate to-lamboot` codifies the steps.
- **Documented-design has been sitting unbuilt for months.** `docs/BOOT-TOOLKIT-LANDSCAPE-2026-04-04.md §4` has the design; nobody's written the code. SDS-7 captures the design as implementation-ready and commits to ship it.

### 1.2 Subcommand structure

```
lamboot-migrate to-uefi [OPTIONS]       — Convert BIOS+MBR install to UEFI+GPT
lamboot-migrate to-lamboot [OPTIONS]    — Migrate an already-UEFI install from GRUB/rEFInd/sd-boot to LamBoot
lamboot-migrate verify [--all|--path PATH]  — Run the verification checklist (§9) against a system
lamboot-migrate rollback                — Revert the last to-uefi run using its backup
```

`to-uefi` is the central, complex subcommand. `to-lamboot` is thin wrapper around `lamboot-install`. `verify` is read-only diagnostics. `rollback` is the safety net.

### 1.3 What this SDS delivers

- Full CLI spec (flags, exit codes, interaction model)
- `to-uefi` pipeline in 10 phases with per-phase safety checks
- Three Proxmox methods (A pre-convert, B live-chroot, C add-ESP-disk) with decision tree
- `to-lamboot` four-step upgrade from competing bootloader
- Top-5 failure-mode guardrails enshrined as pre-flight / in-flight checks
- Backup + restore discipline (what's backed up, where, how to roll back)
- Distro-specific recipes as sub-modules (Ubuntu, Debian, Fedora, Arch)
- Verification checklist as automated subcommand
- Test plan (VM-fleet conversion, negative cases)
- User-facing guide outline

### 1.4 What this SDS does NOT deliver

- **Physical-hardware hands-on migration.** Documented as "caveat emptor: test in a VM first; failure modes on real hardware are worse." Not blocked, but not actively supported in v0.9.x.
- **Converting GPT-UEFI installs BACK to MBR-BIOS.** Nobody should want this; not implemented.
- **MBR→GPT in-place without a backup step.** Always back up. Always.
- **Rescue from a partially-converted state that wasn't made BY lamboot-migrate.** We can only restore our own backups.
- **Dual-boot awareness.** If the system has Windows on MBR we refuse to convert (would require hybrid MBR, which §5 forbids). Documented.
- **Online conversion of a live-running production system.** Always requires a reboot at the end; document the downtime.
- **An interactive GUI / ncurses front-end.** CLI + prompts only.
- **btrfs snapshot-based rollback.** Too distro-specific. Standard rsync-to-a-backup-dir suffices.

### 1.5 Constraints

- **Single Bash file**, `#!/bin/bash`, POSIX-compatible where possible (matches `lamboot-install` style).
- **Zero runtime dependencies beyond coreutils + the external binaries listed above** (sgdisk, mkfs.vfat, efibootmgr, blkid, lsblk, rsync). Each is checked for presence up-front with a clear install hint if missing.
- **No Python, Perl, jq.** Standard utilities only.
- **Must run as root.** Partition operations require it.
- **Target: any Linux distribution with UEFI firmware available on the host/hypervisor.** (For VMs, the conversion result requires the VM to switch to OVMF; the tool may run during the conversion still on BIOS, then instruct the user to switch the firmware.)
- **Idempotent where possible.** Running `to-uefi` on an already-converted system detects the UEFI state and refuses with `EXIT_NOOP`.
- **Non-destructive before the point-of-no-return.** Every phase up to "rewrite partition table" can be aborted with no change to disk.

---

## 2. CLI interface

### 2.1 to-uefi

```
lamboot-migrate to-uefi [OPTIONS]

Convert the current system (or a named disk) from legacy BIOS+MBR boot
to UEFI+GPT boot. Creates an ESP, installs a UEFI-capable bootloader,
updates fstab, creates the UEFI boot entry, and produces an install
manifest so the operation can be rolled back.

Options:
  --disk PATH         Target disk (default: auto-detect root disk)
  --esp-size MB       Size of the ESP to create (default: 512 MB)
  --method A|B|C      Proxmox-specific method selector (see §4). Default
                      is "auto" — detect if we're inside a VM and pick A.
                      Ignored on physical hosts.
  --bootloader NAME   Which UEFI-capable bootloader to install after GPT
                      conversion: "grub-efi" (default) or "lamboot". If
                      "lamboot", we invoke lamboot-install as the final
                      step. If "grub-efi", we install the distro's
                      grub-efi package first so the user has a known-
                      working fallback before they later run lamboot-
                      install.
  --cmdline STRING    Override kernel cmdline for the new BLS entry (if
                      applicable). Default: preserve current /proc/cmdline.
  --backup-dir PATH   Where to store the backup (default:
                      /var/backups/lamboot-migrate-<timestamp>/)
  --dry-run           Plan + print actions; make no changes.
  --force             Skip interactive confirmations (DANGEROUS).
  --skip-verify       Don't run the post-conversion verification suite.
  --keep-legacy-mbr   Leave MBR boot code in place alongside GPT (NOT
                      a hybrid MBR; protective MBR only). Default.
  --quiet / --verbose / --help / --version

Exit codes:
  0  EXIT_OK             Conversion complete; system will boot via UEFI
                         after reboot + firmware change.
  1  EXIT_ERROR          Fatal error before point-of-no-return; no changes
                         to disk or fstab.
  2  EXIT_PARTIAL        Conversion partially complete; some verification
                         step failed but disk is in a known state
                         recoverable via `lamboot-migrate rollback` or
                         `verify --path <backup-dir>`.
  3  EXIT_NOOP           System is already UEFI (nothing to do).
  4  EXIT_UNSAFE         Refused due to a safety check (hybrid MBR,
                         encrypted root, Windows detected, etc.). See
                         §5 top-5 failure modes.
  5  EXIT_ABORT          User declined interactive confirmation.
```

### 2.2 to-lamboot

```
lamboot-migrate to-lamboot [OPTIONS]

Migrate an existing UEFI install from another bootloader (GRUB, rEFInd,
systemd-boot) to LamBoot. Preserves the existing bootloader as a
fallback UEFI boot entry so the user can recover if LamBoot fails.

Options:
  --esp PATH          ESP mount point (default: auto-detect)
  --preserve-grub     Explicitly keep GRUB bootable (default)
  --remove-grub       Remove GRUB after LamBoot verified boot (DANGEROUS)
  --dry-run / --force / --verbose / --help / --version

Exit codes: same as to-uefi plus
  6  EXIT_NOT_UEFI      System is BIOS/MBR; use `to-uefi` first.
```

### 2.3 verify

```
lamboot-migrate verify [--all | --path PATH]

Run the post-conversion verification checklist (§9) on the current
system OR on a backup directory. Non-destructive.

Options:
  --all               Run all checks (default).
  --path PATH         Verify a backup directory produced by --backup-dir.
  --json              Emit JSON output for machine consumption.
```

### 2.4 rollback

```
lamboot-migrate rollback [--backup-dir PATH]

Revert the most recent `to-uefi` run (or a specified backup) by
restoring the original MBR partition table, original fstab, and
original /etc/default/grub. The new ESP, if created, is zeroed but
the partition slot remains on disk (needs manual `sgdisk --delete`
to reclaim the space; spec's this leaves visible after rollback).

Options:
  --backup-dir PATH   Specific backup to restore (default: most recent).
  --force             No interactive confirmation.
```

---

## 3. to-uefi pipeline (10 phases)

Each phase either completes successfully (→ next) or aborts (→ cleanup to last known-good state, exit with appropriate code).

### Phase 1 — Preflight

- **Check we're on BIOS** (`[ ! -d /sys/firmware/efi ]`). If UEFI, exit `EXIT_NOOP`.
- **Check tool availability** (§1.5 external binaries). Any missing → `EXIT_ERROR` with install hint.
- **Check disk identification** (`--disk` or auto-detect root disk via `findmnt / -o SOURCE`). Refuse if root is on LVM / RAID / dm-crypt at this version (§5 failure mode 6: too much complexity to handle safely). Explicit `EXIT_UNSAFE`.
- **Check for hybrid MBR** (Rod Smith's warning, §5). `sgdisk --print-mbr-only <disk>` detecting partitions → `EXIT_UNSAFE`.
- **Check for Windows presence** (NTFS partition with a Boot Manager signature). If present → `EXIT_UNSAFE` (we'd need hybrid MBR handling; not in v1.0 scope).
- **Check disk free space** for the planned ESP. Usually needs 512 MB unallocated at the start OR end of the disk, OR a small rearrangement plan we can compute. If no feasible plan → `EXIT_ERROR`.
- **Check for `/boot` filesystem compatibility** with the chosen `--bootloader`. For `grub-efi`: any FS works. For `lamboot`: ext4 or FAT ESP; btrfs/xfs/ZFS → warn (lamboot-install can still run, might need --with-drivers-legacy per SDS-6).

### Phase 2 — Interactive confirmation

- Print a summary: disk, partition plan, what will be backed up, what will be modified, where rollback file lives.
- If `--force` is absent, prompt `CONVERT TO UEFI: type 'yes' to proceed > `.
- Any answer other than `yes` → `EXIT_ABORT`.

### Phase 3 — Backup

- Create `--backup-dir` (default `/var/backups/lamboot-migrate-<ISO-timestamp>/`).
- Back up: raw MBR first 1 MiB (`dd if=<disk> of=<backup>/mbr-raw.bin bs=1M count=1`), partition table (`sgdisk --backup=<backup>/partition-table.gpt <disk>` — ironically, sgdisk writes GPT-format backup even for MBR disks, which is what we want for restoration), `/etc/fstab`, `/etc/default/grub` (if present), `/boot/grub/grub.cfg` (if present), EFI variables export (even on BIOS; in case of partial conversions).
- Write a `<backup>/MANIFEST.json` with: timestamp, target disk, current root UUID, planned operations, tool version.
- Every subsequent phase checks the backup directory exists before proceeding — if removed mid-conversion, abort.

### Phase 4 — MBR→GPT conversion (point of no return for the partition table)

- Run `sgdisk --mbrtogpt <disk>`. This is atomic per `sgdisk`'s semantics; either succeeds or leaves MBR intact.
- If success: protective MBR is automatically written; GPT entries mirror the MBR partition set.
- If failure: abort with the disk still on MBR. No rollback needed.

### Phase 5 — ESP creation

- Find a free range (at the end of the disk, or between existing partitions, or by shrinking the last FS if it's ext4 and has sufficient free space — `resize2fs` + `sgdisk --resize-partition`). 512 MB default; `--esp-size` customizable.
- Create partition: `sgdisk --new=<N>:<start>:<end> --typecode=<N>:EF00 --change-name=<N>:"ESP" <disk>`.
- Format: `mkfs.vfat -F 32 -n ESP /dev/<disk><N>`.
- Mount at `/mnt/lamboot-esp/` for subsequent phases.

### Phase 6 — fstab update

- Compute ESP partition's UUID (`blkid -o export /dev/<disk><N> | grep UUID=`).
- Append `UUID=<uuid> /boot/efi vfat umask=0077,shortname=winnt 0 1` to `/etc/fstab`.
- Critical: **re-check every existing fstab entry that uses `/dev/sda<N>` and rewrite to `UUID=` form.** Partition numbers change during MBR→GPT (sometimes); UUID-based mounts are stable. This is failure-mode #2 from §5.
- Back up the original fstab to `<backup>/fstab.pre-rewrite` before modifying.

### Phase 7 — Chroot prep

- Bind-mount /sys/firmware/efi/efivars into the chroot target (if this tool is running from a live-media chroot; otherwise it's the host's own).
- Bind-mount /dev, /proc, /sys, /run as appropriate.
- This is failure-mode #3 from §5. The tool handles it automatically.

### Phase 8 — Bootloader install

Branches on `--bootloader`:

- `--bootloader=grub-efi`:
  - Install distro's grub-efi package (`apt install grub-efi-amd64` / `dnf install grub2-efi-x64 shim` / `pacman -S grub efibootmgr`).
  - `grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=<distro>`. **Critical** to specify `--target`; without it `grub-install` falls back to the BIOS build and silently produces a non-UEFI binary (failure-mode #1).
  - `update-grub` / `grub2-mkconfig -o /boot/efi/EFI/<distro>/grub.cfg` to regenerate config.

- `--bootloader=lamboot`:
  - First install grub-efi per above (always ensure a fallback).
  - Then run `lamboot-install --signed` (or unsigned per user preference) to deploy LamBoot alongside.
  - Leave GRUB as the second entry in BootOrder.

### Phase 9 — UEFI boot entry

- `efibootmgr --create --disk <disk> --part <esp-partnum> --loader <path-to-grub-or-shim> --label "<distro>"` for the primary.
- Ensure `\EFI\BOOT\BOOTX64.EFI` fallback path is populated (copy grub-efi or shim into it). Failure-mode #4 guardrail.
- `efibootmgr --bootorder <new-entry>,<existing-bios-entry>,...` — leave BIOS entries present for firmware-menu fallback, but put UEFI first.

### Phase 10 — Verify (runs verify subcommand automatically unless --skip-verify)

- Run §9 checklist.
- On pass: write `<backup>/SUCCESS.flag`.
- On fail: exit `EXIT_PARTIAL`. User can inspect, fix manually, or `rollback`.

After phase 10 succeeds, instruct the user:

> Conversion complete. Reboot now and enter firmware setup to switch
> from Legacy/CSM to UEFI. The system will boot via UEFI from there.

For Proxmox VMs (§4), the instruction becomes:

> Shut down the VM. In Proxmox UI, change BIOS from SeaBIOS to OVMF
> (UEFI) and add an `efidisk0`. Start the VM. It will boot via UEFI.

---

## 4. Proxmox VM methods

### 4.1 Method A — pre-convert (preferred)

Runs inside the still-booted BIOS VM. Phases 1-10 as above. No hypervisor changes mid-run. After phase 10, user shuts down VM and switches firmware at the hypervisor.

**Advantages:** one VM state transition; reversible at any phase.
**Constraint:** VM must have free space for ESP.

### 4.2 Method B — post-convert live chroot

Boot VM into a live Linux ISO (Ubuntu Live, Fedora Live, SystemRescue). Mount the root FS, chroot. Run `lamboot-migrate to-uefi --method=B`. Phases run with the live-media kernel providing efivarfs.

**Advantages:** handles cases where the host OS can't be quiesced enough to do an in-place conversion safely (e.g. running database that doesn't tolerate fstab edits).
**Disadvantages:** requires attaching an ISO; requires reboot into live media; longer downtime.

### 4.3 Method C — add second disk as ESP

Attach a new (tiny, 1 GB) raw disk to the VM. It becomes the ESP-disk. The original root disk stays MBR+BIOS; the new disk is formatted as GPT with an ESP partition that points at GRUB in firmware DB; GRUB on the ESP reads the legacy MBR root's /boot via its ext4/btrfs driver and chains to the kernel.

**Advantages:** ZERO modification to the root disk. Safest possible. Just adding a disk.
**Disadvantages:** cosmetically weird (two-disk system where one is 1 GB); if the ESP disk is ever detached, VM can't boot. Recommended mostly for test-VM conversion (fleet unlock).

### 4.4 Method auto-selection

When `--method=auto` (default) on a Proxmox VM:
- If root disk has ≥ 512 MB free: Method A.
- Else if a second disk can be attached (tool can't verify; asks user): Method C.
- Else recommend Method B with live-media ISO.

On a physical host, `--method=A` is the only option; `--method=B|C` are VM-specific and error out.

---

## 5. Top 5 failure modes — pre-flight guardrails

Per `docs/BOOT-TOOLKIT-LANDSCAPE-2026-04-04.md §4.3`, the documented failure modes, with SDS-7's guardrail for each:

### 5.1 Failure #1: `grub-install` without `--target=x86_64-efi`

**Guardrail:** Phase 8's `grub-install` invocation is hard-coded with `--target=x86_64-efi`. The tool never calls `grub-install` without the flag. Documented as a testable post-condition: `file /boot/efi/EFI/<distro>/grubx64.efi` must report a PE32+ executable (BIOS grub produces ELF or raw binary; UEFI grub produces PE32+).

### 5.2 Failure #2: fstab uses `/dev/sda1` instead of `UUID=`

**Guardrail:** Phase 6 rewrites every `/dev/sda<N>` entry to `UUID=<uuid>` form. Phase 10 (verify) asserts no `/dev/sd*` in fstab.

### 5.3 Failure #3: Missing `/sys/firmware/efi/efivars` bind-mount during chroot

**Guardrail:** Phase 7 handles the bind-mount automatically when operating from a chroot (Method B). When operating directly on the running system (Method A), efivarfs is already mounted.

### 5.4 Failure #4: No fallback path `EFI/BOOT/BOOTX64.EFI`

**Guardrail:** Phase 9 always populates `\EFI\BOOT\BOOTX64.EFI` with a copy of the bootloader's shim/grub binary. Verify step (§9) asserts presence.

### 5.5 Failure #5: Hybrid MBR

**Guardrail:** Phase 1 (preflight) detects existing hybrid MBRs via `sgdisk --print-mbr-only` and refuses to run. Hybrid MBRs happen when earlier tools created BOTH GPT and a non-protective MBR; Rod Smith's (gdisk author) own docs call these "flaky and dangerous." We refuse.

### 5.6 Additional failure mode #6: LVM/dm-crypt root

**Guardrail:** Phase 1 detects root-on-LVM (via `findmnt /` + `lvdisplay`) or dm-crypt (via `cryptsetup status`) and refuses at `EXIT_UNSAFE`. Conversion of encrypted-root systems requires hands-on care we don't automate in v1.0.

### 5.7 Additional failure mode #7: Windows on MBR

**Guardrail:** Phase 1 scans for NTFS partitions with a Windows signature. Presence → `EXIT_UNSAFE`. Windows dual-boot + MBR→GPT requires rewriting Windows BCD, which we don't do. Documented as "manual conversion path; see `docs/LAMBOOT-MIGRATE-GUIDE.md` §Dual-boot."

---

## 6. to-lamboot subcommand

The thin wrapper. Steps:

1. Confirm UEFI state (`[ -d /sys/firmware/efi ]`; else `EXIT_NOT_UEFI`).
2. Detect current bootloader (GRUB / rEFInd / sd-boot) via ESP scanning.
3. Back up current bootloader's ESP presence + efibootmgr output to `<backup-dir>`.
4. Invoke `lamboot-install --signed --make-default` with appropriate flags.
5. Verify LamBoot is installed and is the default via `efibootmgr`.
6. Print instruction: "Reboot to test. If LamBoot fails, use the firmware boot menu to pick `<original-bootloader>` and run `lamboot-migrate rollback`."

This does NOT remove the original bootloader unless `--remove-grub` is passed. Principle: user chose a transition tool; don't strand them if LamBoot misbehaves.

---

## 7. Backup + restore discipline

### 7.1 What's in `<backup-dir>`

```
<backup-dir>/
├── MANIFEST.json                  # tool version, timestamp, actions planned + taken
├── mbr-raw.bin                    # dd of first 1 MiB of target disk
├── partition-table.gpt            # sgdisk --backup output
├── fstab.pre-rewrite              # copy of /etc/fstab before Phase 6
├── default-grub                   # copy of /etc/default/grub (if present)
├── grub.cfg                       # copy of /boot/grub/grub.cfg (if present)
├── efi-vars.json                  # efibootmgr -v | tee output
├── SUCCESS.flag                   # written iff Phase 10 passed
└── ROLLBACK.log                   # append-only log of any rollback attempts
```

### 7.2 Rollback procedure

`lamboot-migrate rollback` steps:

1. Find most recent `<backup-dir>` (or use `--backup-dir`).
2. Restore MBR: `dd if=<backup>/mbr-raw.bin of=<disk> bs=1M count=1`.
3. Restore partition table: `sgdisk --load-backup=<backup>/partition-table.gpt <disk>`. Note: this RESTORES the GPT overlay; to fully revert to MBR, we additionally zero the GPT header + secondary: `sgdisk --zap-all <disk>` THEN write the original MBR back.
4. Restore `/etc/fstab` from `<backup>/fstab.pre-rewrite`.
5. Restore `/etc/default/grub` if it existed.
6. Delete UEFI boot entries created during conversion (from `efi-vars.json` diff).
7. Append to `ROLLBACK.log`.
8. Instruct user to reboot; firmware should now pick the original BIOS boot.

### 7.3 Rollback limitations

- Does NOT recover data if ESP formatting destroyed it (sgdisk should not have touched data, but double-check).
- Does NOT unwind changes made by other tools between the to-uefi run and the rollback.
- Does NOT re-create partitions that were deleted to make ESP room (only relevant if user used `--esp-size` exceeding available unallocated + requested resize of an existing partition).

---

## 8. Distro-specific recipes

Each major distro has slight variations handled by sub-functions in the script. Consolidated:

| Distro | `--bootloader=grub-efi` installs | Post-install update | Notes |
|---|---|---|---|
| Ubuntu / Debian | `apt install grub-efi-amd64 shim-signed` | `update-grub` | shim-signed pre-installed on most |
| Fedora | `dnf install grub2-efi-x64 shim-x64` | `grub2-mkconfig -o /boot/efi/EFI/fedora/grub.cfg` | Requires `shim-x64` for SB |
| RHEL / CentOS / Rocky | same as Fedora | same | ditto |
| Arch / EndeavourOS | `pacman -S grub efibootmgr` | `grub-mkconfig -o /boot/grub/grub.cfg` | No shim in repos by default; user responsible if SB |
| openSUSE | `zypper install grub2-efi-x64 shim` | `update-bootloader` | openSUSE has its own wrapper |
| Alpine | — | — | Uses syslinux/gummiboot by default; conversion documented but not automated in v1.0 |

Distro detection via `/etc/os-release` ID field.

---

## 9. `verify` checklist

Eleven checks:

| # | Check | Command | Pass criterion |
|---|---|---|---|
| 1 | Is the system UEFI now? | `[ -d /sys/firmware/efi ]` | Exists |
| 2 | Is the disk GPT? | `sgdisk -p <disk>` output check | "GPT" token present |
| 3 | ESP partition exists and is type EF00 | `sgdisk -p <disk>` | Partition with typecode EF00 |
| 4 | ESP is mounted | `mount | grep efi` | /boot/efi or /efi mounted vfat |
| 5 | efibootmgr entries | `efibootmgr -v` | Contains entry for the new bootloader |
| 6 | Fallback path exists | `[ -f /boot/efi/EFI/BOOT/BOOTX64.EFI ]` | File exists |
| 7 | GRUB/LamBoot PE32+ binary present | `file /boot/efi/EFI/.../grubx64.efi` or `lambootx64.efi` | Says "PE32+ executable (EFI application)" |
| 8 | fstab uses UUIDs | `grep -E '^/dev/' /etc/fstab | grep -v '^#'` | No matches (all UUID= form) |
| 9 | /boot is readable | `ls /boot/vmlinuz-*` | At least one kernel found |
| 10 | Backup directory consistent | `MANIFEST.json` exists and is valid JSON | Parses + has expected keys |
| 11 | Rollback plan works (dry-run) | `lamboot-migrate rollback --dry-run` | Prints plan; no exit error |

Each check produces a structured `{name, pass, message}` output; `--json` mode emits JSON array for automated consumption.

---

## 10. Test plan

### 10.1 VM conversion matrix

Covering the fleet's SeaBIOS VMs that currently can't be tested against LamBoot. Per `docs/FLEET-TEST-PLAN-2026-04-21.md`:

| VMID | Distro | Method | Expected outcome |
|---|---|---|---|
| 102 | openSUSE Tumbleweed | A (pre-convert) | Convert in-place, boot UEFI |
| 104 | Fedora GNOME | A | ditto |
| 110 | RHEL 10 | A | ditto |
| 121 | Ubuntu 24.04 Sway | A | ditto |
| 126 | Fedora 44 KDE beta | A | ditto |
| 105 | Arch niri | C (add-disk) | Arch with odd partition layout — test C variant |
| 119 | Manjaro KDE | A | ditto |
| 129 | Ubuntu 25 GNOME | A | ditto |
| 130 | openSUSE Leap 16 | A | ditto |
| 131 | Arch GNOME | A | ditto |

Pass criterion: after conversion + VM-firmware switch to OVMF + reboot, the system boots into its OS via UEFI. `lamboot-migrate verify` returns all 11 checks passing.

### 10.2 Physical-hardware-proxy test

Use a QEMU VM configured with a large disk + low-free-space partition to simulate edge cases: no free space at end of disk → requires resize; hybrid MBR (synthetically constructed) → must refuse.

### 10.3 Failure-mode tests

1. Run `to-uefi` on an already-UEFI system → `EXIT_NOOP`.
2. Run `to-uefi` on a BIOS system with a hybrid MBR → `EXIT_UNSAFE` before any modification.
3. Run `to-uefi` on a BIOS system with Windows/NTFS partition → `EXIT_UNSAFE`.
4. Run `to-uefi --bootloader=grub-efi` and verify `file grubx64.efi` reports PE32+ (failure-mode #1 guardrail).
5. Run `to-uefi`, interrupt between Phase 4 and Phase 5 (kill -9); run `rollback` → MBR restored intact, fstab untouched (Phase 6 hadn't run yet).
6. Run `to-uefi` through to Phase 10 success, then run `rollback` → system reverts to BIOS-bootable (verified after firmware switch back to CSM).

### 10.4 to-lamboot tests

1. EndeavourOS VM 123 with systemd-boot: run `to-lamboot`; verify LamBoot is default, sd-boot still present as fallback.
2. Fedora VM 201 with GRUB: run `to-lamboot`; verify LamBoot + GRUB coexistence.
3. Run `to-lamboot` on a BIOS VM: `EXIT_NOT_UEFI`.

### 10.5 Regression

Existing fleet `--update` scenarios (`lamboot-install --update` on v0.8.x ESPs) continue working after `to-lamboot` has been used.

---

## 11. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| `sgdisk --mbrtogpt` corner-case failures on unusual partition layouts | Medium | Widespread disk layouts tested; extreme edge-case disks fall under "manual conversion" documentation. |
| Hypervisor firmware switch is outside our control | Low | Tool instructs user; Proxmox/libvirt-specific wrappers (`lamboot-migrate-proxmox helper`) are a future enhancement, not v1.0. |
| `update-grub` post-install can regenerate grub.cfg in a way that breaks the ESP entry | Low | Verify step (§9) re-runs after Phase 8; catches. |
| Fstab rewrite could break non-root mounts | Low | Backup exists; rollback restores. |
| User runs tool while system is booted from the target disk's MBR — we're reshaping the disk we're running from | Medium | Partition-table changes are atomic; kernel reads partition layout at mount time; as long as no process re-reads the partition table mid-run, we're safe. Tool uses `partprobe` deliberately AFTER Phase 5. |
| Disk identifier (/dev/sda) changes during conversion | Low | UUID-based fstab rewrite (Phase 6) handles this. |
| Method C (add-ESP-disk) depends on user attaching the disk; hard to automate | Low | Documented prerequisite; tool exits with clear "attach disk first" message if disk isn't visible. |
| Tool becomes stale when distros change packaging | Medium | Distro-recipe table is localized (§8); updates are CHANGELOG entries, not architectural changes. |
| Proxmox API hook to auto-switch SeaBIOS→OVMF is unimplemented | Low | v1.1+ enhancement. User currently does it in Proxmox UI post-conversion. |

---

## 12. Acceptance criteria

SDS-7 implementation is complete when all of the following are true:

- [ ] `tools/lamboot-migrate` exists, ~800 lines of bash, `#!/bin/bash` header, `set -euo pipefail`.
- [ ] `package-release.sh` copies `tools/lamboot-migrate` into the release tarball alongside `lamboot-install`.
- [ ] All four subcommands (`to-uefi`, `to-lamboot`, `verify`, `rollback`) work per §2.
- [ ] §3 ten-phase pipeline implemented with per-phase error handling.
- [ ] §5 top-5 (+2 additional) failure modes detected and refused in Phase 1.
- [ ] §7 backup discipline: every `to-uefi` run produces a complete `<backup-dir>` with all files from §7.1.
- [ ] §9 verify subcommand runs all 11 checks.
- [ ] `docs/LAMBOOT-MIGRATE-GUIDE.md` authored as the user-facing walkthrough.
- [ ] `docs/LAMBOOT-TOOLS-OVERVIEW.md` gets a `lamboot-migrate` section.
- [ ] `docs/TROUBLESHOOTING-GUIDE.md`'s reference to `lamboot-migrate` now points at a working tool.
- [ ] §10.1 matrix: at least 5 VM conversions succeed, verify-all passes.
- [ ] §10.3 failure-mode refusals work as spec'd.
- [ ] Shellcheck passes on the script.
- [ ] Pre-commit (for bash files: shellcheck) integrated.
- [ ] `CHANGELOG.md` entry.

---

## 13. Sign-off

Implementation-ready when:
- Founder review + acknowledgement.
- No open question in §11 above "Medium" severity.

---

## Appendix A — Example session transcript

```
$ sudo lamboot-migrate to-uefi --disk /dev/sda --esp-size 512

lamboot-migrate 0.9.0

══ Phase 1: Preflight ══
✓ System is currently BIOS (as expected)
✓ All tools present (sgdisk 1.0.10, mkfs.vfat 4.2, efibootmgr 18, blkid 2.39, lsblk 2.39, rsync 3.3.0)
✓ Target disk: /dev/sda (240 GB NVMe)
✓ Root filesystem: /dev/sda1 ext4, UUID=bea1190c-...
✓ No hybrid MBR detected
✓ No Windows/NTFS partitions detected
✓ No LVM or dm-crypt on root
✓ 2.1 GB unallocated at end of disk (ample for 512 MB ESP)

══ Phase 2: Confirmation ══
Plan:
  - Backup MBR + partition table + fstab + grub config to
    /var/backups/lamboot-migrate-2026-04-22T14-37-22/
  - Convert MBR to GPT on /dev/sda (in place)
  - Create ESP: /dev/sda2, 512 MB, FAT32, typecode EF00
  - Mount ESP at /boot/efi
  - Rewrite fstab to UUID= form
  - Install grub-efi-amd64
  - grub-install --target=x86_64-efi
  - Create UEFI boot entry
  - Verify

CONVERT TO UEFI: type 'yes' to proceed > yes

══ Phase 3: Backup ══
✓ Backup directory: /var/backups/lamboot-migrate-2026-04-22T14-37-22/
…
══ Phase 10: Verify ══
✓ All 11 verification checks passed
✓ Conversion complete. Reboot and switch firmware to UEFI.

Backup retained at: /var/backups/lamboot-migrate-2026-04-22T14-37-22/
Rollback available via: sudo lamboot-migrate rollback
```

## 14. Deviations from existing v0.1.0 — reconciliation table

The existing `lamboot-migrate` v0.1.0 was written on 2026-04-05 ahead of
this SDS. The shape of that implementation is mostly congruent with the
spec but has a few gaps and one addition. This section enumerates them so
the gap-close work is a bounded edit, not a rewrite.

### 14.0 Resolution status — 2026-04-22

**Gap-close work completed in `lamboot-tools-dev` Session C.** The resulting
release is tagged `lamboot-migrate v1.0.0` and ships in toolkit v0.2.0 as
both a subpackage of `lamboot-tools` and a standalone `lamboot-migrate` RPM
(dual publication per toolkit spec R22). Detailed implementation log:
`~/lamboot-tools-dev/docs/SDS-7-GAP-ANALYSIS.md`.

Updated status markers in the tables below use these symbols:

- **RESOLVED** — implementation landed in `lamboot-migrate v1.0.0`
- **RESOLVED+** — implementation went beyond the spec (e.g. `--remove-grub`)
- **KEPT** — v0.1.0 behavior preserved per spec §14.7

Original gap-descriptions retained verbatim below for historical context
(they were the work-plan the implementer followed).

### 14.1 Subcommand deltas

| Subcommand | SDS-7 spec | v0.1.0 | Action | v1.0.0 |
|---|---|---|---|---|
| `to-uefi` | ✓ defined | ✓ implemented (single flow) | Enhance (see §14.2) | **RESOLVED+** — `--remove-grub` distro-aware cleanup added beyond spec |
| `to-lamboot` | ✓ defined | ✓ implemented | Enhance — currently a direct hand-off, needs distro-specific recipes | **RESOLVED** — per-distro recipes + generic fallback |
| `verify` | ✓ defined, 11 checks | ✗ implicit only | **Add as new subcommand** | **RESOLVED** — 11-check subcommand; callable standalone or auto-invoked from `to-uefi` Phase 10 |
| `rollback` | ✓ defined, consumes backup dir | ✗ — backup files written but no rollback command | **Add as new subcommand** | **RESOLVED** — consumes `/var/backups/lamboot-migrate-<ts>/`; manifest-driven reverse of each phase |
| `status` | ✗ not in spec | ✓ implemented | **KEEP** — no reason to remove; add to spec as §2.5 in a future amendment | **KEPT** — unchanged |

### 14.2 `to-uefi` flow deltas

| Spec §3 phase | v0.1.0 implementation | Action | v1.0.0 |
|---|---|---|---|
| Phase 1 — Preflight | `preflight_to_uefi()` function, basic | Augment with top-5 guardrails from §5 | **RESOLVED** — 7 guardrails wired (§5 top-5 + 2 additions) |
| Phase 2 — Interactive confirm | Inline `confirm()` calls | Formalize as separate phase with clear summary | **RESOLVED** — labelled phase; clear pre-mutation summary |
| Phase 3 — Backup | MBR + sfdisk dump to `/tmp/lamboot-migrate-backup-*` | Change backup dir to `/var/backups/lamboot-migrate-<ts>/` per spec; include fstab + bootloader configs | **RESOLVED** — `/var/backups/lamboot-migrate-<ts>/` with MBR + sfdisk + fstab + bootloader configs + rollback manifest |
| Phase 4 — MBR→GPT | `sgdisk -g` call with rollback hint | OK; formalize as labelled phase boundary | **RESOLVED** |
| Phase 5 — ESP creation | Inline logic | Formalize | **RESOLVED** |
| Phase 6 — fstab update | Present | Formalize as phase | **RESOLVED** — UUID-based entries |
| Phase 7 — Chroot prep | Not explicit | **Add** — mount proc/sys/dev, bind-mount ESP | **RESOLVED** — bind-mount + chroot exec wrapper |
| Phase 8 — Bootloader install | Calls `grub-install` generically | **Add distro detection** — per-distro install paths per §3 Phase 8 | **RESOLVED** — 5 distro recipes (Ubuntu 22.04/24.04, Debian 12/13, Fedora 39+, Arch, openSUSE Tumbleweed, Alpine 3.19+) + generic `grub-install` fallback |
| Phase 9 — UEFI entry | `efibootmgr -c` | OK; formalize | **RESOLVED** |
| Phase 10 — Verify | Not separate | **Call the new `verify` subcommand** at end of flow | **RESOLVED** — auto-invokes verify with `--from-to-uefi` context |

### 14.3 Proxmox methods

v0.1.0 treats to-uefi as a single generic flow. SDS-7 §4 defines three
methods (A: pre-convert, B: post-convert live chroot, C: add second disk
as ESP) with an auto-select heuristic.

Action: add `--proxmox-method={auto,a,b,c}` flag. Auto-select per §4.4 when
unspecified. Each method is a variant of the 10-phase flow, not a new
codepath — most phases are shared.

**v1.0.0 status: RESOLVED.** `--proxmox-method={auto,a,b,c}` wired; §4.4
auto-select heuristic implemented; all three methods share the main
10-phase flow.

### 14.4 Top-5 failure-mode guardrails

v0.1.0 preflight checks are generic. SDS-7 §5 mandates five specific
guardrails:

1. `grub-install --target=x86_64-efi` detection
2. fstab uses `UUID=` not `/dev/sda1`
3. Kernel has `CONFIG_EFI=y`
4. Disk has ≥ 513 MiB free for ESP + minimum backup
5. No existing ESP on target disk (would collide)

Action: each guardrail as a separate check function; all must pass (or
`--force` bypass with explicit log line). Integrate into Phase 1.

**v1.0.0 status: RESOLVED+.** 7 guardrails landed (5 from spec + 2
additions: `/boot` writability, EFI runtime services available).
`--force` bypass logs each skipped guardrail with its check name.

### 14.5 Distro-specific recipes (§3 Phase 8)

v0.1.0 assumes `grub-install` is present. SDS-7 §3 Phase 8 specifies
per-distro recipes for Ubuntu 22.04/24.04, Debian 12/13, Fedora 39+, Arch,
openSUSE Tumbleweed, Alpine 3.19+.

Action: lookup table driven by `/etc/os-release` ID. Each recipe is a
small bash function; fallback to generic `grub-install` path if the distro
isn't in the table.

**v1.0.0 status: RESOLVED.** Recipes shipped for Ubuntu 22.04/24.04, Debian
12/13, Fedora 39+, Arch, openSUSE Tumbleweed, Alpine 3.19+. Generic
`grub-install` fallback lands untested distros in a safe path.

### 14.6 Rollback subcommand (§2.4)

Consumes the backup dir written in Phase 3:
```
lamboot-migrate rollback [--backup-dir DIR] [--confirm]
```

If `--backup-dir` unspecified, picks the most recent backup in
`/var/backups/lamboot-migrate-*`. Restores MBR via `dd if=mbr.bin of=DISK`,
re-applies sfdisk dump, restores fstab, removes ESP entry from efibootmgr,
clears the UEFI BootOrder entry LamBoot added. Non-zero exit if any step
fails; each success is atomic (rollback-of-rollback is NOT supported —
spec calls it out).

**v1.0.0 status: RESOLVED.** Rollback manifest (`rollback.json`) written
into each backup dir by `to-uefi` Phase 3 records the exact reverse-order
operations. Rollback reads the manifest and executes in reverse;
rollback-of-rollback remains unsupported per spec.

### 14.7 Not in SDS-7 but in v0.1.0 — `status` subcommand

Prints current boot configuration (boot mode, partition table type, ESP
presence, bootloader inventory, UEFI boot entries). Useful for
pre-migration fact-finding. **Keep as-is.** Future SDS-7 amendment will
add §2.5 describing it.

### 14.8 Work estimate

Gap-close estimated at 8–12 hours of bash work given the existing v0.1.0
structure:

- 2h: `verify` subcommand (11 checks, refactored from existing inline
  verification)
- 2h: `rollback` subcommand (consumes existing backup files; needs new
  logic to reverse each step)
- 2h: 10-phase formalization (mostly renaming existing step comments to
  labelled phase boundaries + extract helpers)
- 2h: Three Proxmox methods (auto-select + per-method flow)
- 2h: Top-5 failure guardrails
- 1–2h: Distro-specific bootloader install recipes

**Actual:** the gap-close landed in `lamboot-tools-dev` Session C on
2026-04-22 in approximately one working sitting, producing the v1.0.0 tag
with 7 guardrails + 5 distro recipes + three Proxmox methods + verify +
rollback + `--remove-grub` (beyond spec).

Delivery target: v1.0 of `lamboot-migrate` tool, versioned independently
of LamBoot itself. Tag `lamboot-migrate-v1.0.0` in `lamboot-tools-dev`
repo when done.

**v1.0.0 status: DONE.** Ships in `lamboot-tools v0.2.0` as the
`lamboot-migrate` subpackage and the dual-published `lamboot-migrate`
standalone RPM (per toolkit spec R22).

### 14.9 Authoring location

The gap-analysis file `docs/SDS-7-GAP-ANALYSIS.md` lives in
`lamboot-tools-dev` (not `lamboot-dev`), so the implementation notes are
with the implementation. This SDS in `lamboot-dev` remains the
authoritative design source; the gap-analysis is the TODO list for the
implementer.

**Sign-off (2026-04-22):** all §14.1–§14.7 gap items resolved; see
`~/lamboot-tools-dev/docs/SDS-7-GAP-ANALYSIS.md` for per-item
implementation notes and `~/lamboot-tools-dev/docs/AUDIT-2026-04-22.md` §9c
for the full-implementation remediation that added `--remove-grub` beyond
spec.

---

## Appendix B — References

- `docs/BOOT-TOOLKIT-LANDSCAPE-2026-04-04.md` §4 — original design this SDS formalizes.
- `docs/TROUBLESHOOTING-GUIDE.md` — advertises `lamboot-migrate` to users; now points at a real tool.
- `docs/FLEET-TEST-PLAN-2026-04-21.md` — the 14 SeaBIOS VMs that become testable after this tool ships.
- `docs/specs/SPEC-LAMBOOT-INSTALL.md` — companion tool; handoff target of `to-lamboot`.
- sgdisk (gptfdisk): https://www.rodsbooks.com/gdisk/
- Rod Smith on hybrid MBRs: https://www.rodsbooks.com/gdisk/hybrid.html
- Proxmox BIOS-to-UEFI thread: https://forum.proxmox.com/threads/convert-ubuntu-vm-from-seabios-to-ovmf.132657/
- Arch Wiki ESP: https://wiki.archlinux.org/title/EFI_system_partition
