# LamBoot Layer Architecture — Authoritative Model

**Version:** 0.13.0
**Date:** 2026-06-01
**Audience:** LamBoot developers, architecture reviewers, SDS authors
**Status:** normative — every module declares its layer, and the contract is
mechanically enforced by `tools/check-layers.py` (run in the pre-commit hook
and in CI).

---

## 1. Why this document exists

LamBoot is organized into eight dependency layers. Every module declares its
layer in a module-level doc comment, and dependencies flow one direction:
toward the firmware boundary. **This is enforced, not aspirational** —
`tools/layer-map.toml` is the machine-readable source of truth and
`tools/check-layers.py` fails any build where a module lacks its declaration or
imports a higher layer.

**Rule:** higher-numbered layers may depend on lower-numbered layers. Never the
reverse. A module that violates this fails the CI gate.

> **History (v0.13.0).** Earlier revisions of this document described a planned
> structure that the code had outgrown: it listed ~28 modules, placed `fs.rs`
> at Layer 1, named `bls.rs` as the Layer-3 pure parser, and asserted the
> declaration mechanism without any enforcement. None of that was true of the
> shipped code. v0.13.0 reconciled the two: the shared boot-entry types were
> extracted into `boot_types.rs`, `select_default_entry` moved to the policy
> layer, every module got its `//! Layer:` declaration, the map was corrected
> to match reality (`fs.rs` is Layer 2; `bls_parse.rs` is the pure parser;
> `bls.rs` is a Layer-4 coordinator), and the whole thing was put under a CI
> gate. The graph is now a verified acyclic DAG.

## 2. The eight layers

The map of every module to its layer lives in `tools/layer-map.toml`. The
layers, low to high:

### Layer 0 — Platform Introspection
Pure-read discovery of the environment. No side effects, no trust decisions.
`acpi`, `hypervisor`, `smbios`, `fw_cfg`, `fw_cfg_config`, `secure` (Secure
Boot state query), `input` (raw key/pointer event source).

### Layer 1 — Firmware Boundary
Direct UEFI protocol access that carries no policy, parsing, or UI.
`security_override` (Security2/SecurityArch hooks), `tpm` (TCG2 measured boot),
`firmware_quirks`.

### Layer 2 — Storage & Filesystems
A filesystem-agnostic read API plus the write path. Consumers above this layer
do not know whether they are reading FAT, ext4, Btrfs, or LVM.
`fs_types`, `fs_backend` (the `FsBackend` trait), the backend family
(`fs_backend_fat`, `_ext4`, `_btrfs`, `_lvm`, `_lvm_btrfs`, `_lvm_dispatch`),
`fs` (the coordinator that dispatches to the right backend per volume),
`fs_writer` (the ESP write path), and `initrd` (the LoadFile2 provider, which
reads via `fs`).

### Layer 3 — Parsers & Shared Types
Pure parsers (bytes in, structured data out) and the structured types they
yield. No I/O, no firmware calls, no state.
`bls_parse` (the pure BLS parser), `pe_loader_pure`, `discovery_pure`,
`boot_types` (the shared `BootEntry`/`EntryKind`/`Icon` + preflight result
types), `uki`, and the I/O shells `pe_loader` (over `pe_loader_pure`).

### Layer 4 — Policy & State
Config-driven decisions and persistent state.
`policy` (parse + apply `policy.toml`, including `select_default_entry`),
`autodiscovery`, `preflight`, `health` (NVRAM state machine), `partitions`
(GPT/XBOOTLDR discovery), `drivers` (policy-gated legacy FS-driver loader), and
`bls` (the BLS discovery coordinator — an I/O shell over the Layer-3 `bls_parse`
that drives the boot counter and autodiscovery, which is why it sits here, not
at Layer 3).

### Layer 5 — Trust & Audit
Append-only records of decisions. The audit modules (`trust_log`,
`trust_log_pure`, `telemetry`, `diag`, `version`) are **cross-cutting**: any
layer may write to them, but nothing reads their state to make a decision. They
are exempt from the direction rule as dependency *targets*. `report` and
`bootlog` are the non-cross-cutting Layer-5 emitters.

### Layer 6 — Presentation
Everything the user sees or types. `gui` (GOP double-buffered menu), `console`
(serial/text fallback).

### Layer 7 — Orchestration
The conductor. Assembles the boot flow from the layers below. **Nothing depends
on Layer 7.** `main` (the 10-phase boot flow), `boot` (chainload / UKI /
native-PE / firmware-LoadImage dispatch), `discovery` (cross-backend entry
aggregation; an I/O shell over `discovery_pure`).

