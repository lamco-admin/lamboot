# SPEC-FS-BACKEND-TRAIT: Layer-2 Filesystem Abstraction

**SDS number:** SDS-1 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/fs_backend.rs` (new), `lamboot-core/src/fs_backend_fat.rs` (new), `lamboot-core/src/fs_backend_ext4.rs` (new), substantial refactor of `lamboot-core/src/fs.rs`
**Architecture reference:** `docs/ARCHITECTURE-LAYERS.md` (Layer 2)
**Strategic reference:** `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md`
**Supersedes:** — (first SDS in the v0.9.x series)
**Blocks:** SDS-2 (ext4 adapter), SDS-3 (PE loader caller requirements), SDS-5 (BLS multi-FS discovery), SDS-6 (UEFI FS driver deprecation)

---

## 1. Overview

This specification defines the `FsBackend` trait — the uniform read-API that every filesystem backend in LamBoot implements — plus the dispatch layer that routes per-volume operations to the correct backend at runtime. The trait is the foundation of LamBoot's Layer 2 and is the single most load-bearing design decision of the v0.9.x series: every piece of v1.0 work (native ext4 reading, the native PE loader's byte-sourcing layer, BLS discovery across multi-FS volumes, trust-log provenance of each read) either calls through this trait or depends on it being correctly shaped.

### 1.1 Context

LamBoot v0.8.3 exposes filesystem access through a single concrete type, `EspVolume` (see `lamboot-core/src/fs.rs`), which wraps UEFI's `SimpleFileSystem` protocol. This is adequate for FAT access on the ESP but has three load-bearing problems v1.0 must fix:

1. **UEFI `SimpleFileSystem` is available for a volume only if some driver produced a `SimpleFileSystem` handle for it.** For ext4, btrfs, xfs, f2fs and other non-FAT filesystems, `SimpleFileSystem` only appears after a UEFI filesystem driver like `ext4_x64.efi` has been `LoadImage`d and `StartImage`d. That driver-load triggers the shim 15.8 `ShimLock`-uninstall bug documented in `CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` and makes the rest of the boot session unable to verify MOK-signed kernels.
2. **`EspVolume` is concrete, not pluggable.** Adding a native Rust ext4 reader (per `NATIVE-FS-AND-PE-LOADER-STRATEGY`) requires either modifying `EspVolume` in place — coupling it to ext4-specific types — or providing a parallel type whose call-sites the existing consumers (`discovery.rs`, `boot.rs`, `trust_log.rs`, `bls.rs`, etc.) don't recognize.
3. **The existing API is a flat file-by-path interface** (`read_to_vec`, `read_to_string`, `exists`, `write_file`) with implicit ESP-relative semantics. It cannot express "read from volume X" or "this read came from the ext4 adapter on partition Y" without plumbing volume identity through every call-site.

### 1.2 What this SDS delivers

- A `FsBackend` trait with a minimal, stable read-surface that every backend implements.
- A `Volume` dispatch struct that holds a `Box<dyn FsBackend>` plus volume identity and caches and exposes the same public API v0.8.3 consumers use today (`read_to_vec`, `exists`, `read_dir`, …).
- Rules for volume identity (partition UUID + filesystem UUID + label) used to select the correct backend at mount time.
- A FAT adapter (`fs_backend_fat.rs`) that implements the trait on top of `uefi-rs`'s `SimpleFileSystem` (preserving the v0.8.3 behavior).
- Lifecycle rules: when backends are created, when they're dropped, who owns what.
- Error discipline: one `FsError` enum with precise variants, mapping to trust-log events.
- A refactor of `lamboot-core/src/fs.rs` from "concrete `EspVolume`" to "volume coordinator."
- Test plan with mock backend + round-trip harness.

### 1.3 What this SDS does NOT deliver

- **ext4 adapter implementation.** That is SDS-2's scope. This SDS only specifies the trait it must satisfy and the integration seams it plugs into.
- **PE loader byte source.** That is SDS-3. The PE loader will be a consumer of `FsBackend`; this SDS defines what it can assume.
- **BLS discovery logic changes.** SDS-5. Discovery will migrate from "scan ESP only" to "scan all mounted volumes via their backends."
- **Write support.** Layer 2 is read-only in v0.9.x. Existing write paths (boot-trust.log, boot.log, NVRAM-mirror files) stay on the ESP FAT volume and bypass the trait via a narrower `EspWriter` type defined in §6.4. Never-on-ext4 is a security property, not a limitation.
- **Volume auto-detection UI.** Volume enumeration is a platform concern (Layer 1) and stays in `partitions.rs`. Layer 2 is fed already-enumerated volumes.

### 1.4 Constraints

- `#![no_std]` + `alloc`. No `std`. No threads. No async.
- Callers are single-threaded UEFI boot-services-time code. No interior mutability concerns beyond Rust's ownership model.
- Every `unsafe` block requires a `// SAFETY:` comment (per `CLAUDE.md`).
- No new `#[allow(clippy::…)]`. Use `#[expect(…, reason = "…")]` with justification if needed.
- Max three nesting levels in new code. Early-return guard clauses preferred.
- Trust log (Layer 5) is write-only from Layer 2's perspective — backends produce events; they do not query the log.
- Backends are layered strictly above Layer 1 (UEFI protocols). A backend may call `uefi::boot::open_protocol_exclusive`, `uefi::boot::find_handles`, `BlockIO::read_blocks`, etc. A backend must NOT call Layer 3 parsers (`bls.rs`, `uki.rs`, `pe_loader.rs`) or Layer 4 policy.

