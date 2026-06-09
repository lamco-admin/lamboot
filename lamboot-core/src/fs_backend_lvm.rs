//! Layer: 2 — Storage & Filesystems.
//!
//! LVM2 read backend — bridges lamlvm's `OwnedLvReader` into LamBoot's
//! `FsBackend` trait via ext4-view.
//!
//! Architecture:
//!
//! ```text
//!  UEFI BlockIO
//!    │  (block-granular sector reads)
//!    ▼
//!  BlockIoSource  ── embedded_io::Read + Seek over the partition's bytes (block_source)
//!    │
//!    ▼
//!  lamlvm::Lvm2::open  ── parses PV label + VG metadata
//!    │
//!    ▼
//!  lamlvm::OwnedLvReader  ── embedded_io::Read + Seek over LV bytes
//!    │
//!    ▼
//!  block_source::SourceReader  ── seek + read_exact for ext4-view (Ext4Read)
//!    │
//!    ▼
//!  ext4_view::Ext4  ── filesystem reads
//! ```
//!
//! Both bridge layers are now shared, `safe` infrastructure in `block_source`:
//! `BlockIoSource` is the BlockIO → embedded_io streaming cursor, and
//! `SourceReader` is the embedded_io → `Ext4Read` random-access adapter. This
//! file only wires them to ext4-view; the btrfs backend wires the same
//! `SourceReader` to lambutter's `BlockRead`.
//!
//! Scope (v0.11.0): linear LV on single PV — the only layout used by
//! Proxmox VE's default `pve-root`. Non-linear segments error cleanly
//! at LV-open time with `OpenLvError::Unsupported`.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::error::Error;

use ext4_view::Ext4;

use crate::{
    block_source::{BlockIoError, BlockIoSource, SourceReader},
    fs_backend::{BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path, Uuid},
};

/// Backend tag — surfaces in trust-log events. The format
/// `lvm+ext4-view@VG/LV` lets audit consumers see at a glance which LV
/// sourced any kernel bytes that get loaded.
pub(crate) const LVM_EXT4_BACKEND_TAG: BackendTag = "lvm+ext4-view@0.9.3";

/// A mounted ext4-on-LV volume.
pub(crate) struct LvmExt4Backend {
    ext4: Ext4,
    fs_uuid: Option<Uuid>,
    label: Option<String>,
    /// Cached `vg/lv` identifier, surfaced in diagnostics.
    vg_lv: String,
}

impl LvmExt4Backend {
    /// Construct an ext4-on-LV backend from a pre-opened `OwnedLvReader`.
    ///
    /// Intended to be called by `fs_backend_lvm_dispatch::open_lvm_lv_backend`
    /// AFTER it has opened the LV and probed the superblock. The caller
    /// is responsible for ensuring the LV bytes actually contain ext4
    /// (the dispatcher's `probe_source_superblock` does this); this factory
    /// will return `FsError::Corrupt` or similar if the bytes aren't a
    /// valid ext4 superblock, since `Ext4::load` re-validates.
    ///
    /// Replaces the v0.11.13 `::open(handle, lv_name)` API. Direct
    /// callers should now go through the dispatch entry point.
    pub(crate) fn from_lv_parts(
        owned: lamlvm::OwnedLvReader<BlockIoSource>,
        vg_lv: String,
    ) -> Result<Self, FsError> {
        let reader = SourceReader::new(owned);
        let ext4 =
            Ext4::load(Box::new(reader)).map_err(crate::fs_backend_ext4::translate_ext4_error)?;

        let label = ext4
            .label()
            .to_str()
            .ok()
            .filter(|s| !s.is_empty())
            .map(String::from);
        let fs_uuid = Some(Uuid::from_bytes(*ext4.uuid().as_bytes()));

        log::info!(
            "LVM+ext4 mounted: {} fs_uuid={:?}",
            vg_lv,
            fs_uuid.map(|u| alloc::format!("{u}"))
        );

        Ok(Self {
            ext4,
            fs_uuid,
            label,
            vg_lv,
        })
    }

    #[expect(
        dead_code,
        reason = "retained for trust-log attribution paths that read the vg_lv \
                  string out of the backend; current callers read it via the \
                  dispatch struct instead"
    )]
    pub(crate) fn vg_lv(&self) -> &str {
        &self.vg_lv
    }
}

