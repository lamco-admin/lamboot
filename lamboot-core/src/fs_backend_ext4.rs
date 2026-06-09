//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only ext4 backend.
//!
//! Implements `FsBackend` over the [`ext4-view`] crate (v0.9.3, pinned
//! exactly in `Cargo.toml` per SDS-2 §11.1). ext4-view is a pure-Rust,
//! no_std, read-only ext4 reader; LamBoot feeds it disk bytes via a
//! `DiskIoReader` adapter that wraps UEFI's `DiskIo` protocol.
//!
//! # Why this exists
//!
//! Loading a UEFI filesystem driver through `BS->LoadImage` +
//! `StartImage` triggers shim 15.8's `ShimLock` uninstall, which breaks
//! later kernel verification. A native Rust reader embedded in the
//! LamBoot binary sidesteps that path entirely. Same security property
//! plus a stronger claim than any FS-driver loader: a read-only crate
//! cannot modify `/boot` by construction.
//!
//! # Architectural position
//!
//! This file is one side of SDS-2. The other side is:
//!   * `partitions.rs::probe_superblock` — identifies ext4 volumes at
//!     mount time by reading superblock magic. SDS-1 landed the
//!     unified probe; SDS-2 consumes its `FsInfo { fs_type: Ext4, .. }`
//!     output without adding a parallel probe.
//!   * `fs.rs::Volume::from_backend` — generic constructor used for
//!     every non-FAT backend.
//!
//! Call site: `main.rs` Phase 6.5 discoverable-partition scan. When the
//! unified probe returns `FsType::Ext4`, `build_ext4_backend` is called
//! and the resulting `Volume` is pushed onto `extra_volumes`.
//!
//! # Error surface
//!
//! Every `ext4_view::Ext4Error` variant is translated to `FsError` per
//! the SDS-2 §7 table. No upstream variants leak through; corruption
//! or incompatibility always ends up as a typed `FsError`.
//!
//! # Protocol choice — BlockIO, not DiskIo
//!
//! The v1.0 spec chose `DiskIo` for its byte-granular read surface;
//! PR-3 bring-up discovered `DiskIo` is NOT installed on partition
//! handles in plain OVMF Q35 firmware (it's a driver-provided optional
//! protocol). `BlockIO` is always present. The adapter therefore wraps
//! `BlockIO` and performs block-alignment internally — the
//! ~30-line read-through that v1.0 spec §5.1 described as "what we'd
//! need to write if we used BlockIO" is in fact what every Rust UEFI
//! ext4 reader does, including Sprout. Spec v1.1 amendment #4
//! documents the reversal.
//!
//! # Security
//!
//! No `unsafe` blocks in this module. All raw-disk access goes through
//! ext4-view's validated superblock + extent logic and uefi-rs safe
//! `BlockIO` wrappers.

use alloc::{boxed::Box, string::String, vec::Vec};

use ext4_view::{Ext4, Ext4Error};
use uefi::Handle;

use crate::{
    block_source::{BlockIoSource, BlockSource, SourceReader},
    fs_backend::{BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path, Uuid},
    partitions::FsInfo,
};

/// Backend tag surfaced via `FsBackend::tag()`. The `@version` suffix
/// propagates into trust-log events so audits can reproduce which
/// ext4-view version produced a given read. Update alongside the
/// Cargo pin in lockstep per SDS-2 §11.2.
pub(crate) const EXT4_BACKEND_TAG: BackendTag = "ext4-view@0.9.3";

/// A mounted ext4 volume. Holds the `Ext4` reader plus probe-time
/// identification metadata. Read-only by construction — there is no
/// write path.
pub(crate) struct Ext4Backend {
    ext4: Ext4,
    fs_uuid: Option<Uuid>,
    /// Label is read from the `ext4_view::Ext4::label()` accessor at
    /// construction. Kept here so `FsBackend::label()` can hand out a
    /// borrowed `&str` without re-entering the ext4 reader.
    label: Option<String>,
}

impl Ext4Backend {
    /// Construct an ext4 backend from a block-device handle that has
    /// been identified as ext4 by the unified `probe_superblock`.
    ///
    /// Ownership of the `DiskIo` protocol transfers into the backend
    /// for its lifetime — per SDS-2 §5.3, ext4 reads are high-frequency
    /// (reading a 30 MiB kernel triggers many superblock / group
    /// descriptor / inode / extent-tree reads) so holding the protocol
    /// open is cheaper than a per-call open.
    pub(crate) fn new(handle: Handle, _info: FsInfo) -> Result<Self, FsError> {
        Self::from_source(BlockIoSource::open(handle)?)
    }

    /// Construct an ext4 backend over any [`BlockSource`] — a whole partition
    /// (`BlockIoSource`), or (via the Seam-C dispatcher) a logical volume or an
    /// `.iso` loopback region. The reader is wrapped in the shared
    /// [`SourceReader`] adapter and boxed into ext4-view, which erases the
    /// concrete source type, so `Ext4Backend` itself stays non-generic.
    pub(crate) fn from_source<S: BlockSource + 'static>(source: S) -> Result<Self, FsError> {
        let reader = SourceReader::new(source);
        let ext4 = Ext4::load(Box::new(reader)).map_err(translate_ext4_error)?;

