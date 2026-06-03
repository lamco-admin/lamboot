# LamBoot Security Model

**Audience:** system administrators, security engineers, auditors evaluating LamBoot's threat model.
**Version:** 0.12.0 (native-path shipped)
**Last updated:** 2026-05-30

> **Authoritative reference:** The formal trust-chain spec is
> [`docs/specs/SPEC-NATIVE-TRUST-CHAIN.md`](specs/SPEC-NATIVE-TRUST-CHAIN.md)
> (SDS-4). That document governs every security claim LamBoot makes
> publicly. When a statement here differs from the SDS, the SDS wins.
> Any user-facing security claim must appear in SDS-4's §8.1
> permitted-claims appendix first, backed by a code path.

This document is LamBoot's honest, plain-language statement of what its security features protect against and what they don't. We write it this way because most bootloaders don't — and we think the Linux boot security ecosystem has accumulated enough unstated gaps that users deserve a clear map.

## The native boot path (shipped since v0.9.0)

The native boot path (SDS-2 ext4/ext2 backend + SDS-3 native PE loader +
SDS-4 trust chain) resolves the older shim-15.8 `ShimLock`-uninstall
failure by structurally bypassing it: LamBoot calls `ShimLock::Verify`
once on the kernel bytes, then loads the kernel via its own PE loader.
`BS->LoadImage` is never invoked for the kernel. Every decision lands
as a `verified_via`-tagged trust-log event on `\loader\boot-trust.log`.
This is the default path on a current install — confirmed in the field on
ext4 and ext2 `/boot` under Secure Boot with shim+MOK.

Under the default policy on Fedora ext4 `/boot` with shim in the
chain, the `image_verified` + `image_loaded_native` event pair carries
the same SHA-256 (SDS-4 §6.4 invariant, enforced via `assert_eq!` at
load time). An auditor reading the trust log can confirm the kernel
byte stream that was verified is the byte stream that ran.

The §3 honest-gaps below are ecosystem limitations that still apply —
they are not LamBoot-specific and LamBoot cannot close them alone. What
LamBoot does provide is that its own contribution to the chain is now
auditable end-to-end.

---

## 1. The ecosystem in one paragraph

UEFI Secure Boot is a chain-of-trust model. The firmware's key database (`db`) contains certificates that signed binaries must chain to. Most Linux distros don't have their keys in `db` directly; instead they ship a Microsoft-signed "shim" loader that contains an embedded distro CA, and shim loads the distro's bootloader. The bootloader then loads the kernel. In theory, every link is cryptographically verified. In practice, **the bootloader → kernel handoff is not verified by most bootloaders** — GRUB, systemd-boot, and rEFInd all read and execute kernels directly without re-checking signatures. The kernel-signature chain is used by the Linux kernel itself (for module signing, IMA, etc.) but not by the bootloader that invokes it. This is widely documented in source code and developer blog posts but rarely in user-facing materials.

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

On traditional BLS deployments where the BLS config points to `/boot/vmlinuz-X.Y` on a root partition, an attacker with offline disk access can swap the kernel. On the native path, LamBoot reads the kernel bytes via its own filesystem backend and runs `ShimLock::Verify` on them before loading — the verdict is recorded as `image_verified` with the bytes' SHA-256, and `image_loaded_native` carries the same digest (the SDS-4 §6.4 invariant). A swap is therefore detectable in the trust log. But ShimLock has documented issues with some Canonical-signed Ubuntu kernels (PE-gap handling differences), and even when it accepts them, SBAT revocation isn't enforceable because Linux kernels don't carry `.sbat` sections.

GRUB "solves" this by skipping kernel verification entirely. LamBoot doesn't silently skip — it attempts verification and surfaces the outcome in the trust log. In practice this means a Canonical-signed Ubuntu kernel may fail to verify under LamBoot where GRUB would silently load it. We think the honest, recorded outcome is better than silent acceptance, but we acknowledge the UX friction.

**Mitigation path**: use UKIs (full signature coverage) or Config 4 with your distro's CA added to your custom OVMF VARS.

