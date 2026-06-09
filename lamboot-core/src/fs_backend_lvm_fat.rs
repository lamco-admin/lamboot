//! Layer: 2 — Storage & Filesystems.
//!
//! FAT-on-LVM read backend — bridges lamlvm's `OwnedLvReader` into the native
//! FAT reader (`fs_backend_fat_ro::FatRoBackend`) via fatfs.
//!
//! Architecture:
//!
//! ```text
//!  UEFI BlockIO
//!    │  (block-granular sector reads)
//!    ▼
//!  BlockIoSource  ── embedded_io::Read + Seek over the partition's bytes (block_source)
//!    │  (shared with the ext4/btrfs LVM backends — same PV adapter)
//!    ▼
//!  lamlvm::Lvm2::open  ── parses PV label + VG metadata
//!    │
//!    ▼
//!  lamlvm::OwnedLvReader  ── embedded_io::Read + Seek over LV bytes
//!    │
//!    ▼
//!  LvFatAdapter  ── fatfs::Read + Write(stub) + Seek over the LV
//!    │
//!    ▼
//!  FatRoBackend<LvFatAdapter>  ── filesystem reads (RO)
//! ```
//!
//! This is the LVM-on-FAT companion to `fs_backend_lvm::LvmExt4Backend` and
//! `fs_backend_lvm_btrfs::LvmBtrfsBackend` — it completes the set of LamBoot's
//! *native* read drivers (ext4, btrfs, FAT) behind the uniform LVM-FS dispatch
//! (`fs_backend_lvm_dispatch`). Identical PV/LV layers; differs only in the
//! LV → filesystem adapter and the filesystem object at the top.
//!
//! ## The adapter is trivial here
//!
//! fatfs's IO traits (`Read`/`Seek`) are *streaming* (implicit position),
//! exactly the shape `embedded_io::Read`/`Seek` already expose on
//! `OwnedLvReader`. So unlike the shared `block_source::SourceReader` (which
//! adapts a streaming LV to the random-access `read(start, dst)` /
//! `read_at(offset, buf)` shape ext4-view and lambutter want), this adapter is
//! a thin trait-name bridge — no seek-then-read-exact dance. `Write` is a stub that
//! errors; fatfs never calls it on the read workload (see
//! `fs_backend_fat_ro` module docs for the RO-correctness argument), and the
//! distinct `FAT_RO_BACKEND_TAG`/`LVM_FAT_RO_BACKEND_TAG` keeps `EspWriter`
//! from ever constructing over either FAT-RO backend.
//!
//! No `unsafe` blocks.

use embedded_io::{Read as EioRead, Seek as EioSeek, SeekFrom as EioSeekFrom};
use fatfs::{IoBase, Read as FatRead, Seek as FatSeek, SeekFrom as FatSeekFrom, Write as FatWrite};

use crate::{
    block_source::BlockIoSource,
    fs_backend::{BackendTag, FsError},
    fs_backend_fat_ro::{FatRoBackend, FatRoIoError},
};

/// Backend tag — surfaces in trust-log events. The `lvm+` prefix mirrors
/// `LVM_EXT4_BACKEND_TAG`/`LVM_BTRFS_BACKEND_TAG` so an audit sees at a glance
/// that the FAT bytes came off a logical volume rather than a raw partition.
pub(crate) const LVM_FAT_RO_BACKEND_TAG: BackendTag = "lvm+fatfs-ro@0.4.0-git";

/// fatfs `Read + Write(stub) + Seek` over an LVM logical volume. A thin
/// trait-name bridge from `embedded_io` (which `OwnedLvReader` already
/// implements) to fatfs's equivalent streaming traits.
pub(crate) struct LvFatAdapter {
    lv: lamlvm::OwnedLvReader<BlockIoSource>,
}

impl IoBase for LvFatAdapter {
    type Error = FatRoIoError;
}

impl FatRead for LvFatAdapter {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        // Short reads at LV segment boundaries are fine: fatfs's read_exact
        // loops over them.
        self.lv
            .read(buf)
            .map_err(|_| FatRoIoError::Block("LV read failed"))
    }
}

impl FatWrite for LvFatAdapter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize, Self::Error> {
        // Read-only by construction (see fs_backend_fat_ro). fatfs never
        // reaches this on the read workload; erroring keeps an unexpected
        // write from corrupting the LV.
        Err(FatRoIoError::WriteAttempted)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl FatSeek for LvFatAdapter {
    fn seek(&mut self, pos: FatSeekFrom) -> Result<u64, Self::Error> {
        let eio = match pos {
            FatSeekFrom::Start(x) => EioSeekFrom::Start(x),
            FatSeekFrom::End(x) => EioSeekFrom::End(x),
            FatSeekFrom::Current(x) => EioSeekFrom::Current(x),
        };
        self.lv
            .seek(eio)
            .map_err(|_| FatRoIoError::Block("LV seek failed"))
    }
}

/// Construct a FAT-on-LVM backend from a pre-opened `OwnedLvReader` positioned
/// at byte 0 of the LV. Called by `fs_backend_lvm_dispatch::open_lvm_lv_backend`
/// after `probe_source_superblock` has identified the LV as FAT. fatfs re-validates
/// the boot sector, so a misidentified LV fails here cleanly.
pub(crate) fn from_lv_parts(
    reader: lamlvm::OwnedLvReader<BlockIoSource>,
) -> Result<FatRoBackend<LvFatAdapter>, FsError> {
    FatRoBackend::from_adapter(LvFatAdapter { lv: reader }, LVM_FAT_RO_BACKEND_TAG)
}
