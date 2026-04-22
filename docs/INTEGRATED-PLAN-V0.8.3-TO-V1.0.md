# Integrated Plan: Ship v0.8.3, Flow Straight to v1.0

**Date:** 2026-04-21
**Status:** binding plan — supersedes release posture discussions in earlier analysis docs
**Scope:** everything from "get v0.8.3 out this week" through "v1.0 — SB on stock Linux just works"
**Architectural anchor:** `docs/ARCHITECTURE-LAYERS.md`
**Technical anchor:** `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md`

---

## 1. Principles driving this plan

1. **No hacks.** The Path G solution (native PE loader + native read-only FS backends + UKI fallback) lands as clean Layer 2 + Layer 3 additions per the architecture doc. No patches, no duplication, no sync layers.
2. **Release momentum matters.** v0.8.3 contains real, working improvements (Path F SecurityOverride, trust-evidence log, production signing pipeline, honest docs, install-script hardening). Shipping it **now** with honest caveats is strictly better than shipping it **later** with an over-sold promise.
3. **SDS-first on Path G.** Every module of Path G work is preceded by a Software Design Specification that's reviewable standalone. No code starts without a checked-in SDS.
4. **Continuous flow.** The moment v0.8.3 tarball is published, SDS authoring begins. No "pause and regroup" phase between release and next-version work.
5. **Technical verification over speculation.** Every crate adoption, every LOC estimate, every feature claim is verified against source before being written into a plan.

## 2. v0.8.3 — honest ship, this week

### 2.1 Ship scope (confirmed in code as of `68682c0`)

- Production Secure Boot signing pipeline — `sign-lamboot.sh` + session-cached key unlock (tmpfs)
- SecurityOverride (Path F) — Rust port of sd-boot's `Security2Arch` hook pattern, with full diagnostic counters
- Trust-evidence log v1 — JSON lines on `\loader\boot-trust.log`, with `kernel_load_failed` events capturing `ACCESS_DENIED`/`SECURITY_VIOLATION` with per-hook counters and a pointer to the analysis doc
- Install script hardened — `--signed` / `--no-shim` / `--no-mok` flags, `--kernel-firmware-db-signed` guardrail, `ShimRetainProtocol` pre-population, make-LamBoot-default default, idempotent `--remove` + empty-dir cleanup, kernel hooks for Debian/Ubuntu + kernel-install plugin for Fedora
- BLS lifecycle — stale-entry detection + regeneration, preflight validation, UKI first-class menu entries
- Four Secure Boot configurations *documented honestly*; known limitation (shim 15.8 + /boot on ext4) acknowledged and linked to the NATIVE-FS-AND-PE-LOADER-STRATEGY doc as the v1.0 resolution

### 2.2 Honest release positioning (binding — website must match)

v0.8.3 ships **"the signing, auditing, and integration layer"**. It does **not** ship **"SB on stock Ubuntu just works"** — that's v1.0's promise.

What v0.8.3 *is* good for:
- Secure-Boot-off installs (everything works, full GUI, driver loading, BLS, UKI, diagnostic modules)
- Secure-Boot-on installs with **UKI on the ESP** (tested, works)
- Secure-Boot-on installs with **kernel staged on ESP** (works, but manual until v0.9.x)
- Secure-Boot-on installs with **a firmware-DB-signed kernel** (self-signed UKI, custom build shops)

What v0.8.3 *is not* good for:
- Secure-Boot-on + stock Ubuntu/Debian/Fedora /boot-on-ext4 with MOK-signed kernels — documented as "v1.0 target; Path G native-FS work in progress"

### 2.3 Release sequence (this session / this week)

All gated on explicit human go per `~/.claude/CLAUDE.md` Public Action Prohibitions.

| Step | Action | Gate |
|---|---|---|
| 1 | Final review of `docs/WEBSITE-CONTENT-CORRECTIONS-2026-04-21.md` — delete the parts now superseded by native-FS strategy; keep the SB-off positioning + honest-limitation framing | No gate — docs |
| 2 | Update `README.md` + `CHANGELOG.md` to reflect v0.8.3 as "signing + audit layer; v1.0 = native boot" | No gate — docs |
| 3 | Rebuild signed artifacts on current HEAD (Cargo.toml version 0.8.3, tag moves as fixes land) | No gate — local build |
| 4 | Regenerate tarball + SHA256 | No gate — local build |
| 5 | Tag `v0.8.3` at polished HEAD | No gate — private repo |
| 6 | Push to private dev repo | No gate — private repo |
| 7 | **GATE:** `./export-to-public.sh --dry-run 0.8.3` (review diff) | Founder review |
| 8 | **GATE:** `./export-to-public.sh 0.8.3` — publishes to `github.com/lamco-admin/lamboot` | Founder approval |
| 9 | **GATE:** `gh release create v0.8.3 …` with tarball asset | Founder approval |
| 10 | **GATE:** Website publish (LAMCO-website team lands the v0.8.3 corrections) | Founder + website team |
| 11 | **GATE:** Announcement posts (HN, OMG!Linux, OMG!Ubuntu, LinkedIn, X) | Founder approval |

