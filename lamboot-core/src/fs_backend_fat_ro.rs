//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only FAT backend (`FatRo`).
//!
//! Implements `FsBackend` over the `fatfs` crate (rafalh/rust-fatfs, 0.4.0
//! git line — the only version with a `no_std` `IoBase`/`Read`/`Write`/`Seek`
//! surface; crates.io 0.3.6 is `std`/`core_io`-based). LamBoot feeds it disk
//! bytes through `BlockIoFatAdapter`, which (like every other BlockIO reader)
//! reuses `block_source::compute_aligned_read` for the block-alignment maths.
//!
//! # Why this exists
//!
//! FAT is *usually* served by the firmware's own `SimpleFileSystem` driver, so
//! LamBoot leans on the firmware for FAT and reads ext4/btrfs/xfs/f2fs/lvm
//! natively. But that firmware dependency is not unconditional: OVMF on i440FX
//! exposes an SFS handle for a *secondary* FAT partition (a separate `vfat`
//! `/boot`, typical of a BIOS→UEFI-migrated Linux box) yet returns
//! `EFI_UNSUPPORTED` from `open_volume()`, and `connect_controller` does not
//! repair it (root cause: memory obs `4d6f6a9a`). A separate vfat `/boot` the
//! firmware declines is then unreadable. `FatRo` closes that last firmware
//! dependency in the `/boot`-read path.
//!
//! # Read-only by construction
//!
//! `fatfs::FileSystem::new` is bounded on `IO: Read + Write + Seek`, so the IO
//! adapter must *implement* `Write` — but it is a stub that errors, and fatfs
//! never calls it on a pure-read workload. Two invariants keep the volume
//! untouched (verified against the fatfs source):
//!   1. `update_accessed_date(false)` — otherwise `File::read` would stamp the
//!      access date into the dir entry and dirty it, forcing a flush-write.
//!   2. We only ever call read/open methods (never create/write/truncate), so
//!      `set_dirty_flag(true)` is never reached; `FileSystem`/`File` `Drop`
//!      both short-circuit before writing because nothing was dirtied.
//!
//! The backend exposes only the read surface of `FsBackend`. Its distinct
//! `BackendTag` (`FAT_RO_BACKEND_TAG`) is *not* `FatBackend::TAG`, so
//! `fs_writer::EspWriter::new` rejects it — the native FAT path has no
//! constructible write surface, making the no-modify guarantee a type property.
//!
//! # BlockIO open mode — shared, never exclusive
//!
//! A FAT partition's BlockIO is held `BY_DRIVER` by the firmware FAT driver.
//! Opening it exclusively cascades a disconnect of that agent and destabilizes
//! the in-flight boot (memory obs `cf2d8fc9`). The adapter therefore opens
//! BlockIO with `GetProtocol` (non-exclusive, shared) and holds it for the
//! backend's lifetime — the same discipline as
//! `partitions::scan_block_io_partitions_for_lvm`.

use alloc::{boxed::Box, string::String, vec, vec::Vec};

use fatfs::{
    Error as FatError, FileAttributes as FatFileAttributes, FileSystem, FsOptions, IoBase,
    IoError as FatIoError, Read as FatRead, Seek as FatSeek, SeekFrom as FatSeekFrom,
    Write as FatWrite,
};
use uefi::{
    boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol},
    proto::media::block::BlockIO,
    Handle,
};

use crate::{
    block_source::compute_aligned_read,
    fs_backend::{
        checked_full_read_len, BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path,
        Uuid,
    },
};

/// Backend tag surfaced via `FsBackend::tag()` and trust-log events. The
/// `@version` suffix follows the ext4/lvm convention so audits can reproduce
/// which reader produced a given read. **Deliberately distinct from
/// `FatBackend::TAG`** so `EspWriter::new` rejects this backend (see module
/// docs — RO-by-type).
pub(crate) const FAT_RO_BACKEND_TAG: BackendTag = "fatfs-ro@0.4.0-git";

/// A mounted read-only FAT volume read natively over an arbitrary fatfs IO
/// source `A`. Two byte sources are wired:
///   * `BlockIoFatAdapter` — a raw GPT partition's BlockIO (the
///     firmware-independent `/boot` path; see `partitions::mount_xbootldr`).
///   * `fs_backend_lvm_fat::LvFatAdapter` — FAT on a logical volume (the
///     uniform LVM-FS dispatch; see `fs_backend_lvm_dispatch`).
///
/// Read-only by construction regardless of source (see module docs).
pub(crate) struct FatRoBackend<A>
where
    A: FatRead + FatWrite + FatSeek + IoBase<Error = FatRoIoError>,
{
    fs: FileSystem<A>,
    fs_uuid: Option<Uuid>,
    label: Option<String>,
    /// Backend tag — varies by byte source (`fatfs-ro@…` for BlockIO,
    /// `lvm+fatfs-ro@…` for an LV) so trust-log attribution names the source.
    tag: BackendTag,
}