---

## 2. The `FsBackend` trait

```rust
//! Layer 2 — Filesystem abstraction.
//!
//! Trait implemented by every filesystem backend. Backends are read-only
//! by design (see NATIVE-FS-AND-PE-LOADER-STRATEGY §5.7). Writes go through
//! `EspWriter` (FAT-ESP only) in a separate path.

use alloc::{string::String, vec::Vec};
use crate::fs_backend::{DirEntry, FsError, Metadata, Path, Uuid};

/// The single trait every filesystem backend implements.
///
/// Mounted volumes in LamBoot are owned by a `Volume` wrapper (see §3)
/// that holds a `Box<dyn FsBackend>` plus identity/cache state. Backends
/// themselves carry only the per-volume state they need (block cache,
/// superblock copy, partition handle).
pub(crate) trait FsBackend {
    // -------- identity --------

    /// Short human-readable backend tag for logs and trust events.
    /// Must be a stable compile-time string, e.g. `"fat"`, `"ext4-view@0.9.3"`.
    fn tag(&self) -> &'static str;

    /// Filesystem UUID if the backend can expose one, else `None`.
    /// (FAT returns `None` unless the volume has an fsinfo serial we care to use.)
    fn uuid(&self) -> Option<Uuid>;

    /// Filesystem label if present.
    fn label(&self) -> Option<&str>;

    // -------- read surface --------

    /// Read the entire file at `path` into a newly-allocated `Vec<u8>`.
    /// Fails with `FsError::NotFound` if the file does not exist;
    /// `FsError::IsDirectory` if the path names a directory.
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError>;

    /// Read a byte range from the file at `path`.
    /// `offset` is interpreted as a zero-based absolute position.
    /// Reads as many bytes as available, up to `len`; returns fewer bytes
    /// only if EOF is reached before `len` bytes are available.
    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError>;

    /// Return `true` iff a file or directory exists at `path`.
    /// Never returns an error for "not present"; only for I/O failures.
    fn exists(&mut self, path: &Path) -> Result<bool, FsError>;

    /// Stat the entry at `path`.
    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError>;

    /// Return the directory entries immediately under `path`.
    /// The order is backend-defined and unspecified; consumers that need
    /// deterministic order must sort after the call.
    /// The returned slice excludes `.` and `..`.
    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError>;

    // -------- capabilities --------

    /// Whether this backend supports opening a streaming read handle.
    /// Backends that can only do whole-file reads return `false`.
    /// The default is `false`; override and implement `open_stream` to enable.
    fn supports_streaming(&self) -> bool { false }

    /// Open a streaming read handle for a file. Default implementation
    /// returns `FsError::Unsupported`. Backends that override must also
    /// override `supports_streaming` to return `true`.
    #[expect(unused_variables, reason = "default impl")]
    fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        Err(FsError::Unsupported("open_stream"))
    }
}

/// Streaming read handle. Optional per-backend. Used by the PE loader (SDS-3)
/// to avoid buffering an entire 100-MB kernel in RAM before validation.
pub(crate) trait FsStream {
    /// Total byte length of the stream, or `None` if unknown.
    fn len(&self) -> Option<u64>;

    /// Fill `buf` starting at the current position; advance position by
    /// the number of bytes actually read. Returns `Ok(0)` at EOF.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Seek to absolute byte offset. Not all backends support non-forward seeks.
    fn seek(&mut self, offset: u64) -> Result<(), FsError>;
}
```

### 2.1 Trait-design decisions and their rationale

