//! Layer: 2 — Storage & Filesystems.
//!
//! LVM-on-FS dispatch: probe-once-and-dispatch for any LV.
//!
//! Replaces the v0.11.13 trial-and-error pattern
//! (try ext4 backend → on failure try btrfs backend) with a clean
//! superblock probe followed by a single backend construction. Scales
//! cleanly to N backends — each new filesystem adds one `LvFsKind`
//! variant + one magic check + one match arm + one factory method.
//!
//! ## Why
//!
//! The original (v0.11.13) wiring opened the PV/VG/LV stack inside
//! `LvmExt4Backend::open`, tried to load ext4 on the LV bytes, and on
//! failure dropped the whole stack and built it again inside
//! `LvmBtrfsBackend::open`. Correct, but:
//!
//! * The PV+VG metadata was parsed twice on the fallback path.
//! * Each new backend deepened the nested-match cascade in main.rs.
//! * Failure-vs-wrong-fs was ambiguous: an ext4 superblock probe failing
//!   could mean "not ext4" OR "corrupted ext4," and the dispatch loop
//!   couldn't distinguish them.
//!
//! This module fixes all three. The PV/VG/LV stack is opened exactly
//! once via `OpenedLv::open`. The LV's first ~68 KB is read once and
//! the superblock magic identifies the filesystem definitively. The
//! caller (main.rs) sees a single `LvmDispatch` outcome and pushes a
//! `Volume` without branching on backend type.
//!
//! ## Adding a new LVM-FS backend
//!
//! Three concrete edits per backend:
//!
//! 1. Add an `LvFsKind` variant (this file).
//! 2. Add a superblock check in `probe_lv_superblock` (this file).
//! 3. Add a match arm in `open_lvm_lv_backend` (this file) that calls
//!    your backend's `from_lv_parts(reader, ...)` factory.
//!
//! The factory itself lives in `fs_backend_lvm_<fs>.rs`. The backend
//! type doesn't need to know about dispatch — only how to wrap an
//! `OwnedLvReader` into a filesystem object.

use alloc::{boxed::Box, format, string::String, vec};

use embedded_io::{Read as EioRead, Seek as EioSeek, SeekFrom as EioSeekFrom};
use uefi::Handle;

use crate::{
    fs_backend::{BackendTag, FsBackend, FsError, Uuid},
    fs_backend_lvm::{translate_lvm_error, translate_open_lv_error, BlockIoPvReader},
};

// ---------------------------------------------------------------------------
// 1. PV+VG+LV access (shared infrastructure)
// ---------------------------------------------------------------------------

/// A successfully opened logical volume: an `OwnedLvReader` positioned
/// at byte 0, plus VG/LV identification. Constructed once per dispatch
/// attempt; consumed when a backend factory takes ownership of the
/// reader.
pub(crate) struct OpenedLv {
    pub(crate) reader: lamlvm::OwnedLvReader<BlockIoPvReader>,
    pub(crate) vg_lv: String,
    pub(crate) lv_len: u64,
}

impl OpenedLv {
    /// Open the LVM PV on `handle`, parse the VG metadata, and open the
    /// LV named `lv_name`. Returns a reader positioned at byte 0 of the
    /// LV plus the identification fields backends need.
    ///
    /// Errors propagate from `lamlvm` (PV-magic, metadata parse, LV
    /// lookup). A "no LV with this name in this VG" outcome surfaces
    /// as `FsError::Unsupported` so the caller can iterate the LV-name
    /// candidate list without misinterpreting it as a real I/O failure.
    pub(crate) fn open(handle: Handle, lv_name: &str) -> Result<Self, FsError> {
        let pv = BlockIoPvReader::open(handle)?;
        let lvm = lamlvm::Lvm2::open(pv).map_err(|e| translate_lvm_error(&e))?;
        let vg_name = String::from(lvm.vg_name());

        // Re-open the PV reader because Lvm2::open consumed the first.
        // lamlvm parses metadata at open time and stores the result;
        // the parsed structures don't keep the reader.
        let pv = BlockIoPvReader::open(handle)?;
        let owned = lvm
            .open_lv_owned_by_name(lv_name, pv)
            .map_err(|e| translate_open_lv_error(&e))?
            .ok_or(FsError::Unsupported("no LV with that name in this VG"))?;

        let lv_len = owned.len();
        let vg_lv = format!("{vg_name}/{lv_name}");
        log::info!(
            "LV opened (dispatch): {} len={} bytes ({} extents)",
            vg_lv,
            lv_len,
            lv_len / lvm.extent_size().max(1),
        );
        Ok(Self {
            reader: owned,
            vg_lv,
            lv_len,
        })
    }
}

// ---------------------------------------------------------------------------
// 2. Superblock probe
// ---------------------------------------------------------------------------