## 3. Dependency rules (normative, enforced)

1. A module may `use` from its own layer or any lower layer.
2. A module **must not** `use` from a higher layer.
3. **Cross-cutting** modules (`diag`, `version`, `telemetry`, `trust_log`,
   `trust_log_pure`) may be used from any layer — they are written-to/observed
   from above, never read as control state. They are tagged
   `//! Layer: N (cross-cutting)`.
4. **Pure pairs** — a pure half and its I/O shell at the same layer
   (`pe_loader_pure`↔`pe_loader`, `trust_log_pure`↔`trust_log`,
   `discovery_pure`↔`discovery`) — may reference each other. The pure half is
   tagged `//! Layer: N (pure)`. (`bls_parse`↔`bls` is *not* such a pair:
   `bls.rs` is genuinely Layer 4, so `bls → bls_parse` is an ordinary downward
   edge.)
5. Layer 0 modules must not import from Layer 1+.

These rules are checked by `tools/check-layers.py` against `tools/layer-map.toml`
on every commit (pre-commit hook) and every push/PR (CI). The module dependency
graph is verified acyclic by `check-layers.py --graph`.

## 4. Introducing new code — where does it go?

Decision tree for any new module:

1. **Does it touch UEFI protocols directly?** → Layer 1.
2. **Does it parse bytes without doing I/O, or define a shared data type?** →
   Layer 3.
3. **Does it read/write files via the FS-agnostic API?** → Layer 2.
4. **Does it make a decision based on config + discovered state?** → Layer 4.
5. **Does it record a decision for audit?** → write to a Layer-5 cross-cutting
   module via the trust log; don't ship a new module unless the record shape is
   structurally new.
6. **Does it draw pixels or read keystrokes?** → Layer 6 (presentation) or
   Layer 0 (raw input source).
7. **Does it schedule the boot phases?** → Layer 7 (`main`).

Then: add the module to `tools/layer-map.toml`, add its `//! Layer: N` doc
comment, and run `python3 tools/check-layers.py`. The gate tells you
immediately if a dependency points the wrong way.

## 5. Anti-patterns the gate (and reviewers) reject

- A UEFI protocol call outside Layer 1.
- A "utility" / "helper" module that crosses layers (also forbidden by the
  naming rules in §7).
- A Layer-5 audit module queried for state to make a decision.
- Layer-4 policy code reading UEFI variables directly (go through Layer 0/1).
- Boot-phase-sequencing logic outside `main`.
- A module without a `//! Layer:` declaration (fails the gate).
- An abstraction with only one implementation, unless a second is imminent in
  the same PR series.

## 6. File naming conventions

- No generic suffixes: no `-manager`, `-helper`, `-utility`, `-common`, `-core`
  (except `lamboot-core/` the crate).
- Domain-specific verbs in function names.
- No helper modules.
- One responsibility per module; split if a module grows past ~500 lines and the
  responsibilities are separable.

## 7. Current module counts and layer totals

46 modules, ~16,000 lines of Rust (lamboot-core). By layer:

| Layer | Name | Modules |
|---|---|---|
| 0 | Platform Introspection | acpi, hypervisor, smbios, fw_cfg, fw_cfg_config, secure, input |
| 1 | Firmware Boundary | security_override, tpm, firmware_quirks |
| 2 | Storage & Filesystems | fs_types, fs_backend, fs_backend_{fat,ext4,btrfs,lvm,lvm_btrfs,lvm_dispatch}, fs, fs_writer, initrd |
| 3 | Parsers & Shared Types | bls_parse, pe_loader_pure, discovery_pure, boot_types, uki, pe_loader |
| 4 | Policy & State | policy, autodiscovery, preflight, health, partitions, drivers, bls |
| 5 | Trust & Audit | trust_log, trust_log_pure, report, bootlog, telemetry, diag, version |
| 6 | Presentation | gui, console |
| 7 | Orchestration | boot, discovery, main |

LamBoot is a medium-sized codebase by bootloader standards (GRUB ~40kLOC of C,
systemd-boot ~10kLOC of C, rEFInd ~30kLOC of C++) — smaller than all of them,
with native filesystem reading none of the small ones have. A deliberate
property, not an accident.

---

**This document is normative and machine-enforced.** New modules must declare
their layer in their module-level doc comment (`//! Layer: N — <name>.`) and
appear in `tools/layer-map.toml`. `tools/check-layers.py` checks the declaration
and the dependency direction first, in the pre-commit hook and in CI.