> **SBAT scope — decision (reviewed in the 2026-05-31 security audit).** SBAT
> revocation in the native trust chain was raised as an audit finding and
> consciously scoped, not deferred by oversight:
> - **LamBoot's own** SBAT generation is enforced **only when a shim is in the
>   boot chain** (Config 1/3): shim checks LamBoot against the firmware
>   `SBATLevel` before launching it (see the §2 threat table and
>   `packaging/SBAT-GENERATION.md`). SBAT is a shim-only mechanism — under the
>   no-shim Config 2/4 (LamBoot launched directly by firmware `db`) there is no
>   SBATLevel check; firmware `db` verifies the signature but does not consult
>   SBAT. That is an inherent property of db-direct boot, not a LamBoot gap.
> - **Linux kernels carry no `.sbat` section**, so per-kernel SBAT enforcement
>   is impossible at the loader regardless of bootloader; this is an ecosystem
>   gap (the fix is kernel `.sbat` adoption or UKIs), not a LamBoot omission.
> - **Chainloaded EFI bootloaders** (GRUB/systemd-boot via `chainload_efi`) DO
>   carry `.sbat`. The chainload routes through firmware `BS->LoadImage`: shim,
>   when present, enforces `SBATLevel` on the image; firmware `db` alone checks
>   only the signature (SBAT is not consulted). An additional in-loader
>   SBAT-vs-SBATLevel pre-check before chaining is possible but is deferred
>   future work — it duplicates the shim check and buys little until the kernel
>   gap above is closed.

### 3.2 Initrd integrity

BLS deployments use a separate, unsigned initrd file. An attacker who can swap the initrd can intercept LUKS unlock keys, add persistence, etc. LamBoot registers the initrd via LoadFile2 (same as GRUB, sd-boot) but doesn't verify it. No Linux bootloader does, on the BLS path.

**Mitigation path**: UKIs bundle initrd into the signed PE (full coverage).

### 3.3 SEC/PEI firmware compromise

UEFI Secure Boot starts at DXE. SEC and PEI phases execute before any SB check. An attacker with SPI flash write access can plant persistent malware that runs before LamBoot and invisibly compromises the system.

**Mitigation path**: hardware root-of-trust (Intel BootGuard, AMD PSB, Microsoft Pluton, discrete TPM attestation). LamBoot measures into TPM and our trust log can be cross-referenced with attestation services — but LamBoot can't close this gap alone.

### 3.4 Kernel command-line injection

