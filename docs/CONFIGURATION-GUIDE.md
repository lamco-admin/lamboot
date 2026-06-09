# LamBoot Configuration Guide

**Version:** 0.16.5
**Updated:** 2026-06-08

---

## Overview

LamBoot is configured through several mechanisms:

| Mechanism | Location | Purpose |
|-----------|----------|---------|
| **policy.toml** | `\EFI\LamBoot\policy.toml` on ESP | Boot behavior, security, UI, modules |
| **manifest.toml** | `\EFI\LamBoot\modules\manifest.toml` on ESP | Diagnostic module display names |
| **SMBIOS OEM Strings** | QEMU/hypervisor config | VM identity (VMID, fleet ID) |
| **fw_cfg** | QEMU host config | Host-injected runtime data |
| **BLS Entries** | `\loader\entries\*.conf` on ESP | Boot entry definitions |

All configuration is optional. LamBoot works with sensible defaults out of the box.

---

## policy.toml Reference

The policy file is at `\EFI\LamBoot\policy.toml` on the ESP. All fields are optional — missing fields use defaults. The file uses TOML syntax with `[section]` headers.

When updating LamBoot via `lamboot-install --update`, the existing policy.toml is preserved. New defaults are written to `policy.toml.new` for reference.

### Root-Level Options

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `version` | integer | `1` | Config format version. Always `1`. |
| `default_timeout_ms` | integer | `5000` | Auto-boot delay in milliseconds. Set to `0` for no auto-boot (wait forever). |
| `default_entry` | string | _(none)_ | Entry ID to auto-boot. If unset, the first entry by sort order is selected. |
| `default_pattern` | string | _(none)_ | Glob over entry `id`. When set, the default selector picks the highest-version entry whose `id` matches the pattern, across all sort-key cohorts. Wins over `default_entry` when both are set and the pattern matches at least one entry. Mirrors sd-boot's escape hatch for distros that emit `sort-key` inconsistently (see Bug 22 / the cohort-split note below). |
| `bls_ignore_sort_key` | boolean | `false` | When `true`, the sort comparison ignores the BLS `sort-key` field entirely and orders by version + filename. Mirrors Fedora bootupd's choice; fixes both menu order and the default cursor when a distro mixes sort-key presence across kernels mid-upgrade. Spec deviation when on. |

**Entry IDs** are derived from BLS filenames: `fedora-6.19.9.conf` → entry ID `bls-fedora-6.19.9`. For ESP-discovered entries: `windows`, `grub`, `refind`, `fallback`. For tools: `tool-diag-shell`, `tool-pci-inventory`, etc.

> **Cohort-split escape hatches.** `default_pattern` and
> `bls_ignore_sort_key` exist for one real failure mode: a distro that
> emits `sort-key` on some kernels but not others (observed mid-upgrade
> on Debian forky/sid and Pop!_OS), which strands the wrong kernel at
> the top of the menu. Fresh v0.11.0+ installs ship the template with
> `bls_ignore_sort_key = true`; existing installs are left as-is and the
> new template lands at `policy.toml.new`. Either knob resolves it; the
> runtime also surfaces a cohort-split notice when it detects the
> condition.

#### Example

```toml
default_timeout_ms = 10000
default_entry = "bls-fedora-6.19.9"
```

### `[security]` Section

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `secure_boot_required` | boolean | `false` | When `true`, logs a warning if Secure Boot is disabled. _(Advisory only — does not block boot.)_ |
| `measured_boot` | boolean | `true` | Informational flag indicating TPM measurement is desired. _(TPM measurement always runs when a TPM is present, regardless of this setting; it defaults on because it is free when a TPM exists and a no-op when it does not.)_ |
| `crash_threshold` | integer | `3` | Number of failed boot attempts before crash loop fallback triggers. |
| `fallback_order` | array | `[]` | Entry IDs to try during crash loop, in priority order. If empty or none match, the menu is shown for manual selection. |
| `allowlist` | array | `[]` | Allowed EFI paths. If empty, all paths are allowed. Supports `*` wildcards. |
| `denylist` | array | `[]` | Denied EFI paths. Overrides the allowlist. Supports `*` wildcards. |

#### Crash Loop Example

```toml
[security]
crash_threshold = 3
fallback_order = ["bls-fedora-6.17.1", "fallback"]
```

If the system fails to boot 3 times in a row (without `lamboot-mark-success.service` running), LamBoot selects `bls-fedora-6.17.1`. If that entry doesn't exist, it tries `fallback`. If neither matches, the menu is shown.

#### Path Filtering Example

```toml
[security]
# Only allow Fedora and Windows entries
allowlist = ["\\EFI\\fedora\\*", "\\EFI\\Microsoft\\*", "\\EFI\\Linux\\*"]

# Block a known-bad kernel
denylist = ["\\EFI\\Linux\\vmlinuz-6.18.0-broken"]
```