/// Filesystems we can identify via on-LV superblock magic. Variants
/// are added in lockstep with backend factories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LvFsKind {
    /// ext2/3/4 — superblock at byte 1024, magic `0xEF53` at offset 56.
    Ext4,
    /// btrfs — superblock at byte 0x10000 (64 KiB), magic
    /// `"_BHRfS_M"` at offset 0x40 from superblock start.
    Btrfs,
    /// FAT12/16/32 — boot-sector signature `0x55,0xAA` at offset 510 plus a
    /// `"FAT"` BS_FilSysType string (offset 0x36 for FAT12/16, 0x52 for
    /// FAT32). Read by `fs_backend_lvm_fat::LvFatAdapter` via fatfs.
    Fat,
    /// No recognized signature in the probed region. The LV may
    /// contain a filesystem we don't yet support (xfs / f2fs / zfs /
    /// raw / swap) or may be corrupt. Caller treats this as "skip and
    /// move on" rather than an error.
    Unknown,
}

/// Bytes read from the LV during probing. Big enough to cover both
/// ext4 (needs offset 1024 + 64) and btrfs (needs offset 0x10000 + 0x48)
/// in a single read. 68 KiB ≈ one alloc, single I/O round-trip.
const PROBE_BYTES: usize = 65_536 + 4_096;

/// Probe an open LV reader for a known filesystem superblock. Reads
/// up to `PROBE_BYTES` from byte 0 and rewinds the reader before
/// returning, so the caller can pass the same `OwnedLvReader` into a
/// backend factory.
///
/// Short reads (LVs smaller than `PROBE_BYTES`) are handled by checking
/// each magic against the actual filled length. An LV smaller than the
/// ext4 superblock region (very rare — only swap-sized LVs) returns
/// `Unknown`.
pub(crate) fn probe_lv_superblock(
    reader: &mut lamlvm::OwnedLvReader<BlockIoPvReader>,
) -> Result<LvFsKind, FsError> {
    let mut buf = vec![0u8; PROBE_BYTES];

    seek_or_err(reader, 0)?;
    let mut filled = 0usize;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..]).map_err(io_err)?;
        if n == 0 {
            // LV shorter than the probe region. Still inspect what we
            // have — ext4 superblock might fit.
            break;
        }
        filled += n;
    }

    // Rewind for the caller. Failure here is unusual (the reader's
    // seek is just position arithmetic) but we surface it cleanly.
    seek_or_err(reader, 0)?;

    // ext4 / ext3 / ext2 superblock at offset 1024.
    // Layout: struct ext2_super_block, magic at offset 56-57 (s_magic).
    // Value: 0xEF53 little-endian → bytes [0x53, 0xEF].
    const EXT4_SB_OFFSET: usize = 1024;
    const EXT4_MAGIC_OFFSET: usize = 56;
    const EXT4_MAGIC: [u8; 2] = [0x53, 0xEF];
    if filled >= EXT4_SB_OFFSET + EXT4_MAGIC_OFFSET + 2 {
        let m = &buf[EXT4_SB_OFFSET + EXT4_MAGIC_OFFSET..EXT4_SB_OFFSET + EXT4_MAGIC_OFFSET + 2];
        if m == EXT4_MAGIC {
            return Ok(LvFsKind::Ext4);
        }
    }

    // btrfs superblock at offset 0x10000 (65 536).
    // Layout: struct btrfs_super_block, magic at offset 0x40 (64),
    // value `"_BHRfS_M"` (8 bytes).
    const BTRFS_SB_OFFSET: usize = 0x10000;
    const BTRFS_MAGIC_OFFSET: usize = 0x40;
    const BTRFS_MAGIC: &[u8; 8] = b"_BHRfS_M";
    if filled >= BTRFS_SB_OFFSET + BTRFS_MAGIC_OFFSET + 8 {
        let m =
            &buf[BTRFS_SB_OFFSET + BTRFS_MAGIC_OFFSET..BTRFS_SB_OFFSET + BTRFS_MAGIC_OFFSET + 8];
        if m == BTRFS_MAGIC.as_slice() {
            return Ok(LvFsKind::Btrfs);
        }
    }

    // FAT12/16/32 boot sector at offset 0. There is no single FAT magic
    // number, so sniff the boot-sector signature (0x55,0xAA at offset 510)
    // plus a "FAT" BS_FilSysType string (offset 0x36 for FAT12/16, 0x52 for
    // FAT32). Checked AFTER ext4/btrfs (whose magics live deeper in the LV),
    // so a real ext4/btrfs LV never reaches here. fatfs re-validates the boot
    // sector at mount, so a loose match that isn't really FAT fails cleanly
    // downstream rather than mounting garbage.
    const FAT_BOOT_SIG: usize = 510;
    const FAT1216_TYPE_OFFSET: usize = 0x36;
    const FAT32_TYPE_OFFSET: usize = 0x52;
    if filled >= FAT_BOOT_SIG + 2 && buf[FAT_BOOT_SIG] == 0x55 && buf[FAT_BOOT_SIG + 1] == 0xAA {
        let fat1216 = filled >= FAT1216_TYPE_OFFSET + 3
            && &buf[FAT1216_TYPE_OFFSET..FAT1216_TYPE_OFFSET + 3] == b"FAT";
        let fat32 = filled >= FAT32_TYPE_OFFSET + 3
            && &buf[FAT32_TYPE_OFFSET..FAT32_TYPE_OFFSET + 3] == b"FAT";
        if fat1216 || fat32 {
            return Ok(LvFsKind::Fat);
        }
    }

    // Future: XFS (`b"XFSB"` at offset 0), F2FS (magic 0xF2F52010 at
    // offset 1024), ZFS (uberblock at offset 0x40000). Each adds one
    // variant + one block of code here.

    Ok(LvFsKind::Unknown)
}

