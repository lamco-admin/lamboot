# LamBoot Layer Architecture — Authoritative Model

**Version:** 0.8.3 (documented for the first time here; binding for v0.9.x and v1.0 development)
**Date:** 2026-04-21
**Audience:** LamBoot developers, architecture reviewers, SDS authors
**Status:** normative — every new module must declare its layer

---

## 1. Why this document exists

LamBoot has been developed with an implicit clean layering discipline from the start. As v1.0 work introduces new subsystems (native PE loader, native read-only FS backends, multi-FS abstraction), **the layering must be made explicit so new work respects it rather than eroding it**. This document codifies the existing architecture and establishes the dependency direction new code must follow.

**Rule:** higher-numbered layers may depend on lower-numbered layers. Never the reverse. A module that violates this is wrong, regardless of what it does.

## 2. The eight layers

### Layer 0 — Platform Introspection
Pure-read discovery of the environment we're running on. No side effects, no trust decisions, no user interaction.

| Module | Responsibility |
|---|---|
| `acpi.rs` | ACPI DMAR/IVRS parsing for IOMMU detection |
| `hypervisor.rs` | CPUID-based hypervisor detection |
| `smbios.rs` | SMBIOS table reading (VM identification, fleet tags) |
| `fw_cfg.rs` | QEMU fw_cfg device read access |

### Layer 1 — UEFI Firmware Boundary
Everything that touches UEFI protocols directly. Every firmware-facing call belongs here. No policy, no parsing, no UI.

| Module | Responsibility |
|---|---|
| `fs.rs` (ESP + volume enumeration only) | Mount ESP; enumerate `SimpleFileSystem` handles |
| `partitions.rs` | GPT walk, `PartitionInfo` protocol, XBOOTLDR mount |
| `tpm.rs` | TCG2 protocol for measured boot |
| `secure.rs` | Secure Boot state query from firmware |
| `drivers.rs` (legacy UEFI FS driver path) | `LoadImage`+`StartImage` for third-party FS drivers |
| `security_override.rs` | `Security2Arch`/`SecurityArch` protocol hooks |
| `initrd.rs` | `LINUX_EFI_INITRD_MEDIA_GUID` `LoadFile2` provider |

### Layer 2 — Block I/O + Filesystem Abstraction (new in v1.0)
A filesystem-agnostic read API on top of Layer 1. Consumers above this layer don't know if they're reading FAT, ext4, Btrfs, or anything else. **This layer is being introduced as part of Path G; existing modules are already aligned with what it would expose.**

| Module (planned) | Responsibility |
|---|---|
| `fs_backend.rs` (new) | Trait `FsBackend { read, exists, read_dir, metadata }` — the abstraction. Implementations plug in per FS. |
| `fs_backend_fat.rs` (new) | FAT adapter via `uefi-rs` SimpleFileSystem |
| `fs_backend_ext4.rs` (new) | ext4 adapter via `ext4-view` crate |
| `fs_backend_btrfs.rs` (future) | Btrfs adapter (v1.1+ community contribution) |
| `fs.rs` (refactored) | Becomes a coordinator that dispatches to the right backend per volume |

### Layer 3 — Content / Format Parsers
Pure parsers — bytes in, structured data out. No I/O, no firmware calls, no state.

| Module | Responsibility |
|---|---|
| `bls.rs` | Boot Loader Specification Type 1 entry parser |
| `uki.rs` | UKI PE section parser (reads `.osrel`, `.cmdline`, `.linux`) |
| `policy.rs` (parse half) | `policy.toml` parser |
| `pe_loader.rs` (new v1.0) | Native PE header parsing + section / relocation layout planning — on top of `goblin` |

### Layer 4 — Policy & State
Config-driven decisions and persistent state. Reads Layer 3 outputs + Layer 0/1 signals to make rules-based decisions.

| Module | Responsibility |
|---|---|
| `policy.rs` (policy-application half) | Applies policy to discovered entries (allowlist, denylist, fallback order) |
| `health.rs` | NVRAM state machine (Fresh→Booting→BootedOK/CrashLoop) |
| `autodiscovery.rs` | Entry-pipeline config |
| `preflight.rs` | Validates discovered entries (files present, signatures where applicable) |

### Layer 5 — Trust & Audit
Cross-cutting append-only record of decisions. Every layer above can record here; nothing depends on its state.

| Module | Responsibility |
|---|---|
| `trust_log.rs` | JSON trust-evidence log (`\loader\boot-trust.log`) |
| `bootlog.rs` | Persistent boot log (`\EFI\LamBoot\reports\boot.log`) |
| `report.rs` | Boot reports / audit log (`\EFI\LamBoot\reports\audit.log`) |
| `telemetry.rs` | Per-phase timing measurements |

### Layer 6 — Presentation
Everything the user sees or types. GUI + console + input dispatch.

