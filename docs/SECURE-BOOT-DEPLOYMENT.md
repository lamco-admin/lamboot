# LamBoot Secure Boot Deployment Guide

**Audience:** system operators installing LamBoot on systems that may have UEFI Secure Boot enabled.
**Scope:** every trust path LamBoot supports, end-to-end, with recovery procedures.
**Version:** 0.8.3

---

## 1. What Secure Boot does and why it matters here

UEFI Secure Boot refuses to load EFI binaries unless their cryptographic signature chains to a key the firmware trusts. The firmware's trust anchors live in three variables — `PK` (Platform Key), `KEK` (Key Exchange Key), and `db` (authorized signature database). A fourth trust store, `MOK` (Machine Owner Key), is managed by `shim` — a small Microsoft-signed pre-bootloader that most distributions chain through.

LamBoot is not signed by Microsoft and is not trusted by default firmware. To run on a Secure-Boot-enabled system, LamBoot's signature must be brought into one of these trust stores. This document covers every supported way to do that.

---

## 2. Decision tree: which configuration are you in?

```
START
│
├── Is Secure Boot enabled on the target system?
│   │  (check with: `mokutil --sb-state` on Linux, or firmware setup menu)
│   │
│   ├── NO  ──→ Go to §3 (Config 1: SB disabled)
│   │
│   └── YES ──→ continue
│       │
│       ├── Is this a VM and do you control the hypervisor (Proxmox/libvirt/etc.)?
│       │   YES ──→ §6 (Config 4: Custom OVMF VARS) is strongly recommended
│       │           (simplest, zero-touch for fleet deployments)
│       │
│       ├── Can you reach the firmware setup menu and edit `db` keys manually?
│       │   YES ──→ §5 (Config 2: firmware db enrollment) is an option
│       │           (best when you own the hardware and want no shim overhead)
│       │
│       └── Otherwise (most distro users, physical hardware, no firmware access)
│           ──→ §4 (Config 3: shim chain + MOK enrollment) — the default path
```

---

## 3. Config 1 — Secure Boot disabled

**Trust path:** none. Firmware loads anything.

**Install:**
```
sudo lamboot-install
```

The install script detects SB-off state and deploys the unsigned `lambootx64.efi` directly. No shim, no MOK, no keys involved.

**When to use:** development VMs, airgapped systems, or platforms that never enabled SB. The simplest path.

**Recovery:** none needed — LamBoot is a normal boot entry you can remove with `lamboot-install --remove`.

---

## 4. Config 3 — Shim chain + MOK enrollment (the distro-user default)

**Trust path:**
```
firmware db  ──trusts──→  Microsoft UEFI CA
Microsoft CA ──signed──→  distro shim (shimx64.efi)
shim MOK     ──trusts──→  LamBoot signing cert (you enrolled it)
LamBoot cert ──signed──→  lambootx64-signed.efi
```

Shim is Microsoft-signed, so firmware trusts it. Shim then checks whatever it's loading against the firmware `db` first, then against its own MOK list. Because LamBoot's cert will be in MOK, shim accepts our binary and hands off execution.

**Prerequisites:**
- Distro shim already installed (Ubuntu: `shim-signed`; Debian/Fedora: ships in default install)
- Root access on the target system
- Console access for the MokManager reboot step

**Procedure:**

