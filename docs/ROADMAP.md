# LamBoot Roadmap

**Updated:** 2026-04-05 (evening)

---

## Completed

### Phase A — Essential Usability ✓

BLS Type 1 parser, boot counting, UAPI.10 sorting, graphical boot menu, filesystem drivers (ext4/btrfs), preflight validation, crash loop detection, kernel-install plugin, systemd mark-success service.

### Phase B — Proxmox Diagnostic Value ✓

PCI inventory module, diagnostic shell (5 screens), memory quick test, NVMe diagnostic (stub), module manifest, loadable module architecture.

### Phase C — Proxmox Integration & Hardening ✓

ACPI DMAR/IVRS parser, hypervisor detection (CPUID), SMBIOS reading (Type 1/11), fw_cfg data channel, boot telemetry, TPM measured boot (PCR 4/5/12), Proxmox hookscript, Secure Boot signing pipeline, enhanced lamboot-monitor.

### Session 1–3 Testing + Bug Fixes ✓

11 bugs found and fixed: write_file truncation, MZ byte order, UKI full-read OOM, extra volume hang, GraphicsConsole disconnect, PCI exclusive protocol, Escape key, module return-to-menu, SMBIOS stub replacement, efivarfs pipefail/SIGPIPE, preflight ESP-only search.

GUI redesign: two-column layout, VMID display, persistent boot log.

### Session 4 — Cross-Distro (Debian) ✓

LamBoot installed on debway (Debian forky/sid) via install script. 4 BLS entries generated for 4 kernels. ext4 driver auto-detected and installed. Boots successfully. Preflight fixed to search all volumes for kernel/initrd files (targeted `exists()` lookups — no directory enumeration on large filesystems).

### Filesystem Expansion ✓

Added f2fs, xfs, zfs, ntfs drivers (EfiFs v1.12) for x86_64 and aarch64. Superblock probing added for f2fs (magic 0xF2F52010), xfs ("XFSB"), zfs (0x00BAB10C). Install script updated for all new filesystem types. Total: 8 filesystem drivers.

### Documentation ✓

13 documents across two repos:

**lamboot-dev (11 docs):**
- User Guide, Configuration Guide, Proxmox Guide, Troubleshooting Guide
- Architecture Guide, Install Reference, Security Guide, Diagnostic Modules
- LamBoot Tools Overview, Roadmap
- Secure Boot & Signing Strategy
- Secure Boot Blocker (shim chain issue)
- Cross-Platform Integration Report (10+ platforms)
- Filesystem Driver Analysis (bcachefs, ReFS, Windows trajectory)
- F2FS Boot Ecosystem Analysis (GRUB breakage, CachyOS, bootloader matrix)

**lamboot-tools-dev (2 docs):**
- README with full CLI reference + Makefile
- Proxmox Integration Roadmap (5 phases)

### Research Completed ✓

- Cross-platform integration (AWS, Azure, GCE, VMware, Hyper-V, libvirt, Firecracker, Cloud Hypervisor)
- Secure Boot signing options (MOK, custom shim, Microsoft CA, OVMF pre-enrollment)
- Ubuntu GRUB customizations and integration requirements
- UEFI Secure Boot key hierarchy and security levels
- Proxmox host-to-guest data injection mechanisms (fw_cfg file-reference pattern)
- Filesystem driver feasibility (bcachefs, ReFS, APFS)
- Windows filesystem trajectory (ReFS successor timeline, ESP stays FAT32)
- f2fs boot ecosystem (GRUB breakage categories, distro status, CachyOS)

---

## Current — Testing & Validation

### Testing Plan Sessions

| Session | Scope | Status |
|---------|-------|--------|
| 1 | Smoke test (fedora-lamboot VM 201) | ✓ Done |
| 2 | Feature testing (F1/F2/F12, navigation, escape) | ✓ Done |
| 3 | Diagnostic modules (diag-shell, pci-inventory) | ✓ Done |
| 4 | Cross-distro (Debian on debway VM 100) | ✓ Done |
| 5 | Install script edge cases | Not started |
| 6 | Feature gating (policy.toml options) | Not started |
| 7 | No-entries and recovery screen | Not started |
| 8 | Toolkit tools (lamboot-diagnose, lamboot-esp, etc.) | Not started |
| 9 | vmgenid snapshot detection | Not started |
| 10 | Optimization verification | ✓ Done |

### VM Migration Testing

| VM | OS | Status |
|----|-----|--------|
| 201 (fedora-lamboot) | Fedora 43 | ✓ Installed, tested, working |
| 100 (debway) | Debian forky/sid | ✓ Installed, tested, working |
| 120 (ubuntu-wayland-test) | Ubuntu 25.10 | Installed, **blocked on Secure Boot** (shim chain needed) |
| 122 (fedora-sway) | Fedora | Not started |
| 123 (endeavouros-sway) | EndeavourOS | Not started |
| 124 (popos-cosmic) | Pop!_OS | Not started |
| 104 (fedora-gnome) | Fedora (SeaBIOS) | Not started — requires BIOS→UEFI migration |