Path matching is case-insensitive. Wildcards match any substring when placed at the start (`*suffix`), end (`prefix*`), or both (`*middle*`).

### `[ui]` Section

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `theme` | string | `"default"` | Color theme. Only `"default"` (Catppuccin Mocha) is currently available. |
| `font_size` | string | `"medium"` | Font size: `"small"`, `"medium"`, `"large"`. _(Parsed but not yet rendered — all sizes use medium.)_ |

### `[modules]` Section

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | boolean | `true` | Enable discovery of diagnostic modules from `\EFI\LamBoot\modules\`. Set to `false` to hide all modules from the boot menu. |

### `[watchdog]` Section

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | boolean | `true` | Enable the UEFI watchdog timer. Prevents indefinite hangs during boot. |
| `grace_seconds` | integer | `15` | Watchdog timeout in seconds. If LamBoot doesn't complete a phase within this time, the firmware resets the system. |

### `[loader]` Section (SDS-3, v0.9.0+)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `native_pe` | enum string | `"auto"` | Controls whether UKIs and Linux kernels are loaded via LamBoot's native Rust PE loader or firmware's `BS->LoadImage` + `BS->StartImage`. Values: `"auto"`, `"always"`, `"never"` (case-insensitive). |

#### Values

| Value | Behavior |
|-------|----------|
| `"auto"` (default) | Native loader when `ShimLock::Verify` succeeds on the image bytes pre-load (and trivially on SB-off + directly-signed setups). Falls back to firmware load when shim is present but rejects the bytes, or when no verify path is available. |
| `"always"` | Always use the native loader, regardless of pre-load verification. Operator asserts trust without delegating to shim. Useful during SB-off validation where every binary is self-signed by the administrator's key chain. |
| `"never"` | Always use `BS->LoadImage`. v0.8.3 behavior. One-flag rollback if the native loader ever hits an unexpected firmware quirk. |

#### Why this exists

Firmware's `BS->LoadImage` invokes the Security2Arch hook for every image. Under shim 15.8, that hook depends on `ShimLock` being installed. Shim 15.8 uninstalls `ShimLock` after `LoadImage`-ing any child binary (e.g. a legacy UEFI FS driver). On stock Ubuntu/Debian/Fedora, that sequence meant LamBoot under Secure Boot could not verify kernels after having loaded an ext4 driver — the v0.8.3 VM 122 failure mode.

Post-SDS-3, the native loader reads PE bytes, validates against §5.1 of `SPEC-NATIVE-PE-LOADER.md`, allocates UEFI pages, applies relocations, installs `LoadedImageProtocol`, and transfers control via a raw function-pointer cast — zero `BS->LoadImage` calls. `ShimLock::Verify` runs ONCE on the bytes before any of this, stamping the trust verdict that the `image_loaded_native` trust-log event records with the image's SHA-256.

The Chainload path (Windows Boot Manager, GRUB, EFI Fallback) stays on the firmware path because those binaries may have imports that the native loader rejects by design (spec §5.2). Chainload's shim interaction is a different surface and not SDS-3's target.

See `docs/specs/SPEC-NATIVE-PE-LOADER.md` for the full spec.

---

### `[drivers]` Section (SDS-6, v0.9.0+)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `legacy_uefi_drivers` | enum string | `"auto"` | Controls loading of `.efi` filesystem drivers from `\EFI\LamBoot\drivers\`. Values: `"auto"`, `"always"`, `"never"` (case-insensitive). |

#### Values

| Value | Behavior |
|-------|----------|
| `"auto"` (default) | Load drivers only for filesystems LamBoot does **not** natively support. Skips `ext4_*.efi`, `ext2_*.efi`, `ext3_*.efi`, `btrfs_*.efi`, `xfs_*.efi`, and `zfs_*.efi` because the native in-binary readers (ext4/btrfs via SDS-2, plus XFS via `lamxfs` and ZFS via `lamzfs` in v0.16.0) cover those filesystems read-only. The bundled `xfs_x64.efi` / `zfs_x64.efi` still ship as inert fallback but are not loaded at boot (loading them would shadow the native backend). Drivers for filesystems without a native skip entry (`ntfs`, `f2fs`, `iso9660`) still load if their `.efi` binary is present. |
| `"always"` | Load every driver in `\EFI\LamBoot\drivers\`, regardless of native coverage. v0.8.3 behavior. Useful for A/B debugging; preserves the shim-15.8 interaction pattern. Loading a natively-covered driver emits a `legacy_driver_redundant` trust event. |
| `"never"` | Load no legacy drivers. Skip all, regardless of coverage. Security-conscious posture for systems whose filesystems are all natively covered — asserts LamBoot will never `LoadImage` a third-party UEFI binary at boot. If a required filesystem has no native backend, the boot enumerating that volume will fail. |

#### Why this exists

LamBoot v0.8.x loaded every driver in the drivers directory at every boot, via `LoadImage`+`StartImage` under `SecurityOverride`. Each such load triggers shim 15.8 to uninstall `ShimLock`, preventing LamBoot from verifying kernels afterward. With the native ext4 reader (SDS-2) in v0.9.x, the ext4 driver is no longer needed for the 95%-of-users case — Auto mode removes it from the load path entirely, closing the shim-uninstall window for ext4-on-/boot systems. As of v0.16.0 the same applies to XFS and ZFS: the native `lamxfs` and `lamzfs` readers cover those filesystems read-only, so Auto mode skips the bundled GPL EfiFs `xfs_x64.efi` and `zfs_x64.efi` drivers entirely (they still ship as inert fallback). The native XFS, exFAT, and ZFS readers are read-only by construction; ZFS coverage is single-disk, mirror, and single-parity RAIDZ1 only.

See `docs/specs/SPEC-UEFI-FSDRV-DEPRECATION.md` for the full deprecation schedule.

---

### `[diagnostics]` Section (v0.11.0+)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `verbose` | boolean | `false` | When `true`, LamBoot writes a full breadcrumb trace of the boot path to the audit log for troubleshooting. Default off — no breadcrumbs are written. Turn on only to investigate a boot problem; on firmware with the `conout_fat_coupling` quirk the breadcrumbs are coalesced in RAM and flushed once before kernel handoff so enabling this never adds FAT-write churn to the hang-prone band. |

```toml
[diagnostics]
verbose = true
```

---

### `[boot-from-iso]` Section (v0.16.0+)

Boot a Linux distribution directly from an `.iso` image — either a file on a mounted volume or a physical optical disc — instead of from an installed BLS/UKI entry. **Experimental and opt-in; both keys default to `false`, so a default-config boot is byte-for-byte unaffected.**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | boolean | `false` | When `true`, LamBoot scans mounted volumes (ext4/btrfs/xfs/FAT, including the ESP) for `*.iso` boot candidates under `/isos` and `/boot/isos` and offers them in the menu. Default off — no ISO scan occurs. |
| `optical` | boolean | `false` | When `true`, LamBoot also enumerates physical CD/DVD/BD drives (`BlockIO` handles carrying a `CD001` primary volume descriptor) and offers an inserted disc as a boot candidate. Independent of `enabled` because the optical scan probes firmware handles directly. Default off. |

```toml
[boot-from-iso]
enabled = true
optical = true
```

When booting an ISO, LamBoot first parses the distribution's own `/boot/grub/loopback.cfg` (Path A1) to reuse the distro's exact kernel, initrd, and `iso-scan`/`findiso`/`root=live:` command line. When no usable `loopback.cfg` exists (and always for an optical disc), it fingerprints the distro family and applies a built-in recipe table covering arch, ubuntu-casper, debian-live, fedora, opensuse, and alpine — each transitively covering its derivatives (Path A2). Distribution kernels are Linux EFI-stub PEs, so the ISO path falls back from LamBoot's native PE loader to firmware `LoadImage`.

> **Validation status.** Only Arch 2026.05 and Fedora 44 are live-ISO-booted end to end. The other four families and their derivatives are recipe/table-validated and host-unit-tested, not yet live-booted. El Torito chainload (Path B) is not yet wired. Treat boot-from-ISO as experimental.

---

### `[boot-entry]` Section (v0.16.3+)

Controls LamBoot's bootloader-side NVRAM self-install. When enabled, LamBoot ensures a labeled `LamBoot` `Boot####` entry pointing at `\EFI\LamBoot\lambootx64.efi` exists and front-loads `BootOrder`, creating it directly from the UEFI environment if absent. This is the OS-independent pathway for the persistent boot entry — no `efibootmgr`, no OS, and no SELinux-confined-service block in the way (`Boot####`/`BootOrder` are non-authenticated, so no Secure Boot keys are involved). It is idempotent: it keys on the exact `LamBoot` description, so it never duplicates or churns NVRAM and coexists with the OS-side `efibootmgr` pathway.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `self_install` | boolean | `true` | When `true` (the default), LamBoot self-installs/repairs its own `Boot####` NVRAM entry at boot. Set to `false` to opt out — for operators who manage firmware boot order externally. Only the literal `false` disables it; a malformed value fails safe to on. |

