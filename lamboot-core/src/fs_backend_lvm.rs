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
//!  BlockIoPvReader  ── embedded_io::Read + Seek over the partition's bytes
//!    │
//!    ▼
//!  lamlvm::Lvm2::open  ── parses PV label + VG metadata
//!    │
//!    ▼
//!  lamlvm::OwnedLvReader  ── embedded_io::Read + Seek over LV bytes
//!    │
//!    ▼
//!  Ext4Read adapter  ── seek + read_exact for ext4-view
//!    │
//!    ▼
//!  ext4_view::Ext4  ── filesystem reads
//! ```
//!
//! Two adapter layers, both `safe` (no `unsafe` in this file). The
//! BlockIO → embedded_io bridge mirrors the established pattern in
//! `fs_backend_ext4::BlockIoReader` / `fs_backend_btrfs`, but exposes a
//! streaming-cursor interface (embedded_io requires position state)
//! instead of the random-access `Ext4Read::read(start, dst)` shape.
//!
//! Scope (v0.11.0): linear LV on single PV — the only layout used by
//! Proxmox VE's default `pve-root`. Non-linear segments error cleanly
//! at LV-open time with `OpenLvError::Unsupported`.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::error::Error;

use embedded_io::{ErrorType, Read as EioRead, Seek as EioSeek, SeekFrom as EioSeekFrom};
use ext4_view::{Ext4, Ext4Read};
use uefi::{
    boot::{self, ScopedProtocol},
    proto::media::block::BlockIO,
    Handle,
};

use crate::{
    fs_backend::{BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path, Uuid},
    fs_backend_ext4::compute_aligned_read,
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
    /// (the dispatcher's `probe_lv_superblock` does this); this factory
    /// will return `FsError::Corrupt` or similar if the bytes aren't a
    /// valid ext4 superblock, since `Ext4::load` re-validates.
    ///
    /// Replaces the v0.11.13 `::open(handle, lv_name)` API. Direct
    /// callers should now go through the dispatch entry point.
    pub(crate) fn from_lv_parts(
        owned: lamlvm::OwnedLvReader<BlockIoPvReader>,
        vg_lv: String,
    ) -> Result<Self, FsError> {
        let reader = LvExt4Adapter { lv: owned };
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
// BlockIO → embedded_io adapter (the PV reader)
// ---------------------------------------------------------------------------

/// Streaming `embedded_io::Read + Seek` over a UEFI BlockIO partition,
/// suitable as input to `lamlvm::Lvm2::open`. Maintains a virtual cursor
/// in byte space; each read computes the LBA range covering the cursor
/// and reads it block-aligned, copying out the requested slice.
///
/// Distinct from `fs_backend_ext4::BlockIoReader` because lamlvm
/// requires a streaming `Read + Seek` interface (position is implicit
/// state), while ext4-view's `Ext4Read::read(start, dst)` is random-access.
///
/// Visibility is `pub(crate)` so the LVM-on-btrfs backend
/// (`fs_backend_lvm_btrfs`) can reuse the same PV adapter without
/// duplicating ~100 lines of BlockIO/embedded_io plumbing. The PV
/// adapter is filesystem-agnostic — only the layer below the LV
/// (ext4-view vs lambutter) varies.
pub(crate) struct BlockIoPvReader {
    block_io: ScopedProtocol<BlockIO>,
    media_id: u32,
    block_size: u64,
    position: u64,
    /// Cached total media bytes for `SeekFrom::End`. `BlockIO::media`
    /// returns the last LBA index, not the count.
    media_len: u64,
}

impl BlockIoPvReader {
    pub(crate) fn open(handle: Handle) -> Result<Self, FsError> {
        let block_io = boot::open_protocol_exclusive::<BlockIO>(handle)?;
        let media = block_io.media();
        let media_id = media.media_id();
        let block_size = u64::from(media.block_size());
        if block_size == 0 {
            return Err(FsError::Unsupported("block_size=0"));
        }
        // last_block is the highest valid LBA index, so byte count is
        // (last_block + 1) * block_size. Use saturating math to avoid
        // overflow if firmware reports something pathological.
        let last_block = media.last_block();
        let media_len = (last_block.saturating_add(1)).saturating_mul(block_size);
        Ok(Self {
            block_io,
            media_id,
            block_size,
            position: 0,
            media_len,
        })
    }
}

#[derive(Debug)]
pub(crate) struct PvIoError(pub(crate) &'static str);

impl core::fmt::Display for PvIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PV I/O error: {}", self.0)
    }
}

impl embedded_io::Error for PvIoError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl ErrorType for BlockIoPvReader {
    type Error = PvIoError;
}

impl EioRead for BlockIoPvReader {
    fn read(&mut self, dst: &mut [u8]) -> Result<usize, Self::Error> {
        if dst.is_empty() {
            return Ok(0);
        }
        // Clamp the read against media length so seeking past EOF
        // followed by read() returns Ok(0) rather than an underflow.
        let remaining = self.media_len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0);
        }
        let want = (dst.len() as u64).min(remaining) as usize;
        let dst = &mut dst[..want];

        let (first_lba, aligned_len, intra) =
            compute_aligned_read(self.position, dst.len(), self.block_size)
                .ok_or(PvIoError("alignment-compute overflow"))?;

        // Allocate per-call. PV reads at open time are infrequent (label
        // + header + metadata-area-header + metadata text). LV reads
        // after open go through a separate adapter at the LV layer, not
        // back through this PV cursor.
        let mut buf = vec![0u8; aligned_len];
        self.block_io
            .read_blocks(self.media_id, first_lba, &mut buf)
            .map_err(|_| PvIoError("BlockIO::read_blocks failed"))?;

        dst.copy_from_slice(&buf[intra..intra + dst.len()]);
        self.position = self.position.saturating_add(dst.len() as u64);
        Ok(dst.len())
    }
}

