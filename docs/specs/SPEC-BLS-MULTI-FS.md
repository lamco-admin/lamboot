# SPEC-BLS-MULTI-FS: BLS Entry Discovery across FAT / ext4 / XBOOTLDR

**SDS number:** SDS-5 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/discovery.rs` (major refactor to volume-polymorphic), `lamboot-core/src/bls.rs` (new boot-counter rename split for RO backends), `tools/lamboot-install` (documented BLS-write location policy), `docs/CONFIGURATION-GUIDE.md` (amended)
**Depends on:** SDS-1 (`FsBackend` + `Volume` dispatch), SDS-2 (ext4 readability) MUST be landed first.
**Blocks:** SDS-6 (UEFI FS Driver Deprecation references multi-FS discovery as the native-coverage replacement).
**External dependencies:** none (pure LamBoot code + existing BLS spec).

---

## 1. Overview

This specification defines how LamBoot discovers Boot Loader Specification (BLS) Type 1 entries across multiple filesystems — the FAT ESP, the optional XBOOTLDR partition, and non-spec-compliant ext4 `/boot` partitions where some distros (notably Fedora and Arch) place their BLS entries. It also specifies how boot-counter rename operations interact with the FAT-only `EspWriter` discipline established in SDS-1, and how discovery events feed the trust log.

This is the SDS that closes the v0.8.3 VM 122 Fedora fleet-test failure — LamBoot could not see Fedora's 4 BLS entries on ext4 `/boot/loader/entries/` because the v0.8.3 scanner only checked the FAT ESP. Post-SDS-5, Fedora's kernels are visible and bootable.

### 1.1 Context and prior art

**BLS Type 1 spec**: [systemd.io/BOOT_LOADER_SPECIFICATION](https://systemd.io/BOOT_LOADER_SPECIFICATION/). Defines a `/loader/entries/*.conf` directory of per-kernel config files with fields `title`, `version`, `machine-id`, `sort-key`, `linux`, `initrd`, `options`, `architecture`.

**Spec-compliant location:** `/loader/entries/` on the ESP or the XBOOTLDR partition.

**Real-world non-spec locations:**

| Distro | BLS entries located at | Partition it lives on |
|---|---|---|
| Fedora | `/boot/loader/entries/*.conf` | **ext4 `/boot`** (separate partition) |
| openSUSE Tumbleweed | `/boot/loader/entries/*.conf` | **Btrfs `/boot`** |
| Pop!_OS | `/boot/efi/loader/entries/*.conf` | FAT ESP ✓ spec-compliant |
| Ubuntu (if configured for BLS) | `/boot/efi/loader/entries/*.conf` | FAT ESP ✓ |
| Debian (if configured for BLS) | `/boot/efi/loader/entries/*.conf` | FAT ESP ✓ |
| EndeavourOS / Arch (systemd-boot) | `/efi/loader/entries/*.conf` | FAT ESP (at `/efi`, not `/boot/efi`) ✓ |

Fedora and openSUSE are the main non-spec cases. Both are significant user populations; both are targets for v1.0.

### 1.2 The v0.8.3 bug this SDS resolves

v0.8.3's `discovery.rs::discover_all_entries` takes a single `&mut EspVolume` and scans only `\loader\entries\*.conf` on the ESP. Fedora's entries on ext4 `/boot/loader/entries/` are invisible. LamBoot falls through to the "EFI Fallback" entry (`\EFI\BOOT\BOOTX64.EFI` = Fedora shim), chainloads shim, which calls GRUB, which reads its own grub.cfg + BLS entries from ext4 — and hangs (observed on VM 122).

Post-SDS-5, LamBoot's own scanner walks ext4 `/boot/loader/entries/` natively (via SDS-2's `Ext4Backend`), finds the entries, loads kernels via SDS-3's native PE loader, never invokes shim+GRUB secondary chain.

### 1.3 What this SDS delivers

- **A canonical BLS-discovery order** specifying which volumes to scan, in what order, and what to do when multiple locations hold entries with the same `machine-id`.
- **Volume-polymorphic `discover_all_entries`** replacing the ESP-only signature with a `&mut [Volume]` input.
- **Per-volume scan logic** that uses the `FsBackend` trait (read_dir + read_to_string + exists) uniformly across backends.
- **Boot-counter write policy**: writable on FAT volumes via `EspWriter`; warned-only on RO ext4 with a graceful-degradation event.
- **Deduplication rules** for entries appearing in multiple locations (filename + `machine-id` heuristic).
- **Install-script policy**: `lamboot-install` ALWAYS writes newly-generated BLS entries to the ESP (spec-compliant), never to ext4, even on distros whose existing entries live on ext4. Install-script-generated entries coexist with distro entries.
- **XBOOTLDR detection + mount** using GPT partition-type GUID (`BC13C2FF-...`). XBOOTLDR is an additional spec-compliant location for BLS entries on systems where the ESP is too small.
- **Trust-log events** per volume scan + counter-rename action.
- **Test plan** with mock filesystems + real-distro disk images.

### 1.4 What this SDS does NOT deliver

- **BLS Type 2 entries** (compiled kernel EFI stub images in `\EFI\Linux\*.efi`). Those are UKIs and are handled by `discovery::discover_linux_uki`, a separate path. Not in SDS-5's scope.
- **GRUB environment-variable expansion** (`$default`, `$tuned_initrd`, `$grub_class`). These appear in Fedora's BLS files but are GRUB-specific. LamBoot's BLS parser tolerates them (treats as literal strings in `options`, ignores unknown fields) but does not expand them. Documented limitation.
- **Non-Linux BLS entries** (FreeBSD, illumos experimental ports). LamBoot discovers them and lists them but cannot boot them. `architecture: x86_64-freebsd` → preflight marks as `unknown-architecture` and blocks auto-boot.
- **A BLS writer** beyond what `lamboot-install` already generates. Runtime LamBoot does not create BLS entries; only the install script does.
- **Write-support on non-FAT boot-counter location.** The BLS spec's filename-rename counter (`+N[-M]`) requires writes; on ext4 via ext4-view we CANNOT write, so counters degrade to warn+skip (§6).
- **Handling Btrfs /boot layouts.** openSUSE Tumbleweed's `/boot` on Btrfs is not readable in v0.9.x (no native Btrfs backend). openSUSE users must use UKI or accept the v0.9.x limitation. v1.1+ Btrfs native backend covers this.
- **A new BLS config format.** We conform to the existing UAPI.10 spec.

### 1.5 Constraints

All SDS-1 through SDS-4 constraints apply. Additionally:

- Discovery MUST complete within 1 second wall-clock on a typical boot (reading a dozen .conf files across 2-3 volumes). Budget per §7.5.
- Discovery MUST NOT panic on malformed .conf files; they are individually skipped with a trust-log `bls_entry_invalid` event.
- Discovery MUST NOT skip an entry silently due to backend error; every skipped entry produces a log event.
- Entry uniqueness is determined by `(volume_identity, filename)` — not by `machine-id` alone. Two entries with the same `machine-id` on different volumes are both presented to the user (deduplication § 5).

---

## 2. BLS locations — the authoritative order

LamBoot scans volumes in this precedence, entry-wise, for every boot:

1. **ESP, at `/loader/entries/*.conf`** — spec-compliant location 1. Always scanned.
2. **XBOOTLDR partition, at `/loader/entries/*.conf`** — spec-compliant location 2 (when present). Scanned if a volume has GPT partition-type GUID `BC13C2FF-59E6-4262-A352-B275FD6F7172`.
3. **Any non-ESP FAT volume with a `/loader/entries/` directory** — rare but spec-permissive. Scanned opportunistically.
4. **Any ext4 volume with a `/loader/entries/` directory** — non-spec-compliant but real-world (Fedora, Arch-with-ext4-/boot). Scanned via SDS-2's `Ext4Backend`.
5. **Any ext4 volume mounted at `/boot` with `/boot/loader/entries/`** — same as 4 but expressed as the Fedora-idiomatic path. Implementation detail: 4 and 5 are the same scan — we look for `/loader/entries/` at the root of the ext4 volume, which for a volume mounted at `/boot` by the OS is equivalent to `/boot/loader/entries/` from the OS view.
6. **Btrfs / XFS volumes** — v0.9.x skipped (no native backend). Entries on these filesystems are INVISIBLE to v0.9.x LamBoot. v1.1+ adds coverage.

### 2.1 Why this order

- **ESP first:** spec-compliant, always-present, always-writable. If a .conf file is in multiple locations, ESP wins (deduplication).
- **XBOOTLDR second:** spec-compliant, optional. Some systems split ESP (tiny, for firmware fallback) and XBOOTLDR (bigger, for kernels).
- **Non-standard locations last:** a security-conscious posture — prefer spec-compliant locations where possible, fall through to discovered non-spec locations only if nothing else provides boot entries.

### 2.2 Directory enumeration within a location

For each scanned location, list `*.conf` files (`read_dir` filtered by suffix). Each .conf is parsed into a `BlsEntry`; parse failures skip the file with a `bls_entry_invalid` event.

### 2.3 Cross-volume entry presentation

Entries from different volumes appear **as separate entries in the LamBoot menu**. The menu UI (Layer 6) may group them by volume label; implementation is up to the GUI, not this SDS.

---

## 3. Dispatch — the new `discover_all_entries` signature

```rust
/// Discover all bootable entries across all mounted volumes.
///
/// Scans BLS entries (spec-compliant + ext4 non-spec), UKIs, Windows
/// chainload, GRUB chainload, fallback EFI/BOOT, and diagnostic modules.
///
/// Each entry carries a `source_volume_index: usize` back-reference so
/// the eventual `boot::boot_entry` call can look up the correct Volume
/// from the `volumes` slice.
pub(crate) fn discover_all_entries(
    volumes: &mut [Volume],
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Vec<BootEntry>;
```

Changes from v0.8.3:

| Was | Is |
|---|---|
| `discover_all_entries(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry>` | Takes `&mut [Volume]`, scans all |
| Returns entries without volume context | Each `BootEntry` carries `source_volume_index` |
| No trust-log interaction | Emits `volume_scanned`, `bls_entries_found`, `bls_entry_invalid` events |
| BLS-only scan path: ESP-only | BLS-only scan path: §2's full ordered list |

The new `source_volume_index` field on `BootEntry` is used by `boot::boot_linux_native` (SDS-4 §7.2) to select the correct `Volume` as the byte source: `&mut volumes[entry.source_volume_index]`.

---

## 4. BLS scan algorithm

### 4.1 Per-volume scan

```rust
fn scan_volume_for_bls(
    volume: &mut Volume,
    volume_index: usize,
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Vec<BlsEntry> {
    let mut found = Vec::new();

    // The BLS-spec-compliant location is /loader/entries/.
    // Every FS backend's `Volume::read_dir` exposes this uniformly.
    let entries_path = Path::from_str("/loader/entries").unwrap();

    let dir_exists = match volume.exists(&entries_path) {
        Ok(true) => true,
        Ok(false) => false,
        Err(e) => {
            trust_log.record(TrustEvent::new("bls_scan_io_error")
                .with_path("/loader/entries")
                .with_note(&format!("volume_index={volume_index} backend={} err={}",
                                    volume.identity().backend_tag, e.as_log_token())));
            return found;
        }
    };

    if !dir_exists {
        // Not every volume has BLS entries; silent on this.
        return found;
    }

    let dir_entries = match volume.read_dir(&entries_path) {
        Ok(v) => v,
        Err(e) => {
            trust_log.record(TrustEvent::new("bls_scan_io_error")
                .with_path("/loader/entries")
                .with_note(&format!("volume_index={volume_index} err={}", e.as_log_token())));
            return found;
        }
    };

    for de in dir_entries {
        if de.kind != FileKind::Regular { continue; }
        if !de.name.ends_with(".conf") { continue; }

        let file_path_str = alloc::format!("/loader/entries/{}", de.name);
        let file_path = Path::from_str(&file_path_str).unwrap();

        let content = match volume.read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                trust_log.record(TrustEvent::new("bls_entry_read_failed")
                    .with_path(&file_path_str)
                    .with_note(&format!("err={}", e.as_log_token())));
                continue;
            }
        };

        match bls::parse_entry(&de.name, &content) {
            Ok(entry) => {
                let mut e = entry;
                e.source_volume_index = volume_index;
                e.source_backend_tag = volume.identity().backend_tag;
                // Policy filter
                if let Some(ref linux_path) = e.linux {
                    if !policy.allowed(linux_path) {
                        continue;  // silently drop; policy is a user-visible control
                    }
                }
                found.push(e);
            }
            Err(parse_err) => {
                trust_log.record(TrustEvent::new("bls_entry_invalid")
                    .with_path(&file_path_str)
                    .with_note(&format!("parse={parse_err}")));
            }
        }
    }

    trust_log.record(TrustEvent::new("bls_entries_found")
        .with_note(&format!(
            "volume_index={volume_index} backend={} count={}",
            volume.identity().backend_tag, found.len()
        )));

    found
}
```

### 4.2 Iterate all volumes

```rust
fn discover_bls_all(volumes: &mut [Volume], policy: &Policy, trust_log: &mut TrustLog)
    -> Vec<BlsEntry>
{
    let mut all = Vec::new();
    for (i, volume) in volumes.iter_mut().enumerate() {
        let entries = scan_volume_for_bls(volume, i, policy, trust_log);
        all.extend(entries);
    }
    all
}
```

Order of iteration matches the `volumes` slice, which matches the GPT-enumeration order from `partitions.rs::mount_all_volumes`. The mount-time ordering puts ESP before other volumes by convention (implementation detail: §5 of SDS-1 mount logic).

### 4.3 XBOOTLDR detection

```rust
const XBOOTLDR_PARTITION_GUID: uefi::Guid = uefi::guid!("BC13C2FF-59E6-4262-A352-B275FD6F7172");

fn volume_is_xbootldr(volume: &Volume) -> bool {
    volume.identity().partition_guid == Some(XBOOTLDR_PARTITION_GUID)
}
```

No special XBOOTLDR handling at discovery time — the `scan_volume_for_bls` function treats XBOOTLDR like any other volume with `/loader/entries/`. XBOOTLDR presence is logged as a `volume_mounted` event at mount time (§ SDS-1), and consumers can grep for it.

### 4.4 Dealing with Fedora's machine-id directory layout

Fedora's BLS entries have filenames like `<machine-id>-<version>.conf` AND the kernel files themselves live at `/<machine-id>/<version>/linux` on the ESP (EndeavourOS) or `/boot/<machine-id>/<version>/` on ext4 (post-kernel-install).

The BLS Type 1 `linux` field is a **path relative to the partition root** — it does not include `/boot/`. For Fedora's layout:

```
title      Fedora Linux (6.19.12-200.fc43.x86_64)
linux      /vmlinuz-6.19.12-200.fc43.x86_64
initrd     /initramfs-6.19.12-200.fc43.x86_64.img $tuned_initrd
```

The `linux /vmlinuz-6.19.12...` is relative to whatever volume the BLS entry was discovered on. If discovered on ext4 `/boot`, the kernel is at `/vmlinuz-6.19.12...` on that ext4 volume (i.e. at `/boot/vmlinuz-6.19.12...` from the OS's perspective — which is exactly the same ext4 path).

This is the normal BLS convention; LamBoot's `boot::boot_linux_native` just needs to know which Volume to look on, which it does via `source_volume_index`.

---

## 5. Deduplication rules

Two BLS entries could hypothetically appear in multiple scanned locations (e.g. an admin copies ESP entries to ext4 backup). Rules:

1. **Primary key:** `(filename, content SHA-256)`. Exact-duplicate entries (same filename AND same parsed content) are deduplicated — only the FIRST occurrence (in volume-scan order) is kept.
2. **Same filename, different content:** BOTH presented. Non-trivial case; log event `bls_duplicate_filename` with both volume tags.
3. **Same `machine-id`, different filenames, different contents:** BOTH presented. BLS sort-key logic (from `bls.rs`) handles menu ordering.
4. **Dedup happens POST-scan, PRE-policy-filter.** Policy denies are applied to the deduplicated set.

### 5.1 Why not dedup by `machine-id` alone

Machine-ID in BLS just identifies "this entry is for this installed OS." Multiple kernels (current + fallback + rescue + kernel-with-debug-options) legitimately share a machine-id. Dedup by machine-id would collapse them all to one menu entry — wrong.

---

## 6. Boot-counter writes on multi-FS volumes

BLS boot counting (`+N[-M]` filename suffix per spec §4.5) requires LamBoot to RENAME the .conf file after each boot attempt. If the file lives on a FAT volume (ESP or XBOOTLDR), we can rename via `EspWriter`. If it lives on ext4 via ext4-view, we CANNOT — ext4-view is read-only and `FsBackend` has no rename method.

### 6.1 Counter-write policy

| Entry source | Counter rename action | Trust-log event |
|---|---|---|
| ESP (FAT) | Performed via `EspWriter::rename` | `boot_counter_renamed` |
| XBOOTLDR (FAT) | Performed via `EspWriter::rename` after obtaining a writer for the XBOOTLDR volume | `boot_counter_renamed` |
| ext4 /boot | **Skipped**; user warned once per boot | `boot_counter_skipped_ro` |
| Btrfs /boot (v1.1+) | Depends on Btrfs backend support | TBD |

### 6.2 User-visible consequence

For entries on ext4, `systemd-bless-boot.service` has nothing to bless (the counter never decrements). Users with automatic-fallback requirements MUST either:

1. Keep `/boot` on FAT (ESP) — effectively kernel-on-ESP flow. Works fine.
2. Switch to UKI with the UKI on the ESP — same effect.
3. Accept that BLS boot counting is unavailable for their ext4 /boot configuration until `EspWriter`-equivalent ext4 write support lands (not v1.0, possibly never — would compromise the read-only security property).

This is a **documented limitation**, not a bug. The security gain from read-only ext4 outweighs the boot-counter loss for most users.

### 6.3 `EspWriter::rename` API

```rust
impl<'v> EspWriter<'v> {
    /// Rename `from` to `to` within the same volume. Both paths must be
    /// on the FAT volume this writer is bound to.
    pub(crate) fn rename(&mut self, from: &Path, to: &Path) -> Result<(), FsError>;
}
```

Implementation via `uefi::proto::media::file::File::rename` (available on UEFI ≥ 2.3.1). No cross-volume rename; if the bless-boot service marks success on an XBOOTLDR entry, we need an `EspWriter` for XBOOTLDR, not ESP.

### 6.4 Warn-once-per-boot behavior

The `boot_counter_skipped_ro` warning is emitted once per boot per ext4 volume, not per entry:

```rust
// In main.rs, after entry selection, before boot_entry:
if entry.source_is_readonly_fs() && entry.has_boot_counter() {
    if !warned_volumes.contains(&entry.source_volume_index) {
        trust_log.record(TrustEvent::new("boot_counter_skipped_ro")
            .with_path(&entry.filename)
            .with_note(&format!(
                "volume_index={} backend={} entries_affected=<count>",
                entry.source_volume_index,
                volumes[entry.source_volume_index].identity().backend_tag
            )));
        warned_volumes.insert(entry.source_volume_index);
    }
}
```

Keeps the log from spamming the warning for every entry on the same volume.

---

## 7. Install-script policy (`lamboot-install`)

### 7.1 Spec-compliance by default

`lamboot-install`'s BLS-generation phase (Phase 5 per SPEC-LAMBOOT-INSTALL.md) writes generated .conf files to `$ESP/loader/entries/`, which is the FAT ESP — ALWAYS. Even on Fedora where existing distro entries are on ext4 `/boot`, install-script-generated entries go to ESP.

Rationale:
- Spec-compliant location.
- Writable at runtime via `EspWriter` (boot counting works for install-generated entries).
- Boot at runtime via fast FAT backend.
- Coexists with distro entries (they appear in separate menu groups).
- Makes our install visible to non-LamBoot systemd-boot.

### 7.2 What install does NOT do

- Does NOT copy distro BLS entries from ext4 to ESP. That would duplicate state and fight the distro's own kernel-install flow.
- Does NOT rewrite distro entries. Read-only stance.
- Does NOT install BLS entries to `/boot/loader/entries/` even if `/boot` is FAT. Always at `/loader/entries/` on the ESP.

### 7.3 Install-script BLS documentation

Post-SDS-5, add a section to `docs/INSTALL-REFERENCE.md`:

> **Where are LamBoot's BLS entries written?**
>
> Always at `$ESP/loader/entries/` on the FAT ESP, regardless of whether your distro uses ESP-native BLS (Debian, Ubuntu with BLS, Pop!_OS, EndeavourOS/Arch) or ext4-native BLS (Fedora). LamBoot's own entries coexist with your distro's entries; both are presented in the LamBoot menu. Boot counting via systemd-bless-boot works for LamBoot-generated entries on the ESP; it does NOT work for ext4-located distro entries (that's a documented limitation — see `docs/specs/SPEC-BLS-MULTI-FS.md` §6).

---

## 8. Architecture integration

### 8.1 `discovery.rs` refactor

Before:
```rust
pub(crate) fn discover_all_entries(esp: &mut EspVolume, policy: &Policy) -> Vec<BootEntry>
```

After:
```rust
pub(crate) fn discover_all_entries(
    volumes: &mut [Volume],
    policy: &Policy,
    trust_log: &mut TrustLog,
) -> Vec<BootEntry>
```

Internal function changes:
- `discover_bls_all` uses the §4.2 loop.
- `discover_windows` scans only the ESP (Windows boot manager is always on ESP by MS spec).
- `discover_linux_uki` scans ESP + any FAT volume with `\EFI\Linux\` (already the case; minor refactor).
- `discover_other_loaders` scans only ESP.
- `discover_tools` scans only ESP.
- `discover_linux_legacy` — SDS-5 descopes this. Rationale: legacy distro-path scanning (e.g. `/EFI/ubuntu/grubx64.efi` without a shim chain) was a v0.2.0 heuristic that v0.8.3+ proper BLS/UKI discovery supersedes. Removing it tightens the trust story (fewer ambiguous paths).

### 8.2 `bls.rs` additions

`BlsEntry` struct gains:
```rust
pub(crate) struct BlsEntry {
    // existing fields: filename, title, version, machine_id, sort_key,
    //                  linux, initrd, options, architecture, tries_left, tries_done
    pub source_volume_index: usize,          // NEW — back-reference to Volume
    pub source_backend_tag: &'static str,    // NEW — for trust log + UI
}
```

`BootEntry` (the menu-facing struct in `discovery.rs`) gains the same two fields.

### 8.3 `main.rs` wiring

```rust
// Phase 7: discovery (replaces v0.8.3 Phase 7)
let entries = discovery::discover_all_entries(&mut volumes, &policy, &mut trust_log);
```

No other changes.

### 8.4 `boot.rs` wiring

The boot-entry dispatch uses `entry.source_volume_index` to select the Volume:

```rust
let volume = &mut volumes[entry.source_volume_index];
boot_linux_native(volume, /* ... */)
```

---

## 9. Test plan

### 9.1 Unit tests (mock volumes)

1. **Single volume, three entries.** Mock FsBackend with three `.conf` files under `/loader/entries/`. Assert 3 entries returned, each with `source_volume_index=0`.
2. **Two volumes, entries on each.** Mock FAT (ESP) + mock ext4, each with 2 entries. Assert 4 entries total, `source_volume_index` values 0 and 1 present.
3. **Empty volume.** Mock volume has no `/loader/entries/`. Assert 0 entries, no errors logged.
4. **Malformed .conf.** Mock volume has one valid + one garbage file. Assert 1 entry returned, 1 `bls_entry_invalid` event emitted.
5. **Read I/O error.** Mock read_dir returns FsError. Assert 0 entries from that volume, `bls_scan_io_error` event emitted.
6. **Policy denylist.** Policy denies `/vmlinuz-*`. Assert entries with matching `linux` field are dropped.
7. **Duplicate filename, different content across volumes.** Assert both returned with `bls_duplicate_filename` event.
8. **Duplicate filename, same content.** Assert only first kept.
9. **XBOOTLDR volume.** Mock volume with XBOOTLDR GUID. Assert entries discovered; no special handling beyond the regular scan.
10. **Boot-counter on RO volume.** `boot_counter_skipped_ro` event emitted once per boot, not per entry.

### 9.2 QEMU integration tests

1. **Fedora-style disk.** GPT with ESP + ext4 /boot. ext4 /boot has `/loader/entries/*.conf`. LamBoot boots, discovers the BLS entries via ext4, selects the newest, boots. VM 122 equivalent.
2. **EndeavourOS-style disk.** Everything on ESP (/efi). Entries at `/loader/entries/`. Existing v0.8.3 case; regression test.
3. **Pop!_OS-style disk.** ESP-native BLS + kernelstub-managed. Entries at `/boot/efi/loader/entries/` (which is `/loader/entries/` from the ESP volume's perspective). Current kernel selected, not Recovery (regression test for #51 fix).
4. **Multi-volume disk with BOTH ESP entries AND ext4 entries.** Assert both appear in menu, menu order follows bls.rs sort rules (ESP entries with sort-key first, ext4 entries second).
5. **Mixed spec-compliant + legacy.** ESP has BLS entries, `/EFI/ubuntu/grubx64.efi` also present. BLS entries win; legacy path is offered as chainload (§8.1).

### 9.3 Real-distro fleet regression

Part of SDS-2's fleet-regression shared harness. Each image:

- Ubuntu 25.10 (ESP-native BLS via kernel-install)
- Debian 13 (no BLS by default; UKI alternative)
- Fedora 43 (ext4 /boot BLS — SDS-5's primary target)
- EndeavourOS (ESP native)
- Pop!_OS (ESP native)
- Arch (user choice; test with ESP)

For each: LamBoot post-SDS-5 discovers the distro's entries via the correct scan path, boots the latest kernel, trust log contains the `bls_entries_found` event with the right backend tag.

---

## 10. Performance budget

| Phase | Budget |
|---|---|
| read_dir `/loader/entries` on each volume | 50 ms per volume × up to 3 volumes = 150 ms |
| Parse up to 16 .conf files | 16 × 5 ms = 80 ms |
| Sort + dedup | 10 ms |
| Policy filter | 5 ms |
| **Total BLS discovery** | **~250 ms** |

Total boot-time discovery including UKI + Windows + other-loaders: under 1 second. Measured in §9.2.

---

## 11. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| Fedora's GRUB-env-var fields in .conf produce unparseable options | Medium | Parser tolerates unknown fields; `$tuned_initrd`/`$grub_class` stay as literal strings in options; kernel's EFI stub ignores unknown cmdline tokens. Verified on VM 124 Pop!_OS (PASS). |
| openSUSE Btrfs /boot not readable in v0.9.x | Medium | Documented limitation; openSUSE users use UKI; v1.1+ Btrfs backend. |
| Legacy `/EFI/ubuntu/grubx64.efi` path descoped | Low | v0.2.0 heuristic; modern Ubuntu uses BLS or UKI. If a user still relies on it, they can pass `--keep-legacy-discovery` (add to install script? YAGNI — descope default). |
| Cross-volume dedup by content SHA is compute cost | Low | SHA-256 of a 1-KiB .conf file is ~50 µs; negligible. |
| Boot-counter rename on XBOOTLDR requires a second EspWriter | Low | Install script doesn't write to XBOOTLDR by default; if user manually places entries there, counter rename uses a separate writer. Implementation is mechanical. |
| `source_volume_index` stays valid through menu selection | Medium | Volumes slice is borrowed from main.rs for the entire boot; no mutation between discovery and boot_entry. Assert with Rust's borrow checker. |

---

## 12. Acceptance criteria

SDS-5 implementation is complete when all of the following are true:

- [ ] `lamboot-core/src/discovery.rs::discover_all_entries` takes `&mut [Volume]` and emits per-volume events.
- [ ] `BlsEntry` and `BootEntry` carry `source_volume_index` + `source_backend_tag`.
- [ ] §4.1 `scan_volume_for_bls` walks `/loader/entries/` on every volume that has it, using only `FsBackend` APIs (works uniformly across FAT and ext4).
- [ ] §5 dedup rules implemented; `bls_duplicate_filename` event emitted when rule 2 fires.
- [ ] `EspWriter::rename` exists; §6.1 counter policy applied; `boot_counter_skipped_ro` event emitted once per boot per RO volume.
- [ ] §9.1 mock-volume unit tests pass (10 cases).
- [ ] §9.2 QEMU integration tests pass (5 cases).
- [ ] §9.3 fleet regression: Fedora boots with the newest kernel via ext4 BLS discovery (VM 122 previous FAIL now PASS); EndeavourOS regression not broken (VM 123 still PASS); Pop!_OS regression not broken (VM 124 still PASS with correct kernel, not Recovery).
- [ ] `docs/INSTALL-REFERENCE.md` §7.3 text added.
- [ ] No `#[allow(clippy::…)]` introduced.
- [ ] Pre-commit passes.
- [ ] `CHANGELOG.md` entry under Unreleased.

---

## 13. Sign-off

Implementation-ready when:
- Founder review + acknowledgement.
- SDS-1 and SDS-2 implemented.
- No open question in §11 above Medium severity.

Implementation PR(s) must cite this SDS.

---

## Appendix A — BLS spec quick reference

From https://systemd.io/BOOT_LOADER_SPECIFICATION/ :

- Type 1: `$BOOT/loader/entries/<id>.conf` — text-config BLS entry. `$BOOT` = ESP or XBOOTLDR.
- Fields: `title`, `version`, `machine-id`, `sort-key`, `linux`, `initrd` (multiple), `options` (multiple, concatenated), `efi`, `architecture`, `devicetree`.
- Boot-counting suffix: `<id>+<tries-left>[-<tries-done>].conf`.
- Discovery ordering: see spec §4.

## Appendix B — References

- `docs/specs/SPEC-FS-BACKEND-TRAIT.md` — Volume dispatch + FsBackend trait.
- `docs/specs/SPEC-EXT4-INTEGRATION.md` — ext4 backend enabling non-spec-compliant location coverage.
- `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` §6 — trust-log event schema.
- `docs/FLEET-TEST-RESULTS-2026-04-21.md` — VM 122 FAIL case that motivates this SDS.
- `lamboot-core/src/bls.rs` — existing parser (sort logic unchanged by this SDS; commit 988851b fix preserved).
- `lamboot-core/src/discovery.rs` — current v0.8.3 implementation.
- BLS spec: https://systemd.io/BOOT_LOADER_SPECIFICATION/
- XBOOTLDR partition GUID: https://systemd.io/BOOT_LOADER_SPECIFICATION/#mount-points