### Known Blockers

- **Secure Boot chain (resolved in v0.8.3)** — LamBoot now supports four deployment configurations (SB disabled / firmware db / shim + MOK / custom OVMF VARS) with signed binaries, Path F SecurityArchProtocol override for driver loading, ShimRetainProtocol for shim 15.8+, and first-class UKI discovery. See `docs/SECURITY-MODEL.md` for the honest trust model and `docs/analysis/` for the underlying architecture research.
- **Hookscript config locking** — current hookscript calls `qm set` during pre-start which fails due to Proxmox config lock. Fix: fw_cfg file-reference pattern (documented in Proxmox Integration Roadmap).
- **Ubuntu kernel hooks** — `/etc/kernel/postinst.d/zz-lamboot` needed for BLS entry management on Ubuntu. Not implemented.

---

## Pre-Release Requirements

### Must Have (blocks release)

- [ ] Shim chain loading for Secure Boot systems
- [ ] Install script `--signed` flag to deploy signed binaries through shim
- [ ] Session 5 testing (install script edge cases: remove, update, binary validation)
- [ ] Session 7 testing (no-entries recovery screen)
- [ ] Release packaging (tarball with binary + drivers + modules + installer + policy)

### Should Have (important but not blocking)

- [ ] Ubuntu kernel hooks (`/etc/kernel/postinst.d/zz-lamboot`)
- [ ] Session 6 testing (policy.toml feature gating)
- [ ] Session 9 testing (vmgenid snapshot detection)
- [ ] VM migrations on 122, 123, 124 (cross-distro validation)

### Nice to Have (can ship without)

- [ ] VM 104 BIOS→UEFI migration test
- [ ] NVMe diagnostic module (currently stub)
- [ ] Website content and product page

---

## Toolkit Pivot — coordinated v0.8.4 release with lamboot-tools v0.2.0

**Added 2026-04-22.** Per `docs/STATUS-2026-04-22-TOOLKIT-PIVOT.md` (note §9
post-Q appendix) and the authoritative toolkit spec at
`~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md`, further bootloader
implementation work (SDS-2 through SDS-6) is paused while the `lamboot-tools`
toolkit reaches its v0.2.0 publishable milestone.

**Toolkit-side state (2026-04-22 end-of-day):** `lamboot-tools v0.2.0` is
feature-complete and claim-accurate. Sessions A–Q landed 11 tools, unified
RPM spec (three subpackages from one source per Option 2), 13 man pages, 24
website pages, 84-check `verify-claims.sh`, 28/28 release-rehearsal, 11
Proxmox fixture images. Five founder-gated items remain on the toolkit side
(self-hosted runner, Tier 1 baseline, this repo's v0.8.4 coordination path
choice — **resolved: Path A**, release-runbook execution, optional public
fixture hosting).

**Rolling cross-repo coordination tracker:**
[`docs/CROSS-REPO-STATUS.md`](CROSS-REPO-STATUS.md). Mirror counterpart lives
at `~/lamboot-tools-dev/docs/CROSS-REPO-STATUS.md`; keep them in sync.

### What lamboot-dev owes the toolkit's v0.2.0 release

Cross-repo coordination per SPEC-LAMBOOT-TOOLKIT-V1.md §14.3. All items ship in a coordinated v0.8.4 release alongside toolkit v0.2.0:

**Must have (blocks coordinated release):**
- [ ] **Hookscript rewrite** — `tools/lamboot-hookscript.pl` must use the fw_cfg file-reference pattern per `~/lamboot-tools-dev/docs/PROXMOX-INTEGRATION-ROADMAP.md` Phase 1. Current implementation calls `qm set` mid-pre-start which config-locks. Blocks `lamboot-pve-setup` from functioning. Hookscript reads `/etc/lamboot/fleet.toml` per shared schema in `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md` Appendix C.
- [ ] **`/etc/lamboot/fleet.toml` consumption** — hookscript + `lamboot-monitor.py` read settings per the `[hookscript]` and `[monitor]` sections of the shared schema.
- [ ] **`lamboot-install` opt-in toolkit prompt** — adds `Install lamboot-tools for diagnostic and repair utilities? [y/N]` to the interactive path, `--install-toolkit` / `--no-install-toolkit` flag forms. Prompt occurs after successful install.
- [ ] **README / USER-GUIDE cross-reference** — adds toolkit recommendation and link. Non-functional but brand-coherent.

