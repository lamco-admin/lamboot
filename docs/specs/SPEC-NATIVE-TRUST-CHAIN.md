# SPEC-NATIVE-TRUST-CHAIN: Trust Chain Under Native Boot

**SDS number:** SDS-4 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/trust_log.rs` (extended event schema), `lamboot-core/src/security_override.rs` (narrowed scope — kernel-load path no longer invokes it), `lamboot-core/src/boot.rs` (new verify+load sequence), `docs/SECURITY-MODEL.md` (amended), `docs/WEBSITE-CONTENT.md` (amended §7)
**Depends on:** SDS-1 (`FsBackend` tag in trust events), SDS-3 (native PE loader that sits after verification) MUST be landed first; SDS-2 (ext4 backend) as secondary dependency for one-pass-trust test scenarios.
**Blocks:** SDS-6 (UEFI FS Driver Deprecation — references the native chain as the replacement), future website + press claims (this SDS is the authoritative source).
**External dependencies:** none (`ShimLock` protocol access already in `uefi-rs` ≥ 0.37).

---

## 1. Overview

This specification defines LamBoot's trust chain under the native boot path (SDS-2 native ext4 + SDS-3 native PE loader). It specifies **what is verified, by whom, when, with what key material, and how every decision is recorded in the trust-evidence log** — such that the security claims LamBoot makes on its website, in its README, and to auditors can be traced back to exactly this document and the code it governs.

### 1.1 Positioning statement

This is the authoritative source for LamBoot's native-path security narrative. Every claim — "verified once," "read-only /boot," "auditable decisions" — lives here first. When marketing copy or user-facing documentation describes the trust chain, it must quote or link to this SDS. When the chain changes (v0.9.x → v1.0, v1.0 → v1.1), this SDS is amended, and consumers of it are updated in lockstep.

**If this SDS does not document a security property, LamBoot does not have that property.** No unstated assumptions. No marketing-ahead-of-code.

### 1.2 Why a separate SDS

Each of SDS-1 through SDS-3 focuses on a single technical subsystem and touches trust only at its boundary. The trust chain CROSSES all of them — it starts in firmware, passes through shim, enters LamBoot, is carried through SDS-1's `FsBackend`-source-tagging, consumes SDS-2's ext4 bytes, and terminates in SDS-3's `start_image` call. A technical-per-subsystem spec cannot adequately describe a property that lives in the seams. Hence this document.

### 1.3 What this SDS delivers

- **A canonical trust-chain diagram** showing every step from firmware power-on to kernel entry-point, with verification points labelled.
- **Per-step verification policy** describing who performs the check, against what data, using which key material, and what outcomes are possible.
- **Trust-log event schema v2** extending the v0.8.3 schema to carry per-step provenance (source backend, SHA-256, verifier identity, outcome).
- **Degraded-trust narratives** for each failure mode (shim absent, ShimLock refused, firmware DB only, SB off).
- **The authoritative what-LamBoot-does-NOT-verify list** inheriting the honest-gaps section of `docs/SECURITY-MODEL.md` and extending it with new items specific to the native path.
- **A website-claims appendix** enumerating every security claim LamBoot is permitted to make publicly, keyed to the code path that backs it.
- **Marketing discipline rules** ensuring this SDS and the live website cannot drift.

### 1.4 What this SDS does NOT deliver

- **Implementation of ShimLock::Verify calls.** Already exists in `lamboot-core/src/security_override.rs`. This SDS reshapes WHEN that function is invoked but does not reimplement it.
- **A new verifier.** We continue to rely on shim's `ShimLock::Verify` for kernel trust in v1.0. LamBoot does NOT ship its own PE authenticode verifier, does NOT parse MOK entries, does NOT maintain its own certificate store.
- **Post-quantum signing.** Referenced as a future v2.x research item; not in this SDS's scope.
- **Offline / detached signature verification.** No `.sig` sidecar handling in v1.0.
- **A secondary Verifier trait abstracting between ShimLock and alternatives.** YAGNI for v1.0. If v1.1+ adds a second verifier, introduce the abstraction then.
- **Boot-service vs runtime-service trust distinctions beyond the single handoff.** Once LamBoot calls `ExitBootServices`-equivalent (which actually happens in the kernel after SDS-3's `start_image`), LamBoot has no further trust assertions.

### 1.5 Constraints

All SDS-1 through SDS-3 constraints apply. Additionally:

- Every trust-chain decision MUST produce a trust-log event. No silent verifications.
- The trust log MUST remain append-only within a boot (v0.9.0 inherits the flush-preserving behavior from v0.8.3's `trust_log.rs` cumulative-flush fix).
- Trust-log events MUST NOT contain the byte contents of any verified image — only the SHA-256 digest. Keeps the log small and audit-safe.
- Trust-log events MUST NOT omit failures. A boot that successfully reaches a kernel with trust evaluated MUST have the event stream documenting it; a failure MUST produce an event describing what failed.
- Changes to the event schema (§6) are BREAKING for any external log consumer and require a major-version bump.

---

## 2. The trust chain — canonical diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 0: Firmware power-on                                                   │
│  Verifier:   Platform firmware                                               │
│  Verifies:   Itself (Intel Boot Guard / AMD PSB if present — OUT OF SCOPE)  │
│  Trust log:  not emitted (pre-LamBoot)                                       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 1: Firmware loads shim                                                 │
│  Verifier:   Firmware SecureBoot (EFI_SECURITY2_ARCH_PROTOCOL)               │
│  Against:    Firmware DB (distro-shim signed by Microsoft UEFI CA 2011)     │
│  Trust log:  not emitted by LamBoot (pre-LamBoot)                            │
│  Failure:    Firmware drops to fallback; LamBoot never runs                  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 2: Shim verifies LamBoot and loads it                                  │
│  Verifier:   shim (invokes MokListRT lookup, MokListXRT revocation, and/or  │
│              shim's vendor-cert chain)                                       │
│  Against:    Lamco db.crt enrolled via MOK OR Lamco db.crt in firmware DB    │
│  Trust log:  FIRST LamBoot trust-log event: `boot_start` with sb=ActiveWith │
│              Shim (or ActiveDirect, Disabled, etc.) per secure::detect_     │
│              secure_boot()                                                   │
│  Failure:    Shim refuses to StartImage; LamBoot never reaches main         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 3: LamBoot acquires ShimLock protocol handle (once, early)             │
│  Verifier:   N/A (handle acquisition, not verification)                      │
│  Trust log:  `shimlock_acquired` event with shim-version string if readable  │
│  Failure:    If ShimLock is not installed (e.g. shim not in chain, SB off),  │
│              LamBoot records `shimlock_absent` and proceeds in degraded-    │
│              trust mode (§5)                                                 │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 4: LamBoot requests ShimRetainProtocol NVRAM var (no-op on shim 15.8) │
│  Verifier:   N/A                                                             │
│  Trust log:  `shim_retain_requested` if var set                              │
│  Failure:    Best-effort; var write failure produces a warn, not a failure   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 5: LamBoot reads its own policy.toml (from ESP / FAT backend)          │
│  Verifier:   TPM measurement (if available) into PCR 5                       │
│  Trust log:  `policy_loaded` event with SHA-256 of policy bytes              │
│  Failure:    If policy parse fails, use defaults + log `policy_invalid`      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 6: LamBoot enumerates volumes via SDS-1 dispatch                       │
│  Verifier:   N/A                                                             │
│  Trust log:  one `volume_mounted` event per volume with backend tag +       │
│              fs_uuid + partition_guid                                        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 7: LamBoot discovers boot entries (BLS + UKI + chainload)              │
│  Verifier:   N/A (file existence, not trust)                                 │
│  Trust log:  `entries_discovered` event with counts per volume               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 8: User selects (or timeout auto-selects) an entry                     │
│  Verifier:   N/A                                                             │
│  Trust log:  `entry_selected` event with entry id                            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 9: LamBoot reads kernel bytes via Volume::read (FsBackend-dispatched) │
│  Verifier:   N/A (byte sourcing, not trust)                                  │
│  Trust log:  `kernel_bytes_read` event with path + size + backend_tag       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 10: ★ LamBoot invokes ShimLock::Verify(bytes) ★ (THE verify step)     │
│  Verifier:   shim (MokListRT / vendor-cert / SbatLevelRT)                    │
│  Against:    Kernel's embedded signature (authenticode)                      │
│  Trust log:  `image_verified` event with:                                    │
│    - path                                                                    │
│    - size                                                                    │
│    - sha256 (of the bytes, pre-verify)                                       │
│    - verified_via = "shim_mok" | "shim_vendor" | "shim_sbat_rejected" |    │
│                     "firmware_db_fallback" | "degraded_trust_sb_off"        │
│    - verifier_tag = "shim-15.8" (read at best-effort)                       │
│    - status = "SUCCESS" | "REJECTED"                                         │
│  Failure:    If REJECTED, emit event, return to menu, do NOT proceed to     │
│              step 11                                                         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 11: LamBoot measures kernel into TPM PCR 4                             │
│  Verifier:   N/A (measurement, not verification)                             │
│  Trust log:  `kernel_measured` event with PCR 4 extend value                 │
│  Failure:    If no TCG2 protocol, skip (log `tpm_absent`)                    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 12: LamBoot measures cmdline into TPM PCR 12                           │
│  Trust log:  `cmdline_measured` event with PCR 12 extend value               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 13: LamBoot loads kernel via SDS-3 native PE loader                    │
│  Verifier:   N/A (already verified at step 10)                               │
│  Trust log:  `image_loaded_native` event with:                               │
│    - path                                                                    │
│    - size                                                                    │
│    - sha256 (MUST MATCH step 10's sha256 — invariant check)                  │
│    - entry_point_rva                                                         │
│    - relocation_applied (bool)                                               │
│    - image_base (for audit correlation)                                      │
│  Failure:    Any PeLoadError produces `image_load_failed` with error token   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 14: LamBoot registers initrd via LoadFile2 protocol                    │
│  Verifier:   N/A                                                             │
│  Trust log:  `initrd_registered` event with concatenated initrd SHA-256      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 15: LamBoot flushes trust log to ESP (write-through)                   │
│  This is the last point the trust log can capture before handoff.            │
│  Everything from here on is kernel responsibility (dm-verity, IMA, etc.)     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 16: LamBoot calls pe_loader::start_image → kernel entry                │
│  LamBoot is done. Kernel is running.                                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

The **★ verify step at Step 10 ★** is load-bearing. It is the ONE place LamBoot performs a trust check on the kernel. After Step 10, the bytes' trust status is cached in the `image_verified` trust-log event and in LamBoot's in-memory state. SDS-3's PE loader consumes those same bytes (no re-read) and asserts the SHA-256 invariant at Step 13. Firmware's `Security2Arch` hook is never invoked for the kernel — we bypassed that entirely.

---

## 3. What happens at each step — detailed

### 3.1 Step 10 in depth — the single verify point

```rust
// Pseudocode for the verify step (actual implementation in boot.rs).

