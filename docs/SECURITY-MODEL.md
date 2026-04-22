# LamBoot Security Model

**Audience:** system administrators, security engineers, auditors evaluating LamBoot's threat model.
**Version:** 0.8.3
**Last updated:** 2026-04-21

This document is LamBoot's honest, plain-language statement of what its security features protect against and what they don't. We write it this way because most bootloaders don't — and we think the Linux boot security ecosystem has accumulated enough unstated gaps that users deserve a clear map.

---

## 1. The ecosystem in one paragraph

UEFI Secure Boot is a chain-of-trust model. The firmware's key database (`db`) contains certificates that signed binaries must chain to. Most Linux distros don't have their keys in `db` directly; instead they ship a Microsoft-signed "shim" loader that contains an embedded distro CA, and shim loads the distro's bootloader. The bootloader then loads the kernel. In theory, every link is cryptographically verified. In practice, **the bootloader → kernel handoff is not verified by most bootloaders** — GRUB, systemd-boot, and rEFInd all read and execute kernels directly without re-checking signatures. The kernel-signature chain is used by the Linux kernel itself (for module signing, IMA, etc.) but not by the bootloader that invokes it. This is widely documented in source code and developer blog posts but rarely in user-facing materials. See [analysis doc](analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md) §4 for the full catalog.

LamBoot inherits this ecosystem. What follows is what we do differently, what we keep the same, and what gaps we openly acknowledge.

---

## 2. What LamBoot defends against

| Threat | Defense |
|---|---|
| Unsigned / wrong-key bootloader binary | Firmware `db` verification (Config 2/4) or shim+MOK (Config 3) — same as GRUB, but with our signing keys |
| Tampered LamBoot binary on ESP | Detected at next boot via signature check |
| Offline swap of ESP files | ESP sits inside the Secure Boot chain — unsigned replacements rejected |
| Rollback to vulnerable bootloader version | SBAT generation checks (shim enforces this on LamBoot) |
| Unauthorized driver load under Secure Boot | Path F (SecurityArchProtocol override) routes driver LoadImage through ShimLock::Verify — matches systemd-boot precedent |
| Stale BLS entries referencing purged kernels | Install-script lifecycle fix: `--update` regenerates our manifest-tracked entries when their kernels disappear |
| Opaque boot decisions | **Trust-evidence log** at `\loader\boot-trust.log` records every trust decision (LamBoot-unique in this class) |
| TPM PCR manipulation via variant GRUB paths | UKIs have deterministic PCR values (LamBoot first-class supports UKIs) |

---

## 3. What LamBoot does NOT defend against (and neither does anyone else)

These gaps are inherited from the Linux SB ecosystem. LamBoot doesn't close them alone — they require ecosystem-wide fixes.

### 3.1 Kernel binary swap

On traditional BLS deployments where the BLS config points to `/boot/vmlinuz-X.Y` on an ext4 root partition, an attacker with offline disk access can swap the kernel. LamBoot calls firmware LoadImage on the kernel, which under `--signed` goes through our SecurityOverride → ShimLock::Verify. But ShimLock has documented issues with Canonical-signed Ubuntu kernels (PE-gap handling differences), and even when it accepts them, SBAT revocation isn't enforceable because Linux kernels don't have `.sbat` sections.

GRUB "solves" this by skipping kernel verification entirely. LamBoot doesn't silently skip — we attempt verification and surface the failure in the trust log. For v0.8.3, this means: a Canonical-signed Ubuntu kernel may fail to load under LamBoot where GRUB would silently load it. We think the honest failure is better than the silent acceptance, but we acknowledge the UX friction.

**Mitigation path**: use UKIs (full signature coverage) or Config 4 with your distro's CA added to your custom OVMF VARS.

### 3.2 Initrd integrity

BLS deployments use a separate, unsigned initrd file. An attacker who can swap the initrd can intercept LUKS unlock keys, add persistence, etc. LamBoot registers the initrd via LoadFile2 (same as GRUB, sd-boot) but doesn't verify it. No Linux bootloader does, on the BLS path.

**Mitigation path**: UKIs bundle initrd into the signed PE (full coverage).

### 3.3 SEC/PEI firmware compromise

UEFI Secure Boot starts at DXE. SEC and PEI phases execute before any SB check. An attacker with SPI flash write access can plant persistent malware that runs before LamBoot and invisibly compromises the system.

**Mitigation path**: hardware root-of-trust (Intel BootGuard, AMD PSB, Microsoft Pluton, discrete TPM attestation). LamBoot measures into TPM and our trust log can be cross-referenced with attestation services — but LamBoot can't close this gap alone.

### 3.4 Kernel command-line injection

