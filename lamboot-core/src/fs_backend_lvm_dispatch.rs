//! Layer: 2 — Storage & Filesystems.
//!
//! LVM-on-FS dispatch: probe-once-and-dispatch for any LV.
//!
//! Replaces the v0.11.13 trial-and-error pattern
//! (try ext4 backend → on failure try btrfs backend) with a clean
//! superblock probe followed by a single backend construction. Scales
//! cleanly to N backends — each new filesystem adds one `ProbedFs`
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
//! 1. Add a `ProbedFs` variant (this file).
//! 2. Add a superblock check in `probe_source_superblock` (this file).
//! 3. Add a match arm in `open_lvm_lv_backend` (this file) that calls
//!    your backend's `from_lv_parts(reader, ...)` factory.
//!
//! The factory itself lives in `fs_backend_lvm_<fs>.rs`. The backend
//! type doesn't need to know about dispatch — only how to wrap an
//! `OwnedLvReader` into a filesystem object.

use alloc::{boxed::Box, format, string::String, vec};

use embedded_io::{Seek as EioSeek, SeekFrom as EioSeekFrom};
use uefi::Handle;

use crate::{
    block_source::{BlockIoSource, BlockSource},
    fs_backend::{BackendTag, FsBackend, FsError, Uuid},
    fs_backend_lvm::{translate_lvm_error, translate_open_lv_error},
};

// ---------------------------------------------------------------------------
// 1. PV+VG+LV access (shared infrastructure)
// ---------------------------------------------------------------------------

/// A successfully opened logical volume: an `OwnedLvReader` positioned
/// at byte 0, plus VG/LV identification. Constructed once per dispatch
/// attempt; consumed when a backend factory takes ownership of the
/// reader.
pub(crate) struct OpenedLv {
    pub(crate) reader: lamlvm::OwnedLvReader<BlockIoSource>,
    pub(crate) vg_lv: String,
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
        let pv = BlockIoSource::open(handle)?;
        let lvm = lamlvm::Lvm2::open(pv).map_err(|e| translate_lvm_error(&e))?;
        let vg_name = String::from(lvm.vg_name());

        // Re-open the PV reader because Lvm2::open consumed the first.
        // lamlvm parses metadata at open time and stores the result;
        // the parsed structures don't keep the reader.
        let pv = BlockIoSource::open(handle)?;
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
        })
    }
}

// ---------------------------------------------------------------------------
// 2. Superblock probe
// ---------------------------------------------------------------------------

/// Filesystems we can identify from superblock magic on any [`BlockSource`] —
/// a logical volume, a whole partition, or an `.iso` loopback region. Variants
/// are added in lockstep with backend factories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbedFs {
    /// ext2/3/4 — superblock at byte 1024, magic `0xEF53` at offset 56.
    Ext4,
    /// btrfs — superblock at byte 0x10000 (64 KiB), magic
    /// `"_BHRfS_M"` at offset 0x40 from superblock start.
    Btrfs,
    /// XFS — superblock at byte 0, magic `"XFSB"` at offset 0. Read by the
    /// Lamco-authored `lamxfs` backend (the RHEL-family `/boot` + `/` default).
    Xfs,
    /// FAT12/16/32 — boot-sector signature `0x55,0xAA` at offset 510 plus a
    /// `"FAT"` BS_FilSysType string (offset 0x36 for FAT12/16, 0x52 for
    /// FAT32). Read by `fs_backend_lvm_fat::LvFatAdapter` via fatfs.
    Fat,
    /// EROFS — superblock at byte 1024, 32-bit magic `0xE0F5E1E2` at its start.
    /// Read by the lamfold `lamfold-erofs` frontend (SPEC-LAMFOLD-INTEGRATION).
    Erofs,
    /// ISO 9660 — Primary Volume Descriptor standard-id `"CD001"` at byte 0x8001
    /// (sector 16). Read by lamfold `lamfold-iso`.
    Iso9660,
    /// cramfs — superblock at byte 0, 32-bit magic `0x28CD3D45`. Read by lamfold
    /// `lamfold-cramfs`.
    Cramfs,
    /// romfs — 8-byte signature `"-rom1fs-"` at byte 0. Read by lamfold
    /// `lamfold-romfs`.
    Romfs,
    /// SquashFS — 32-bit magic `0x73717368` ("hsqs") at byte 0. Read by lamfold
    /// `lamfold-squash`.
    Squashfs,
    /// UDF — Anchor Volume Descriptor Pointer (tag id 2) at logical sector 256
    /// (512 KiB, past the in-window probe). Read by lamfold `lamfold-udf`.
    Udf,
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