fn seek_or_err(
    reader: &mut lamlvm::OwnedLvReader<BlockIoPvReader>,
    pos: u64,
) -> Result<(), FsError> {
    reader
        .seek(EioSeekFrom::Start(pos))
        .map(|_| ())
        .map_err(io_err)
}

fn io_err<E: core::fmt::Debug>(e: E) -> FsError {
    FsError::Unsupported(stringify!("LV probe I/O error"))
        .with_debug(e)
        .unwrap_or(FsError::Unsupported("LV probe I/O error"))
}

// Helper: attach debug info to an FsError without a dedicated variant.
trait FsErrorExt {
    fn with_debug<E: core::fmt::Debug>(self, e: E) -> Option<FsError>;
}

impl FsErrorExt for FsError {
    fn with_debug<E: core::fmt::Debug>(self, e: E) -> Option<FsError> {
        log::warn!("LV probe I/O detail: {e:?}");
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// 3. Single dispatch entry point
// ---------------------------------------------------------------------------

/// Outcome of a successful LV mount: a boxed FsBackend trait object
/// plus the identification fields main.rs needs to construct a
/// `VolumeIdentity` and a trust-log event.
///
/// Returning a single struct rather than `Result<EnumOfBackends, _>`
/// keeps the dispatch site in main.rs branchless over backend type —
/// the caller treats every LVM-FS mount uniformly.
pub(crate) struct LvmDispatch {
    pub(crate) backend: Box<dyn FsBackend>,
    pub(crate) backend_tag: BackendTag,
    pub(crate) fs_uuid: Option<Uuid>,
    pub(crate) label: Option<String>,
    pub(crate) vg_lv: String,
}

/// Open the named LV on `handle`, probe its superblock, and construct
/// the appropriate filesystem backend. This is the canonical entry
/// point for LVM-on-FS mounting. Callers should NOT invoke the
/// individual backend `from_lv_parts` factories directly — go through
/// this function so the probe-and-dispatch path stays unified.
pub(crate) fn open_lvm_lv_backend(handle: Handle, lv_name: &str) -> Result<LvmDispatch, FsError> {
    let mut opened = OpenedLv::open(handle, lv_name)?;
    let kind = probe_lv_superblock(&mut opened.reader)?;
    let OpenedLv {
        reader,
        vg_lv,
        lv_len,
    } = opened;

    match kind {
        LvFsKind::Ext4 => {
            let backend =
                crate::fs_backend_lvm::LvmExt4Backend::from_lv_parts(reader, vg_lv.clone())?;
            let fs_uuid = FsBackend::uuid(&backend);
            let label = FsBackend::label(&backend).map(String::from);
            Ok(LvmDispatch {
                backend: Box::new(backend),
                backend_tag: crate::fs_backend_lvm::LVM_EXT4_BACKEND_TAG,
                fs_uuid,
                label,
                vg_lv,
            })
        }
        LvFsKind::Btrfs => {
            let backend = crate::fs_backend_lvm_btrfs::LvmBtrfsBackend::from_lv_parts(
                reader,
                lv_len,
                vg_lv.clone(),
            )?;
            let fs_uuid = FsBackend::uuid(&backend);
            let label = FsBackend::label(&backend).map(String::from);
            Ok(LvmDispatch {
                backend: Box::new(backend),
                backend_tag: crate::fs_backend_lvm_btrfs::LVM_BTRFS_BACKEND_TAG,
                fs_uuid,
                label,
                vg_lv,
            })
        }
        LvFsKind::Fat => {
            let backend = crate::fs_backend_lvm_fat::from_lv_parts(reader)?;
            let fs_uuid = FsBackend::uuid(&backend);
            let label = FsBackend::label(&backend).map(String::from);
            Ok(LvmDispatch {
                backend: Box::new(backend),
                backend_tag: crate::fs_backend_lvm_fat::LVM_FAT_RO_BACKEND_TAG,
                fs_uuid,
                label,
                vg_lv,
            })
        }
        LvFsKind::Unknown => Err(FsError::Unsupported(
            "LV superblock did not match any supported filesystem (ext2/3/4, btrfs, or FAT)",
        )),
    }
}