        // Extract the label via ext4-view's validated superblock
        // accessor rather than a second raw read. `to_str()` trims at
        // the first null and returns `Err` for non-UTF-8; treat either
        // failure mode as "no label" rather than an error — label is
        // cosmetic.
        let label = ext4
            .label()
            .to_str()
            .ok()
            .filter(|s| !s.is_empty())
            .map(String::from);

        // Prefer the ext4-view crate's parsed uuid over the probe's
        // raw-string uuid — it's already been validated by the
        // superblock-load path. `Uuid::as_bytes()` returns `&[u8; 16]`.
        let fs_uuid = Some(Uuid::from_bytes(*ext4.uuid().as_bytes()));

        Ok(Self {
            ext4,
            fs_uuid,
            label,
        })
    }
}

impl FsBackend for Ext4Backend {
    fn tag(&self) -> BackendTag {
        EXT4_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let p = ext4_path(path);
        self.ext4.read(p).map_err(translate_ext4_error)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let p = ext4_path(path);
        let mut file = self.ext4.open(p).map_err(translate_ext4_error)?;
        file.seek_to(offset).map_err(translate_ext4_error)?;
        let mut buf = alloc::vec![0u8; len];
        let n = file.read_bytes(&mut buf).map_err(translate_ext4_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let p = ext4_path(path);
        self.ext4.exists(p).map_err(translate_ext4_error)
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let p = ext4_path(path);
        let md = self.ext4.metadata(p).map_err(translate_ext4_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let p = ext4_path(path);
        let iter = self.ext4.read_dir(p).map_err(translate_ext4_error)?;
        let mut out = Vec::new();
        for entry in iter {
            let entry = entry.map_err(translate_ext4_error)?;
            let name_ref = entry.file_name();
            let name = name_ref
                .as_str()
                .map_err(|_| FsError::InvalidPath("non-utf8 ext4 filename"))?;
            if name == "." || name == ".." {
                continue;
            }
            let md = entry.metadata().map_err(translate_ext4_error)?;
            out.push(DirEntry {
                name: String::from(name),
                kind: translate_file_type(md.file_type()),
                size: md.len(),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Translate a LamBoot `&Path` to an `ext4_view::Path`. Both types use
/// UTF-8 + forward-slash + absolute conventions, so this is a direct
/// wrap — no reformatting.
fn ext4_path(path: &Path) -> ext4_view::Path<'_> {
    ext4_view::Path::new(path.as_str())
}

/// Translate `ext4_view::Ext4Error` into `FsError`. Table per SDS-2 §7.
pub(crate) fn translate_ext4_error(err: Ext4Error) -> FsError {
    match err {
        Ext4Error::NotFound => FsError::NotFound,
        Ext4Error::NotADirectory => FsError::NotDirectory,
        Ext4Error::IsADirectory => FsError::IsDirectory,
        Ext4Error::IsASpecialFile => FsError::Unsupported("special_file"),
        Ext4Error::Encrypted => FsError::Unsupported("encrypted"),
        Ext4Error::Incompatible(_) => FsError::UnsupportedFeature {
            // ext4-view's Incompatible type is opaque as of v0.9.3
            // (changelog v0.9.0: "Made the Incompatible type opaque.").
            // Surface a generic label; the specific feature name is
            // preserved in ext4-view's Display impl which any future
            // trust-log writer can capture via err::to_string().
            feature: "ext4_incompatible_feature",
            backend: EXT4_BACKEND_TAG,
        },
        Ext4Error::Corrupt(_) => FsError::Corrupt("ext4 structural corruption"),
        // Every remaining variant (Io, NotAbsolute, NotASymlink,
        // FileTooLarge, MalformedPath, PathTooLong, TooManySymlinks,
        // plus any future additions to the non-exhaustive
        // `Ext4Error`) surfaces as an opaque backend error. The
        // original Display text is preserved via `Debug`+`Display`
        // impls on the boxed error for log emission.
        other => FsError::BackendOther(Box::new(other)),
    }
}

fn translate_metadata(md: &ext4_view::Metadata) -> Metadata {
    Metadata {
        kind: translate_file_type(md.file_type()),
        size: md.len(),
        mode: u32::from(md.mode()),
        // ext4 owner-write bit: mode & 0o200 set ⇒ writable by owner.
        read_only: (md.mode() & 0o200) == 0,
        source_backend: EXT4_BACKEND_TAG,
    }
}

fn translate_file_type(ft: ext4_view::FileType) -> FileKind {
    if ft.is_dir() {
        FileKind::Directory
    } else if ft.is_regular_file() {
        FileKind::Regular
    } else if ft.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    }
}

// ---------------------------------------------------------------------------
// Tests — host-side, live in the workspace `lamboot-fs-tests` crate.
// Nothing testable lives in this module directly because every entry
// point requires a real UEFI `DiskIo` handle. The host tests construct
// a `Vec<u8>`-backed `Ext4Read` and exercise the translation/error
// surface with the bundled ext4-view test disk images.
// ---------------------------------------------------------------------------
