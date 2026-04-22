# LamBoot Configuration Guide

**Version:** 0.8.3
**Updated:** 2026-04-21

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
| `default_timeout_ms` | integer | `4000` | Auto-boot delay in milliseconds. Set to `0` for no auto-boot (wait forever). |
| `default_entry` | string | _(none)_ | Entry ID to auto-boot. If unset, the first entry by sort order is selected. |

**Entry IDs** are derived from BLS filenames: `fedora-6.19.9.conf` → entry ID `bls-fedora-6.19.9`. For ESP-discovered entries: `windows`, `grub`, `refind`, `fallback`. For tools: `tool-diag-shell`, `tool-pci-inventory`, etc.

#### Example

```toml
default_timeout_ms = 10000
default_entry = "bls-fedora-6.19.9"
```

### `[security]` Section

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `secure_boot_required` | boolean | `false` | When `true`, logs a warning if Secure Boot is disabled. _(Advisory only — does not block boot.)_ |
| `measured_boot` | boolean | `false` | Informational flag indicating TPM measurement is desired. _(TPM measurement always runs when TPM is present, regardless of this setting.)_ |
| `crash_threshold` | integer | `2` | Number of failed boot attempts before crash loop fallback triggers. |
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