/// Probe any [`BlockSource`] for a known filesystem superblock. Reads up to
/// `PROBE_BYTES` from byte 0 and rewinds before returning, so the caller can
/// hand the same source to a backend factory.
///
/// Source-generic (Seam C): the same probe serves a logical volume, a whole
/// partition, and an `.iso` loopback region. Short reads (sources smaller than
/// `PROBE_BYTES`) are handled by checking each magic against the actual filled
/// length; a source smaller than the ext4 superblock region returns `Unknown`.
pub(crate) fn probe_source_superblock<S: BlockSource>(reader: &mut S) -> Result<ProbedFs, FsError> {
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
            return Ok(ProbedFs::Ext4);
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
            return Ok(ProbedFs::Btrfs);
        }
    }

    // XFS superblock at byte 0; 4-byte magic "XFSB". Checked here among the
    // real-partition filesystems (lamxfs re-validates version/geometry/CRC at
    // mount, so a loose byte-0 match that isn't really XFS fails cleanly).
    const XFS_MAGIC: &[u8; 4] = b"XFSB";
    if filled >= 4 && &buf[0..4] == XFS_MAGIC.as_slice() {
        return Ok(ProbedFs::Xfs);
    }

    // EROFS superblock at byte 1024; 32-bit magic 0xE0F5E1E2 at its very start
    // (distinct sub-offset from ext4's, whose magic is at 1024+56). Checked
    // before the byte-0 sniffs so a deep, specific magic wins.
    const EROFS_SB_OFFSET: usize = 1024;
    const EROFS_MAGIC: [u8; 4] = [0xE2, 0xE1, 0xF5, 0xE0]; // 0xE0F5E1E2 little-endian
    if filled >= EROFS_SB_OFFSET + 4 {
        let m = &buf[EROFS_SB_OFFSET..EROFS_SB_OFFSET + 4];
        if m == EROFS_MAGIC {
            return Ok(ProbedFs::Erofs);
        }
    }

    // ISO 9660 Primary Volume Descriptor: standard identifier "CD001" at byte
    // 0x8001 (sector 16, +1 past the volume-descriptor type byte).
    const ISO_MAGIC_OFFSET: usize = 0x8001;
    if filled >= ISO_MAGIC_OFFSET + 5 {
        let m = &buf[ISO_MAGIC_OFFSET..ISO_MAGIC_OFFSET + 5];
        if m == b"CD001".as_slice() {
            return Ok(ProbedFs::Iso9660);
        }
    }

    // cramfs superblock at byte 0; 32-bit magic 0x28CD3D45.
    const CRAMFS_MAGIC: [u8; 4] = [0x45, 0x3D, 0xCD, 0x28]; // 0x28CD3D45 little-endian
    if filled >= 4 {
        let m = &buf[0..4];
        if m == CRAMFS_MAGIC {
            return Ok(ProbedFs::Cramfs);
        }
    }

    // romfs at byte 0: the 8-byte signature "-rom1fs-".
    if filled >= 8 && &buf[0..8] == b"-rom1fs-".as_slice() {
        return Ok(ProbedFs::Romfs);
    }

    // SquashFS superblock at byte 0; 32-bit magic 0x73717368 ("hsqs").
    const SQUASHFS_MAGIC: [u8; 4] = [0x68, 0x73, 0x71, 0x73]; // 0x73717368 little-endian
    if filled >= 4 {
        let m = &buf[0..4];
        if m == SQUASHFS_MAGIC {
            return Ok(ProbedFs::Squashfs);
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
            return Ok(ProbedFs::Fat);
        }
    }

    // UDF's Anchor Volume Descriptor Pointer lives at logical sector 256
    // (512 KiB), past PROBE_BYTES — a targeted read rather than inflating every
    // probe to half a megabyte.
    if probe_udf_anchor(reader)? {
        return Ok(ProbedFs::Udf);
    }

    // Future: F2FS (magic 0xF2F52010 at offset 1024), ZFS (uberblock at offset
    // 0x40000). Each adds one variant + one block of code here. (XFS landed.)

    Ok(ProbedFs::Unknown)
}

