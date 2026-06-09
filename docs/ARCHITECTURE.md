# LamBoot Architecture

## Boot Flow

LamBoot executes a 10-phase boot sequence:

```
Phase 1: Health Assessment
    Read previous BootState from NVRAM
    If prev=Booting: increment crash counter (previous boot failed)
    If prev=BootedOK/Fresh: reset crash counter
    Set state=Booting, write timestamp
    Set Boot Loader Interface variables (LoaderInfo, LamBootVersion)
    NVRAM self-install (if [boot-entry] self_install, default on):
        ensure a labeled Boot#### entry pointing at
        \EFI\LamBoot\lambootx64.efi exists and front-load BootOrder,
        creating it from the UEFI environment if absent — idempotent,
        keyed on the exact "LamBoot" description (no efibootmgr, no OS;
        Boot####/BootOrder are non-authenticated, so no Secure Boot keys)

Phase 2: Security Initialization
    Read SecureBoot EFI variable
    Check for ShimLock protocol (shim loaded?)
    Log Secure Boot state: disabled / active+shim / active+direct
    Initialize TPM context (check TCG2 protocol, tpm_present())

Phase 3: Mount ESP
    Get LoadedImage protocol from our image handle
    Get device handle from LoadedImage
    Open SimpleFileSystem on device handle
    Open volume root directory -> EspVolume

Phase 4: Load Policy
    Read \EFI\LamBoot\policy.toml
    Parse with section-aware TOML parser (qualified keys)
    On failure: use defaults (4s timeout, threshold=2)
    Measure config into TPM PCR 5

Phase 5: Load Legacy Filesystem Drivers (fallback only)
    Scan \EFI\LamBoot\drivers\ for *.efi files
    Skip any driver whose filesystem is covered by a native in-binary reader
        (e.g. xfs_x64.efi / zfs_x64.efi are skipped — native readers mount those)
    For each remaining: LoadImage() + StartImage() (driver registers DriverBinding)
    ConnectController(recursive=true) on ALL handles
    New SimpleFileSystem handles now available for any filesystem still served
        by a firmware driver. Native readers (ext4, btrfs, FAT, XFS, exFAT, ZFS,
        plus the lamfold media stack) do not depend on this phase.

Phase 6: Enumerate Volumes
    Discover partitions from three sources, de-duplicated by handle:
        GPT via PartitionInfo, MBR via PartitionInfo.mbr_partition_record(),
        and BlockIO-only logical partitions (firmware that installs
        PartitionInfo on the ESP alone)
    find_handles::<SimpleFileSystem>() -> all filesystem handles
    Open each as EspVolume
    Result: ESP + any newly accessible partitions (GPT, MBR, or BlockIO-only)

Phase 7: Discover Boot Entries
    For each volume: scan /loader/entries/*.conf (BLS Type 1)
        Parse each .conf file (14 fields, multi-value initrd/options)
        Filter by architecture, apply policy allowlist/denylist
        Sort by: bad-entries-last, sort-key, machine-id, version (UAPI.10)
    ESP fallback: scan for Windows, UKI, GRUB, rEFInd, tools
    Legacy distro scanning only if no BLS entries found
    Deduplicate by path (BLS entries take precedence)

Phase 8: Crash Loop Check
    If crash_counter >= crash_threshold:
        Try fallback_order entries from policy
        If no fallback found: fall through to menu

Phase 9: Interactive Menu
    If GOP available: graphical menu (double-buffered framebuffer)
        Render to off-screen Vec<BltPixel> buffer
        Single BltOp::BufferToVideo per frame (~60 FPS)
        Mouse + keyboard input
    If no GOP: text console (SimpleTextOutput)
        Numbered entries, 0-9 keys + arrow keys
    Auto-boot on timeout (configurable, disabled for tool-only entries)
    System actions always visible at bottom of menu:
        F2: Reboot to Firmware Setup (sets OsIndications, cold resets)
        F12: Cold reboot
    If no bootable entries found: show recovery screen with system actions
    If crash loop detected: disable auto-boot, wait for manual selection

Phase 10: Boot Handoff
    Record boot entry to NVRAM (LamBootLastEntry, LoaderEntrySelected)
    Write boot report to ESP (\EFI\LamBoot\reports\boot.json)
    If boot-counted entry: decrement tries_left, rename .conf, set LoaderBootCountPath
    Measure kernel cmdline into TPM PCR 12
    For Linux: search ALL volumes (ESP + driver-exposed) for kernel/initrd
    For Linux: read initrd, register LoadFile2 protocol (LINUX_EFI_INITRD_MEDIA_GUID)
    Set kernel load options (command line)
    start_image() -> kernel takes control
    If kernel returns: mark_boot_success(), cleanup initrd handle
    If menu returns error: reboot to firmware setup (safety net)
```

