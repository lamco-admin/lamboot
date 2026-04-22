# SPEC: Preflight Validation Engine

**Status:** Draft
**Date:** 2026-04-03
**Scope:** Validate boot entries before presenting them in the menu — file existence, PE validity, Secure Boot compatibility, initrd sanity, root resolvability
**Core Principle:** NEVER block boot. All results are advisory. The user can always attempt to boot any entry.
**References:** [ARCHITECTURE.md](../ARCHITECTURE.md) line 186: "Never block boot for optional features"

---

## 1. Overview

No existing bootloader validates entries before presenting them to the user. They all load-and-hope. LamBoot performs 8 preflight checks during discovery (Phase 7) and displays results as visual indicators in the boot menu.

**Module location:** New file `lamboot-core/src/preflight.rs`
**Integration point:** Called during `discover_all_entries()` after entry construction, before dedup

---

## 2. Data Structures

### 2.1 PreflightResult

```rust
/// Aggregate preflight validation result for a boot entry.
#[derive(Debug, Clone)]
pub(crate) struct PreflightResult {
    /// Overall status (derived from worst individual check)
    pub status: PreflightStatus,
    /// Individual check results
    pub checks: Vec<CheckResult>,
}

impl PreflightResult {
    /// Derive overall status from individual checks.
    pub fn from_checks(checks: Vec<CheckResult>) -> Self {
        let status = if checks.iter().any(|c| c.severity == Severity::Error) {
            PreflightStatus::Error
        } else if checks.iter().any(|c| c.severity == Severity::Warning) {
            PreflightStatus::Warning
        } else {
            PreflightStatus::Ok
        };
        Self { status, checks }
    }

    /// Get all failed checks (Warning or Error).
    pub fn issues(&self) -> impl Iterator<Item = &CheckResult> {
        self.checks.iter().filter(|c| c.severity != Severity::Ok)
    }
}
```

### 2.2 PreflightStatus

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PreflightStatus {
    Ok,      // All checks passed
    Warning, // Non-critical issues detected
    Error,   // Critical issues — boot likely to fail
}
```

### 2.3 CheckResult

```rust
#[derive(Debug, Clone)]
pub(crate) struct CheckResult {
    pub check: Check,
    pub severity: Severity,
    pub message: String,
}
```

### 2.4 Check (8 variants)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    FileExists,       // Kernel/initrd/EFI files exist on volume
    PeHeaderValid,    // PE/COFF header is well-formed
    ArchitectureMatch,// PE Machine field matches build target
    InitrdSanity,     // Initrd files exist and are non-empty
    RootResolvable,   // root= parameter in options resolves to a known partition
    SecureBootCompat, // Image passes Secure Boot verification
    FsDriverLoaded,   // Required filesystem driver is available
    MemoryAvailable,  // Sufficient memory for kernel + initrd
}
```

### 2.5 Severity

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Ok,
    Warning,
    Error,
}
```

### 2.6 BootEntry Extension

```rust
pub(crate) struct BootEntry {
    // ... existing fields ...
    /// Preflight validation results (None if validation not yet run)
    pub preflight: Option<PreflightResult>,
}
```

---

## 3. Check Definitions

### 3.1 FileExists

**Applies to:** All EntryKind variants
**Detects:** Deleted kernels, broken symlinks, missing EFI binaries
**Severity on failure:** `Error`

| EntryKind | Paths Checked |
|-----------|--------------|
| Chainload | `path` |
| Uki | `path` |
| LinuxLegacy | `kernel_path` |

```rust
fn check_file_exists(esp: &mut EspVolume, path: &str) -> CheckResult {
    if esp.exists(path) {
        CheckResult { check: Check::FileExists, severity: Severity::Ok, message: String::new() }
    } else {
        CheckResult {
            check: Check::FileExists,
            severity: Severity::Error,
            message: format!("File not found: {path}"),
        }
    }
}
```

### 3.2 PeHeaderValid

**Applies to:** Chainload, Uki
**Detects:** Corrupt downloads, truncated files, non-PE binaries
**Severity on failure:** `Warning`
**Prerequisite:** FileExists passed (short-circuit if file not found)

```rust
fn check_pe_header(esp: &mut EspVolume, path: &str) -> CheckResult {
    // Read first 512 bytes (enough for DOS+PE+COFF headers)
    let Ok(data) = esp.read_to_vec(path) else {
        return CheckResult {
            check: Check::PeHeaderValid,
            severity: Severity::Warning,
            message: format!("Cannot read file: {path}"),
        };
    };

    if data.len() < 64 {
        return warning("File too small for PE binary");
    }
    if data[0..2] != [0x5A, 0x4D] {
        return warning("Invalid PE header (no MZ signature)");
    }
    let pe_offset = u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
    if pe_offset + 4 > data.len() {
        return warning("PE header offset out of bounds");
    }
    if data[pe_offset..pe_offset+4] != [0x50, 0x45, 0x00, 0x00] {
        return warning("Invalid PE signature");
    }
    ok()
}
```

### 3.3 ArchitectureMatch

**Applies to:** Chainload, Uki
**Detects:** x86_64 kernel on aarch64 system (or vice versa)
**Severity on failure:** `Warning`
**Prerequisite:** PeHeaderValid passed

Read `Machine` field from COFF header (2 bytes at `pe_offset + 4`):

```rust
const MACHINE_AMD64: u16 = 0x8664;
const MACHINE_ARM64: u16 = 0xAA64;

