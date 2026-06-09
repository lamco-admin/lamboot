//! Layer: 2 — Storage & Filesystems.
//!
//! The block-source seam: `BlockSource` (Seam A) and the one generic
//! `SourceReader<S>` adapter (Seam B) that every byte-addressed filesystem
//! reader sits on. See `docs/specs/SPEC-FS-BACKEND-TRAIT.md` §5.2.
//!
//! ## What this replaces
//!
//! Before this module, each "filesystem-over-an-LV" backend carried its own
//! near-identical adapter: `fs_backend_lvm::LvExt4Adapter` (implementing
//! ext4-view's `Ext4Read::read(start, dst)`) and
//! `fs_backend_lvm_btrfs::BtrfsBlockAdapter` (implementing lambutter's
//! `BlockRead::read_at(offset, buf)`). The two wrapped the *same*
//! `OwnedLvReader` with the *same* seek-then-read-exact loop — they differed
//! only in which foreign trait they satisfied and the name of their error
//! type. `SourceReader<S>` writes that loop once and implements both reader
//! traits, so a new filesystem crate that wants either shape gets the LV (or
//! any other `BlockSource`) for free.
//!
//! ## Why a trait and not a concrete type
//!
//! The discipline rule is "no single-implementation traits." `BlockSource` is
//! a deliberate, ratified seam, not premature generality: it already has a
//! real second consumer landing next (`lamoptical`'s `OpticalVolume` reads
//! over a `BlockSource`, and boot-from-ISO's file-backed loopback source is a
//! `BlockSource` over an `FsBackend` file — `SPEC-BOOT-FROM-ISO` §3). The
//! `byte_len()` method is not a marker either: `lambutter::Btrfs::open` needs
//! the volume length, which this lets the backend read straight off the source
//! instead of threading a separate `lv_len` parameter through the dispatcher.
//!
//! `origin()` (the nested-source provenance used for trust-tag composition
//! when sources stack — `STORAGE-LAYERING-AND-DRIVER-INTEGRATION` §4) is
//! intentionally NOT here yet: it gains its first consumer with the nesting
//! work, and the seam stays honest until then.

use alloc::{boxed::Box, format, string::String, vec};
use core::error::Error;

use embedded_io::{ErrorType, Read as EioRead, Seek as EioSeek, SeekFrom as EioSeekFrom};
use ext4_view::Ext4Read;
use lambutter::BlockRead;
use uefi::{
    boot::{self, ScopedProtocol},
    proto::media::block::BlockIO,
    Handle,
};

use crate::{
    fs_backend::{FsError, PathBuf},
    fs_shared::SharedBackend,
};

/// A read-only, byte-addressed source a filesystem can be read over — an LV,
/// a whole partition, an `.iso` file region, or (recursively) another source.
///
/// The supertrait bounds (`embedded_io::Read + Seek`) supply random access via
/// seek-then-read; `byte_len()` supplies the total byte length filesystems need
/// for end-of-volume bounds checks. Object safety is not required — consumers
/// take `S: BlockSource` by generic, not `dyn BlockSource`.
pub(crate) trait BlockSource: EioRead + EioSeek {
    /// Total length of the source in bytes. (Named `byte_len` rather than `len`
    /// because it is a byte count, not an element count — and to stay clear of
    /// the `len`/`is_empty` collection convention, which does not fit a volume.)
    fn byte_len(&self) -> u64;
}

/// Any `lamlvm` logical-volume reader is a `BlockSource`. Blanket over the PV
/// reader type so this module never has to name a concrete backend's PV
/// adapter (which would be a backwards dependency from the seam onto a
/// backend); it also covers a future nested PV (an LV over a loopback source).
impl<R: EioRead + EioSeek> BlockSource for lamlvm::OwnedLvReader<R> {
    fn byte_len(&self) -> u64 {
        lamlvm::OwnedLvReader::len(self)
    }
}

