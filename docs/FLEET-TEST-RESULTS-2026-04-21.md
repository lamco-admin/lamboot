# LamBoot v0.8.3 Fleet Install Test — Results (in progress)

**Started:** 2026-04-21
**Plan:** `docs/FLEET-TEST-PLAN-2026-04-21.md`
**Method:** for each VM: start → deploy current dist tree → `lamboot-install` → 10-point checklist → `lamboot-install --remove` → shut down.

---

## Primary wave

| # | VMID | Name | Install | Distro | ESP | Kernels | BLS | Driver | Boot Entry | BootOrder | Hook Helper | --remove | Overall |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| — | 115 | debian-13-template | — | — | — | — | — | — | — | — | — | — | **SKIP** (template, not bootable) |
| — | 121 | ubuntu-24.04-sway | n/a | n/a | n/a | n/a | n/a | n/a | **FAIL** | n/a | n/a | ok | **SKIP — actually BIOS/SeaBIOS not UEFI; `bios: ovmf` missing in config; no `efidisk0`. Install ran through Phase 1–5 + 7–8 but Phase 6 (UEFI boot entry creation) failed silently because `/sys/firmware/efi/` doesn't exist. dist-upgrade performed (apt update + dist-upgrade; new kernel 6.8.0-110 installed). VM shut down.** |
| 1 | 122 | fedora-sway | ✓ | fedora | ✓ | 4 | native (4 exist on /boot) | ext4 | Boot0004 | default | hook+plugin | ✓ | **FAIL — boot chain incomplete; root cause is BLS-on-ext4 discovery gap (SDS-5 v1.0 item).** Real sequence (verified by 2nd run + 7min wait + post-mortem trust log): LamBoot loads ext4 driver ✓ → scans ESP /loader/entries/ (empty; Fedora puts them on ext4 /boot) → finds only "EFI Fallback" → chainloads \EFI\BOOT\BOOTX64.EFI (Fedora shim, sha 4773d74d — NOT LamBoot self-loop) → Fedora shim → GRUB → GRUB hangs, never loads a kernel. No kernel_load_failed event. No SSH / guest agent after 7min. If LamBoot could see Fedora's BLS entries, it would boot_linux directly via LoadFile2 and avoid the shim→GRUB re-entry problem. This is the v1.0 Path G gap manifesting on Fedora-with-SB-off, same release posture as the already-documented shim-15.8-SB-on limitation. |
| 2 | 123 | endeavouros-sway | ✓ | endeavouros | ✓ (/efi) | on-ESP | native (2 exist) | ext4 | Boot0002 | default | hook+plugin | ✓ | **PASS — real live boot verified.** pacman -Syu upgrade to kernel 6.19.12-arch1-1 completed. EndeavourOS uses systemd-boot convention: ESP at `/efi` (not `/boot/efi`), kernels+initrds on the ESP itself at `/<machine-id>/<version>/{linux,initrd}`, BLS entries on ESP at `/efi/loader/entries/`. LamBoot's install script auto-detected `/efi` as ESP. BLS entries visible to LamBoot scanner (they're on FAT ESP, same place LamBoot looks). Reboot: `BootCurrent: 0002` (LamBoot). Trust log: `boot_start` → `driver_loaded ext4 via security_override SUCCESS` → `boot_attempt EndeavourOS` → kernel booted. Post-boot: `uname -r = 6.19.12-arch1-1` matches LamBoot's selected entry. --remove clean: Boot0002 removed, systemd-boot restored as default. VM shut down. **This is the kernel-on-ESP happy path that v0.8.3 actually delivers today.** |
| 3 | 124 | popos-cosmic | ✓ | pop (debian-like) | ✓ | on-ESP | native (3 exist) | ext4 | Boot0004 | default | hook+plugin | ✓ | **PASS after fix.** apt full-upgrade completed. Initial test FAILED with LamBoot auto-selecting Recovery-79EB-58C6 casper live-media instead of Pop_OS-current. Root cause: bls.rs filename-fallback sort reversed plain-name comparisons that shouldn't be reversed. Fixed in commit 988851b (only reverse when BOTH entries have version metadata). Patched ESP via qemu-nbd, rebooted: trust log now shows `boot_attempt: Pop!_OS (bls-Pop_OS-current)`, `/proc/cmdline` matches installed system, `hostname=popos`. --remove clean. VM shut down. |

## Secondary wave

(pending primary completion)

---

## Notes as they come up

_(appended per VM)_