#[cfg(target_arch = "x86_64")]
const NATIVE_MACHINE: u16 = MACHINE_AMD64;
#[cfg(target_arch = "aarch64")]
const NATIVE_MACHINE: u16 = MACHINE_ARM64;

fn check_architecture(data: &[u8], pe_offset: usize) -> CheckResult {
    let machine = u16::from_le_bytes([data[pe_offset + 4], data[pe_offset + 5]]);
    if machine == NATIVE_MACHINE {
        ok()
    } else {
        let expected = if NATIVE_MACHINE == MACHINE_AMD64 { "x86_64" } else { "aarch64" };
        let found = match machine {
            MACHINE_AMD64 => "x86_64",
            MACHINE_ARM64 => "aarch64",
            _ => "unknown",
        };
        CheckResult {
            check: Check::ArchitectureMatch,
            severity: Severity::Warning,
            message: format!("Architecture mismatch: expected {expected}, found {found}"),
        }
    }
}
```

### 3.4 InitrdSanity

**Applies to:** LinuxLegacy only
**Detects:** Missing initramfs, zero-byte initramfs
**Severity on failure:** `Warning` (missing) or `Warning` (empty)

```rust
fn check_initrd_sanity(
    esp: &mut EspVolume,
    initrd_paths: &[String],
) -> Vec<CheckResult> {
    let mut results = Vec::new();
    for path in initrd_paths {
        if !esp.exists(path) {
            results.push(CheckResult {
                check: Check::InitrdSanity,
                severity: Severity::Warning,
                message: format!("Initrd not found: {path}"),
            });
            continue;
        }
        // Check file size > 0 via open + get_info
        match esp.open_file(path) {
            Ok(mut file) => {
                let mut buf = [0u8; 512];
                if let Ok(info) = file.get_info::<FileInfo>(&mut buf) {
                    if info.file_size() == 0 {
                        results.push(CheckResult {
                            check: Check::InitrdSanity,
                            severity: Severity::Warning,
                            message: format!("Initrd is empty (0 bytes): {path}"),
                        });
                    }
                }
            }
            Err(_) => {
                results.push(CheckResult {
                    check: Check::InitrdSanity,
                    severity: Severity::Warning,
                    message: format!("Cannot read initrd: {path}"),
                });
            }
        }
    }
    results
}
```

### 3.5 RootResolvable

**Applies to:** LinuxLegacy (entries with `options` containing `root=`)
**Detects:** `root=UUID=...` or `root=PARTUUID=...` that doesn't match any partition
**Severity on failure:** `Warning`

```rust
fn check_root_resolvable(options: &str) -> CheckResult {
    // Extract root= value from options string
    let root_spec = match extract_root_param(options) {
        Some(spec) => spec,
        None => return ok(), // No root= parameter — not our problem
    };

    // Validate format (not actual partition presence — that requires BlockIO scanning)
    if root_spec.starts_with("UUID=") {
        let uuid = &root_spec[5..];
        if !is_valid_uuid_format(uuid) {
            return CheckResult {
                check: Check::RootResolvable,
                severity: Severity::Warning,
                message: format!("Malformed UUID in root=: {uuid}"),
            };
        }
    }
    // For PARTUUID=, LABEL=, /dev/* — accept without further validation
    // (partition scanning is expensive and may not be reliable at boot time)
    ok()
}