fn verify_kernel_bytes(
    bytes: &[u8],
    path: &Path,
    backend_tag: &'static str,
    trust_log: &mut TrustLog,
) -> Result<VerifiedBytes, VerifyError> {
    let sha256 = sha256::hash(bytes);
    let verify_result = shim_lock::verify(bytes);

    let (verified_via, status) = match verify_result {
        Ok(Trust::Mok) => ("shim_mok", "SUCCESS"),
        Ok(Trust::Vendor) => ("shim_vendor", "SUCCESS"),
        Err(ShimError::SbatRejected) => ("shim_sbat_rejected", "REJECTED"),
        Err(ShimError::NotEnrolled) => ("shim_not_enrolled", "REJECTED"),
        Err(ShimError::Absent) => {
            // ShimLock protocol was not present at acquire time (step 3).
            // Fall back to per-policy decision: firmware DB attempted, or
            // degraded-trust for SB-off.
            fall_back_verify(bytes, &sha256, &secure_boot_state())?
        }
        Err(e) => return Err(VerifyError::ShimOther(e)),
    };

    trust_log.record(TrustEvent::new("image_verified")
        .with_path(path.as_str())
        .with_size(bytes.len())
        .with_sha256_hex(&hex::encode(sha256))
        .with_verified_via(verified_via)
        .with_verifier_tag(shim_version_string())
        .with_status(status));

    match status {
        "SUCCESS" => Ok(VerifiedBytes { bytes, sha256, verified_via }),
        _ => Err(VerifyError::Rejected(verified_via)),
    }
}
```

The `VerifiedBytes` carries the SHA-256 forward so Step 13 can assert the invariant. The `verified_via` tag is propagated all the way to the `image_loaded_native` event (Step 13).

### 3.2 Degraded-trust fallback (Step 10, ShimLock absent)

If ShimLock is not present at step 3, LamBoot captures `shimlock_absent` and enters **degraded-trust mode**. The degraded-trust policy is:

| Secure Boot state | LamBoot in degraded mode does… |
|---|---|
| Disabled | Accept any bytes (log `degraded_trust_sb_off`); TPM measurements still happen |
| Enabled, firmware DB signs LamBoot directly (Config 2/4 without shim) | Attempt firmware-DB verify via `EFI_SECURITY2_ARCH_PROTOCOL` delegation. Accept → `firmware_db_fallback`. Reject → load fails. |
| Enabled with shim but ShimLock somehow uninstalled mid-boot | Same as above — firmware DB fallback; if kernel's cert isn't in DB, fail |

In ALL cases, the trust-log event records the exact posture. An auditor reading `\loader\boot-trust.log` can always tell whether the boot went through shim, went through firmware DB, or skipped verification because SB was off.

### 3.3 Where SecurityOverride still applies — scope narrowed

v0.8.3 used `security_override.rs` to hook `EFI_SECURITY2_ARCH_PROTOCOL` so that `uefi::boot::load_image(kernel_bytes)` could be routed through `ShimLock::Verify`. This is **no longer needed for the kernel load** because SDS-3's native PE loader doesn't call `BS->LoadImage` at all.

SecurityOverride still applies to:

1. **UEFI filesystem driver loads** (when native backends don't cover a filesystem — btrfs, xfs, zfs, f2fs). In v0.9.0, this covers the legacy `\EFI\LamBoot\drivers\*.efi` path that SDS-6 is deprecating.
2. **Diagnostic module loads** (`\EFI\LamBoot\modules\*.efi`). We could migrate these to the native loader too, but diagnostic modules are signed by the same key as LamBoot itself so firmware DB verification works fine. SDS-3's native loader will eventually replace this path; not a v0.9.0 deliverable.

The `security_override.rs` module stays but is flagged in code comments as "narrow-scope: legacy UEFI driver loads only." SDS-6 will eventually remove it.

### 3.4 TPM measurements are not trust (Steps 11, 12)

PCR measurements are **evidence for external verifiers** (remote attestation), not LamBoot-side trust decisions. LamBoot never reads its own PCR values to gate anything. An auditor comparing PCRs to expected reference values is a legitimate trust model; LamBoot's role is to produce the measurements honestly, not to consume them.

### 3.5 Initrd is NOT verified (Step 14)

This is an **inherited ecosystem gap**. Linux's Secure Boot model does not cover initrd integrity. LamBoot reads the initrd bytes, SHA-256s them into the trust log (for audit), concatenates them per ordering, and hands to the kernel via LoadFile2. No signature check.

**The correct solution is UKI** — Unified Kernel Images embed the initrd in a signed PE container, so Step 10's `ShimLock::Verify` on the UKI covers the initrd. LamBoot supports UKI; for non-UKI boots, we inherit the gap and document it.

This is named explicitly in the website-claims appendix (§8) and in `SECURITY-MODEL.md`.

---

## 4. What LamBoot's native chain does NOT do (the honest-gaps list)

Inherits v0.8.3's honest-gaps list and extends it.

| Threat | Defended? | Why not | Mitigation path |
|---|---|---|---|
| Unsigned `lambootx64.efi` replacement on ESP | **Yes** | firmware DB + shim + MOK | — |
| Unsigned kernel substitution | **Yes (if SB on + UKI or ShimLock available)** | Step 10 rejects | — |
| **Initrd tampering (non-UKI)** | **No** | Linux SB doesn't cover initrd | Use UKI. Document on website. |
| Kernel cmdline injection after boot | **Measured into PCR 12** but not prevented | cmdline comes from policy.toml / BLS entry → LamBoot doesn't modify it; user-level tamper of BLS file possible | Sign policy.toml (roadmap v0.9.x §4.2 of `NATIVE-FS-AND-PE-LOADER-STRATEGY`); measured into PCR 12 for attestation |
| Pre-firmware compromise (SEC / PEI) | **No** | Below SB boundary | Platform firmware updates; Intel Boot Guard / AMD PSB |
| Post-kernel compromise (kernel CVE, rootkit) | **No** | Kernel's problem | dm-verity, IMA, etc. |
| Runtime SMM attacks | **No** | Below OS boundary | Firmware hardening |
| Shim MOK database tamper (attacker with root) | **Partial** | MokListRT in runtime NVRAM is writable by root by design; measured into PCR 14 | Post-root-compromise is already game-over |
| Bootloader supply chain compromise | **Partial** | Source on GitHub, tarball SHA-256 in release, build reproducibility TBD | Reproducible builds (roadmap); signed source tarballs |
| **NEW — ext4-view CVE exploited by crafted filesystem** | **Depends on CVE** | We depend on the crate's correctness | CVE response procedure in SDS-2 §11.3 |
| **NEW — PE loader bug exploited by crafted kernel** | **Pure-Rust loader reduces surface but can't eliminate** | Native loader is 600 LOC of our code; fuzzed 30 min minimum per release | Continuous fuzzing; if exploited, patch + security release per SECURITY.md |
| **NEW — Trust-log forgery** | **Not protected** | Log is written by LamBoot itself before handoff; post-handoff writes are kernel-controlled | Path G v1.x: crypto-sign each log entry with a LamBoot ephemeral key |

---

## 5. Degraded-trust modes — full enumeration

For every Secure Boot state × LamBoot-load-path combination, what's in the trust log:

| SB | LamBoot loaded via | Kernel source | Step 10 verifier | `verified_via` token | Acceptable? |
|---|---|---|---|---|---|
| Off | Any | Any | — (skipped) | `degraded_trust_sb_off` | Homelab only |
| On | shim (MOK) | ext4 /boot via SDS-2 | ShimLock::Verify | `shim_mok` | **v1.0 target** |
| On | shim (MOK) | ESP (UKI) via FAT | ShimLock::Verify on UKI | `shim_mok` | v0.8.3+ |
| On | Firmware DB direct, no shim | ext4 /boot via SDS-2 | Firmware DB fallback | `firmware_db_fallback` | Only if kernel DB-signed (rare) |
| On | Firmware DB direct, no shim | self-signed UKI with cert in DB | Firmware DB fallback | `firmware_db_fallback` | v0.8.3+ |
| On | shim 15.8 | ext4 /boot via UEFI driver | ShimLock::Verify — but protocol gone | `shim_absent_after_driver_load` → degraded | **BROKEN in v0.8.3**; fixed in v1.0 via SDS-2+SDS-3 |
| On | shim 16+ | any | ShimLock::Verify (shim-native) | `shim_mok` | Future — when Ubuntu/Debian adopt shim 16 |

The `verified_via` tokens are a STABLE vocabulary — any tool consuming trust logs may match on these strings. §6.2 lists the complete set.

---

## 6. Trust-log event schema v2

### 6.1 New and changed events (v0.9.0)

| Event | New or changed? | Emitted by |
|---|---|---|
| `boot_start` | unchanged | `main.rs` |
| `volume_mounted` | NEW | `partitions.rs::mount_all_volumes` per volume |
| `shimlock_acquired` | NEW | `security_override::request_shim_retain_protocol` + new acquire |
| `shimlock_absent` | NEW | same, else branch |
| `shim_retain_requested` | NEW | `security_override::request_shim_retain_protocol` on success |
| `policy_loaded` | NEW (replaces v0.8.3 implicit TPM measure) | `main.rs` Phase 4 |
| `policy_invalid` | NEW | `main.rs` Phase 4 error branch |
| `entries_discovered` | NEW | `discovery.rs` after enumeration |
| `entry_selected` | NEW | after menu or timeout |
| `kernel_bytes_read` | NEW | `boot.rs` before verify |
| `image_verified` | NEW (the load-bearing verify event) | `boot.rs::verify_kernel_bytes` |
| `kernel_measured` | NEW | `boot.rs` after TPM PCR 4 extend |
| `cmdline_measured` | NEW | `boot.rs` after TPM PCR 12 extend |
| `image_loaded_native` | NEW (introduced in SDS-3 §12; spec'd here) | `boot.rs` after `pe_loader::load_pe` |
| `image_load_failed` | changed (fields extended) | `boot.rs` on PeLoadError |
| `initrd_registered` | NEW | `initrd.rs::register` |
| `driver_loaded` | unchanged (legacy path) | `drivers.rs` |
| `driver_rejected` | unchanged | `drivers.rs` |
| `boot_attempt` | changed (fields extended) | `main.rs` before `boot_entry` |
| `kernel_load_failed` | unchanged from v0.8.3 fix (commit 988851b predecessor) | `main.rs` on boot_entry Err |
| `tpm_absent` | NEW | `boot.rs` when TCG2 not available |

### 6.2 Fields per event

```json
// image_verified — THE load-bearing event. Every native-path boot produces exactly one.
{
  "seq": 42,
  "event": "image_verified",
  "path": "/boot/vmlinuz-6.19.12-200.fc43.x86_64",
  "size": 18225512,
  "sha256": "3df266502d2443ed68a609a0b7e3febf0d37fca52deb704de15637990e8dea82",
  "verified_via": "shim_mok",
  "verifier_tag": "shim-15.8-0ubuntu2",
  "status": "SUCCESS",
  "note": ""
}

