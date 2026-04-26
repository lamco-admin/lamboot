//! Host-runnable unit tests for SPEC-FS-BACKEND-TRAIT §10.2.
//!
//! The real bootloader types live in `lamboot-core`, which is a `no_std` +
//! `no_main` UEFI binary crate. That crate cannot host `cargo test`, so we
//! re-include the pure pieces here via `#[path]` and run the §10.2 suite
//! against them. No code duplication — the real module file IS the tested
//! module file.
//!
//! What this crate covers:
//!   * `fs_types` module verbatim (Path, PathBuf, Uuid, FileKind, DirEntry,
//!     Metadata, PathError). Pure Rust, zero UEFI deps — re-included as-is.
//!   * An in-memory `MockBackend` that implements a host-compatible
//!     equivalent of `FsBackend` for cache/behavior tests.
//!   * A host-side `VolumeCache` mirror to exercise LRU semantics.
//!
//! What this crate does NOT cover (deferred to SPEC §10.3 QEMU tests):
//!   * FAT adapter behavior (requires UEFI SimpleFileSystem)
//!   * EspWriter FAT-only compile-time enforcement (requires Volume)
//!   * Real boot of EndeavourOS VM 123 (SPEC §12 last bullet)

#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(unreachable_pub)]

extern crate alloc;

// Re-include the pure-types module verbatim. File lives at
// `../lamboot-core/src/fs_types.rs` and is authoritative.
#[path = "../../lamboot-core/src/fs_types.rs"]
mod fs_types;

// SDS-5 PR-2: include the pure BLS parser verbatim. Lives at
// `../../lamboot-core/src/bls_parse.rs`. Tests exercise the parser
// here; the Volume-dependent side (`scan_volume_for_bls`,
// `decrement_boot_count`) is covered by the QEMU harness (PR-4).
//
// This module is `pub` (not `pub(crate)`) because tests under
// `tests/bls_parse.rs` are compiled as external consumers of the
// library and need access. The parser items themselves stay
// `pub(crate)` in the included file — they are crate-visible in
// both lamboot-core and this test crate, which matches the design
// intent (parser is internal; tests are part of "crate internals"
// from a governance perspective even though cargo compiles them
// against the published surface).
#[path = "../../lamboot-core/src/bls_parse.rs"]
pub mod bls_parse;

// SDS-3 PR-2: same pattern as bls_parse. Pure PE-loader logic lives
// at `../../lamboot-core/src/pe_loader_pure.rs`; tests under
// `tests/pe_loader_pure.rs` compile against it as an external
// consumer. The Volume-dependent side (`pe_loader.rs` — allocate,
// install protocol, start_image) is out of scope for host tests and
// is exercised by the QEMU harness in SDS-3 PR-4.
#[path = "../../lamboot-core/src/pe_loader_pure.rs"]
pub mod pe_loader_pure;

// SDS-4 PR-4: same pattern. Pure trust-log logic (schema, accumulator,
// stable vocabulary, JSON-Lines serializer) lives at
// `../../lamboot-core/src/trust_log_pure.rs`; tests under
// `tests/trust_log_pure.rs` exercise ordering, field-schema stability,
// and vocabulary constants. The Volume-dependent side (the `flush()`
// wrapper in `trust_log.rs` that writes the buffer via `EspWriter`) is
// exercised by the QEMU harness.
#[path = "../../lamboot-core/src/trust_log_pure.rs"]
pub mod trust_log_pure;

// Pop!_OS / systemd-boot-discoverable-EFI auto-discovery helpers. Lives
// at `../../lamboot-core/src/discovery_pure.rs`; tests under
// `tests/discovery_pure.rs` cover the dirname-pattern recognizer and
// the pretty-name renderer. The Volume-dependent scanner
// (`discover_systemd_boot_dir_style` in `discovery.rs`) is exercised
// by the fleet QEMU validation on VM 124 (Pop!_OS).
#[path = "../../lamboot-core/src/discovery_pure.rs"]
pub mod discovery_pure;

pub(crate) mod cache;
pub(crate) mod mock;

// The `fs_types` items are `pub(crate)` by design (they mirror the
// lamboot-core originals verbatim). Tests reference them through the
// module path rather than re-exporting, keeping visibility identical
// to the authoritative source.

#[cfg(test)]
mod tests;