```toml
[boot-entry]
self_install = false
```

---

## Module Manifest

The file `\EFI\LamBoot\modules\manifest.toml` provides friendly display names for diagnostic modules. Without it, modules are shown as "Tool: _filename_".

### Format

```toml
[modules.diag-shell]
name = "Diagnostic Shell"

[modules.pci-inventory]
name = "PCI Device Inventory"

[modules.mem-quick]
name = "Quick Memory Test"

[modules.nvme-diag]
name = "NVMe Health Check"
```

The section ID (e.g., `diag-shell`) matches the `.efi` filename without the extension. Only the `name` field is read.

---

## SMBIOS OEM Strings

LamBoot reads SMBIOS Type 11 (OEM Strings) to get host-injected identity information. Strings must use the format `lamboot.KEY=VALUE`.

### Recognized Keys

| Key | Used In | Description |
|-----|---------|-------------|
| `lamboot.vmid` | GUI header, boot.json | VM identifier (e.g., Proxmox VMID). Displayed prominently in the boot menu header. |
| `lamboot.fleet-id` | boot.json | Fleet or cluster identifier for grouping VMs in monitoring. |

Additional keys (e.g., `lamboot.role`, `lamboot.monitor`) are stored in SMBIOS but not currently read by LamBoot. They can be used by host-side tooling like `lamboot-monitor.py`.