| Decision | Rationale |
|---|---|
| `&mut self` on every read method | Backends are single-threaded and may cache blocks or hold a file handle. `&mut self` lets the implementation mutate that cache without `RefCell`/`Mutex` boilerplate. Matches ext4-view's `&mut self` read signature. |
| Owned `Vec<u8>` return from `read` / `read_at` | Mirrors today's `EspVolume::read_to_vec`. Zero callsite churn for existing consumers. A buffer-in API would be more efficient but forces every call site to pre-size; not worth it for boot-time code. |
| Streaming API is optional | FAT streaming is trivially possible via `SimpleFileSystem::RegularFile::read`. ext4-view does not expose a streaming API today (SDS-2 will add a thin wrapper if needed). Marking streaming optional lets the FAT backend expose it immediately without blocking SDS-2. |
| `tag()` returns `&'static str` | Trust-log events embed the backend tag (`verified_via=fat` or `verified_via=ext4-view@0.9.3`). Static strings avoid allocation during log emission. |
| `read_dir` excludes `.` and `..` | Every caller filters them anyway; doing it once in the backend wrapper removes a landmine. |
| No `create_dir` / `write` / `remove` | Read-only Layer 2. Writes are narrower (SDS-1 §6.4 `EspWriter`) and cannot pretend to be a general-purpose backend operation. |

### 2.2 Non-methods (intentionally absent)

| Absent | Rationale |
|---|---|
| `flush` | Read-only; nothing to flush. |
| `sync` | Same. |
| `chmod` / `chown` | Never needed at boot time. |
| Async anything | UEFI boot-services are synchronous; async would add an unnecessary runtime. |
| Mount / unmount | Handled by `Volume::new()` / `Drop` (see §4). |
| Generic `<T>` on any method | Keeps the vtable shape stable and the trait object-safe. |

---

## 3. The `Path` and `Uuid` types

### 3.1 `Path`

```rust
/// Filesystem-backend path.
///
/// Always absolute, always forward-slash-separated, always UTF-8.
/// NOT `std::path::Path` — that depends on `std`.
/// NOT raw `&str` — a newtype makes the intent explicit and allows
/// compile-time rejection of relative paths (see From impl).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct PathBuf(String);

#[derive(Debug, Eq, PartialEq, Hash)]
pub(crate) struct Path(str);
```

Separator convention: **forward slash only**. UEFI paths use backslash; the FAT backend translates `/foo/bar` ↔ `\foo\bar` at the I/O boundary. Consumers deal only in forward slashes — no mixed conventions bleed into Layer 3+ code.

Construction rules:

- `Path::from_str("/a/b/c")` succeeds.
- `Path::from_str("a/b")` returns `Err(PathError::NotAbsolute)` — relative paths are a bug, never a feature, at boot time.
- `Path::from_str("/a//b")` canonicalizes to `/a/b` (collapses redundant separators).
- `Path::from_str("/a/./b")` canonicalizes to `/a/b`.
- `Path::from_str("/a/../b")` canonicalizes to `/b`. Traversal above root (`/../a`) is an error (`PathError::AboveRoot`).
- `Path::from_str("")` is an error.

### 3.2 `Uuid`

Use the existing `uefi::data_types::Guid` type when the UUID originates from UEFI (partition GUID). Use a local `Uuid` newtype wrapping `[u8; 16]` for filesystem UUIDs (ext4 superblock UUID, FAT volume-serial widened to 16 bytes). The backend decides which is appropriate; the `Volume` struct stores both.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Uuid(pub [u8; 16]);
```

Display: hyphenated lowercase hex (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`) for log output. No base64, no bytewise-reversed-endian quirks.

---

## 4. The `Volume` dispatch struct

### 4.1 Definition

```rust
/// A mounted filesystem, routed through a concrete backend.
///
/// `Volume` owns the backend's lifetime. Dropping a `Volume` drops the
/// backend, which must release any UEFI protocol handles it holds
/// (`SimpleFileSystem` for FAT, `BlockIO` for ext4).
pub(crate) struct Volume {
    /// Opaque identity for dispatch. Not interpreted by consumers directly.
    identity: VolumeIdentity,

    /// The backend. `Box<dyn FsBackend>` keeps the trait object-safe.
    backend: Box<dyn FsBackend>,

    /// Per-volume cache state that survives across `read` calls but is
    /// dropped with the volume. Keeping cache HERE (not in the backend)
    /// means backends stay pure-protocol; the cache is a pure Layer 2
    /// concern and can be tuned without touching any backend impl.
    cache: VolumeCache,
}

pub(crate) struct VolumeIdentity {
    /// Partition GUID from the GPT entry (preferred identity).
    pub partition_guid: Option<uefi::Guid>,
    /// Filesystem-level UUID if the backend exposes one.
    pub fs_uuid: Option<Uuid>,
    /// Filesystem label if present. Human-readable only.
    pub label: Option<String>,
    /// Zero-based index assigned at mount time. Stable within a boot.
    pub index: u32,
    /// Backend tag (same as `FsBackend::tag()`).
    pub backend_tag: &'static str,
}
```