/// Extract the value of root= from a kernel command line string.
fn extract_root_param(options: &str) -> Option<&str> {
    for token in options.split_whitespace() {
        if let Some(value) = token.strip_prefix("root=") {
            return Some(value);
        }
    }
    None
}

/// Validate UUID format: 8-4-4-4-12 hex chars.
fn is_valid_uuid_format(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}
```

### 3.6 SecureBootCompat

**Applies to:** All EntryKind variants (except when Secure Boot is disabled)
**Detects:** Unsigned kernels/binaries when Secure Boot is active
**Severity on failure:** `Warning`
**Prerequisite:** FileExists passed; only runs when SB is active

```rust
fn check_secure_boot_compat(
    esp: &mut EspVolume,
    path: &str,
    sb_state: SecureBootState,
) -> CheckResult {
    if sb_state == SecureBootState::Disabled {
        return ok(); // No verification needed
    }

    let Ok(data) = esp.read_to_vec(path) else {
        return warning("Cannot read file for Secure Boot verification");
    };

    match crate::secure::verify_image(&data) {
        Ok(()) => ok(),
        Err(_) => CheckResult {
            check: Check::SecureBootCompat,
            severity: Severity::Warning,
            message: String::from("Not signed for Secure Boot"),
        },
    }
}
```

**Note:** This is the most expensive check (reads full file + verification). Skip if policy disables it.

### 3.7 FsDriverLoaded

**Applies to:** LinuxLegacy (entries referencing non-ESP paths)
**Detects:** BLS entries pointing to ext4/btrfs paths without loaded drivers
**Severity on failure:** `Warning`

```rust
fn check_fs_driver_loaded(
    kernel_path: &str,
    driver_count: usize,
) -> CheckResult {
    // Heuristic: if the path starts with common /boot patterns
    // and no filesystem drivers were loaded, it's likely on a
    // non-ESP partition that we can't read
    if driver_count == 0 && !is_likely_esp_path(kernel_path) {
        return CheckResult {
            check: Check::FsDriverLoaded,
            severity: Severity::Warning,
            message: String::from("Path may require a filesystem driver (none loaded)"),
        };
    }
    ok()
}

