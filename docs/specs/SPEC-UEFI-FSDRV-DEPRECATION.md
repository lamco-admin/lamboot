# SPEC-UEFI-FSDRV-DEPRECATION: Legacy UEFI Filesystem Driver Deprecation

**SDS number:** SDS-6 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/drivers.rs` (gated, eventually deleted), `lamboot-core/src/security_override.rs` (scope narrows, eventually deleted), `tools/lamboot-install` (driver-deploy phase becomes opt-in), `dist/EFI/LamBoot/drivers/` (tarball content becomes optional per architecture+feature), `docs/LAMBOOT-TOOLS-OVERVIEW.md` (amended), `docs/SECURE-BOOT-DEPLOYMENT.md` (amended)
**Depends on:** SDS-1 (native backend trait), SDS-2 (ext4-view native backend), SDS-5 (multi-FS discovery using native backends) MUST be landed first
**Blocks:** nothing. This SDS is the orderly retirement of the v0.8.x legacy driver path.
**External dependencies:** None. This SDS is about removing dependencies we currently ship (rEFInd/EfiFs GPL-2.0 binaries).

---

## 1. Overview

This specification defines how LamBoot phases out the `\EFI\LamBoot\drivers\*.efi` UEFI filesystem driver mechanism introduced in v0.2.x and hardened in v0.8.3. The deprecation replaces a model built on third-party GPL-2.0 binaries loaded via `BS->LoadImage`+`StartImage` (the root cause of the v0.8.3 shim-15.8 `ShimLock`-uninstall failure) with a native-first model where every filesystem LamBoot reads is read by Rust code compiled into the LamBoot binary itself.

### 1.1 Context

**v0.8.3 ships `\EFI\LamBoot\drivers\` with:**

- `ext4_x64.efi`, `btrfs_x64.efi`, `ntfs_x64.efi`, `xfs_x64.efi`, `zfs_x64.efi`, `f2fs_x64.efi`, `iso9660_x64.efi` for x86_64
- `ext2_x64.efi` (the ext4 driver also handles ext2/ext3)
- `f2fs_aa64.efi`, `zfs_aa64.efi` for aarch64

These are third-party binaries from the rEFInd project (GPL-2.0) and EfiFs project (GPL-3.0). At install time, `lamboot-install` copies them to the ESP; at boot, LamBoot loads them via `LoadImage` + `StartImage` under the SecurityOverride hook, then calls `connect_all_controllers` to trigger firmware to attach them to block-I/O handles, after which `SimpleFileSystem` becomes available on those partitions.

**This is the mechanism that broke v0.8.3.** The `LoadImage` + `StartImage` of any UEFI FS driver triggers shim 15.8 to uninstall its `ShimLock` protocol on the child's `StartImage` return, leaving LamBoot unable to verify the kernel it subsequently wants to load. Full root-cause in `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md`.

**v1.0's replacement:**

- **ext4** — covered natively by `Ext4Backend` (SDS-2) using the `ext4-view` crate. Zero UEFI driver load; zero shim interaction. This is the 95%-of-users case.
- **FAT** — covered natively by `FatBackend` (SDS-1) via `uefi-rs`'s `SimpleFileSystem` protocol. FAT is already native to UEFI; no third-party driver needed.
- **Btrfs, XFS, ZFS, NTFS, F2FS, iso9660** — still require UEFI FS drivers in v1.0 because no mature `no_std` Rust readers exist. Deferred to v1.1+ community contribution (per `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` §5).

The deprecation is therefore **partial for v1.0**: ext4 + FAT paths go native, niche filesystems continue via the UEFI driver path as a documented fallback with known security consequences.

### 1.2 What this SDS delivers

- A **version-by-version schedule** for retiring the legacy driver path, from v0.9.x (SDS-1..5 land) to v2.0 (driver loading entirely removed).
- **Backend-selection policy** in `partitions.rs` / `fs_backend_ext4.rs` / etc.: when a volume's filesystem has a native backend, use it; only fall through to the UEFI driver path for filesystems without native coverage.
- **Install-script defaults**: drivers become opt-in via `--with-drivers-legacy` (replacing the v0.8.3 default-on behavior for ext4 driver). By default `lamboot-install` installs ONLY the drivers needed for filesystems present on the system AND not covered natively.
- **SecurityOverride retirement plan**: the module stays in v0.9.x for legacy driver loads, is deleted in the LamBoot release after all v1.1+ community-contributed native backends land.
- **Migration path** for existing v0.8.x installs: `lamboot-install --update` running under v0.9.x / v1.0 prunes the now-unused ext4 driver binary from the ESP.
- **Trust-log events** marking legacy-driver boots so auditors can distinguish native-backend boots from legacy-driver boots.
- **Documentation updates** explaining the transition to users.
- **GPL-bundled-binary licensing note** for the tarball, unchanged for now (we still ship some drivers in v1.0) but scheduled for removal when all natively covered.

### 1.3 What this SDS does NOT deliver

- **Removing UEFI-driver-load machinery from the codebase in v0.9.x.** The machinery stays; it is gated OFF for filesystems with native backends. Full removal is a v2.0 task contingent on full native coverage.
- **Shipping v1.0 without Btrfs/XFS/NTFS drivers.** They remain shipped because users with those filesystems need them. The security story for v1.0 is honest: "ext4 is native; niche filesystems still use drivers."
- **Writing Btrfs/XFS/ZFS native backends.** v1.1+ community work per the ROADMAP.
- **Removing `SecurityOverride` in v0.9.x.** The hook is still required for UEFI FS driver loads when they happen. Removal is scheduled for when the last UEFI driver path goes away.
- **A legacy-driver signer.** Third-party drivers arrive already-signed (rEFInd's binaries are signed against our LamBoot db key by `tools/sign-lamboot.sh` during release build). No change.
- **Deprecating anything outside the FS-driver path.** Diagnostic modules (`\EFI\LamBoot\modules\`) stay. Their trust model is identical (signed by LamBoot key), they don't interact with shim (they're loaded in LamBoot's own security context for on-demand execution), and they're a user-facing feature.
- **Removing bundled GPL binaries from the tarball in v1.0.** Still shipped as long as we load any of them at runtime. Licensing note in `LICENSE` stays.

### 1.4 Constraints

All SDS-1 through SDS-5 constraints apply. Additionally:

- Backward compatibility through v1.x: an existing v0.8.x install must continue to boot after an in-place upgrade to v0.9.x / v1.0. The ESP's `\EFI\LamBoot\drivers\` directory content might change, but the user's chosen filesystem continues to work.
- Legacy path MUST be gated behind a single policy flag (`policy.toml` field) + compile-time feature flag, so a v1.0+ distribution may opt out of bundling the legacy drivers entirely.
- Every v0.9.x/v1.0 boot that uses a legacy UEFI driver MUST emit a trust-log event identifying which filesystem and which driver; this distinguishes audit trails.
- The v0.8.3 `security_override.rs` stays functional through v1.x unchanged; its scope is documented in its module header as narrowed.

---

## 2. The deprecation schedule

### 2.1 v0.8.x (current, done as of commit `988851b`)

**State:**
- `drivers.rs` loads every driver present in `\EFI\LamBoot\drivers\` via `LoadImage`+`StartImage` under `SecurityOverride`.
- `lamboot-install` copies every x86_64 driver from `dist/EFI/LamBoot/drivers/` to the ESP by default.
- Trust-log event: `driver_loaded` with `verified_via=security_override` per driver.
- Security consequence: the documented shim-15.8 `ShimLock`-uninstall issue.

**Change:** none. v0.8.x is frozen; this is the baseline.

### 2.2 v0.9.0 — preview: native ext4 coexists with legacy drivers

**State:**
- SDS-1 + SDS-2 implemented. `FatBackend` and `Ext4Backend` exist.
- `partitions.rs::mount_all_volumes` probes for ext4 first via `probe_ext4_superblock`; if a volume is ext4, constructs `Ext4Backend` natively. If it's something else (Btrfs, XFS, NTFS…), the mount fails silently and the volume falls through to legacy driver handling (same as v0.8.x).
- `drivers.rs` STILL RUNS but `drivers::load_drivers` is gated: it skips loading drivers whose target filesystem already has a mounted native backend. Specifically: if any volume mounted via `Ext4Backend` exists, `ext4_x64.efi` is NOT loaded.
- `lamboot-install` default driver-install set trims: `ext4_x64.efi` is no longer copied by default. Drivers for non-native filesystems (btrfs, xfs, zfs, ntfs, f2fs, iso9660) still copied when `--with-drivers` is set (default).
- Existing v0.8.3 installs that have `ext4_x64.efi` on the ESP: it's still there, LamBoot notices `Ext4Backend` is already mounted, skips loading it. No harm. `lamboot-install --update` removes the unused file.

**New trust-log events:**
- `native_backend_preferred` emitted per volume when a native backend was selected over a legacy driver. Payload: `fs=ext4 backend=ext4-view@0.9.3 skipped_driver=ext4_x64.efi`.
- `legacy_driver_loaded` replaces `driver_loaded` for loads that genuinely went through the old path (non-ext4, niche FS).

**Behavior:**
- Users with `/boot` on ext4: native path. No shim-uninstall. Clean boots.
- Users with `/boot` on btrfs/xfs/zfs: legacy path. Same v0.8.x security consequence. Documented.
- Users with `/boot` on FAT: native path (FAT was always native).

### 2.3 v0.9.x — SDS-3 native PE loader adds kernel-load native path

**State:**
- SDS-3 implemented. Kernels load via `pe_loader` instead of `BS->LoadImage`.
- The shim-15.8 `ShimLock`-uninstall issue no longer affects kernel loads, even if a UEFI FS driver was loaded to reach the kernel bytes.
- Legacy driver path still gated as in v0.9.0.

**Behavior:**
- Users with ext4 `/boot`: fully native (SDS-2 read + SDS-3 load).
- Users with btrfs/xfs `/boot`: UEFI driver load triggers shim's protocol-uninstall, BUT kernel load via `pe_loader` doesn't need shim anymore. Native loader calls `ShimLock::Verify(bytes)` ONCE before any driver load; if it was available then, verdict stands. **Even the legacy FS path works now under SB**, because we extracted shim-verify from the LoadImage codepath.
- The interplay is documented carefully in SDS-4's §3.2 degraded-trust narrative.

### 2.4 v1.0 — native ext4 + FAT promoted to primary; ready-for-release

**State:**
- SDS-1 through SDS-5 fully implemented and tested.
- v0.9.x preview stabilized.
- Policy gate: `policy.toml` `[drivers]` section with `legacy_uefi_drivers = "auto" | "always" | "never"`. Default `"auto"` per §4.
- Install-script: `--with-drivers-legacy` flag replaces `--with-drivers`; defaults to auto-detect (copy drivers only for filesystems present on the system AND not natively covered).

**Tarball content:**
- `dist/EFI/LamBoot/drivers/` ships only: `btrfs_x64.efi`, `xfs_x64.efi`, `ntfs_x64.efi`, `zfs_x64.efi`, `f2fs_x64.efi`, `iso9660_x64.efi`. Note **`ext4_x64.efi` and `ext2_x64.efi` REMOVED** — native backend supersedes.
- Same for aarch64.
- `LICENSE` notes the remaining GPL binaries.

**User-visible change:** "SB on stock Linux just works" because ext4 is native. Legacy-driver path continues for niche cases.

### 2.5 v1.1 — community-contributed Btrfs native backend (if delivered)

**State:** Per ROADMAP, community may contribute `BtrfsBackend` with the same `FsBackend` trait surface.

**When landed:**
- `btrfs_x64.efi` removed from tarball.
- openSUSE Tumbleweed users with `/boot` on Btrfs gain native coverage.

### 2.6 v1.x — additional community backends (XFS, ZFS)

Similar mechanism. Each native backend removes its corresponding `*_x64.efi` from the tarball.

### 2.7 v2.0 — full retirement (goal, not commitment)

**Condition:** All filesystems LamBoot encounters at a real user's install are covered by native backends. No bundled GPL binaries. No `drivers.rs`. No `SecurityOverride`.

If this condition is unreachable (e.g. NTFS never gets a Rust no_std reader), `v2.0` simply keeps whichever legacy drivers remain. The goal is stated for direction; the promise is to eliminate what we can.

---

## 3. Backend-selection logic

Post-SDS-6, `partitions.rs::mount_all_volumes` becomes:

```rust
pub(crate) fn mount_all_volumes(image: Handle) -> Result<Vec<Volume>, …> {
    let mut volumes = Vec::new();

    // Phase 1: enumerate all BlockIO handles.
    let handles = uefi::boot::find_handles::<BlockIO>()?;

    // Phase 2: for each handle, probe native backends in priority order.
    for handle in handles {
        // Try FAT (UEFI-native).
        if let Some(volume) = try_mount_fat(handle) {
            volumes.push(volume);
            continue;
        }

        // Try ext4 native (SDS-2).
        if let Some(volume) = try_mount_ext4_native(handle) {
            volumes.push(volume);
            continue;
        }

        // Future: try_mount_btrfs_native, try_mount_xfs_native when v1.1+.

        // Fall through — will be picked up by legacy driver path if enabled.
    }

    // Phase 3: legacy driver loading, gated by policy.
    if policy::drivers_legacy_mode() != DriversLegacyMode::Never {
        let legacy_drivers = drivers::discover_installed_drivers();
        for driver in legacy_drivers {
            // Only load a driver if NO native backend already covers its FS.
            if native_backend_covers(&driver.filesystem(), &volumes) {
                trust_log.record(TrustEvent::new("native_backend_preferred")
                    .with_path(driver.name())
                    .with_note(&format!("fs={} backend={}",
                                        driver.filesystem(), backend_tag_for(&driver.filesystem()))));
                continue;
            }
            drivers::load_driver(driver, &mut trust_log)?;
        }

        // Phase 4: re-enumerate block handles after driver loads may have
        // produced new SimpleFileSystem instances.
        let new_handles = uefi::boot::find_handles::<SimpleFileSystem>()?;
        for handle in new_handles.iter().filter(|h| !already_mounted(h, &volumes)) {
            if let Some(volume) = try_mount_fat(*handle) {
                volumes.push(volume);  // treated as FAT (it's SimpleFileSystem)
            }
        }
    }

    Ok(volumes)
}
```

The key property: **a volume is mounted through AT MOST one backend — the first one that successfully probes.** Native backends are tried first; legacy drivers fill the residue.

### 3.1 `native_backend_covers` function

```rust
fn native_backend_covers(filesystem: &str, volumes: &[Volume]) -> bool {
    match filesystem {
        "ext2" | "ext3" | "ext4" => {
            // Any ext4 native backend covers all three.
            volumes.iter().any(|v| v.identity().backend_tag.starts_with("ext4-view@"))
        }
        "fat" | "vfat" | "fat32" => true,  // FAT is always native via UEFI.
        "btrfs" => {
            volumes.iter().any(|v| v.identity().backend_tag.starts_with("btrfs-"))
        }
        "xfs" => {
            volumes.iter().any(|v| v.identity().backend_tag.starts_with("xfs-"))
        }
        // NTFS, ZFS, f2fs, iso9660 have no native backend as of v1.0.
        _ => false,
    }
}
```

### 3.2 Policy gate

`policy.toml` (SDS-6 introduces):

```toml
[drivers]
# Whether to load legacy UEFI filesystem drivers from \EFI\LamBoot\drivers\.
#
# Values:
#   "auto" (default): load drivers only for filesystems without native backends
#   "always":        load all installed drivers (v0.8.3 behavior)
#   "never":         skip all legacy drivers; fail boot if a filesystem is
#                    unreachable
legacy_uefi_drivers = "auto"
```

`"never"` is useful for security-conscious users who have migrated entirely to native-supported filesystems and want to assert LamBoot will never load a third-party UEFI binary.

### 3.3 `drivers::load_drivers` change

Current (v0.8.3) signature:
```rust
pub(crate) fn load_drivers(image: Handle, esp: &mut EspVolume, tpm: &TpmContext, trust_log: &mut TrustLog) -> usize
```

v0.9.0 signature (unchanged structurally; internals gated):
```rust
pub(crate) fn load_drivers(
    image: Handle,
    esp: &mut EspVolume,
    tpm: &TpmContext,
    trust_log: &mut TrustLog,
    already_mounted_volumes: &[Volume],   // NEW — skip drivers covered natively
    policy_mode: DriversLegacyMode,       // NEW — "auto" | "always" | "never"
) -> usize
```

Driver binary enumeration, verification (signing), and `LoadImage`+`StartImage` stay identical to v0.8.3. Only the per-driver decision ("should we load this one?") changes.

---

## 4. Install-script changes

### 4.1 New flag

`tools/lamboot-install` gains:

```
  --with-drivers-legacy=MODE
                    Install legacy UEFI filesystem drivers to
                    \EFI\LamBoot\drivers\. MODE is one of:
                      auto   (default) — install drivers only for filesystems
                             present on this system AND not natively covered
                      all    — install all drivers we ship
                      none   — install no drivers

                    Supersedes the v0.8.3 --with-drivers flag. --with-drivers
                    continues to work as an alias for --with-drivers-legacy=all
                    during the v0.9.x → v1.0 transition.