**Should have:**
- [ ] **`SECURE-BOOT-AND-SIGNING-STRATEGY.md` cross-reference** — `lamboot-signing-keys` tool Scope 1 mode invokes procedures documented here. Add back-link to the toolkit's tool.
- [ ] **`KEY-GENERATION.md` cross-reference** — same; authoritative doc for the key-management procedure `lamboot-signing-keys` consumes.
- [ ] **`OVMF-VARS-PROXMOX.md` cross-reference** — `lamboot-pve-ovmf-vars` is the mirror of `build-ovmf-vars.sh` for tooling; doc remains authoritative.

**Release coordination:**
- [ ] Combined release announcement covers lamboot v0.8.4 bootloader changes + lamboot-tools v0.2.0 + lamboot-toolkit-pve v0.2.0
- [ ] Release notes cross-link between repos

### Files mirrored at toolkit release-build time

**Canonical source stays in lamboot-dev. Never edit in toolkit:**
- `tools/lamboot-inspect` → mirrored unchanged into `lamboot-tools` core
- `tools/lamboot_inspect/` (Python package dir) → mirrored unchanged
- `tools/lamboot-inspect.1` → mirrored as toolkit man page
- `tools/lamboot-monitor.py` → mirrored into `lamboot-toolkit-pve` as `lamboot-pve-monitor` (renamed)
- `tools/build-ovmf-vars.sh` → mirrored into `lamboot-toolkit-pve` as `lamboot-pve-ovmf-vars` (renamed)

### What's paused while the toolkit ships

- SDS-2 (ext4-view integration) implementation
- SDS-3 (native PE loader) implementation
- SDS-4 (native trust chain) implementation
- SDS-5 (BLS multi-FS discovery) implementation
- SDS-6 (UEFI FS driver deprecation) implementation
- v0.9.0 release

Resumes after toolkit v0.2.0 ships. Per-SDS implementation order re-evaluated at that point.

---

## Next — Publishing & Content

### Website Content

- Adapt documentation for lamco.io website
- Product page with feature overview, screenshots, comparison matrix
- Download/install instructions for release binaries
- Blog post / announcement for initial release

### Release Packaging

- Binary release tarball (lambootx64.efi + drivers + modules + installer + policy)
- lamboot-tools tarball (5 bash tools + Makefile)
- GitHub release with changelog
- Versioned documentation snapshots

---

## Future — Distribution Installer Integration

### Research Required

How Linux distribution installers handle bootloader selection and installation. This is the path to native adoption — LamBoot installed at OS install time rather than retrofitted.

| Installer | Distros | Bootloader mechanism | Integration path |
|-----------|---------|---------------------|-----------------|
| **Anaconda** | Fedora, RHEL, CentOS | Blivet storage module selects GRUB/systemd-boot | Plugin or patch to offer LamBoot as an option |
| **Calamares** | Manjaro, EndeavourOS, CachyOS, many others | `bootloader` module with GRUB/systemd-boot backends | Custom module in `modules/bootloader/` |
| **d-i** (debian-installer) | Debian | `grub-installer` udeb package | New `lamboot-installer` udeb |
| **Subiquity** | Ubuntu Server | Snap-based, uses curtin for bootloader | curtin hook or plugin |
| **archinstall** | Arch Linux | Python module, supports GRUB/systemd-boot/limine | `LamBoot` class implementing `AbstractBootloader` |
| **YaST** | openSUSE, SLES | Ruby-based, `Bootloader` module | New LamBoot backend |

### Practical Steps

1. **Start with archinstall** — Python, simple architecture, Arch users are early adopters
2. **Then Calamares** — widest reach across distros (including CachyOS), modular design
3. **Document the integration API** — what an installer needs to call (`lamboot-install` flags)
4. **Package for distro repos** — AUR (Arch), COPR (Fedora), PPA (Ubuntu) as prerequisites

---

## Future — Proxmox Integration

