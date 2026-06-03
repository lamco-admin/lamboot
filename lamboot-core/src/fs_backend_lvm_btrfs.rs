//! Layer: 2 — Storage & Filesystems.
//!
//! LVM2 read backend — bridges lamlvm's `OwnedLvReader` into LamBoot's
//! `FsBackend` trait via lambutter (btrfs).
//!
//! Architecture:
//!
//! ```text
//!  UEFI BlockIO
//!    │  (block-granular sector reads)
//!    ▼
//!  BlockIoPvReader  ── embedded_io::Read + Seek over the partition's bytes
//!    │  (shared with fs_backend_lvm::LvmExt4Backend — same PV adapter,
//!    │   different filesystem at the bottom of the chain)
//!    ▼
//!  lamlvm::Lvm2::open  ── parses PV label + VG metadata
//!    │
//!    ▼
//!  lamlvm::OwnedLvReader  ── embedded_io::Read + Seek over LV bytes
//!    │
//!    ▼
//!  BtrfsBlockAdapter  ── lambutter::BlockRead (random-access) over the LV
//!    │
//!    ▼
//!  lambutter::Btrfs  ── filesystem reads (RO)
//! ```
//!
//! This is the LVM-on-btrfs companion to `fs_backend_lvm::LvmExt4Backend`.
//! Identical PV layer; differs only in the LV → filesystem adapter and
//! the filesystem object at the top.
//!
//! ## Why this exists (the v0.11.13 LVM-lambutter oversight)
//!
//! v0.11.0 landed `LvmExt4Backend` (LVM → ext4-view) but hardcoded the
//! chain to ext4. v0.10.0 landed `BtrfsBackend` (native lambutter on
//! plain partitions). The two were never composed: LVM-on-btrfs roots
//! (real-world layouts on openSUSE with LVM, Fedora-Server-with-LVM,
//! and some Arch installs) could NOT boot via LamBoot even though both
//! halves of the stack — lamlvm and lambutter — were individually
//! known-good.
//!
//! This module closes that gap.
//!
//! ## Scope (matches LvmExt4Backend)
//!
//! Linear LV on single PV — the only layout covered by `lamlvm` today.
//! Multi-PV / striped / mirrored / thin LVs error cleanly at LV-open
//! time with `OpenLvError::Unsupported`. Same v1.x roadmap item gates
//! both LVM backends in lockstep.
//!
//! ## Adapter layer
//!
//! `BtrfsBlockAdapter` differs from `LvExt4Adapter` (in fs_backend_lvm)
//! only in which upper-layer trait it implements. The adapter shape is:
//!
//! * ext4 case: `Ext4Read::read(start, dst)` — random-access I/O hint
//!   from ext4-view; we seek-then-read-exact over the streaming LV.
//! * btrfs case: `lambutter::BlockRead::read_at(offset, buf)` —
//!   identical shape, identical seek-then-read-exact logic, just a
//!   different trait name and error type.
//!
//! No `unsafe` blocks. Each byte read goes through validated tree-walk
//! + metadata-CRC32C verification inside lambutter before being
//! returned (same security guarantee as `BtrfsBackend`).

use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::error::Error;

use embedded_io::{Read as EioRead, Seek as EioSeek, SeekFrom as EioSeekFrom};
use lambutter::{BlockRead, Btrfs};

use crate::{
    fs_backend::{BackendTag, DirEntry, FsBackend, FsError, Metadata, Path, Uuid},
    fs_backend_btrfs::{lb_path, parse_btrfs_uuid_string, translate_lb_error, translate_metadata},
    fs_backend_lvm::BlockIoPvReader,
    partitions::FsInfo,
};

/// Backend tag — surfaces in trust-log events. Format
/// `lvm+lambutter@<lambutter-version>` lets audit consumers see which
/// LV's btrfs bytes sourced any kernel/initrd that got loaded.
///
/// Lockstep with `BtrfsBackend`'s `BTRFS_BACKEND_TAG` version suffix so
/// a single dep bump on lambutter updates both tags.
pub(crate) const LVM_BTRFS_BACKEND_TAG: BackendTag = "lvm+lambutter@0.3.0-path";

/// A mounted btrfs-on-LV volume.
pub(crate) struct LvmBtrfsBackend {
    fs: Btrfs<BtrfsBlockAdapter>,
    fs_uuid: Option<Uuid>,
    /// Cached `vg/lv` identifier, surfaced in diagnostics.
    vg_lv: String,
}