/// The one adapter bridging a [`BlockSource`] into the two random-access reader
/// shapes the filesystem crates expect: ext4-view's `Ext4Read::read(start,
/// dst)` and lambutter's `BlockRead::read_at(offset, buf)`. Both are the same
/// operation — "fill this buffer from this byte offset" — so the seek-then-
/// read-exact loop lives once in [`SourceReader::fill_at`].
pub(crate) struct SourceReader<S: BlockSource> {
    source: S,
}

impl<S: BlockSource> SourceReader<S> {
    pub(crate) fn new(source: S) -> Self {
        Self { source }
    }

    /// Total length of the underlying source, for callers that must pass the
    /// volume size to a filesystem opener (e.g. `lambutter::Btrfs::open`).
    pub(crate) fn byte_len(&self) -> u64 {
        self.source.byte_len()
    }

    /// Seek to `offset` and fill `buf` completely, looping over the short reads
    /// the source produces at segment boundaries. The single implementation
    /// behind both reader-trait impls below.
    fn fill_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), SourceReaderError> {
        if buf.is_empty() {
            return Ok(());
        }
        self.source
            .seek(EioSeekFrom::Start(offset))
            .map_err(|e| SourceReaderError(format!("seek: {e:?}")))?;
        let mut filled = 0;
        while filled < buf.len() {
            let n = self
                .source
                .read(&mut buf[filled..])
                .map_err(|e| SourceReaderError(format!("read: {e:?}")))?;
            if n == 0 {
                return Err(SourceReaderError(String::from("EOF before read completed")));
            }
            filled += n;
        }
        Ok(())
    }
}

impl<S: BlockSource> Ext4Read for SourceReader<S> {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        self.fill_at(start_byte, dst)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

impl<S: BlockSource> BlockRead for SourceReader<S> {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.fill_at(offset_bytes, buf)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

/// The lamfold read-only media stack reads over its own `BlockSource` (the
/// flock's `read_at(off, buf)` + `len()`). It is the same "fill this buffer at
/// this offset" operation as the ext4/btrfs reader shapes above, so one more
/// foreign-trait impl over the same local `SourceReader` finishes the bridge
/// (orphan-rule clean: local type, foreign trait). `SPEC-LAMFOLD-INTEGRATION` §2.
impl<S: BlockSource> lamfold::BlockSource for SourceReader<S> {
    fn len(&self) -> u64 {
        self.byte_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> lamfold::Result<()> {
        self.fill_at(offset, buf)
            .map_err(|_| lamfold::FoldError::Io("block-source read"))
    }
}

/// lamxfs reads over its own `BlockRead::read_at(offset, buf)` — identical in
/// shape to lambutter's (the lamxfs trait doc says so explicitly), so the same
/// `fill_at` loop satisfies it. Fully qualified because the bare `BlockRead`
/// name is already bound to lambutter's trait above (orphan-rule clean: local
/// type, foreign trait). `SPEC-LAMXFS` §8.
impl<S: BlockSource> lamxfs::BlockRead for SourceReader<S> {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.fill_at(offset_bytes, buf)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

/// lamexfat reads over its own `BlockRead::read_at(offset, buf)` — identical in
/// shape to lamxfs's, so the same `fill_at` loop satisfies it. Fully qualified
/// (the bare `BlockRead` name is bound to lambutter's trait above; orphan-rule
/// clean: local type, foreign trait). `SPEC-LAMEXFAT` §8.
impl<S: BlockSource> lamexfat::BlockRead for SourceReader<S> {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.fill_at(offset_bytes, buf)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

/// lamzfs reads each ZFS member device over its own `BlockRead::read_at(offset,
/// buf)` — the same shape as lamexfat/lamxfs, so the same `fill_at` loop serves.
/// One `SourceReader` wraps one pool member (a whole ZFS partition); a mirror or
/// raidz pool builds one `PoolMember<SourceReader<…>>` per member. Fully
/// qualified (the bare `BlockRead` is bound to lambutter's trait above;
/// orphan-rule clean: local type, foreign trait). `SPEC-LAMZFS` §8.
impl<S: BlockSource> lamzfs::BlockRead for SourceReader<S> {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.fill_at(offset_bytes, buf)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

#[derive(Debug)]
pub(crate) struct SourceReaderError(String);

impl core::fmt::Display for SourceReaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "block-source read error: {}", self.0)
    }
}

impl Error for SourceReaderError {}

// ---------------------------------------------------------------------------
// BlockIoSource — the UEFI BlockIO byte source (bottom of the stack)
// ---------------------------------------------------------------------------

/// A streaming `embedded_io::Read + Seek` byte view over a UEFI `BlockIO`
/// partition or whole disk — the canonical bottom-of-stack [`BlockSource`].
/// Holds a virtual byte cursor; each read computes the LBA range covering the
/// cursor and reads it block-aligned (via [`compute_aligned_read`]), copying
/// the requested slice out of the aligned buffer.
///
/// Every BlockIO reader in lamboot-core funnels through this one type: it feeds
/// `lamlvm::Lvm2::open` as the PV reader, and (wrapped in a [`SourceReader`]) it
/// is the reader under a whole-partition ext4 or btrfs backend. Before the seam
/// consolidation it was `fs_backend_lvm::BlockIoPvReader`, with two more
/// near-identical random-access copies in the ext4 and btrfs backends.
pub(crate) struct BlockIoSource {
    block_io: ScopedProtocol<BlockIO>,
    media_id: u32,
    block_size: u64,
    position: u64,
    /// Cached total media bytes for `SeekFrom::End` and [`BlockSource::byte_len`].
    /// `BlockIO::media` reports the last LBA index, not the byte count.
    media_len: u64,
}

impl BlockIoSource {
    /// Open the disk/partition `BlockIO` **exclusively**. Used for raw whole-disk,
    /// partition, and LVM-PV reads, where no firmware FS driver is attached to the
    /// handle so an exclusive grab is both available and desirable.
    pub(crate) fn open(handle: Handle) -> Result<Self, FsError> {
        Self::from_block_io(boot::open_protocol_exclusive::<BlockIO>(handle)?)
    }