// image_loaded_native — SDS-3 produces data; SDS-4 formalizes the event schema here.
{
  "seq": 43,
  "event": "image_loaded_native",
  "path": "/boot/vmlinuz-6.19.12-200.fc43.x86_64",
  "size": 18225512,
  "sha256": "3df266502d2443ed68a609a0b7e3febf0d37fca52deb704de15637990e8dea82",
  "verified_via": "shim_mok",
  "verifier_tag": "",
  "status": "SUCCESS",
  "note": "entry_rva=0x1000 base=0xffffffff80000000 rebased=false pages=4449"
}

// volume_mounted
{
  "seq": 5,
  "event": "volume_mounted",
  "path": "",
  "size": 0,
  "sha256": "",
  "verified_via": "",
  "verifier_tag": "",
  "status": "SUCCESS",
  "note": "backend=ext4-view@0.9.3 fs_uuid=0f80db14-9d69-4c89-911f-1f4a55717c3d partition_guid=cd2d2f09-... index=1 label="
}
```

### 6.3 Stable vocabulary — `verified_via` values (v0.9.0)

| Token | Meaning |
|---|---|
| `shim_mok` | `ShimLock::Verify` accepted via MOK-enrolled certificate (normal distro boot) |
| `shim_vendor` | `ShimLock::Verify` accepted via shim's built-in vendor cert (RHEL/Fedora path) |
| `shim_sbat_rejected` | `ShimLock::Verify` rejected on SBAT level (generation-based revocation) |
| `shim_not_enrolled` | `ShimLock::Verify` rejected; signing cert not in any shim trust store |
| `shim_absent_after_driver_load` | ShimLock gone mid-boot (documented v0.8.3 bug, should not occur in v1.0) |
| `firmware_db_fallback` | Fell back to firmware `EFI_SECURITY2_ARCH_PROTOCOL`; success (cert in DB) |
| `firmware_db_rejected` | Firmware DB fallback returned ACCESS_DENIED |
| `degraded_trust_sb_off` | Secure Boot disabled; no verification attempted |
| `security_override` | Legacy v0.8.x path: `SecurityOverride` hook invoked (transient, SDS-6 deprecates) |
| `rejected` | Catch-all rejected; see note field for details |
| `sb_disabled` | Historical v0.8.3 token, preserved for back-compat |

Consumers of the trust log MAY match on any of these. They are SEMVER-STABLE: adding new tokens is additive and non-breaking; removing or renaming existing tokens is a major-version event.

### 6.4 Invariant: SHA-256 match across Step 10 and Step 13

Any boot that produces `image_verified` SHOULD also produce `image_loaded_native` with the **same SHA-256 field**. A mismatch indicates a TOCTOU bug (bytes changed between verify and load), which is a security regression and must be caught in tests. SDS-3 §13.1 unit test 19 asserts this invariant.

### 6.5 Log rotation / retention

Unchanged from v0.8.3: log is truncated on each boot's first flush and appended thereafter (cumulative-flush fix from `trust_log.rs`). Consumers wanting cross-boot history must read + copy post-boot from the OS side.

**Future (v1.x):** Append-mode log across multiple boots, with per-entry signature from a LamBoot ephemeral key, with rotation at size threshold. Not a v0.9.0 or v1.0 deliverable.

---

## 7. Integration with existing modules

### 7.1 `main.rs`

Phase ordering already matches §2's step diagram. SDS-4 mostly adds `trust_log.record(...)` calls at specific points (steps 3-8 above). No logic changes.

### 7.2 `boot.rs`

The kernel-boot path gains a dedicated verify-then-load function:

```rust
pub(crate) fn boot_linux_native(
    volume: &mut Volume,
    kernel_path: &Path,
    initrd_paths: &[&Path],
    cmdline: &str,
    tpm: &TpmContext,
    trust_log: &mut TrustLog,
) -> Result<!, BootError> {
    // Step 9: read kernel bytes
    let bytes = volume.read(kernel_path)?;
    trust_log.record(TrustEvent::new("kernel_bytes_read")
        .with_path(kernel_path.as_str())
        .with_size(bytes.len())
        .with_verified_via(volume.identity().backend_tag)
        .with_status("SUCCESS"));

    // Step 10: verify
    let verified = verify_kernel_bytes(&bytes, kernel_path, volume.identity().backend_tag, trust_log)?;

    // Step 11: measure kernel
    tpm.measure_kernel(&bytes);
    trust_log.record(TrustEvent::new("kernel_measured").with_note("pcr4"));

    // Step 12: measure cmdline
    tpm.measure_cmdline(cmdline);
    trust_log.record(TrustEvent::new("cmdline_measured").with_note("pcr12"));

    // Step 13: native load
    let loaded = pe_loader::load_pe(&bytes, Some(&cmdline_to_u16(cmdline)),
                                    volume.identity().backend_tag)?;

    // Invariant check
    debug_assert_eq!(loaded.sha256_hex(), hex::encode(verified.sha256));

    trust_log.record(TrustEvent::new("image_loaded_native")
        .with_path(kernel_path.as_str())
        .with_size(bytes.len())
        .with_sha256_hex(&loaded.sha256_hex())
        .with_verified_via(verified.verified_via)
        .with_status("SUCCESS")
        .with_note(&format!("entry_rva=0x{:x} pages={}",
                            loaded.entry_point_rva(),
                            loaded.pages_allocated)));

    // Step 14: initrd
    let initrd_handle = initrd::register_from_paths(volume, initrd_paths, trust_log)?;

    // Step 15: flush
    trust_log.flush(&mut fat_esp_writer()?);

    // Step 16: start
    // SAFETY: loaded.bytes are shim-verified; no further trust check needed.
    let status = unsafe { pe_loader::start_image(loaded) };
    drop(initrd_handle);
    // start_image returning is abnormal (kernel usually ExitBootServices'es).
    Err(BootError::KernelReturned(status))
}
```

The old `boot::boot_linux` (v0.8.3 firmware-LoadImage path) stays as a fallback for configurations where native load is not preferred (UEFI FS driver-dependent non-ext4 filesystems, policy opt-out).

### 7.3 `security_override.rs`

Narrow scope to legacy UEFI FS driver loads only. Kernel-load path no longer reaches it. Annotate clearly in module doc comment.

### 7.4 `trust_log.rs`

Gains an extended event builder with convenience methods for the new fields (`with_sha256_hex`, `with_verifier_tag`). No structural changes to the log file format — all new fields live in the existing `note` field or are top-level strings already in the schema.

### 7.5 `secure.rs`

The `detect_secure_boot()` function is extended to return richer state per §5: `ActiveWithShim` vs `ActiveDirect` vs `Disabled`. The value is consumed by `boot_start` event.

---

## 8. Website-claims appendix — what LamBoot may say publicly

This appendix IS the API that marketing copy consumes. Claims not on this list are not backed by code.

### 8.1 Permitted claims (v1.0)

| Claim | Code path backing it | Log-grep proof |
|---|---|---|
| "LamBoot verifies every kernel via shim before loading it (under Secure Boot with stock Linux)." | `boot.rs::verify_kernel_bytes` step 10 | `grep '"event":"image_verified"' boot-trust.log` |
| "LamBoot writes a JSON audit log to the ESP recording every image-authentication decision." | `trust_log.rs` + §6 schema | The file itself |
| "LamBoot does not modify `/boot`. Ever." | SDS-1 §6.4 type-system constraint (EspWriter is FAT-only by type) | Static: grep for any ext4 write path returns zero hits |
| "LamBoot's native boot reads your `/boot` via a pure-Rust read-only library (ext4-view)." | SDS-2 §4 + §8 | `volume_mounted` event with `backend=ext4-view@...` |
| "LamBoot's PE loader is ~600 lines of memory-safe Rust. Every `unsafe` block is documented." | SDS-3 §3 + §11 | `rg unsafe lamboot-core/src/pe_loader.rs` + `rg SAFETY: lamboot-core/src/pe_loader.rs` |
| "Kernels are verified once and loaded natively — firmware's LoadImage is never called for your kernel." | SDS-3 §1.1 + step 13 | `image_verified` followed by `image_loaded_native` in the log; no intervening firmware-call events |
| "LamBoot measures your kernel, cmdline, and policy into TPM PCRs 4, 12, and 5." | `tpm.rs` + steps 5/11/12 | `kernel_measured`, `cmdline_measured`, `policy_loaded` events |
| "LamBoot's trust log includes SHA-256 of every verified and loaded image." | §6.2 field spec | `sha256` field in every relevant event |
| "If Secure Boot is off, LamBoot says so in the log. No silent acceptance." | §5 `degraded_trust_sb_off` | `verified_via=degraded_trust_sb_off` events |

### 8.2 Prohibited claims (until backing code exists)

- "LamBoot verifies kernels without shim." **FALSE.** We depend on shim's `ShimLock::Verify` for MOK-chained certificates. Removing shim dependency requires a LamBoot-native MOK parser (roadmap v1.x, not v1.0).
- "LamBoot verifies initrd integrity." **FALSE for non-UKI boots.** Inherited ecosystem gap (§4). UKI boots inherit verification through the signed UKI container.
- "LamBoot's PE loader has been formally verified." **FALSE.** Tested, fuzzed, reviewed. Not formally proven. Don't claim what hasn't been done.
- "LamBoot is shim-review approved." **Until approved — FALSE.** Submission is a v1.0+ parallel track.
- "LamBoot is post-quantum ready." **FALSE.** Research posture, no shipping code in v1.0.
- "LamBoot protects against runtime tampering." **FALSE.** Boot-time-only posture.

### 8.3 Marketing-discipline rules

1. Any public claim not in §8.1 must be added HERE FIRST (with its code-path backing) before going into README / website / announcements.
2. If §8.1 changes (claim added or refined), the corresponding code-path reference must be updated in the same commit.
3. Marketing copy authors reference this document by URL (`/docs/specs/SPEC-NATIVE-TRUST-CHAIN.md#8-website-claims-appendix`) when drafting.
4. If a user disputes a claim, the resolution process is: open the trust log on their system, grep for the backing event, confirm or refute.

