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

pub(crate) mod cache;
pub(crate) mod mock;

// The `fs_types` items are `pub(crate)` by design (they mirror the
// lamboot-core originals verbatim). Tests reference them through the
// module path rather than re-exporting, keeping visibility identical
// to the authoritative source.

#[cfg(test)]
mod tests;