## Memory Model

LamBoot uses UEFI Boot Services memory allocation:
- `global_allocator` feature provides Rust's `alloc` via UEFI pool allocation
- Framebuffer: `Vec<BltPixel>` of width * height (~8MB at 1920x1080)
- Initrd: `Box<[u8]>` leaked via `Box::into_raw()` for stable addresses during LoadFile2
- Font: Terminus Bold bitmap fonts — 16px (8x16, 4KB) and 32px (16x32, 16KB), compiled into .rodata

No heap fragmentation concerns since UEFI apps run single-threaded with a flat memory model. The bootloader exits Boot Services before the kernel, so all UEFI memory is reclaimed.

## NVRAM Variable Protocol

All LamBoot variables use:
- Vendor GUID: `4C414D42-4F4F-5400-0000-000000000001` (ASCII "LAMBOOT")
- Attributes: `BOOTSERVICE_ACCESS | RUNTIME_ACCESS`
- RUNTIME_ACCESS allows the running OS to read/write variables

The Boot Loader Interface variables use the systemd vendor GUID (`4a67b082-...`) for compatibility with `bootctl`, `systemd-bless-boot`, and other tools that expect these standard variables.

## LoadFile2 Initrd Protocol

The initrd delivery mechanism follows the same pattern as systemd-boot and Sprout:

```
1. Read initrd file into Vec<u8>
2. Leak the Vec via Box::into_raw() (stable address)
3. Build VenMedia device path with LINUX_EFI_INITRD_MEDIA_GUID
4. Create InitrdProvider struct (first field = LoadFile2 function pointer)
5. Install DevicePathProtocol on new handle
6. Install LoadFile2Protocol on same handle
7. start_image(kernel) -> kernel EFI stub discovers initrd via LocateDevicePath
8. Kernel calls our callback:
   - NULL buffer -> return BUFFER_TOO_SMALL with size
   - Valid buffer -> memcpy data, return SUCCESS
9. On cleanup (Drop): uninstall both protocols, reclaim memory
```

RAII via Rust's Drop trait ensures cleanup even on early returns.

## Filesystem Backends: Native Readers and Legacy Driver Fallback

LamBoot reads on-disk filesystems through native, in-binary `no_std` Rust
backends — there is no firmware filesystem driver in the primary path. Each
volume is probed by superblock magic and dispatched to the matching backend
behind a common `FsBackend` trait:

- **ESP / FAT**: the firmware's own SimpleFileSystem (plus a native read-only
  `FatRo` reader for non-ESP FAT volumes the firmware refuses to serve).
- **ext4, btrfs**: native readers.
- **XFS** (`lamxfs`), **exFAT** (`lamexfat`), **ZFS** (`lamzfs`): native
  read-only readers added in v0.16.0. XFS also mounts on an LVM logical volume
  and inside an `.iso` loopback region; exFAT mounts whole partitions only; ZFS
  reads single-disk, mirror, and single-parity RAIDZ1 boot pools only.
- **EROFS, ISO 9660, SquashFS, cramfs, romfs, UDF**: the native read-only
  `lamfold` media stack.

Every non-FAT backend is read-only by construction (no write path; the ESP
writer only ever targets the FAT backend).

A policy-gated legacy path still exists for any filesystem a native reader does
not cover. It follows the UEFI DriverBinding pattern:

