# LamBoot v0.8.3 Fleet Install Test — Plan

**Date:** 2026-04-21
**Supersedes:** pre-release gate task #41 (bare-metal SB — not possible in this environment)
**Goal:** validate `lamboot-install` runs cleanly across a diverse distro fleet. Catches distro-detection gaps, packaging assumptions, path conventions, and install-script regressions that single-VM testing misses.

---

## 1. Test fleet (distro targets, from pve cluster inventory)

Filtered to distro-representative VMs (excludes infra / service VMs). 18 candidates across the major Linux distro families + compositors.

### Excluded — already validated in dedicated tasks

| VMID | Name | Validation |
|---|---|---|
| 100 | debway | PASS #39 (Debian kernel-hook cycle) |
| 120 | ubuntu-wayland-test | PASS (earlier session, Ubuntu 25.10) |
| 201 | fedora-lamboot | PASS #38 (Fedora kernel-install plugin) |

### Revised — reality is much smaller OVMF fleet

After cluster-wide audit (discovery 2026-04-21), only these VMs actually have `bios: ovmf` + `efidisk0`:

- `100 debway` (already PASS #39)
- `120 ubuntu-wayland-test` (already PASS earlier session)
- `201 fedora-lamboot` (already PASS #38)
- `113 mark`, `122 fedora-sway`, `123 endeavouros-sway`, `124 popos-cosmic` — stopped, distro-testable
- (infra VMs excluded: aibox / pgdbsrv / ainode / toom / inferer)

Every other "Linux" VM in the fleet (`104 fedora-gnome`, `110 RHEL10`, `102 openSuseTumbleweed`, `130 osuse16-leap`, `105 arch-niri`, `131 arch-gnome`, `126 f44-beta-kde`, `129 ubu25-gnome`, `119 manjaro-kde`, `125 debian13-lxqt-wayfire`, `128 deb13-gnome`, `114 solus-hypr`, `118 end-budgie1010`, `121 ubuntu-24.04-sway`) is `bios: seabios` (legacy BIOS), not UEFI. LamBoot is UEFI-only, so these cannot be meaningfully tested without reconfiguring the VMs to OVMF firmware (not done as part of v0.8.3 release gate — not LamBoot's bug).

### Actual primary wave (fleet install test #44) — 4 VMs

| Order | VMID | Name | Status | Distro |
|---|---|---|---|---|
| 1 | 113 | mark | stopped | (check on boot) |
| 2 | 122 | fedora-sway | stopped | Fedora + Sway |
| 3 | 123 | endeavouros-sway | stopped | EndeavourOS + Sway |
| 4 | 124 | popos-cosmic | stopped | Pop!_OS COSMIC |

Per-VM workflow: start → dist-upgrade if needed → deploy dist/ → `lamboot-install` → 10-point checklist → `--remove` → shut down.

### Already-validated (from prior tasks)

- #100 debway (Debian forky/sid) — task #39 PASS
- #120 ubuntu-wayland-test (Ubuntu 25.10) — earlier session PASS
- #201 fedora-lamboot (Fedora 43) — task #38 PASS

### Not testable as UEFI in current fleet state

14 VMs are configured BIOS/SeaBIOS. Not LamBoot's problem; would require reconfiguring each VM's firmware to OVMF (separate effort). Documented here so future fleet expansion picks these up if/when they're converted.

### Explicit exclusions

| VMID | Reason |
|---|---|
| 166 RHEL97-gnome-tester | RHEL 9.7 glibc too old |
| 127 pdm | Proxmox Datacenter Manager — infra |
| 150, 155, 200, 350 | Service VMs |
| 101 archie | Sparse system |
| 115 | template, not bootable |
| 360 cachyos-budgie | ovmf but no efidisk0 — misconfigured |

## 2. What's tested per VM

On each VM, running through this checklist:

| # | Check | PASS criterion |
|---|---|---|
| 1 | Install from current `dist/` tree + `tools/lamboot-install` | Exit code 0; all Phase 1–8 steps pass or produce only known-acceptable warnings |
| 2 | Distro detection (`DISTRO_ID`) | Matches `/etc/os-release` `ID=` |
| 3 | ESP mount detection | Correct mount point identified |
| 4 | Kernel discovery | All installed kernels found under `/boot` |
| 5 | BLS entry generation (or detection if distro-native) | For BLS-generating distros: one .conf per kernel, well-formed. For Fedora: existing native entries respected. |
| 6 | Filesystem driver install | `ext4_x64.efi` (or appropriate) copied to `\EFI\LamBoot\drivers\` |
| 7 | UEFI boot entry creation | `Boot000X LamBoot` present in `efibootmgr` output |
| 8 | Default boot order preference | LamBoot first in BootOrder (per v0.8.3 new default) |
| 9 | Kernel-hook helper deploy | `/usr/local/bin/lamboot-kernel-hook` present (matters for Debian/Ubuntu; 633e5f2 fix) |
| 10 | Clean `--remove` | Subsequent `lamboot-install --remove` cleans everything without error |

## 3. What is NOT tested per VM

- **Actual reboot into LamBoot.** Known v0.8.3 limitation on SB + ext4 /boot (shim 15.8 issue). The fleet test validates *installation*, which is what the v0.8.3 release actually ships. Boot is a v1.0 goal.
- **Kernel-hook cycle** (full apt/dnf install). Only tested on one Debian (#39) and one Fedora (#38) — the test matrix intent is distro detection breadth, not redundantly exercising the same hook path.
- **MokManager enrollment.** Only tested on VMs where SB is on *and* shim+MOK is the target. Most fleet VMs have SB off.

## 4. Execution plan

Serial, one VM at a time. No parallelism — avoids confounding failures across VMs and respects the "never switch VMs mid-test" rule.

For each VM:
1. `ssh <vm>` or via pve1 if direct access fails (direct fix if unreachable, don't skip)
2. `scp` tools/lamboot-install + tools/lamboot-kernel-hook + dist/ tree → `/tmp/lamboot-deploy/`
3. Run 10-point check, capture output, append to `docs/FLEET-TEST-RESULTS-2026-04-21.md`
4. Clean up with `sudo ./lamboot-install --remove`
5. Move to next VM

Only stop if a **release-blocking** bug is found — fix in tree, rebuild, redeploy, continue. Known-acceptable warnings (e.g. Fedora's Phase 8 BLS false-warning, already filed as polish) are flagged in the results doc, not treated as failures.

## 5. Acceptance criterion for v0.8.3

- **All 10 primary-wave VMs pass** → v0.8.3 is cross-distro-validated and publication-ready.
- **Any primary-wave VM fails with a real bug** → fix in tree, redo the single failing VM + any it could have affected.
- **Secondary wave ≥ 6/8 pass** → acceptable coverage. Remainder noted as known quirks, not release-blocking.
- **< 80% aggregate pass** → hold v0.8.3, escalate.

## 6. Output

- `docs/FLEET-TEST-RESULTS-2026-04-21.md` — per-VM pass/fail table, detailed logs for any failure, known warnings catalogued.
- `docs/TESTING-RESULTS-2026-04-21-TASK-44.md` — closeout summary, links to fleet-results doc, recommendation on whether v0.8.3 release gate #32 can proceed.

## 7. Timeline

- 2–4 minutes per VM install + verify + remove → ~40–80 minutes for all 18.
- Budget for bug-fixes discovered mid-test: treat each as commit→push→continue, not blocking.
