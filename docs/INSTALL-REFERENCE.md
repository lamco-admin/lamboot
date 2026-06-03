# lamboot-install Command Reference

**Applies to:** lamboot-install as shipped with LamBoot v0.12.0
**Installer protocol:** v1 (see `--protocol-version`, `--capabilities`)

`lamboot-install` installs, updates, or removes the LamBoot UEFI
bootloader on the local system, or stages it into a target root for a
distro installer. It is a single bash script (bash 4.0+) and is the
only supported way to put LamBoot onto an ESP — it owns the ESP file
layout, the UEFI boot entry, BLS-entry generation, the Secure Boot
shim chain, and the systemd integration.

This document is the authoritative reference for the command-line
surface. For task walkthroughs see the [User Guide](USER-GUIDE.md) and
[Secure Boot Deployment Guide](SECURE-BOOT-DEPLOYMENT.md); for the
machine-readable contract consumed by distro installers see
`--capabilities` and the installer-protocol spec.

---

## Synopsis

```
lamboot-install [OPTIONS]
```

There is no positional command. The operation is selected by flags:

| Operation | Selector | Notes |
|---|---|---|
| Install (default) | *(no operation flag)* | Fresh install or re-install |
| Update | `--update` | Preserve config; refresh binary + entries |
| Remove | `--remove` | Reads the install manifest |
| Refresh (hook) | `--refresh` | BLS regen only; called by kernel hooks |
| Repair BLS | `--repair-bls` | Force-rebuild ESP BLS entries |

`--capabilities`, `--protocol-version`, `--version`, and `--help` are
informational short-circuits: they print and exit without touching the
system.

---

## Quick reference — all options

Grouped by purpose. Every long option the parser accepts is listed.

### Operation selectors

| Flag | Effect |
|---|---|
| *(none)* | Install (or re-install) LamBoot |
| `--update` | Update an existing install; preserve `policy.toml`; skip unchanged files (exit `3` if nothing changed) |
| `--remove` | Remove LamBoot using `.install-manifest`; deletes the UEFI entry and disables services |
| `--refresh` | Hook-callback mode: ensure every `/boot/vmlinuz-*` has an ESP BLS entry, then exit. **Additive** — never modifies existing entries. Not for direct operator use |
| `--repair-bls` | Force-rebuild ESP BLS entries: re-derive cmdline, overwrite all, prune removed kernels. Implies `--proxmox-host` + `--refresh` |

### Target / ESP

| Flag | Effect |
|---|---|
| `--esp PATH` | Override ESP auto-detection with an explicit mount point |
| `--root PATH` | Operate against a chroot/target root (distro-installer use). NVRAM ops defer to first boot; `--fallback` auto-enables (see below). `--root=PATH` also accepted |

### Boot entry / default

| Flag | Effect |
|---|---|
| `--set-default` / `--make-default` | Make LamBoot the default (first in `BootOrder`). This is already the install default; retained for explicit scripts |
| `--no-make-default` | Do **not** promote LamBoot. Prior default stays first; LamBoot is appended **last** in `BootOrder` |
| `--no-efi-entry` | Do not create a UEFI boot entry (file copy only) |
| `--fallback` | Also install at `\EFI\BOOT\BOOT{X64,AA64}.EFI` (removable-media path). Auto-enabled under `--root` |
| `--no-fallback` | Suppress the `--root` fallback auto-enable. Resulting install has no firmware-visible path on first boot unless arranged externally |
| `--replace` | Replace an existing bootloader (backup + make-default). Implies `--set-default` |

### Secure Boot

| Flag | Effect |
|---|---|
| `--signed` | Deploy the pre-signed binary. Required for Secure Boot |
| `--no-shim` | Skip shim-chain setup even when SB is on. **Requires a firmware-DB-signed kernel** — pair with `--kernel-firmware-db-signed`. Stock MOK-chained distro kernels will fail to load under LamBoot without shim |
| `--no-mok` | Skip the MOK-enrollment prompt. Shim is still deployed; trust relies on a pre-enrolled MokList |
| `--kernel-firmware-db-signed` | Assert the kernel is signed by a cert in firmware DB. Unblocks `--no-shim` under SB. Rare |

### Filesystem drivers (SDS-6 policy)

