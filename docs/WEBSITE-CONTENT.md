# LamBoot Website Content — Authoritative Brief for LAMCO-website

**Version:** 0.8.3
**Date:** 2026-04-21
**Audience:** LAMCO-website team (content population, page authoring, marketing assets)
**Source of truth:** this document. Product facts live here; copy and rephrase as needed.

---

## How to use this document

This is the **canonical content brief** for everything the LamBoot product occupies on the LAMCO website. It is designed to be mined directly: each section maps to a website surface (page, section, card, CTA block). Where there is product-truth ambiguity, this document wins — update it here first, then propagate to web copy.

**For the web team:**
- §1–§3 give you the product in 10 words, 30 words, 100 words — pick what each page needs.
- §4 is the canonical feature list. The website's feature cards should map 1:1.
- §5 is the competitive matrix. Use it for the Comparison page.
- §7 is the honest security story — this is the single biggest differentiator; do not soften it.
- §10 is the FAQ — drop into an accordion.
- §14 is the proposed page structure (routes, sections, CTA hierarchy).
- §15 is the taglines/headlines catalog for A/B variants.
- §16 is the assets inventory — what screenshots, diagrams, and downloads need to exist.
- §17 maps every LamBoot repo doc to a website URL.

Everything in this brief has been checked against the shipping v0.8.3 codebase. When in doubt, trust the code.

---

## 1. Elevator pitches (pick by surface)

### 10 words (nav subtitle / meta description)
> A memory-safe UEFI bootloader in Rust with honest Secure Boot.

### 30 words (hero subheading)
> LamBoot is a memory-safe UEFI bootloader written in Rust. It ships Secure Boot signing, shim+MOK integration, and the industry's first user-visible trust-evidence log — not just another GRUB replacement.

### 100 words (home-page lead paragraph)
> LamBoot is a memory-safe UEFI bootloader written in Rust, built for Proxmox VM fleets, homelabs, and security-conscious Linux desktops. It talks the same Boot Loader Specification language as `systemd-boot`, runs Linux via LoadFile2 like a modern loader should, but adds things no mainstream bootloader ships: a graphical menu with full mouse support, an NVRAM-based crash-loop state machine, loadable EFI filesystem drivers, pre-boot diagnostic modules — and most importantly, a JSON trust-evidence log written to the ESP on every boot that documents every image-authentication decision the firmware and bootloader made. When things go wrong, LamBoot tells you why.

