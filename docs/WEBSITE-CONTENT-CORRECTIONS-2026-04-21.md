# LamBoot Website Content Corrections — 2026-04-21

**Audience:** LAMCO-website team
**Status:** Corrections required across 10 LamBoot pages on toom before any public-facing release of v0.8.3.
**Companion to:** `docs/WEBSITE-CONTENT.md` (v0.8.3 canonical brief — still the source of truth for everything not contradicted here)
**Companion to:** `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` (the root-cause finding)

---

## 1. Why this document exists

After shipping v0.8.3's trust-evidence log and SecurityOverride, live-boot testing on VM 120 (Ubuntu 25.10, shim-signed 1.59+15.8-0ubuntu2) revealed that our **Config 4 "zero-touch Proxmox" story doesn't actually boot stock distro kernels today**. Root cause: shim 15.8 (Ubuntu's current shim) lacks `ShimRetainProtocol`. When LamBoot loads a UEFI filesystem driver (ext4, btrfs, …), shim uninstalls its `ShimLock` protocol, leaving the kernel unverifiable.

The v0.8.3 code ships with honest instrumentation (per-hook counters, `kernel_load_failed` trust events) and install-script guardrails. The full architectural fix is Path G (v0.9.x), which implements a native PE loader bypassing this entire failure mode.

Until Path G lands, the website content must be corrected to:
- Remove the "Linux kernel just boots under SB" framing from Config 4
- Add the UKI-on-ESP / kernel-on-ESP working configurations as the recommended SB path
- Explicitly note Path G (v0.9.x) as the landing place for "SB + stock distro + ext4 /boot"
- Preserve the v0.8.3 SB-off, trust-log, install-safety, and differentiation claims unchanged

---

## 2. Files requiring correction on toom

All under `~/LAMCO-website/src/website/templates/website/`.

| File | Corrections required | Priority |
|---|---|---|
| `lamboot_install.html` | §3.1 | **Critical** — misleads users |
| `lamboot_proxmox.html` | §3.2 | **Critical** |
| `lamboot_security.html` | §3.3 | **Critical** |
| `lamboot_faq.html` | §3.4 | **Critical** (structured data includes the claim) |
| `lamboot_roadmap.html` | §3.5 | High (Path G must reflect this motivation) |
| `lamboot.html` (home) | §3.6 | High (hero messaging) |
| `lamboot_compare.html` | §3.7 | Medium (matrix annotation) |
| `lamboot_architecture.html` | §3.8 | Medium (technical detail) |
| `lamboot_innovations.html` | §3.9 | Low (generic enough) |
| `lamboot_tools.html` | §3.10 | Low |
| `_lamboot_subnav.html` | no change | N/A |

---

## 3. Per-file corrections

### 3.1 `lamboot_install.html`

**Current claim (install page):**
- Hero subtitle: "Three paths — homelab, Linux desktop with Secure Boot, or Proxmox zero-touch"
- Proxmox zero-touch card: "Pre-enrolled OVMF_VARS. No MokManager dance."
- Proxmox section body: `sudo ./lamboot-install --signed --no-mok # cert already in firmware DB` followed by "All clones boot LamBoot without MokManager."

**Correction:**
1. The three-card landing block stays, but the Proxmox card description should read:
   > *"Pre-enrolled OVMF_VARS plus shim in the chain. No interactive MokManager during first boot. Works with UKI or ESP-local kernels today; stock distro /boot on ext4 lands in v0.9.x."*

2. The Proxmox section code block must be updated to reflect shim-is-mandatory:
   ```bash
   # on the Proxmox host
   cp lamboot-0.8.3/OVMF_VARS_lamboot.fd /var/lib/vz/images/100/OVMF_VARS_100.fd
   ```
   ```bash
   # inside the VM — first boot into any Linux installer
   sudo ./lamboot-install --signed --no-mok
   # --no-mok skips MokManager; shim is STILL deployed (that's not optional under SB).
   # Kernel trust comes from the guest's existing distro MOK (Canonical/Debian/Fedora CA)
   # + shim's ShimLock protocol.
   ```

3. Add a callout box below the code:
   > **Caveat for stock Ubuntu/Debian/Fedora /boot on ext4 (v0.8.3):** shim 15.8 uninstalls its `ShimLock` protocol after LamBoot's ext4 driver loads, so the kernel can't be verified. Workarounds: (a) use UKI (`/EFI/Linux/*.efi`) instead of `/boot/vmlinuz-*`; (b) stage kernel+initrd on the ESP; (c) wait for v0.9.x (Path G native PE loader). Full detail on the Security page.

4. Remove "All clones boot LamBoot without MokManager" from the closing sentence — replace with: "Clones inherit the trust state; Path G in v0.9.x removes the remaining ext4-/boot limitation."

### 3.2 `lamboot_proxmox.html`

**Current claim:** The "Pair with zero-touch Secure Boot" section suggests Config 4 is turnkey for stock distro images.

**Correction:**
- Keep the monitoring (`lamboot-monitor.py`) story unchanged.
- Rewrite the zero-touch section to:
  > **Config 4 (Proxmox zero-touch) today ships with a known limitation:** the pre-enrolled-VARS + shim-in-chain approach works for UKI or ESP-local-kernel VM images. For stock Debian/Ubuntu/Fedora cloud images with `/boot` on ext4, the kernel-verification step currently fails on shim 15.8 (the ShimLock protocol is uninstalled once a UEFI filesystem driver loads). **Path G (v0.9.x) replaces the LoadImage flow with a native PE loader and closes this gap.** Until then, the recommended Proxmox template flow is to build UKI images, or to stage kernel+initrd on the ESP.
- Link to `/products/lamboot/security/#config-4` and `/products/lamboot/roadmap/#path-g`.

### 3.3 `lamboot_security.html`

**Current claim at line 196:**
> "Config 4 — Zero-touch Proxmox. Custom OVMF_VARS with LamBoot's db cert pre-enrolled. VM templates at fleet scale."

**Correction:**
Replace the Config 4 callout with:
> **Config 4 — Pre-enrolled OVMF_VARS (zero MokManager).** Custom `OVMF_VARS` with LamBoot's db cert enrolled. Shim is still deployed in the chain; the "zero-touch" aspect is *no interactive MokManager on first boot* — distro MOK (already populated by the guest install) continues to provide kernel trust. **v0.8.3 known limitation:** on shim 15.8 (current Ubuntu / Debian), ShimLock is uninstalled after LamBoot's UEFI filesystem driver loads, so this configuration cannot verify kernels that live on `/boot` on ext4. Recommended workarounds: UKI on the ESP, or kernel+initrd staged on the ESP. **Path G (v0.9.x)** adds a native PE loader that removes the dependency on `BS->LoadImage` at kernel-load time, closing this gap.

In the trust-log example values list (`verified_via` enumeration), add a new entry to document the new event:
- `kernel_load_failed` — kernel's `BS->LoadImage` returned an error; the trust log's `note` field includes per-hook counters for diagnosis.

### 3.4 `lamboot_faq.html`

**Current FAQ answer (both the visible accordion and the schema.org structured data):**
> "Yes — four different configurations. Simplest for Linux desktops is shim + MOK enrollment. Simplest for Proxmox VM fleets is the pre-enrolled OVMF_VARS template."

**Correction (short form to keep FAQ punchy, linked to security page for detail):**
> "Yes — four configurations. For Linux desktops: shim + MOK enrollment. For Proxmox VM fleets: pre-enrolled OVMF_VARS (no MokManager needed; shim still in chain). Note: v0.8.3 SB support assumes either UKI on the ESP or kernel+initrd staged on the ESP. Stock /boot on ext4 works under SB starting in v0.9.x (Path G). SB-off works everywhere today. See the security page for the full breakdown."

Update the JSON-LD structured-data block to match. Keep the `Is LamBoot shim-review approved?` answer unchanged (still v1.0 target).

### 3.5 `lamboot_roadmap.html`

**Current Path G entry** mentions closing "shim asymmetric-SBAT gap and several documented bypasses" — keep that.

**Correction — add explicit v0.8.3 limitation and v0.9.x resolution:**
- Under the v0.8.3 "Shipping now" checklist, add:
  > ✗ **Known limitation:** Under Secure Boot, stock-distro `/boot` on ext4 is not verifiable because shim 15.8 uninstalls ShimLock after LamBoot's filesystem driver loads. Workarounds for v0.8.3: UKI on ESP, or stage kernel+initrd on ESP.
- Under the v0.9.x Path G section, add a bullet:
  > • **Resolves the v0.8.3 Config-4 ext4-/boot limitation.** Native PE loader verifies the kernel up front via ShimLock (while it's still installed), then loads and jumps to the kernel without going back through firmware's `LoadImage` — so the post-driver-load uninstall of ShimLock no longer matters.
- Under v0.8.4 (insert between v0.8.3 and v0.9.x), add:
  > • **Kernel / UKI staging on the ESP.** Install script + kernel hook copy kernel+initrd (or a generated UKI) to `\EFI\LamBoot\kernels\`, with ESP-path BLS entries. Makes SB-with-stock-distro work without requiring Path G.

### 3.6 `lamboot.html` (home)

**Current claims of concern:** the home page is fine in aggregate; the only claim that needs softening is the "four Secure Boot configurations" bullet which overpromises.

**Correction:** In the feature-cards / bullet-list area, rephrase any "Four Secure Boot configurations — from off to zero-touch enterprise" to:
> "Four Secure Boot configurations — SB off, firmware-DB trust, shim + MOK, and pre-enrolled OVMF_VARS for Proxmox. Known v0.8.3 limitation on stock /boot on ext4 under SB; Path G (v0.9.x) closes it."

Keep everything else. The honest-security framing is still the right pitch.

### 3.7 `lamboot_compare.html`

**Current matrix cell (LamBoot Secure Boot row):**
> "DB + shim + MOK + SecurityOverride"

**Correction:** Add an asterisk footnote:
> "DB + shim + MOK + SecurityOverride*"
> "*v0.8.3 known limitation: stock /boot on ext4 under SB requires UKI or kernel staged on ESP; Path G (v0.9.x) closes this."

### 3.8 `lamboot_architecture.html`

**Current claim at line 236:**
> "ShimRetainProtocol (u8) — asks shim to leave ShimLock installed across kernel boot"

**Correction:** Add a clarifying note:
> "ShimRetainProtocol (u8) — asks shim to leave `ShimLock` installed across kernel boot. **Effective on shim 16+ only;** shim 15.8 (current Ubuntu/Debian) does not read this variable. LamBoot still writes it so that stacks with newer shim benefit automatically; Path G (v0.9.x) eliminates the dependency."

### 3.9 `lamboot_innovations.html`

No change needed; the "why the trust log matters" argument remains fully valid. The new `kernel_load_failed` event — added as part of this finding — is itself an exemplar of the "LamBoot answers 'which step failed, and why'" promise, and should be mentioned on this page as a concrete example. Suggested addition after the existing "GRUB, systemd-boot, rEFInd, shim itself — none write a post-boot audit trail" paragraph:

> **Case in point:** in v0.8.3 testing on Ubuntu 25.10, LamBoot recorded a `kernel_load_failed` trust event with per-hook counters (`hook_security2 invocations=2, shim_validate calls=2 (true=1), ShimLock handle not found=1`) identifying shim 15.8's post-driver-load protocol uninstall as the precise cause. No other bootloader would have produced that evidence. The fix is Path G (v0.9.x); the ability to diagnose it precisely is already here.

### 3.10 `lamboot_tools.html`

No claim-correction required. The `status` / `autodetect` language is accurate.

---

## 4. New page content: `/products/lamboot/known-issues/`

Recommend creating a new sub-page `lamboot_known_issues.html` linked from the security page and the FAQ. Content:

> # Known Issues — v0.8.3
>
> ## Secure Boot + stock `/boot` on ext4
> **Symptom:** LamBoot menu shows the kernel, user selects it, kernel fails to load, firmware falls through to the distro's default bootloader.
>
> **Cause:** shim 15.8 (Ubuntu, Debian) uninstalls its `ShimLock` protocol after a UEFI filesystem driver (like LamBoot's ext4 driver) has finished loading. Without `ShimLock`, LamBoot can't verify a MOK-signed kernel, and firmware's DB-based fallback doesn't trust distro-Master-CA-signed kernels.
>
> **Evidence on your system:** check `\loader\boot-trust.log` on the ESP (mounted under `/boot/efi/loader/` in Linux). If you see a `kernel_load_failed` event with `ShimLock handle not found=1`, you're hitting this issue.
>
> **Workarounds for v0.8.3:**
> 1. Use a Unified Kernel Image (UKI) instead of separate `vmlinuz` + `initrd`. UKIs live at `\EFI\Linux\*.efi` on the ESP (FAT), no ext4 driver needed. systemd 253+ ships `systemd-ukify` to produce them.
> 2. Stage kernel + initrd on the ESP manually and point your BLS entries at `/EFI/LamBoot/kernels/…`. `lamboot-install --copy-kernel-to-esp` (v0.8.4) will automate this.
> 3. Disable Secure Boot for the VM / system. All LamBoot features except the SB verification chain still work.
>
> **Resolution:** v0.9.x Path G implements a native PE loader that verifies the kernel via `ShimLock` up front (while it's still installed) and then loads it without going back through firmware's `BS->LoadImage`, eliminating the dependency on post-driver-load `ShimLock` availability.

## 5. Structured-data additions

For `lamboot_faq.html` JSON-LD block, add a Q/A:

```json
{
  "@type": "Question",
  "name": "Does LamBoot work under Secure Boot today?",
  "acceptedAnswer": {
    "@type": "Answer",
    "text": "Yes, with caveats. SB-off works everywhere. SB with UKI on the ESP works today. SB with stock /boot on ext4 is a v0.9.x target (Path G) because shim 15.8 uninstalls ShimLock after LamBoot's UEFI filesystem driver loads. Full known-issues breakdown on the website."
  }
}
```

## 6. Linking and navigation

- Add "Known Issues" to `_lamboot_subnav.html` between "Security" and "Roadmap".
- Cross-link the Config 4 claim on the Security and Install pages to the new Known-Issues page.
- Add a prominent link to `/products/lamboot/known-issues/` in the FAQ "Does LamBoot work under Secure Boot?" answer.

## 7. Press-kit and boilerplate (from `docs/WEBSITE-CONTENT.md` §18)

Update the 100-word boilerplate to add the caveat:

> LamBoot is a memory-safe UEFI bootloader written in Rust, developed by Lamco Development and released under the MIT and Apache-2.0 dual license. It targets Proxmox virtualized environments, Linux homelabs, and security-conscious desktop users, and introduces the first structured trust-evidence log among mainstream bootloaders. LamBoot v0.8.3 ships the signing + shim/MOK integration and the JSON audit trail; Path G (v0.9.x) adds a native PE loader that closes a known v0.8.3 limitation where stock Linux `/boot` on ext4 cannot be verified under Secure Boot on shim 15.8.

## 8. What NOT to change

- **The honest-security framing** throughout the Security page — this is the differentiator and the whole point. Do not soften.
- **The comparison-matrix headline claims** (memory-safe Rust, trust-evidence log, GUI with mouse, BLS + UKI, Proxmox-native). All still true.
- **The "SB-off works fully" positioning.** This is the largest honest market for v0.8.3 and must stay front-and-center.
- **Pricing / licensing / contact / legal.** Unaffected.

## 9. Timing

These corrections must land **before** any public-facing push of v0.8.3 (GitHub release, website publish, announcement posts). The content is not privacy- or legal-sensitive; the concern is purely "don't publish claims the code can't back up." Timing:

1. Code changes are already in the private dev repo (commits `68682c0` onward).
2. This document ships as the v0.8.3 content brief amendment. Pair with the existing `docs/WEBSITE-CONTENT.md`.
3. Website copy changes are a LAMCO-website team task; recommend landing them before the public-repo push of `lamco-admin/lamboot`.
4. Announcement (HN / OMG!Linux / OMG!Ubuntu / X / LinkedIn) should reference the honest-limitations framing, not the over-sold "zero-touch" promise.

---

**End of corrections.** Pair with `docs/WEBSITE-CONTENT.md` (the original comprehensive brief) and `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` (root-cause narrative). When Path G ships in v0.9.x, re-amend.