    /// Open the `BlockIO` **non-exclusively** (`GetProtocol`). Required for an
    /// optical disc: the firmware ISO9660 FS driver holds the disc's BlockIO
    /// BY_DRIVER (to expose its `SimpleFileSystem`), so an exclusive open is
    /// `ACCESS_DENIED` — and would risk the documented BY_DRIVER disconnect
    /// cascade (`partitions.rs` scan note). `GetProtocol` grants shared read
    /// access that coexists with the driver — the same access
    /// [`scan_optical_handles`](crate::partitions::scan_optical_handles) uses to
    /// probe the PVD. The boot-from-ISO optical path reads the raw ISO9660 through
    /// lamfold-iso over this shared handle rather than the firmware FS driver.
    pub(crate) fn open_shared(handle: Handle) -> Result<Self, FsError> {
        use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
        let params = OpenProtocolParams {
            handle,
            agent: boot::image_handle(),
            controller: None,
        };
        // SAFETY: GetProtocol is non-exclusive and does not disturb the BY_DRIVER
        // agent (the firmware optical FS driver). The returned ScopedProtocol is
        // owned by this BlockIoSource and dropped with it, so the BlockIO
        // interface stays valid for every read_blocks call in between.
        let block_io =
            unsafe { boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol) }?;
        Self::from_block_io(block_io)
    }

