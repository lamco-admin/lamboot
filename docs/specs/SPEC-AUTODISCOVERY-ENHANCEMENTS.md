# SPEC: Autodiscovery Enhancements

**Status:** Draft
**Date:** 2026-04-03
**Scope:** 8 independent enhancements to the Phase 7 boot entry discovery pipeline
**Principle:** Each enhancement is independently implementable and independently fallible. No enhancement blocks boot or hides entries.
**References:** [UAPI.2 Discoverable Partitions](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/), [UAPI.1 BLS](https://uapi-group.org/specifications/specs/boot_loader_specification/)

---

## 1. Overview

LamBoot's discovery pipeline (Phase 7) currently covers BLS entries and ESP fallback scanning. These 8 enhancements add automatic detection, resolution, and healing capabilities that no other bootloader provides.

### Enhancement Summary

| # | Enhancement | Primary Value | New Module |
|---|------------|--------------|------------|
| 1 | Discoverable partitions | Auto `root=` generation | `gpt.rs` |
| 2 | XBOOTLDR detection | systemd-boot layout compat | `gpt.rs` |
| 3 | Initrd auto-discovery | Handle missing initrd | `discovery.rs` |
| 4 | Kernel path cross-referencing | Resolve broken paths | `discovery.rs` |
| 5 | OS identification from root partitions | Beautiful multi-boot menus | `discovery.rs` |
| 6 | Filesystem superblock probing | UUID/label extraction | `gpt.rs` |
| 7 | Stale entry detection | Clean up deleted kernels | `discovery.rs` |
| 8 | vmgenid crash counter reset | Fix false Proxmox crash loops | `health.rs` |

---

## 2. Discoverable Partitions (UAPI.2)

### 2.1 Purpose

Scan the GPT partition table for architecture-specific root partition type GUIDs. When a BLS or LinuxLegacy entry lacks `root=` in its options, automatically generate `root=PARTUUID=<uuid>`.

**No other bootloader does this at boot time.** systemd's `systemd-gpt-auto-generator` does it after kernel boot, but that's too late for kernels needing `root=` on the command line.

### 2.2 Partition Type GUIDs

```rust
/// Well-known GPT partition type GUIDs (UAPI.2 Discoverable Partitions Specification)
/// Stored as 16-byte arrays in mixed-endian format (first 3 groups LE, last 2 BE)

/// x86_64 Root: 4f68bce3-e8cd-4db1-96e7-fbcaf984b709
const ROOT_X86_64: [u8; 16] = [
    0xe3, 0xbc, 0x68, 0x4f,  // LE: 4f68bce3
    0xcd, 0xe8,              // LE: e8cd
    0xb1, 0x4d,              // LE: 4db1
    0x96, 0xe7,              // BE: 96e7
    0xfb, 0xca, 0xf9, 0x84, 0xb7, 0x09,  // BE: fbcaf984b709
];

/// aarch64 Root: b921b045-1df0-41c3-af44-4c6f280d3fae
const ROOT_AARCH64: [u8; 16] = [
    0x45, 0xb0, 0x21, 0xb9,
    0xf0, 0x1d,
    0xc3, 0x41,
    0xaf, 0x44,
    0x4c, 0x6f, 0x28, 0x0d, 0x3f, 0xae,
];

/// ESP: c12a7328-f81f-11d2-ba4b-00a0c93ec93b
const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1,
    0x1f, 0xf8,
    0xd2, 0x11,
    0xba, 0x4b,
    0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];

/// XBOOTLDR: bc13c2ff-59e6-4262-a352-b275fd6f7172
const XBOOTLDR_TYPE: [u8; 16] = [
    0xff, 0xc2, 0x13, 0xbc,
    0xe6, 0x59,
    0x62, 0x42,
    0xa3, 0x52,
    0xb2, 0x75, 0xfd, 0x6f, 0x71, 0x72,
];

#[cfg(target_arch = "x86_64")]
const NATIVE_ROOT_TYPE: [u8; 16] = ROOT_X86_64;
#[cfg(target_arch = "aarch64")]
const NATIVE_ROOT_TYPE: [u8; 16] = ROOT_AARCH64;
```

### 2.3 GPT Header Structure

```
LBA 1 (512 bytes into disk):
  Offset 0:   Signature "EFI PART" (8 bytes)
  Offset 8:   Revision (4 bytes)
  Offset 12:  Header size (4 bytes, usually 92)
  Offset 16:  Header CRC32 (4 bytes)
  Offset 20:  Reserved (4 bytes, zero)
  Offset 24:  MyLBA (8 bytes)
  Offset 32:  AlternateLBA (8 bytes)
  Offset 40:  FirstUsableLBA (8 bytes)
  Offset 48:  LastUsableLBA (8 bytes)
  Offset 56:  DiskGUID (16 bytes)
  Offset 72:  PartitionEntryLBA (8 bytes) — LBA of partition entry array
  Offset 80:  NumberOfPartitionEntries (4 bytes)
  Offset 84:  SizeOfPartitionEntry (4 bytes, usually 128)
  Offset 88:  PartitionEntryCRC32 (4 bytes)
```

### 2.4 GPT Partition Entry Structure

```
Each entry (128 bytes):
  Offset 0:   PartitionTypeGUID (16 bytes, mixed-endian)
  Offset 16:  UniquePartitionGUID (16 bytes, mixed-endian)
  Offset 32:  StartingLBA (8 bytes)
  Offset 40:  EndingLBA (8 bytes)
  Offset 48:  Attributes (8 bytes)
  Offset 56:  PartitionName (72 bytes, UTF-16LE)
```

### 2.5 Data Structure

```rust
pub(crate) struct PartitionInfo {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
}
```

### 2.6 Scanning Algorithm

```rust
/// Scan GPT partition table on the boot device.
/// Uses BlockIO protocol to read LBA 1 (GPT header) and partition entries.
pub(crate) fn scan_gpt_partitions(device: Handle) -> Vec<PartitionInfo>
```

Algorithm:
```
1. Open BlockIO protocol on device handle
2. Read LBA 1 (512 bytes) → verify "EFI PART" signature
3. Read PartitionEntryLBA, NumberOfPartitionEntries, SizeOfPartitionEntry
4. Read partition entry array (NumberOfPartitionEntries * SizeOfPartitionEntry bytes)
5. For each entry:
   a. If TypeGUID is all zeros → skip (unused entry)
   b. Parse TypeGUID, UniqueGUID, StartingLBA, EndingLBA
   c. Push to results
6. Return Vec<PartitionInfo>
```

**UEFI alternative:** Instead of raw BlockIO reads, use the `PartitionInfo` protocol or enumerate handles with `BlockIO` + `DevicePathProtocol` to find partitions. The UEFI firmware already parsed the GPT — we can query it through protocols rather than re-parsing the disk.

```rust
/// Find partitions matching a type GUID using UEFI protocol enumeration.
pub(crate) fn find_partitions_by_type(type_guid: &[u8; 16]) -> Vec<Handle>
```

This approach is simpler: iterate all `BlockIO` handles, check device path for Hard Drive Media node with matching partition type GUID.

### 2.7 Auto root= Generation

Integration point: after all entries are collected but before boot.

```rust
/// If a LinuxLegacy entry has no root= in options and a discoverable
/// root partition exists, append root=PARTUUID=<uuid> to the options.
pub(crate) fn auto_append_root(
    entries: &mut [BootEntry],
    partitions: &[PartitionInfo],
)
```

Algorithm:
```
1. Find first partition with type_guid == NATIVE_ROOT_TYPE
   If none found: return (no auto root= possible)
2. Format its unique_guid as PARTUUID string: "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
3. For each entry with EntryKind::LinuxLegacy:
   a. Check if options already contains "root="
   b. If not: append " root=PARTUUID={partuuid} rw" to options
   c. Log: "Auto root=: appended PARTUUID={partuuid} for entry {id}"
```

### 2.8 UUID Formatting

GPT stores UUIDs in mixed-endian format. The first 3 groups are little-endian, last 2 are big-endian:

```rust
/// Format a GPT-style mixed-endian UUID as a standard string.
fn format_gpt_uuid(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0],  // Group 1: LE
        bytes[5], bytes[4],                       // Group 2: LE
        bytes[7], bytes[6],                       // Group 3: LE
        bytes[8], bytes[9],                       // Group 4: BE
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],  // Group 5: BE
    )
}
```

---

## 3. XBOOTLDR Partition Detection

### 3.1 Purpose

The Extended Boot Loader Partition (XBOOTLDR) is used by systemd-boot for systems where `/boot` is a separate FAT or ext4 partition. Detecting it ensures compatibility with systemd-boot-style disk layouts.

### 3.2 Algorithm

```
1. During GPT scan (section 2), identify partition with type XBOOTLDR_TYPE
2. If found:
   a. Get partition's device handle
   b. Attempt ConnectController(recursive=true) to expose SimpleFileSystem
   c. Open SimpleFileSystem → Directory → EspVolume
   d. Add to extra_volumes array (same path as driver-loaded volumes)
3. The existing BLS scan loop in discover_all_entries() already scans extra_volumes
```

### 3.3 Integration

No new function needed — XBOOTLDR detection is a specific use of the GPT scanning from section 2. The partition handle is added to `extra_volumes` before the BLS scan loop at `discovery.rs:68`.

---

## 4. Initrd Auto-Discovery

### 4.1 Purpose

When a BLS entry specifies a kernel but no initrd, or the specified initrd doesn't exist, search for matching initrd files using distro-specific patterns.

### 4.2 Function Signature

```rust
/// Search for initrd files matching a kernel version.
/// Returns discovered initrd paths (may be empty).
pub(crate) fn find_initrd(
    kernel_version: &str,
    volume: &mut EspVolume,
    kernel_dir: &str,
) -> Vec<String>
```

### 4.3 Search Algorithm

```
1. EXTRACT version from kernel path
   "vmlinuz-6.12.0-200.fc43.x86_64" → "6.12.0-200.fc43.x86_64"
   "vmlinuz-linux" → "linux"
   Strip "vmlinuz-" or "linux-" prefix; if no prefix recognized, use full filename

2. TRY distro-specific patterns in order (stop at first match):
   Pattern                              | Example
   initramfs-{VERSION}.img             | initramfs-6.12.0-200.fc43.x86_64.img (Fedora)
   initrd.img-{VERSION}                | initrd.img-6.12.0-amd64 (Debian)
   initramfs-{VERSION}-fallback.img    | initramfs-linux-fallback.img (Arch)
   initrd-{VERSION}                    | initrd-6.12.0 (openSUSE)
   initramfs-{VERSION}.img             | initramfs-6.12.0.img (Void/Alpine)

3. SEARCH LOCATIONS (in order):
   a. Same directory as kernel (kernel_dir)
   b. Root of the volume (\)
   c. \boot\ (if different from above)

4. CHECK FOR MICROCODE (prepend to result):
   Look for intel-ucode.img or amd-ucode.img in same directory as kernel
   If found: prepend to initrd list (microcode must load first)

5. RETURN Vec of discovered paths (empty if nothing found)
```

### 4.4 Integration

Called in two places:
1. **During BLS discovery** (in `bls.rs`): If a parsed BLS entry has empty `initrd` vec, call `find_initrd()` to populate it
2. **During legacy discovery** (in `discovery.rs`): When creating LinuxLegacy entries from detected kernels

---

## 5. Kernel Path Cross-Referencing

### 5.1 Purpose

When a BLS entry references a kernel path that doesn't exist on the entry's source volume, search alternative locations before marking it as broken.

### 5.2 Function Signature

```rust
/// Attempt to resolve a broken kernel path across all volumes.
/// Returns the corrected path if found, None otherwise.
pub(crate) fn resolve_kernel_path(
    broken_path: &str,
    all_volumes: &mut [EspVolume],
) -> Option<String>
```

### 5.3 Algorithm

```
1. EXTRACT version string from broken_path
   e.g., "/vmlinuz-6.12.0-200.fc43.x86_64" → "6.12.0-200.fc43.x86_64"

2. GENERATE alternative paths:
   \boot\vmlinuz-{VERSION}
   \{VERSION}\linux
   \{VERSION}\vmlinuz
   \vmlinuz-{VERSION}
   \EFI\Linux\ (scan for UKI containing this version — via .uname)

3. For each alternative path, for each volume:
   If volume.exists(path):
     log::info!("Corrected kernel path: {broken_path} → {path}")
     Return Some(path)

4. Return None (preflight will flag as Error)
```

---

## 6. OS Identification from Root Partitions

### 6.1 Purpose

When filesystem drivers are loaded and non-ESP volumes are accessible, read `/etc/os-release` to generate `PRETTY_NAME`-based menu entries for multi-boot systems.

### 6.2 Function Signature

```rust
/// Read /etc/os-release from a volume and extract OS identity.
pub(crate) fn identify_os(volume: &mut EspVolume) -> Option<OsReleaseInfo>
```

### 6.3 Algorithm

```
1. Try reading \etc\os-release from volume
   If fails: try \usr\lib\os-release (fallback per spec)
   If both fail: return None

2. Parse using the same os-release parser from SPEC-UKI-PE-PARSER section 7
   (shared code: uki::parse_os_release())

3. Return OsReleaseInfo with pretty_name, id, version_id
```

### 6.4 Integration

Used by `discover_linux_legacy()` to replace generic distro names. When creating a LinuxLegacy entry from a volume with os-release, use `pretty_name` as the display name instead of path-based extraction.

---

## 7. Filesystem Superblock Probing

### 7.1 Purpose

Extract UUID and label from ext4/btrfs superblocks for `root=UUID=` generation when GPT PARTUUID is unavailable.

### 7.2 Superblock Structures

**ext4** (superblock at byte offset 1024):

| Offset (from superblock start) | Size | Field |
|-------------------------------|------|-------|
| 0x38 | 2 | Magic: `0xEF53` |
| 0x68 | 16 | UUID |
| 0x78 | 16 | Volume label (null-terminated) |

**btrfs** (superblock at byte offset 0x10000):

| Offset (from superblock start) | Size | Field |
|-------------------------------|------|-------|
| 0x40 | 8 | Magic: `_BHRfS_M` |
| 0x20 | 16 | UUID |
| 0x12B | 256 | Label (null-terminated) |

### 7.3 Function Signature

```rust
pub(crate) struct FsInfo {
    pub uuid: [u8; 16],
    pub label: Option<String>,
    pub fs_type: FsType,
}

pub(crate) enum FsType {
    Ext4,
    Btrfs,
    Unknown,
}

/// Probe a block device for filesystem superblock metadata.
/// Requires BlockIO protocol access on the device handle.
pub(crate) fn probe_superblock(handle: Handle) -> Option<FsInfo>
```

### 7.4 Algorithm

```
1. Open BlockIO protocol on handle
2. Read 4096 bytes at offset 0 (covers ext4 superblock at 1024)
3. Check ext4 magic at offset 1024+0x38:
   If bytes match 0x53EF (LE for 0xEF53):
     Extract UUID at offset 1024+0x68
     Extract label at offset 1024+0x78
     Return FsInfo { uuid, label, fs_type: Ext4 }

4. Read 4096 bytes at offset 0x10000 (btrfs superblock)
5. Check btrfs magic at offset 0x10000+0x40:
   If bytes match "_BHRfS_M":
     Extract UUID at offset 0x10000+0x20
     Extract label at offset 0x10000+0x12B
     Return FsInfo { uuid, label, fs_type: Btrfs }

6. Return None (unknown filesystem)
```

### 7.5 UUID Formatting

ext4 and btrfs store UUIDs in standard byte order (NOT mixed-endian like GPT):

```rust
fn format_fs_uuid(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}
```

---

## 8. Stale Entry Detection

### 8.1 Definition

A stale entry is a BLS entry whose referenced kernel file does not exist on any accessible volume. This typically occurs when a kernel package is uninstalled but the BLS `.conf` file is not cleaned up.

### 8.2 Detection

Performed by the preflight engine's `FileExists` check (see SPEC-PREFLIGHT-VALIDATION section 3.1). If `FileExists` fails for the kernel path, the entry is stale.

### 8.3 Sort Treatment

Stale entries sort after all non-stale entries. Within the existing UAPI.10 sort tiers, stale acts as a lower-priority tier:

```
Tier 0: Non-stale, non-bad entries (normal sort order)
Tier 1: Stale entries (kernel not found) — sorted by version descending among themselves
Tier 2: Bad entries (tries_left == 0) — existing behavior
```

### 8.4 Display

- **GUI:** Dimmed text (reduce alpha/brightness) + red `X` indicator from preflight
- **Text mode:** ` [stale]` suffix

### 8.5 Policy

```toml
[cleanup]
hide_stale = false   # If true, filter stale entries from menu (default: false)
```

### 8.6 Constraints

- LamBoot **never modifies or deletes** BLS `.conf` files — stale entries are displayed but demoted
- Stale entries are still selectable (the user might know the kernel exists on an unmounted volume)
- Boot report includes the stale entry list for admin awareness

---

## 9. vmgenid-Aware Crash Counter Reset

### 9.1 Purpose

QEMU VMs with the vmgenid device expose a 128-bit generation ID via ACPI that changes on snapshot restore, template clone, and some migrations. If a snapshot is restored to a state where `LamBootState=Booting`, the next boot incorrectly increments the crash counter. vmgenid detection prevents this false positive.

### 9.2 vmgenid Location

The vmgenid is a 128-bit GUID stored at an address provided by the VMGENID ACPI table. The ACPI SSDT contains a device with `_HID = "QEMUVGID"` and a variable `VGIA` pointing to the physical address of the generation ID.

**Simpler approach:** QEMU also exposes vmgenid via `fw_cfg` at `etc/vmgenid_guid`. Read it through I/O ports 0x510/0x511 (same fw_cfg device from Proxmox research).

### 9.3 NVRAM Variable

```
Variable:   LamBootVmGenId
GUID:       4C414D42-4F4F-5400-0000-000000000001 (LAMBOOT vendor)
Attributes: NV | BS_ACCESS | RT_ACCESS
Size:       16 bytes
Content:    Raw 128-bit vmgenid value
```

### 9.4 Integration with assess_boot_health()

Modified `health.rs::assess_boot_health()`:

```rust
pub(crate) fn assess_boot_health() -> Result<u8> {
    // --- NEW: vmgenid check (before crash detection) ---
    if let Some(current_genid) = read_vmgenid() {
        let stored_genid = read_stored_vmgenid();
        if stored_genid.is_none() || stored_genid.as_ref() != Some(&current_genid) {
            // VM generation changed — snapshot restored or template cloned
            log::info!("VM generation ID changed — resetting boot state");
            set_state(BootState::Fresh)?;
            runtime::set_variable(CRASH_COUNTER_VAR, LAMBOOT_VENDOR, ATTRS, &[0])?;
            write_vmgenid(&current_genid)?;
            return Ok(0); // Fresh state, zero crash counter
        }
    }

    // --- Existing crash detection logic (unchanged) ---
    let prev_state = get_state();
    let mut counter = get_crash_counter();
    // ... rest of existing function ...
}
```

### 9.5 vmgenid Reading

**Via fw_cfg:**
```rust
/// Read vmgenid from QEMU fw_cfg device.
/// Returns None on non-QEMU systems or if vmgenid is not configured.
fn read_vmgenid() -> Option<[u8; 16]> {
    // Select fw_cfg file directory (item 0x0019)
    // Scan for "etc/vmgenid_guid" entry
    // Read 16 bytes of GUID data
    // Return Some([u8; 16])
    //
    // If fw_cfg device not present or vmgenid not in directory: return None
}
```

**Implementation note:** fw_cfg access requires I/O port reads (0x510 selector, 0x511 data). On x86_64, this is done via `in`/`out` instructions, which require `unsafe` in Rust. The SAFETY comment should note that I/O port 0x510/0x511 is guaranteed to be the fw_cfg device on QEMU/KVM platforms, and that reading is side-effect-free.

### 9.6 Constraints

- If vmgenid device is not present (bare metal, non-QEMU): skip entirely, no NVRAM variable written
- vmgenid check runs **before** existing crash detection logic — it's a pre-condition
- A changed vmgenid always resets to Fresh/0 regardless of previous state

---

## 10. Revised Discovery Pipeline

### 10.1 Phase 7 Sub-Phases

```
Phase 7: Discover Boot Entries (Enhanced)
  ├── 7a. Scan BLS entries from all volumes (EXISTING)
  │     └── For entries with empty initrd: call find_initrd() [Enhancement 4]
  ├── 7b. Scan UKIs from \EFI\Linux\ (ENHANCED per SPEC-UKI-PE-PARSER)
  │     └── Parse PE sections for .osrel, .cmdline, .uname
  ├── 7c. GPT scan [Enhancement 1+2]
  │     ├── Find discoverable root partition → store for auto root=
  │     └── Find XBOOTLDR partition → add to extra_volumes, scan for BLS
  ├── 7d. Kernel path cross-referencing [Enhancement 4]
  │     └── For entries with broken kernel paths: search alternatives
  ├── 7e. ESP fallback scanning (EXISTING)
  │     └── Windows, GRUB, rEFInd, tools, legacy distro
  ├── 7f. OS identification [Enhancement 5]
  │     └── For legacy entries: read os-release from root partitions
  ├── 7g. Auto root= generation [Enhancement 1]
  │     └── Append root=PARTUUID= to entries without root=
  ├── 7h. Preflight validation (per SPEC-PREFLIGHT-VALIDATION)
  │     └── Run checks, populate entry.preflight
  ├── 7i. Deduplication (ENHANCED)
  │     └── BLS + UKI dedup by kernel version (not just filename)
  └── 7j. Sort with health indicators (ENHANCED)
        ├── Healthy entries first
        ├── Warning entries next (stale entries here)
        ├── Bad/Error entries last
        └── Within tiers: UAPI.10 version sort (newest first)
```

### 10.2 Dependency Ordering

- 7c (GPT scan) must complete before 7g (auto root= needs partition data)
- 7c (XBOOTLDR) should complete before 7a's extra_volumes scan
- 7b (UKI parsing) is independent of 7a (BLS scan)
- 7h (preflight) runs after 7d (cross-referencing) so resolved paths are checked
- 7i (dedup) needs both 7a and 7b complete

### 10.3 Performance Budget

| Sub-phase | Target Time |
|-----------|------------|
| 7a BLS scan (existing) | <100ms |
| 7b UKI scan + parse | <200ms (10 UKIs × 20ms each) |
| 7c GPT scan | <50ms |
| 7d Kernel cross-reference | <100ms |
| 7e ESP fallback | <50ms (existing) |
| 7f OS identification | <100ms per root partition |
| 7g Auto root= | <1ms |
| 7h Preflight | <500ms (see SPEC-PREFLIGHT) |
| 7i Dedup | <1ms |
| 7j Sort | <1ms |
| **Total Phase 7** | **<2 seconds** |

---

## 11. New Data Structures Summary

### From this spec:

```rust
// gpt.rs
pub(crate) struct PartitionInfo {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
}

pub(crate) struct FsInfo {
    pub uuid: [u8; 16],
    pub label: Option<String>,
    pub fs_type: FsType,
}

pub(crate) enum FsType {
    Ext4,
    Btrfs,
    Unknown,
}
```

### From SPEC-UKI-PE-PARSER:

```rust
// uki.rs
pub(crate) struct UkiMetadata { ... }
pub(crate) struct OsReleaseInfo { ... }
```

### From SPEC-PREFLIGHT-VALIDATION:

```rust
// preflight.rs
pub(crate) struct PreflightResult { ... }
pub(crate) struct CheckResult { ... }
pub(crate) enum Check { ... }
pub(crate) enum Severity { ... }
pub(crate) enum PreflightStatus { ... }
```

### BootEntry (extended):

```rust
pub(crate) struct BootEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub icon: Icon,
    pub bls_filename: Option<String>,
    pub os_name: Option<String>,         // from UKI .osrel or root os-release
    pub kernel_version: Option<String>,  // from UKI .uname or BLS version
    pub preflight: Option<PreflightResult>,
}
```

---

## 12. Error Handling

Each enhancement is independently fallible:

| Enhancement | Failure Mode | Log Level | Fallback |
|------------|-------------|-----------|----------|
| Discoverable partitions | GPT scan fails (non-GPT disk, BlockIO error) | `warn` | No auto root=; existing manual root= only |
| XBOOTLDR | Partition not found | `info` | Skip; no extra volumes |
| Initrd discovery | No pattern match | `info` | Entry has empty initrd; preflight warns |
| Kernel cross-ref | No alternative found | `info` | Keep broken path; preflight flags Error |
| OS identification | Can't read os-release | `debug` | Use generic distro name from path |
| Superblock probing | Unknown filesystem, read error | `debug` | Skip UUID extraction |
| Stale detection | N/A (handled by preflight) | — | — |
| vmgenid | Device not present | `debug` | Normal crash detection (no genid check) |

**No enhancement ever blocks boot. No enhancement hides an entry. No enhancement deletes files.**

---

## 13. Test Criteria

### 13.1 Per-Enhancement Tests

| Enhancement | Test Scenario | Expected Result |
|------------|--------------|-----------------|
| Discoverable partitions | Disk with x86_64 root type GUID, entry lacks root= | root=PARTUUID=... appended |
| Discoverable partitions | Disk with no discoverable partitions | No modification to entries |
| XBOOTLDR | XBOOTLDR partition with 2 BLS entries | Entries merged into main list |
| XBOOTLDR | No XBOOTLDR partition | No effect |
| Initrd discovery | Fedora: vmlinuz-6.12.0 without initrd | Finds initramfs-6.12.0.img |
| Initrd discovery | Debian: vmlinuz-6.12.0 without initrd | Finds initrd.img-6.12.0 |
| Initrd discovery | No matching initrd found | Empty initrd, preflight warns |
| Kernel cross-ref | BLS points to /vmlinuz-6.12.0 (missing), exists at /boot/vmlinuz-6.12.0 | Path corrected |
| Kernel cross-ref | No alternative found | Path unchanged, preflight Error |
| OS identification | Root partition with os-release | Entry name uses PRETTY_NAME |
| OS identification | Root partition without os-release | Generic name from path |
| Superblock probe | ext4 volume | UUID and label extracted |
| Superblock probe | btrfs volume | UUID and label extracted |
| Stale detection | BLS entry with deleted kernel | Entry shown with [stale] / dimmed |
| vmgenid | Changed genid (snapshot restore) | Crash counter reset to 0, state=Fresh |
| vmgenid | Same genid (normal reboot) | Normal crash detection |
| vmgenid | No vmgenid device (bare metal) | Normal crash detection |

### 13.2 Integration Test (QEMU)

1. Create a disk with: ESP (FAT32), root (ext4, discoverable type GUID), /boot on root
2. Install kernel + initrd on root, create BLS entry without root=
3. Load ext4 driver, boot LamBoot
4. Verify: auto root= appended, correct PARTUUID, initrd found via auto-discovery
5. Verify: OS name from root's /etc/os-release displayed in menu
6. Delete kernel file, reboot — verify stale indicator
7. Restore QEMU snapshot — verify crash counter resets (vmgenid)