BLS entries and most bootloaders allow editing the kernel command line at the menu (GRUB's `e`, sd-boot's cmdline editing). This bypasses the signed cmdline that UKIs provide. LamBoot currently does NOT allow command-line editing in the menu — v0.8.3 uses the BLS `options` field as-is. This is a quiet hardening vs GRUB default behavior.

### 3.5 Kernel-module compromise

Once Linux is running, kernel modules can be loaded. Modules are signed separately (MOK-based) and the kernel enforces. LamBoot has no visibility into this — it's Linux's problem. We do measure the bootloader handoff to TPM PCR 4 so runtime attestation can catch divergence.

---

## 4. What configurations we support and their trust properties

| Config | Trust root | What's verified | Gaps |
|---|---|---|---|
| **1: SB disabled** | None | Nothing — runs anything | User-chosen trust model |
| **2: Firmware db (manual enrollment)** | User's key in firmware db | LamBoot + drivers + UKIs via firmware SB | Kernel handoff gaps per §3.1-3.4 |
| **3: Shim + MOK** | Microsoft-signed shim + user-enrolled MOK | LamBoot via shim chain, drivers via Path F, UKIs via ShimLock | Same §3 gaps + ShimLock edge cases for some distro kernels |
| **4: Custom OVMF VARS (Proxmox zero-touch)** | User's key pre-enrolled in firmware db of VM | LamBoot + drivers + UKIs via firmware SB | Same §3 gaps; need Canonical/distro CA also in VARS for BLS distro kernels |

UKIs in all configs: full signature coverage of kernel + initrd + cmdline.

---

## 5. Trust-evidence log

LamBoot writes `\loader\boot-trust.log` on the ESP recording every trust decision. Format: JSON Lines, UTF-8. One event per line. Sample:

```json
{"seq":0,"event":"boot_start","note":"version=0.8.3 arch=x86_64 sb=ActiveDirect crash_counter=0"}
{"seq":1,"event":"driver_loaded","path":"\\EFI\\LamBoot\\drivers\\ext4_x64.efi","verified_via":"security_override","status":"SUCCESS"}
{"seq":2,"event":"boot_attempt","path":"Ubuntu 25.10 (6.17.0-22-generic)","note":"..."}
```

**Integration guidance for host-side tooling:**
- Read this file on successful boot-up. A host-side agent can tail and ship to SIEM.
- v0.8.3 log is NOT cryptographically signed. An attacker with ESP write access could rewrite it. Treat as advisory, not evidentiary. Cryptographic signatures come in v0.9.x (Path G).
- The file is truncated and rewritten each boot.

**Fields:**

| Field | Meaning |
|---|---|
| `seq` | Monotonic sequence within a boot |
| `event` | `boot_start` / `driver_loaded` / `driver_rejected` / `boot_attempt` (more in v0.9.x) |
| `path` | File path, or menu entry name for `boot_attempt` |
| `size` | Bytes (0 when not applicable) |
| `sha256` | Hex digest (empty in v0.8.3 when not captured) |
| `verified_via` | `firmware_db` / `shim_mok` / `shim_vendor` / `security_override` / `rejected` / `sb_disabled` |
| `status` | EFI_STATUS name |
| `note` | Free-form context |

---

## 6. Reporting security issues

See `SECURITY.md` at the repo root for CVE reporting procedures.

If you believe you've found a bootloader-level vulnerability in LamBoot, contact [office@lamco.io](mailto:office@lamco.io) with GPG-encrypted details. We aim to acknowledge within 48 hours and coordinate disclosure.

For issues in the underlying ecosystem (shim, GRUB, kernel, firmware), please report upstream to those projects — LamBoot can't fix them and we'd rather see them fixed for everyone.

---

## 7. Roadmap for closing these gaps

| Gap | Closure path | Target |
|---|---|---|
| Non-crypto-signed trust log | Path G — signed JSON event stream | v0.9.x |
| Silent ShimLock rejection | Path G — own PE loader with direct Authenticode verification + cert-chain walker | v0.9.x |
| BLS kernel not SBAT-verifiable | Ecosystem: wait for kernel `.sbat` or push UKIs | Ongoing |
| initrd unsigned in BLS | UKI adoption by distros | Ongoing |
| No hardware root of trust | TrenchBoot / DRTM integration | v2.0+ |
| User MOK enrollment friction | Microsoft shim-review submission | v1.0 |
| PQ crypto readiness | LMS/XMSS via MOK | v1.x |

---

## 8. What we ask of users

1. **Prefer UKIs when available.** They close more gaps than any other boot artifact shape.
2. **Use Config 4 (custom OVMF VARS) on Proxmox fleets** where you control the hypervisor. It's the cleanest LamBoot deployment.
3. **Read `\loader\boot-trust.log` post-boot.** It tells you what actually got trusted. Nothing else surfaces this.
4. **Report gaps.** We publish this model as honestly as we can; if something's inaccurate or missing, tell us.