    fn from_block_io(block_io: ScopedProtocol<BlockIO>) -> Result<Self, FsError> {
        let media = block_io.media();
        let media_id = media.media_id();
        let block_size = u64::from(media.block_size());
        if block_size == 0 {
            return Err(FsError::Unsupported("block_size=0"));
        }
        // last_block is the highest valid LBA index, so byte count is
        // (last_block + 1) * block_size. Saturating math guards against
        // pathological firmware values.
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

impl BlockSource for BlockIoSource {
    fn byte_len(&self) -> u64 {
        self.media_len
    }
}

#[derive(Debug)]
pub(crate) struct BlockIoError(pub(crate) &'static str);

impl core::fmt::Display for BlockIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "BlockIO source error: {}", self.0)
    }
}

impl embedded_io::Error for BlockIoError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl ErrorType for BlockIoSource {
    type Error = BlockIoError;
}

impl EioRead for BlockIoSource {
    fn read(&mut self, dst: &mut [u8]) -> Result<usize, Self::Error> {
        if dst.is_empty() {
            return Ok(0);
        }
        // Clamp against media length so a seek-past-EOF then read returns Ok(0)
        // rather than underflowing.
        let remaining = self.media_len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0);
        }
        let want = (dst.len() as u64).min(remaining) as usize;
        let dst = &mut dst[..want];

        let (first_lba, aligned_len, intra) =
            compute_aligned_read(self.position, dst.len(), self.block_size)
                .ok_or(BlockIoError("alignment-compute overflow"))?;

        let mut buf = vec![0u8; aligned_len];
        self.block_io
            .read_blocks(self.media_id, first_lba, &mut buf)
            .map_err(|_| BlockIoError("BlockIO::read_blocks failed"))?;

        dst.copy_from_slice(&buf[intra..intra + dst.len()]);
        self.position = self.position.saturating_add(dst.len() as u64);
        Ok(dst.len())
    }
}

impl EioSeek for BlockIoSource {
    fn seek(&mut self, pos: EioSeekFrom) -> Result<u64, Self::Error> {
        let new = match pos {
            EioSeekFrom::Start(x) => x,
            EioSeekFrom::End(x) => {
                let signed = i64::try_from(self.media_len)
                    .map_err(|_| BlockIoError("media_len overflows i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(BlockIoError("seek-from-end overflow"))?;
                u64::try_from(t).map_err(|_| BlockIoError("seek-from-end negative"))?
            }
            EioSeekFrom::Current(x) => {
                let signed = i64::try_from(self.position)
                    .map_err(|_| BlockIoError("position overflows i64"))?;
                let t = signed
                    .checked_add(x)
                    .ok_or(BlockIoError("seek-from-current overflow"))?;
                u64::try_from(t).map_err(|_| BlockIoError("seek-from-current negative"))?
            }
        };
        // Allow seeking past EOF (matches std::io::Seek); the next read returns Ok(0).
        self.position = new;
        Ok(new)
    }
}

// ---------------------------------------------------------------------------
// Block-alignment math (pure)
// ---------------------------------------------------------------------------

/// Compute the block-aligned read parameters for an arbitrary byte-range read
/// over a block-granular UEFI `BlockIO`. Returns `None` on an empty
/// destination, on `start_byte + dst_len` overflow, or on aligned-byte
/// overflow of `usize`.
///
/// Otherwise returns `(first_lba, aligned_len, intra)` such that
/// `read_blocks(first_lba, &mut buf[..aligned_len])` covers the requested
/// range plus any leading/trailing padding, and `buf[intra .. intra + dst_len]`
/// is the slice the caller copies out.
///
/// Pure — no UEFI dependencies. Kept in sync with `fuzz/fuzz_targets/aligned_read.rs`
/// (which carries its own copy). Was `fs_backend_ext4::compute_aligned_read`
/// before the seam consolidation.
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
    if intra.checked_add(dst_len)? > aligned_len {
        return None;
    }
    Some((first_lba, aligned_len, intra))
}

// ---------------------------------------------------------------------------
// FileBlockSource — a file-region on a held FsBackend, as a block device
// ---------------------------------------------------------------------------

/// A read-only [`BlockSource`] over a single file held by an `FsBackend` — an
/// `.iso` on ext4/btrfs/xfs/FAT, **including the ESP**. It owns a *shared* handle
/// to the holding backend (`Rc<RefCell<…>>`, see [`SharedBackend`]), so it is
/// `'static` and dispatches through
/// [`dispatch_fs_over_source`](crate::fs_backend_lvm_dispatch::dispatch_fs_over_source)
/// like any other source. Every read goes through the holding backend's
/// **already-open** handle (`FatBackend::read_at` re-opens only its cached
/// `SimpleFileSystem`, issuing no per-read `open_protocol_exclusive`), so reading
/// an ISO off the ESP never disconnects the FAT driver — the SFS-disconnect
/// hazard is avoided structurally. `SPEC-BOOT-FROM-ISO-DESIGN` §1.3.
///
/// Because `FileBlockSource` is itself a `BlockSource`, the loopback stack nests:
/// a `.squashfs` *inside* a mounted ISO is read by another `FileBlockSource` over
/// the ISO's own backend, dispatched recursively through the same dispatcher.
pub(crate) struct FileBlockSource {
    backend: SharedBackend,
    path: PathBuf,
    /// File size, captured once at construction. The ISO is read-only during
    /// boot, so it cannot change underneath the source.
    len: u64,
    /// Virtual byte cursor for `Read`/`Seek`.
    position: u64,
}

impl FileBlockSource {
    /// Open a file-region source over `path` on the shared `backend`. Stats the
    /// file once for its length. The borrow is released before returning.
    pub(crate) fn new(backend: SharedBackend, path: PathBuf) -> Result<Self, FsError> {
        let len = backend
            .try_borrow_mut()
            .map_err(|_| FsError::Unsupported("file-block-source: holding backend busy"))?
            .metadata(path.as_path())?
            .size;
        Ok(Self {
            backend,
            path,
            len,
            position: 0,
        })
    }
}

#[derive(Debug)]
pub(crate) struct FileSourceError(&'static str);

impl core::fmt::Display for FileSourceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "file-block-source error: {}", self.0)
    }
}