impl LvmBtrfsBackend {
    /// Construct a btrfs-on-LV backend from a pre-opened `OwnedLvReader`.
    ///
    /// Intended to be called by `fs_backend_lvm_dispatch::open_lvm_lv_backend`
    /// AFTER it has opened the LV and probed the superblock. The caller
    /// passes:
    ///   * `owned` — the opened LV reader, positioned at byte 0
    ///   * `lv_len` — LV size in bytes (needed by `lambutter::Btrfs::open`
    ///     for end-of-volume bounds checking)
    ///   * `vg_lv` — the `"VG/LV"` identifier string used in trust logs
    ///
    /// Mirror of `LvmExt4Backend::from_lv_parts`, differing only in the
    /// filesystem at the bottom of the chain (lambutter vs ext4-view).
    pub(crate) fn from_lv_parts(
        owned: lamlvm::OwnedLvReader<BlockIoPvReader>,
        lv_len: u64,
        vg_lv: String,
    ) -> Result<Self, FsError> {
        let reader = BtrfsBlockAdapter { lv: owned };
        let fs = Btrfs::open(reader, lv_len).map_err(translate_lb_error)?;

        // lambutter v0.3.x doesn't expose superblock UUID accessor.
        // For plain-partition btrfs, `BtrfsBackend` reuses the UUID
        // the partition prober extracted via byte peek. Here the LV
        // bytes were never seen by `partitions::probe_superblock` —
        // the LV only becomes addressable after Lvm2 parses the VG
        // metadata. Leave the FS UUID un-populated; identity at trust-
        // log time uses backend_tag + vg_lv string. v0.4.x lambutter
        // is tracked to expose UUID directly.
        let fs_uuid = None;

        log::info!("LVM+btrfs mounted: {vg_lv}");

        Ok(Self { fs, fs_uuid, vg_lv })
    }

