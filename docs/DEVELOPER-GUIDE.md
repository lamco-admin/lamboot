# LamBoot Developer Guide

**Version:** 0.8.3
**Updated:** 2026-04-21
**Audience:** new contributors, integrators, and anyone reading the LamBoot source

---

## 1. Project layout

```
lamboot-dev/
├── Cargo.toml                     Workspace manifest
├── .cargo/config.toml             Default target = x86_64-unknown-uefi, build-std flags
├── build.sh                       Build both targets + modules, prepare dist/
├── package-release.sh             Build + sign + produce release tarball
├── run-qemu.sh                    QEMU test harness
├── lamboot-core/                  The bootloader
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                Orchestration — 10-phase boot flow
│       ├── bls.rs                 BLS Type 1 parser (UAPI.10 version sort, boot counting)
│       ├── boot.rs                Chainload, UKI, Linux boot under SecurityOverride
│       ├── console.rs             Serial/text console fallback menu
│       ├── discovery.rs           BLS-first entry discovery with ESP fallback
│       ├── drivers.rs             EFI filesystem driver loader, trust events
│       ├── fs.rs                  ESP mount, volume I/O, multi-partition scan
│       ├── gui.rs                 Double-buffered framebuffer, boot menu
│       ├── health.rs              NVRAM state machine, Boot Loader Interface vars
│       ├── initrd.rs              LoadFile2 provider for Linux initrd
│       ├── input.rs               Keyboard + mouse dispatch
│       ├── policy.rs              policy.toml parser
│       ├── report.rs              Boot reports, audit logging
│       ├── secure.rs              Secure Boot state detection + shim integration
│       ├── security_override.rs   PATH F — Security/Security2 arch protocol hooks
│       ├── smbios.rs              SMBIOS string extraction for diagnostics
│       ├── tpm.rs                 TPM 2.0 measured boot (TCG2)
│       └── trust_log.rs           v0.8.3 — JSON-lines audit log
├── lamboot-modules/               Chainloaded diagnostic EFI applications
│   ├── diag-shell/
│   ├── mem-quick/
│   ├── nvme-diag/
│   └── pci-inventory/
├── tools/                         Host-side tooling
│   ├── lamboot-install            Install script (bash)
│   ├── sign-lamboot.sh            Sign bootloader + drivers + modules
│   ├── sign-unlock / sign-lock    Session-cached signing-key unlock
│   ├── lamboot-monitor.py         Proxmox host-side VM boot health monitor
│   └── build-ovmf-vars.sh         Build pre-enrolled OVMF_VARS
├── docs/                          User-facing documentation
├── examples/                      Sample policy.toml, BLS entries
├── dist/                          Build output (gitignored)
├── keys/                          Signing keys (gitignored)
└── .githooks/                     pre-commit: fmt + clippy + cargo check
```

---

## 2. Toolchain

**Required:**
- Rust nightly (for `cargo fmt` import-ordering and `-Zbuild-std`): `rustup install nightly`
- Rust stable (for everything else; >= 1.88)
- Targets: `rustup target add x86_64-unknown-uefi aarch64-unknown-uefi`
- `sbsign`, `sbverify`, `mkfs.vfat` (for testing / signing)
- `llvm-objcopy` (strongly preferred over GNU objcopy for PE section embedding — GNU objcopy corrupts UEFI PE binaries)
- `qemu-system-x86_64`, `qemu-system-aarch64`, `ovmf` / `edk2-ovmf` (for QEMU tests)
- `openssl` (for key generation; see `docs/KEY-GENERATION.md`)

**Optional:**
- `swtpm` (for QEMU TPM tests)
- `virt-fw-vars` from virt-firmware (for OVMF_VARS editing)

---

## 3. Build and test loop

```bash
# Build both targets + modules (primary dev loop)
./build.sh

# Fast iteration: x86_64 only
cargo build --target x86_64-unknown-uefi --release -p lamboot-core

# Format + lint — pre-commit hook runs these, but do it yourself first
rustup run nightly cargo fmt
cargo clippy

# QEMU smoke test (Secure Boot off)
./run-qemu.sh
```

**No `cargo test` for UEFI targets** — UEFI binaries cannot run on the host. Tests that must run cross-compiled live inside in-tree runtime check modules; QEMU is the integration-test vehicle.

---

## 4. Coding standards (enforced)

See `CLAUDE.md` in the repo root for the canonical rules. Summary:

### Comments
- Write *why* comments, not *what* comments. If the code's intent isn't obvious, add a one-line rationale. Don't restate the code.
- Every `unsafe` block requires a `// SAFETY:` comment explaining the invariant.

### Naming
- Verbs describe domain actions: `parse_entry`, `discover_partitions`, `measure_pcr` — never `process`, `handle`, `do_thing`.
- No generic suffixes: no `-Manager`, `-Helper`, `-Utility`. Use `Orchestrator`, `Bridge`, `Provider` when a role word helps.
- No helper modules — no `utils.rs`, `common.rs`, `helpers.rs`.

### Abstractions
- No single-implementation traits. If there's one impl, use the concrete type.
- Max nesting: 3 levels. Use early returns for preconditions.
- Happy path left-aligned; guard clauses at the top.

### Lint suppression
- Never `#[allow(clippy::...)]`. Use `#[expect(..., reason = "...")]`.
- `#[expect]` requires a documented reason. If you can't explain why, fix the code.

### Errors
- UEFI errors propagate with `?` and `uefi::Result`.
- Panic for unrecoverable init only (allocator, UEFI helpers init).
- Optional features degrade gracefully — a missing TPM logs and continues, it doesn't abort boot.

---

