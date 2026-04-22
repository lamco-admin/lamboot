# SPEC: UKI PE Section Parser

**Status:** Draft
**Date:** 2026-04-03
**Scope:** Parse PE/COFF sections from Unified Kernel Image .efi files for metadata extraction
**References:** [UAPI.5 UKI Specification](https://uapi-group.org/specifications/specs/unified_kernel_image/), [systemd-stub(7)](https://manpages.debian.org/unstable/systemd-boot/systemd-stub.7.en.html)

---

## 1. Overview

LamBoot currently discovers UKI files by scanning `\EFI\Linux\*.efi` and displaying the filename as the menu entry name, with a hardcoded `"quiet splash"` command line. This spec defines a PE section parser that extracts rich metadata from UKI binaries.

**Scope:** Extract `.osrel`, `.cmdline`, and `.uname` sections. Future: `.profile`, `.hwids`, `.pcrsig`.

**Constraints:**
- `#![no_std]` — no standard library
- No external crate dependency (manual parser ~150 lines)
- No `unsafe` code
- Zero additional I/O — operates on existing `Vec<u8>` buffer from `EspVolume::read_to_vec()`

**Module location:** New file `lamboot-core/src/uki.rs`

---

## 2. Existing Code Context

### Current UKI Discovery (`discovery.rs:144-166`)

```rust
fn discover_linux_uki(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let uki_paths = esp.glob("\\EFI\\Linux\\*.efi");
    for path in uki_paths {
        // Only extracts filename — no PE parsing
        let name = extract_filename(&path).unwrap_or_else(|| String::from("Linux"));
        entries.push(BootEntry {
            name: format!("Linux ({name})"),
            kind: EntryKind::Uki {
                path: path.clone(),
                options: String::from("quiet splash"),  // HARDCODED
            },
            // ...
        });
    }
}
```

### Current BootEntry (`discovery.rs:9-17`)

```rust
pub(crate) struct BootEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub icon: Icon,
    pub bls_filename: Option<String>,
}
```

### Existing Version Comparison (`bls.rs:308-458`)

`bls::version_compare(a: &str, b: &str) -> Ordering` — Full UAPI.10 implementation. Reusable for UKI sorting.

---

## 3. PE/COFF Binary Structure

### 3.1 DOS Header

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| `0x00` | 2 | `e_magic` | `0x5A4D` ("MZ") |
| `0x02` | 58 | (unused fields) | — |
| `0x3C` | 4 | `e_lfanew` | u32 LE — offset to PE signature |

### 3.2 PE Signature

At offset `e_lfanew`: 4 bytes = `0x50450000` ("PE\0\0").

### 3.3 COFF Header (20 bytes, immediately after PE signature)

| Offset | Size | Field | Use |
|--------|------|-------|-----|
| +0 | 2 | `Machine` | `0x8664` = x86_64, `0xAA64` = aarch64 |
| +2 | 2 | `NumberOfSections` | Section count |
| +4 | 4 | `TimeDateStamp` | — |
| +8 | 4 | `PointerToSymbolTable` | — |
| +12 | 4 | `NumberOfSymbols` | — |
| +16 | 2 | `SizeOfOptionalHeader` | Skip this many bytes to reach section table |
| +18 | 2 | `Characteristics` | — |

### 3.4 Section Table

Offset: `e_lfanew + 4 + 20 + SizeOfOptionalHeader`

Each `IMAGE_SECTION_HEADER` is 40 bytes:

| Offset | Size | Field | Use |
|--------|------|-------|-----|
| +0 | 8 | `Name` | Null-padded ASCII (e.g., `.osrel\0\0`) |
| +8 | 4 | `VirtualSize` | Size in memory (used for TPM measurement) |
| +12 | 4 | `VirtualAddress` | — |
| +16 | 4 | `SizeOfRawData` | **Size of section data on disk** |
| +20 | 4 | `PointerToRawData` | **File offset to section data** |
| +24 | 16 | (relocation/line fields) | — |

### 3.5 Section Data

Located at `PointerToRawData` in the file, length `SizeOfRawData` bytes.

---

## 4. Target Sections

| Section Name | 8-Byte Padded | Content Type | Priority |
|-------------|---------------|-------------|----------|
| `.osrel` | `2E 6F 73 72 65 6C 00 00` | Key=value text (os-release format) | **P0** — display name |
| `.cmdline` | `2E 63 6D 64 6C 69 6E 65` | Plain text | **P0** — command line |
| `.uname` | `2E 75 6E 61 6D 65 00 00 00` | `uname -r` string | **P0** — version sort |
| `.profile` | `2E 70 72 6F 66 69 6C 65` | Key=value text (ID=, TITLE=) | P1 — multi-profile |
| `.sbat` | `2E 73 62 61 74 00 00 00` | CSV | P2 — revocation check |
| `.hwids` | `2E 68 77 69 64 73 00 00` | CHID binary format | P2 — hardware match |
| `.pcrsig` | `2E 70 63 72 73 69 67 00` | JSON | P2 — integrity verify |

**Note:** `.uname` name field is 6 characters + 2 null bytes in the 8-byte field: `[0x2E, 0x75, 0x6E, 0x61, 0x6D, 0x65, 0x00, 0x00]`. Section name comparison must use all 8 bytes.

---

## 5. Data Structures

### 5.1 UkiMetadata

```rust
/// Metadata extracted from UKI PE sections
pub(crate) struct UkiMetadata {
    /// Display name from .osrel PRETTY_NAME (e.g., "Fedora Linux 43")
    pub os_name: Option<String>,
    /// Version from .osrel VERSION_ID (e.g., "43")
    pub os_version: Option<String>,
    /// Kernel version from .uname (e.g., "6.12.0-200.fc43.x86_64")
    pub kernel_version: Option<String>,
    /// Command line from .cmdline
    pub cmdline: Option<String>,
    /// Machine type from COFF header
    pub machine: u16,
}
```

### 5.2 OsReleaseInfo

```rust
/// Parsed os-release key=value fields
pub(crate) struct OsReleaseInfo {
    pub pretty_name: Option<String>,
    pub name: Option<String>,
    pub id: Option<String>,
    pub version_id: Option<String>,
    pub image_version: Option<String>,
    pub version: Option<String>,
    pub build_id: Option<String>,
    pub image_id: Option<String>,
}

impl OsReleaseInfo {
    /// Best display name: PRETTY_NAME > NAME > ID > "Linux"
    pub fn display_name(&self) -> &str { ... }

    /// Best version: VERSION_ID > IMAGE_VERSION > VERSION > BUILD_ID
    pub fn display_version(&self) -> Option<&str> { ... }
}
```

### 5.3 BootEntry Extensions

```rust
pub(crate) struct BootEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub icon: Icon,
    pub bls_filename: Option<String>,
    // --- New fields ---
    /// OS name from UKI .osrel or root partition os-release
    pub os_name: Option<String>,
    /// Kernel version from UKI .uname or BLS version field
    pub kernel_version: Option<String>,
}
```

---

## 6. Parsing Algorithm

### 6.1 Main Entry Point

```rust
/// Parse UKI metadata from a PE binary buffer.
/// Returns None if the buffer is not a valid PE binary or lacks UKI sections.
pub(crate) fn parse_uki_metadata(data: &[u8]) -> Option<UkiMetadata>
```

### 6.2 Algorithm Steps

```
1. VERIFY data.len() >= 64
     If not: return None

2. CHECK MZ magic
     If data[0..2] != [0x5A, 0x4D]: return None

3. READ e_lfanew
     pe_offset = read_u32_le(data, 0x3C) as usize

4. BOUNDS CHECK
     If pe_offset + 4 + 20 > data.len(): return None

5. CHECK PE signature
     If data[pe_offset..pe_offset+4] != [0x50, 0x45, 0x00, 0x00]: return None

6. READ COFF header
     machine = read_u16_le(data, pe_offset + 4)
     num_sections = read_u16_le(data, pe_offset + 6) as usize
     opt_header_size = read_u16_le(data, pe_offset + 24) as usize

7. COMPUTE section table offset
     section_table = pe_offset + 4 + 20 + opt_header_size

8. BOUNDS CHECK section table
     If section_table + (num_sections * 40) > data.len(): return None

9. ITERATE sections
     For i in 0..num_sections:
       offset = section_table + (i * 40)
       name = &data[offset..offset+8]
       raw_size = read_u32_le(data, offset + 16) as usize
       raw_offset = read_u32_le(data, offset + 20) as usize

       If raw_offset + raw_size > data.len(): continue (skip corrupt section)

       section_data = &data[raw_offset..raw_offset + raw_size]

       Match name against targets:
         ".osrel\0\0"   → osrel_data = section_data
         ".cmdline\0"   → cmdline_data = section_data
         ".uname\0\0\0" → uname_data = section_data

10. BUILD UkiMetadata
      Parse osrel_data → OsReleaseInfo → extract os_name, os_version
      Parse cmdline_data → trim whitespace → cmdline string
      Parse uname_data → trim null bytes and whitespace → kernel_version string
      Return Some(UkiMetadata { ... })
```

### 6.3 Helper Functions

```rust
/// Read u16 little-endian from a byte slice at the given offset.
fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Read u32 little-endian from a byte slice at the given offset.
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset], data[offset + 1],
        data[offset + 2], data[offset + 3],
    ])
}

/// Compare an 8-byte section name against a target.
fn section_name_eq(name: &[u8], target: &[u8; 8]) -> bool {
    name.len() >= 8 && name[..8] == *target
}
```

---

## 7. os-release Parsing

### 7.1 Format

Per [freedesktop.org os-release specification](https://www.freedesktop.org/software/systemd/man/os-release.html):

```
PRETTY_NAME="Fedora Linux 43 (Workstation Edition)"
NAME="Fedora Linux"
VERSION_ID=43
ID=fedora
```

Rules:
- Lines beginning with `#` are comments — skip
- Empty lines — skip
- Format: `KEY=VALUE` (no spaces around `=`)
- Values may be quoted with `"` or `'` — strip matching outer quotes
- Unquoted values end at newline
- Escaping within quotes: `\\`, `\"`, `\$`, `\`` , `\n` (not implemented — edge case)

### 7.2 Parser Function

```rust
/// Parse os-release format text into key-value pairs.
pub(crate) fn parse_os_release(data: &[u8]) -> OsReleaseInfo
```

Algorithm:
```
1. Convert data to UTF-8 string (lossy — replace invalid bytes)
2. For each line:
   a. Trim whitespace
   b. Skip if empty or starts with '#'
   c. Split on first '='
   d. Key = left side
   e. Value = right side, strip matching outer quotes (" or ')
   f. Match key against known fields:
      "PRETTY_NAME" → info.pretty_name
      "NAME"        → info.name
      "ID"          → info.id
      "VERSION_ID"  → info.version_id
      "IMAGE_VERSION" → info.image_version
      "VERSION"     → info.version
      "BUILD_ID"    → info.build_id
      "IMAGE_ID"    → info.image_id
3. Return OsReleaseInfo
```

### 7.3 Display Name Priority

systemd-boot uses this priority chain (from `bootspec_pick_name_version_sort_key()`):

**Name:** `PRETTY_NAME` > `IMAGE_ID` > `NAME` > `ID` > filename
**Version:** `VERSION_ID` > `IMAGE_VERSION` > `VERSION` > `BUILD_ID`
**Sort key:** `IMAGE_ID` > `ID`

LamBoot follows the same priority for compatibility.

---

## 8. Discovery Pipeline Integration

### 8.1 Modified `discover_linux_uki()`

```rust
fn discover_linux_uki(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry> {
    let mut entries = Vec::new();
    let uki_paths = esp.glob("\\EFI\\Linux\\*.efi");

    for path in uki_paths {
        if !policy.allowed(&path) { continue; }

        // Load the binary (same buffer that boot_uki() will use later)
        let Ok(data) = esp.read_to_vec(&path) else { continue; };

        // Parse PE sections for metadata
        let metadata = uki::parse_uki_metadata(&data);

        // Build display name
        let name = match &metadata {
            Some(m) if m.os_name.is_some() => {
                let os = m.os_name.as_deref().unwrap();
                match &m.kernel_version {
                    Some(kv) => format!("{os} ({kv})"),
                    None => String::from(os),
                }
            }
            _ => {
                // Fallback: filename without .efi extension
                let fname = extract_filename(&path)
                    .unwrap_or_else(|| String::from("Linux"));
                format!("Linux ({fname})")
            }
        };

        // Use embedded cmdline or empty (NOT hardcoded "quiet splash")
        let options = metadata.as_ref()
            .and_then(|m| m.cmdline.clone())
            .unwrap_or_default();

        let kernel_version = metadata.as_ref()
            .and_then(|m| m.kernel_version.clone());

        let os_name = metadata.as_ref()
            .and_then(|m| m.os_name.clone());

        entries.push(BootEntry {
            id: format!("uki-{}", entries.len()),
            name,
            kind: EntryKind::Uki { path, options },
            icon: Icon::Linux,
            bls_filename: None,
            os_name,
            kernel_version,
        });
    }
    entries
}
```

### 8.2 Performance

PE parsing adds <1ms per UKI on top of the existing `read_to_vec()` call. The file is already loaded for boot — discovery can reuse the same buffer or load it again (ESP reads are fast, UKIs are typically 10-50MB, loaded into UEFI pool memory).

**Optimization:** If the discovery phase doesn't want to load large UKIs just for metadata, it can read only the first 4KB (enough for PE headers and section table) and then read individual section data at their offsets. This requires `EspVolume::read_range(path, offset, length)` — a new function.

For initial implementation: load the full file. Optimize later if discovery is slow with many UKIs.

---

## 9. BLS/UKI Deduplication

### 9.1 Current Deduplication

`dedup_entries()` in `discovery.rs:100` matches on the kernel filename extracted from the path (case-insensitive). BLS entries appear first and survive dedup.

### 9.2 Enhanced Deduplication

Add version-based matching:

```
For each pair of entries (BLS entry, UKI entry):
  If BLS entry has version field AND UKI has kernel_version:
    If bls.version == uki.kernel_version:
      Keep BLS entry (it appeared first), remove UKI entry
```

This catches cases where the BLS entry references a kernel at `/vmlinuz-6.12.0` and the UKI has `.uname` = `6.12.0-200.fc43.x86_64` — the version strings won't match exactly but the base version does. Use a relaxed comparison: if one version starts with the other, consider them matching.

### 9.3 Precedence Rules

1. BLS entries take precedence over UKI entries (BLS has user-customizable options)
2. UKI entries take precedence over legacy scanning entries
3. Within UKI entries: newest version first (UAPI.10 descending)

---

## 10. Version Sorting

Reuse `bls::version_compare()` which already implements the full UAPI.10 algorithm:

```rust
// Sort UKI entries by kernel_version (newest first)
uki_entries.sort_by(|a, b| {
    let av = a.kernel_version.as_deref().unwrap_or("");
    let bv = b.kernel_version.as_deref().unwrap_or("");
    bls::version_compare(av, bv).reverse()  // Descending = newest first
});
```

Entries without `.uname` (no version) sort after entries with versions.

---

## 11. TPM PCR 11 Measurement Order

For future PCR pre-calculation, sections are measured into PCR 11 in this canonical order:

1. `.linux`
2. `.osrel`
3. `.cmdline`
4. `.initrd`
5. `.ucode`
6. `.splash`
7. `.dtb`
8. `.dtbauto` (selected match only)
9. `.hwids`
10. `.uname`
11. `.sbat`

**Critical:** systemd-stub measures `VirtualSize` bytes (not `SizeOfRawData`). This means extra zero bytes padding the section to page alignment are included in the measurement. Any PCR pre-calculation must use `VirtualSize` to match.

`.pcrsig` is excluded (it contains signatures of the measurement output).

**Not implemented in initial version.** Documented here for future reference.

---

## 12. Multi-Profile UKI Support (Future)

### 12.1 Profile Structure

`.profile` sections act as separators. Sections before the first `.profile` are the base profile. Each profile overrides identically-named base sections.

```
[base sections: .osrel, .cmdline, .linux, .initrd]
[.profile: ID=default, TITLE=Default Boot]
[profile-specific .cmdline, .osrel if different]
[.profile: ID=debug, TITLE=Debug Mode]
[profile-specific .cmdline with debug options]
```

### 12.2 Discovery Impact

When multi-profile UKI detected:
- Generate one `BootEntry` per profile
- Entry ID: `uki-{index}@{profile_id}` (e.g., `uki-0@default`, `uki-0@debug`)
- Entry name includes profile title: `"Fedora Linux 43 (6.12.0) — Debug Mode"`
- Each entry's `options` uses the profile-specific `.cmdline`

### 12.3 Selection

Profile selected via first kernel command line argument: `@0`, `@1`, `@2`, etc. If no profile specified, `@0` (first profile) is default.

**Not implemented in initial version.** The parser must not break on UKIs containing `.profile` sections — it should simply use the base sections and ignore profile-specific overrides.

---

## 13. Architecture Validation

Read `Machine` field from COFF header:

| Value | Architecture |
|-------|-------------|
| `0x8664` | x86_64 (IMAGE_FILE_MACHINE_AMD64) |
| `0xAA64` | aarch64 (IMAGE_FILE_MACHINE_ARM64) |

Cross-check:
```rust
#[cfg(target_arch = "x86_64")]
const NATIVE_MACHINE: u16 = 0x8664;
#[cfg(target_arch = "aarch64")]
const NATIVE_MACHINE: u16 = 0xAA64;
```

If `metadata.machine != NATIVE_MACHINE`: log a warning but still show the entry. The user may be inspecting cross-architecture UKIs.

---

## 14. Error Handling

All parse failures return `None` from `parse_uki_metadata()`. No panics. Specific failure modes:

| Condition | Log Level | Behavior |
|-----------|-----------|----------|
| File too small (<64 bytes) | `warn` | Return None, use filename display |
| Bad MZ magic | `debug` | Return None (may not be a PE binary) |
| Bad PE signature | `warn` | Return None, use filename display |
| Section table truncated | `warn` | Return None, use filename display |
| Individual section corrupt | `warn` | Skip that section, extract others |
| Invalid UTF-8 in .osrel | `warn` | Lossy conversion, extract what's possible |
| Missing .osrel section | `info` | Use filename as display name |
| Missing .cmdline section | `info` | Use empty string (no hardcoded default) |
| Missing .uname section | `info` | Skip version-based dedup and sorting |

---

## 15. Test Criteria

### 15.1 Test Vectors

| Test | Input | Expected Output |
|------|-------|-----------------|
| Valid Fedora UKI | PE with .osrel(PRETTY_NAME="Fedora Linux 43"), .cmdline("root=UUID=..."), .uname("6.12.0-200.fc43.x86_64") | name="Fedora Linux 43 (6.12.0-200.fc43.x86_64)", cmdline="root=UUID=...", kernel_version="6.12.0-200.fc43.x86_64" |
| Valid Ubuntu UKI | PE with .osrel(PRETTY_NAME="Ubuntu 24.04.1 LTS") | name="Ubuntu 24.04.1 LTS" |
| No .osrel | PE with .cmdline and .uname only | name="Linux ({filename})", cmdline from .cmdline |
| No .cmdline | PE with .osrel only | cmdline="" (empty, NOT "quiet splash") |
| Truncated at 32 bytes | 32-byte file | Return None, use filename |
| Non-PE .efi file | UEFI app without MZ/PE | Return None (debug log only) |
| Wrong architecture | PE with Machine=0xAA64 on x86_64 | Parse succeeds, log architecture warning |
| Multiple UKIs sorted | Two UKIs: 6.12.0 and 6.11.5 | 6.12.0 sorts first (descending) |
| BLS+UKI dedup | BLS version=6.12.0, UKI .uname=6.12.0-200.fc43 | BLS entry kept, UKI removed |

### 15.2 QEMU Integration Test

1. Build LamBoot with UKI parser
2. Place a Fedora/Ubuntu UKI on the ESP at `\EFI\Linux\`
3. Boot in QEMU — verify menu shows `PRETTY_NAME` instead of filename
4. Verify boot uses embedded `.cmdline` instead of `"quiet splash"`
5. Verify boot report shows correct entry_name with OS name
