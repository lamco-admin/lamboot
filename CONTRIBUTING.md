# Contributing to LamBoot

## Development Setup

```bash
# Install Rust and targets
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi

# Clone and build
git clone <repository-url>
cd lamboot
./build.sh

# Test in QEMU
./run-qemu.sh
```

## Code Structure

```
lamboot-core/src/
  main.rs         Boot orchestration (10-phase flow)
  bls.rs          BLS Type 1 parser + UAPI.10 version sort
  gui.rs          Double-buffered framebuffer + VGA font
  console.rs      Text console fallback
  discovery.rs    BLS-first entry discovery + ESP fallback
  boot.rs         Boot execution (chainload/UKI/Linux+LoadFile2)
  initrd.rs       LoadFile2 initrd protocol provider
  drivers.rs      EFI filesystem driver loader
  fs.rs           Filesystem abstraction + multi-partition scan
  health.rs       NVRAM boot health state machine
  policy.rs       Section-aware TOML parser
  input.rs        Keyboard + mouse input
  secure.rs       Secure Boot + shim integration
  tpm.rs          TPM 2.0 measured boot
  report.rs       Boot logging with timestamps
```

## Coding Guidelines

- Use `rustfmt` for formatting
- Minimize `unsafe` — document all safety invariants
- `no_std` only — use `alloc` crate, no `std`
- Prefer `uefi-rs` safe wrappers over raw protocol FFI
- Handle absent protocols gracefully — never block boot for optional features
- Keep binary size small — avoid unnecessary dependencies

## Adding Features

### New Boot Entry Type

1. Add variant to `EntryKind` in `discovery.rs`
2. Implement detection in `discovery.rs`
3. Handle boot in `boot::boot_entry()`
4. Add icon in `Icon` enum

### New Diagnostic Module

1. Create `lamboot-modules/your-module/` with `Cargo.toml` + `src/main.rs`
2. Add to workspace members in root `Cargo.toml`
3. Use `#![no_main]` `#![no_std]` with `#[entry]` and `uefi::helpers::init()`
4. Add to `modules/manifest.toml` for a friendly name
5. Build: `cargo build --target x86_64-unknown-uefi --release -p your-module`

### New UEFI Protocol

1. Check if `uefi-rs` has a wrapper (prefer wrappers over raw)
2. Handle protocol absence gracefully (`Ok(()) if not found`)
3. Test on systems without the protocol

## Testing

```bash
# QEMU test
./run-qemu.sh

# Build both architectures
./build.sh
```

### Test Scenarios

- [ ] BLS entry discovery + boot
- [ ] Windows chainload
- [ ] Linux UKI boot
- [ ] Boot counting (+N suffix decrement)
- [ ] Crash loop detection + fallback
- [ ] GUI rendering at multiple resolutions
- [ ] Text console (no GOP)
- [ ] Mouse + keyboard navigation
- [ ] Timeout and auto-boot
- [ ] Policy allowlist/denylist
- [ ] Filesystem driver loading
- [ ] Secure Boot with signed binary
- [ ] TPM measurements (with swtpm)

## Commit Messages

```
feat: add btrfs driver support
fix: mouse bounds at 4K resolution
docs: update Proxmox integration guide
refactor: extract BLS parser to separate crate
```

## Areas Needing Help

- Advanced graphics (icons, themes, backgrounds)
- Network boot support (HTTP/PXE)
- UKI Type 2 PE section parsing
- Localization framework
- Touch screen input
- NVMe diagnostic module implementation

## Issue Lifecycle

LamBoot follows a release-anchored issue lifecycle:

- **Open**: an issue stays open while the work it describes is incomplete or unshipped to users.
- **Closed on release, not on commit**: a fix landing in `main` does not close its issue. The issue closes when a release that contains the fix is published. This keeps the issue tracker honest about what users actually have access to.

If you're a contributor sending a PR, you do not need to close the linked issue. Reference it with `Refs #N` (rather than `Closes #N`) and let the maintainers close it as part of a release cycle. Using `Closes #N` is fine if the change ships imminently; just be aware the maintainers may reopen it briefly if the release is delayed.

## Labels

The repository uses three orthogonal label axes:

- **`type:*`** — what kind of issue (`bug`, `enhancement`, `documentation`, `question`, `tracking`, `packaging`, `security`). Pick one.
- **`area:*`** — what part of the project (`boot`, `trust-log`, `filesystem`, `install`, `policy`, `modules`, `proxmox`, `testing`). Pick zero or more.
- **`distro:*`** — which distribution is affected, when distro-specific (`debian`, `ubuntu`, `fedora`, `arch`, `opensuse`, `nixos`). Pick zero or more.

Priority is tracked via milestones, not labels. If an issue is slated for a release, it gets that milestone.

## License

Contributions are dual-licensed under MIT/Apache-2.0.