### 250 words (About page opener)
> LamBoot was born from a specific frustration: UEFI Secure Boot is a complicated, opaque trust chain, and when it fails nothing tells you why. The error message is "access denied" — if you get one at all. The bootloaders that dominate Linux today — GRUB, systemd-boot, rEFInd — hide most of what the firmware did from the user, log almost nothing to persistent storage, and routinely bypass their own stated security policies to make things work. We wanted a bootloader that could be trusted because it was *auditable*, not because it claimed to be secure.
>
> LamBoot is memory-safe Rust top-to-bottom. It integrates the shim+MOK ecosystem the Linux world actually uses, but it does so honestly: every image-authentication decision — firmware DB accept, ShimLock accept, MOK cert accept, security override delegate, outright reject — is recorded to a structured JSON log on the EFI system partition, readable from the OS, correlatable with TPM event logs, useful for audit tools and humans alike. When a kernel won't boot, you can see exactly which check failed and why.
>
> Beyond the security story, LamBoot is practical. A 215 KB binary (compared to GRUB's 4 MB). A genuine graphical menu with mouse support. Proxmox host-side boot health monitoring via NVRAM variables readable from the hypervisor. Full BLS Type 1 support, Unified Kernel Images, boot counting compatible with `systemd-bless-boot`. It runs on x86_64 and aarch64 UEFI firmware. It's dual-licensed MIT or Apache-2.0.

---

## 2. Who it's for

| Audience | Pain point | How LamBoot helps |
|---|---|---|
| **Proxmox VM fleet operators** | Dozens of VMs, no visibility when one enters a boot loop. | Host-side `lamboot-monitor.py` reads NVRAM crash state from `OVMF_VARS` — no agent inside the VM. |
| **Homelab Linux enthusiasts** | GRUB configs break on kernel upgrades; no decent graphical menu. | BLS-native with `kernel-install` plugin, GUI with mouse, tiny binary. |
| **Security-conscious Linux desktop users** | Secure Boot works but is a black box — no way to audit what verified what. | Trust-evidence log. TPM measurement of kernel + cmdline + config. Honest documentation of what LamBoot does and does not defend. |
| **Rust / systems developers** | Want to read a bootloader that isn't 40k lines of 1990s C. | 3,500 lines of idiomatic Rust, `#![no_std]`, clear subsystem boundaries, documented unsafe blocks. |
| **Hypervisor integrators (long-term)** | Proxmox/QEMU lack a bootloader story beyond "install GRUB in the guest." | Bootloader-as-platform: Proxmox integration is first-class, not retrofitted. |
| **Audit / compliance teams** | Need evidence of which kernel signature verified and how. | Every boot writes `\loader\boot-trust.log` — structured JSON — plus TPM measurements on PCR 4/5/12. |

---

## 3. What LamBoot is NOT (expectation-setting)

To avoid wasting users' time:

- **Not a drop-in GRUB replacement for GRUB's entire feature surface.** No Btrfs snapshots menu. No GRUB scripting. No multi-boot exotica.
- **Not a Windows-only loader.** Primarily aimed at Linux; Windows is chainloaded via the ESP as any UEFI loader would.
- **Not shim-review-approved yet.** v0.8.3 ships with self-signed db keys or requires MOK enrollment for production use. Shim-review submission is on the v1.0 roadmap.
- **Not an enterprise vendor-supported product.** Community-supported, dual-licensed open source. Commercial support is a future conversation.
- **Not a firmware replacement.** It runs *on* UEFI firmware (EDK II, AMI, Phoenix, Insyde, etc.), it is not a coreboot alternative.
- **Not a rescue shell.** No interactive CLI. The GUI picks a boot entry; diagnostic modules are chainloaded EFI applications, not a command prompt.

---

## 4. Feature catalog (canonical)

### 4.1 Security & Trust

| Feature | Status | Description |
|---|---|---|
| **Trust-evidence log** | v0.8.3 | Structured JSON lines written to `\loader\boot-trust.log` on the ESP. Every image-authentication decision: firmware DB accept, ShimLock accept, MOK cert accept, SecurityOverride delegate, outright reject. Readable from the OS post-boot. No other mainstream bootloader ships this. |
| **Secure Boot (firmware DB)** | v0.8.3 | Binary signed with production db key. Bootable directly on systems that trust your db cert. |
| **Secure Boot (shim + MOK)** | v0.8.3 | `\EFI\LamBoot\shimx64.efi` → deployed as `grubx64.efi` → `lambootx64-signed.efi`. MokManager enrollment via bundled `.der` cert. Works on Ubuntu / Debian / Fedora. |
| **SecurityOverride (Path F)** | v0.8.3 | Rust port of the sd-boot pattern: hooks `EFI_SECURITY_ARCH_PROTOCOL` so shim 15.8's `ShimLock::Verify` gates driver and kernel loads even though upstream shim's Makefile no longer defines `OVERRIDE_SECURITY_POLICY`. Makes filesystem drivers loadable under Secure Boot. |
| **TPM measured boot** | v0.2.0+ | Measures kernel (PCR 4), policy (PCR 5), cmdline (PCR 12) via TCG2 protocol. Degrades gracefully when no TPM present. |
| **SBAT metadata** | v0.8.3 | Embedded via `llvm-objcopy` in PE `.sbat` section. Supports generation-based revocation. |
| **Production key hierarchy** | v0.8.3 | Documented RSA-4096 PK/KEK + RSA-2048 db (Debian #1013320 shim-MOK freeze bug workaround). Offline backup discipline documented. |
| **OVMF_VARS zero-touch SB** | v0.8.3 | Pre-enrolled VARS template for Proxmox/QEMU — no MokManager dance required. |
| **Honest threat model** | v0.8.3 | `docs/SECURITY-MODEL.md` documents what LamBoot defends vs. inherited ecosystem gaps (initrd integrity, cmdline injection, SEC/PEI compromise). |

### 4.2 Boot Management

| Feature | Status | Description |
|---|---|---|
| **Boot Loader Specification Type 1** | v0.2.0 | Full Type 1 parser with UAPI.10 version sort. `kernel-install` plugin for automatic entry creation. |
| **Unified Kernel Images (UKI)** | v0.8.3 | `\EFI\Linux\*.efi` first-class menu entries. PE parser reads `.osrel` for title. |
| **Linux boot via LoadFile2** | v0.2.0 | Proper initrd delivery using `LINUX_EFI_INITRD_MEDIA_GUID` (kernel 5.7+) — no legacy handoff protocol. |
| **Multi-partition volume enumeration** | v0.2.0 | Scans all `SimpleFileSystem` volumes: ESP, XBOOTLDR, filesystem-driver-exposed partitions. |
| **Loadable EFI filesystem drivers** | v0.2.0 | Ships ext4, btrfs, ntfs, xfs, zfs, f2fs, iso9660 drivers from the rEFInd project. Loaded under SecurityOverride on SB-enabled systems. |
| **Windows chainload** | v0.2.0 | `\EFI\Microsoft\Boot\bootmgfw.efi` detected automatically. |
| **Diagnostic modules** | v0.2.0 | `\EFI\LamBoot\modules\*.efi` — pre-boot EFI applications (NVMe SMART, memory test, PCI inventory, diag shell). Signed with the same key as the bootloader. |

### 4.3 Reliability & Recovery

| Feature | Status | Description |
|---|---|---|
| **NVRAM crash-loop state machine** | v0.2.0 | `Fresh → Booting → BootedOK`, counter-driven `CrashLoop` state. Automatic fallback entry selection after N failures. |
| **Boot counting** | v0.2.0 | systemd-compatible `+N[-M]` filename suffix handling. Works with `systemd-bless-boot`. |
| **BLS preflight validation** | v0.8.3 | Install script and bootloader both verify referenced kernel and initrd files exist before presenting an entry as bootable. |
| **Stale-BLS regeneration** | v0.8.3 | Install script detects BLS entries pointing to purged kernels and regenerates them. |
| **No-entries recovery screen** | v0.2.0 | GUI recovery menu with Reboot-to-Firmware, cold reboot, power-off actions. |

### 4.4 User Interface

| Feature | Status | Description |
|---|---|---|
| **Double-buffered GOP renderer** | v0.2.0 | Framebuffer composited off-screen; no flicker. |
| **Full mouse support** | v0.2.0 | UEFI Simple Pointer and Absolute Pointer protocols. Hover, click, scroll. Unique among Rust bootloaders. |
| **Keyboard navigation** | v0.2.0 | Arrows, Enter, F1 (help), F2 (firmware setup), F12 (cold reboot), Esc. |
| **Serial console fallback** | v0.2.0 | Full text menu when GOP unavailable (headless VMs, SOL). |
| **Trust-evidence panel (roadmap)** | v0.9.x | On-screen trust decisions during boot — Path G deliverable. |

### 4.5 Platform

| Feature | Status | Description |
|---|---|---|
| **x86_64 UEFI** | v0.2.0 | Primary target. Binary ~215 KB. |
| **aarch64 UEFI** | v0.2.0 | Secondary target. Binary ~200 KB. |
| **Memory-safe Rust** | v0.2.0 | `#![no_std]`, `#![no_main]`, explicit `unsafe` only for UEFI protocol access. Every `unsafe` block carries a `SAFETY:` comment. |
| **Tiny binary** | v0.2.0 | 215 KB x86_64 unsigned, 217 KB signed. Compare GRUB ~4 MB, systemd-boot ~300 KB, rEFInd ~1 MB. |
| **Dual MIT/Apache-2.0 license** | v0.2.0 | Standard Rust ecosystem dual license. |

### 4.6 Integration

| Feature | Status | Description |
|---|---|---|
| **Proxmox host-side monitor** | v0.2.0 | `lamboot-monitor.py` reads NVRAM crash state from `OVMF_VARS` files. JSON output, webhook alerts, per-VM filtering. No agent in the VM. |
| **`kernel-install` plugin** | v0.2.0 | `/usr/lib/kernel/install.d/90-lamboot.install` for automatic BLS entries on Debian/Ubuntu/Fedora. |
| **Kernel hooks (Debian/Ubuntu)** | v0.8.3 | `/etc/kernel/postinst.d/zz-lamboot` regenerates BLS entries after `apt install linux-image-*`. |
| **systemd Boot Loader Interface** | v0.2.0 | Sets `LoaderInfo`, `LoaderEntrySelected`, `LoaderBootCountPath`, enabling `systemd-bless-boot` and `bootctl status`. |

---

## 5. Competitive analysis

### 5.1 Headline matrix (one-screen comparison)

| | **LamBoot** | GRUB | systemd-boot | rEFInd | Clover | sd-boot-rs (Sprout) |
|---|---|---|---|---|---|---|
| **Language** | Rust | C | C | C++ | C++ | Rust |
| **Binary size** | **215 KB** | 4 MB | 300 KB | 1 MB | 2 MB | 200 KB |
| **Memory-safe** | **Yes** | No | No | No | No | Yes |
| **Graphical menu** | **Yes + mouse** | No | Text | Yes + icons | Yes | No |
| **BLS Type 1** | **Full** | Partial | Full | None | None | Partial |
| **UKI first-class** | **Yes** | Wrapper | Yes | No | No | No |
| **Crash-loop recovery** | **NVRAM state machine** | None | Boot counting | None | None | None |
| **Trust-evidence log** | **Yes (JSON)** | None | None | None | None | None |
| **Secure Boot** | **Firmware DB + shim + MOK + SecurityOverride** | Shim | Shim | Shim | No | Shim |
| **TPM measured boot** | **PCR 4/5/12** | None | PCR 4/5/8/12 | None | None | Roadmap |
| **Loadable FS drivers** | **Yes** | Built-in | FAT only | Yes | Yes | Yes |
| **Hypervisor integration** | **Proxmox first-class** | Generic | Generic | Generic | None | Cloud |
| **x86_64 + aarch64** | **Both** | Both | Both | x86_64 only | x86_64 only | Both |
| **License** | MIT OR Apache-2.0 | GPLv3 | LGPL-2.1+ | GPLv3 | CDDL | MIT |

### 5.2 Head-to-head narratives

**LamBoot vs. GRUB.** GRUB is the default almost everywhere and will be for years. It's also 40,000+ lines of C with a colorful CVE history, a binary 20× LamBoot's size, and a configuration language (`grub.cfg` generation, os-prober, `update-grub`) that is infamous for breaking silently on kernel upgrades. LamBoot is not trying to replicate GRUB's universal-hardware theatre; it is trying to be the *right* tool for people running modern BLS-compliant Linux, VMs, or UKIs, who want a bootloader they can read in a weekend and audit on every boot. If you need GRUB's legacy-BIOS support, PXE chainloading, Btrfs snapshot menu, or 40 filesystem drivers built in, stay on GRUB. If you're running UEFI, BLS, and kernel-install, LamBoot gives you the same power in 215 KB of Rust.

**LamBoot vs. systemd-boot.** `systemd-boot` (née gummiboot) is the closest philosophical cousin. Tiny, BLS-native, UEFI-only, minimal feature surface. Differences:
- **Language.** `systemd-boot` is still C. LamBoot is Rust — fewer memory safety footguns, easier to reason about.
- **GUI.** `systemd-boot` is text-mode only. LamBoot ships a proper graphical menu with mouse support for people who want it (serial console fallback for people who don't).
- **Trust-evidence log.** LamBoot writes structured audit evidence. `systemd-boot` does not.
- **Crash-loop recovery.** LamBoot's state machine is more explicit and exposes its state via NVRAM variables readable from the host (useful in hypervisor contexts). `systemd-boot` has `+N[-M]` boot counting but no persistent cross-boot state model.
- **SecurityOverride for driver loading.** Same mechanism both use; LamBoot's is documented and inspectable; `systemd-boot` treats it as internal plumbing.
- **Proxmox/hypervisor integration.** First-class in LamBoot (NVRAM variables the host can read via `OVMF_VARS`); absent in `systemd-boot`.

If you already love `systemd-boot` and don't need a GUI or audit log, stay on it. If you want the same minimal surface in a memory-safe language plus the hypervisor/audit story, come to LamBoot.

**LamBoot vs. rEFInd.** rEFInd is the go-to "pretty graphical bootloader" for UEFI. It's written in C++, is about 4× LamBoot's size, has been unmaintained-ish for stretches, and its security model is "sign the binary, enroll via MokManager, hope for the best." It does not write an audit log. It does not do BLS. Its filesystem driver collection is excellent (and LamBoot borrows from it — rEFInd's ext4/btrfs drivers ship with LamBoot under GPL-2.0 as separate binaries). If rEFInd's visuals and icon themes matter to you, stay there. If you want an auditable, BLS-native, memory-safe loader, switch.

**LamBoot vs. Clover / OpenCore.** These target a specific ecosystem (Hackintosh) and aren't really comparable. Mentioned for completeness.

**LamBoot vs. sd-boot-rs / Sprout.** Other Rust UEFI bootloaders exist. Sprout is cloud-focused and small; `sd-boot-rs` is a straight Rust port of `systemd-boot`. Neither ships a trust-evidence log, a full GUI, a crash-loop state machine, or Proxmox integration. Community is small across all three; pick based on the features that matter to you.

### 5.3 When *not* to choose LamBoot

Honesty sells. Add this to the Comparison page:
- **You need legacy BIOS support.** LamBoot is UEFI-only.
- **You depend on GRUB's os-prober Windows/Mac detection and menu.** LamBoot detects Windows but not arbitrary `EFI/*` vendors the way os-prober does.
- **You need a rescue shell with a command line.** LamBoot is menu-only by design.
- **You run a fleet already managed by shim-review-approved binaries.** LamBoot is not in the shim-review database yet. (Roadmap: v1.0.)
- **Your platform is not UEFI.** LamBoot does not implement coreboot or U-Boot handoff.

---

## 6. Deep architecture one-pager (for the Architecture page)

### Boot flow (10 phases, ordered)

```
UEFI Firmware → LamBoot EFI entry
  1. Assess boot health (NVRAM state machine)
  2. Detect Secure Boot state; initialise TPM (TCG2)
  3. Mount ESP; instantiate TrustLog
  4. Load policy.toml; measure into TPM PCR 5
  5. Request ShimRetainProtocol (v0.8.3+)
  6. Install SecurityOverride guard (Path F)
  7. Load filesystem drivers (\EFI\LamBoot\drivers\*.efi), record trust events
  8. Reconnect all controllers; enumerate all SimpleFileSystem volumes
  9. Discover entries: BLS entries (/loader/entries), ESP entries, UKI, modules
 10. Evaluate crash state; if CrashLoop, select fallback entry
 11. Display menu (GUI / serial), or auto-boot if configured
 12. Decrement boot counter; measure cmdline (PCR 12); measure kernel (PCR 4)
 13. Register initrd via LoadFile2 protocol
 14. Flush TrustLog to \loader\boot-trust.log
 15. Boot selected entry via uefi::boot::start_image under SecurityOverride
```

### Subsystem map

```
lamboot-core/src/
  main.rs              Orchestration — the 10-phase boot flow
  security_override.rs PATH F — Security/Security2 arch protocol hooks (Rust port of sd-boot)
  trust_log.rs         v0.8.3 — JSON-lines audit log on ESP
  bls.rs               BLS Type 1 parser, UAPI.10 version sort, boot counting
  gui.rs               Double-buffered framebuffer, VGA font, boot menu, mouse input
  discovery.rs         BLS-first entry discovery with ESP fallback, UKI detection
  policy.rs            Section-aware TOML parser for policy.toml
  health.rs            NVRAM state machine, Boot Loader Interface vars
  fs.rs                ESP/volume mounting, file I/O, multi-partition scan
  initrd.rs            LoadFile2 protocol provider for Linux initrd (LINUX_EFI_INITRD_MEDIA_GUID)
  boot.rs              Chainload, UKI, Linux boot under SecurityOverride
  input.rs             Keyboard + mouse input dispatch
  tpm.rs               TPM 2.0 measured boot (TCG2 protocol)
  console.rs           Serial/text console fallback menu
  drivers.rs           EFI filesystem driver loader, records trust events
  secure.rs            Secure Boot state detection + shim integration
  report.rs            Boot reports and audit logging with timestamps
```

### Trust-evidence log format

Each boot, LamBoot truncates `\loader\boot-trust.log` and writes one JSON object per line:

```json
{"seq":0,"event":"boot_start","path":"","size":0,"sha256":"","verified_via":"","status":"","note":"secure_boot=true"}
{"seq":1,"event":"driver_loaded","path":"\\EFI\\LamBoot\\drivers\\ext4_x64.efi","size":0,"sha256":"","verified_via":"shim_mok","status":"Success","note":""}
{"seq":2,"event":"boot_attempt","path":"\\EFI\\Linux\\ubuntu-6.11.efi","size":0,"sha256":"","verified_via":"firmware_db","status":"Success","note":"uki"}
```

Fields:
- `seq` — monotonic sequence within boot
- `event` — `boot_start | driver_loaded | driver_rejected | boot_attempt`
- `path` — EFI path (empty for `boot_start`)
- `verified_via` — `firmware_db | shim_mok | shim_vendor | security_override | rejected | sb_disabled`
- `status` — UEFI status name (`Success`, `AccessDenied`, `SecurityViolation`, …)
- `note` — free-form

Path G (v0.9.x) extends with per-image SHA-256, crypto-signed entries, policy records, and TPM event correlation.

### Four Secure Boot configurations

LamBoot supports four distinct trust postures, documented in `docs/SECURE-BOOT-DEPLOYMENT.md`:

1. **Config 1: Secure Boot off.** Development / homelab. Everything works, nothing is verified. Trust log records `sb_disabled`.
2. **Config 2: Secure Boot on, firmware DB trusts LamBoot cert.** Enterprise / internal CA. No shim needed. Simplest production deployment.
3. **Config 3: Secure Boot on, shim + MOK.** Standard Linux desktop deployment. LamBoot runs behind the distro shim; users enroll the LamBoot db cert into MOK via MokManager.
4. **Config 4: Zero-touch Proxmox.** Custom `OVMF_VARS` with LamBoot's db cert pre-enrolled. No MokManager dance. Used for VM templates at fleet scale.

### NVRAM variables

**LamBoot vendor GUID:** `4C414D42-4F4F-5400-0000-000000000001`

| Variable | Type | Purpose |
|---|---|---|
| `LamBootState` | u8 | 0=Fresh, 1=Booting, 2=BootedOK, 3=CrashLoop |
| `LamBootCrashCount` | u8 | Crash counter (resets on successful boot) |
| `LamBootLastEntry` | UTF-8 | ID of last booted entry |
| `LamBootTimestamp` | 8 bytes | Packed UTC y/m/d/h/m/s |
| `LamBootVersion` | u32 | Packed major.minor.patch |
| `ShimRetainProtocol` | u8 | Asks shim to leave ShimLock installed across kernel boot |

**Boot Loader Interface** (GUID `4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`):
`LoaderInfo`, `LoaderEntrySelected`, `LoaderBootCountPath`, `LoaderConfigTimeout`.

### TPM measurements

| PCR | What | Event type |
|---|---|---|
| 4 | Kernel image Authenticode hash | `EFI_BOOT_SERVICES_APPLICATION` |
| 5 | `policy.toml` content | `IPL` |
| 12 | Kernel command line (UTF-16) | `IPL` |

Gracefully degrades when TPM is absent.

---

## 7. The honest security story (this is the differentiator — DO NOT SOFTEN)

Every Linux bootloader page on every project's website lies a little bit about security. They say "Secure Boot: yes" and stop. The real picture is uncomfortable: shim 15.8's `OVERRIDE_SECURITY_POLICY` is undefined in upstream Makefile, making the `SecurityArch` hook dead code; shim's SBAT enforcement is asymmetric (different behaviour for in-protocol vs. direct loads); every major Linux bootloader (GRUB, systemd-boot, rEFInd, even shim itself) bypasses Secure Boot for kernel loads via `gBS->LoadImage` replacement or filesystem driver tricks or out-of-band verification.

LamBoot does not hide this. The website should not either.

### 7.1 What LamBoot defends against

| Threat | Defense |
|---|---|
| Unsigned `lambootx64.efi` replacement on ESP | Firmware DB / shim / MOK verification refuses to load |
| Unsigned filesystem driver substitution | SecurityOverride delegates to ShimLock; unsigned drivers are rejected and logged |
| Unsigned kernel substitution | Same — ShimLock-gated via SecurityOverride when the kernel loads |
| Policy-file tampering | Measured into PCR 5; remote attestation can detect |
| Cmdline injection at LamBoot layer | Measured into PCR 12; remote attestation can detect |
| Silent crash loops masking an attack | State machine surfaces to NVRAM readable by host monitoring |
| Unauditable trust decisions | Every decision recorded to `\loader\boot-trust.log` on the ESP |

### 7.2 What LamBoot does NOT defend against (inherited ecosystem gaps)

| Threat | Why not | Mitigation |
|---|---|---|
| Kernel binary swap post-verification | LoadFile2 is a protocol handoff; the kernel reads its own initrd/setup-header after LamBoot exits | Measured Boot + IMA inside the kernel; `ima-evm-utils` |
| Initrd content tampering | Initrd integrity is not covered by Secure Boot in any mainstream Linux distro | Use Unified Kernel Images (UKI) — LamBoot supports them first-class — or dm-verity / systemd-pcrphase |
| Firmware-level compromise (SEC / PEI) | Below the Secure Boot trust boundary | Platform manufacturer firmware updates; Intel Boot Guard / AMD PSB |
| GRUB-style cmdline injection via config file on writable ESP | ESP is typically FAT and writable from the OS | Lock down ESP mount; sign policy.toml (roadmap); measure cmdline to PCR 12 (already done) |
| Runtime SMM attacks | Below the OS trust boundary | SMM hardening is a firmware problem |

### 7.3 Why this honesty is the differentiator

Every other bootloader page treats Secure Boot as a checkbox. Ours treats it as a **system property with documented gaps and a plan**. That's what the security-conscious users LamBoot targets actually want to read. It's also the foundation for Path G — a custom PE loader that closes some of these gaps in-bootloader rather than punting to the kernel.

**Messaging rule for the website:** when someone asks "is LamBoot secure?", the answer is "it's more *auditable* than the alternatives — here's every decision it made last boot, here's what it can and cannot protect, here's the roadmap for closing the remaining gaps." That answer is rare and it's what buys credibility.

---

## 8. Use cases & scenarios (for Solutions / Use Cases pages)

### 8.1 Proxmox fleet operator

> Fifty Debian VMs, each with OVMF firmware. Yesterday two of them rebooted into a kernel that panics. In GRUB-land I'd SSH in, see nothing, eventually open the console and discover the initrd is wrong. With LamBoot, `lamboot-monitor.py` on the Proxmox host shows me the VM in `CrashLoop` state with crash count 3, flagged red. I know which VMs are broken before I've opened the console.

### 8.2 Security-conscious laptop user

> Dual-booted Ubuntu + Windows with Secure Boot enabled. I enrolled the LamBoot signing cert into MOK. Every boot, I can cat `/boot/efi/loader/boot-trust.log` and see the chain: LamBoot verified by firmware DB, ext4 driver verified by shim MOK, kernel verified by shim MOK. When my boot *fails*, there is always a line in that file telling me which step and why.

### 8.3 Rust systems developer curious about UEFI

> I've read chunks of the GRUB source. It is 40,000 lines of C with a custom build system. LamBoot is 3,500 lines of idiomatic Rust I can read in a long afternoon. Every `unsafe` block has a `SAFETY:` comment. The subsystem names are verbs and nouns, not managers and processors. If I wanted to add a new protocol, I know which file it goes in.

### 8.4 Homelab enthusiast tired of GRUB breaking

> `apt install linux-image-*` on Debian. Before: run `update-grub`, hope os-prober doesn't miss my Windows partition, pray nothing in `/etc/default/grub` conflicts with the new kernel's cmdline. After: the zz-lamboot kernel hook writes a BLS entry, LamBoot picks it up on the next boot, done. If I add a bad kernel, LamBoot's crash-loop state machine falls back to the last known-good one automatically.

### 8.5 Audit / compliance analyst

> We need to prove to an auditor that only signed kernels booted on a given machine over the last 30 days. With LamBoot, every boot appends to a trust log (v0.9.x will ship append-mode; v0.8.3 truncates per boot; both export to the host OS for log aggregation). Combined with TPM measurements on PCRs 4 / 5 / 12, we have cryptographic evidence plus human-readable narrative.

---

## 9. Installation walkthroughs (for the Install / Download page)

### 9.1 30-second install (Secure Boot off, homelab)

```bash
# Download release tarball from GitHub
curl -L -o lamboot.tar.gz \
  https://github.com/lamco-admin/lamboot/releases/download/v0.8.3/lamboot-0.8.3-x86_64.tar.gz
tar xzf lamboot.tar.gz && cd lamboot-0.8.3

# Install
sudo ./lamboot-install

# Reboot and pick "LamBoot" from the firmware boot menu
```

### 9.2 Secure Boot deployment (Ubuntu / Debian / Fedora)

```bash
# Install with signed binaries and MOK enrollment
sudo ./lamboot-install --signed

# You will be prompted to set a MOK enrollment passphrase.
# Reboot; in MokManager (blue screen) → Enroll MOK → Continue
# → enter passphrase → Yes → Reboot.
```

### 9.3 Proxmox zero-touch (VM template)

```bash
# On the Proxmox host — apply LamBoot-enrolled OVMF_VARS to a VM
cp lamboot-0.8.3/OVMF_VARS_lamboot.fd /var/lib/vz/images/100/OVMF_VARS_100.fd

# Inside the VM (first boot into Linux installer):
sudo ./lamboot-install --signed --no-mok   # cert already in firmware DB

# Convert VM to template. All clones boot LamBoot without MokManager.
```

### 9.4 Uninstall

```bash
sudo lamboot-install --remove
# Restores previous default boot entry, removes \EFI\LamBoot, cleans BLS entries
```

---

## 10. FAQ (accordion on product page)

**Q: Will LamBoot replace GRUB on my current Ubuntu install?**
A: No — LamBoot is added as a separate UEFI boot entry alongside GRUB. You can choose which to boot from the firmware boot menu (usually F12 / F11 / Esc at POST). `lamboot-install` never touches GRUB.

**Q: Does LamBoot work with Secure Boot?**
A: Yes — four different configurations (see §6). The simplest for Linux desktops is shim + MOK enrollment via `lamboot-install --signed`. The simplest for Proxmox VM fleets is the pre-enrolled `OVMF_VARS_lamboot.fd` template.

**Q: Is it shim-review approved?**
A: Not yet. v1.0 target. Today, LamBoot ships with self-signed db certs that users enroll via MOK (standard Linux practice) or via pre-populated OVMF VARS (Proxmox Config 4). When shim-review approves us, no user action is required — existing distro shim will chain-trust.

**Q: What's this "trust-evidence log" I keep hearing about?**
A: `\loader\boot-trust.log` on the EFI System Partition — a JSON-lines file listing every image-authentication decision LamBoot made during the current boot. No other mainstream bootloader ships this. It makes Secure Boot *auditable* rather than just claimed.

**Q: Why not just use systemd-boot?**
A: systemd-boot is an excellent bootloader. If you already run it and love it, stay. LamBoot is what you want if you also want: a memory-safe language, a GUI, an audit log, a Proxmox host-side monitoring story, and a maintainer who writes honest threat models.

**Q: Can I use LamBoot on non-UEFI (legacy BIOS) systems?**
A: No. Modern UEFI firmware (x86_64 or aarch64) only.

**Q: Does it support Btrfs snapshots / ZFS boot environments?**
A: It can *load kernels from* ext4 / btrfs / ZFS / xfs / ntfs / f2fs / iso9660 via the bundled EFI filesystem drivers. It does not present a Btrfs-snapshot-aware menu like `grub-btrfs`. A Btrfs-snapshot-aware discovery plugin is on the community wish list.

**Q: Is it production-ready?**
A: Yes for homelabs and controlled VM fleets. For enterprise-scale deployment with formal support SLAs, the commercial-support conversation hasn't started yet. The code has been running on developer machines and Proxmox VMs for months; v0.8.3 is the first release aimed at a broader audience.

**Q: What's the license?**
A: MIT OR Apache-2.0 (same dual license as most Rust ecosystem projects). Filesystem drivers shipped in `dist/EFI/LamBoot/drivers/` are GPL-2.0 from the rEFInd project and distributed as separate binaries per GPL linking exception.

**Q: Can I contribute?**
A: Yes — see `CONTRIBUTING.md` and `docs/DEVELOPER-GUIDE.md`. Good first issues: new filesystem driver packaging, distro-specific install script polish, diagnostic modules (memory tests, NVMe SMART, PCI enumeration).

**Q: Who's behind it?**
A: Lamco Development (a small vendor-neutral shop working on Linux tooling). LamBoot is one of several projects; others include a Wayland-native RDP stack. See the About page.

**Q: How do I report a security issue?**
A: See `SECURITY.md` — responsible disclosure via `security@lamco.io` with GPG key. Please do not file public issues for vulnerabilities.

---

## 11. Roadmap (public-facing — trim internal details)

### Shipping (v0.8.3, April 2026)
- Memory-safe UEFI bootloader in Rust (x86_64 + aarch64)
- Production signing pipeline with session-cached key unlock
- Shim 15.8 + MOK integration via SecurityOverride (Path F)
- Trust-evidence log v1 (JSON lines, truncate-per-boot)
- Four Secure Boot configurations documented
- Proxmox host-side monitoring
- BLS + UKI + kernel-install + kernel-hooks
- Full GUI with mouse, serial fallback

### Near-term (v0.9.x, Q2–Q3 2026)
- **Path G: own PE loader.** Stop relying on firmware-ambiguous `LoadImage`; inspect + verify PE ourselves. Closes the shim asymmetric-SBAT gap and several documented bypasses.
- **Trust-evidence log v2.** Per-image SHA-256, crypto-signed entries, TPM event correlation, append-mode across boots.
- **On-screen Trust Evidence Panel.** Show the trust decisions live in the boot UI.
- **Policy signing.** `policy.toml.sig` + `policy.toml.cert` so policy tampering fails closed.
- **Btrfs-snapshot discovery** (community-contributed, if a champion steps forward).
- **Post-quantum-ready signature scheme plumbing.** Dual-sign with Dilithium alongside RSA for forward-compatibility.

### Medium-term (v1.0, H2 2026)
- **Shim-review submission.** Distro-chained trust for Fedora / Ubuntu / Debian / Arch.
- **Documented SBAT generation bump policy.**
- **First commercial-support conversation** (if there's interest).

### Long-term (v1.x+, 2027)
- **Network boot.** UEFI TCP/HTTP stack, signed artefacts.
- **VM lifecycle integration with Lamco RDP / Proxmox stack.**
- **Audit-tool ecosystem** — tools that consume the trust-evidence log to produce fleet-wide boot-health reports.

### Explicitly **not** on roadmap
- BIOS legacy mode. UEFI only.
- Built-in rescue shell. Menu only.
- GRUB-style config language.

---

## 12. Licensing & governance

- **Code license:** `MIT OR Apache-2.0` (dual). Pick either. Top-level `LICENSE` file plus `LICENSE-MIT` and `LICENSE-APACHE`.
- **Bundled GPL binaries:** `dist/EFI/LamBoot/drivers/*.efi` are GPL-2.0 from the rEFInd project, distributed as separate binaries (GPL linking exception applies).
- **Contribution policy:** standard inbound=outbound (MIT/Apache-2.0 dual). Sign-off recommended but not required. See `CONTRIBUTING.md`.
- **Governance:** benevolent-maintainer model at v0.8.3; formal governance document when community size justifies it.
- **Trademark:** "LamBoot" and the LamBoot logo are claimed marks of Lamco Development. Source-code forks are welcome; name/logo usage for distribution requires notice.
- **Security disclosure:** 90-day responsible disclosure via `security@lamco.io`. See `SECURITY.md`.

---

## 13. Assets & screenshots inventory (what the site needs)

### Screenshots required
1. **Main boot menu (GUI, default theme)** — mouse cursor visible, one BLS entry and one UKI entry, TPM / SB indicator in corner.
2. **Main boot menu (serial console)** — same entries, text mode.
3. **No-entries recovery screen** — showing Reboot-to-Firmware / Cold Reboot / Power Off items.
4. **Crash-loop fallback** — "Previous boot failed; fell back to fedora-6.11.0" style banner.
5. **MokManager enrollment** — terminal + blue MokManager screen, step-by-step.
6. **Trust log extract** — terminal `cat /boot/efi/loader/boot-trust.log | jq .`, colorized JSON.
7. **Proxmox lamboot-monitor output** — the table with OK / WARN / CRIT rows.
8. **Install script dry-run output** — showing distro detection, shim path, BLS preflight.

### Diagrams required
1. **Boot-phase sequence diagram** (Firmware → LamBoot → Kernel handoff) with trust-evidence emission points.
2. **SecurityOverride hook diagram** — who calls whom in shim + firmware Security Arch protocol.
3. **NVRAM state machine** (Fresh → Booting → BootedOK / CrashLoop).
4. **Four-config Secure Boot matrix** — one-slide visualization.
5. **LamBoot vs. GRUB vs. systemd-boot** headline bar chart — binary size + feature count.

### Downloads required
1. `lamboot-0.8.3-x86_64.tar.gz` + `.sha256` (from `dist/` after `package-release.sh`).
2. `lamboot-signing.der` (public cert for MOK enrollment).
3. `OVMF_VARS_lamboot.fd` (Proxmox zero-touch template).
4. Linked: source tarball / Git tag `v0.8.3`.

### Videos (nice-to-have, not blocking)
1. 60-second install demo (Secure Boot off).
2. 2-minute Secure Boot + MOK enrollment walkthrough.
3. 90-second Proxmox `lamboot-monitor` demo.

---

## 14. Proposed website structure (page-by-page)

Existing LAMCO-website conventions: `src/website/templates/website/*.html`. Proposed LamBoot pages:

### 14.1 `/lamboot/` — Product home
- Hero: logo + tagline (§15) + 30-word pitch (§1)
- Three feature cards: Security honesty, Memory-safe, Proxmox-native
- CTA: "Get started" → `/lamboot/install/`, "View on GitHub" → repo
- Screenshots carousel (3 images from §13)
- Brief elevator (100-word pitch from §1)
- Secondary nav: Features, Install, Docs, Compare, FAQ, Roadmap

### 14.2 `/lamboot/features/` — Feature catalog
- Full feature matrix from §4 (collapse by category)
- Per-category short narrative

### 14.3 `/lamboot/security/` — Security & trust
- §7 rendered in full
- Embed example trust-log JSON (copy-able)
- Link: `docs/SECURITY-MODEL.md`

### 14.4 `/lamboot/install/` — Installation
- Tabbed: Homelab (SB off) / Linux desktop (shim + MOK) / Proxmox zero-touch
- Inline `lamboot-install` flags reference
- Download cards for tarball + `OVMF_VARS_lamboot.fd` + signing cert
- Link: MOK enrollment guide, OVMF VARS Proxmox guide

### 14.5 `/lamboot/compare/` — Against GRUB, systemd-boot, rEFInd, others
- §5 matrix and head-to-heads
- "When not to choose LamBoot" callout box

### 14.6 `/lamboot/architecture/` — Technical deep-dive
- §6 in full
- Link to each `docs/*.md` by subsystem

### 14.7 `/lamboot/proxmox/` — Proxmox integration
- The VM fleet narrative (§8.1)
- `lamboot-monitor.py` reference
- OVMF_VARS Config 4 walkthrough
- Link: `docs/PROXMOX-GUIDE.md`, `docs/OVMF-VARS-PROXMOX.md`

### 14.8 `/lamboot/roadmap/` — Roadmap
- §11 rendered
- Note: shim-review, Path G, Trust Evidence Panel

### 14.9 `/lamboot/docs/` — Documentation portal
- Index of all `docs/*.md` mapped to categories (see §17)
- Search via existing pagefind

### 14.10 `/lamboot/faq/` — FAQ
- §10 accordion

### 14.11 `/lamboot/developers/` — Developer guide
- Link to `docs/DEVELOPER-GUIDE.md` + `CONTRIBUTING.md`
- Architecture summary
- Repo links (dev repo is private; public mirror linked)
- "Good first issues" list

### 14.12 `/lamboot/download/` — Release artefact downloads
- Version dropdown (v0.8.3 is current)
- Tarball, sha256, signing cert, OVMF VARS
- Source via Git tag

### 14.13 Integration with existing `/about`, `/contact`, `/security`, `/developers` pages
- Add a LamBoot row to the existing product list
- Link from the existing `comparison.html` if there's a meta-comparison page

---

## 15. Marketing copy catalog (taglines, headlines, CTAs)

### Taglines (short, any of these can be the lock-up)
- "Memory-safe UEFI. Honest Secure Boot."
- "The bootloader that tells you why."
- "Auditable boot, by design."
- "A UEFI bootloader in Rust — and a trust log on every boot."
- "Small, safe, documented."

### Headline variants (hero section)
- **Technical:** "A memory-safe UEFI bootloader in 215 KB of Rust. With receipts."
- **Security-focused:** "Secure Boot is a black box. LamBoot opens it."
- **Proxmox-focused:** "See every VM's boot health from the Proxmox host. No agent required."
- **Dev-focused:** "3,500 lines of Rust. Every unsafe block documented. Read it in an afternoon."

### Sub-headlines (30–60 chars)
- "BLS-native. UKI-ready. Mouse-friendly."
- "Shim + MOK, without the guessing game."
- "Trust decisions, on-disk, every boot."

### Primary CTAs
- "Get LamBoot" → /lamboot/install/
- "See every feature" → /lamboot/features/
- "Read the threat model" → /lamboot/security/
- "Compare with GRUB" → /lamboot/compare/

### Secondary CTAs
- "Contribute on GitHub"
- "View the trust log schema"
- "Download signed release"

### Email / social short copy
- **Tweet / X:** "LamBoot v0.8.3: memory-safe UEFI bootloader in Rust. Honest Secure Boot with a JSON trust-evidence log written to the ESP every boot. No other mainstream bootloader ships this. MIT/Apache-2.0. <link>"
- **LinkedIn/HN title:** "LamBoot 0.8.3 — a Rust UEFI bootloader with a trust-evidence audit log"
- **Newsletter lead:** "Most bootloaders treat Secure Boot as a checkbox. LamBoot treats it as an auditable system property — every decision written to a JSON log on your ESP."

### "Why LamBoot" bullets (pick any three)
- 🦀 **Memory-safe Rust** — no more bootloader UAFs
- 📝 **Trust-evidence log** — every boot, every decision, on disk
- 📦 **215 KB binary** — 20× smaller than GRUB
- 🖱️ **Full graphical menu with mouse** — yes, even on servers
- 🏭 **Proxmox-native** — monitor VM boot health from the host
- ⚖️ **BLS + UKI + kernel-install** — Linux-first, done right
- 🔬 **Readable in an afternoon** — 3,500 lines, documented unsafe
- 🔐 **Four Secure Boot configurations** — from off to zero-touch enterprise

---

## 16. Objection-handling cheat-sheet (for sales / support / community engagement)

| Objection | Response |
|---|---|
| "GRUB already works." | "It does, for millions of users. If you are happy, stay. LamBoot is for people who want something smaller, safer, and auditable." |
| "systemd-boot is smaller." | "It is, by ~80 KB. LamBoot's extra size buys you a GUI, mouse, trust log, and crash-state machine. If you don't want those, systemd-boot is the right answer." |
| "Why should I trust a new bootloader?" | "Read it. It's 3,500 lines of Rust. Every `unsafe` block has a `SAFETY:` comment. Every boot writes a log of what it did. That's a stronger claim to trust than *any* mainstream bootloader makes." |
| "I can't get Secure Boot to work." | "Four documented configurations with walkthroughs. If you're stuck, the trust-evidence log on your ESP has the exact failure reason. Send it to us." |
| "Is Rust suitable for firmware?" | "Yes. Linux kernel merged Rust for drivers. Windows is writing firmware in Rust. `uefi-rs` is mature and LamBoot exercises it in production-like scenarios." |
| "Shim isn't in shim-review for LamBoot — that's disqualifying." | "Fair. Today MOK enrollment or firmware-DB trust replaces shim-review approval; both are standard Linux practice. v1.0 target is shim-review." |
| "Our auditor needs a SOC 2 vendor, not open source." | "LamBoot is open source; Lamco Development is not a SOC 2 vendor today. That conversation starts when there is revenue. Today LamBoot is a tool, not a vendor relationship." |

---

## 17. Documentation map (for `/lamboot/docs/` and internal linking)

| Category | Doc | URL suggestion |
|---|---|---|
| Overview | `README.md` | /lamboot/ |
| Quickstart | `QUICKSTART.md` | /lamboot/install/quickstart/ |
| Install reference | `docs/INSTALL-REFERENCE.md` | /lamboot/install/reference/ |
| User guide | `docs/USER-GUIDE.md` | /lamboot/docs/user-guide/ |
| Architecture | `docs/ARCHITECTURE.md` | /lamboot/architecture/ |
| Configuration | `docs/CONFIGURATION-GUIDE.md` | /lamboot/docs/configuration/ |
| Security model | `docs/SECURITY-MODEL.md` | /lamboot/security/ |
| Secure Boot deployment | `docs/SECURE-BOOT-DEPLOYMENT.md` | /lamboot/security/deployment/ |
| Key generation | `docs/KEY-GENERATION.md` | /lamboot/security/keys/ |
| MOK enrollment | `docs/MOK-ENROLLMENT-GUIDE.md` | /lamboot/security/mok/ |
| OVMF VARS (Proxmox) | `docs/OVMF-VARS-PROXMOX.md` | /lamboot/proxmox/ovmf/ |
| Proxmox guide | `docs/PROXMOX-GUIDE.md` | /lamboot/proxmox/ |
| Troubleshooting | `docs/TROUBLESHOOTING-GUIDE.md` | /lamboot/docs/troubleshooting/ |
| Tools overview | `docs/LAMBOOT-TOOLS-OVERVIEW.md` | /lamboot/docs/tools/ |
| Diagnostic modules | `docs/DIAGNOSTIC-MODULES.md` | /lamboot/docs/modules/ |
| Roadmap | `docs/ROADMAP.md` | /lamboot/roadmap/ |
| Implementation plan | `docs/IMPLEMENTATION-PLAN.md` | (internal — do not publish) |
| Ecosystem deep-dive | `docs/analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md` | /lamboot/docs/analysis/ecosystem/ |
| Changelog | `CHANGELOG.md` | /lamboot/changelog/ |
| Contributing | `CONTRIBUTING.md` | /lamboot/developers/contributing/ |
| Security policy | `SECURITY.md` | /lamboot/security-policy/ |
| Licenses | `LICENSE-MIT`, `LICENSE-APACHE` | /lamboot/license/ |

**Internal-only (do NOT publish):** `IPV6-INFRASTRUCTURE.md`, `STATUS-*.md`, `docs/STATUS-*.md`, `docs/analysis/README.md`, any `docs/specs/SPEC-*.md` pre-v1.0.

---

## 18. Press kit (for the Press / Media page)

### Boilerplate (100 words)
> LamBoot is a memory-safe UEFI bootloader written in Rust, developed by Lamco Development and released under the MIT and Apache-2.0 dual license. It targets Proxmox virtualized environments, Linux homelabs, and security-conscious desktop users, and introduces the first structured trust-evidence log among mainstream bootloaders — a JSON audit file written to the EFI System Partition on every boot documenting every image-authentication decision made during the boot process. LamBoot integrates with the standard Linux shim and MOK ecosystem, supports Boot Loader Specification entries and Unified Kernel Images, and provides host-side VM boot-health monitoring without requiring an in-guest agent.

### Key facts table
- **Current version:** 0.8.3 (April 2026)
- **License:** MIT OR Apache-2.0
- **Languages:** Rust (100% of bootloader core)
- **Binary size:** 215 KB (x86_64 unsigned), 217 KB (signed)
- **Platforms:** x86_64 UEFI, aarch64 UEFI
- **Distro support:** Ubuntu, Debian, Fedora (validated); others untested but codepath-equivalent
- **Repository:** github.com/lamco-admin/lamboot (public)
- **Dev repository:** github.com/lamco-admin/lamboot-dev (private)
- **Maintainer:** Lamco Development
- **Contact:** `office@lamco.io`

### Logo & mark usage
- Logo files: `src/website/static/images/lamboot/logo-*.svg` (to be produced)
- Primary colors: TBD — web team to specify from LAMCO brand
- Minimum clear space: 1x logo-height
- Usage: editorial / fork / derivative-work distribution requires "LamBoot is a trademark of Lamco Development" notice

---

## 19. Technical SEO / metadata recommendations

- **Title tag** (home): "LamBoot — Memory-safe UEFI Bootloader in Rust · Lamco Development"
- **Meta description:** "LamBoot is a memory-safe UEFI bootloader written in Rust, with honest Secure Boot integration and a trust-evidence audit log. BLS, UKI, Proxmox-native. MIT/Apache-2.0."
- **Open Graph image:** large render of main GUI menu with `boot-trust.log` JSON overlay
- **Canonical target keywords:** "rust uefi bootloader", "memory safe bootloader", "secure boot audit log", "bootloader with mouse", "proxmox bootloader monitoring"
- **Schema.org:** `SoftwareApplication` with `applicationCategory=DeveloperApplication`, `operatingSystem=UEFI`
- **robots.txt:** default allow; `/lamboot/internal/` (if any) disallow

---

## 20. Pricing & business model (for future Pricing page — currently N/A)

v0.8.3 is free / open source. There is no paid tier today.

**Conversation-starters for when there is revenue:**
- Enterprise support contract (24/7 incident, SLA)
- Custom shim-review submission for enterprise self-signed fleets
- Proprietary integrations with non-Proxmox hypervisors
- Vendor-provided signed release builds for regulated environments

**Current status:** no commercial offering. Any revenue conversation routes to `office@lamco.io`.

---

## 21. Change log (for `/lamboot/changelog/`)

Source of truth: `CHANGELOG.md` in the repo. Mirror to site on every release.

### v0.8.3 (2026-04-21) — Secure Boot Release

**Headline features:**
- Production Secure Boot signing pipeline with session-cached key unlock
- SecurityOverride (Path F) — shim 15.8 + MOK integration for driver and kernel loading
- Trust-evidence log v1 — JSON lines on `\loader\boot-trust.log`
- Four documented Secure Boot configurations (off, firmware DB, shim + MOK, Proxmox zero-touch)
- Install script: `--signed` / `--no-shim` / `--no-mok` flags, MOK enrollment, shim chain deployment
- Kernel hooks for Debian/Ubuntu + kernel-install plugin for Fedora
- BLS lifecycle: stale-entry detection + regeneration, preflight validation
- UKI first-class menu entries

**Documentation:**
- `SECURITY.md` (responsible disclosure policy)
- `docs/SECURITY-MODEL.md` (honest threat model)
- `docs/SECURE-BOOT-DEPLOYMENT.md`, `docs/KEY-GENERATION.md`, `docs/MOK-ENROLLMENT-GUIDE.md`, `docs/OVMF-VARS-PROXMOX.md`
- `docs/analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md`

**Validated on:** Ubuntu 25.10, Debian forky/sid, Proxmox OVMF 4.2025.02-4

**Documented limitations:** Shim-review not yet approved (v1.0 target). Fedora Config 3 full MokManager dance deferred to post-release fleet-sweep.

### v0.2.0 and earlier
See `CHANGELOG.md` and Git tag history.

---

## 22. Contact & community

- **General inquiries:** `office@lamco.io`
- **Security disclosure:** `security@lamco.io` (GPG key in `SECURITY.md`)
- **Public repository:** github.com/lamco-admin/lamboot
- **Issue tracker:** public repo Issues
- **Discussions:** GitHub Discussions (on public repo)
- **IRC / Matrix:** none yet — open to setting up when community justifies it

---

## 23. Appendix: content-maintenance rules for the web team

1. **Do not soften §7.** The honest security story is the differentiator. Any rewrite that removes the "here's what we don't defend" table is wrong.
2. **Always link back to `docs/*.md`** — the deep technical content lives in the repo. The website summarizes and directs.
3. **Version numbers must match `Cargo.toml` `version`.** When there's a new release, update `v0.8.3` → new version across this document and on the web pages. A version-mismatch audit should run in CI.
4. **Keep the comparison matrix honest.** When a competitor ships a feature LamBoot lists as unique, update the matrix the same week.
5. **The trust-evidence log JSON example (§6) must match the actual schema in `lamboot-core/src/trust_log.rs`.** When the schema evolves (Path G v0.9.x), update this document and the web copy simultaneously.
6. **Do not publish docs listed as internal in §17.**

---

**End of brief.**

For any clarification, the source-of-truth files in this repo are:
- `README.md` (product overview, features)
- `docs/SECURITY-MODEL.md` (security honesty)
- `docs/SECURE-BOOT-DEPLOYMENT.md` (install configurations)
- `docs/ARCHITECTURE.md` (boot flow)
- `CHANGELOG.md` (release history)
- `lamboot-core/src/trust_log.rs` (schema)