impl embedded_io::Error for FileSourceError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl ErrorType for FileBlockSource {
    type Error = FileSourceError;
}

impl EioRead for FileBlockSource {
    fn read(&mut self, dst: &mut [u8]) -> Result<usize, Self::Error> {
        if dst.is_empty() {
            return Ok(0);
        }
        let remaining = self.len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0); // clean EOF, mirrors BlockIoSource
        }
        let want = (dst.len() as u64).min(remaining) as usize;
        // `read_at` goes through the holding backend's already-open cached handle
        // (no per-call open_protocol_exclusive) — the ESP-safe path. `try_borrow_mut`
        // turns the should-never-happen reentrant borrow into an error, never a panic.
        let got = self
            .backend
            .try_borrow_mut()
            .map_err(|_| FileSourceError("holding backend busy (reentrant borrow)"))?
            .read_at(self.path.as_path(), self.position, want)
            .map_err(|_| FileSourceError("holding read_at failed"))?;
        let n = got.len().min(dst.len());
        dst[..n].copy_from_slice(&got[..n]);
        self.position = self.position.saturating_add(n as u64);
        Ok(n)
    }
}

impl EioSeek for FileBlockSource {
    fn seek(&mut self, pos: EioSeekFrom) -> Result<u64, Self::Error> {
        // Identical checked arithmetic to BlockIoSource::seek; seek-past-EOF is
        // allowed (the next read returns Ok(0)).
        let new = match pos {
            EioSeekFrom::Start(x) => x,
            EioSeekFrom::End(x) => {
                let s =
                    i64::try_from(self.len).map_err(|_| FileSourceError("len overflows i64"))?;
                let t = s
                    .checked_add(x)
                    .ok_or(FileSourceError("seek-from-end overflow"))?;
                u64::try_from(t).map_err(|_| FileSourceError("seek-from-end negative"))?
            }
            EioSeekFrom::Current(x) => {
                let s = i64::try_from(self.position)
                    .map_err(|_| FileSourceError("position overflows i64"))?;
                let t = s
                    .checked_add(x)
                    .ok_or(FileSourceError("seek-from-current overflow"))?;
                u64::try_from(t).map_err(|_| FileSourceError("seek-from-current negative"))?
            }
        };
        self.position = new;
        Ok(new)
    }
}

impl BlockSource for FileBlockSource {
    fn byte_len(&self) -> u64 {
        self.len
    }
}
