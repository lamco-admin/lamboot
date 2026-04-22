# Changelog

All notable changes to LamBoot are documented here. Format inspired by Keep a Changelog; semantic versioning is loose during pre-1.0.

## [0.8.3] — 2026-04 (unreleased)

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