---

## 9. Test plan

### 9.1 Unit tests (mock shim + mock TPM)

1. `verify_kernel_bytes` with mock shim returning `Trust::Mok` → `VerifiedBytes` with `verified_via="shim_mok"`, event emitted with `status="SUCCESS"`.
2. Same with `Trust::Vendor` → `verified_via="shim_vendor"`.
3. `ShimError::SbatRejected` → `VerifyError::Rejected("shim_sbat_rejected")`, event emitted with `status="REJECTED"`.
4. `ShimError::Absent` with SB on + firmware DB accepting → `verified_via="firmware_db_fallback"`.
5. `ShimError::Absent` with SB off → `verified_via="degraded_trust_sb_off"`, `status="SUCCESS"`.
6. SHA-256 invariant: `verify_kernel_bytes`'s SHA-256 matches the SHA-256 in the subsequent `image_loaded_native` event.
7. TPM absent → `tpm_absent` event emitted; verify step still proceeds.
8. Trust-log flush preserves order: events appear in the log file in the same order `TrustLog::record` was called.

### 9.2 Integration tests (QEMU + real shim)

1. Boot VM 201 (Fedora, SB off) via LamBoot native path → trust log contains `degraded_trust_sb_off`, kernel boots, all expected events in order.
2. Boot VM 120 (Ubuntu, SB on, shim 15.8) via LamBoot native path → trust log contains `shim_mok` at Step 10, kernel loads via SDS-3 native loader, no `image_load_failed`.
3. Boot a crafted bad-signature kernel → `image_verified` with `status="REJECTED"`, no `image_loaded_native`, boot returns to menu.