BLS entries and most bootloaders allow editing the kernel command line at the menu (GRUB's `e`, sd-boot's cmdline editing). This bypasses the signed cmdline that UKIs provide. LamBoot does NOT allow command-line editing in the menu — it uses the BLS `options` field as-is. This is a quiet hardening vs GRUB default behavior.

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
{"seq":0,"event":"boot_start","note":"version=0.12.0 arch=x86_64 sb=ActiveWithShim crash_counter=0"}
{"seq":1,"event":"image_verified","path":"\\vmlinuz-6.12.90","sha256":"5af22ccd…","verified_via":"shim_mok","status":"SUCCESS"}
{"seq":2,"event":"image_loaded_native","path":"\\vmlinuz-6.12.90","sha256":"5af22ccd…","verified_via":"shim_mok","status":"SUCCESS","note":"backend=ext4-view loader=native_pe_loader"}
```

**Integration guidance for host-side tooling:**
- Read this file on successful boot-up. A host-side agent can tail and ship to SIEM.
- The log is NOT cryptographically signed. An attacker with ESP write access could rewrite it. Treat as advisory, not evidentiary. A signed event stream is roadmap work, not yet shipped.
- The file is appended per boot via positional `true_append` (it is not rewritten in place); rotate it host-side.

> **`boot-trust.log` vs `boot.json` — which `verified_via` is authoritative.**
> Two artifacts carry a `verified_via` field and they answer different
> questions. `\loader\boot-trust.log` (this file) is the **authoritative
> per-image record**: its `image_verified` event carries the actual outcome
> (`SUCCESS` / `SKIPPED` / `DEFERRED` / `REJECTED`, see the `status` row below),
> emitted at the moment of verification. `\EFI\LamBoot\reports\boot.json` is a
> **summary written at selection time, before** the kernel is read or verified
> (LamBoot hands off to the kernel after verification and never returns to
> update it); its `verified_via` therefore states the **intended Secure Boot
> posture**, not a post-hoc cryptographic result. Both derive from the single
> `sb_state` detected once per boot, so they agree on posture — but a consumer
> needing the actual pass/fail of a specific image must read the trust log's
> `image_verified` `status`, not `boot.json`.

**Fields:**

| Field | Meaning |
|---|---|
| `seq` | Monotonic sequence within a boot |
| `event` | `boot_start` / `image_verified` / `image_loaded_native` / `legacy_loadimage_used` / `boot_attempt` / `driver_loaded` / `driver_rejected` (and others) |
| `path` | File path, or menu entry name for `boot_attempt` |
| `size` | Bytes (0 when not applicable) |
| `sha256` | Hex digest (empty when not captured) |
| `verified_via` | `shim_mok` / `degraded_trust_sb_direct` / `degraded_trust_sb_off` / `shim_rejected` / `native_pe_loader` / `firmware_loadimage` (the stable SDS-4 vocabulary) |
| `status` | On `image_verified`: `SUCCESS` **only** when a signature was actually validated (shim+MOK accepted); `SKIPPED` when Secure Boot is off (nothing to verify); `DEFERRED` when SB is on without shim and the firmware `db` check runs later on the firmware LoadImage path; `REJECTED` when ShimLock refused. Elsewhere: the EFI_STATUS name. A consumer keying on `status=SUCCESS` therefore sees only genuinely-verified loads, not degraded ones. |
| `note` | Free-form context |
| `kernel_measured` / `cmdline_measured` | Emitted only when the TPM PCR was actually extended. If no TPM is present (or a TCG2 call fails) a `kernel_measurement_skipped` / `cmdline_measurement_skipped` event is recorded instead, so the log never implies a measurement that did not happen. |

---

## 6. Reporting security issues

See `SECURITY.md` at the repo root for CVE reporting procedures.

If you believe you've found a bootloader-level vulnerability in LamBoot, contact [office@lamco.io](mailto:office@lamco.io) with GPG-encrypted details. We aim to acknowledge within 48 hours and coordinate disclosure.

For issues in the underlying ecosystem (shim, GRUB, kernel, firmware), please report upstream to those projects — LamBoot can't fix them and we'd rather see them fixed for everyone.

---

## 7. Roadmap for closing these gaps

| Gap | Closure path | Target |
|---|---|---|
| Native PE loader bypassing the ShimLock-uninstall failure | Own PE loader + once-per-boot `ShimLock::Verify` | ✅ shipped v0.9.0 |
| Per-event trust evidence with matching verify/load SHA-256 | SDS-4 trust chain on `\loader\boot-trust.log` | ✅ shipped v0.9.0 |
| Non-crypto-signed trust log | Signed JSON event stream | not yet scheduled |
| BLS kernel not SBAT-verifiable | Ecosystem: wait for kernel `.sbat` or push UKIs | Ongoing |
| initrd unsigned in BLS | UKI adoption by distros | Ongoing |
| TPM2 remote attestation of the boot evidence | AIK-signed quote + evidence bundle | v1.1 (issue #16) |
| No hardware root of trust | TrenchBoot / DRTM integration | v2.0+ |
| User MOK enrollment friction | Microsoft shim-review submission | v1.1 (issue #12) |
| PQ crypto readiness | LMS/XMSS via MOK | future |

---

## 8. What we ask of users

1. **Prefer UKIs when available.** They close more gaps than any other boot artifact shape.
2. **Use Config 4 (custom OVMF VARS) on Proxmox fleets** where you control the hypervisor. It's the cleanest LamBoot deployment.
3. **Read `\loader\boot-trust.log` post-boot.** It tells you what actually got trusted. Nothing else surfaces this.
4. **Report gaps.** We publish this model as honestly as we can; if something's inaccurate or missing, tell us.