1. **Install with the signed-binary flag:**
   ```
   sudo lamboot-install --signed
   ```
   The script:
   - Detects SB state, finds distro shim at `/boot/efi/EFI/{ubuntu,debian,fedora,...}/shimx64.efi`
   - Copies shim to `/boot/efi/EFI/LamBoot/shimx64.efi`
   - Deploys `lambootx64-signed.efi` as `/boot/efi/EFI/LamBoot/grubx64.efi` (shim's expected follow-on loader name)
   - Copies `db.der` (our public cert) to `/boot/efi/EFI/LamBoot/db.der`
   - Creates UEFI boot entry pointing at `\EFI\LamBoot\shimx64.efi`
   - Prompts you for a MOK enrollment password and runs `mokutil --import /boot/efi/EFI/LamBoot/db.der`

2. **Reboot.** Firmware detects the pending MOK import and launches **MokManager.efi** (blue screen, keyboard-driven).

3. **In MokManager:** choose *Enroll MOK* → *View key 0* (verify the fingerprint matches what you expect for LamBoot) → *Continue* → *Yes, enroll* → enter the password you chose in step 1 → *Reboot*.

4. **After reboot:** shim now trusts LamBoot's key. Set LamBoot as boot default if it isn't already:
   ```
   sudo efibootmgr -o $(efibootmgr | awk '/LamBoot/{gsub(/\*/,"",$1); sub(/Boot/,"",$1); printf "%s,",$1}')$(efibootmgr | awk '/BootOrder/{print $2}' | tr -d ',' | tr -s ' ')
   ```
   Or simply select LamBoot from the firmware boot menu (usually F12 or F11) on next reboot to test first.

5. **Verify LamBoot boots:** watch the console. LamBoot splash appears, followed by entry selection.

**Verifying MOK enrollment after the fact:**
```
mokutil --list-enrolled | grep -A3 'LamBoot'
```
Should show our cert subject. If absent, repeat step 3 — sometimes the reboot timing misses MokManager.

**Common failure modes:**

| Symptom | Cause | Fix |
|---|---|---|
| MokManager never appears on reboot | `mokutil --import` wasn't run, or a pending import was cancelled | `sudo mokutil --import /boot/efi/EFI/LamBoot/db.der` then reboot |
| MokManager appears but key reverts | Password mismatch (MokManager is strict about entry) | Retry with the same password you gave `mokutil --import` |
| LamBoot entry in firmware menu, but selecting it returns to firmware | Signed binary was missing or unsigned was shipped as `grubx64.efi` | Verify `sbverify --cert keys/db.crt /boot/efi/EFI/LamBoot/grubx64.efi` succeeds on the host. Reinstall with `--signed` |
| Boot hangs after shim | LamBoot's SBAT revocation metadata is newer than shim's | Update shim (`apt install --reinstall shim-signed`) or regenerate LamBoot with older SBAT level |

**Recovery:** see §7.

---

## 5. Config 2 — Firmware db enrollment (advanced, owner-controlled hardware)

**Trust path:** LamBoot's cert is enrolled directly into the firmware's `db`. Firmware validates LamBoot; shim **is still deployed in the chain** and remains in memory to provide the `ShimLock` protocol that LamBoot uses to verify MOK-signed Linux kernels.

**Critical note (v0.8.3):** even in Config 2, do **not** pass `--no-shim` if your kernel is a stock distro kernel (Ubuntu, Debian, Fedora, …) — those are signed against MOK, not firmware DB. Without shim in the chain, LamBoot cannot verify them and the kernel will fail to boot. See `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21.md` for the root-cause write-up.

`--no-shim` is appropriate **only** when the kernel you want to boot is signed by a cert that is itself in firmware DB — typically a self-signed UKI or a kernel from a custom build shop. If that's your scenario, pair `--no-shim` with `--kernel-firmware-db-signed` to acknowledge the constraint.

**When to use:** you own the hardware, can reach firmware setup, and want the firmware to trust LamBoot directly without relying on MokManager. Typical for homelab operators, developers, and Proxmox host boot (as opposed to guests).

**Procedure varies by firmware.** The general shape:

1. Copy `keys/db.der` to a FAT-formatted USB stick or mount it accessible from firmware setup.
2. Reboot into firmware setup (DEL/F2/F10 depending on vendor).
3. Navigate to *Security → Secure Boot → Key Management* (wording varies).
4. Enter *Custom* or *Setup* mode if not already.
5. Append `db.der` to the `db` variable.
6. Save, exit, reboot.
7. Run `sudo lamboot-install --signed` (default — deploys shim in the chain, no MokManager prompt needed because firmware already trusts LamBoot).
   * If your kernel is genuinely firmware-DB-signed (UKI + self-signed scenario), you may use `sudo lamboot-install --signed --no-shim --kernel-firmware-db-signed` instead.

Vendor-specific guidance: see `docs/appendix/firmware-enrollment-per-vendor.md` (TBD — contribute examples as you test hardware).

**Verifying:**
```
sudo efi-readvar -v db 2>&1 | grep -A5 'LamBoot'
```
(Requires `efivar` or `efitools` package.)

**Recovery:** remove LamBoot's entry from `db` via firmware setup (same menu, *Delete*). No cleanup inside Linux beyond `lamboot-install --remove`.

---

## 6. Config 4 — Custom OVMF VARS (Proxmox and libvirt operators)

**Trust path:** the VM's OVMF firmware ships with LamBoot's cert pre-enrolled in `db`. Firmware validates LamBoot; shim is deployed in the chain by `lamboot-install`, and the guest's own MOK (populated when the distro was installed) provides the kernel trust chain. "Zero-touch" means **no MokManager interaction is required** — it does **not** mean "no shim in the chain."

**Critical v0.8.3 clarification.** Earlier drafts of this section described Config 4 as "no shim, no MOK, no user interaction, Linux kernel just boots." That was wrong for stock Linux distro kernels and users who followed that path hit a hard kernel-load failure inside LamBoot. The corrected model:

- **`db` (firmware):** pre-enrolled with Lamco's LamBoot cert via the shipped `OVMF_VARS_lamboot.fd`. Lets firmware trust LamBoot without setup-mode enrollment.
- **Shim:** deployed by `lamboot-install` into `\EFI\LamBoot\shimx64.efi`, used as the boot entry target; shim chainloads LamBoot as `grubx64.efi`. Shim's presence in the running process is what provides the `ShimLock` protocol LamBoot needs to validate the distro kernel.
- **MOK:** whatever the distro installer already set up (Canonical Master CA on Ubuntu, Debian Secure Boot CA on Debian, Fedora CA on Fedora). `lamboot-install --signed --no-mok` under Config 4 skips the `mokutil --import` step because the guest MOK already trusts the distro kernel — we don't need to add our own cert for kernel verification.

If your use case is genuinely "no shim, kernel is firmware-DB-signed" (self-signed UKI, custom build shop with custom cert in DB), pass `--no-shim --kernel-firmware-db-signed` and skip the shim deployment. That path is supported but narrow.

**When to use:** deploying LamBoot across a fleet of VMs where the operator controls the hypervisor and the guest is a stock Linux distro. The operator controls the trust via pre-enrolled VARS; the guest doesn't need MokManager at all.

**Prerequisites:**
- `OVMF_VARS_lamboot.fd` (ships in release tarball at `dist/OVMF_VARS_lamboot.fd`)
- Proxmox/libvirt operator privileges
- VM is shut down before swapping VARS

**Proxmox procedure:**

1. Copy the VARS file to the Proxmox node:
   ```
   scp dist/OVMF_VARS_lamboot.fd root@pve:/var/lib/vz/snippets/
   ```

2. Shut down the target VM:
   ```
   qm shutdown <VMID>
   ```

3. Remove the existing efidisk and create a new one seeded from our VARS:
   ```
   qm set <VMID> --delete efidisk0
   qm set <VMID> --efidisk0 <STORAGE>:1,efitype=4m,pre-enrolled-keys=0
   # Then write our VARS over the new efidisk:
   qemu-img convert -O raw /var/lib/vz/snippets/OVMF_VARS_lamboot.fd \
       /dev/<ZFS or LVM path to vm-<VMID>-disk-N>
   ```
   (Exact path depends on storage backend — see `docs/OVMF-VARS-PROXMOX.md` — TBD — for per-backend specifics.)

4. Inside the guest (boot once with the old bootloader to reach Linux, OR use `qemu-nbd` from the host), install LamBoot:
   ```
   sudo lamboot-install --signed --no-mok
   ```
   `--no-mok` skips the interactive MokManager enrollment prompt — the guest's existing MOK (Canonical/Debian/Fedora Master CA) is what we'll rely on for kernel trust. Shim is still deployed; LamBoot runs behind it.

5. Reboot. Shim loads, runs LamBoot (as `grubx64.efi`), LamBoot presents the menu and successfully verifies the distro kernel via `ShimLock::Verify` + existing MOK.

**libvirt procedure:** analogous, but edit the domain XML to point `<nvram>` at a copy of `OVMF_VARS_lamboot.fd`. Details TBD.

**Verifying:** same as Config 2 — `efi-readvar -v db` from inside the guest should show LamBoot's cert.

**Recovery:** swap efidisk back to stock OVMF VARS:
```
qm set <VMID> --delete efidisk0
qm set <VMID> --efidisk0 <STORAGE>:1,efitype=4m,pre-enrolled-keys=1
```
(Stock VARS with Microsoft keys.) LamBoot boot entry will fail, firmware falls through to any other bootable entry.

---

## 7. Recovery procedures (all configurations)

### If LamBoot entry exists but LamBoot fails to load

The VM/system will fall through to the next `BootOrder` entry. You can still boot Linux, then:

```
# Remove the LamBoot NVRAM entry
sudo efibootmgr | grep LamBoot
sudo efibootmgr -B -b <HEX>   # e.g., -b 0003

# Remove LamBoot files
sudo rm -rf /boot/efi/EFI/LamBoot

# Remove pending MOK import (if any)
sudo mokutil --reset
# or, if already enrolled:
sudo mokutil --delete /boot/efi/EFI/LamBoot/db.der
```

Reboot. System boots via the original bootloader.

### If LamBoot is the ONLY remaining boot entry and fails

Two sub-cases:

**(a) Firmware falls to "Fallback Bootloader":** use the firmware menu (F12/F11 typically, or firmware setup) to pick an internal UEFI shell or another entry. If none exist, you need host-side recovery.

**(b) Host-side recovery (VM only):** stop the VM, use `qemu-nbd` to mount the guest's ESP from the host:
```
qm stop <VMID>
modprobe nbd
qemu-nbd -c /dev/nbd0 /dev/<zfs or lvm path to vm-<VMID>-disk-N>
# Wait for kernel to detect partitions
mount /dev/nbd0p1 /mnt   # assuming ESP is p1 — verify with fdisk -l
# Replace LamBoot with the original bootloader — copy the original back:
cp /mnt/EFI/<distro>/grubx64.efi /mnt/EFI/BOOT/BOOTX64.EFI  # fallback path
umount /mnt
qemu-nbd -d /dev/nbd0
qm start <VMID>
```

**(c) Physical hardware recovery:** boot from a live USB (any Linux distro), chroot into the installed system's `/`, then follow procedure (a).

### If MOK enrollment went wrong

```
sudo mokutil --list-enrolled                    # what's currently enrolled
sudo mokutil --reset                            # cancel pending imports
sudo mokutil --delete <cert.der>                # remove an enrolled cert
                                                # (requires reboot + MokManager again)
```

### If firmware db enrollment went wrong (Config 2)

Only recoverable through firmware setup. Reboot into firmware, navigate to the same Key Management menu, remove our entry. If firmware is locked down, some boards require a CMOS reset to return to Setup Mode.

---

## 7.5 Unified Kernel Images (UKI)

LamBoot supports **UKIs as first-class menu entries** in all four configurations. UKIs are single-file PE binaries that bundle a Linux kernel, initrd, cmdline, and (optionally) embedded signature per the [UAPI UKI specification](https://uapi-group.org/specifications/specs/unified_kernel_image/).

LamBoot automatically discovers UKIs in `\EFI\Linux\*.efi` on the ESP. Each discovered UKI appears as a separate menu entry:

- **Display name** is extracted from the UKI's `.osrel` section (PRETTY_NAME + kernel version when present).
- **Command line** is extracted from the `.cmdline` section — no distro-specific knowledge needed.
- **Sort order**: newest kernel version first.
- **Preflight**: LamBoot validates the PE header, verifies signature under Secure Boot, and flags missing entries with a red indicator.

No LamBoot-side configuration required. If your distro writes UKIs (kernel-install with ukify on Arch, Fedora 41+, Debian with the `systemd-ukify` package, Ubuntu with opt-in configuration), LamBoot will discover them.

**Why this matters for Secure Boot:** UKIs live on the FAT ESP. Firmware reads FAT natively — no ext4/btrfs/xfs driver needed. Under Config 3 (shim + MOK), UKIs boot even if filesystem driver loading has quirks, because the UKI itself is the kernel. On Ubuntu systems where kernels live on an ext4 root partition, converting to a UKI workflow sidesteps driver-loading issues entirely.

**Signing UKIs:** distro ukify tooling signs UKIs with the distro's MOK or with user-enrolled custom keys. LamBoot does not need to sign UKIs — it validates the existing signature via ShimLock or firmware db (depending on config) before booting.

---

## 8. Production-grade signing keys

LamBoot's development tree ships with test keys labeled "LamBoot Dev" (see `keys/db.crt`). **These are not production signing keys.** A production release requires a distinct key hierarchy with:

- Proper organizational identity in the cert subject (CN, O, OU, C)
- 10-year or longer validity for PK/KEK; 3-5 years for db (rotatable)
- Private keys protected (offline, HSM, or minimum encrypted at rest)
- A documented key-rotation plan

Procedure for generating production keys is in `docs/KEY-GENERATION.md` (TBD). Until that document exists, LamBoot binaries signed with the dev keys should not be distributed to end users under the LamBoot name. Test builds signed with dev keys are fine for internal validation of the trust-path logic.

---

## 9. Testing matrix (per release)

Before v0.8.3 ships, each row must pass end-to-end on a fresh VM:

| VM | Config | Signed? | What it proves |
|---|---|---|---|
| fedora-lamboot (201) SB-off | 1 | No | Baseline unsigned install |
| debway (100) SB-off | 1 | No | Baseline cross-distro |
| ubuntu-wayland-test (120) stock OVMF | 3 | Yes | Shim chain + MOK on Ubuntu |
| A Debian VM with SB forced on | 3 | Yes | Shim chain + MOK on Debian |
| A Fedora VM with SB forced on | 3 | Yes | Shim chain + MOK on Fedora |
| Any VM with `OVMF_VARS_lamboot.fd` swapped | 4 | Yes | Proxmox zero-touch |
| At least one physical machine, manual db enrollment | 2 | Yes | Firmware db path |

---

## 10. Related documentation

- `docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md` — architectural rationale
- `docs/MOK-ENROLLMENT-GUIDE.md` — step-by-step MOK walkthrough with screenshots (TBD)
- `docs/OVMF-VARS-PROXMOX.md` — Proxmox-specific Config 4 procedures (TBD)
- `docs/SB-RECOVERY.md` — expanded recovery scenarios (TBD)
- `docs/SECURITY-GUIDE.md` — full LamBoot security model
- `docs/KEY-GENERATION.md` — production key-hierarchy generation (TBD)