### 9.3 Marketing-discipline test

A CI job `tools/verify-claims.sh` that:
1. Parses §8.1 of this SDS for each claim + code-path reference.
2. Confirms the referenced file/function exists and contains expected symbols.
3. Fails the build if any reference is stale.

Keeps the claims appendix honest as the code evolves.

---

## 10. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| ShimLock::Verify behavior varies across shim versions | Medium | Record `verifier_tag=shim-X.Y.Z` in every event; test on shim 15.8 (Ubuntu/Debian current) and shim 16+ (Fedora/RHEL recent). |
| Step 13 SHA-256 invariant violation indicates TOCTOU bug | High if triggered | Debug-assert in boot.rs; CI fuzzing to ensure no legitimate cause. |
| Log schema v2 breaks external consumers of v0.8.3 logs | Low | v0.8.3 schema is subset of v0.9.0; all v0.8.3 tokens preserved. Consumers targeting only v0.8.3 fields continue to work. |
| TPM measurements emit events even when TPM is absent | Low | `tpm_absent` event is emitted once; subsequent measure calls are no-ops without further events. |
| Degraded-trust mode accepted by users who didn't intend it | Medium | `degraded_trust_sb_off` is loud in the log; website explains the posture; install script's SB-off message warns. |
| Marketing claims drift ahead of code again | Medium (already happened once with Config 4) | §8.3 rules + CI check per §9.3. |
| SHA-256 overhead at every kernel load | Low | ~80 ms for 30 MB kernel (§SDS-3 §14). Worth it for audit. |

