# LamBoot v0.8.3 Implementation Plan

**Date:** 2026-04-21 (revised — adopted deep-dive security strategy)
**Purpose:** Implementation roadmap with clear release-blocking vs non-blocking classification

> **See also:** [`SECURITY-MODEL.md`](SECURITY-MODEL.md) (user-facing threat model) and [`analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md`](analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md) (architecture + bypasses catalog + differentiators). v0.9.x+ security direction (Path G, Trust Evidence Panel, Microsoft shim-review) is documented in [`ROADMAP.md`](ROADMAP.md).

---

## Release Definition

**LamBoot v0.8.3 release** = the bootloader binary + install script + drivers + modules + documentation that a user downloads and installs. Must work end-to-end on systems with and without Secure Boot.

**lamboot-tools** = companion toolkit (separate repo, separate release). Enhances LamBoot but is NOT required for LamBoot to function. Never blocks a LamBoot release.

---

## Release-Blocking (must be done before v0.8.3 ships)

### 1. Secure Boot: Shim Chain Deployment

**What:** When Secure Boot is enabled, `lamboot-install` must deploy LamBoot through the distro's shim so MOK-enrolled keys are checked.

**Implementation:**
- Detect Secure Boot state during install (`/sys/firmware/efi/efivars/SecureBoot-*`)
- If SB enabled: copy distro shim to `\EFI\LamBoot\shimx64.efi`, deploy signed LamBoot as `\EFI\LamBoot\grubx64.efi` (shim's expected loader name), create boot entry pointing to `\EFI\LamBoot\shimx64.efi`
- If SB disabled: deploy unsigned LamBoot as `\EFI\LamBoot\lambootx64.efi`, create boot entry pointing directly to it
- New install script flag: `--signed` to use pre-signed binaries
- Distro shim discovery: search `\EFI\ubuntu\shimx64.efi`, `\EFI\fedora\shimx64.efi`, `\EFI\debian\shimx64.efi`, etc.

**Files:** `tools/lamboot-install` (modify Phase 6)

**Validates:** VM 120 (Ubuntu, Secure Boot enabled)

### 2. Secure Boot: Custom OVMF VARS for Proxmox

**What:** Build an OVMF VARS template with LamBoot's signing key in the firmware db, enabling zero-touch Secure Boot deployment on Proxmox VM fleets.

**Implementation:**
- Script that uses `qemu-ovmf-secureboot` or manual `EnrollDefaultKeys.efi` + db enrollment to produce a VARS file
- VARS file includes: Microsoft keys (for Windows compatibility) + LamBoot signing key in db
- Document how to use this VARS file with Proxmox (`qm set VMID --efidisk0 ...`)
- Ship the VARS file in the release tarball

**Files:** New script `tools/build-ovmf-vars.sh`, output `dist/OVMF_VARS_lamboot.fd`

**Validates:** VM 120 with custom VARS (direct boot, no shim)

### 3. Ubuntu/Debian Kernel Hooks

**What:** Create BLS entries automatically when kernels are installed/removed on Debian/Ubuntu systems.

**Implementation:**
- Core tool: `lamboot-kernel-hook` — standalone BLS entry generator
- Wrapper: `/etc/kernel/postinst.d/zz-lamboot` — calls `lamboot-kernel-hook add`
- Wrapper: `/etc/kernel/postrm.d/zz-lamboot` — calls `lamboot-kernel-hook remove`
- `zz-` prefix ensures it runs AFTER initramfs generation
- Deduplication: check if BLS entry already exists (in case kernel-install also ran)
- `lamboot-install` detects hook system and installs appropriate wrappers

**Files:** New `tools/lamboot-kernel-hook`, new `dist/kernel-hooks/zz-lamboot-postinst`, new `dist/kernel-hooks/zz-lamboot-postrm`, modify `tools/lamboot-install` (Phase 7)

**Validates:** Install new kernel on Ubuntu VM 120, verify BLS entry created automatically

### 4. Migration Tier 1: Preserve and Supplement (default)

**What:** Install LamBoot alongside existing bootloader without disrupting it.

**Implementation:**
- Install LamBoot binary, drivers, modules, BLS entries
- Create UEFI boot entry but NOT as first in boot order (append to end)
- Do NOT install as `\EFI\BOOT\BOOTX64.EFI`
- Generate BLS entries from existing kernels (reuse systemd-boot/Fedora entries if present)
- Install kernel hooks
- Old bootloader completely untouched
- This is the current `lamboot-install` default behavior (already mostly works)

**Files:** `tools/lamboot-install` (verify default behavior matches Tier 1)

### 5. Migration Tier 2: Backup + Replace with Fallback

**What:** Replace existing bootloader with LamBoot, with automated fallback.

**Implementation:**
- New flag: `lamboot-install --replace`
- Full ESP backup: `tar czf /root/lamboot-migration-backup-{date}.tar.gz /boot/efi/`
- NVRAM dump: `efibootmgr -v > /root/lamboot-migration-nvram-{date}.txt`
- Extract GRUB params: parse `/etc/default/grub` → `/etc/kernel/cmdline`
- Set LamBoot as first in boot order
- Keep old bootloader as second entry (automatic UEFI fallback)
- Old bootloader appears as chainload entry in LamBoot menu

**Files:** `tools/lamboot-install` (new `--replace` flag)

### 6. Testing Sessions 5 + 7

**What:** Validate install script edge cases and no-entries recovery.

- Session 5: `--remove`, `--update`, binary validation, boot order, config preservation
- Session 7: Hide all entries, verify recovery screen, F2/F12 from recovery

**Files:** No code changes — testing only

### 7. Release Packaging

**What:** Create distributable tarball.

**Contents:**
```
lamboot-0.8.3-x86_64/
├── lamboot-install              # Install script
├── lambootx64.efi               # Unsigned binary
├── lambootx64-signed.efi        # Signed binary
├── drivers/
│   ├── ext4_x64.efi
│   ├── btrfs_x64.efi
│   ├── f2fs_x64.efi
│   ├── xfs_x64.efi
│   ├── zfs_x64.efi
│   ├── ntfs_x64.efi
│   ├── ext2_x64.efi
│   └── iso9660_x64.efi
├── modules/
│   ├── manifest.toml
│   ├── diag-shell.efi
│   ├── pci-inventory.efi
│   ├── mem-quick.efi
│   └── nvme-diag.efi
├── policy.toml
├── keys/
│   └── db.der                   # Signing certificate for enrollment
├── systemd/
│   └── lamboot-mark-success.service
├── kernel-install/
│   └── 90-lamboot.install
├── kernel-hooks/
│   ├── zz-lamboot-postinst
│   └── zz-lamboot-postrm
├── OVMF_VARS_lamboot.fd         # Pre-enrolled OVMF VARS for Proxmox
└── docs/
    └── (all documentation)
```

---

## Non-Blocking (lamboot-tools — separate release, separate repo)

These enhance LamBoot but do NOT block the v0.8.3 release:

### lamboot-tools items

| Item | Tool | Status |
|------|------|--------|
| Proxmox host-side deployment | `lamboot-proxmox-deploy` (new) | Not started |
| Hookscript fw_cfg rewrite | `lamboot-hookscript.pl` | Documented, not implemented |
| Fleet management | `lamboot-fleet` (new) | Not started |
| VMID injection automation | hookscript | fw_cfg pattern documented |
| lamboot-diagnose testing | Session 8 | Not started |
| lamboot-esp testing | Session 8 | Not started |
| lamboot-backup testing | Session 8 | Not started |
| lamboot-repair testing | Session 8 | Not started |
| lamboot-migrate testing | Session 8 | Not started |

### Post-release improvements

| Item | Status |
|------|--------|
| vmgenid snapshot testing (Session 9) | Not started |
| Migration Tiers 3-4 | Designed, not implemented |
| XBOOTLDR partition scanning | Disabled, needs partition type filter |
| NVMe diagnostic module | Stub only |
| Independent f2fs driver (Rust, clean-room) | Research complete |
| Website content | Not started |
| archinstall/Calamares integration | Research complete |
| Custom shim via rhboot/shim-review | Future |

---

## Implementation Order

| Step | Item | Status |
|------|------|--------|
| 1 | Ubuntu kernel hooks (postinst.d) | ✓ Done |
| 2 | Shim chain deployment in install script | ✓ Done |
| 3 | Custom OVMF VARS build script | ✓ Done |
| 4 | Migration Tier 1 verification | ✓ Done |
| 5 | Migration Tier 2 (--replace) | ✓ Done |
| 6 | VM 120 Config 3 install + MOK enrollment validation | ✓ Done (boot loads; drivers blocked pending Path F) |
| 7 | Production signing keys (PK 4096, KEK 4096, db 2048) | ✓ Done |
| 8 | `build.sh` signing pipeline (llvm-objcopy SBAT + sbsign) | ✓ Done |
| 9 | Local QEMU SB smoke test | ✓ Done |
| 10 | `--signed` / `--no-shim` / `--no-mok` install flags | ✓ Done |
| 11 | MOK enrollment guide + OVMF VARS Proxmox guide + deployment guide | ✓ Done |
| **12** | **Path F — SecurityOverride for shim+MOK driver loading** | **In progress** — unblocks Config 3 FS drivers on all distros |
| **13** | **UKI first-class menu integration** (extend `uki.rs`, BLS-style `.efi` discovery from `\EFI\Linux\`) | **New for 0.8.3** |
| 14 | Fix `boot.rs:162` `find_volume_for_path` logging bug | Pending (15 min) |
| 15 | VM 120 Config 3 re-test with Path F (full end-to-end boot) | Pending |
| 16 | VM 120 Config 4 test (custom OVMF VARS, zero-touch) | Pending |
| 17 | Cross-distro Config 3 validation (Debian, Fedora) | Pending |
| 18 | Testing Session 5 (install edge cases) | Pending — needs VM |
| 19 | Testing Session 7 (no-entries recovery) | Pending — needs VM |
| 20 | Public repo reservation (`lamco-admin/lamboot`, `lamboot-tools`) | Pending |
| 21 | `export-to-public.sh` skeleton | Pending |
| 22 | Release tarball packaging | Pending (after 15-19) |

## v0.8.3 Additions (adopted from deep-dive 2026-04-21)

| Step | Item | Status |
|------|------|--------|
| A | SECURITY.md + docs/analysis/README.md + CHANGELOG.md | ✓ Done |
| B | docs/SECURITY-MODEL.md — honest threat model | ✓ Done |
| C | Trust-evidence log (`lamboot-core/src/trust_log.rs` → `\loader\boot-trust.log`) | ✓ Done |
| D | Public repo reservation (`lamco-admin/lamboot`, `lamboot-tools`) | ✓ Done |
| E | Install script BLS lifecycle (kernel-hooks packaging + stale-entry regeneration) | ✓ Done |
| F | export-to-public.sh skeleton | Pending |
| G | Cross-distro Config 3 validation (Debian + Fedora VMs) | Pending |
| H | Testing Sessions 5 + 7 | Pending |
| I | Release tarball packaging (final gate) | Pending — fires last |

---

## Validation Criteria for Release

- [ ] LamBoot boots on Fedora (SB off) — ✓ already validated
- [ ] LamBoot boots on Debian (SB off) — ✓ already validated
- [x] LamBoot itself boots on Ubuntu with Secure Boot via shim chain — ✓ validated on VM 120 (Task #9)
- [ ] LamBoot loads signed FS drivers under shim+MOK via Path F (Task #12)
- [ ] LamBoot boots a BLS kernel end-to-end on Ubuntu with SB+shim+MOK
- [ ] LamBoot boots on Proxmox VM with custom OVMF VARS (direct db, Task #16)
- [ ] UKI entries appear in the menu alongside BLS entries (Task #13)
- [ ] UKI boots correctly under SB with Path F active (Task #13 + #15)
- [ ] `lamboot-install` works on Fedora, Debian, Ubuntu
- [ ] `lamboot-install --remove` cleanly removes everything
- [ ] `lamboot-install --update` preserves config
- [ ] `lamboot-install --replace` backs up and migrates from GRUB
- [ ] New kernel install on Ubuntu creates BLS entry automatically
- [ ] New kernel install on Fedora creates BLS entry automatically (kernel-install)
- [ ] New UKI installed to `\EFI\Linux\` appears in menu without config
- [ ] No-entries recovery screen shows F2/F12 options
- [ ] Release tarball installs on a fresh system
- [ ] All documentation accurate and current