impl FatRoBackend<BlockIoFatAdapter> {
    /// Mount a FAT volume from a partition's block-device handle using the
    /// native reader. The handle must carry `BlockIO` (every partition handle
    /// does). Returns an error if the bytes are not a readable FAT volume.
    pub(crate) fn open(handle: Handle) -> Result<Self, FsError> {
        let adapter = BlockIoFatAdapter::open(handle)?;
        Self::from_adapter(adapter, FAT_RO_BACKEND_TAG)
    }
}

impl<A> FatRoBackend<A>
where
    A: FatRead + FatWrite + FatSeek + IoBase<Error = FatRoIoError>,
{
    /// Construct from any fatfs IO source — the shared path behind the BlockIO
    /// and LV constructors. Applies the RO-correctness mount options (module
    /// docs): never update the accessed date; parse leniently (we only reach
    /// this on a fallback / LV path, and cluster walks still validate per
    /// read).
    pub(crate) fn from_adapter(adapter: A, tag: BackendTag) -> Result<Self, FsError> {
        let options = FsOptions::new().update_accessed_date(false).strict(false);
        let fs = FileSystem::new(adapter, options).map_err(translate_fatfs_error)?;

        let fs_uuid = Some(Uuid::from_fat_serial(fs.volume_id()));
        let label = {
            let l = fs.volume_label();
            if l.is_empty() {
                None
            } else {
                Some(l)
            }
        };

        Ok(Self {
            fs,
            fs_uuid,
            label,
            tag,
        })
    }

    /// Look up a single entry by absolute path, returning its kind/size/RO
    /// attribute. Used by `exists` and `metadata`. `Ok(None)` means "not
    /// present" (including a missing parent directory), distinct from an I/O
    /// error.
    fn find_entry(&self, path: &Path) -> Result<Option<EntryInfo>, FsError> {
        let name = path.file_name();
        if name.is_empty() {
            // The root directory itself.
            return Ok(Some(EntryInfo {
                kind: FileKind::Directory,
                size: 0,
                read_only: false,
            }));
        }

        let root = self.fs.root_dir();
        let parent = path.parent();
        let dir = if parent.as_str() == "/" {
            root
        } else {
            match root.open_dir(parent.as_str()) {
                Ok(d) => d,
                Err(FatError::NotFound) => return Ok(None),
                Err(e) => return Err(translate_fatfs_error(e)),
            }
        };

        for r in dir.iter() {
            let e = r.map_err(translate_fatfs_error)?;
            // FAT is case-insensitive; match the canonical path component
            // ignoring ASCII case (kernel/initrd names are ASCII).
            if e.file_name().eq_ignore_ascii_case(name) {
                return Ok(Some(EntryInfo {
                    kind: if e.is_dir() {
                        FileKind::Directory
                    } else {
                        FileKind::Regular
                    },
                    size: e.len(),
                    read_only: e.attributes().contains(FatFileAttributes::READ_ONLY),
                }));
            }
        }
        Ok(None)
    }
}

/// Minimal per-entry facts needed by `exists`/`metadata`.
struct EntryInfo {
    kind: FileKind,
    size: u64,
    read_only: bool,
}

impl<A> FsBackend for FatRoBackend<A>
where
    A: FatRead + FatWrite + FatSeek + IoBase<Error = FatRoIoError> + 'static,
{
    fn tag(&self) -> BackendTag {
        self.tag
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let root = self.fs.root_dir();
        let mut file = root
            .open_file(path.as_str())
            .map_err(translate_fatfs_error)?;

        // Seek to EOF to learn the size (fatfs reports it from the dir entry),
        // then rewind. Clamp the allocation to the read-contract cap so a
        // hostile size field cannot drive an OOM denial-of-boot.
        let size = file
            .seek(FatSeekFrom::End(0))
            .map_err(translate_fatfs_error)?;
        file.seek(FatSeekFrom::Start(0))
            .map_err(translate_fatfs_error)?;
        let cap = checked_full_read_len(size)?;

        let mut out = vec![0u8; cap];
        let got = fill_from_file(&mut file, &mut out)?;
        out.truncate(got);
        Ok(out)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let root = self.fs.root_dir();
        let mut file = root
            .open_file(path.as_str())
            .map_err(translate_fatfs_error)?;
        file.seek(FatSeekFrom::Start(offset))
            .map_err(translate_fatfs_error)?;

        let mut out = vec![0u8; len];
        let got = fill_from_file(&mut file, &mut out)?;
        out.truncate(got);
        Ok(out)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        Ok(self.find_entry(path)?.is_some())
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let info = self.find_entry(path)?.ok_or(FsError::NotFound)?;
        let mode = match info.kind {
            FileKind::Directory => 0o755,
            FileKind::Regular if info.read_only => 0o444,
            FileKind::Regular => 0o644,
            _ => 0o000,
        };
        Ok(Metadata {
            kind: info.kind,
            size: info.size,
            mode,
            read_only: info.read_only,
            source_backend: self.tag,
        })
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let root = self.fs.root_dir();
        let dir = if path.as_str() == "/" {
            root
        } else {
            root.open_dir(path.as_str())
                .map_err(translate_fatfs_error)?
        };

        let mut out = Vec::new();
        for r in dir.iter() {
            let e = r.map_err(translate_fatfs_error)?;
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            out.push(DirEntry {
                name,
                kind: if e.is_dir() {
                    FileKind::Directory
                } else {
                    FileKind::Regular
                },
                size: e.len(),
            });
        }
        Ok(out)
    }
}

