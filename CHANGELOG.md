# Changelog

All notable changes to LamBoot are documented here. Format inspired by Keep a Changelog; semantic versioning is loose during pre-1.0.

## [Unreleased]

(Forward work after v0.9.0 — see `docs/ROADMAP-V1.0-AND-BEYOND.md`.)

## [0.9.0]

The v0.9.x SDS ladder: native ext4 read backend, native PE loader,
native trust chain, BLS multi-FS discovery, legacy UEFI FS driver
deprecation. Plus first-class Pop!_OS / systemd-boot-discoverable-EFI
auto-discovery, the lamboot-tools v0.2.0 toolkit gap closures
(esp-deploy.sh canonical lib + offline `lamboot-esp deploy` +
fallback-foreign detection + BootOrder reorder), and a comprehensive
five-document strategic coverage analysis.

### Added — Pop!_OS / systemd-boot-discoverable-EFI auto-discovery

LamBoot now picks up Pop!_OS kernels at the kernelstub-managed
`\EFI\Pop_OS-<root-uuid>\vmlinuz.efi` layout and the Pop!_OS Recovery
counterpart at `\EFI\Recovery-<short-uuid>\vmlinuz.efi`. Same scanner
covers Garuda variants and any custom systemd-boot installation
that uses the `<distro>-<uuid>` directory pattern.

- New `lamboot-core/src/discovery.rs::discover_systemd_boot_dir_style`
  function. Iterates `/EFI/<dirname>` directories on the ESP, recognizes
  `<name>-<uuid>` patterns via `discovery_pure::looks_like_distro_uuid_dir`,
  reads sibling `vmlinuz.efi` + `cmdline` + `initrd.img` files, emits
  `EntryKind::LinuxLegacy` entries.
- New `lamboot-core/src/discovery_pure.rs` — host-testable pure helpers
  (recognizer + display renderer + path-style converter). Same
  `#[path]`-include pattern used by `bls_parse`, `pe_loader_pure`,
  `trust_log_pure`. 24 host tests covering positive/negative dirname
  recognition + display rendering + path conversion.
- Pop!_OS-specific display: `Pop_OS-<uuid>` renders as `Pop!_OS`
  (System76 escapes the `!` as `_` in directory names).
- Last-known-good kernel surfaced as a sibling entry: when
  `vmlinuz-previous.efi` is present, a `(previous kernel)` BootEntry is
  emitted with paired `initrd.img-previous`.

**Live evidence (VM 124 Pop!_OS COSMIC):** trust log shows
`entries_discovered total=7` with Pop!_OS Recovery selected via
`entry_selected id=sdboot-Recovery-79EB-58C6 kind=LinuxLegacy`,
`image_verified verified_via=degraded_trust_sb_direct sha256=c74f5c68…`,
`image_loaded_native loader=native_pe_loader` — full v0.9.x boot path
exercised on Pop!_OS for the first time.

### Added — EFI Fallback chainload self-loop guard

`discover_other_loaders` now takes the running LamBoot image's
SHA-256 and skips the synthetic "EFI Fallback" entry when
`\EFI\BOOT\BOOTX64.EFI` is itself a copy of LamBoot. Prevents
chainload self-loop when operators use the toolkit's
belt-and-suspenders pattern (`repair.fallback.replace_with_lamboot`)
to write LamBoot's signed binary to the firmware-fallback path.

`run_bootloader` computes the self-hash via `LoadedImageProtocol`
(image_base + image_size) and threads it through
`discovery::discover_all_entries(self_image_sha256: Option<[u8; 32]>)`.

### v0.9.x ladder context

The five SDSes that constitute the v0.9.x foundation are now landed
(see prior ## [Unreleased] sections, retained below for the per-SDS
detail). This release is the first to ship them all together as a
coherent stack.

— end of v0.9.0 entry —

## [Pre-v0.9.0 SDS ladder — historical detail retained]

With v0.8.4 shipped, the toolkit pivot's pause on SDS-2..SDS-6 is
cleared. Per-task polish starts here before diving into SDS-2.

### Added — SDS-2 (native ext4 read backend) — LANDED

Four-PR sequence (see `docs/specs/SPEC-EXT4-INTEGRATION.md` v1.1):

- **PR-1** `feat(fs): SDS-2 native ext4 read backend` (34b8775) —
  core `Ext4Backend` implementation against `ext4-view@0.9.3`
  exact-pinned. Probe unified with ext4 magic at offset 0x438 +
  0xEF53. Partition dispatcher in `main.rs` Phase 6.5 mounts every
  ext4 partition it finds via native backend.
- **PR-2** `test(ext4): host tests vs. ext4-view 0.9.3` (a6a18d0)
  — 14 host tests under `lamboot-fs-tests` pinning ext4-view API
  surface + error translation table.
- **PR-3** `test(ext4): QEMU harness + 6-distro fleet regression`
  (7719efd) — parametric QEMU harness; streaming fixture
  decompression; 6 distro fixtures (ubuntu/debian/fedora/fedora-
  lamboot/endeavouros/popos). BlockIO adapter (not DiskIo — spec
  v1.1 amendment) because DiskIo isn't universal on OVMF partition
  handles.