/// UDF detection: the Anchor Volume Descriptor Pointer sits at logical sector
/// 256 (× 2048 = 512 KiB), past `PROBE_BYTES`. Read its descriptor-tag
/// identifier and check it is 2 (AVDP) — the same test `lamfold-udf`'s own probe
/// applies. Rewinds the reader; a source too short to reach sector 256 is simply
/// not UDF.
fn probe_udf_anchor<S: BlockSource>(reader: &mut S) -> Result<bool, FsError> {
    const UDF_SECTOR: u64 = 2048;
    const AVDP_SECTOR: u64 = 256;
    const TAG_AVDP: u16 = 2;

    seek_or_err(reader, AVDP_SECTOR * UDF_SECTOR)?;
    let mut tag = [0u8; 2];
    let mut filled = 0usize;
    while filled < tag.len() {
        let n = reader.read(&mut tag[filled..]).map_err(io_err)?;
        if n == 0 {
            seek_or_err(reader, 0)?; // shorter than sector 256 → not UDF
            return Ok(false);
        }
        filled += n;
    }
    seek_or_err(reader, 0)?;
    Ok(u16::from_le_bytes(tag) == TAG_AVDP)
}

fn seek_or_err<S: EioSeek>(reader: &mut S, pos: u64) -> Result<(), FsError> {
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
    let kind = probe_source_superblock(&mut opened.reader)?;
    let OpenedLv { reader, vg_lv } = opened;

    match kind {
        ProbedFs::Ext4 => {
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
        ProbedFs::Btrfs => {
            let backend =
                crate::fs_backend_lvm_btrfs::LvmBtrfsBackend::from_lv_parts(reader, vg_lv.clone())?;
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
        ProbedFs::Xfs => {
            // The default RHEL/Rocky/Alma/Oracle root layout is LVM + XFS.
            // lamxfs's XfsBackend is BlockSource-generic, so it mounts directly
            // over the LV reader — no LVM-specific wrapper (unlike ext4/btrfs,
            // whose pre-generic backends predate the BlockSource seam, and which
            // also need an LVM-derived UUID because lambutter exposes none).
            // lamxfs reads its own superblock UUID/label; LV provenance is
            // recorded in `vg_lv` below.
            let backend = crate::fs_backend_xfs::XfsBackend::from_source(reader)?;
            let fs_uuid = FsBackend::uuid(&backend);
            let label = FsBackend::label(&backend).map(String::from);
            Ok(LvmDispatch {
                backend: Box::new(backend),
                backend_tag: crate::fs_backend_xfs::XFS_BACKEND_TAG,
                fs_uuid,
                label,
                vg_lv,
            })
        }
        ProbedFs::Fat => {
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
        ProbedFs::Erofs
        | ProbedFs::Iso9660
        | ProbedFs::Cramfs
        | ProbedFs::Romfs
        | ProbedFs::Squashfs
        | ProbedFs::Udf => {
            // The lamfold media frontends mount over any BlockSource via
            // `dispatch_fs_over_source`, not the LVM-tagged path (which carries
            // the ext4/btrfs/FAT-on-LV backends with their LVM identity).
            Err(FsError::Unsupported(
                "a lamfold media filesystem on an LV is mounted via dispatch_fs_over_source",
            ))
        }
        ProbedFs::Unknown => Err(FsError::Unsupported(
            "LV superblock did not match any supported filesystem (ext2/3/4, btrfs, xfs, or FAT)",
        )),
    }
}

// ---------------------------------------------------------------------------
// 4. Seam C — source-generic mount
// ---------------------------------------------------------------------------

/// **Seam C.** Probe any [`BlockSource`] and mount the matching read backend
/// over it — the source-generic sibling of [`open_lvm_lv_backend`]. Where that
/// function is hard-wired to a logical volume and the LVM-tagged backends, this
/// one mounts the plain partition backends (`Ext4Backend`/`BtrfsBackend`) over
/// *any* source: a whole partition, a logical volume, or an `.iso` loopback
/// region. It is the entry point boot-from-ISO uses to mount a downloaded image
/// (`SPEC-BOOT-FROM-ISO` §2), and it composes recursively — the source may
/// itself be a `SourceReader` over another source (an LV over a loopback ISO).
///
/// `btrfs_fs_uuid` is the only identity the dispatcher cannot derive from the
/// mounted filesystem: lambutter v0.3.x exposes no UUID accessor, so the caller
/// pre-peeks it (the partition prober does; an ISO-loopback caller passes
/// `None`). ext4 derives its own UUID and ignores it. Rewinds the source after
/// probing, so the returned backend reads from byte 0.
///
/// Exercised at compile time by `block_source::assert_file_source_dispatches`
/// (the boot-from-ISO composition proof) until its first live caller — the
/// boot-from-ISO glue (M2/M3) — lands.
pub(crate) fn dispatch_fs_over_source<S: BlockSource + 'static>(
    mut source: S,
    btrfs_fs_uuid: Option<Uuid>,
) -> Result<Box<dyn FsBackend>, FsError> {
    match probe_source_superblock(&mut source)? {
        ProbedFs::Ext4 => Ok(Box::new(crate::fs_backend_ext4::Ext4Backend::from_source(
            source,
        )?)),
        ProbedFs::Btrfs => Ok(Box::new(
            crate::fs_backend_btrfs::BtrfsBackend::from_source(source, btrfs_fs_uuid)?,
        )),
        // lamxfs reads its own superblock UUID/label, so (unlike btrfs) it needs
        // no pre-peeked identity from the caller.
        ProbedFs::Xfs => Ok(Box::new(crate::fs_backend_xfs::XfsBackend::from_source(
            source,
        )?)),
        // lamfold Family-A media frontends. Each mounts over `SourceReader<S>`
        // (the BlockSource bridge) with the unverified shepherd; a trust root,
        // when present, would inject a MerkleVerifier here instead (§5 trust
        // composition, a later pass).
        ProbedFs::Erofs => {
            mount_lamfold::<S, lamfold_erofs::Erofs<_>>(source, "lamfold-erofs@0.0.1")
        }
        ProbedFs::Iso9660 => {
            mount_lamfold::<S, lamfold_iso::Iso9660<_>>(source, "lamfold-iso@0.0.1")
        }
        ProbedFs::Cramfs => {
            mount_lamfold::<S, lamfold_cramfs::Cramfs<_>>(source, "lamfold-cramfs@0.0.1")
        }
        ProbedFs::Romfs => {
            mount_lamfold::<S, lamfold_romfs::Romfs<_>>(source, "lamfold-romfs@0.0.1")
        }
        ProbedFs::Squashfs => {
            mount_lamfold::<S, lamfold_squash::SquashFs<_>>(source, "lamfold-squash@0.0.1")
        }
        ProbedFs::Udf => mount_lamfold::<S, lamfold_udf::Udf<_>>(source, "lamfold-udf@0.0.1"),
        // FAT over an arbitrary BlockSource needs a fatfs adapter over
        // `SourceReader` (the El Torito embedded-ESP path); that adapter arrives
        // with lamfat. The LVM and whole-partition FAT paths are unaffected.
        ProbedFs::Fat => Err(FsError::Unsupported(
            "FAT over a generic BlockSource is not yet wired (arrives with lamfat)",
        )),
        ProbedFs::Unknown => Err(FsError::Unsupported(
            "source superblock did not match any supported filesystem",
        )),
    }
}

/// Mount **only** a lamfold media filesystem (erofs / iso / squashfs / cramfs /
/// romfs / udf) over `source`. ext4 / btrfs / FAT return `Unsupported` — they
/// have dedicated mount paths earlier in the boot sequence, so this never
/// produces a duplicate `Volume`. Used by the standalone media-partition scan
/// (boot sequence step 4); boot-from-ISO recursion uses the full
/// [`dispatch_fs_over_source`] (which also mounts ext4/btrfs inside an image).
pub(crate) fn dispatch_media_fs_over_source<S: BlockSource + 'static>(
    mut source: S,
) -> Result<Box<dyn FsBackend>, FsError> {
    match probe_source_superblock(&mut source)? {
        ProbedFs::Erofs => {
            mount_lamfold::<S, lamfold_erofs::Erofs<_>>(source, "lamfold-erofs@0.0.1")
        }
        ProbedFs::Iso9660 => {
            mount_lamfold::<S, lamfold_iso::Iso9660<_>>(source, "lamfold-iso@0.0.1")
        }
        ProbedFs::Squashfs => {
            mount_lamfold::<S, lamfold_squash::SquashFs<_>>(source, "lamfold-squash@0.0.1")
        }
        ProbedFs::Cramfs => {
            mount_lamfold::<S, lamfold_cramfs::Cramfs<_>>(source, "lamfold-cramfs@0.0.1")
        }
        ProbedFs::Romfs => {
            mount_lamfold::<S, lamfold_romfs::Romfs<_>>(source, "lamfold-romfs@0.0.1")
        }
        ProbedFs::Udf => mount_lamfold::<S, lamfold_udf::Udf<_>>(source, "lamfold-udf@0.0.1"),
        ProbedFs::Ext4 | ProbedFs::Btrfs | ProbedFs::Xfs | ProbedFs::Fat => {
            Err(FsError::Unsupported(
                "block filesystem (ext4/btrfs/xfs/FAT) has a dedicated boot-sequence mount, not media",
            ))
        }
        ProbedFs::Unknown => Err(FsError::Unsupported("no recognized media filesystem")),
    }
}

/// Largest decompressed-block working set a lamfold backend caches (LRU,
/// evicting; read-only ⇒ no invalidation). Comfortable for a kernel + initrd
/// read off compressed media without an unbounded footprint.
const LAMFOLD_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// Mount a lamfold Family-A frontend `F` over `source` — wrapped in a
/// `SourceReader` to satisfy lamfold's `BlockSource` — with the unverified
/// shepherd, and box it as a read-only [`FsBackend`]. The generic adapter
/// `LamfoldBackend` keeps `F` out of the `dyn FsBackend` surface.
fn mount_lamfold<S, F>(source: S, tag: BackendTag) -> Result<Box<dyn FsBackend>, FsError>
where
    S: BlockSource + 'static,
    F: lamfold::FoldFrontend<crate::block_source::SourceReader<S>> + 'static,
{
    let reader = crate::block_source::SourceReader::new(source);
    let mut cache = lamfold::BlockCache::new(LAMFOLD_CACHE_BYTES);
    let nov = lamfold::NoVerifier;
    let fs = {
        let mut cx = lamfold::SubstrateCtx {
            cache: &mut cache,
            verifier: &nov,
        };
        F::open(reader, &mut cx).map_err(crate::fs_backend_lamfold::map_fold_err)?
    };
    Ok(Box::new(crate::fs_backend_lamfold::LamfoldBackend::new(
        fs,
        cache,
        Box::new(lamfold::NoVerifier),
        tag,
    )))
}