| Module | Responsibility |
|---|---|
| `gui.rs` | GOP-based double-buffered menu, cursor, mouse support |
| `console.rs` | Serial/text-mode menu fallback |
| `input.rs` | Keyboard + pointer dispatch to GUI/console |

### Layer 7 — Orchestration
The conductor. Assembles the 10-phase boot flow from the layers below. **Nothing else depends on Layer 7.** It is the top.

| Module | Responsibility |
|---|---|
| `main.rs` | 10-phase boot orchestration |
| `discovery.rs` | Aggregates entries across backends (Layer 2 + Layer 3) |
| `boot.rs` | Chainload / UKI / native-PE / legacy-LoadImage dispatch |

## 3. Dependency rules (normative)

1. A module may `use` from its own layer or any lower layer.
2. A module **must not** `use` from a higher layer.
3. Cross-cutting concerns (Layer 5 Trust & Audit) are *written-to* from above (via a `&mut TrustLog` argument) but never *read-from* as state. There is no module that queries "what did the trust log say?" — the log is an output, not a control surface.
4. Layer 0 modules may not import from Layer 1+ (platform introspection must be pure).
5. Layer 2 (FS abstraction) imports from Layer 1 (UEFI block I/O) but not from Layer 3+. Layer 3 parsers do not perform I/O; they take byte slices.
6. Layer 7 (orchestration) is the only place phase-sequencing logic is allowed. Individual layer modules must not know the global phase.

## 4. Introducing new code — where does it go?

Decision tree for any new module:

1. **Does it touch UEFI protocols?** → Layer 1.
2. **Does it parse bytes without doing I/O?** → Layer 3.
3. **Does it read files via an FS-agnostic API?** → Layer 2 consumer; the adapter lives in Layer 2.
4. **Does it make a decision based on config + discovered state?** → Layer 4.
5. **Does it record a decision for audit?** → Write to Layer 5 via `&mut TrustLog`; don't ship a new module unless the record shape is structurally new.
6. **Does it draw pixels or read keystrokes?** → Layer 6.
7. **Does it schedule the boot phases?** → Layer 7 (`main.rs` — extend the existing phase enum, don't add a new orchestrator).

## 5. How Path G changes will land in this structure

Path G (v0.9.x → v1.0) adds:
- **Layer 2:** `fs_backend.rs` trait + `fs_backend_fat.rs` + `fs_backend_ext4.rs` — new modules, clean insertion.
- **Layer 3:** `pe_loader.rs` (native PE header + section/reloc planning) — new module, clean insertion.
- **Layer 1:** `security_override.rs` becomes *optional* — only needed for legacy UEFI FS driver compatibility; native path doesn't call `BS->LoadImage`.
- **Layer 7:** `boot.rs` gains a branch: native PE path (uses Layer 2 `ext4` backend + Layer 3 `pe_loader` + Layer 5 trust log + existing Layer 1 `initrd` provider).

Zero layer rule violations introduced. No upward dependencies. The filesystem abstraction lets `boot.rs` not care which FS the kernel lives on — it asks Layer 2, which dispatches to the right backend. `main.rs` still owns phase ordering.

This is why the architecture survives a big feature addition: the layers were already right.

## 6. Anti-patterns to refuse

Future reviewers should reject any PR that:

- Puts a UEFI protocol call outside Layer 1.
- Adds a "utility" or "helper" module that crosses layers.
- Has Layer 5 (Trust log) querying state to make a decision.
- Has Layer 4 policy code reading UEFI variables directly (go through Layer 1).
- Introduces phase-sequencing logic in a module other than `main.rs`.
- Creates an abstraction with only one implementation (single-impl traits) unless a second is imminent in the same PR series.

## 7. File naming conventions

- No generic suffixes: no `-manager`, `-helper`, `-utility`, `-common`, `-core` (except `lamboot-core/` the crate).
- Domain-specific verbs in function names.
- No helper modules.
- One responsibility per module; split if a module grows past ~500 lines and the responsibilities are separable.

## 8. Current module counts and layer totals

| Layer | Modules | LoC (approximate) |
|---|---|---|
| 0 — Platform | 4 | 1008 |
| 1 — UEFI | 7 | 1573 |
| 2 — FS abstraction | 1 (fs.rs) → 4+ planned | 249 → ~1200+ |
| 3 — Parsers | 3 → 4 planned | ~1114 → ~1800 |
| 4 — Policy | 4 | 1182 |
| 5 — Audit | 4 | 577 |
| 6 — UI | 3 | 1529 |
| 7 — Orchestration | 3 | ~1110 |
| **Total** | **28 → 36+** | **~8292 → ~12000** |

LamBoot is a medium-sized codebase by bootloader standards (GRUB ~40kLOC of C, systemd-boot ~10kLOC of C, rEFInd ~30kLOC of C++). It will remain smaller than all of them after Path G lands — a deliberate property, not an accident.

---

**This document is normative.** New modules must declare their layer in their module-level doc comment: `//! Layer: N — <name>. <one-line description>`. Reviews check the layer declaration first.