```
1. Read .efi driver file from ESP
2. Skip drivers whose filesystem is natively covered (e.g. xfs_/zfs_ — the
   native readers mount those; loading the GPL EfiFs driver here would attach
   BY_DRIVER and shadow the native backend)
3. LoadImage() -> firmware creates image handle
4. StartImage() -> driver's entry point runs, registers DriverBindingProtocol
5. After all drivers loaded:
   ConnectController(recursive=true) on ALL handles in the system
6. Firmware matches DriverBinding to block devices
7. New SimpleFileSystem handles appear for supported partitions
```

The bundled EfiFs `.efi` drivers still ship as inert fallback, but the native
in-binary readers are what actually mount ext4/btrfs/XFS/exFAT/ZFS.

## BLS Entry Sorting

The sort algorithm implements the full UAPI Group specification:

```
Tier 1: Boot count state
    Bad entries (tries_left == 0) sort LAST

Tier 2: Sort-key presence
    Entries WITH sort-key sort BEFORE entries without

Tier 3: Multi-field comparison (both have sort-key)
    sort-key: ascending strcmp
    machine-id: ascending strcmp
    version: DESCENDING UAPI.10 comparison (newest first)

Tier 4: Filename fallback (no sort-key or all fields equal)
    Entry ID: DESCENDING UAPI.10 comparison
```

### UAPI.10 Version Comparison

The version comparison algorithm handles:
- `~` creates pre-release: `1.0~rc1 < 1.0`
- `^` creates post-release: `1.0 < 1.0^post1`
- Numeric segments compared as integers (leading zeros stripped)
- Alphabetic segments: uppercase sorts LOWER than lowercase
- Separators (`_`, `+`) are skipped

## Security Model

### Secure Boot

Three modes of operation:

1. **Shim 16.1+**: Shim overrides SystemTable's LoadImage/StartImage. LamBoot's standard `boot::load_image()` calls go through shim's verification hooks transparently. No bootloader code changes needed.

2. **Legacy shim**: LamBoot detects ShimLock protocol and uses `shim_lock.verify()` to validate images before loading.

3. **Direct signing**: LamBoot binary is signed with `sbsign` against the machine's db key. Firmware verifies the signature during initial load.

### TPM Measured Boot

Measurements follow the Linux TPM PCR Registry:
- PCR 4: Kernel image (using PE_COFF_IMAGE flag for Authenticode hash)
- PCR 5: Boot configuration (policy.toml raw bytes)
- PCR 12: Kernel command line (UTF-16 without trailing NUL)

All measurements use `hash_log_extend_event` which hashes the data, extends the PCR, and logs the event in the TCG event log. This is compatible with `systemd-cryptenroll` for TPM-bound disk encryption.

## Module Manifest

Diagnostic modules in `\EFI\LamBoot\modules\` are discovered via `discover_tools()`. An optional `manifest.toml` provides friendly names:

```toml
[modules.nvme-diag]
name = "NVMe Health Diagnostic"