```

### 4.2 Auto-detection algorithm

```bash
detect_present_filesystems() {
    # Enumerate filesystems of mounted partitions; return distinct sorted list.
    lsblk -no FSTYPE | sort -u | grep -v '^$' | tr '\n' ' '
}

is_natively_covered() {
    local fs="$1"
    case "$fs" in
        ext2|ext3|ext4|vfat|fat|fat32) echo "yes" ;;
        # Future v1.1+: btrfs, xfs if native backends land.
        *) echo "no" ;;
    esac
}

install_drivers_auto() {
    for fs in $(detect_present_filesystems); do
        if [[ $(is_natively_covered "$fs") == "no" ]]; then
            # Install driver for this fs if we have one.
            copy_driver_if_present "$fs"
        fi
    done
}
```

User-facing: `lamboot-install` on a system with `/boot` on ext4 + `/data` on btrfs → v1.0 copies `btrfs_x64.efi` to the ESP, does NOT copy `ext4_x64.efi` (native covers it).

### 4.3 Upgrade behavior

`lamboot-install --update` running against a v0.8.x install:

1. Detects existing `\EFI\LamBoot\drivers\ext4_x64.efi`.
2. Checks manifest → yes, we installed it.
3. Checks current-version native-coverage map → ext4 is covered.
4. Removes `\EFI\LamBoot\drivers\ext4_x64.efi` from the ESP.
5. Emits an install-log message explaining the removal.

Same for `ext2_x64.efi` at v1.0. (Both removed.)

### 4.4 No-regret guarantee

If a future LamBoot version re-introduces the need for a driver that was removed (hypothetical: we discover ext4-view has a critical CVE and we must fall back to the rEFInd driver while a fix ships), the driver binary is ALWAYS available in the release tarball and `lamboot-install --with-drivers-legacy=all` re-deploys it.

---

## 5. Trust-log interaction

New events in v0.9.0+:

| Event | When emitted | Replaces v0.8.3 event |
|---|---|---|
| `native_backend_preferred` | Per volume + native backend selection over a legacy driver | — |
| `legacy_driver_loaded` | Per legacy driver load that actually ran | replaces `driver_loaded` for non-native-covered cases |
| `legacy_driver_skipped_covered` | Per legacy driver skipped because native backend covers it | — |
| `legacy_driver_skipped_policy` | Per legacy driver skipped because policy=`never` | — |
| `fs_backend_selection` | Per mounted volume, records which backend won the probe race | — |

`driver_loaded` is retired from v0.9.0 for FS drivers; the name moves to `legacy_driver_loaded` for clarity. Log consumers must update accordingly — documented as breaking (SDS-4 §6.1 schema v2 covers this).

Diagnostic module loads (`\EFI\LamBoot\modules\*.efi`) continue to emit `driver_loaded` unchanged; they are not filesystem drivers and are not deprecated by this SDS.

---

## 6. Documentation updates

SDS-6 triggers edits to these user-facing docs:

### 6.1 `docs/LAMBOOT-TOOLS-OVERVIEW.md`

Add section: "Legacy filesystem drivers (v0.9.x → v1.0 transition)". Explain the deprecation schedule per §2. Point users at `docs/specs/SPEC-UEFI-FSDRV-DEPRECATION.md` for the authoritative story.

### 6.2 `docs/SECURE-BOOT-DEPLOYMENT.md`

Amend Config 3 + Config 4 sections: "ext4 `/boot` is natively supported in v1.0" removes the shim-15.8 caveat from those configs.

### 6.3 `docs/CONFIGURATION-GUIDE.md`

Add `[drivers]` section reference per §3.2.

### 6.4 `docs/INSTALL-REFERENCE.md`

Add `--with-drivers-legacy` flag per §4.1. Note `--with-drivers` alias.

### 6.5 `docs/ROADMAP.md`

Timeline entries for v1.1+ community-backend work that further reduces the legacy path.

### 6.6 `CHANGELOG.md`

v0.9.0: "ext4 /boot now natively supported; legacy UEFI ext4 driver deprecated but still bundled in tarball for users who opt in via `--with-drivers-legacy=all`."

v1.0: "Default install no longer deploys ext4/ext2 UEFI drivers; native backend covers both. Tarball still ships drivers for btrfs/xfs/zfs/ntfs/f2fs/iso9660 (opt-in via flag)."

---

## 7. Security narrative post-SDS-6

Combining SDS-4's trust chain with SDS-6's driver-path deprecation, the LamBoot v1.0 boot narrative becomes:

- **If your `/boot` is on ext4 (95% of users):** zero UEFI filesystem drivers are loaded. LamBoot reads your `/boot` via pure-Rust `ext4-view`. Kernel is verified via `ShimLock::Verify` once, then loaded via native PE loader. Shim interaction is minimal and well-defined. The v0.8.3 shim-15.8 `ShimLock`-uninstall failure mode is STRUCTURALLY IMPOSSIBLE.
- **If your `/boot` is on btrfs/xfs/zfs:** LamBoot still needs to load a UEFI FS driver via `LoadImage`+`StartImage`. `SecurityOverride` still applies. shim 15.8 still uninstalls `ShimLock` after the driver's `StartImage` returns — **but that doesn't matter**, because SDS-3's native PE loader no longer calls `BS->LoadImage` for the kernel. ShimLock's one-shot verify at §SDS-4 Step 10 happened before the driver load. The bytes are cached as trusted. Loading the kernel is a purely local operation.
- **If your `/boot` is on UKI on the ESP:** purely native; the UKI PE contains kernel+initrd, FAT ESP is UEFI-native, everything is in LamBoot's control.

**The shim-15.8 `ShimLock`-uninstall issue is solved for every configuration**, because the solution is architectural (don't rely on shim during/after driver load) not dependency-chasing (wait for shim 16).

§8 marketing-claims update: the honest claim becomes "**LamBoot v1.0 is the only Linux bootloader that boots stock distros under Secure Boot without requiring shim 16**."

---

## 8. Test plan

### 8.1 Per-milestone regression

**v0.9.0:**
- Installing on Fedora (ext4 `/boot`) leaves NO `ext4_x64.efi` on the ESP. `lsblk` + ESP directory listing confirms.
- `lamboot-install --update` on a pre-existing v0.8.3 install with `ext4_x64.efi` on ESP removes it cleanly. Manifest updates.
- Boot with `legacy_uefi_drivers = "auto"` + ext4 /boot: trust log shows `native_backend_preferred` event; no `legacy_driver_loaded` event for ext4; boot succeeds.
- Boot with `legacy_uefi_drivers = "always"` + ext4 /boot: trust log shows BOTH `native_backend_preferred` AND — no wait, if native is preferred we skip the driver. Revise: `legacy_uefi_drivers = "always"` means load it anyway. Trust log shows `legacy_driver_loaded` for ext4; native backend also mounts the same volume. `fs_backend_selection` event documents the tie-break (native wins; driver is loaded for side-effect-free redundancy — primarily for users debugging).
- Boot with `legacy_uefi_drivers = "never"` + btrfs /boot (no native backend in v0.9.0): trust log shows `legacy_driver_skipped_policy`; boot fails with a clear `no accessible /boot` error.

**v1.0:**
- Fresh install on Fedora: native-only operation. No UEFI FS driver load. VM 122 (v0.8.3 FAIL case) passes end-to-end.
- Tarball does not include `ext4_x64.efi` or `ext2_x64.efi`. `MANIFEST.sha256` confirms.
- Users with btrfs /boot (openSUSE) still boot fine via legacy driver path; trust log documents the driver load.

### 8.2 Upgrade path testing

Fleet regression captures a v0.8.3 install + runs `lamboot-install --update` against the same system running a v0.9.0 build. Assertions:

- ESP contents match §2.2 state.
- Boot still works (kernel reachable via native backend).
- `lamboot-install --remove` after upgrade cleans everything, including drivers that the v0.8.3 install had deployed.

### 8.3 Negative tests

- `lamboot-install --with-drivers-legacy=bogus`: exits with clear error.
- `lamboot-install --with-drivers-legacy=none` then `--update`: ESP has no drivers.

### 8.4 Policy edge cases

- `policy.toml` has `legacy_uefi_drivers = "never"` + ext4 /boot: works fine (ext4 is native; policy gate only affects legacy drivers).
- Policy has `"never"` + btrfs /boot: fails with a distinctive error.
- Missing `[drivers]` section: default to `"auto"`.

---

## 9. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| Users with btrfs /boot see no improvement in v1.0 | Medium | Documented limitation; UKI-on-ESP is the path. v1.1+ community Btrfs backend. |
| Schema change renaming `driver_loaded` → `legacy_driver_loaded` breaks v0.8.3 log consumers | Low | v0.8.3 log consumers are rare; we control the few that exist. Release notes call out the rename. |
| Removing ext4 driver from the default install confuses users who expected v0.8.3 behavior | Low | CHANGELOG is explicit; `--with-drivers-legacy=all` preserves old behavior; `lamboot-install --update` emits a message explaining the removal. |
| `lamboot-install --update` fails to delete the ext4 driver due to ESP permission issue | Low | Error path falls back to warning; boot continues fine (native backend takes over). |
| Policy `"never"` blocks a user from booting after an install accidentally left them on btrfs | Low | `"never"` is a user-opt-in; don't default to it. |
| GPL binary licensing note in LICENSE becomes stale | Low | Review per release; remove references to drivers we no longer bundle. |

---

## 10. Acceptance criteria

SDS-6 implementation is complete when all of the following are true (for v0.9.0 scope):

- [ ] `lamboot-core/src/drivers.rs::load_drivers` takes the new `already_mounted_volumes` and `policy_mode` parameters and gates per-driver decisions accordingly.
- [ ] `policy.toml` parser recognizes `[drivers].legacy_uefi_drivers` with "auto"/"always"/"never".
- [ ] `tools/lamboot-install` accepts `--with-drivers-legacy=auto|all|none` + preserves `--with-drivers` as alias for `=all`.
- [ ] `tools/lamboot-install --update` on a v0.8.x install: `ext4_x64.efi` / `ext2_x64.efi` are removed from the ESP.
- [ ] §5 new trust-log events fire at the right points.
- [ ] §6 doc updates landed.
- [ ] §8.1 v0.9.0 regression tests pass.

For v1.0:

- [ ] `dist/EFI/LamBoot/drivers/` does NOT ship `ext4_x64.efi` / `ext2_x64.efi`.
- [ ] `package-release.sh` asserts the tarball content matches §2.4.
- [ ] All §8 tests pass on the v1.0 release candidate.
- [ ] `docs/ROADMAP.md` updated with v1.1+ community-backend timeline.

---

## 11. Sign-off

Implementation-ready when:
- Founder review + acknowledgement.
- SDS-1, SDS-2, SDS-5 implemented.
- No open question in §9 above Medium severity.

Implementation PR(s) must cite this SDS.

---

## Appendix A — Current v0.8.3 driver inventory

Listed here because it's not written down elsewhere and we need to know what we're deprecating.

```
dist/EFI/LamBoot/drivers/
├── aarch64/
│   ├── f2fs_aa64.efi       (EfiFs, GPL-3.0)
│   └── zfs_aa64.efi        (rEFInd, GPL-2.0)
├── btrfs_x64.efi           (rEFInd, GPL-2.0)
├── ext2_x64.efi            (rEFInd, GPL-2.0) — covers ext2/ext3
├── ext4_x64.efi            (rEFInd, GPL-2.0)
├── f2fs_x64.efi            (EfiFs, GPL-3.0)
├── iso9660_x64.efi         (rEFInd, GPL-2.0)
├── ntfs_x64.efi            (rEFInd, GPL-2.0)
├── xfs_x64.efi             (rEFInd, GPL-2.0)
└── zfs_x64.efi             (rEFInd, GPL-2.0)
```

Signing: all are re-signed during `tools/sign-lamboot.sh` with LamBoot's db key so SecurityOverride delegation succeeds under SB.

## Appendix B — References

- `docs/specs/SPEC-FS-BACKEND-TRAIT.md` — SDS-1, trait that replaces driver-based access.
- `docs/specs/SPEC-EXT4-INTEGRATION.md` — SDS-2, the ext4 native implementation.
- `docs/specs/SPEC-NATIVE-PE-LOADER.md` — SDS-3, removes kernel-load dependency on LoadImage.
- `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` — SDS-4, §3.3 scope-narrowing of SecurityOverride.
- `docs/specs/SPEC-BLS-MULTI-FS.md` — SDS-5, uses native backends.
- `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` — the shim-15.8 issue this SDS architecturally resolves.
- `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` §5 — deferred-backend rationale.
- rEFInd drivers source: https://www.rodsbooks.com/refind/
- EfiFs drivers source: https://github.com/pbatard/efifs
