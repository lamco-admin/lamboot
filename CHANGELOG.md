# Changelog

All notable changes to LamBoot are documented here. Format inspired by Keep a Changelog; semantic versioning is loose during pre-1.0.

## [0.8.4] — 2026-04-22

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

## [Unreleased — earlier items carried forward from 2026-04-21]

### Added

- **`lamboot-inspect`** — new diagnostic tool for parsing LamBoot's on-disk artefacts. Stdlib-only Python, six subcommands (`trust-log`, `boot-log`, `summary`, `show`, `verify`, `dump`). Parses the SDS-4 schema-v2 trust log with full validation, renders boot.log with phase timing, produces one-page last-boot summaries, verifies SDS-4 §8.1 website-claims against code-path evidence (CI-ready), and creates diagnostic bundles for bug reports. Includes man page (`lamboot-inspect(1)`), bash + zsh completions, 44-case host test suite, and user guide at `docs/LAMBOOT-INSPECT.md`. Ships in the release tarball alongside `lamboot-install`.

### Changed

- **SDS-7 spec clarified.** `SPEC-LAMBOOT-MIGRATE.md` now correctly frames itself as the v1.0 target for the existing `lamboot-migrate` v0.1.0 tool in `lamco-admin/lamboot-tools-dev`, not a green-field design. Added §14 "Deviations from existing v0.1.0" with a full reconciliation table. **Flipped to RESOLVED status in the v0.8.4 prep section above.**

### Infrastructure

- **Layer-2 FS backend trait** (SDS-1) implemented: `FsBackend` trait with `FatBackend` adapter + `FatStream` streaming, `Volume` dispatch coordinator with 8 MiB LRU cache, `EspWriter` FAT-only-by-construction write path, `Ext4Backend` skeleton (SDS-2 swap-in target). Migrated all 13 consumer files to the new API. Extracted pure value types to `fs_types.rs` for host testing. New `lamboot-fs-tests` host crate with 36 passing tests.

## [0.8.3] — 2026-04-21

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

## [0.2.0] — 2026-03

Pre-release development; not formally tagged. Work tracked in `docs/STATUS-*.md`.