- **PR-4** `test(ext4): cargo-fuzz harnesses + parser extraction`
  (34b0068) — libFuzzer targets for `parse_ext4_superblock`,
  `compute_aligned_read`, and full `Ext4::load` API-drift canary.

### Added — SDS-5 (BLS multi-filesystem discovery) — LANDED

Four-PR sequence (see `docs/specs/SPEC-BLS-MULTI-FS.md` v1.1):

- **PR-1** `feat(discovery): volume-polymorphic BLS discovery`
  (f6ed798) — `discover_all_entries(volumes, policy, trust_log)`
  replaces ESP-only scanner. Every volume with `/loader/entries/`
  gets scanned via the FsBackend-uniform API. `BootEntry` and
  `BlsEntry` gain `source_volume_index` + `source_backend_tag`.
  Per-volume trust events: `bls_entries_found`,
  `bls_entry_read_failed`, `bls_entry_invalid`. Legacy
  `\EFI\fedora\vmlinuz*` distro-path heuristic descoped — proper
  BLS discovery on every volume supersedes it.
- **PR-2** `test(bls): pure parser extraction + 23 host tests`
  (0221121) — pure parsing split into `bls_parse.rs` so
  `lamboot-fs-tests` can include it verbatim via `#[path]`. 23
  host unit tests covering Fedora-style entries, multi-value
  initrd, multiple `options` lines, GRUB `$variable` stripping
  (including the v0.8.3 `\$var` form bug fixed inline), boot-
  counter filename suffixes, UAPI.10 version comparison, and BLS
  sort ordering (incl. task #51 Pop!_OS regression).
- **PR-3** `feat(discovery): dedup + boot-counter policy`
  (e3bcfb9) — parsed-entry-equality dedup via manual `PartialEq`
  excluding source fields. `bls_duplicate_filename` event on
  filename collision with different content. Boot-counter
  decrement now routes through `selection.source_volume_index`
  (previously always ESP); on non-FAT source, emits
  `boot_counter_skipped_ro` + human-readable warning instead of
  silent failure. Fixes the silent NotFound that occurred when
  selecting a Fedora BLS entry on ext4.
- **PR-4** `test(bls): VM 122 boot-through + harness + docs`
  (this commit) — `run-qemu-ext4-backend-test.sh` extended with
  SDS-5 assertions: `bls_entries_found` event presence, ext4-
  backend scan confirmation, Fedora-fixture entry count > 0.
  Fleet 5/5 PASS; Fedora fixtures (VM 122 + VM 201) discover 4
  BLS entries each on ext4 — **previously invisible in v0.8.3,
  now visible + selectable + bootable-via-shim-chainload**.
  (Native boot-via-ext4 remains gated on SDS-3's PE loader per
  spec v1.1 §8.4 amendment.) `docs/INSTALL-REFERENCE.md` §5
  documents the `$ESP/loader/entries/` write policy.

**Spec v1.1 amendments** (all three SDS-5 specs):

- §5 dedup key → `(filename_lowercase, parsed BlsEntry equality)`.
  SHA-256 content hashing deferred until SDS-3 brings `sha2` in
  naturally; parsed equality is semantically superior anyway.
- §6.3 signature corrected to landed `EspWriter::rename(dir_path,
  old_name, new_name)` — intra-directory, matches UEFI
  `File::rename` semantics.
- §8.4 boot wiring re-homed to SDS-3. SDS-5 delivers
  discovery + visibility; native kernel-read-from-ext4 is SDS-3.

### Added — SDS-3 (native Rust PE loader) — LANDED

Four-PR sequence (see `docs/specs/SPEC-NATIVE-PE-LOADER.md` v1.1):

- **PR-1** `feat(pe_loader): SDS-3 PR-1 — native PE loader core + pure module`
  — new `lamboot-core/src/pe_loader.rs` (Volume-dependent side:
  allocate_pages, zero, copy sections, apply relocations, install
  LoadedImageProtocol, start_image) + `lamboot-core/src/pe_loader_pure.rs`
  (pure parse/validate/relocate/SHA-256). Goblin 0.10.5 + sha2 0.10.8
  exact-pinned. Every `unsafe` block carries a `SAFETY:` comment.
- **PR-2** `test(pe_loader): SDS-3 PR-2 — host unit tests via #[path]`
  — 19 tests covering §5.1 validation + §9 relocation-math + error
  taxonomy + SHA-256 known-vectors + LamBoot's own lambootx64.efi
  as the known-good PE corpus entry.
- **PR-3** `feat(boot): SDS-3 PR-3 — native-vs-firmware branch + policy`
  — boot.rs wires `boot_uki` + `boot_linux` through the native path
  when policy + verify allow. `LoaderNativePeMode::Auto` default
  routes through `pe_loader::load_pe` + `pe_loader::start_image`
  after `ShimLock::Verify` succeeds; fallback to `uefi::boot::load_image`
  when verify is unavailable. New trust events: `image_loaded_native`
  (SHA-256 + backend + path), `image_load_failed` (with
  `PeLoadError::as_log_token()` token), `image_loaded_firmware`
  (audit flag for the fallback path). Chainload targets stay on the
  firmware path because they may have imports the native loader
  rejects by design.
- **PR-4** `test(pe_loader): SDS-3 PR-4 — fuzz + harness + docs`
  — cargo-fuzz target `pe_load` in the standalone fuzz crate (12M+
  runs in 30s smoke, zero crashes); QEMU harness extended with
  image-load event tallies; CONFIGURATION-GUIDE `[loader]` section;
  example policy.toml with `[loader].native_pe`.

**Policy gate** (SDS-3 §2.2 amendment #2): `policy.toml [loader].native_pe`
accepts `"auto" | "always" | "never"` (case-insensitive). Default
`"auto"` — the safe rollout choice with one-flag rollback to firmware.

**Security effect** on a Fedora ext4 `/boot` system under default
policy: the shim-15.8 `ShimLock`-uninstall failure mode is
**structurally unreachable** for the kernel-load step. LamBoot
calls `ShimLock::Verify(kernel_bytes)` once, then loads the kernel
via its own PE loader — `BS->LoadImage` is never invoked for the
kernel. Combined with SDS-6 (no legacy FS driver load for ext4),
the entire v0.8.3 failure path is eliminated for the 95%-of-users
ext4 configuration.

**Spec v1.1 amendments** (SPEC-NATIVE-PE-LOADER.md):
- §2.2 `load_pe_stream` deferred (goblin::PE::parse is &[u8]-only;
  kernels fit RAM with headroom). Tracked in `docs/ROADMAP.md`.
- §2.2 new `[loader].native_pe` policy gate following SDS-6 pattern.
- §13.1 host tests via #[path] pure-module include, not mocked
  firmware.
- §13.2 corpus from LamBoot's own binary + hand-crafted bad-PE
  fixtures, not captured kernels.

**§16 acceptance** — v0.9.0 scope complete except the release-
validating live boot-through, which is human-in-the-loop on actual
VM 122 hardware (SDS-3 §13.5 — not a CI-time check). Specifically:
  [x] goblin + sha2 pinned, default-features=false
  [x] pe_loader.rs with LoadedImage, PeLoadError, load_pe, start_image
  [x] §13.1 unit tests pass (19/19)
  [x] §13.2 corpus parses — LamBoot's own lambootx64.efi
  [x] §13.3 QEMU integration — `run-qemu-native-pe-harness.sh` boots
       LamBoot with a synthesized BLS entry, auto-selects via
       `loader.conf timeout=1` (with QMP `sendkey ret` as defensive
       backup), and asserts `image_verified` + `image_loaded_native`
       events appear in the serial log. CI-time exerciser of the
       native PE path; closes P-S3-1 in V0.9.0-RELEASE-PLAN.
  [x] §13.4 fuzz — 12M+ runs / 30s / zero crashes; extended pe_load
       657M-run / 30-min clean
  [x] §13.5 VM 122 live boot — validated on Fedora 43 Sway
       VM (separate ext4 /boot): `image_verified sha256=c734236e…
       verified_via=degraded_trust_sb_off` → `image_loaded_native`
       same sha256, loader=native_pe_loader; Fedora 6.19.12-200.fc43
       booted cleanly. Also validated on VM 120 Ubuntu 25.10 under
       shim+MOK SB-on (verified_via=shim_mok, sha256=4839d9d7…).

### Added — SDS-6 (legacy UEFI FS driver deprecation) — LANDED for v0.9.0 scope (the §2.4 tarball-content removal of `ext4_x64.efi` from `dist/` is explicitly v1.0-deferred behind founder sign-off and is NOT a v0.9.0 gate)

- `lamboot-core/src/policy.rs` — new `[drivers].legacy_uefi_drivers`
  policy.toml field with values `"auto" | "always" | "never"`
  (case-insensitive). Default `"auto"`. `DriversLegacyMode` enum
  added to the `Policy` struct with documented semantics.
- `lamboot-core/src/drivers.rs` — per-driver gating. In `"auto"` mode,
  drivers whose target filesystem is natively covered by a compiled-in
  backend are skipped. Current coverage (via `Ext4Backend` from SDS-2):
  `ext4_*.efi`, `ext2_*.efi`, `ext3_*.efi`. FAT is always UEFI-native
  so `fat_*.efi`/`vfat_*.efi` (not currently bundled) would also skip.
  `"always"` restores v0.8.3 load-everything behavior; emits
  `legacy_driver_redundant` when loading a driver whose FS is already
  covered. `"never"` skips all drivers + emits
  `legacy_driver_skipped_policy`.
- **Trust-log event taxonomy expanded** for audit clarity:
    * `legacy_driver_loaded` — replaces the old `driver_loaded`
      for FS drivers. Diagnostic module loads (`\EFI\LamBoot\modules\`)
      keep their distinct code path and are unaffected.
    * `legacy_driver_rejected` — replaces `driver_rejected`.
    * `native_backend_preferred` — emitted per driver skipped
      because a native backend covers its FS.
    * `legacy_driver_skipped_covered` — companion to
      `native_backend_preferred`; annotates the Auto-mode decision.
    * `legacy_driver_skipped_policy` — emitted per skip in Never mode.
    * `legacy_driver_redundant` — emitted in Always mode when a
      natively-covered driver is loaded anyway.
- `examples/policy.toml` — documents the new `[drivers]` section.
- Security effect: under Auto mode on an ext4-only system, LamBoot
  now loads **zero** third-party UEFI filesystem binaries at boot.
  The shim 15.8 `ShimLock`-uninstall failure mode is structurally
  unreachable for that configuration — matches SDS-2's §1.2
  architectural claim.

- `tools/lamboot-install` — new `--with-drivers-legacy=MODE` flag
  (`auto | all | none`); default `auto` skips driver install for
  natively-covered filesystems. `--with-drivers` retained as alias
  for `--with-drivers-legacy=all`. `phase2_assess_drivers` gates
  `NEED_FS_DRIVER` on `is_filesystem_natively_covered(BOOT_FSTYPE)`
  in Auto mode. `none` warns loudly if `/boot` is non-native.
- `tools/lamboot-install --update` — new `prune_natively_covered_drivers`
  pass removes `ext4_x64.efi` / `ext2_x64.efi` / `ext3_x64.efi`
  (and `-signed` + aarch64 variants) from the ESP when they're
  residue from a v0.8.x install and policy is Auto. Re-installing
  them on a v0.9.x install is explicit: `--with-drivers-legacy=all`.
- `docs/INSTALL-REFERENCE.md` — new "SDS-6 Driver Install Policy"
  section documenting the four flag values + prune behavior.

**Spec v1.1 amendments** (SPEC-UEFI-FSDRV-DEPRECATION.md):

- §1 cross-repo scope — SDS-6 is a single logical deliverable
  crossing lamboot-dev drivers.rs + policy AND lamboot-install
  (both in this repo). Tarball content changes (§2.4 removal of
  ext4_x64.efi from dist/) are a v1.0 release artifact deferred
  behind v0.9.0 + founder sign-off.
- §3 native-coverage decision uses compile-time knowledge
  (`filesystem_natively_covered(filename)` static match) rather than
  a runtime probe against `&[Volume]`. Equivalent behavior, simpler
  code, no main.rs phase reorder needed. When v1.1+ adds a
  compile-time-gated `BtrfsBackend`, the match extends via `#[cfg]`.

### Added — SDS-4 (native trust chain) — LANDED

Four-PR sequence (see `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` v1.1):

- **PR-1** `feat(trust): SDS-4 PR-1 — verify_kernel_bytes + VerifiedBytes + §6.4 invariant`
  (2a328ba) — `verify_kernel_bytes(bytes, path, trust_log)` is the
  single canonical kernel-verify site. Returns `VerifiedBytes<'b>`
  (zero-copy borrow + pre-computed SHA-256 + verified_via token) and
  emits the `image_verified` trust event with the digest attached.
  The §6.4 release-mode `assert_eq!` between the bytes we verified
  and the bytes `pe_loader` loaded (via `LoadedImage::sha256()`) is
  the TOCTOU bright line: if they diverge, the bootloader aborts.
  `trust_log.rs` schema extended with `verifier_tag` (e.g. `"shim-15.8"`)
  + `sha256_hex` fields; stable `verified_via` vocabulary constants
  pinned at this layer. Spec v1.1 amendments #1–4 recorded inline.
- **PR-2** `feat(trust): SDS-4 PR-2 — Step 5/6/7/8/11/12 events throughout boot flow`
  (5846d38) — the intermediate trust events — `policy_loaded`,
  `policy_invalid`, `volume_mounted`, `entries_discovered`,
  `entry_selected`, `kernel_measured`, `cmdline_measured` — are
  recorded at every decision point so the audit timeline lines up
  with SDS-4 §7's Step-by-Step boot diagram. `boot_entry` now takes
  `&mut [Volume]` and `split_first_mut`s the ESP slot out so
  `native_load_and_start` can call `trust_log.flush(esp_for_flush)`
  right before `start_image`, guaranteeing the `image_verified` +
  `image_loaded_native` pair is persisted before control transfers.
- **PR-3** `docs(trust): SDS-4 PR-3 — SECURITY-MODEL v0.9.x + claims contract + verify-claims.sh`
  (d0ebeb2) — `docs/SECURITY-MODEL.md` bumped to 0.9.x with an
  authoritative-reference blockquote pointing to the SDS; new
  "What changed in v0.9.x (from v0.8.3)" section explains the
  native-path security story end-to-end. `docs/WEBSITE-CONTENT.md`
  §7 gains a marketing-claim contract front-matter + new §7.4
  "Verified claims — every sentence backed by code" subsection
  tying on-site prose to SDS-4 §8.1. `lamboot-core/src/security_override.rs`
  module-doc narrowed to flag its scope as **legacy UEFI FS driver
  loads only** — kernel-load path no longer reaches it under SDS-4.
  New `tools/verify-claims.sh` (chmod +x, CI-ready) walks §8.1
  permitted-claims and confirms each file + symbol reference still
  resolves, greps user-facing docs for §8.2 prohibited substrings
  with WARN-only behavior (so disclaimers stay allowed).
  Current tree: 10/10 claim references pass, 2 legitimate disclaimers
  flagged.
- **PR-4** `test(trust): SDS-4 PR-4 — host tests + QEMU assertions + pure split`
  (this commit) — `lamboot-core/src/trust_log.rs` split into a pure
  module (`trust_log_pure.rs` — schema, accumulator, stable vocabulary,
  JSON-Lines serializer) + a thin UEFI wrapper (`flush()` via
  `EspWriter`). 19 host tests under `lamboot-fs-tests/tests/trust_log_pure.rs`
  pin the JSON field order, optional-field elision, JSON escape
  behavior, sequence monotonicity, accumulation across flushes,
  verify→load ordering, sha256 round-trip, and every token value in
  `ALL_VERIFIED_VIA`. `run-qemu-ext4-backend-test.sh` gains three
  SDS-4 soft-assertion blocks (§6.4 sha256 cross-check, §6.2
  ordering, §6.3 vocabulary closure) — hard-fail when either event
  is present with a violation, neutral (~) when the harness menu
  timeout means neither fires.

**Spec v1.1 amendments** (SPEC-NATIVE-TRUST-CHAIN.md):

- §6.1 adds `verified_via = degraded_trust_sb_direct` for
  firmware-DB-direct LamBoot loads (SB on, no shim in chain) —
  same trust model as SB-off from LamBoot's perspective (the
  admin's db key covers the tree). Distinct token for audit
  clarity.
- §6.4 invariant promoted from debug-only `debug_assert!` to
  release-mode `assert_eq!` — a TOCTOU-class divergence is a
  bright-line abort, not a log warning.
- §7 `VerifiedBytes` carries a zero-copy byte borrow rather than
  cloning — SHA-256 computes once on the original, `pe_loader`
  consumes the same slice.
- §8.4 `boot_linux` stays one function with policy-branch inside
  rather than splitting into `boot_linux_native` +
  `boot_linux_firmware` — keeps call-site topology single-owner
  for the trust-log flush ordering.

**Acceptance coverage** (SDS-4 §13):
  [x] §13.1 host unit tests — trust_log schema (19 tests passing)
  [x] §13.2 QEMU integration — soft assertions in ext4-backend harness
  [x] §13.3 live-boot pass on VM 122 hardware — validated.
       VM 122 Fedora 43 Sway (separate /boot ext4, SB off): full
       trust chain `image_verified → image_loaded_native` with
       matching sha256 `c734236e…`, verified_via=degraded_trust_sb_off,
       loader=native_pe_loader, kernel 6.19.12-200.fc43 booted
       cleanly. Companion validation on VM 120 Ubuntu 25.10
       (single-root ext4, SB-on shim+MOK): same invariant satisfied
       with verified_via=shim_mok, sha256=4839d9d7…. Two SB states
       × two partition layouts covered.

### Fixed — pre-v0.9.0 polish

- **Policy defaults reconciled (Task #43).** Runtime `Policy::default()`
  in `lamboot-core/src/policy.rs` now matches `examples/policy.toml`:
  `default_timeout_ms = 5000`, `crash_threshold = 3`, `measured_boot =
  true`. Added a compiled-in policy floor (`MAX_CRASH_THRESHOLD = 10`,
  `MAX_WATCHDOG_GRACE_SECONDS = 300`, plus build-time switches
  `FLOOR_REQUIRE_SECURE_BOOT` / `FLOOR_REQUIRE_MEASURED_BOOT`) so
  offline tampering of `policy.toml` cannot weaken security below the
  baseline. `load_policy()` now returns `(Policy, Vec<PolicyClampReport>)`
  and clamped fields surface as `log::warn!` entries at boot. See
  `docs/specs/SPEC-POLICY-DEFAULTS.md` for the full rationale.

- **Up-front BIOS / non-UEFI refusal in `lamboot-install` (Task #45).**
  New `detect_firmware_mode()` function runs in Phase 1 (after distro
  detection) and refuses to install on a BIOS-booted host by checking
  for `/sys/firmware/efi`. Previously, Phase 6 would eventually fail
  against `efivarfs`, but only after Phase 1–5 had already copied files
  to the ESP. New check: refuses with distro-specific migration guidance
  (Ubuntu/Debian/Fedora/Arch all get `lamboot-migrate to-uefi --disk`
  pointers); `--force` overrides for advanced users preparing a disk
  for a different host; chroot is detected and the check is skipped.

- **Fallback self-loop prevention (Task #49).** `install_fallback()` now
  identifies what bootloader currently occupies `\EFI\BOOT\BOOTX64.EFI`
  (via PE string-markers for shim / GRUB / systemd-boot / rEFInd /
  Windows) and refuses `--fallback` if it would displace a distro
  bootloader, explaining the self-loop risk: if LamBoot's primary
  Boot#### entry ever fails, firmware re-invokes `\EFI\BOOT\BOOTX64.EFI`
  which — if that path is LamBoot — produces a loop with no recovery
  unless the operator manually edits the ESP. `--force` overrides for
  removable-disk scenarios. Previously every `--fallback` silently
  replaced whatever was there.

## [0.8.4]

**Release posture:** coordinated `lamboot v0.8.4` + `lamboot-tools v0.2.0`
per `docs/CROSS-REPO-STATUS.md`. v0.8.3 shipped the signing + audit
layer; v0.8.4 closes the Proxmox-toolkit coordination gaps so the
companion `lamboot-tools` v0.2.0 toolkit (11 tools across 3 RPM
subpackages) can ship alongside.

Tarball SHA256: `4671691f597627ee354f36e945dc5d68a75709af4524a88c5a6aa9ae87056830`

Proxmox integration test on `pve.a.lamco.io` (VM 120): PASS on all 8
verifications. See `docs/analysis/V0.8.4-PROXMOX-INTEGRATION-TEST-2026-04-22.md`.

See also: `lamco-admin/lamboot-tools v0.2.0`.

### Fixed

- **`tools/lamboot-hookscript.pl` rewritten to fw_cfg file-reference pattern** (commit `2892446`). Pre-0.8.4 hookscript called `qm set --args` during pre-start, which silently failed because Proxmox config-locks the VM config during that lifecycle phase. The rewrite removes all `qm set` calls: `lamboot-pve-setup` from the toolkit sets the permanent `args:` line once on a stopped VM, and this hookscript's pre-start job becomes writing `/var/lib/lamboot/<VMID>.json` for QEMU to expose via fw_cfg. Version header `# version: 0.8.4` allows `lamboot-pve-setup doctor-hookscript` to detect and verify. See `docs/specs/` + `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md` §11.2 for the full protocol.

### Added

- **`lamboot-install --toolkit-prompt`** (commit `c4a9b4e`). Interactive `Install lamboot-tools for diagnostic and repair utilities? [y/N]` prompt at the end of a successful install, plus `--install-toolkit` / `--no-install-toolkit` flag overrides for non-interactive scripts. Distro-aware install guidance: Fedora/RHEL/EPEL shows `dnf copr enable lamco/lamboot-tools`; Debian/Ubuntu/Arch shows the source-tarball URL with a note that native packaging lands in `lamboot-tools v0.3`. Skipped on `--dry-run`, `--update`, `--quiet`, or partial failure.
- **`/etc/lamboot/fleet.toml` schema v1 consumption** (commits `ada5cb6` + `2892446`). Both `tools/lamboot-monitor.py` and `tools/lamboot-hookscript.pl` now read the shared fleet config authored as canonical in `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md` §16 Appendix C. Monitor seeds argparse defaults for `--alert-webhook` (HTTPS enforced) and `--log-path` from `[monitor]`. Hookscript reads `[hookscript]` inject flags plus `[roles]` explicit VMID mapping and `[tags]` tag-to-role mapping for per-VM role resolution. Additive with graceful fallback: missing file / missing TOML parser / wrong schema version / malformed TOML all fall back to hardcoded defaults so v0.8.4 is safe to deploy before fleet.toml exists.
- **`docs/CROSS-REPO-STATUS.md`** (commit `51ce546`). Rolling coordination tracker between `lamboot-dev` and `lamboot-tools-dev` per the toolkit spec §14.5. Mirror counterpart lives in the toolkit repo; owner perspectives flipped between them.

### Changed

- **`docs/LAMBOOT-TOOLS-OVERVIEW.md` rewritten** (commit `51ce546`) to reflect the real state of the companion toolkit. Previously described "5 bash CLI utilities"; now describes 11 tools across 3 RPM subpackages (`lamboot-tools`, `lamboot-migrate` dual-pub, `lamboot-toolkit-pve`) with Copr-based install flow and Option 2 packaging architecture.
- **`README.md` adds "Diagnostic and repair utilities" section** (commit `b812fea`) linking `github.com/lamco-admin/lamboot-tools` and cross-referencing `CROSS-REPO-STATUS.md` + the toolkit spec.
- **`docs/STATUS-2026-04-22-TOOLKIT-PIVOT.md` §9 "Post-Q state" appendix added** (commit `51ce546`). Captures what happened after the pivot doc was written: the toolkit repo ran its Session A–Q arc the same day, turning 5 v0.1.0 scripts into 11 production-grade tools; all 23 R1–R23 research questions from §4 of the pivot doc are resolved in `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md`; `lamboot-migrate v1.0.0` ships SDS-7 in full. §1-§8 of the pivot doc preserved as historical record.
- **`docs/ROADMAP.md` "Toolkit Pivot" intro updated** (commit `51ce546`) with current toolkit state and `CROSS-REPO-STATUS.md` pointer.
- **`docs/specs/SPEC-LAMBOOT-MIGRATE.md` §14 reconciliation flipped to RESOLVED** (commit `51ce546`). The v1.0.0 implementation landed in `lamboot-tools-dev` Session C closes every gap from §14.1–§14.7, plus `--remove-grub` distro-aware cleanup beyond spec. Each row marked `RESOLVED` / `RESOLVED+` / `KEPT`.

### Added — should-have cross-references

- **`docs/KEY-GENERATION.md` §10 "Operator tooling"** (commit `51ce546`) back-links to `lamboot-signing-keys` in the toolkit; lists `generate`/`rotate`/`enroll`/`sign-binary`/`revoke`/`list`/`show`/`verify`/`import`/`export` subcommands.
- **`docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md` "Operator tooling"** (commit `51ce546`) maps `sign-binary`/`rotate`/`verify` to procedures in the strategy doc.
- **`docs/OVMF-VARS-PROXMOX.md` §12** (commit `51ce546`) notes `lamboot-pve-ovmf-vars` in the toolkit is a mirror of `tools/build-ovmf-vars.sh` here; canonical source stays in this repo.

## [Unreleased — earlier items carried forward]

### Added

- **`lamboot-inspect`** — new diagnostic tool for parsing LamBoot's on-disk artefacts. Stdlib-only Python, six subcommands (`trust-log`, `boot-log`, `summary`, `show`, `verify`, `dump`). Parses the SDS-4 schema-v2 trust log with full validation, renders boot.log with phase timing, produces one-page last-boot summaries, verifies SDS-4 §8.1 website-claims against code-path evidence (CI-ready), and creates diagnostic bundles for bug reports. Includes man page (`lamboot-inspect(1)`), bash + zsh completions, 44-case host test suite, and user guide at `docs/LAMBOOT-INSPECT.md`. Ships in the release tarball alongside `lamboot-install`.

### Changed

- **SDS-7 spec clarified.** `SPEC-LAMBOOT-MIGRATE.md` now correctly frames itself as the v1.0 target for the existing `lamboot-migrate` v0.1.0 tool in `lamco-admin/lamboot-tools-dev`, not a green-field design. Added §14 "Deviations from existing v0.1.0" with a full reconciliation table. **Flipped to RESOLVED status in the v0.8.4 prep section above.**

### Infrastructure

- **Layer-2 FS backend trait** (SDS-1) implemented: `FsBackend` trait with `FatBackend` adapter + `FatStream` streaming, `Volume` dispatch coordinator with 8 MiB LRU cache, `EspWriter` FAT-only-by-construction write path, `Ext4Backend` skeleton (SDS-2 swap-in target). Migrated all 13 consumer files to the new API. Extracted pure value types to `fs_types.rs` for host testing. New `lamboot-fs-tests` host crate with 36 passing tests.

## [0.8.3]

Release posture: **the signing + audit layer.** v0.8.3 ships the production signing pipeline, SecurityOverride (Path F), trust-evidence log, and install-script hardening. It is the foundation every later release builds on.

**What v0.8.3 is good for:** Secure-Boot-off installs (full feature set, all filesystems, no caveats); Secure-Boot-on installs with UKI on the ESP; Secure-Boot-on installs with firmware-DB-signed kernels; Proxmox VM fleets using UKI-based images.

**Known limitation accepted for v0.8.3:** stock `/boot` on ext4 under Secure Boot fails on shim 15.8 (current Ubuntu/Debian shim). Root cause: shim 15.8 uninstalls its `ShimLock` protocol after our UEFI ext4 driver's `StartImage` completes, so later kernel verification has no shim to delegate to. Fully diagnosed with per-hook counters; documented in `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md`. **Structural fix in v1.0** via a native Rust ext4 reader (`ext4-view`) plus a native PE loader (on top of `goblin`) — no UEFI FS driver load required, no firmware `LoadImage` re-check. See `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` and `docs/INTEGRATED-PLAN-V0.8.3-TO-V1.0.md`.

**Effective positioning for v0.8.3:**
- "Modern Rust UEFI bootloader for Linux, with an honest signing and audit layer"
- NOT "Secure Boot on stock Linux just works" — that's v1.0's promise (~14 weeks out)

### Changed (breaking default)

- **`lamboot-install` now makes LamBoot the default boot entry on install.** Previous default was to preserve the existing default and append LamBoot last in `BootOrder` (safe-trial behavior). New default matches `grub-install` / `bootctl install` / `refind-install`: when you install LamBoot, the firmware will pick LamBoot next boot. The previous default is always preserved as the next entry in `BootOrder`, so it remains one keystroke away from the firmware boot menu. Opt back into the old behavior with `--no-make-default`. The explicit `--set-default` / `--make-default` flags are retained for scripts that want to be unambiguous.

### Added

- **Production signing key hierarchy** — PK 4096, KEK 4096, db 2048 per shim-MOK compatibility constraints (see `docs/KEY-GENERATION.md` and `docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md`).
- **Bootloader signing pipeline** — `build.sh` + `tools/sign-lamboot.sh` produce `lambootx64-signed.efi` using `sbsign` with SBAT section added via `llvm-objcopy` (GNU objcopy produces corrupt PE output).
- **Session-cached signing workflow** — `tools/sign-unlock` + `tools/sign-lock` for tmpfs-backed db.key decryption per terminal session.
- **Secure Boot deployment documentation** — `docs/SECURE-BOOT-DEPLOYMENT.md` covering four configurations: (1) SB disabled, (2) firmware db enrollment, (3) shim + MOK, (4) custom OVMF VARS (Proxmox zero-touch).
- **MOK enrollment guide** — `docs/MOK-ENROLLMENT-GUIDE.md` with screen-by-screen MokManager walkthrough.
- **Proxmox OVMF VARS guide** — `docs/OVMF-VARS-PROXMOX.md` for zero-touch fleet deployment per storage backend.
- **Install-script Secure Boot flags** — `--signed`, `--no-shim`, `--no-mok` on `tools/lamboot-install`. MOK auto-enrollment via `mokutil --import`.
- **Path F: SecurityArchProtocol override** — new `lamboot-core/src/security_override.rs` implements the systemd-boot pattern for driver loading under shim+MOK. Resolves MOK-signed driver ACCESS_DENIED on shim < v16 (Ubuntu 25.10, Debian 13, current Fedora). See `docs/analysis/SECURE-BOOT-ECOSYSTEM-AND-REAL-FIX-2026-04-21.md`.
- **ShimRetainProtocol support** — `request_shim_retain_protocol()` sets the shim 15.8+ variable to prevent ShimLock protocol from being uninstalled after driver `StartImage()` (shim #444 / sd-boot precedent).
- **Trust-evidence log** — `lamboot-core/src/trust_log.rs` writes structured JSON to `\loader\boot-trust.log` on the ESP recording every image-authentication decision. First-of-kind in the Linux bootloader space.
- **Unified Kernel Image (UKI) first-class menu integration** — automatic discovery of UKIs in `\EFI\Linux\*.efi` with PE section metadata extraction (`.osrel`, `.cmdline`, `.uname`).
- **Migration Tier 2 (`--replace`)** — ESP + NVRAM backup, GRUB cmdline extraction, atomic migration to LamBoot with fallback entry.
- **Debian/Ubuntu kernel hooks** — `kernel-hooks/zz-lamboot-postinst` + `zz-lamboot-postrm`, deployed to `/etc/kernel/{postinst,postrm}.d/` on install, keeping BLS entries synchronized with installed kernels.
- **BLS entry lifecycle handling** — install script's `--update` path detects stale LamBoot-owned BLS entries (kernel they reference no longer exists) and regenerates.
- **`SECURITY.md`** — responsible-disclosure policy and security-reporting procedures.
- **`docs/SECURITY-MODEL.md`** — honest threat model: what LamBoot defends against vs. what remains in the ecosystem. Breaks from the bootloader norm of glossing over SB trust-chain gaps.
- **Public repo reservations** — `lamco-admin/lamboot` and `lamco-admin/lamboot-tools` GitHub repositories (initially empty placeholders).

### Changed

- Retired "LamBoot Dev" test keys; replaced with production keys named `CN=LamBoot Platform Key`, `CN=LamBoot Key Exchange Key`, `CN=LamBoot Release Signing Key 2026` under `O=Lamco Development, C=US, ST=IL, emailAddress=office@lamco.io`.
- Driver loading now wraps `uefi::boot::load_image` with SecurityArchProtocol override for MOK-trust delegation.
- `boot.rs::find_volume_for_path` now returns `Option<usize>` — explicit None instead of silently defaulting to the ESP when a BLS-referenced kernel file is absent.

### Fixed

- `build.sh` now copies `kernel-hooks/` into `dist/kernel-hooks/` so the install script's `find_dist_file` locates the Debian/Ubuntu postinst/postrm scripts.
- Install script's `--update` no longer treats previously-generated BLS entries as "native distro" — it distinguishes manifest-tracked entries and regenerates when their kernels are missing.
- Logging clarity: `boot.rs` no longer emits misleading `Kernel found on volume 0` when the kernel was actually absent everywhere.

### Documented limitations

- Canonical-signed Ubuntu kernels may fail to load under LamBoot in Config 3 (shim+MOK) on distros where `/boot` is on ext4 root. Workaround: use UKIs in `\EFI\Linux\`, or Config 4 with Canonical's CA added to custom OVMF VARS. See `docs/SECURITY-MODEL.md` §3.1.
- Trust-evidence log is not yet cryptographically signed (planned for v0.9.x via Path G — own PE loader).
- ShimLock authenticode verification has edge cases with PE-gap-containing kernels that shim doesn't accept even when they chain to trusted CAs. Root cause documented in `docs/analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md` §4.4.

### Roadmap

- **v0.9.x:** Path G — own PE loader with direct Authenticode verification, structured cert-chain walking, signed trust log entries, Trust Evidence Panel in GUI, policy-driven trust enforcement.
- **v1.0:** Microsoft `rhboot/shim-review` submission (first Rust bootloader), PQ signature support via MOK, `composefs` integration, `bootupd`-style self-update.
- **v2.0:** TrenchBoot / DRTM integration, full UKI management suite, upstream PE-loader crate contribution to `uefi-rs`.

---

## [0.2.0]

Pre-release development; not formally tagged. Work tracked in `docs/STATUS-*.md`.