---

## 11. Acceptance criteria

SDS-4 implementation is complete when all of the following are true:

- [ ] `lamboot-core/src/trust_log.rs` supports all §6.1 events with all §6.2 fields.
- [ ] `lamboot-core/src/boot.rs::boot_linux_native` implements the §7.2 flow with Step 10 verification + Step 13 invariant check.
- [ ] `lamboot-core/src/secure.rs::detect_secure_boot` returns the extended state from §5.
- [ ] `lamboot-core/src/security_override.rs` scope narrowed to legacy UEFI FS driver loads only; module comment explicitly states this.
- [ ] Every §8.1 claim has a working code-path reference that §9.3 `tools/verify-claims.sh` validates.
- [ ] §8.2 prohibited claims grep-fails: `rg "shim-review approved"` / `rg "post-quantum"` on user-facing docs return zero false-positive matches.
- [ ] `docs/SECURITY-MODEL.md` is amended to cite this SDS as authoritative and match §4's honest-gaps table.
- [ ] `docs/WEBSITE-CONTENT.md` §7 (the "honest security story") is updated to reference §8 claims list.
- [ ] All §9.1 unit tests pass.
- [ ] All §9.2 integration tests pass.
- [ ] §9.3 marketing-discipline CI job runs and passes.
- [ ] `CHANGELOG.md` entry under Unreleased section.
- [ ] No `#[allow(clippy::…)]` introduced; any `unsafe` has SAFETY comment.
- [ ] Pre-commit hook passes.