### 2.4 Lamco-admin ledger update

On tag push: update `~/lamco-admin/projects/lamboot/PUBLICATION-LOG.md` with:
- Release artifacts (tarball sha256, tag commit)
- Pre-flight check results (fmt, clippy, check, sbverify, tarball audit)
- Approval timestamps for each gate
- Known limitations documented + linked to resolution docs

## 3. v0.9.x Path G — the SDS phase (starts the moment v0.8.3 ships)

SDSes are written **first**. Code does not start until all six are checked in and cross-reviewed.

### 3.1 The six SDSes (ordered by implementation dependency)

| Order | SDS file (in `docs/specs/`) | Blocks | Writing effort |
|---|---|---|---|
| 1 | `SPEC-FS-BACKEND-TRAIT.md` | All FS work | 2–3 days |
| 2 | `SPEC-EXT4-INTEGRATION.md` | ext4-view adapter | 2–3 days |
| 3 | `SPEC-NATIVE-PE-LOADER.md` | Native kernel boot | 3–4 days |
| 4 | `SPEC-NATIVE-TRUST-CHAIN.md` | Security claims | 2 days |
| 5 | `SPEC-BLS-MULTI-FS.md` | BLS discovery across ESP/ext4/XBOOTLDR | 2 days |
| 6 | `SPEC-UEFI-FSDRV-DEPRECATION.md` | Migration path from legacy drivers | 1 day |

Total SDS authoring: **2–3 weeks** of focused writing. This is NOT wasted time — each SDS is the checkpoint where a reviewer (initially the founder, later contributors / security auditors) can push back before any code drifts.

### 3.2 SDS-1 — FS Backend Trait (the foundation)

**Purpose:** define the Layer 2 trait `FsBackend` that all filesystem backends implement.

**Must specify:**
- Trait signature:
  ```rust
  pub trait FsBackend {
      fn read(&mut self, path: &Path) -> Result<Vec<u8>>;
      fn exists(&mut self, path: &Path) -> Result<bool>;
      fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>>;
      fn metadata(&mut self, path: &Path) -> Result<Metadata>;
      fn label(&self) -> Option<&str>;
      fn uuid(&self) -> Option<Uuid>;
  }
  ```
- `Path` type (our own; follows `std::path` semantics but no_std-compatible)
- `Metadata` fields: size, is_dir, is_symlink, mode (for permission-preserving reads of signed binaries)
- Error type `FsError` with variants for NotFound, Io, UnsupportedFeature, Corrupt, ReadOnly (for future write attempts — fail closed)
- How `fs.rs` dispatches across backends: volume identity (GPT GUID, partition UUID, FS label) → backend selector
- Ordering guarantees (or lack thereof) for `read_dir`
- Thread/interrupt safety (none; boot-time single-threaded UEFI context)
- Test plan: mock backend for unit testing; round-trip test corpus

**Output artifact:** a reviewable spec that the ext4 and FAT adapter implementations must satisfy.

### 3.3 SDS-2 — ext4-view Integration

**Purpose:** specify how `ext4-view` plugs into LamBoot's Layer 2.

**Key technical facts (verified 2026-04-21 from the crate source):**
- ext4-view v0.9.3, 9,576 LoC, no_std + alloc, MIT/Apache-2.0, Google-authored
- Supports all features stock `mkfs.ext4` emits: extents, 64bit, flex_bg, metadata_csum, metadata_csum_seed, dir_index (htree), huge_files, has_journal, sparse_super, large_file, extra_isize, orphan_file, journal recovery for unclean unmounts
- Exposes `Ext4Read` trait for block-I/O adapter — exactly the plug-in point
- Has its own `xtask/uefibench/` that exercises the crate in UEFI context with uefi-rs 0.36
- Explicitly rejects (returns UnsupportedFeatures error on load): compression, separate journal device, multi-mount protection, large-xattr-in-inode, data-in-inode (inline_data), data-in-dir-entry, meta-block-groups, large-directories — none of these are `mkfs.ext4` defaults