/// Fill `dst` from a fatfs reader, looping over the short reads fatfs returns
/// at cluster boundaries. Returns the number of bytes filled (may be `<
/// dst.len()` at EOF). Generic over `fatfs::Read` so the concrete
/// `File<'_, IO, TP, OCC>` provider types never have to be named.
fn fill_from_file<R>(file: &mut R, dst: &mut [u8]) -> Result<usize, FsError>
where
    R: FatRead + IoBase<Error = FatError<FatRoIoError>>,
{
    let mut filled = 0;
    while filled < dst.len() {
        let n = file
            .read(&mut dst[filled..])
            .map_err(translate_fatfs_error)?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

// ---------------------------------------------------------------------------
// BlockIO → fatfs IO adapter
// ---------------------------------------------------------------------------

/// Streaming `fatfs::Read + Seek` over a UEFI BlockIO partition. Maintains a
/// virtual byte cursor; each read computes the covering LBA range and reads it
/// block-aligned via `compute_aligned_read` (shared with the ext4 backend),
/// copying out the requested slice. `Write` is a stub that errors — fatfs
/// never invokes it on the read-only workload (see module docs).
pub(crate) struct BlockIoFatAdapter {
    block_io: ScopedProtocol<BlockIO>,
    media_id: u32,
    block_size: u64,
    position: u64,
    /// Total media byte count, for `SeekFrom::End` and read clamping.
    media_len: u64,
}

impl BlockIoFatAdapter {
    fn open(handle: Handle) -> Result<Self, FsError> {
        // Non-exclusive GetProtocol open: the firmware FAT driver holds this
        // BlockIO BY_DRIVER, and an exclusive open would disconnect it
        // (memory obs cf2d8fc9). Held for the adapter's lifetime.
        let params = OpenProtocolParams {
            handle,
            agent: boot::image_handle(),
            controller: None,
        };
        // SAFETY: GetProtocol is non-exclusive and does not invalidate other
        // agents (the firmware FAT driver's BY_DRIVER hold stays intact). The
        // returned `ScopedProtocol` is owned by this adapter and closes the
        // protocol on drop; we never alias the inner pointer past that drop.
        let block_io =
            unsafe { boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol) }
                .map_err(FsError::from)?;

        let media = block_io.media();
        let media_id = media.media_id();
        let block_size = u64::from(media.block_size());
        if block_size == 0 {
            return Err(FsError::Unsupported("block_size=0"));
        }
        // last_block is the highest valid LBA, so byte count is
        // (last_block + 1) * block_size. Saturating math guards pathological
        // firmware values.
        let media_len = media
            .last_block()
            .saturating_add(1)
            .saturating_mul(block_size);

        Ok(Self {
            block_io,
            media_id,
            block_size,
            position: 0,
            media_len,
        })
    }
}

/// IO error for the fatfs adapters. Must implement `fatfs::IoError` so fatfs
/// can synthesize its own EOF / write-zero sentinels. Shared by
/// `BlockIoFatAdapter` here and `fs_backend_lvm_fat::LvFatAdapter` so a single
/// `fatfs::Error<FatRoIoError>` translation serves both byte sources.
#[derive(Debug)]
pub(crate) enum FatRoIoError {
    /// A BlockIO sector read failed, or alignment arithmetic overflowed.
    Block(&'static str),
    /// fatfs hit EOF mid-`read_exact`.
    UnexpectedEof,
    /// fatfs `write_all` saw a zero-length write (write path is unused here).
    WriteZero,
    /// A write was attempted on the read-only adapter (should never happen).
    WriteAttempted,
}

impl core::fmt::Display for FatRoIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FatRoIoError::Block(m) => write!(f, "FatRo BlockIO error: {m}"),
            FatRoIoError::UnexpectedEof => f.write_str("FatRo: unexpected end of file"),
            FatRoIoError::WriteZero => f.write_str("FatRo: write zero"),
            FatRoIoError::WriteAttempted => {
                f.write_str("FatRo: write attempted on read-only volume")
            }
        }
    }
}