impl FsBackend for LvmExt4Backend {
    fn tag(&self) -> BackendTag {
        LVM_EXT4_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let p = ext4_path(path);
        self.ext4
            .read(p)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let p = ext4_path(path);
        let mut file = self
            .ext4
            .open(p)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
        file.seek_to(offset)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
        let mut buf = vec![0u8; len];
        let n = file
            .read_bytes(&mut buf)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let p = ext4_path(path);
        self.ext4
            .exists(p)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let p = ext4_path(path);
        let md = self
            .ext4
            .metadata(p)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let p = ext4_path(path);
        let iter = self
            .ext4
            .read_dir(p)
            .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
        let mut out = Vec::new();
        for entry in iter {
            let entry = entry.map_err(crate::fs_backend_ext4::translate_ext4_error)?;
            let name_ref = entry.file_name();
            let name = name_ref
                .as_str()
                .map_err(|_| FsError::InvalidPath("non-utf8 ext4 filename"))?;
            if name == "." || name == ".." {
                continue;
            }
            let md = entry
                .metadata()
                .map_err(crate::fs_backend_ext4::translate_ext4_error)?;
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
// Adapter error
// ---------------------------------------------------------------------------

/// String-carrying error used by the LVM error translators below to wrap a
/// `lamlvm` I/O failure into `FsError::BackendOther`. (The LV→ext4 read adapter
/// that previously also used it now lives in `block_source::SourceReader`.)
#[derive(Debug)]
struct LvAdapterError(String);

impl core::fmt::Display for LvAdapterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "LV→ext4 adapter error: {}", self.0)
    }
}

impl Error for LvAdapterError {}

// ---------------------------------------------------------------------------
// Error translation
// ---------------------------------------------------------------------------

pub(crate) fn translate_lvm_error(e: &lamlvm::Error) -> FsError {
    match e {
        lamlvm::Error::WrongMagic => FsError::Unsupported("LVM PV: wrong label magic"),
        lamlvm::Error::MultipleVGs => FsError::Unsupported("LVM PV: multiple VGs"),
        lamlvm::Error::PVDoesntContainItself => FsError::Corrupt("LVM PV: metadata mismatch"),
        lamlvm::Error::MissingMetadata => FsError::Corrupt("LVM PV: missing metadata area"),
        lamlvm::Error::MetadataNotUtf8 => FsError::Corrupt("LVM PV: metadata not UTF-8"),
        lamlvm::Error::Io { .. } | lamlvm::Error::UnexpectedEof => {
            FsError::BackendOther(Box::new(LvAdapterError(alloc::format!("PV I/O: {e}"))))
        }
        lamlvm::Error::Parse { .. } => FsError::Corrupt("LVM PV: parse error"),
        lamlvm::Error::Serde { .. } => FsError::Corrupt("LVM PV: metadata deserialize error"),
    }
}

pub(crate) fn translate_open_lv_error(e: &lamlvm::OpenLvError<BlockIoError>) -> FsError {
    match e {
        lamlvm::OpenLvError::Pv(inner) => {
            FsError::BackendOther(Box::new(LvAdapterError(alloc::format!("LV PV: {inner}"))))
        }
        lamlvm::OpenLvError::NoSegment => FsError::Corrupt("LV has no segments"),
        lamlvm::OpenLvError::Unsupported(msg) => FsError::Unsupported(msg),
        lamlvm::OpenLvError::SeekOverflow => FsError::Corrupt("LV seek-arithmetic overflow"),
    }
}

// ---------------------------------------------------------------------------
// Shared ext4-path / metadata translators (duplicated from fs_backend_ext4
// to keep this module self-contained; the originals are `pub(crate)`-only
// in fs_backend_ext4 — promoting them is out of scope for this change).
// ---------------------------------------------------------------------------

fn ext4_path(path: &Path) -> ext4_view::Path<'_> {
    ext4_view::Path::new(path.as_str())
}

fn translate_metadata(md: &ext4_view::Metadata) -> Metadata {
    Metadata {
        kind: translate_file_type(md.file_type()),
        size: md.len(),
        mode: u32::from(md.mode()),
        read_only: (md.mode() & 0o200) == 0,
        source_backend: LVM_EXT4_BACKEND_TAG,
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