fn is_likely_esp_path(path: &str) -> bool {
    let p = path.to_uppercase();
    p.starts_with("\\EFI\\") || p.starts_with("\\LOADER\\")
}
```

### 3.8 MemoryAvailable

**Applies to:** Uki, LinuxLegacy
**Detects:** Kernel + initrd too large for available memory
**Severity on failure:** `Warning`

```rust
fn check_memory_available(
    esp: &mut EspVolume,
    kernel_path: &str,
    initrd_paths: &[String],
) -> CheckResult {
    // Estimate total size needed
    let mut total_bytes: u64 = 0;

    if let Ok(mut file) = esp.open_file(kernel_path) {
        let mut buf = [0u8; 512];
        if let Ok(info) = file.get_info::<FileInfo>(&mut buf) {
            total_bytes += info.file_size();
        }
    }

    for path in initrd_paths {
        if let Ok(mut file) = esp.open_file(path) {
            let mut buf = [0u8; 512];
            if let Ok(info) = file.get_info::<FileInfo>(&mut buf) {
                total_bytes += info.file_size();
            }
        }
    }

    // Check against available UEFI conventional memory
    // Query memory map is expensive — use a conservative threshold
    let threshold_bytes = 2 * 1024 * 1024 * 1024; // 2 GB reasonable maximum
    if total_bytes > threshold_bytes {
        return CheckResult {
            check: Check::MemoryAvailable,
            severity: Severity::Warning,
            message: format!(
                "Kernel + initrd may exceed available memory ({} MB)",
                total_bytes / (1024 * 1024)
            ),
        };
    }
    ok()
}
```

---

## 4. Check Execution

### 4.1 Main Entry Point

```rust
/// Run all applicable preflight checks for a boot entry.
pub(crate) fn run_preflight(
    entry: &BootEntry,
    esp: &mut EspVolume,
    sb_state: SecureBootState,
    driver_count: usize,
) -> PreflightResult {
    let mut checks = Vec::new();

    match &entry.kind {
        EntryKind::Chainload { path } => {
            let file_ok = check_file_exists(esp, path);
            let file_passed = file_ok.severity == Severity::Ok;
            checks.push(file_ok);

            if file_passed {
                checks.push(check_pe_header(esp, path));
                // Architecture check uses data from PE header check
                checks.push(check_secure_boot_compat(esp, path, sb_state));
            }
        }

        EntryKind::Uki { path, .. } => {
            let file_ok = check_file_exists(esp, path);
            let file_passed = file_ok.severity == Severity::Ok;
            checks.push(file_ok);

            if file_passed {
                checks.push(check_pe_header(esp, path));
                checks.push(check_secure_boot_compat(esp, path, sb_state));
            }
        }

        EntryKind::LinuxLegacy { kernel_path, initrd_paths, options } => {
            let file_ok = check_file_exists(esp, kernel_path);
            let file_passed = file_ok.severity == Severity::Ok;
            checks.push(file_ok);

            if file_passed {
                checks.push(check_pe_header(esp, kernel_path));
                checks.push(check_secure_boot_compat(esp, kernel_path, sb_state));
                checks.push(check_memory_available(esp, kernel_path, initrd_paths));
            }

            checks.extend(check_initrd_sanity(esp, initrd_paths));
            checks.push(check_root_resolvable(options));
            checks.push(check_fs_driver_loaded(kernel_path, driver_count));
        }
    }

    PreflightResult::from_checks(checks)
}
```

### 4.2 Short-Circuit Rules

- If `FileExists` fails for a path → skip `PeHeaderValid`, `ArchitectureMatch`, `SecureBootCompat`, `MemoryAvailable` for that path (file can't be read)
- If `PeHeaderValid` fails → skip `ArchitectureMatch` (can't read COFF header)
- `SecureBootCompat` only runs when `sb_state != Disabled`

### 4.3 Execution Order (Cheapest First)

1. `FileExists` (~1ms — single `exists()` call)
2. `FsDriverLoaded` (~0ms — string comparison only)
3. `RootResolvable` (~0ms — string parsing only)
4. `InitrdSanity` (~1ms per initrd — open + get_info)
5. `PeHeaderValid` (~1ms — read first 512 bytes)
6. `ArchitectureMatch` (~0ms — read from data already loaded by PeHeaderValid)
7. `MemoryAvailable` (~2ms — open + get_info for each file)
8. `SecureBootCompat` (~10-50ms — reads full file + verification)

---

## 5. Integration with Discovery Pipeline

### 5.1 Call Site

In `main.rs`, after `discover_all_entries()` returns but before the menu:

```rust
// Phase 7: Discover boot entries
let mut entries = discovery::discover_all_entries(&mut esp, &mut extra_volumes, &policy);