impl core::error::Error for FatRoIoError {}

impl FatIoError for FatRoIoError {
    fn is_interrupted(&self) -> bool {
        false
    }
    fn new_unexpected_eof_error() -> Self {
        FatRoIoError::UnexpectedEof
    }
    fn new_write_zero_error() -> Self {
        FatRoIoError::WriteZero
    }
}

impl IoBase for BlockIoFatAdapter {
    type Error = FatRoIoError;
}

impl FatRead for BlockIoFatAdapter {
    fn read(&mut self, dst: &mut [u8]) -> Result<usize, Self::Error> {
        if dst.is_empty() {
            return Ok(0);
        }
        // Clamp against media length so a seek past EOF then read returns
        // Ok(0) rather than underflowing.
        let remaining = self.media_len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0);
        }
        let want = (dst.len() as u64).min(remaining) as usize;
        let dst = &mut dst[..want];

        let (first_lba, aligned_len, intra) =
            compute_aligned_read(self.position, dst.len(), self.block_size)
                .ok_or(FatRoIoError::Block("alignment-compute overflow"))?;

        let mut buf = vec![0u8; aligned_len];
        self.block_io
            .read_blocks(self.media_id, first_lba, &mut buf)
            .map_err(|_| FatRoIoError::Block("read_blocks failed"))?;

        dst.copy_from_slice(&buf[intra..intra + dst.len()]);
        self.position = self.position.saturating_add(dst.len() as u64);
        Ok(dst.len())
    }
}

impl FatWrite for BlockIoFatAdapter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize, Self::Error> {
        // Read-only by construction. fatfs never reaches this on the read
        // workload; erroring (rather than panicking) keeps an unexpected
        // write from corrupting the volume.
        Err(FatRoIoError::WriteAttempted)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl FatSeek for BlockIoFatAdapter {
    fn seek(&mut self, pos: FatSeekFrom) -> Result<u64, Self::Error> {
        let new = match pos {
            FatSeekFrom::Start(x) => x,
            FatSeekFrom::End(x) => {
                let signed =
                    i64::try_from(self.media_len).map_err(|_| FatRoIoError::Block("len > i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(FatRoIoError::Block("seek-end overflow"))?;
                u64::try_from(t).map_err(|_| FatRoIoError::Block("seek-end negative"))?
            }
            FatSeekFrom::Current(x) => {
                let signed =
                    i64::try_from(self.position).map_err(|_| FatRoIoError::Block("pos > i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(FatRoIoError::Block("seek-cur overflow"))?;
                u64::try_from(t).map_err(|_| FatRoIoError::Block("seek-cur negative"))?
            }
        };
        // Allow seeking past EOF (std::io::Seek semantics); a later read
        // returns Ok(0).
        self.position = new;
        Ok(new)
    }
}

// ---------------------------------------------------------------------------
// Error translation
// ---------------------------------------------------------------------------

/// Map a `fatfs::Error<FatRoIoError>` into LamBoot's `FsError`. fatfs `Error`
/// is `#[non_exhaustive]`, hence the catch-all arm.
fn translate_fatfs_error(e: FatError<FatRoIoError>) -> FsError {
    match e {
        FatError::NotFound => FsError::NotFound,
        FatError::Io(inner) => FsError::BackendOther(Box::new(inner)),
        FatError::CorruptedFileSystem => FsError::Corrupt("fatfs: corrupted volume"),
        FatError::UnexpectedEof => FsError::Corrupt("fatfs: unexpected eof"),
        FatError::InvalidInput => FsError::InvalidPath("fatfs: invalid input"),
        FatError::AlreadyExists => FsError::Unsupported("fatfs: already exists"),
        FatError::DirectoryIsNotEmpty => FsError::Unsupported("fatfs: directory not empty"),
        FatError::NotEnoughSpace => FsError::Unsupported("fatfs: not enough space"),
        FatError::WriteZero => FsError::Unsupported("fatfs: write zero"),
        FatError::InvalidFileNameLength => FsError::InvalidPath("fatfs: invalid name length"),
        FatError::UnsupportedFileNameCharacter => {
            FsError::InvalidPath("fatfs: unsupported name character")
        }
        _ => FsError::Corrupt("fatfs: unexpected error variant"),
    }
}