See [Proxmox Integration Roadmap](https://github.com/lamco-admin/lamboot-tools-dev/blob/main/docs/PROXMOX-INTEGRATION-ROADMAP.md) for 5-phase plan:

1. Basic fix (lamboot-setup, hookscript fw_cfg rewrite)
2. Fleet management tooling
3. Native Proxmox option (like `spice_enhancements`)
4. Web UI dashboard
5. Advanced (auto-rollback, RDP integration)

---

## Future — Advanced Features (Phase D)

| Feature | Priority | Notes |
|---------|----------|-------|
| WASM diagnostic runtime | Research complete | wasmi-based, allows safe third-party diagnostics |
| Network agent | Low | HTTP /status and /entries endpoints for remote management |
| Theme system | Low | Custom color schemes beyond Catppuccin Mocha |
| Proxmox web UI plugin | Low | Boot health dashboard in PVE web interface |
| TPM attestation report | Low | Remote attestation for measured boot verification |
| NVMe diagnostic module | Medium | Implement actual NVMe SMART data reading (currently stub) |
| Independent f2fs driver | Medium | Clean-room Rust implementation with extra_attr + compression support |

---

## Secure Boot Architecture — Long-Term Paths

Path F (SecurityOverride) is being implemented for v0.8.3 — see `docs/analysis/SECURE-BOOT-ECOSYSTEM-AND-REAL-FIX-2026-04-21.md`. Two further paths sit beyond that release:

### Path G — Custom PE Loader (v0.9.x target)

**Goal:** replace LamBoot's reliance on `BS->LoadImage` with an in-process Rust PE loader that parses, allocates, relocates, and invokes EFI images after ShimLock::Verify confirms trust.

**Why:** eliminates the SecurityOverride hack entirely. Matches systemd-boot's stated long-term direction (their `src/boot/secure-boot.c` comment: *"this can be dropped once shim < v16 is no longer supported"*). Reduces attack surface by removing firmware-protocol mutation. Would be the **first production Rust PE-loader-for-UEFI** in the OSS ecosystem — candidate for upstream contribution to `uefi-rs`.

**Scope:** PE32+ header parsing, EfiLoaderCode page allocation, section loading with relocations, EFI_LOADED_IMAGE protocol installation, entry-point invocation, TPM measurement integration.

**Effort:** ~1-2 weeks focused Rust + security review. `unsafe` density audited carefully.

**Success criteria:** remove `install_security_override` calls from `drivers.rs` and `boot.rs`; full SB+MOK functionality unchanged; measured benefit in TPM event granularity and structured error reporting.

### Path D — Microsoft shim-review signing (v1.0+ target)

**Goal:** submit a LamBoot-specific shim binary to `rhboot/shim-review` for Microsoft UEFI CA signing. Once accepted, Microsoft signs the LamBoot shim; firmware on every modern system trusts it by default. Users no longer need MOK enrollment for LamBoot itself.

**Why:** broadens LamBoot's trust baseline to Microsoft-signed level. Removes the MokManager reboot dance for most deployments. Positions LamBoot as a first-class peer to distro shims (Ubuntu, Debian, Fedora) rather than a MOK-trusted add-on. **First Rust bootloader accepted through shim-review** would be an industry first.

**Prerequisites:**
- Reproducible build pipeline (Cargo.lock pinning, deterministic build environment, tightly managed SBAT metadata)
- Public audit trail (git history, release notes, signed commits)
- SBAT entries registered and maintained across releases
- Documented security threat model
- Multiple committed maintainer contacts
- Update and key-rotation processes documented
- CVE response capability demonstrated

**Scope:** administrative + sustained engineering. shim-review iteration cycle is 3-6 months. Requires a build host Microsoft trusts (reproducible, deterministic). Ongoing compliance: annual re-signing, SBAT level updates, CVE response.

**Strategic value:** adds Config 5 (Microsoft-signed direct) to the existing Config 1 (SB off) / Config 3 (shim+MOK) / Config 4 (custom OVMF VARS) deployment matrix. Config 5 is the only path that removes user action entirely on SB-enforcing systems — the universal-deploy endgame.

**Success criteria:** LamBoot shim accepted through shim-review; signed binary distributed in release tarball; user-facing install procedure drops MOK enrollment on Config 5 systems; CVE response plan tested against at least one drill.

---

## Upstream Contributions

| Project | Contribution | Status |
|---------|-------------|--------|
| virt-firmware (kernel-bootcfg) | Add `--add-bls` for non-UKI entries | Not started |
| efivar-rs | Offline OVMF variable support | Not started |
| archinstall | LamBoot bootloader backend | Research needed |
| Calamares | LamBoot bootloader module | Research needed |
| uefi-rs | PE loader module (byproduct of Path G) | Research needed — v0.9.x+ |
| rhboot/shim-review | LamBoot shim submission (Path D) | Prerequisites in progress — v1.0+ |

---

## Key Risks

1. **Secure Boot shim chain + MOK-signed driver loading** — shim chain is implemented in v0.8.3 via Path F (SecurityOverride pattern per systemd-boot precedent). Without this, filesystem drivers cannot load under shim-mediated Secure Boot on Ubuntu/Debian/Fedora. Path G (own PE loader) and Path D (Microsoft shim-review) are the long-term evolutions.

2. **GRUB f2fs incompatibility** — the EfiFs f2fs driver inherits GRUB's extra_attr bug. An independent f2fs driver (Rust, clean-room) would be a significant differentiator but is a large effort.

3. **aarch64 untested** — builds but no ARM VM available for boot testing.

4. **Installer integration timeline** — distro adoption is measured in release cycles (6-12 months). Upstream engagement needed early.

5. **Microsoft UEFI CA 2011 expires June 2026** — affects all Secure Boot signing. The shim path is unaffected but needs to be implemented before this deadline.