// Phase 7.5: Preflight validation
if policy.preflight_enabled {
    let sb_state = secure::detect_secure_boot();
    for entry in &mut entries {
        entry.preflight = Some(preflight::run_preflight(entry, &mut esp, sb_state, driver_count));
    }
}
```

### 5.2 Sort Integration

Within each existing UAPI.10 sort tier, entries with errors sort after entries with warnings, which sort after healthy entries:

```rust
// After existing bls_sort_compare(), apply health-based sub-sort:
fn health_sort_tiebreak(a: &BootEntry, b: &BootEntry) -> Ordering {
    let a_status = a.preflight.as_ref().map(|p| p.status).unwrap_or(PreflightStatus::Ok);
    let b_status = b.preflight.as_ref().map(|p| p.status).unwrap_or(PreflightStatus::Ok);
    a_status.cmp(&b_status) // Ok < Warning < Error
}
```

---

## 6. GUI Rendering

### 6.1 Health Indicators

In `gui.rs`, within `draw_entries()`, after drawing each entry tile:

| Status | Indicator | Color | Position |
|--------|-----------|-------|----------|
| Ok | (none or subtle checkmark) | — | — |
| Warning | `"!"` character | `BltPixel::new(0xfa, 0xb3, 0x87)` (WARNING_COLOR, already defined) | Right side of tile, 14px from edge |
| Error | `"X"` character | `BltPixel::new(0xf3, 0x8b, 0xa8)` (red-pink) | Right side of tile, 14px from edge |

### 6.2 Detail Display

When a highlighted entry has Warning or Error status, display the first issue message below the entry list:

```rust
if let Some(ref pf) = entries[self.selected_index].preflight {
    if pf.status != PreflightStatus::Ok {
        if let Some(issue) = pf.issues().next() {
            self.fb.draw_text(
                &issue.message,
                50,
                warning_y as i32,
                WARNING_COLOR,
                Font::Small,
            );
        }
    }
}
```

---

## 7. Text Console Rendering

In `console.rs`, append status suffix to entry name:

```rust
let status_suffix = match entry.preflight.as_ref().map(|p| p.status) {
    Some(PreflightStatus::Warning) => " [!]",
    Some(PreflightStatus::Error) => " [X]",
    _ => "",
};
println_console(&format!(" {} {}) {}{}", marker, i, entry.name, status_suffix));
```

---

## 8. Boot Report Enrichment

In `report.rs`, add preflight results to `boot.json`:

```json
{
  "timestamp": "2026-04-03T14:30:00",
  "entry_id": "bls-fedora-6.12.0",
  "entry_name": "Fedora Linux 43 (6.12.0)",
  "entry_type": "linux_legacy",
  "path": "/vmlinuz-6.12.0",
  "preflight": {
    "status": "warning",
    "checks": [
      {"check": "FileExists", "severity": "ok", "message": ""},
      {"check": "SecureBootCompat", "severity": "warning", "message": "Not signed for Secure Boot"}
    ]
  }
}
```

---

## 9. Policy Integration

### 9.1 New Policy Fields

```rust
pub(crate) struct Policy {
    // ... existing fields ...
    pub preflight_enabled: bool,     // [preflight] enabled = true
    pub hide_broken: bool,           // [preflight] hide_broken = false
    pub warn_unsigned: bool,         // [preflight] warn_unsigned = true
}
```

### 9.2 Policy Keys

```toml
[preflight]
enabled = true         # Run preflight checks (default: true)
hide_broken = false    # Hide entries with Error status (default: false)
warn_unsigned = true   # Check Secure Boot signatures (default: true)
```

### 9.3 Policy Application

- `enabled = false`: Skip all preflight, `entry.preflight = None`
- `hide_broken = true`: Filter entries with `PreflightStatus::Error` from menu (but still log them)
- `warn_unsigned = false`: Skip `SecureBootCompat` check

---

## 10. Performance Budget

| Metric | Target |
|--------|--------|
| Single entry, all checks (no SB) | <5ms |
| Single entry, all checks (with SB) | <50ms |
| 10 entries total | <500ms |
| Preflight disabled | 0ms |

SecureBootCompat is the bottleneck (reads full binary). If performance is a concern, make it opt-in rather than opt-out.

---

## 11. Error Handling

Preflight itself must never cause errors that prevent boot:

| Condition | Handling |
|-----------|----------|
| Check function panics | Should be impossible — all operations use `?` and `match` |
| File read error during check | `Severity::Warning` with "Cannot verify: {reason}" |
| Unknown EntryKind variant | Skip (no checks for unknown types) |
| Policy parsing error for preflight keys | Use defaults (enabled=true, hide_broken=false, warn_unsigned=true) |

The `Option<PreflightResult>` on BootEntry means entries without preflight results render normally with no indicators.

---

## 12. Test Criteria

### 12.1 Scenario Matrix

| Scenario | Expected Status | Expected Message |
|----------|----------------|-----------------|
| Valid entry, all files present | Ok | (none) |
| Kernel file deleted | Error | "File not found: /vmlinuz-6.12.0" |
| Corrupt PE header (random bytes) | Warning | "Invalid PE header (no MZ signature)" |
| aarch64 kernel on x86_64 | Warning | "Architecture mismatch: expected x86_64, found aarch64" |
| Missing initrd | Warning | "Initrd not found: /initramfs-6.12.0.img" |
| Zero-byte initrd | Warning | "Initrd is empty (0 bytes): /initramfs-6.12.0.img" |
| root=UUID=invalid-format | Warning | "Malformed UUID in root=: invalid-format" |
| Unsigned kernel + SB active | Warning | "Not signed for Secure Boot" |
| Non-ESP path + no drivers | Warning | "Path may require a filesystem driver (none loaded)" |
| Kernel + initrd = 3GB | Warning | "Kernel + initrd may exceed available memory (3072 MB)" |

### 12.2 GUI Rendering Verification

1. Entry with Warning: yellow `!` indicator visible on right side of tile
2. Entry with Error: red `X` indicator visible
3. Highlighted Warning entry: issue message displayed below entry list
4. Entry with Ok: no indicator (clean)

### 12.3 Integration Test (QEMU)

1. Create entries with: valid kernel, missing kernel, corrupt PE, unsigned kernel
2. Boot LamBoot in QEMU
3. Verify correct indicators on each entry
4. Verify boot report includes preflight results
5. Verify booting a Warning entry still works
6. Verify booting an Error entry attempts boot (doesn't block)