[modules.mem-quick]
name = "Quick Memory Test"
```

Modules with `Icon::Tools` are excluded from the auto-boot timeout — only real OS entries (BLS, UKI, Windows, other bootloaders) trigger auto-boot.

## Error Handling

LamBoot follows these principles:
- **Never block boot for optional features**: TPM absent? Skip. No drivers directory? ESP-only mode. ShimLock unavailable? Use standard LoadImage.
- **Never trap the user**: Recovery options (F2 firmware setup, F12 reboot) always visible. No-entries screen shows diagnostic info. Menu error → automatic reboot to firmware setup.
- **Cascade to simpler modes**: No GOP? Text console. No BLS entries? Legacy scanning. Crash loop? Fallback entry.
- **Search all volumes**: Kernels and initrds may live on any volume a native reader can mount (ext4, btrfs, XFS, exFAT, ZFS boot pools, FAT, and the lamfold media filesystems), or on a volume served by a legacy firmware driver — not just the FAT ESP.
- **Log everything**: All errors are logged via the `log` crate, captured by UEFI's debug infrastructure.
- **Report to ESP**: Boot reports with timestamps go to `\EFI\LamBoot\reports\boot.json`.

## Hardware Detection (Phases 2.5–2.8)

Added in v0.2.0, these phases run between security init and ESP mount:

**Phase 2.5 — SMBIOS**: Walks SMBIOS 2.x/3.x structure table for Type 1 (System Information: manufacturer, product, serial) and Type 11 (OEM Strings). OEM strings with `lamboot.KEY=VALUE` prefix are parsed for VMID, fleet-id, and other host-injected tags.

**Phase 2.6 — Hypervisor Detection**: CPUID leaf 0x40000000 to detect KVM, Hyper-V, VMware, Xen, Parallels, VirtualBox. x86_64 only.

**Phase 2.7 — IOMMU Detection**: Walks ACPI RSDP → XSDT/RSDT → DMAR (Intel VT-d) or IVRS (AMD-Vi) tables. Extracts DRHD/IVHD hardware unit descriptions with PCI device scope paths. Reports RMRR reserved memory regions.

**Phase 2.8 — fw_cfg Data Channel**: Reads QEMU fw_cfg I/O ports (0x510/0x511, x86_64 only). Reads VM Generation ID from `etc/vmgenid_guid` for snapshot detection. Reads optional `opt/lamboot/config` for host-injected configuration.

## Persistent Boot Log

The `bootlog.rs` module provides crash-safe boot logging to `\EFI\LamBoot\reports\boot.log`:

- **Write-through mode** (phases 1–8): Every log entry is appended to the ESP file immediately. If LamBoot crashes during init, the log captures how far it got.
- **Buffered mode** (phase 9, menu): Log entries accumulate in memory. Reduces I/O during the interactive menu where crashes are unlikely.
- **Flush**: All buffered content is written to ESP before booting the selected entry.
- **Size cap**: 64 KB maximum. Previous boot's log is overwritten on each boot.

## Two-Column GUI Layout

The GUI (`gui.rs`) uses a two-column layout:

- **Left column (55%)**: Boot entries — kernels, UKIs, EFI loaders. Selection index 0..boot_count.
- **Right column (40%)**: Tools + system actions. Separate scroll state.
- **Navigation**: Up/Down within a column, Left/Right to switch columns.
- **Header**: Logo + title (left), VMID + hypervisor + build info (right).
- **Footer**: Status message (left), keyboard hints (right).

The GUI opens GOP with `open_protocol_exclusive`, which disconnects OVMF's GraphicsConsole driver. This is necessary for direct framebuffer access but has a critical side effect.

## GraphicsConsole Reconnection

When the GUI opens GOP exclusively, OVMF's GraphicsConsole driver (which renders text via GOP) is disconnected. After the GUI closes, text-mode ConOut becomes invisible — child images appear to hang but are actually running with no visible output.

**Fix**: Before every `start_image` call (in `chainload_efi`, `boot_uki`, `boot_linux`), LamBoot calls `connect_controller(gop_handle, None, None, true)` to reconnect the GraphicsConsole driver. This restores text rendering for child applications.

This pattern is documented in `boot.rs::reconnect_console_drivers()`.

## Extra Volume Scanning Limitation

After mounting native backends (and reconnecting controllers for any legacy driver volumes), `enumerate_volumes()` discovers every mountable volume — including the root filesystem partition. However, directory traversal of Linux-native formats (ext4, btrfs, XFS) on large populated filesystems is slow under UEFI's single-threaded I/O model. Operations like `read_dir("\\loader\\entries")` on a 37 GB ext4 root partition hang indefinitely.

**Current limitation**: BLS entry scanning is ESP-only. Extra volume BLS scanning and OS identification are disabled. This is sufficient for Fedora and Debian, which write BLS entries to the ESP regardless of XBOOTLDR partition presence.

**Impact**: Kernels and initrds on non-ESP partitions are still accessible for loading (targeted file reads work fine) — only directory enumeration is affected.

## UKI Two-Pass PE Parsing

Unified Kernel Images (UKIs) embed kernel, initrd, and metadata in a single PE binary (60–100 MB on Fedora). Reading the entire file into memory for metadata extraction would exhaust UEFI pool memory.

**Solution** (`uki.rs::read_uki_metadata`):
1. **Pass 1**: Read 4 KB (PE headers + section table). Parse section offsets and sizes.
2. **Pass 2**: For each target section (`.osrel`, `.cmdline`, `.uname`), seek to its offset and read only that section. Sections > 64 KB are skipped (these are `.linux` and `.initrd` — the large binary payloads).

This reads ~5 KB total instead of 60+ MB.

## Menu → Boot Loop

When a chainloaded tool (diagnostic module) returns `Status::SUCCESS`, LamBoot re-enters the menu loop instead of rebooting. The `run_bootloader` function wraps the menu + boot sequence in a loop:

1. Display menu, wait for selection
2. Record selection, write reports, flush boot log
3. Call `boot_entry()`
4. If the entry was a tool (`Icon::Tools`) and `start_image` returned: reclaim volumes, loop to step 1
5. If the entry was a kernel/UKI: kernel takes over (never returns)

This allows running multiple diagnostic tools in succession without rebooting.

## Boot-from-ISO (opt-in, default off)

Added in v0.16.0, LamBoot can boot a Linux distribution directly from an `.iso`
image rather than from an installed `/boot`. It is **opt-in and off by default**
— controlled entirely by the `[boot-from-iso]` section of `policy.toml`:

```toml
[boot-from-iso]
enabled = false   # boot from an .iso file on a mounted volume
optical = false   # boot from a physical CD/DVD/BD
```

Both keys default `false`; a default-config boot is byte-for-byte unaffected.

When `enabled`, ISO discovery scans `/isos` and `/boot/isos` for `*.iso` on any
mounted volume (ext4/btrfs/xfs/FAT, including the ESP). A file-hosted ISO is
read through a `FileBlockSource` over the holding backend's already-open handle,
so reading an ISO off the ESP never disconnects the FAT driver. When `optical`,
discovery enumerates 2048-byte-sector `BlockIO` handles carrying a `CD001`
primary volume descriptor, opened non-exclusively (an exclusive open is
`ACCESS_DENIED` while the firmware ISO9660 driver holds the disc `BY_DRIVER`).

Kernel/initrd resolution has two paths:

- **Path A1 — `loopback.cfg`**: LamBoot parses the distribution's own
  `/boot/grub/loopback.cfg`, taking the kernel, first initrd, and command line
  from the first bootable `menuentry` and substituting `${iso_path}` with the
  real path, so the distro's exact iso-find token is reused with no per-distro
  knowledge.
- **Path A2 — per-distro-family fallback table**: when no usable `loopback.cfg`
  exists (and always for an optical disc, which has no `${iso_path}`), LamBoot
  fingerprints the distro family by marker paths and applies a static recipe
  table covering arch, ubuntu-casper, debian-live, fedora, opensuse, and alpine
  (each transitively covering its derivatives).

Distribution kernels are Linux EFI-stub PEs. Since v0.16.1 LamBoot's native PE
loader reads the EFI-stub layout directly (a hand-rolled `no_std` PE/COFF reader
that parses only the load-bearing COFF + PE32+ optional-header fields from fixed
offsets, with no third-party PE parser; the old `goblin`-based loader rejected
the DOS-stub-less image). If the native load fails it falls back to firmware
`LoadImage`, the same native -> firmware fallback that `boot_linux` uses for any
Linux kernel. The kernel and initrd are then handed off through the same measure
/ verify / initrd-register path as a normal Linux boot.

**Validation status**: only Arch 2026.05 and Fedora 44 are live-ISO-booted end
to end; the other families and all derivatives are recipe/table-validated and
host-unit-tested, not yet live-booted. El Torito chainload (Path B) is not yet
wired. Media mounts (the lamfold stack) are read-only but currently unverified —
there is no integrity / trust-root check of media content yet.

## See Also

- [User Guide](USER-GUIDE.md) — usage reference
- [Configuration Guide](CONFIGURATION-GUIDE.md) — all configurable options
- [Security Guide](SECURITY-GUIDE.md) — TPM, Secure Boot, crash loop internals