    /// Construct with an externally-supplied FS UUID (from a future
    /// probe path that peeks the LV's btrfs superblock bytes). Not
    /// invoked today; the v0.11.14 probe doesn't extract UUIDs yet.
    #[expect(
        dead_code,
        reason = "reserved entry point for once probe_lv_superblock starts \
                  extracting filesystem UUIDs; symmetry with BtrfsBackend::new"
    )]
    pub(crate) fn from_lv_parts_with_info(
        owned: lamlvm::OwnedLvReader<BlockIoPvReader>,
        lv_len: u64,
        vg_lv: String,
        info: &FsInfo,
    ) -> Result<Self, FsError> {
        let mut backend = Self::from_lv_parts(owned, lv_len, vg_lv)?;
        backend.fs_uuid = info.uuid.as_deref().and_then(parse_btrfs_uuid_string);
        Ok(backend)
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

impl LvmBtrfsBackend {
    /// Resolve a path through leaf-component symlinks, mirroring
    /// `BtrfsBackend::resolve_following`. Returns the inode of the
    /// final (non-symlink) target.
    ///
    /// Resolution depth limit matches `BtrfsBackend` (40 — Linux's
    /// value, generous for any realistic /boot layout).
    fn resolve_following(&mut self, path: &Path) -> Result<lambutter::Inode, FsError> {
        const MAX_SYMLINK_DEPTH: u8 = 40;
        let mut current: crate::fs_backend::PathBuf = Path::try_from_str(path.as_str())?;
        for _ in 0..=MAX_SYMLINK_DEPTH {
            let lb = lb_path(&current)?;
            let inode = self.fs.resolve(lb).map_err(translate_lb_error)?;
            let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
            if !md.is_symlink() {
                return Ok(inode);
            }
            let target_bytes = self.fs.read_link(lb).map_err(translate_lb_error)?;
            let target_str = core::str::from_utf8(&target_bytes)
                .map_err(|_| FsError::InvalidPath("non-utf8 symlink target"))?;
            current = if target_str.starts_with('/') {
                Path::try_from_str(target_str)?
            } else {
                current.parent().join(target_str)?
            };
        }
        Err(FsError::Unsupported("symlink_chain_too_deep"))
    }
}

impl FsBackend for LvmBtrfsBackend {
    fn tag(&self) -> BackendTag {
        LVM_BTRFS_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        // Same as `BtrfsBackend`: lambutter v0.3.x doesn't expose label
        // through its public API. The probe layer extracts the label
        // for plain-partition btrfs; LV-bytes probing isn't implemented
        // yet. Identity at trust-log time uses backend_tag + vg_lv
        // string, which is more diagnostically useful than a btrfs
        // label for the LVM case anyway.
        None
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        // Clamp the up-front allocation to the read-contract cap (attacker-
        // controlled on-disk inode size — see fs_backend::checked_full_read_len).
        let size = crate::fs_backend::checked_full_read_len(md.size)?;
        let mut buf = alloc::vec![0u8; size];
        let n = self
            .fs
            .read_file_at(&inode, 0, &mut buf)
            .map_err(translate_lb_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let inode = self.resolve_following(path)?;
        let mut buf = alloc::vec![0u8; len];
        let n = self
            .fs
            .read_file_at(&inode, offset, &mut buf)
            .map_err(translate_lb_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let p = lb_path(path)?;
        match self.fs.resolve(p) {
            Ok(_) => Ok(true),
            Err(lambutter::Error::NotFound) => Ok(false),
            Err(other) => Err(translate_lb_error(other)),
        }
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let inode = self.resolve_following(path)?;
        let dir_md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        if !dir_md.is_dir() {
            return Err(FsError::NotDirectory);
        }
        // lambutter's read_dir() takes a path, not an inode. Use the
        // original path (post-symlink-follow); for the common
        // no-symlink case this is the same path the caller passed in.
        let p = lb_path(path)?;
        let entries = self.fs.read_dir(p).map_err(translate_lb_error)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = core::str::from_utf8(&entry.name)
                .map_err(|_| FsError::InvalidPath("non-utf8 btrfs filename"))?;
            if name == "." || name == ".." {
                continue;
            }
            // The objectid comes from the directory entry, so this metadata is
            // ADVISORY (the name→inode binding the directory claims, not one
            // re-resolved by name) — callers must re-resolve before any
            // security decision. A per-entry metadata failure must not abort
            // the whole listing (else a crafted bad entry could hide its
            // siblings): surface the entry with unknown kind/size instead.
            let inode = lambutter::Inode {
                objectid: entry.inode_number,
            };
            let (kind, size) = match self.fs.metadata(&inode) {
                Ok(md) => {
                    let kind = if md.is_file() {
                        crate::fs_backend::FileKind::Regular
                    } else if md.is_dir() {
                        crate::fs_backend::FileKind::Directory
                    } else if md.is_symlink() {
                        crate::fs_backend::FileKind::Symlink
                    } else {
                        crate::fs_backend::FileKind::Other
                    };
                    (kind, md.size)
                }
                Err(_) => (crate::fs_backend::FileKind::Other, 0),
            };
            out.push(DirEntry {
                name: String::from(name),
                kind,
                size,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// LV → lambutter::BlockRead adapter
// ---------------------------------------------------------------------------

/// Wraps an `OwnedLvReader` to satisfy lambutter's `BlockRead::read_at`
/// shape. Performs seek-then-read-exact, looping over short reads (which
/// the LV reader produces at segment boundaries).
///
/// Parallel to `fs_backend_lvm::LvExt4Adapter`. The only difference is
/// which trait the adapter implements — `BlockRead::read_at(offset, buf)`
/// vs `Ext4Read::read(start, dst)`. Both have the same signature shape
/// and the same seek-then-fill semantics.
struct BtrfsBlockAdapter {
    lv: lamlvm::OwnedLvReader<BlockIoPvReader>,
}

impl BlockRead for BtrfsBlockAdapter {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.is_empty() {
            return Ok(());
        }
        self.lv
            .seek(EioSeekFrom::Start(offset_bytes))
            .map_err(|e| {
                Box::new(LvBtrfsAdapterError(format!("seek: {e:?}")))
                    as Box<dyn Error + Send + Sync>
            })?;
        let mut filled = 0;
        while filled < buf.len() {
            let n = self.lv.read(&mut buf[filled..]).map_err(|e| {
                Box::new(LvBtrfsAdapterError(format!("read: {e:?}")))
                    as Box<dyn Error + Send + Sync>
            })?;
            if n == 0 {
                return Err(Box::new(LvBtrfsAdapterError(String::from(
                    "EOF before read_at completed",
                ))));
            }
            filled += n;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct LvBtrfsAdapterError(String);

impl core::fmt::Display for LvBtrfsAdapterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "LV→btrfs adapter error: {}", self.0)
    }
}

impl Error for LvBtrfsAdapterError {}
