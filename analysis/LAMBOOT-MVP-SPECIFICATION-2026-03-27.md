# LamBoot MVP Specification & Development Roadmap

**Date:** 2026-03-27
**Status:** Draft — Research-informed specification
**Target:** Open-source Proxmox-first VM bootloader, expandable to general UEFI use

---

## 1. Vision

LamBoot is a **memory-safe UEFI bootloader** written in Rust, targeting the underserved niche of **hypervisor-managed VM environments** while remaining usable as a general-purpose boot manager. It differentiates from competitors through:

1. **Interactive GUI** — Graphical boot menu with mouse support (unique among Rust bootloaders)
2. **Deep crash recovery** — NVRAM-based boot health monitoring with automatic fallback, exceeding systemd-boot's boot counting
3. **Pre-boot diagnostics** — Modular diagnostic tools loadable without booting the OS
4. **Proxmox integration** — UEFI NVRAM variables readable by hypervisor for boot health monitoring
5. **Zero-config operation** — Auto-detects OS installations via BLS entries and ESP scanning

### 1.1 Strategic Positioning

| Competitor | Focus | LamBoot Advantage |
|------------|-------|-------------------|
| GRUB | Universal, complex | Simpler, memory-safe, smaller |
| systemd-boot | systemd ecosystem | GUI, diagnostics, not systemd-dependent |
| Sprout | Cloud-native containers | Interactive GUI, crash recovery, VM diagnostics |
| rEFInd | Power users | Smaller, Rust memory safety, crash recovery |

### 1.2 Target Users

**Primary:** Proxmox administrators managing VM fleets who want:
- Boot loop detection and automated fallback
- Pre-boot diagnostics for troubleshooting
- Simplified VM template management
- Small bootloader footprint for fast VM cold boot

**Secondary:** Linux enthusiasts, developers, and educators who want:
- A hackable, readable Rust UEFI bootloader
- An alternative to GRUB that "just works"
- A platform for UEFI experimentation

---

## 2. MVP Feature Scope

### 2.1 Phase 1 — Fix Foundations (make it actually work)

These fix existing broken or incomplete functionality:

| ID | Feature | Priority | Effort | Description |
|----|---------|----------|--------|-------------|
| F1 | **Framebuffer rendering** | Critical | 2d | Replace per-pixel BLT with off-screen buffer + single BufferToVideo transfer |
| F2 | **Complete bitmap font** | Critical | 1d | Embed full VGA 8x8 ROM font (256 characters including lowercase) |
| F3 | **Dynamic resolution** | Critical | 0.5d | Query GOP mode for actual dimensions, pass to InputManager |
| F4 | **Fix TOML parser** | Critical | 2d | Implement section-aware parsing with array support for allowlist/denylist |
| F5 | **Timestamps** | High | 0.5d | Use `uefi::runtime::get_time()` for report timestamps |
| F6 | **Proper GUID** | Low | 0.5d | Generate proper vendor GUID for NVRAM variables |

### 2.2 Phase 2 — Core Boot Capabilities (make it useful)