**Must specify:**
- Adapter module path: `lamboot-core/src/fs_backend_ext4.rs`
- How `Ext4Read` is implemented on top of our `BlockIoProtocol` reads from a GPT partition handle
- Caching strategy: ext4-view v0.9.2+ has an internal block cache; decide if we tune it
- Version-pinning policy: lock to the exact ext4-view version shipped with each LamBoot release; update only on CVE or feature need
- Upstream PR posture: if we find missing features (unlikely given verification above), we file with nicholasbishop/ext4-view-rs rather than forking
- Graceful-degradation test: filesystem using a disallowed feature → `FsError::UnsupportedFeature`, trust-log event `fs_unsupported_feature`, fall back to UKI path if available

### 3.4 SDS-3 — Native PE Loader

**Purpose:** specify LamBoot's own PE loader that replaces `BS->LoadImage` for kernel loads.

**Must specify:**
- Module path: `lamboot-core/src/pe_loader.rs`
- Dependencies: `goblin 0.10.5` (PE header + section parsing, `no_std` via disabled default features)
- Supported PE subsystems: EFI Application (10), EFI Boot Service Driver (11), EFI Runtime Driver (12). Kernels are EFI Applications.
- Image-type coverage: bare EFI-stub kernels (vmlinuz-*), UKIs (PE with embedded .linux/.initrd/.cmdline), our own signed drivers, diagnostic modules
- Memory layout: prefer preferred-base; relocate if collision (ASLR-style reloc) — relocation-table-driven
- Entry point invocation: `extern "efiapi" fn entry(ImageHandle, *mut SystemTable) -> Status`
- LoadOptions passing: set via `LoadedImage::set_load_options` equivalent (our own "loaded image" state since we bypassed firmware LoadImage)
- Authenticode handling: **not verified by pe_loader**. Verification is the caller's responsibility (via `ShimLock::Verify` at the right point in the flow per SPEC-NATIVE-TRUST-CHAIN).
- Trust log events: `image_loaded_native` with sha256, entry point address, size; `image_load_failed` with reason
- Failure modes: unsupported architecture, relocation overflow, malformed PE, allocation failure — each distinct
- Test corpus: sample kernels from VM 100 (Debian), VM 120 (Ubuntu), VM 201 (Fedora); self-signed UKI; a known-malformed PE fuzz input

### 3.5 SDS-4 — Native Trust Chain

**Purpose:** spec the end-to-end trust narrative under Path G, so marketing / audit claims match code.

**Must specify:**
- Chain diagram: firmware DB → shim (verified by DB) → LamBoot (verified by shim via DB-or-MOK) → `ShimLock::Verify` on kernel buffer (verified before native PE load) → native `pe_loader::load_and_start`
- Trust-log events at each step with field layout
- What happens when `ShimLock::Verify` fails (trust-log event, no load, menu re-entry)
- What happens when ShimLock isn't available (e.g., SB-off, or direct firmware DB path): we fall back to firmware DB verification via `SecurityOverride`; document the degraded-trust-but-still-valid narrative
- Degrees of trust we do NOT implement (own MOK parser, firmware DB parser) — these stay ecosystem dependencies for v1.0; natural Path H work for v1.1+
- Claims the website may make, with line references to code

### 3.6 SDS-5 — BLS Multi-FS Discovery

**Purpose:** specify how BLS entries are discovered and updated across FAT ESP / ext4 /boot / XBOOTLDR.

**Must specify:**
- Discovery order: ESP first, XBOOTLDR second, ext4 /boot (if detected) third
- Write-policy: boot-counter rename only on writable backends (ESP, XBOOTLDR — both FAT). RO ext4 → warn in trust log, proceed without counter update.
- Non-spec deployments (BLS on ext4) handled as a graceful-degradation class
- Install-script policy: always write *new* BLS entries to ESP for spec compliance (even if we discover/read existing ones on ext4)

### 3.7 SDS-6 — Legacy UEFI FS Driver Deprecation

**Purpose:** specify the migration path from `\EFI\LamBoot\drivers\*.efi` (Path F) to native Layer 2 backends.

**Must specify:**
- v0.8.x: existing driver machinery ships unchanged (backward compat)
- v0.9.x: native backend preferred; UEFI FS driver becomes fallback when native backend unavailable (e.g., Btrfs before v1.1 gets native)
- v1.0: stabilized — native ext4 + FAT are primary; UEFI drivers still supported for niche FS (btrfs, xfs, zfs, f2fs)
- v2.0: optionally remove UEFI FS driver loading if native coverage is complete
- What happens to SecurityOverride: stays for UEFI driver path + for niche-FS filesystem driver use; kernel-load path no longer uses it

