# LamBoot Tools — Companion Toolkit

**Toolkit version:** 0.2.0 (release candidate; see
`~/lamboot-tools-dev/docs/STATUS-2026-04-22-SESSION-HANDOVER.md` for state)
**LamBoot version this doc tracks:** 0.8.3 (shipped) → 0.8.4 (coordinated
release pending; see `docs/CROSS-REPO-STATUS.md`)
**Updated:** 2026-04-22

---

## Overview

LamBoot Tools is a companion toolkit for diagnosing, repairing, backing up,
and migrating Linux UEFI boot configurations. It ships from the
[`lamboot-tools-dev`](https://github.com/lamco-admin/lamboot-tools-dev) source
repo (development) as **three RPM subpackages** built from one unified spec,
plus a standalone `lamboot-migrate` dual-publication for operators who only
need the BIOS→UEFI migrator.

The toolkit is **CLI-only, permanent** — no GUI is planned at any version.
Audience is intentionally broader than LamBoot users: a Linux admin with a
broken ESP, a fleet operator, or someone who searched for "bios to uefi
tool" should all land here productively. The toolkit is LamBoot's gift to
the broader UEFI boot ecosystem, with LamBoot-specific subcommands where
useful.

Authoritative product spec: `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md`.

---

## Packaging

| RPM subpackage | Ships | Audience |
|---|---|---|
| `lamboot-tools` (core) | 10 tools: `lamboot-toolkit` dispatcher + `lamboot-diagnose` + `lamboot-esp` + `lamboot-backup` + `lamboot-repair` + `lamboot-doctor` + `lamboot-uki-build` + `lamboot-signing-keys` + `lamboot-migrate` + `lamboot-inspect` (mirror) | Any Linux UEFI admin |
| `lamboot-migrate` (standalone) | `lamboot-migrate` alone | Operators who want only the migrator without the wider toolkit; dual-published per toolkit spec R22 |
| `lamboot-toolkit-pve` | 4 tools: `lamboot-pve-setup` + `lamboot-pve-fleet` + `lamboot-pve-monitor` (mirror of `lamboot-monitor.py` in this repo) + `lamboot-pve-ovmf-vars` (mirror of `build-ovmf-vars.sh` in this repo) | Proxmox VE host operators managing LamBoot VMs |

**Same tarball, three RPMs.** Built from one `lamboot-tools.spec`. Install
layout: `/usr/bin/lamboot-*`, shared library at `/usr/lib/lamboot-tools/`,
man pages at `/usr/share/man/man1/lamboot-*.1` + `lamboot-tools(7)` +
`lamboot-tools-schema(5)`.

Distribution channels for v0.2.0:

- **Copr** (Fedora/EPEL): `lamco/lamboot-tools` + `lamco/lamboot-migrate`
- **GitHub release** tarball at `lamco-admin/lamboot-tools`
- **Debian / Ubuntu packaging**: deferred to v0.3 per toolkit roadmap
- **Homebrew / Snap / Flatpak**: explicitly out of scope

---

## When to Use Which Tool

| Situation | Tool | Command |
|-----------|------|---------|
| "I don't know what's wrong with boot, look everywhere" | `lamboot-doctor` | `sudo lamboot-doctor` |
| Guided diagnose → plan → repair orchestrator | `lamboot-doctor` | `sudo lamboot-doctor --offline /dev/pve/vm-201-disk-1` |
| Deep UEFI boot-chain health scan | `lamboot-diagnose` | `sudo lamboot-diagnose` |
| ESP filesystem integrity or space issues | `lamboot-esp` | `sudo lamboot-esp check` |
| List what's on the ESP | `lamboot-esp` | `sudo lamboot-esp inventory` |
| Clean stale files from ESP | `lamboot-esp` | `sudo lamboot-esp clean` |
| Before making changes to boot config | `lamboot-backup` | `sudo lamboot-backup save` |
| System won't boot, automated repair | `lamboot-repair` | `sudo lamboot-repair` |
| VM won't boot, repair from Proxmox host | `lamboot-repair` | `sudo lamboot-repair --offline /dev/pve/vm-201-disk-1` |
| Migrating BIOS/MBR → UEFI/GPT | `lamboot-migrate` | `sudo lamboot-migrate to-uefi` |
| Replacing GRUB with LamBoot (cleanup included) | `lamboot-migrate` | `sudo lamboot-migrate to-lamboot --remove-grub` |
| Check boot configuration | `lamboot-migrate status` | `lamboot-migrate status` |
| Verify a migration succeeded | `lamboot-migrate verify` | `lamboot-migrate verify` |
| Roll back a migration | `lamboot-migrate rollback` | `sudo lamboot-migrate rollback` |
| Build a Unified Kernel Image matching LamBoot's expectations | `lamboot-uki-build` | `lamboot-uki-build` |
| Sign a binary with SBAT injection | `lamboot-signing-keys` | `lamboot-signing-keys sign-binary` |
| Rotate SB keys (db / KEK / PK) with cross-sign | `lamboot-signing-keys` | `sudo lamboot-signing-keys rotate db` |
| Inspect LamBoot's own on-disk artefacts (trust log, boot log) | `lamboot-inspect` | `lamboot-inspect summary` |
| Set up a Proxmox VM for LamBoot one-time | `lamboot-pve-setup` | `sudo lamboot-pve-setup <VMID>` |
| Fleet-wide LamBoot integration on Proxmox | `lamboot-pve-fleet` | `sudo lamboot-pve-fleet setup --all` |
| Monitor boot health of Proxmox VMs | `lamboot-pve-monitor` | `lamboot-pve-monitor` |
| Build OVMF_VARS.fd with LamBoot db cert pre-enrolled | `lamboot-pve-ovmf-vars` | `lamboot-pve-ovmf-vars build` |

For a guided walk-through:

```bash
$ lamboot-toolkit --help           # suite-level help
$ lamboot-toolkit <tool> --help    # per-tool help; works like git subcommands
```

---

## lamboot-doctor (beta)

Guided orchestrator that chains `diagnose` → plan → confirm → `repair` →
`verify` into one workflow. Preferred entry point when you don't know which
individual tool to reach for.

`--offline DISK` propagates through to sub-tool calls (via the shared
`lamboot-toolkit-lib.sh` offline plumbing), so one invocation can triage a
dead VM image from the Proxmox host. ESP clean is suppressed in offline mode
(needs live mount detection).

---

## lamboot-diagnose (stable v0.2)

System-wide UEFI boot-chain scanner. 11 categories, ~30 checks, unified JSON
schema v1 output (`--json`), `--offline DISK` for unmounted-disk analysis.

What it checks:

- **Partition table**: GPT vs MBR, ESP partition type GUID
- **ESP health**: Mount state, filesystem type, free space, permissions, fallback path
- **Boot entries**: UEFI boot variables, BootOrder, LamBoot entry presence
- **Bootloader files**: Binary existence, size, signature on ESP
- **Kernels**: Installed kernels, matching initrds, valid BLS entries
- **Secure Boot**: Status, enrolled keys, MOK state, shim presence
- **fstab**: ESP mount configuration
- **VM-specific**: OVMF NVRAM variables, LamBoot health state (crash counter, boot state), trust-log events

Exit codes: 0 (healthy), 1 (warnings only), 2 (at least one failure).
Common flags: `--json`, `--verbose`, `--quiet`, `--offline DISK`.

Reads `\loader\boot-trust.log` written by LamBoot per the trust-log event
schema v2 documented in `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` §6.

---

## lamboot-esp (stable v0.2)

ESP filesystem health, inventory, and cleanup with bootloader-critical safety
gates. Subcommands: `check` (default), `inventory`, `clean`. Will refuse to
remove anything that looks load-bearing (the binary in use, MOK blobs, the
fallback path) unless the operator explicitly forces past a warning.

Requirements: bash 4.0+, findmnt, lsblk, blkid. Optional: dosfstools for
`fsck.fat`.

---

## lamboot-backup (stable v0.2)

UEFI boot configuration save/restore/show/list. Four subcommands, unified
JSON schema v1.

`--vars-file OVMF_VARS.fd` enables **true offline NVRAM read/write** via
`virt-fw-vars`, decoding EFI_LOAD_OPTION little-endian UTF-16 entries and
synthesizing efibootmgr-compatible output. Auto-backs-up the VARS file before
any mutation.

---

## lamboot-repair (stable v0.2)

Six-phase flow (Diagnose → Plan → Show → Confirm → Execute → Verify) with
nine distinct repair actions grouped by risk tier. Never mutates before
showing the plan.

Online mode repairs the running system; `--offline DISK` uses `qemu-nbd` (or
`losetup`) to mount the disk image, applies repairs, and unmounts cleanly.

---

## lamboot-migrate (stable v1.0.0)

Complete migration toolkit. **First automated end-to-end BIOS→UEFI migration
tool for Linux.** Four subcommands: `to-uefi`, `to-lamboot`, `verify`,
`rollback`, plus `status` (boot-config reporter).

The 10-phase `to-uefi` pipeline is specified in
`docs/specs/SPEC-LAMBOOT-MIGRATE.md` §3:

1. Preflight (7 guardrails, see spec §5)
2. Interactive confirmation
3. Backup to `/var/backups/lamboot-migrate-<ts>/`
4. MBR → GPT via `sgdisk -g`
5. ESP creation + format
6. fstab update (UUID-based)
7. Chroot prep (bind-mount proc/sys/dev/ESP)
8. Bootloader install (per-distro recipes: Ubuntu 22.04/24.04, Debian 12/13, Fedora 39+, Arch, openSUSE Tumbleweed, Alpine 3.19+; generic `grub-install` fallback)
9. UEFI boot entry via `efibootmgr -c`
10. Verify (runs the `verify` subcommand's 11 checks)

`--remove-grub` performs post-verify distro-aware cleanup: apt/dnf/pacman/
zypper GRUB removal, ESP file cleanup across 14 known paths, NVRAM
`Boot####` deletion via `efibootmgr -B`, rollback manifest written to the
backup dir.

Three Proxmox methods (auto-selected): (A) pre-convert via live media,
(B) post-convert live chroot, (C) add second disk as ESP. See spec §4.

SDS-7 gap-close status: **done** (see
`~/lamboot-tools-dev/docs/SDS-7-GAP-ANALYSIS.md` and `SPEC-LAMBOOT-MIGRATE.md`
§14 reconciliation table).

---

## lamboot-uki-build (beta v0.2)

Build Unified Kernel Images matching LamBoot's expectations. Wraps `ukify`
or `objcopy` for section assembly, then `sbsign`/`sbverify` for Secure Boot
signing. Reads `/etc/lamboot/sbat.csv` for SBAT provenance if present.

---

## lamboot-signing-keys (experimental v0.2)

10 subcommands covering the key lifecycle documented in
`docs/KEY-GENERATION.md` and `docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md`:
generate, import, export, enroll, sign-binary, verify, list, rotate, revoke,
show.

`rotate db|kek|pk` does public-key-hash validation of the old keypair,
cross-signs with the parent (db ← KEK, KEK ← PK, PK self-signed with warning
on fallback), writes a timestamped rotation dir with a `rotation.json`
manifest.

`sign-binary` includes SBAT injection via `objcopy --add-section
.sbat=...` with resolution order `--sbat-file > --sbat > /etc/lamboot/sbat.csv
> built-in default`, preserving any existing `.sbat` section.

**Scope 1** (the canonical LamBoot key lifecycle) is fully covered in v0.2.
**Scope 2** (multi-key environment with parallel rotations) is partial; full
coverage at toolkit v0.5.

---

## lamboot-inspect (stable, mirrored from this repo)

Python tool for reading LamBoot's own on-disk artefacts: trust log (`trust-log.jsonl`),
boot log (`boot.log`), boot summary (`boot.json`), audit log (`audit.log`).
Strictly read-only.

Subcommands: `trust-log`, `boot-log`, `summary`, `verify`, `dump`, `show`.
Renders as text, JSON, timeline, or statistical summary.

Canonical source: `tools/lamboot-inspect` in this repo. Mirrored unchanged
into the toolkit tarball at release-build time via
`publish/mirror-from-lamboot-dev.sh`; never edit the mirror.

---

## lamboot-toolkit (dispatcher, stable v0.2)

Single-entry-point wrapper so `lamboot-toolkit diagnose --json` works like
`git status`. The individual tools remain independently invokable; the
dispatcher exists for discoverability and for packaging convenience.

---

## PVE subpackage: `lamboot-toolkit-pve`

Host-side Proxmox tools. Installed on the PVE host, not inside the VM.

**`lamboot-pve-setup`** (beta) — one-time per-VM setup. Attaches the fw_cfg
file-reference pattern (permanent `args:` line) and the
`lamboot-hookscript.pl` from this repo. `doctor-hookscript` subcommand
verifies the rewritten hookscript is installed and at version ≥ 0.8.4 — it
refuses to run against pre-0.8.4 hookscripts with remediation text pointing
at this repo's release.

**`lamboot-pve-fleet`** (experimental) — fleet-wide orchestration over
`lamboot-pve-setup`. Reads `/etc/lamboot/fleet.toml` (schema v1, authoritative
at `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md` §16 Appendix C) and
applies to the matching VMs.

**`lamboot-pve-monitor`** (stable, mirrored) — boot-health read via OVMF
NVRAM. Canonical source `tools/lamboot-monitor.py` in this repo.

**`lamboot-pve-ovmf-vars`** (stable, mirrored) — build
`OVMF_VARS_lamboot.fd` with this repo's db cert pre-enrolled for Config 4
zero-touch VM templates. Canonical source `tools/build-ovmf-vars.sh` in this
repo.

---

## Cross-repo coordination

Because the toolkit depends on this repo's `lamboot-hookscript.pl` to
function on Proxmox hosts, releases are coordinated. See
[`docs/CROSS-REPO-STATUS.md`](CROSS-REPO-STATUS.md) for the current state of
the four coordination items and the toolkit spec's §14.3 for the full table.

Brief: toolkit v0.2.0 depends on this repo v0.8.4 for the fw_cfg
file-reference hookscript rewrite. Path A (full coordinated release) is the
chosen approach. This repo owes: hookscript rewrite, `fleet.toml`
consumption in `lamboot-monitor.py`, `lamboot-install --toolkit-prompt`,
README cross-reference, three should-have doc back-links.

---

## Installation (once shipped)

```bash
# Fedora / EPEL via Copr
sudo dnf copr enable lamco/lamboot-tools
sudo dnf install lamboot-tools                     # core
sudo dnf install lamboot-toolkit-pve               # PVE host add-on
# Or just the migrator:
sudo dnf copr enable lamco/lamboot-migrate
sudo dnf install lamboot-migrate

# Or from release tarball (any distro)
tar xzf lamboot-tools-0.2.0.tar.gz
cd lamboot-tools-0.2.0
sudo make install PREFIX=/usr/local
```

Until v0.2.0 ships, source install from the private dev repo:

```bash
git clone git@github.com:lamco-admin/lamboot-tools-dev.git
cd lamboot-tools-dev
sudo make install PREFIX=/usr/local
```

### Opt-in via lamboot-install

In v0.8.4, `lamboot-install` will prompt after successful install:

```
? Install lamboot-tools for diagnostic and repair utilities? [y/N]
```

Non-interactive override: `--install-toolkit` / `--no-install-toolkit`.

---

## See Also

- [Cross-Repo Coordination Status](CROSS-REPO-STATUS.md) — current v0.8.4 + v0.2.0 release alignment
- [User Guide](USER-GUIDE.md) — LamBoot bootloader usage
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — when to reach for which tool
- [Install Reference](INSTALL-REFERENCE.md) — `lamboot-install` documentation
- [Proxmox Guide](PROXMOX-GUIDE.md) — offline repair + fleet integration
- [`SPEC-LAMBOOT-TOOLKIT-V1.md`](https://github.com/lamco-admin/lamboot-tools-dev/blob/main/docs/SPEC-LAMBOOT-TOOLKIT-V1.md) — authoritative product spec (toolkit repo)
- [`SPEC-LAMBOOT-MIGRATE.md`](specs/SPEC-LAMBOOT-MIGRATE.md) — SDS-7, the tool-level spec for `lamboot-migrate`