| ID | Feature | Priority | Effort | Description |
|----|---------|----------|--------|-------------|
| F7 | **BLS Type 1 support** | Critical | 3d | Parse `/loader/entries/*.conf` files. Fields: title, linux, initrd, options, version, machine-id |
| F8 | **Crash recovery system** | Critical | 3d | Implement full boot assessment: per-entry counting, fallback selection, NVRAM state machine |
| F9 | **Initrd loading** | Critical | 2d | Load initrd via EFI LoadFile2 protocol or direct memory loading for EFI stub kernels |
| F10 | **EFI driver loading** | High | 2d | LoadImage/StartImage for .efi drivers from `\EFI\LamBoot\drivers\` directory |
| F11 | **Ship ext4 driver** | High | 1d | Bundle rEFInd's `ext4_x64.efi` or build from Sprout's driver (Apache-2.0 compatible) |
| F12 | **Multi-partition scanning** | High | 2d | Enumerate all SimpleFileSystem handles to discover boot entries across disks |
| F13 | **BLS autoconfiguration** | Medium | 2d | If no entries found, scan for BLS-compatible layouts and generate config |

### 2.3 Phase 3 — Proxmox Integration (make it special)

| ID | Feature | Priority | Effort | Description |
|----|---------|----------|--------|-------------|
| F14 | **NVRAM boot health protocol** | High | 2d | Define structured UEFI variables: crash counter, last boot entry, boot status, timestamps |
| F15 | **Proxmox monitor script** | High | 2d | Python/bash tool for Proxmox host that reads OVMF_VARS via qemu-nbd, extracts LamBoot health variables, alerts on boot loops |
| F16 | **VM template installer** | Medium | 1d | Script to inject LamBoot into a Proxmox VM template's EFI disk |
| F17 | **Serial console mode** | Medium | 1d | Text-mode fallback when GOP unavailable, works over serial/SOL |
| F18 | **Boot report via UEFI variables** | Medium | 1d | Write last boot report to NVRAM (not just ESP file) for host-side reading |

### 2.4 Phase 4 — Production Readiness (make it trustworthy)

| ID | Feature | Priority | Effort | Description |
|----|---------|----------|--------|-------------|
| F19 | **Secure Boot support** | Critical | 5d | Shim integration, MOK enrollment, or self-signed chain for testing |
| F20 | **kernel-install plugin** | High | 2d | Installable hook that creates BLS entries when kernels are installed |
| F21 | **TPM measured boot** | High | 3d | Extend PCR registers for boot chain measurement |
| F22 | **Boot counting (systemd-compatible)** | High | 2d | Support `+N` suffix in BLS filenames, compatible with systemd-bless-boot |
| F23 | **Diagnostic module framework** | Medium | 3d | Define module ABI, implement module loader, build 1-2 sample diagnostics |
| F24 | **ARM64 support** | Medium | 2d | Add `aarch64-unknown-uefi` target, test with QEMU aarch64 OVMF |

---

## 3. Architecture Decisions

### 3.1 GUI Rendering — Double-Buffered Framebuffer

**Current:** Per-pixel `BltOp::VideoFill` (1x1) — ~128 BLT calls per character.

**MVP approach:**
```
1. Query GOP for mode info (resolution, pixel format, framebuffer address)
2. Allocate off-screen buffer: Vec<BltPixel> of width × height
3. Render all UI elements into buffer using CPU drawing routines
4. Transfer entire buffer to screen with single BltOp::BufferToVideo
5. On input events, re-render dirty regions and BLT
```

This gives tear-free, fast rendering. In VM environments (OVMF), the framebuffer is automatically exposed via the hypervisor's VNC/SPICE — no additional work needed for remote access.

**Future:** Consider a theme/skin system where UI assets (backgrounds, icons) are loaded from ESP files rather than compiled into the binary.

### 3.2 Filesystem Access — Driver Loading Bridge

**Current:** Only FAT via UEFI SimpleFileSystem (ESP only).

**MVP approach:**
```
1. At startup, scan \EFI\LamBoot\drivers\ for *.efi files
2. For each: LoadImage() + StartImage() → driver registers with UEFI
3. ConnectController() to bind drivers to block devices
4. Now SimpleFileSystem handles appear for ext4/btrfs partitions
5. Use standard SimpleFileSystem API to read /boot, /loader/entries, etc.
```

Ship ext4_x64.efi from rEFInd (GPL-compatible) or build from the open-source ext4 UEFI driver project. This gets ext4 support without implementing a filesystem in Rust.

**Future:** Native Rust ext4 read-only implementation for no external dependency.

### 3.3 Boot Entry Discovery — BLS-First

**Current:** Hardcoded path scanning only.

**MVP approach:**
```
Priority order:
1. BLS entries: /loader/entries/*.conf (standard, kernel-install compatible)
2. Custom entries: \EFI\LamBoot\entries\*.toml (LamBoot-specific)
3. ESP scanning: Windows, GRUB, rEFInd, UKI at known paths (fallback)
```

BLS support makes LamBoot compatible with `kernel-install` out of the box. When a distro installs a new kernel, the BLS entry appears automatically.

### 3.4 Crash Recovery — State Machine

**Current:** Monotonically increasing counter, never read or cleared.

**MVP approach — NVRAM State Machine:**

```
Variables (vendor GUID: LamBoot-specific):
  LamBootState: enum { Fresh, Booting, BootedOK, CrashLoop }
  LamBootCrashCount: u8 (per-entry or global)
  LamBootLastEntry: string (entry ID)
  LamBootLastTimestamp: u64 (EFI time)
  LamBootFallbackEntry: string (configured fallback)

Boot flow:
  1. Read LamBootState
  2. If state == Booting → previous boot didn't complete → increment CrashCount
  3. If CrashCount >= threshold → state = CrashLoop → select fallback entry
  4. Set state = Booting, record entry ID and timestamp
  5. Boot selected entry
  6. [OS side]: systemd service or init script sets state = BootedOK and resets counter

Fallback selection:
  1. If configured fallback exists → use it
  2. Else: skip current entry, try next in list
  3. If all entries exhausted → show menu with warning, wait for user
```

**Compatibility with systemd-boot:** Also support the `+N` filename suffix convention so that `systemd-bless-boot.service` can bless LamBoot entries without modification.

### 3.5 Proxmox Integration — NVRAM Variable Protocol

**Architecture:**
```
[VM Guest]                          [Proxmox Host]
LamBoot writes NVRAM vars    →    OVMF_VARS.fd file on host storage
  - LamBootState                    ↓
  - LamBootCrashCount          lamboot-monitor reads vars
  - LamBootLastEntry              ↓
  - LamBootLastTimestamp       Alerts via Proxmox API/webhook
                                   ↓
                               Optional: auto-action on boot loop
                               (stop VM, rollback snapshot, notify admin)
```

The `lamboot-monitor` tool (Python, runs on Proxmox host):
1. Periodically scans VM OVMF_VARS files
2. Extracts LamBoot UEFI variables
3. Detects boot loops (state stuck in Booting, high crash count)
4. Sends alerts via Proxmox notification system or webhook
5. Optionally triggers recovery actions (snapshot rollback, VM stop)

### 3.6 Module System — Chainloaded EFI Apps

**Architecture:**
```
\EFI\LamBoot\modules\
├── manifest.toml          # Module metadata
├── nvme-diag.efi          # NVMe SMART health check
├── mem-quick.efi          # Quick memory test
├── net-diag.efi           # Network interface diagnostic
└── edid-view.efi          # Display information viewer
```

Modules are standalone EFI applications loaded via chainload. They appear as boot entries in the menu under a "Diagnostics" section. No special ABI needed — standard UEFI app conventions.

For the MVP, ship 1-2 sample modules. The framework is the directory scanning + menu integration, which already works via `discover_tools()`.

---

## 4. Development Roadmap

### 4.1 Phase 1: Fix Foundations (2 weeks)

**Goal:** Make the existing code actually work correctly.

**Week 1:**
- F1: Framebuffer rendering (replace per-pixel BLT)
- F2: Complete bitmap font (embed VGA 8x8)
- F3: Dynamic resolution
- F5: Timestamps

**Week 2:**
- F4: Fix TOML parser (section-aware, arrays)
- F6: Proper GUID
- Test in QEMU: verify GUI renders correctly, policy loads, timestamps work

**Deliverable:** Working bootloader that displays a correct GUI in QEMU.

### 4.2 Phase 2: Core Capabilities (3 weeks)

**Goal:** Boot real Linux systems reliably.

**Week 3:**
- F7: BLS Type 1 parser
- F9: Initrd loading

**Week 4:**
- F8: Crash recovery state machine
- F10: EFI driver loading

**Week 5:**
- F11: Ship ext4 driver
- F12: Multi-partition scanning
- F13: BLS autoconfiguration
- Integration testing: boot actual Linux VMs in Proxmox

**Deliverable:** Bootloader that can discover and boot Linux systems via BLS entries with ext4 support.

### 4.3 Phase 3: Proxmox Integration (2 weeks)

**Goal:** Demonstrate unique value for VM environments.

**Week 6:**
- F14: NVRAM boot health protocol
- F15: Proxmox monitor script
- F17: Serial console mode

**Week 7:**
- F16: VM template installer
- F18: Boot report via UEFI variables
- End-to-end testing: VM template → boot → monitor → crash recovery

**Deliverable:** Complete Proxmox integration demo with boot health monitoring.

### 4.4 Phase 4: Production Readiness (3 weeks)

**Goal:** Ready for open-source release and real-world use.

**Week 8:**
- F19: Secure Boot support (shim integration)
- F20: kernel-install plugin

**Week 9:**
- F21: TPM measured boot
- F22: Boot counting (systemd-compatible +N suffix)

**Week 10:**
- F23: Diagnostic module framework + 1-2 sample modules
- F24: ARM64 support
- Documentation, packaging, release preparation

**Deliverable:** Open-source release with Proxmox-focused documentation and guides.

---

## 5. Technical Specifications

### 5.1 BLS Entry Parser Specification

```
Input: /loader/entries/*.conf files on any discovered SimpleFileSystem volume
Output: Vec<BootEntry> with populated fields

Required fields:
  title → BootEntry.name
  linux → BootEntry.kind.LinuxLegacy.kernel_path (or Uki.path if .efi)
  initrd → BootEntry.kind.LinuxLegacy.initrd_path
  options → BootEntry.kind.LinuxLegacy.options

Optional fields:
  version → stored in BootEntry metadata for display
  machine-id → used for entry grouping
  sort-key → used for ordering
  architecture → filter for current platform
  efi → if present, treat as Chainload entry

Boot counting support:
  filename "linux-6.1+3.conf" → entry with 3 remaining tries
  On each boot: rename to decrement (+3 → +2)
  At +0: skip entry, select next
```

### 5.2 NVRAM Variable Specification

```
Vendor GUID: {4C414D42-4F4F-5400-0000-000000000001} ("LAMBOOT\0...")

Variables:
  LamBootState       : UINT8  : 0=Fresh, 1=Booting, 2=BootedOK, 3=CrashLoop
  LamBootCrashCount  : UINT8  : 0-255, resets on BootedOK
  LamBootLastEntry   : CHAR16 : Entry ID string (max 64 chars)
  LamBootTimestamp    : UINT64 : EFI_TIME packed as epoch seconds
  LamBootVersion     : UINT32 : LamBoot version (major.minor.patch packed)
  LamBootFallback    : CHAR16 : Fallback entry ID (from policy)

Attributes: BOOTSERVICE_ACCESS | RUNTIME_ACCESS
  (RUNTIME_ACCESS allows OS-side tools to read/write)
```

### 5.3 EFI Driver Loading Specification

```
Startup sequence:
  1. Mount ESP
  2. Read \EFI\LamBoot\drivers\ directory
  3. For each *.efi:
     a. LoadImage(parent_image, buffer)
     b. StartImage(driver_handle) → driver installs DriverBindingProtocol
  4. For each block device handle:
     a. ConnectController(handle, null, null, true) → recursive
  5. Now SimpleFileSystem handles exist for all supported filesystems

Driver discovery:
  - Ship ext4_x64.efi (from rEFInd, GPL-compatible with Apache-2.0 project)
  - Optional: btrfs_x64.efi, ntfs_x64.efi
  - Users can add custom drivers to the directory

Error handling:
  - Driver load failures are logged but non-fatal
  - Missing drivers directory is normal (ESP-only mode)
```

### 5.4 Proxmox Monitor Tool Specification

```
Tool: lamboot-monitor (Python 3, runs on Proxmox host)

Operation:
  1. Enumerate VMs with OVMF (bios=ovmf in /etc/pve/qemu-server/*.conf)
  2. For each VM:
     a. Locate OVMF_VARS file (from efidisk0 config)
     b. Mount via qemu-nbd (read-only)
     c. Extract LamBoot UEFI variables
     d. Assess boot health:
        - State=Booting for >5 minutes → likely stuck
        - CrashCount >= threshold → boot loop
        - State=CrashLoop → confirmed boot loop
     e. Unmount
  3. Report findings:
     - Healthy VMs: state=BootedOK, count=0
     - Warning VMs: state=Booting (recent)
     - Critical VMs: state=CrashLoop or high count
  4. Alert via:
     - Proxmox notification system (pvesh)
     - Webhook (configurable URL)
     - Email (via Proxmox mail config)
     - Log file

Deployment:
  - Cron job or systemd timer on Proxmox host
  - Configurable check interval (default: 5 minutes)
  - Configurable thresholds per VM or globally
```

---

## 6. Open Questions

1. **Licensing of shipped drivers:** rEFInd's ext4 driver is GPL-2.0. LamBoot is MIT/Apache-2.0. Can we ship GPL drivers alongside without contaminating the license? (Answer: yes, as separate binaries, but need to document clearly.)

2. **BLS vs custom entries priority:** When both BLS and custom entries exist for the same kernel, which takes precedence? Recommend: custom entries override BLS for the same ID.

3. **Secure Boot strategy:** Self-signed for development/testing, or pursue shim signing from the start? Recommend: shim integration first (path of least resistance), pursue Microsoft signing later if demand warrants.

4. **Scope of Proxmox integration:** Should lamboot-monitor be a Proxmox plugin (with web UI) or a standalone script? Recommend: standalone script first, then pursue Proxmox plugin if adoption warrants.

5. **GRUB coexistence:** Should LamBoot be able to chainload GRUB and vice versa? Recommend: yes — chainloading already works, and it allows gradual migration.

---

## 7. Success Criteria

### MVP (Phase 1-2 complete):
- [ ] Boots a Linux VM in Proxmox via BLS entries
- [ ] Auto-detects Windows if present
- [ ] GUI renders correctly at multiple resolutions
- [ ] Crash recovery triggers fallback after N failures
- [ ] ext4 driver loads and /boot is readable

### Production (Phase 3-4 complete):
- [ ] Proxmox monitor detects boot loops in a fleet of VMs
- [ ] Secure Boot chain works with shim
- [ ] kernel-install integration: new kernels appear automatically
- [ ] ARM64 builds work in QEMU aarch64
- [ ] At least one diagnostic module ships
- [ ] Documentation sufficient for community adoption

---

## 8. Dependencies and Prerequisites

| Dependency | Source | License | Purpose |
|-----------|--------|---------|---------|
| uefi-rs 0.36+ | crates.io | MIT/Apache-2.0 | Core UEFI abstractions |
| ext4_x64.efi | rEFInd project | GPL-2.0 | Filesystem driver (shipped separately) |
| OVMF | EDK2/TianoCore | BSD-2-Clause | Testing firmware |
| QEMU | qemu.org | GPL-2.0 | Testing environment |
| Python 3 | System | PSF | Proxmox monitor tool |
| qemu-nbd | QEMU | GPL-2.0 | NVRAM variable extraction |

---

## 9. Estimated Timeline

| Phase | Duration | Cumulative |
|-------|----------|------------|
| Phase 1: Fix Foundations | 2 weeks | 2 weeks |
| Phase 2: Core Capabilities | 3 weeks | 5 weeks |
| Phase 3: Proxmox Integration | 2 weeks | 7 weeks |
| Phase 4: Production Readiness | 3 weeks | 10 weeks |

**Total estimated: ~10 weeks of focused development** to go from prototype to production-ready open-source release.