## 5. Adding a new subsystem

**Example: adding a network-boot subsystem.**

1. **Create the module.** `lamboot-core/src/netboot.rs`.
2. **Choose a domain-specific name.** Not `network.rs`; maybe `pxe.rs` or `http_boot.rs` depending on the protocol.
3. **Declare in `main.rs`.**
   ```rust
   mod netboot;
   ```
4. **Wire into the boot flow.** `main.rs` is the orchestration. Add a phase with a log line and explicit ordering relative to other phases. Don't hide cross-phase state in globals; pass it through function arguments.
5. **Record trust events.** If your subsystem loads code from an external source, push a `TrustEvent` so the boot-trust log captures the decision.
6. **Test in QEMU.** Add a harness script if the setup is non-trivial.

**Example: adding a diagnostic module.**

1. `mkdir lamboot-modules/my-diag; cd lamboot-modules/my-diag`
2. `Cargo.toml`:
   ```toml
   [package]
   name = "my-diag"
   version.workspace = true
   edition.workspace = true
   license.workspace = true

   [[bin]]
   name = "my-diag"
   path = "src/main.rs"

   [dependencies]
   uefi.workspace = true
   log.workspace = true
   ```
3. `src/main.rs`:
   ```rust
   #![no_std]
   #![no_main]

   use uefi::prelude::*;

   #[entry]
   fn main() -> Status {
       uefi::helpers::init().unwrap();
       log::info!("my-diag: hello from a diagnostic module");
       // ... do stuff ...
       Status::SUCCESS
   }
   ```
4. Add `"lamboot-modules/my-diag"` to the root `Cargo.toml` `workspace.members`.
5. Add a manifest entry in `dist/EFI/LamBoot/modules/manifest.toml`:
   ```toml
   [modules.my-diag]
   name = "My Diagnostic"
   description = "What it does in one sentence"
   version = "0.1.0"
   ```
6. Rebuild via `./build.sh`. The module appears in the LamBoot menu.

---

## 6. Secure Boot signing pipeline

```bash
# One-time: generate production keys (see docs/KEY-GENERATION.md)
# Stores keys/pk.{key,crt}, keys/kek.{key,crt}, keys/db.{key,crt}

# One-time per session: unlock the signing key into tmpfs
./tools/sign-unlock
# Prompts for the db.key passphrase, stashes it in
# /run/user/$UID/lamboot-signing/db.key, exports LAMBOOT_SIGN_KEY.

# Build + sign
./build.sh
./tools/sign-lamboot.sh
# Produces dist/EFI/LamBoot/lambootx64-signed.efi and db.der.

# End of session
./tools/sign-lock   # Drops the unlocked key from tmpfs
```

The signing script uses `llvm-objcopy` to embed the SBAT section; falls back to GNU `objcopy` with a warning because GNU objcopy corrupts UEFI PE binaries in some configurations.

---

## 7. Pre-commit hook

`.githooks/pre-commit` runs on every commit:
1. `cargo fmt --check` (nightly)
2. `cargo clippy` — all warnings treated as errors
3. `cargo check --target x86_64-unknown-uefi`

Enabled via `git config core.hooksPath .githooks` (already set in this repo). To run manually:

```bash
.githooks/pre-commit
```

---

## 8. Release cadence

1. Land features on `main` with feature-complete commits.
2. Bump version in `Cargo.toml` (`workspace.package.version`).
3. Update `CHANGELOG.md` with a new dated section.
4. Build + sign: `./build.sh && ./tools/sign-lamboot.sh`.
5. Package: `./package-release.sh`.
6. Tag: `git tag -a vX.Y.Z -m '...'` and push.

---

## 9. Debugging tips

- **QEMU serial log.** Every test harness pipes to a file — grep that first for `ERROR` / `trust:` / `Security`.
- **SB failures leave artefacts.** If the firmware refuses to boot LamBoot with SB enabled, check the trust log — `\loader\boot-trust.log` is written before handoff, readable from any Linux booted afterward.
- **Binary integrity after signing.** `sbverify --cert keys/db.crt dist/EFI/LamBoot/lambootx64-signed.efi` — if this fails, the signing pipeline is broken. If it passes but firmware refuses, the cert isn't enrolled.
- **Shim ACCESS_DENIED on kernel.** This is typically not LamBoot; it's ShimLock::Verify failing on the kernel. Check the kernel's signature separately with `sbverify`.
- **Driver loading failures.** Under SB, filesystem drivers must be signed *and* SecurityOverride must be active. Check the trust log; look for `driver_rejected` events with reason.

---

## 10. Contributing

See `CONTRIBUTING.md` for:
- Branch and PR conventions (conventional commits)
- CI expectations
- DCO / sign-off (recommended but not required)

**Good first issues:**
- Diagnostic modules (NVMe SMART richness, PCI enumeration completeness, memory-test patterns)
- New filesystem driver packaging (testing, signing)
- Distro-specific install script hardening (OpenSUSE, Arch, Gentoo equivalence)
- Documentation refinements for unusual hardware platforms
- Localization of the GUI

**Not a good first issue:**
- `security_override.rs` — needs deep UEFI protocol understanding
- `tpm.rs` TCG2 work — bring a TPM test harness
- `lamboot-core/src/main.rs` boot-flow ordering — almost every change is subtle

---

## 11. Where to ask questions

- **Code-level:** open a GitHub Discussion on the public repo.
- **Security:** `security@lamco.io` only. Do not file public issues for suspected vulnerabilities.
- **Packaging / distro integration:** GitHub Issues on the public repo.

---

**For full architecture, see `docs/ARCHITECTURE.md`.**
