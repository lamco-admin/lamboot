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

> **Authoritative tracker for v0.9.0 work: see
> [`V0.9.0-RELEASE-PLAN.md`](V0.9.0-RELEASE-PLAN.md).** The tables
> below summarize current state; the plan owns the open punch list.

### Testing Plan Sessions

| Session | Scope | Status |
|---------|-------|--------|
| 1 | Smoke test (fedora-lamboot VM 201) | ✓ Done |
| 2 | Feature testing (F1/F2/F12, navigation, escape) | ✓ Done |
| 3 | Diagnostic modules (diag-shell, pci-inventory) | ✓ Done |
| 4 | Cross-distro (Debian on debway VM 100) | ✓ Done |
| 5 | Install script edge cases | Open — **P-SESS-5** |
| 6 | Feature gating (policy.toml options) | Open — **P-SESS-6** |
| 7 | No-entries and recovery screen | Open — **P-SESS-7** |
| 8 | Toolkit tools (lamboot-diagnose, lamboot-esp, etc.) | ✓ Done (lamboot-tools v0.2.0 ship + this session's deploy/repair gap closure) |
| 9 | vmgenid snapshot detection | Open — **P-SESS-9** |
| 10 | Optimization verification | ✓ Done |

### VM Migration Testing

| VM | OS | Status |
|----|-----|--------|
| 201 (fedora-lamboot) | Fedora 43 | ✓ Installed, tested, working |
| 100 (debway) | Debian forky/sid | ✓ Installed (SB-off); SB+MOK validation open — **P-TM-1** |
| 120 (ubuntu-wayland-test) | Ubuntu 25.10 | ✓ Installed, SB+MOK + native PE loader validated 2026-04-24 |
| 122 (fedora-sway) | Fedora 43 | ✓ Installed (SB-off, native PE) 2026-04-24; SB+MOK validation open — **P-TM-2** |
| 123 (endeavouros-sway) | EndeavourOS | ✓ Installed + recovered + booted 2026-04-25 (lamboot OVMF VARS + canonical ESP layout) |
| 124 (popos-cosmic) | Pop!_OS | Open — **P-TM-5** |
| 104 (fedora-gnome) | Fedora (SeaBIOS) | Deferred — BIOS→UEFI migration test; nice-to-have |
| openSUSE (TBD VMID) | openSUSE Tumbleweed | NEW fixture needed — **P-TM-3** (btrfs UKI test) |
| Bare metal | (any UEFI hardware) | Open — **P-TM-4** |

### Known Blockers

- **Secure Boot chain** — resolved in v0.8.3. LamBoot supports four deployment configurations (SB disabled / firmware db / shim + MOK / custom OVMF VARS). v0.9.x adds native PE loader so the shim-15.8 ShimLock-uninstall failure mode is structurally unreachable for kernel load. See `docs/SECURITY-MODEL.md`.
- **Hookscript config locking** — resolved in v0.8.4 via fw_cfg file-reference pattern (`docs/PROXMOX-INTEGRATION-ROADMAP.md`).
- **Ubuntu kernel hooks** — `/etc/kernel/postinst.d/zz-lamboot` still needed for BLS entry management on Ubuntu. Open — **P-INF-1**.

---

## Pre-Release Requirements (v0.9.0)

> **Authoritative list:** [`V0.9.0-RELEASE-PLAN.md §3`](V0.9.0-RELEASE-PLAN.md).
> Summary below; plan owns the IDs and execution order.

### Must Have (blocks tag)

- [ ] **P-DOC-1/2/3** Doc refresh (this file, CHANGELOG LANDING→LANDED, STATUS supersession)
- [ ] **P-S3-1** SDS-3 §13.3 QEMU harness menu-selection injection
- [ ] **P-TM-1** Debian 13 SB+MOK native ext4 (VM 100 flip)
- [ ] **P-TM-2** Fedora SB+MOK native (VM 122 flip)
- [ ] **P-TM-3** openSUSE /boot=btrfs UKI (NEW fixture)
- [ ] **P-TM-4** Bare metal + ext4 (real hardware OR documented v0.9.1 deferral)
- [ ] **P-REL-1** Tag, sign, package, GitHub release, ledger update

### Should Have (defer to v0.9.1 only with explicit rationale)

- [ ] **P-INF-1** Ubuntu kernel hooks (`/etc/kernel/postinst.d/zz-lamboot`)
- [ ] **P-SESS-5** Test session 5: install script edge cases
- [ ] **P-SESS-6** Test session 6: policy.toml feature gating
- [ ] **P-SESS-7** Test session 7: no-entries recovery screen
- [ ] **P-SESS-9** Test session 9: vmgenid snapshot detection
- [ ] **P-TM-5** VM 124 Pop!_OS migration

### Optional polish

- [ ] **P-S2-1** SDS-2 SHA-256 hashing in `volume_mounted` events (now unblocked by sha2 in SDS-3)

### Nice to Have (can ship without)

- [ ] VM 104 BIOS→UEFI migration test
- [ ] NVMe diagnostic module real SMART implementation
- [ ] Website content and product page (parallel content track, not release-gating)

---

## Toolkit Pivot — coordinated v0.8.4 + v0.2.0 release ✅ COMPLETE 2026-04-23

Both sides shipped 2026-04-23:

- `lamboot v0.8.4` → <https://github.com/lamco-admin/lamboot/releases/tag/v0.8.4>
- `lamboot-tools v0.2.0` → <https://github.com/lamco-admin/lamboot-tools/releases/tag/v0.2.0>

All v0.8.4 must-haves + should-haves landed (hookscript rewrite to
fw_cfg file-reference, fleet.toml schema consumption, `--toolkit-prompt`,
README / LAMBOOT-TOOLS-OVERVIEW rewrite, three doc back-links). Proxmox
integration test on VM 120: **PASS 8/8**
(`docs/analysis/V0.8.4-PROXMOX-INTEGRATION-TEST-2026-04-22.md`).
Full commit log in `docs/CROSS-REPO-STATUS.md §4.1`.

**Rolling cross-repo coordination tracker:**
[`docs/CROSS-REPO-STATUS.md`](CROSS-REPO-STATUS.md). Mirror counterpart
at `~/lamboot-tools-dev/docs/CROSS-REPO-STATUS.md`; keep them in sync.

### Files mirrored at toolkit release-build time

**Canonical source stays in lamboot-dev. Never edit in toolkit:**
- `tools/lamboot-inspect` → mirrored unchanged into `lamboot-tools` core
- `tools/lamboot_inspect/` (Python package dir) → mirrored unchanged
- `tools/lamboot-inspect.1` → mirrored as toolkit man page
- `tools/lamboot-monitor.py` → mirrored into `lamboot-toolkit-pve` as `lamboot-pve-monitor` (renamed)
- `tools/build-ovmf-vars.sh` → mirrored into `lamboot-toolkit-pve` as `lamboot-pve-ovmf-vars` (renamed)

### What's next — v0.9.0 ladder unblocked

The pivot's pause gate cleared with toolkit ship. SDS-2 through SDS-6
implementation is unblocked:

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

## Deferred work

### `pe_loader::load_pe_stream` streaming variant (post-v0.9.0)

SPEC-NATIVE-PE-LOADER.md v1.0 §2.2 defined a streaming `load_pe_stream(Box<dyn FsStream>, ...)` entry point alongside the full-buffer `load_pe(&[u8], ...)`. v1.1 amendment #1 deferred this: goblin's `PE::parse` takes a byte slice not a stream, implementing a from-scratch streaming PE parser is large scope, and typical kernels are 17-18 MB with the largest UKIs at ~60 MB — comfortably inside the 256 MiB full-buffer cap.

**When it returns:** if binaries we need to load ever exceed 256 MiB (hypothetical: kernel + initrd combined UKI for a large server workload), SDS-3 gets an amendment restoring the streaming variant. Until then, the spec line stays in §2.2 marked "deferred — not in v0.9.0" and ROADMAP tracks it here.

**What's needed if restored:**
- Either a streaming PE parser (no goblin) or buffering strategy that reads headers, then sections on demand.
- `FsStream::seek` guarantee (SDS-1 trait addition).
- Test fixture for a genuinely-large PE (one that warrants streaming).

## Key Risks

1. **Secure Boot shim chain + MOK-signed driver loading** — shim chain is implemented in v0.8.3 via Path F (SecurityOverride pattern per systemd-boot precedent). Without this, filesystem drivers cannot load under shim-mediated Secure Boot on Ubuntu/Debian/Fedora. Path G (own PE loader) and Path D (Microsoft shim-review) are the long-term evolutions.

2. **GRUB f2fs incompatibility** — the EfiFs f2fs driver inherits GRUB's extra_attr bug. An independent f2fs driver (Rust, clean-room) would be a significant differentiator but is a large effort.

3. **aarch64 untested** — builds but no ARM VM available for boot testing.

4. **Installer integration timeline** — distro adoption is measured in release cycles (6-12 months). Upstream engagement needed early.

5. **Microsoft UEFI CA 2011 expires June 2026** — affects all Secure Boot signing. The shim path is unaffected but needs to be implemented before this deadline.