| Flag | Effect |
|---|---|
| `--with-drivers-legacy=MODE` | Legacy UEFI FS-driver install policy. `auto` (default): install a driver only for a `/boot` filesystem LamBoot does **not** natively cover — ext2/3/4 are skipped (native via ext4-view), btrfs/xfs/ntfs/zfs/f2fs/iso9660 installed when applicable. `all`: install every applicable driver (v0.8.3 behavior). `none`: install no drivers |
| `--with-drivers` | Alias for `--with-drivers-legacy=all` |
| `--with-modules` | Install diagnostic modules to `EFI/LamBoot/modules/` |

> On `--update` in `auto` mode, the installer **prunes** previously
> deployed `ext{2,3,4}_*.efi` drivers from the ESP — they are
> redundant under the native backend and harmful under Secure Boot
> (loading any UEFI FS driver makes shim 15.8 uninstall ShimLock). Use
> `--with-drivers-legacy=all` on update to keep them for A/B testing.

### Removal modifiers

| Flag | Effect |
|---|---|
| `--keep-entries` | With `--remove`: keep generated BLS entries |
| `--keep-logs` | With `--remove`: keep `boot.log` / `boot.json` / `audit.log` / `error.json` forensic data under `reports/` |

### Proxmox VE host modes

See [Proxmox host install](#proxmox-ve-host-install) below for the path
framework.

| Flag | Effect |
|---|---|
| `--proxmox-host` | Coexist install (PATH C). LamBoot deploys alongside Proxmox shim+GRUB; a chainload menuentry is added to `/etc/grub.d/40_custom`; GRUB stays default. No BLS generation, no kernel hooks, no NVRAM displacement |
| `--replace-grub` | PATH A subset. Adds BLS-entry generation + `/etc/kernel/cmdline` derivation from `/etc/default/grub` + the `00-lamboot-cmdline-sync` kernel hook. Implies `--proxmox-host` |

### Installer protocol (v1) — for distro installers

| Flag | Effect |
|---|---|
| `--protocol-version` | Print the protocol version integer and exit |
| `--capabilities` | Print the capability JSON (filesystems, signing modes, SB, flags, exit codes) and exit. Pure read; no privileged ops |
| `--json` | Emit structured JSON events on stdout instead of human text |
| `--no-prompt` | Non-interactive: never prompt; exit with a documented code where input would be required |
| `--capcheck-json PATH` | Consume a `lamboot-capcheck` audit JSON (schema v1) for install hints (driver selection, SB setup, warnings). No subprocess call; capcheck is not a runtime dependency. Does not override explicit flags. `--capcheck-json=PATH` also accepted |

### Toolkit recommendation

| Flag | Effect |
|---|---|
| `--install-toolkit` | Print lamboot-tools install guidance at the end (non-interactive) |
| `--no-install-toolkit` | Suppress the recommendation. (Default: prompt `[y/N]` on a TTY; skip otherwise) |

### Output / safety / info

| Flag | Effect |
|---|---|
| `--dry-run` | Show what would happen without doing it |
| `--force` | Skip safety checks |
| `--no-bls` | Do not generate BLS entries (use existing ones) |
| `--quiet` | Minimal output (mutually exclusive with `--verbose`) |
| `--verbose` | Detailed output |
| `--version` | Print `lamboot-install <version>` and exit |
| `--help`, `-h` | Print usage and exit |

---

## Mutual exclusions

The parser rejects these combinations with a fatal error:

- `--remove` with `--fallback`
- `--remove` with `--update`
- `--quiet` with `--verbose`
- `--refresh` with any of `--remove`, `--update`, `--replace`

`--remove` silently disables `--set-default` (promoting a removed
loader is meaningless). The Proxmox-host modes adjust defaults
automatically (see below).

---

## Exit codes

The installer-protocol v1 vocabulary. Codes 0–3 retain their
historical meaning; 4–7 were added for installer integration.

| Code | Name | Meaning |
|---|---|---|
| 0 | `EXIT_OK` | Success |
| 1 | `EXIT_ERROR` | Fatal error — installation failed |
| 2 | `EXIT_PARTIAL` | Partial success — some steps completed, some failed (e.g. deferred NVRAM under `--root`) |
| 3 | `EXIT_NOOP` | Nothing to do — already up to date (`--update`) |
| 4 | `EXIT_UNSAFE` | Refused on safety grounds; `--force` may override |
| 5 | `EXIT_ABORT` | Operator declined a prompt |
| 6 | `EXIT_NOT_APPLICABLE` | Operation does not apply to this system |
| 7 | `EXIT_PREREQUISITE_MISSING` | A required tool (e.g. efibootmgr) is absent |

---

## Installation phases

A standard install runs these phases in order. Proxmox-host and
`--root` modes insert or skip phases as noted.

| Phase | Name | What it does |
|---|---|---|
| 1 | Detect environment | Arch, ESP mount, distro, existing install, ESP free space (≥ 2 MB) |
| 2 | Assess drivers | Determine whether a legacy FS driver is needed for `/boot` per the SDS-6 policy |
| 3 | Discover entries | Inventory existing BLS entries + installed kernels in `/boot` |
| 3b | Backup & migrate | Back up a prior bootloader / migrate state when relevant |
| 4 | Install files | Deploy binary (canonical name; signed variant under `--signed`), drivers, modules, `policy.toml` (no-clobber → `policy.toml.new`). Atomic writes; tracked in `.install-manifest` |
| 4b | Proxmox GRUB integration | *(Proxmox-host)* add the `40_custom` chainload menuentry; write `/etc/kernel/cmdline` on PATH A |
| 4c | Chroot initramfs fixup | *(`--root` on mkinitcpio distros)* add `lvm2` / `sd-encrypt` HOOKS as needed, regenerate initramfs |
| 5 | Generate BLS | Write per-kernel BLS `.conf` to the ESP `loader/entries/`; gap-fill only what is missing |
| 5b | Proxmox BLS backfill | *(PATH A)* retire the legacy root-fs `/boot/loader/entries` scheme |
| 6 | UEFI boot entry | Create the `Boot####` entry; set/preserve `BootOrder` per `--*-default`; deferred to first boot under `--root` |
| 7 | Systemd integration | Install + enable `lamboot-mark-success.service`; install the `90-lamboot.install` kernel-install plugin and Debian/Ubuntu hooks |
| 7b | Proxmox hooks | *(Proxmox-host)* marker file, observability units, cmdline-sync hook (PATH A) |
| 8 | Verify | Binary present, UEFI entry present, every BLS entry's kernel + initrd resolve, coverage + hook checks |
| 8b | Proxmox drift check | *(Proxmox-host)* compare cmdline against `/etc/default/grub` |
| 9 | Toolkit prompt | Recommend lamboot-tools (TTY prompt / `--install-toolkit` / skip) |

Under Secure Boot, Phase 4 also deploys the shim and places LamBoot at
shim's chainload-target name(s) — `grubx64.efi` plus the shim-embedded
name (e.g. SUSE `grub.efi`). See SPEC-LAMBOOT-INSTALL §1.2.

---

## Scenarios

### Homelab, Secure Boot off

```sh
sudo lamboot-install
```

Native install, LamBoot promoted to default. No signing, no shim.

### Stock distro, Secure Boot on (shim + MOK)

```sh
sudo lamboot-install --signed
```

Deploys the signed binary behind the distro shim and stages MOK
enrollment (a MokManager prompt appears on the next reboot). This is
the common SB path for Ubuntu/Debian/Fedora/openSUSE whose kernels are
MOK-chained.

### Trial install alongside the existing loader

```sh
sudo lamboot-install --no-make-default
```

LamBoot is installed and appended **last** in `BootOrder`; the existing
default still boots. Select LamBoot from the firmware menu to try it.

### Distro-installer integration (chroot)

```sh
lamboot-install --root /mnt --signed --no-prompt --json
```

Operates against the target at `/mnt`. NVRAM writes defer to first
boot; `--fallback` auto-enables so the firmware can find LamBoot on
that first boot. JSON events stream to stdout for the installer to
parse. Typically exits `2` (partial) to signal the deferred NVRAM step.

### Proxmox host, coexist (recommended first deployment)

```sh
sudo lamboot-install --proxmox-host
```

PATH C. LamBoot installs alongside GRUB; pick "LamBoot (chainload)" at
the GRUB menu or `grub-reboot` for a one-shot test. Non-destructive;
rollback is `--remove`.

### Proxmox host, LamBoot owns the menu

```sh
sudo lamboot-install --replace-grub
```

PATH A. Run only after `--proxmox-host` has validated. Generates BLS
entries for every Proxmox kernel and installs the cmdline-sync hook.

### Preview any of the above

```sh
sudo lamboot-install --update --signed --dry-run
```

`--dry-run` prints the planned actions without writing. Run it before
any real install/update on a production host.

---

## Proxmox VE host install

Three integration paths, in increasing order of how much of the boot
chain LamBoot owns:

| Path | Flag | LamBoot role | Boot chain | Rollback |
|---|---|---|---|---|
| C (coexist) | `--proxmox-host` | Opt-in via GRUB menu | GRUB → LamBoot (chainload) | `--remove` strips the menuentry |
| A (replace-grub) | `--replace-grub` | Owns BLS entries + cmdline | GRUB chainloads LamBoot; LamBoot reads `/boot` via lvm2-view+ext4-view | `--remove` |
| B (proxmox-boot-tool) | *(runtime-detected via `--refresh`)* | ESP-mirror entries | proxmox-boot-tool → LamBoot | n/a |

PATH C is the safe first deployment on any Proxmox host. Escalate to
PATH A only after C boots cleanly. The `dpkg-divert` of `grubx64.efi`
(making the firmware reach LamBoot without a GRUB menu pick) is future
work; today both paths keep the `40_custom` chainload as the entry
point.

Kernel-update integration on a Proxmox host is handled by the
`zzzz-lamboot-proxmox` dpkg trigger plus the `--refresh` /
`--repair-bls` callbacks, gated by `/etc/lamboot/proxmox-host.conf`.

---

## ESP detection

When `--esp` is not given, the ESP is found in priority order:

1. `--esp PATH` (explicit override)
2. `findmnt --target /boot/efi`
3. `findmnt --target /efi`
4. `lsblk` scan for the ESP partition-type GUID
   `c12a7328-f81f-11d2-ba4b-00a0c93ec93b`

Under `--root`, a vfat mountpoint at `<root>/boot` is accepted as the
ESP even before an `EFI/` directory exists (fresh-install flow), and
the directory is created.

The detected ESP must have at least 2 MB free.

---

## Configuration preservation

**On `--update`:** `policy.toml` is never overwritten — new defaults
land at `policy.toml.new` for reference. Other files are updated by
SHA-256 comparison; unchanged files are skipped (exit `3` if nothing
changed).

**On `--remove`:** every file in `.install-manifest` is removed, the
UEFI entry is deleted, services are disabled, empty ESP directories are
pruned. BLS entries and forensic logs are removed unless
`--keep-entries` / `--keep-logs` are given.

---

## Requirements

| Requirement | Usage |
|---|---|
| bash 4.0+ | Script execution (associative arrays, arithmetic). Not dash/ash/busybox/bash 3.x |
| GNU coreutils **or** uutils | File ops (cp, mv, stat, sha256sum, …) |
| util-linux | `findmnt`, `lsblk`, `mount`, `mountpoint` |
| efibootmgr | UEFI boot-entry management (checked at runtime) |
| systemctl *(optional)* | Service enable/disable; skipped if absent |
| file *(optional)* | Arch kernel-version detection; skipped if absent |

Tested on Fedora 43, Debian 13, Ubuntu 24.04, Arch Linux, openSUSE
Tumbleweed, Alpine (with `apk add bash`). Not supported on non-Linux
systems, RHEL/CentOS 6 (bash 3.x), or environments without util-linux.

---

## The capabilities contract

`lamboot-install --capabilities` emits a JSON object describing what
this installer supports — filesystems (native vs via-driver), signing
modes, Secure Boot shim search paths, the trust-log token vocabulary,
the command/flag set, and the exit-code map. Downstream installers
(archinstall, Calamares, openSUSE script-contract) negotiate against it
via `--protocol-version`. It is generated from the same build-time
constants the installer uses, so it cannot drift from the real
behavior — treat it as the machine-readable companion to this
document.

---

## See also

- [User Guide](USER-GUIDE.md) — getting started, daily use
- [Configuration Guide](CONFIGURATION-GUIDE.md) — `policy.toml` reference
- [Secure Boot Deployment Guide](SECURE-BOOT-DEPLOYMENT.md) — trust configurations
- [MOK Enrollment Guide](MOK-ENROLLMENT-GUIDE.md) — MokManager walkthrough
- [Proxmox Guide](PROXMOX-GUIDE.md) — host + guest integration
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — common problems