---

## 12. Sign-off

This spec is **implementation-ready** when:
- Founder has reviewed and explicitly acknowledged.
- SDS-1, SDS-2, SDS-3 have landed in code.
- No open question in §10 above "Medium" severity.

Implementation PR(s) must cite this SDS and must not add public claims to README / website / announcements without amending §8.1 first.

---

## Appendix A — Canonical `verified_via` vocabulary

(Reproduced from §6.3 for convenience; this is the authoritative list.)

```
shim_mok                       — ShimLock accepted via MOK cert
shim_vendor                    — ShimLock accepted via shim vendor cert
shim_sbat_rejected             — ShimLock rejected on SBAT revocation
shim_not_enrolled              — ShimLock rejected; cert not enrolled
shim_absent_after_driver_load  — ShimLock disappeared mid-boot (v0.8.3 bug)
firmware_db_fallback           — Firmware DB Security2Arch accepted
firmware_db_rejected           — Firmware DB returned ACCESS_DENIED
degraded_trust_sb_off          — SB off; no verify attempted
security_override              — Legacy SecurityOverride hook (v0.8.x, SDS-6 deprecates)
rejected                       — Catch-all; details in note
sb_disabled                    — Historical v0.8.3 token; preserved
```

## Appendix B — References

- `docs/specs/SPEC-FS-BACKEND-TRAIT.md` — SDS-1, backend tags used in trust events.
- `docs/specs/SPEC-EXT4-INTEGRATION.md` — SDS-2, ext4-view@0.9.3 tag.
- `docs/specs/SPEC-NATIVE-PE-LOADER.md` — SDS-3, the `image_loaded_native` producer + SHA invariant source.
- `docs/SECURITY-MODEL.md` — honest-gaps list this SDS extends.
- `docs/WEBSITE-CONTENT.md` §7 — "honest security story" anchor.
- `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` — v0.8.3 shim-15.8 ShimLock-uninstall issue resolved by this chain.
- `docs/analysis/UEFI-SECURITY-ECOSYSTEM-DEEP-DIVE-2026-04-21.md` §9 — ecosystem-bypass catalog.
- Trust-log source: `lamboot-core/src/trust_log.rs`.