### 3.8 SDS review process

Each SDS goes through:
1. Author drafts (1–4 days depending on scope)
2. Self-review + cross-ref update (1 day)
3. Push to private repo branch
4. Founder review (async)
5. Revisions (0–2 rounds)
6. Merge to main

Code work on the subsystem covered by an SDS starts the moment that SDS merges. Work on ***other*** SDSes can parallelize with code work if dependency order allows.

## 4. v0.9.x implementation phase

### 4.1 Milestones

**v0.9.0 — FS abstraction + ext4 native path (preview)**
- Layer 2 trait + FAT adapter + ext4 adapter landed
- Kernel loading still uses `BS->LoadImage` (not yet native PE) — but reads from ext4 natively
- Trust log records `fs_backend_used=ext4-view v0.9.3`
- UEFI FS driver path still present as fallback
- v0.9.0 is "early preview" — opt-in via policy flag

**v0.9.x — Native PE loader merged**
- `pe_loader.rs` lands with test corpus coverage
- `boot.rs` gains a branch: if Layer 2 backend + PE loader + ShimLock all green, go native; else fall back
- Trust log records `boot_path=native` or `boot_path=uefi-loadimage`
- Broad QEMU testing across Ubuntu / Debian / Fedora

**v0.9.y — Native path is default under SB**
- Policy flip: `native_boot = true` by default in `policy.toml`
- Fallback still works for niche cases
- Full cross-distro QEMU + bare-metal validation
- Release candidate quality