impl EioSeek for BlockIoPvReader {
    fn seek(&mut self, pos: EioSeekFrom) -> Result<u64, Self::Error> {
        let new = match pos {
            EioSeekFrom::Start(x) => x,
            EioSeekFrom::End(x) => {
                let signed = i64::try_from(self.media_len)
                    .map_err(|_| PvIoError("media_len overflows i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(PvIoError("seek-from-end overflow"))?;
                u64::try_from(t).map_err(|_| PvIoError("seek-from-end negative"))?
            }
            EioSeekFrom::Current(x) => {
                let signed = i64::try_from(self.position)
                    .map_err(|_| PvIoError("position overflows i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(PvIoError("seek-from-current overflow"))?;
                u64::try_from(t).map_err(|_| PvIoError("seek-from-current negative"))?
            }
        };
        // Allow seeking past EOF (matches std::io::Seek semantics).
        // Subsequent read() returns Ok(0).
        self.position = new;
        Ok(new)
    }
}

// ---------------------------------------------------------------------------
// LV → Ext4Read adapter
// ---------------------------------------------------------------------------

/// Wraps an `OwnedLvReader` to satisfy ext4-view's `Ext4Read::read(start, dst)`
/// shape. Performs seek-then-read-exact, looping over short reads (which
/// the LV reader produces at segment boundaries).
struct LvExt4Adapter {
    lv: lamlvm::OwnedLvReader<BlockIoPvReader>,
}

impl Ext4Read for LvExt4Adapter {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        self.lv.seek(EioSeekFrom::Start(start_byte)).map_err(|e| {
            Box::new(LvAdapterError(alloc::format!("seek: {e:?}"))) as Box<dyn Error + Send + Sync>
        })?;
        let mut filled = 0;
        while filled < dst.len() {
            let n = self.lv.read(&mut dst[filled..]).map_err(|e| {
                Box::new(LvAdapterError(alloc::format!("read: {e:?}")))
                    as Box<dyn Error + Send + Sync>
            })?;
            if n == 0 {
                return Err(Box::new(LvAdapterError(String::from(
                    "EOF before read_exact completed",
                ))));
            }
            filled += n;
        }
        Ok(())
    }
}

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

pub(crate) fn translate_open_lv_error(e: &lamlvm::OpenLvError<PvIoError>) -> FsError {
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
