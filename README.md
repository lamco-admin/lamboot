# LamBoot

**Modern, memory-safe UEFI boot for Linux — in Rust.**

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)
[![UEFI](https://img.shields.io/badge/target-UEFI-green.svg)](https://uefi.org/)

LamBoot is a UEFI bootloader for Linux targeting hypervisor-managed VM environments (Proxmox in particular) and homelab / desktop systems. Eight clean architectural layers, ~8,300 lines of Rust. Graphical menu with mouse. BLS + UKI + `kernel-install` done right. Host-side fleet monitoring without an in-guest agent. An honest Secure Boot posture with a JSON trust-evidence log written to the ESP on every boot.

**v0.9.0 lands the native trust chain.** Native read-only ext4 reader, native PE loader, BLS multi-filesystem discovery, and legacy UEFI FS driver deprecation — the kernel-load path no longer touches `BS->LoadImage` or external filesystem drivers, so the shim 15.8 `ShimLock`-uninstall failure mode that blocked stock `/boot` on ext4 under Secure Boot is structurally unreachable. Pop!_OS / systemd-boot-discoverable-EFI auto-discovery is first-class. v0.8.4 (April 2026) shipped the coordinated Proxmox-toolkit integration this builds on. See the [roadmap](docs/ROADMAP.md).

- **Current version:** 0.9.0 (April 2026)
- **Binary size:** 215 KB (x86_64 unsigned), 217 KB (signed)
- **Platforms:** x86_64 UEFI, aarch64 UEFI
- **License:** MIT OR Apache-2.0
- **Architecture:** 8 layers, codified — see [`docs/ARCHITECTURE-LAYERS.md`](docs/ARCHITECTURE-LAYERS.md)

---

## Why LamBoot

| | |
|---|---|
| 🧱 **Clean architecture** | Eight layers, each with a single job. Every module declares its layer; dependencies flow one direction. Contribute in an afternoon. |
| 🏭 **Proxmox-native** | Host-side VM boot-health monitoring via NVRAM — no agent in the guest. First Linux bootloader with fleet ops as a first-class concern. |
| ⚖️ **Modern Linux boot** | BLS Type 1 with UAPI.10 version sort. UKI first-class. LoadFile2 initrd (kernel 5.7+). kernel-install + postinst hooks. No GRUB scripting. |
| 🖱️ **Graphical menu with a mouse** | Double-buffered GOP renderer, full pointer support, no flicker. Serial console fallback. Unique among Rust bootloaders. |
| 🦀 **Memory-safe Rust** | `#![no_std]`, every `unsafe` block documented. ~8,300 LoC now, ~12,000 after v1.0 — still smaller than GRUB by 3×. |
| 📝 **Honest Secure Boot** | JSON trust-evidence log on `\loader\boot-trust.log` — every image-authentication decision recorded. Threat model documents what LamBoot can and cannot verify. |
| 🧪 **Crash-loop recovery** | NVRAM state machine with automatic fallback entry selection, systemd-bless-boot compatible. |

**Honest posture for v0.9.0:**
- **Ships now:** native ext4 read backend, native PE loader, native trust chain, BLS multi-FS discovery, signing pipeline, SecurityOverride (Path F), trust-evidence log, install-script hardening, Proxmox integration, BLS+UKI handling. The previous shim 15.8 `ShimLock`-uninstall failure on stock `/boot` ext4 under Secure Boot is structurally unreachable on the kernel-load path.
- **Pop!_OS / systemd-boot-discoverable-EFI** kernels and recovery entries are auto-discovered.
- Not shim-review approved yet (parallel track).
- Not a GRUB drop-in — no legacy BIOS, no rescue shell, no GRUB config language.
- Standard ecosystem gaps inherited (initrd integrity, kernel cmdline post-verification). See [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md).

---

## Documentation

### Getting started
- [**QUICKSTART.md**](QUICKSTART.md) — 30-second build and install
- [**docs/INSTALL-REFERENCE.md**](docs/INSTALL-REFERENCE.md) — `lamboot-install` flags and walkthrough
- [**docs/USER-GUIDE.md**](docs/USER-GUIDE.md) — end-user usage

### Secure Boot
- [**docs/SECURITY-MODEL.md**](docs/SECURITY-MODEL.md) — honest threat model (what LamBoot defends vs. what it doesn't)
- [**docs/SECURE-BOOT-DEPLOYMENT.md**](docs/SECURE-BOOT-DEPLOYMENT.md) — four trust configurations with walkthroughs
- [**docs/KEY-GENERATION.md**](docs/KEY-GENERATION.md) — PK/KEK/db production key hierarchy
- [**docs/MOK-ENROLLMENT-GUIDE.md**](docs/MOK-ENROLLMENT-GUIDE.md) — MokManager screen-by-screen
- [**docs/OVMF-VARS-PROXMOX.md**](docs/OVMF-VARS-PROXMOX.md) — zero-touch Proxmox Config 4

### Architecture
- [**docs/ARCHITECTURE.md**](docs/ARCHITECTURE.md) — 10-phase boot flow, subsystem map
- [**docs/DEVELOPER-GUIDE.md**](docs/DEVELOPER-GUIDE.md) — project layout, coding standards, adding a subsystem

### Integration
- [**docs/PROXMOX-GUIDE.md**](docs/PROXMOX-GUIDE.md) — VM fleet operation
- [**docs/CONFIGURATION-GUIDE.md**](docs/CONFIGURATION-GUIDE.md) — `policy.toml` reference
- [**docs/LAMBOOT-TOOLS-OVERVIEW.md**](docs/LAMBOOT-TOOLS-OVERVIEW.md) — companion tooling

### Troubleshooting & policy
- [**docs/TROUBLESHOOTING-GUIDE.md**](docs/TROUBLESHOOTING-GUIDE.md)
- [**CHANGELOG.md**](CHANGELOG.md) — release notes
- [**SECURITY.md**](SECURITY.md) — disclosure policy
- [**docs/ROADMAP.md**](docs/ROADMAP.md)

---

## Quick install

### Homelab (Secure Boot off)

```bash
tar xzf lamboot-0.9.0-x86_64.tar.gz && cd lamboot-0.9.0
sudo ./lamboot-install
# Reboot → pick "LamBoot" from firmware boot menu
```

### Linux desktop (Secure Boot on, shim + MOK)

```bash
sudo ./lamboot-install --signed
# Reboot → MokManager → Enroll MOK → enter passphrase → Yes → Reboot
```

### Proxmox zero-touch (VM template)

```bash
# On host
cp lamboot-0.9.0/OVMF_VARS_lamboot.fd /var/lib/vz/images/100/OVMF_VARS_100.fd
# In VM
sudo ./lamboot-install --signed --no-mok
```

Full walkthroughs in [`docs/SECURE-BOOT-DEPLOYMENT.md`](docs/SECURE-BOOT-DEPLOYMENT.md).

---

## Diagnostic and repair utilities

Companion toolkit: **[`lamboot-tools`](https://github.com/lamco-admin/lamboot-tools)** —
eleven CLI tools for diagnosing, repairing, backing up, and migrating Linux
UEFI boot configurations. Works on any UEFI system regardless of bootloader;
has LamBoot-specific subcommands where useful.

Ships as three RPM subpackages from one source tree:

- `lamboot-tools` — core utilities (`lamboot-diagnose`, `lamboot-doctor`, `lamboot-esp`, `lamboot-backup`, `lamboot-repair`, `lamboot-migrate`, `lamboot-uki-build`, `lamboot-signing-keys`, `lamboot-toolkit` dispatcher, and a mirror of this repo's `lamboot-inspect`).
- `lamboot-migrate` — standalone RPM for operators who want only the BIOS→UEFI migrator.
- `lamboot-toolkit-pve` — Proxmox host add-on (`lamboot-pve-setup`, `lamboot-pve-fleet`, plus mirrors of `lamboot-monitor.py` and `build-ovmf-vars.sh` from this repo).

See the companion repository [github.com/lamco-admin/lamboot-tools](https://github.com/lamco-admin/lamboot-tools) for the toolkit source, releases, and tool-by-tool documentation.

---

## Build from source

```bash
# Prerequisites
rustup install nightly
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi

# Build
./build.sh

# Output:
#   dist/EFI/LamBoot/lambootx64.efi     (215 KB, x86_64)
#   dist/EFI/LamBoot/lambootaa64.efi    (aarch64)
#   dist/EFI/LamBoot/drivers/*.efi      (ext4, btrfs, ntfs, etc.)
#   dist/EFI/LamBoot/modules/*.efi      (nvme-diag, mem-quick, ...)

# Test in QEMU
./run-qemu.sh
```

### Secure Boot signing (optional)

```bash
# Generate production keys — one time; see docs/KEY-GENERATION.md
# ... populates keys/{pk,kek,db}.{key,crt}

# Unlock signing key for this session
./tools/sign-unlock

# Sign bootloader + drivers + modules
./tools/sign-lamboot.sh
# Output: dist/EFI/LamBoot/lambootx64-signed.efi + 14 signed modules/drivers
```

---

## Trust-evidence log

On every boot, LamBoot writes `\loader\boot-trust.log` to the ESP. One JSON object per line:

```json
{"seq":0,"event":"boot_start","verified_via":"","status":"","note":"secure_boot=true"}
{"seq":1,"event":"driver_loaded","path":"\\EFI\\LamBoot\\drivers\\ext4_x64.efi","verified_via":"shim_mok","status":"Success","note":""}
{"seq":2,"event":"boot_attempt","path":"\\EFI\\Linux\\ubuntu-6.11.efi","verified_via":"firmware_db","status":"Success","note":"uki"}
```

Read it post-boot from Linux:
```bash
cat /boot/efi/loader/boot-trust.log | jq .
```

**No other mainstream bootloader ships this.** See [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md) for the schema and [`lamboot-core/src/trust_log.rs`](lamboot-core/src/trust_log.rs) for the source.

---

## Architecture

```
UEFI Firmware → LamBoot
  1. Assess boot health (NVRAM state machine)
  2. Detect Secure Boot; initialise TPM
  3. Mount ESP; instantiate TrustLog
  4. Load policy.toml; measure into PCR 5
  5. Request ShimRetainProtocol (v0.8.3+)
  6. Install SecurityOverride (Path F)
  7. Load filesystem drivers, record trust events
  8. Reconnect controllers; enumerate volumes
  9. Discover BLS entries, ESP entries, UKI, modules
 10. Evaluate crash state; select fallback if CrashLoop
 11. Display menu (GUI or serial)
 12. Decrement boot counter; measure cmdline (PCR 12); measure kernel (PCR 4)
 13. Register initrd via LoadFile2
 14. Flush TrustLog to \loader\boot-trust.log
 15. Boot via uefi::boot::start_image under SecurityOverride
```

```
lamboot-core/src/
  main.rs                Orchestration
  security_override.rs   Path F — Security/Security2 arch protocol hooks
  trust_log.rs           v0.8.3 — JSON-lines audit log
  bls.rs                 BLS Type 1 + UAPI.10 sort + boot counting
  gui.rs                 Double-buffered GOP + mouse
  discovery.rs           BLS-first entry discovery, UKI detection
  policy.rs              policy.toml parser
  health.rs              NVRAM state machine, Boot Loader Interface
  fs.rs                  ESP/volume mounting, multi-partition scan
  initrd.rs              LoadFile2 provider
  boot.rs                Chainload, UKI, Linux under SecurityOverride
  input.rs               Keyboard + mouse
  tpm.rs                 TCG2 measured boot
  console.rs             Serial fallback menu
  drivers.rs             Filesystem driver loader + trust events
  secure.rs              SB state + shim integration
```

Full explanation in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Configuration

`policy.toml` on the ESP:

```toml
version = 1
default_timeout_ms = 5000
default_entry = "fedora-6.12.0"

[security]
secure_boot_required = false
measured_boot = true
crash_threshold = 3
fallback_order = ["fedora-6.11.0", "fallback"]
allowlist = ["\\EFI\\fedora\\*", "\\EFI\\Microsoft\\*"]
denylist = []

[ui]
theme = "default"
font_size = "medium"

[watchdog]
enabled = true
grace_seconds = 15
```

Full reference in [`docs/CONFIGURATION-GUIDE.md`](docs/CONFIGURATION-GUIDE.md).

---

## Boot entry discovery priority

1. **BLS entries** — `/loader/entries/*.conf` on all volumes (ESP + XBOOTLDR + driver-mounted)
2. **Unified Kernel Images** — `\EFI\Linux\*.efi`
3. **Windows** — `\EFI\Microsoft\Boot\bootmgfw.efi`
4. **Other distros** — `\EFI\{fedora,ubuntu,debian,...}\grubx64.efi`
5. **Fallback** — `\EFI\Boot\BOOTX64.EFI`
6. **Diagnostic modules** — `\EFI\LamBoot\modules\*.efi` (surfaced in menu with manifest names)

When BLS entries exist, legacy distro-path scanning is skipped to avoid duplicates.

---

## Crash recovery

LamBoot's NVRAM state machine:

```
Fresh → Booting → BootedOK (OS calls systemd-bless-boot)
           │
           └→ CrashLoop (counter >= threshold)
                 │
                 └→ Fallback entry selected automatically
```

The OS-side `systemd-bless-boot.service` marks successful boots; LamBoot counts unsuccessful ones. At `crash_threshold`, LamBoot auto-selects from `fallback_order`.

**NVRAM variables** (`4C414D42-4F4F-5400-0000-000000000001`):
| Variable | Type | Purpose |
|---|---|---|
| `LamBootState` | u8 | 0=Fresh 1=Booting 2=BootedOK 3=CrashLoop |
| `LamBootCrashCount` | u8 | Crash counter |
| `LamBootLastEntry` | UTF-8 | Last booted entry ID |
| `LamBootTimestamp` | 8 bytes | Packed UTC |
| `LamBootVersion` | u32 | Packed semver |
| `ShimRetainProtocol` | u8 | Ask shim to leave ShimLock installed |

---

## Proxmox integration

`tools/lamboot-monitor.py` runs on the hypervisor host, reads LamBoot NVRAM from OVMF_VARS files on the backing storage — **no agent inside the VM**.

```bash
sudo python3 tools/lamboot-monitor.py

# VMID    Status       State  Crashes  Name
# ----------------------------------------------------------
#   100        OK    BootedOK        0  web-server
#   101      WARN     Booting        1  database
#   102      CRIT   CrashLoop        5  broken-vm

sudo python3 tools/lamboot-monitor.py --json
sudo python3 tools/lamboot-monitor.py --alert-webhook https://hooks.example.com/alert
sudo python3 tools/lamboot-monitor.py --vmid 102
```

Full walkthrough in [`docs/PROXMOX-GUIDE.md`](docs/PROXMOX-GUIDE.md).

---

## TPM measured boot

| PCR | What's measured | Event type |
|---|---|---|
| 4 | Kernel image (Authenticode hash) | `EFI_BOOT_SERVICES_APPLICATION` |
| 5 | `policy.toml` content | `IPL` |
| 12 | Kernel command line (UTF-16) | `IPL` |

Gracefully degrades when TPM is absent — never blocks boot.

---

## Diagnostic modules

Chainloaded EFI applications in `\EFI\LamBoot\modules\`:

| Module | Size | Description |
|---|---|---|
| `mem-quick.efi` | 24 KB | Walking-ones and address-pattern memory tests |
| `nvme-diag.efi` | 15 KB | NVMe S.M.A.R.T. health check |
| `pci-inventory.efi` | — | PCI device enumeration |
| `diag-shell.efi` | — | Interactive diagnostic shell |

Manifest (`modules/manifest.toml`) provides friendly names. Modules signed with the same key as the bootloader. Build your own: see [`docs/DEVELOPER-GUIDE.md`](docs/DEVELOPER-GUIDE.md).

---

## Filesystem drivers

LamBoot loads EFI filesystem drivers from `\EFI\LamBoot\drivers\` at startup under the SecurityOverride Path F guard:

```
ext4_x64.efi    btrfs_x64.efi    ntfs_x64.efi    xfs_x64.efi
zfs_x64.efi     f2fs_x64.efi     iso9660_x64.efi
```

After loading, LamBoot reconnects all controllers, making new SimpleFileSystem handles available.

**Sources:** [rEFInd](https://www.rodsbooks.com/refind/) (GPL-2.0) and [EfiFs](https://github.com/pbatard/efifs) (GPL-3.0). Distributed as separate binaries under GPL linking exception — LamBoot itself is MIT/Apache-2.0.

---

## Comparison

| | **LamBoot** | GRUB | systemd-boot | rEFInd | sd-boot-rs |
|---|---|---|---|---|---|
| Language | **Rust** | C | C | C++ | Rust |
| Binary | **215 KB** | ~4 MB | ~300 KB | ~1 MB | ~200 KB |
| Memory-safe | **Yes** | No | No | No | Yes |
| GUI | **Yes + mouse** | No | Text | Yes + icons | No |
| BLS | **Full** | Partial | Full | None | Partial |
| UKI first-class | **Yes** | Wrapper | Yes | No | No |
| Crash recovery | **NVRAM state machine** | No | Boot counting | No | No |
| **Trust-evidence log** | **Yes (JSON)** | **No** | **No** | **No** | **No** |
| Secure Boot | **DB + shim + MOK + SecurityOverride** | Shim | Shim | Shim | Shim |
| TPM | **PCR 4/5/12** | No | PCR 4/5/8/12 | No | Roadmap |
| Loadable FS drivers | **Yes** | Built-in | FAT only | Yes | Yes |
| Proxmox integration | **First-class** | Generic | Generic | Generic | Cloud |
| x86_64 + aarch64 | **Both** | Both | Both | x86_64 | Both |
| License | MIT OR Apache-2.0 | GPLv3 | LGPL-2.1+ | GPLv3 | MIT |

---

## Development

Prerequisites: Rust nightly + stable, `sbsign`, `llvm-objcopy`, QEMU + OVMF.

```bash
# Build
./build.sh

# x86_64 only
cargo build --target x86_64-unknown-uefi --release -p lamboot-core

# Lint + format (pre-commit hook runs these)
rustup run nightly cargo fmt
cargo clippy

# QEMU smoke test
./run-qemu.sh
```

See [`docs/DEVELOPER-GUIDE.md`](docs/DEVELOPER-GUIDE.md) and [`CONTRIBUTING.md`](CONTRIBUTING.md).

---

## License

**LamBoot:** MIT OR Apache-2.0 (dual-licensed).
See [`LICENSE`](LICENSE), [`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE).

**Bundled filesystem drivers:** GPL-2.0 from the rEFInd project, distributed as separate binaries. See [`LICENSE`](LICENSE) §"Third-party binary notice".

---

## Contact

- **General:** `office@lamco.io`
- **Security disclosure:** `security@lamco.io` — see [`SECURITY.md`](SECURITY.md) for GPG key
- **Issues / discussion:** [github.com/lamco-admin/lamboot](https://github.com/lamco-admin/lamboot) (public repo)

**Development happens in a private repo.** The public repo mirrors each release. Pull requests and issues on the public repo are welcome.