### 4.2 Public API of `Volume`

`Volume` is what Layer 3+ code holds. It presents the same semantics the v0.8.3 `EspVolume` did — callers that migrate from `EspVolume` to `Volume` change types, not logic.

```rust
impl Volume {
    pub(crate) fn identity(&self) -> &VolumeIdentity;

    pub(crate) fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError>;
    pub(crate) fn read_to_string(&mut self, path: &Path) -> Result<String, FsError>;
    pub(crate) fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError>;
    pub(crate) fn exists(&mut self, path: &Path) -> Result<bool, FsError>;
    pub(crate) fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError>;
    pub(crate) fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError>;

    /// Streaming handle. Returns `FsError::Unsupported` for backends
    /// that don't implement streaming.
    pub(crate) fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError>;
}
```

### 4.3 What the cache does

Every read path through `Volume::read*` consults the cache first. Cache keys are `(path_hash, offset, len)` tuples. Cache payload is `Arc<[u8]>` so multiple callers can hold the same slice without copy.

Cache size: 8 MiB default (configurable via `policy.toml` in a later release; hard-coded in v0.9.0). LRU eviction.

Cache is per-volume, not global. Rationale: cross-volume cache invalidation would have to reason about write ordering (writes are ESP-only, cache is per-volume → writes on one volume can never invalidate another's cache).

The cache is a performance tool, not a correctness tool. Backends are expected to return the same bytes for the same `(path, offset, len)` within one boot. This is trivially true of read-only backends.

### 4.4 Lifecycle

Volumes are created exactly once per volume per boot, in `partitions.rs`'s enumeration phase (Layer 1). The enumeration returns `Vec<Volume>`. Callers hold onto volumes for the rest of the boot; volumes drop when `run_bootloader` returns or when `ExitBootServices` is called (whichever is first). No mid-boot unmount.

Volumes are **not** `Clone`, **not** `Copy`, **not** `Send`. They hold `&mut`-exclusive UEFI protocol handles through the backend.

---

## 5. Dispatch — picking the right backend at mount time

Volume enumeration (the `partitions.rs::mount_all_volumes()` function after SDS-1 lands) proceeds as follows:

1. Enumerate all `BlockIO` handles via `uefi::boot::find_handles::<BlockIO>()`.
2. For each handle, open the `BlockIO` protocol, read the first 4 KiB (enough for GPT entry + common FS signatures).
3. Determine filesystem type by probing:
   - **FAT**: if `uefi::boot::open_protocol::<SimpleFileSystem>` returns `Ok`, it's FAT (or firmware claims so). Probe magic (`0x28` or `0x29` boot-signature byte) to confirm.
   - **ext4**: superblock magic at offset 0x438 = `0xEF53` and `s_rev_level` in supported range.
   - **Btrfs** (v1.1+): superblock magic at offset 0x10040 = `"_BHRfS_M"`.
   - **XFS** (v1.1+): superblock magic at offset 0 = `"XFSB"`.
4. Pick the backend whose probe succeeded. If multiple probes match (rare, pathological), use the first-match order `fat > ext4 > btrfs > xfs` — correctness cost of picking wrong is high and FAT+ESP is the universally safest default.
5. Instantiate the backend with the `BlockIO` handle (or `SimpleFileSystem` handle for FAT).
6. Read the filesystem UUID and label (if any).
7. Construct `VolumeIdentity` and wrap the backend in a `Volume`.
8. Push to the enumerated volume list.

If no backend probe matches, the volume is **skipped silently** (trust-log event `volume_skipped_unknown_fs`) — it's not an error, it's just an unsupported volume. Layer 7 logs the total.

### 5.1 Dispatch conflicts

Two non-pathological cases need specific handling:

**Case A: FAT volume with UEFI `SimpleFileSystem` already installed (ESP).** Use the FAT backend. Don't re-probe; trust firmware.

**Case B: ext4 volume with `SimpleFileSystem` ALSO installed** (an ext4 UEFI driver loaded earlier in this boot). v0.9.x native-first policy: **use the native ext4-view backend**, not the UEFI driver. The UEFI driver is left attached (doesn't harm anything) but we do not use its `SimpleFileSystem`. Trust-log event: `fs_backend_selected` with `reason=native-preferred`.

**Case C: ext4 volume with no SimpleFileSystem.** Native backend, same as Case B without the UEFI-driver cohabitation. This is the common case for `/boot` on modern distros.

**Case D: FAT volume with partition GUID matching the GPT ESP-type GUID (`c12a7328-…`) but located on a non-boot disk.** Still FAT, still mounted as a volume. LamBoot may choose to prefer the boot-disk ESP for its own policy.toml / logs (see §6.4 `EspWriter`); other ESPs are read-only from LamBoot's perspective.

---

## 6. Metadata, errors, writes

### 6.1 `Metadata`

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Metadata {
    pub kind: FileKind,
    /// Byte length for regular files; `0` for directories.
    pub size: u64,
    /// Unix-style mode bits where backends provide them; `0o600` default if not.
    pub mode: u32,
    /// Read-only flag exposed as a fast property (most-common check).
    pub read_only: bool,
    /// Backend tag that produced this metadata.
    pub source_backend: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileKind {
    Regular,
    Directory,
    Symlink,
    Other,
}
```

### 6.2 `FsError`

```rust
#[derive(Debug)]
pub(crate) enum FsError {
    NotFound,
    PermissionDenied,
    IsDirectory,
    NotDirectory,
    InvalidPath(&'static str),
    Unsupported(&'static str),
    /// Filesystem uses a feature this backend can't handle.
    UnsupportedFeature { feature: &'static str, backend: &'static str },
    /// FS structural corruption detected at read time.
    Corrupt(&'static str),
    /// Underlying block I/O error.
    Io { source: uefi::Error },
    /// Backend-specific error (ext4-view returns a boxed dyn Error).
    BackendOther(Box<dyn core::error::Error + Send + Sync>),
}

impl FsError {
    /// Short stable string for trust-log `status` field.
    pub(crate) fn as_log_token(&self) -> &'static str {
        match self {
            FsError::NotFound => "not_found",
            FsError::PermissionDenied => "permission_denied",
            FsError::IsDirectory => "is_directory",
            FsError::NotDirectory => "not_directory",
            FsError::InvalidPath(_) => "invalid_path",
            FsError::Unsupported(_) => "unsupported",
            FsError::UnsupportedFeature { .. } => "unsupported_feature",
            FsError::Corrupt(_) => "fs_corrupt",
            FsError::Io { .. } => "io_error",
            FsError::BackendOther(_) => "backend_other",
        }
    }
}
```

Rules for error construction:

- `NotFound` is not a log-worthy event on its own; the caller decides whether absence is expected.
- `Corrupt` ALWAYS emits a trust-log event (Layer 5) with the path and the specific corruption description.
- `UnsupportedFeature` emits a trust-log event one-time per (backend, feature) combination per boot — gives the user actionable diagnostics when a filesystem uses a non-standard feature, without spamming.
- `BackendOther` is a last resort. Only for errors the adapter layer (SDS-2, future SDS-N) genuinely can't classify.

### 6.3 Error-to-log mapping

When the Layer 7 orchestrator (or any Layer 3+ caller) catches an `FsError`, it is responsible for emitting the appropriate trust-log event. The `FsError` type is intentionally plain; the log emission happens where the caller knows the context (what file was being read, why). Layer 2 must not emit trust-log events on its own (respects the "Trust & Audit is written to, not read from" rule per `ARCHITECTURE-LAYERS.md`).

### 6.4 `EspWriter` — narrow write path

Writes in v0.8.3 target `\loader\boot-trust.log`, `\EFI\LamBoot\reports\boot.log`, `\EFI\LamBoot\reports\audit.log`, `\EFI\LamBoot\reports\boot.json`. All of these live on the FAT ESP. The `FsBackend` trait is read-only; writes go through:

```rust
/// Write path — FAT-ESP only by construction. No ext4/btrfs writes possible.
/// (Explicit design choice, documented as a security property.)
pub(crate) struct EspWriter<'v> {
    volume: &'v mut Volume,  // must be a FAT backend; constructor enforces.
}

impl<'v> EspWriter<'v> {
    /// Construct an EspWriter from a FAT-backed volume.
    /// Returns `None` if the volume's backend is not FAT.
    pub(crate) fn new(volume: &'v mut Volume) -> Option<Self>;

    /// Overwrite `path` with `data`. Creates if missing.
    pub(crate) fn write(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError>;

    /// Append `data` to `path`. Creates if missing.
    /// (Future Path G enhancement; v0.9.0 MAY fall back to read-overwrite if
    /// `FileProtocol::Write`-at-EOF isn't cleanly exposed.)
    pub(crate) fn append(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError>;
}
```

The constructor inspecting `backend_tag == "fat"` is the ONLY place Layer 2 code branches on backend identity. This is a deliberate seam: it makes the FAT-only write promise a compile-time reality of the type system (you can't hold an `EspWriter` without a FAT `Volume`), and it localizes the discipline so no one adds an ext4-write path by accident.

---

## 7. Module structure

```
lamboot-core/src/
├── fs.rs                     — REFACTORED. Coordinator + Volume + VolumeIdentity + VolumeCache.
│                               Re-exports FsBackend, FsError, Metadata, Path, PathBuf, Uuid, DirEntry, FsStream.
├── fs_backend.rs             — NEW. FsBackend trait definition, FsStream trait, DirEntry type.
├── fs_backend_fat.rs         — NEW. FAT adapter over uefi-rs SimpleFileSystem. Implements FsBackend.
├── fs_backend_ext4.rs        — (SDS-2) skeleton only in SDS-1; returns Unsupported for every method.
│                               Must compile cleanly against the trait. Full impl lands in SDS-2.
├── fs_writer.rs              — NEW. EspWriter type, FAT-only write path.
└── (existing files unchanged except imports referring to fs.rs public API)
```

### 7.1 Old-to-new migration table

| v0.8.3 usage | v0.9.x replacement | Notes |
|---|---|---|
| `EspVolume::new(root)` | (removed) | Volume creation is now centralized in `partitions.rs::mount_all_volumes()`. |
| `EspVolume::read_to_vec(path)` | `Volume::read(&Path::from_str(path)?)` | Callsite adds `Path` conversion. |
| `EspVolume::read_to_string(path)` | `Volume::read_to_string(&Path::from_str(path)?)` | Same. |
| `EspVolume::exists(path)` | `Volume::exists(&Path::from_str(path)?)` | Now returns `Result<bool>` instead of `bool` (better error propagation). |
| `EspVolume::read_file_at(path, off, len)` | `Volume::read_at(&Path::from_str(path)?, off, len)` | |
| `EspVolume::write_file(path, data)` | `EspWriter::new(&mut volume).unwrap().write(&path, data)?` | Explicit FAT-only. |

Paths that used backslash (`\EFI\LamBoot\…`) migrate to forward-slash (`/EFI/LamBoot/…`). The FAT adapter does the backslash translation internally.

---

## 8. FAT adapter (`fs_backend_fat.rs`)

This adapter is in scope for SDS-1 because it anchors the trait against real UEFI behavior and lets SDS-1 be implemented without waiting for SDS-2.

### 8.1 Internal structure

```rust
pub(crate) struct FatBackend {
    /// The UEFI SimpleFileSystem protocol handle.
    /// We don't hold the open-protocol guard directly because it ties to
    /// the volume lifetime handled by `Volume`; we re-open per call.
    handle: uefi::Handle,
    uuid: Option<Uuid>,   // FAT volume serial widened; None if unreadable.
    label: Option<String>,
}
```

### 8.2 Per-call protocol opening

Every `FsBackend::read` / `exists` / etc. call on `FatBackend` does:

```rust
let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(self.handle)?;
let mut root = fs.open_volume()?;
// … translate Path → CString16 (backslash), open file, read, done.
```

**Design choice: per-call protocol re-open.** Alternative was to hold the `ScopedProtocol` in a field for the life of the backend. Per-call re-open trades microseconds of protocol open overhead for dramatically simpler lifetime handling (no `'a`-lifetime needed on `FatBackend`). At boot time, read volume is < dozens of `open_protocol_exclusive` calls — overhead is measurement noise. Keep it simple.

### 8.3 Path translation

`Path::from_str("/EFI/LamBoot/policy.toml")` → CString16 `"\EFI\LamBoot\policy.toml"`. Algorithm: replace `/` with `\`, pass through `CString16::try_from`, handle the "CString16 construction can fail on non-BMP codepoints" edge via `FsError::InvalidPath("non-BMP-codepoint")`.

### 8.4 Streaming

`FatBackend::supports_streaming() = true`. Implementation returns a `FatStream` wrapping `uefi::proto::media::file::RegularFile`. `seek` maps to `RegularFile::set_position`. `read` to `RegularFile::read`.

### 8.5 Known FAT-specific limitations

- Case-insensitive filesystem. Consumers requesting `"/EFI/LamBoot/policy.toml"` will match `"\EFI\lamboot\POLICY.TOML"`. This is UEFI-spec behavior; we don't alter it.
- No symlinks. `FileKind::Symlink` never returned.
- `Metadata::mode` synthesized: directories get `0o755`, readable files `0o644`, read-only-flagged files `0o444`.

---

## 9. Integration seams

### 9.1 `main.rs`

```rust
// Old (v0.8.3):
let mut esp = fs::mount_esp(image)?;

// New (v0.9.x, after SDS-1):
let mut volumes = partitions::mount_all_volumes(image)?;
let esp_idx = volumes.iter().position(|v| v.identity().backend_tag == "fat"
                                       && v.identity().partition_guid == Some(ESP_GUID))
                    .ok_or(Status::NOT_FOUND)?;
let esp = &mut volumes[esp_idx]; // or however the split-borrow pattern wants it.
```

The ESP remains a designated volume for LamBoot's own writes (via `EspWriter`) even after the abstraction lands — Layer 2 is about reading any FS; writing is still ESP-only as policy.

### 9.2 `discovery.rs`

BLS discovery (today: `discovery.rs` scans ESP only at `\loader\entries\*.conf`) becomes "iterate over `&mut [Volume]` and scan each". This is mostly a SDS-5 change but SDS-1 provides the shape.

### 9.3 `boot.rs`

`load_efi_image` and `boot_linux` will accept a `&mut Volume` instead of the concrete `EspVolume`. SDS-3's PE loader will call `volume.open_stream(path)?` for kernels > 16 MiB, `volume.read(path)?` for smaller images.

### 9.4 `trust_log.rs`

Gains a method for recording backend-tagged reads:

```rust
pub(crate) fn record_read(&mut self, backend: &'static str, path: &str, size: usize, note: &str);
```

Used by the PE loader at verification time to record which backend sourced the bytes. Example: `verified_via="ext4-view@0.9.3" path="/boot/vmlinuz-6.12"`.

---

## 10. Test plan

### 10.1 Mock backend

A `MockBackend` used by unit tests in a future host-runnable test crate (uefi targets can't host tests, so the mock lives in a separate `-dev` crate or behind `cfg(test)` on a host build). Exposes a `HashMap<PathBuf, Vec<u8>>` and a failure-injection knob.

### 10.2 Required unit tests (mock backend)

1. `read` returns bytes correctly.
2. `read_at(offset, len)` returns correct slice; handles offset past EOF (returns empty); returns shorter-than-requested at end-of-file.
3. `exists` returns `Ok(true)` / `Ok(false)` correctly.
4. `exists` never returns `NotFound` as an error.
5. `read_dir` returns all immediate children; excludes `.` and `..`.
6. `metadata` returns correct `FileKind` and `size`.
7. `Path::from_str` canonicalizes all documented edge cases (§3.1).
8. `FsError::as_log_token` returns the correct stable token for every variant.
9. Cache: reading the same `(path, offset, len)` twice returns identical bytes; second call does not go through the backend (verified by failure injection on the mock).
10. `EspWriter::new()` returns `None` when the underlying volume isn't FAT.
11. `Volume::open_stream` returns `Unsupported` for non-streaming backends.

### 10.3 Required integration tests (FAT backend + real UEFI)

Built into a harness that runs in QEMU + OVMF via `run-qemu.sh` with a pre-populated FAT image. Not automated host-side; exercised by `./run-qemu-fs-backend-test.sh` (to be authored alongside impl).

1. Read `\EFI\LamBoot\policy.toml` from the ESP → contents match the file written in the test-image build step.
2. Read a 2 MiB file via `read_at(offset=1_000_000, len=4096)` → contents match a reference slice.
3. `exists` on present and absent paths.
4. `read_dir("/EFI/LamBoot")` → returns `["lambootx64.efi", "drivers", "modules", "policy.toml", "reports"]` (order unspecified, content verified set-equal).
5. Streaming: read a 10 MiB file in 4 KiB chunks, SHA-256 at the end matches reference.

### 10.4 Required regression tests (real distro images)

SDS-2 brings these — mentioned here for continuity. A `fleet-regression.sh` that walks a fleet of FAT + ext4 images captured from real distro installs and verifies read results match expected bytes.

---

## 11. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| FAT adapter per-call open-protocol overhead | Low | Boot-time code; measurement noise. If profiling shows it dominant, add a session handle as an option, gated on benchmark data. |
| `Box<dyn FsBackend>` dynamic dispatch overhead | Low | One vtable lookup per call. Boot-critical path has dozens of such calls. Acceptable. |
| Cache size 8 MiB chosen without data | Medium | v0.9.0 ships with `#[allow(clippy::…)]`-free constant. v0.9.x polish: promote to `policy.toml` field, default to measured best for a representative boot. |
| `Path` newtype adds call-site friction | Low | Offset by removing backslash-vs-forward-slash ambiguity across the codebase. Net readability win. |
| PE loader (SDS-3) streaming contract not fully known yet | Medium | `FsBackend::open_stream` is optional and returns `Unsupported` by default. SDS-3 may request a richer streaming interface; if so, SDS-1's trait gains a method and backends opt in. Change is additive. |
| ext4 backend (SDS-2) might need features this trait doesn't expose (e.g. sparse-range iteration) | Low | Additive extension path: if SDS-2 needs `extents_iter`, add a method with default impl returning `Unsupported`. Backends opt in. |
| `Uuid` vs `uefi::Guid` duality in `VolumeIdentity` | Low | Documented in §3.2; convention: partition GUID → `Guid`, FS UUID → `Uuid`. One field each. |
| Volume ordering not specified across backends | Low | Consumers sort by `VolumeIdentity.index` (assignment order) if they care. BLS discovery uses a spec-defined order independent of mount order. |

---

## 12. Acceptance criteria

SDS-1 implementation is complete when all of the following are true:

- [ ] `fs_backend.rs` compiles with `cargo check --target x86_64-unknown-uefi` and `cargo check --target aarch64-unknown-uefi`.
- [ ] `fs_backend_fat.rs` implements `FsBackend` for `FatBackend` and all §10.3 integration tests pass against it in QEMU.
- [ ] `fs.rs` is refactored: no more `EspVolume`; `Volume` + coordinator are in place. All existing consumers (`discovery.rs`, `boot.rs`, `main.rs`, `bls.rs`, `policy.rs`, `uki.rs`, `health.rs`, `report.rs`, `bootlog.rs`, `trust_log.rs`, `drivers.rs`, `preflight.rs`) compile unchanged on their logic side and only import changes.
- [ ] `EspWriter` exists and is the ONLY code path that writes to the ESP. A `grep` of the codebase finds zero direct `SimpleFileSystem::open_volume` + `FileMode::CreateReadWrite` outside `fs_writer.rs`.
- [ ] All §10.2 unit tests pass on the host (host-test crate is acceptable for this, per §10.1).
- [ ] `fs_backend_ext4.rs` skeleton compiles (returns `Unsupported` for every method) — unblocks SDS-2.
- [ ] `ARCHITECTURE-LAYERS.md` already lists this layer correctly; no changes required there.
- [ ] `CHANGELOG.md` gets an entry under an `Unreleased` section.
- [ ] No `#[allow(clippy::…)]` introduced; no new `unsafe` without `SAFETY:` comment.
- [ ] Pre-commit hook (fmt + clippy + check) passes on the tip commit.
- [ ] One fleet-test VM (EndeavourOS VM 123, the known-good v0.8.3 configuration) reboots successfully through a LamBoot built from post-SDS-1 code — no regression vs v0.8.3 behavior.

---

## 13. Sign-off

This spec is **implementation-ready** when:
- Founder has reviewed and explicitly acknowledged.
- No unresolved questions remain in §11 above "Medium" severity.

Implementation PR(s) must cite this SDS in their description and must not deviate from §2, §3, §4, §6 without a documented amendment to this file.

---

## Appendix A — Rust idioms reference

For implementers new to the codebase, a few conventions used throughout:

- **Orphan rule & trait objects.** `Box<dyn FsBackend>` is object-safe because every method takes `self` / `&mut self` by value/reference and no method uses generics. Keep it that way; adding `fn foo<T: Something>(&mut self, …)` would break object-safety.
- **Error propagation.** `?` everywhere. The top-level caller (`run_bootloader` in `main.rs`) maps `FsError` to trust-log events and either continues (for non-fatal) or bubbles to UEFI `Status`.
- **`#[expect]` instead of `#[allow]`.** Per `CLAUDE.md`. Every suppression needs a written reason.
- **`Arc<[u8]>` in the cache.** Not `Vec<u8>`. Allows multiple consumers to hold the same byte slice; avoids copies.
- **No `async`.** UEFI boot-services are sync. A runtime would be 300 KB of code LamBoot doesn't need.

## Appendix B — References

- `docs/ARCHITECTURE-LAYERS.md` — Layer 2 inventory.
- `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` — §5.1 ext4-view adoption rationale, §5.7 native-PE-loader prerequisites.
- `lamboot-core/src/fs.rs` — current v0.8.3 `EspVolume` implementation (to be replaced).
- `lamboot-core/src/partitions.rs` — current `BlockIO` enumeration code (volume mount hooks go here).
- uefi-rs docs: `https://docs.rs/uefi/latest/uefi/proto/media/block/struct.BlockIO.html`
- ext4-view trait: `https://docs.rs/ext4-view/0.9.3/ext4_view/trait.Ext4Read.html`
- Boot Loader Specification: `https://systemd.io/BOOT_LOADER_SPECIFICATION/` — §4.5 filesystem-path conventions.
