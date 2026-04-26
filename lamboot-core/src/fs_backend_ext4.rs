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
//! `StartImage` triggers shim 15.8's `ShimLock` uninstall (see
//! `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md`),
//! which breaks later kernel verification. A native Rust reader
//! embedded in the LamBoot binary sidesteps that path entirely. Same
//! security property plus a stronger claim than any FS-driver loader:
//! a read-only crate cannot modify `/boot` by construction.
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

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::error::Error;

use ext4_view::{Ext4, Ext4Error, Ext4Read};
use uefi::{
    boot::{self, ScopedProtocol},
    proto::media::block::BlockIO,
    Handle,
};

use crate::{
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
        let block_io = boot::open_protocol_exclusive::<BlockIO>(handle)?;
        let media = block_io.media();
        let media_id = media.media_id();
        let block_size = media.block_size() as u64;
        if block_size == 0 {
            return Err(FsError::Unsupported("block_size=0"));
        }
        let reader = BlockIoReader {
            block_io,
            media_id,
            block_size,
        };

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
// BlockIoReader — the Ext4Read adapter
// ---------------------------------------------------------------------------

/// Wraps a `ScopedProtocol<BlockIO>` so ext4-view can pull arbitrary
/// byte slices from a UEFI block device. Internally performs the
/// block-alignment that `BlockIO::read_blocks` requires:
///
/// * First LBA = `start_byte / block_size`
/// * Last LBA covers `start_byte + dst.len() - 1`
/// * Aligned buffer sized to cover the full LBA range
/// * Copy requested bytes out of aligned buffer into `dst`
///
/// Chosen over `DiskIo` because the `DiskIo` protocol is NOT
/// guaranteed to be installed on partition handles in plain OVMF
/// firmware; `BlockIO` is. Same pattern Sprout and other Rust UEFI
/// ext4 readers use. The spec v1.0 §5.1 originally chose `DiskIo` —
/// v1.1 amendment #4 reverts that choice on empirical grounds.
struct BlockIoReader {
    block_io: ScopedProtocol<BlockIO>,
    media_id: u32,
    block_size: u64,
}

/// Compute the block-aligned read parameters for an arbitrary byte-range
/// read over a block-granular UEFI `BlockIO`. Returns `None` on:
///   * empty destination (no work to do; caller short-circuits)
///   * u64 overflow (`start_byte + dst_len > u64::MAX`)
///   * overflow of the aligned byte count into `usize`
///
/// Otherwise returns `(first_lba, aligned_len, intra)` such that:
///   * `read_blocks(first_lba, &mut buf[..aligned_len])` covers the
///     requested byte range plus any leading/trailing padding,
///   * `buf[intra .. intra + dst_len]` is the byte slice the caller
///     copies into `dst`.
///
/// Pure function — no UEFI dependencies. Kept in sync with
/// `fuzz/fuzz_targets/aligned_read.rs`. SDS-2 PR-4 fuzz target.
pub(crate) fn compute_aligned_read(
    start_byte: u64,
    dst_len: usize,
    block_size: u64,
) -> Option<(u64, usize, usize)> {
    if dst_len == 0 || block_size == 0 {
        return None;
    }
    let first_lba = start_byte / block_size;
    let end_byte = start_byte.checked_add(dst_len as u64)?;
    let last_lba = (end_byte - 1) / block_size;
    let aligned_blocks = last_lba - first_lba + 1;
    let aligned_bytes = aligned_blocks.checked_mul(block_size)?;
    let aligned_len = usize::try_from(aligned_bytes).ok()?;
    let intra = (start_byte - first_lba * block_size) as usize;
    // By construction: intra + dst_len <= aligned_len. Assert for safety.
    if intra.checked_add(dst_len)? > aligned_len {
        return None;
    }
    Some((first_lba, aligned_len, intra))
}

impl Ext4Read for BlockIoReader {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        if dst.is_empty() {
            return Ok(());
        }
        let (first_lba, aligned_len, intra) =
            compute_aligned_read(start_byte, dst.len(), self.block_size)
                .ok_or_else(|| Box::<dyn Error + Send + Sync>::from("read overflow"))?;

        let mut aligned = vec![0u8; aligned_len];
        self.block_io
            .read_blocks(self.media_id, first_lba, &mut aligned)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync + 'static>)?;

        dst.copy_from_slice(&aligned[intra..intra + dst.len()]);
        Ok(())
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
fn translate_ext4_error(err: Ext4Error) -> FsError {
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