**v1.0 — "SB on stock Linux just works"**
- v0.9.y + docs + website alignment + broad validation
- Optional: shim-review submission starts in parallel (doesn't block v1.0)
- Announcement posture: "first Linux bootloader that reads /boot read-only, verifies once, in memory-safe Rust"

### 4.2 Effort calendar (solo developer)

| Phase | Effort | Cumulative |
|---|---|---|
| SDS authoring (6 specs) | 2–3 weeks | 3 wk |
| ext4 adapter + Layer 2 trait implementation | 1–2 weeks | 5 wk |
| PE loader implementation + unit tests | 2–3 weeks | 8 wk |
| Integration + orchestration updates in `boot.rs` + `main.rs` | 1 week | 9 wk |
| Cross-distro QEMU test matrix | 1–2 weeks | 11 wk |
| Bare-metal validation + polish | 1–2 weeks | 13 wk |
| Docs + website sync | 1 week | 14 wk |
| **Total to v1.0** | **~3.5 months** | |

**v0.9.0 ship target:** ~5 weeks from v0.8.3 ship (ext4 native reading, but still firmware LoadImage for kernel)
**v0.9.x ship target (native PE + ext4):** ~9 weeks from v0.8.3
**v1.0 ship target:** ~14 weeks from v0.8.3

Release cadence of ~monthly in the v0.9.x series — each release is a genuine improvement, not a bugfix churn.

## 5. Parallel tracks (not blocking v1.0)

### 5.1 Btrfs native backend (v1.1+)
Community contribution preferred. Scope: 2000–4000 LOC on top of `btrfs-no-std` structs. Not a v1.0 blocker (UKI workaround exists).

### 5.2 XFS native backend (v1.1+)
Currently no Rust starting point — all from scratch. Lower priority (XFS /boot is rare). Not a v1.0 blocker.

### 5.3 Shim-review submission (parallel)
Can be filed anytime; approval is on Red Hat / upstream shim-review-board timeline. Not blocking v1.0.

### 5.4 Post-quantum signing posture (v1.x research)
Dual-sign with Dilithium alongside RSA. Not blocking v1.0. Explicit roadmap item.

## 6. Testing discipline

### 6.1 Per-SDS test expectations

- SDS-1: mock backend + trait compliance test suite
- SDS-2: ext4-view round-trip: dump /boot from VM 100/120/201 into test images, read same files via ext4-view, diff output. Target: byte-identical.
- SDS-3: PE loader round-trip: load VM kernel into memory via pe_loader, compare sections against goblin-parsed reference; fuzz malformed inputs.
- SDS-4: trust-chain events produced in expected order on golden-path boots; rejection path produces `kernel_load_failed` event.
- SDS-5: BLS discovery across three volume layouts (ESP-only, ESP+XBOOTLDR, ESP+ext4-boot).
- SDS-6: migration: legacy drivers + native backend coexist cleanly; per-FS selection correct.

### 6.2 Per-release test matrix

| Config | SB state | /boot FS | v0.8.3 | v0.9.0 | v1.0 |
|---|---|---|---|---|---|
| Ubuntu 25.10 homelab | off | ext4 | must pass | must pass | must pass |
| Ubuntu 25.10 SB+MOK | on | ext4 via UKI | must pass | must pass | must pass |
| Ubuntu 25.10 SB+MOK native | on | ext4 raw | documented fail (known limitation) | must pass | must pass |
| Debian 13 SB+MOK native | on | ext4 raw | documented fail | must pass | must pass |
| Fedora 41 SB+MOK native | on | ext4 raw | documented fail | must pass | must pass |
| Proxmox VM zero-touch | on | ext4 raw | documented fail | must pass | must pass |
| openSUSE /boot=btrfs | on | btrfs via UKI | must pass | must pass | must pass |
| Bare metal + ext4 | on | ext4 raw | documented fail | must pass | must pass |

"Documented fail" = `kernel_load_failed` event in trust log + honest limitation in release notes + roadmap link to v1.0 resolution.

## 7. Governance and communication

### 7.1 Public artifacts per release

Each release (v0.8.3, v0.9.0, v0.9.x, v1.0):
- Signed tarball + sha256
- GitHub release notes (from CHANGELOG.md)
- Website release post
- `lamco-admin/projects/lamboot/PUBLICATION-LOG.md` append-only entry
- Announcement posts (optional per release)

### 7.2 Public action gates (from `~/.claude/CLAUDE.md`)

Every public action requires explicit founder approval per release:
- `export-to-public.sh` execution
- `gh release create`
- Website publish
- Announcement posts

No batching approvals across releases.

### 7.3 Private dev repo cadence

Commits to `lamco-admin/lamboot-dev` (private) happen continuously on the main branch, pre-commit hook enforced. Tags are moved/replaced during an unreleased-code window; frozen at release time.

## 8. What changes in `docs/` as this plan unfolds

- `docs/SECURE-BOOT-DEPLOYMENT.md` — amended per release (Config 4 story evolves v0.8.3 → v0.9.x → v1.0)
- `docs/SECURITY-MODEL.md` — "native trust chain" section added in v0.9.x
- `docs/ROADMAP.md` — tracks the v0.9.x → v1.0 arc; older Path G/D notes refactored into current milestone structure
- `docs/ARCHITECTURE.md` — cross-linked to `docs/ARCHITECTURE-LAYERS.md` (authored today); updated per release as new layer-2 backends land
- `docs/analysis/` — this doc + NATIVE-FS-AND-PE-LOADER-STRATEGY + CONFIG-4-TRUST-CHAIN-GAP-AMENDED are the decision record; do not edit after landing (history matters)
- `docs/WEBSITE-CONTENT.md` + corrections docs — re-synced per release; `CHANGED` banner at top of each corrections doc

## 9. Risk register (summarized)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| ext4-view edge-case incompatibility with some distro ext4 | Low (verified default features match) | Medium | Upstream PR or thin adapter workaround |
| PE loader relocation bug corrupts kernel | Low with careful review | High | Exhaustive test corpus + fuzz + code review + QEMU testing before bare-metal |
| ShimLock::Verify rejects Ubuntu kernel in-protocol path | Unknown (never tested directly yet) | High | SDS-3 demands early isolation test; if rejection, Path H (native MOK parser) becomes necessary earlier |
| Scope creep into Btrfs / XFS v1.0 | Medium | Medium | Explicit "v1.1+ community contribution" gate in SDS-5 and ROADMAP |
| Release fatigue on monthly v0.9.x cadence | Medium | Low | Honest release notes; smaller visible changes okay; tests are the release bar |
| Founder approval latency on public actions | Low | Low | Plan communication ahead of each gate; prepare artifacts in advance |

## 10. Immediate next actions (in order)

1. **Write updated `README.md` + `CHANGELOG.md` text** reflecting the honest v0.8.3 posture described in §2.2.
2. **Rebuild + re-sign + re-package** at HEAD; update `PUBLICATION-LOG.md` pre-flight section.
3. **Present release gates** to founder for v0.8.3 public export.
4. **On release complete: begin SDS-1** (`SPEC-FS-BACKEND-TRAIT.md`) immediately.

---

**This plan is binding.** Deviations require founder approval recorded in `PUBLICATION-LOG.md`. The architecture doc (`docs/ARCHITECTURE-LAYERS.md`) is the reviewer's checklist for any PR touching module boundaries. The strategy doc (`docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md`) is the technical justification for why v1.0 looks like this.