### Setting OEM Strings

**QEMU/KVM (direct)**:
```bash
qemu-system-x86_64 ... -smbios type=11,value=lamboot.vmid=201
```

**Proxmox (VM config)**:
```bash
qm set 201 --args "-smbios type=11,value=lamboot.vmid=201"
```

**Proxmox (automatic via hookscript)**: The LamBoot hookscript auto-injects the VMID on every VM start. See the [Proxmox Guide](PROXMOX-GUIDE.md).

**Other hypervisors**: Any method that sets SMBIOS Type 11 OEM strings will work. Consult your hypervisor documentation.

**Bare metal**: SMBIOS OEM strings are set by the hardware manufacturer. LamBoot reads them if present but does not require them.

---

## fw_cfg Data Channel

On QEMU/KVM (x86_64 only), LamBoot reads the QEMU fw_cfg device for host-injected configuration:

| fw_cfg File | Description |
|-------------|-------------|
| `opt/lamboot/config` | Arbitrary configuration string from the host |
| `etc/vmgenid_guid` | VM Generation ID (used for snapshot detection) |

### Injecting fw_cfg Data

```bash
qemu-system-x86_64 ... -fw_cfg name=opt/lamboot/config,string="key1=value1;key2=value2"
```

The `opt/lamboot/config` file is logged at boot time. Its contents are available in the boot log but are not currently parsed for configuration — this is a data channel for host-side integration.

The VM Generation ID (`etc/vmgenid_guid`) is read automatically to detect snapshot restores and reset the crash counter.

---

## BLS Entry Format

LamBoot discovers boot entries from BLS (Boot Loader Specification) Type 1 files at `\loader\entries\*.conf` on the ESP.

### Recognized Fields

| Field | Required | Description |
|-------|----------|-------------|
| `title` | Yes | Display name in the boot menu |
| `version` | No | Kernel version (used for sorting) |
| `machine-id` | No | Machine identifier |
| `sort-key` | No | Sort priority prefix (e.g., distro name) |
| `linux` | Yes* | Kernel path (relative to ESP root) |
| `initrd` | No | Initrd path(s), one per line. Multiple initrds supported. |
| `efi` | Yes* | EFI application path (alternative to `linux`) |
| `options` | No | Kernel command line arguments. Multiple `options` lines are concatenated. |
| `architecture` | No | Target architecture (e.g., `x64`, `aa64`). Entries for other architectures are hidden. |

*Either `linux` or `efi` is required.

### Boot Counting

BLS entries support boot assessment via filename suffixes:

- `entry+3.conf` — 3 tries remaining, entry is being assessed
- `entry+0-2.conf` — 0 tries remaining, 2 attempts made, entry failed assessment
- `entry.conf` — no counters, entry is known good

LamBoot decrements the counter on each boot attempt. When tries reach 0, the entry is considered failed and sorted to the bottom.

### Sort Order

Entries are sorted using the UAPI.10 algorithm:

1. `sort-key` (lexicographic, entries without sort-key come last)
2. Machine ID grouping
3. `version` (version-aware comparison — `6.19.9` > `6.17.1`)
4. Failed boot-count entries sorted to the bottom

### GRUB Variable Stripping

BLS entries written by GRUB may contain `$kernelopts` or `${variable}` syntax in the `options` field. LamBoot strips these and falls back to `/proc/cmdline` (via the installer) for the actual kernel command line.

---

## See Also

- [User Guide](USER-GUIDE.md) — getting started, daily use
- [Proxmox Guide](PROXMOX-GUIDE.md) — VM fleet deployment, hookscript, VMID injection
- [Security Guide](SECURITY-GUIDE.md) — Secure Boot, TPM, signing pipeline
- [Install Reference](INSTALL-REFERENCE.md) — lamboot-install CLI reference
- [Architecture Guide](ARCHITECTURE.md) — internal design and boot phases
