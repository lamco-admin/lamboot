//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only XFS backend.
//!
//! Implements `FsBackend` over the [`lamxfs`] crate — a Lamco-authored
//! clean-room `no_std` read-only XFS reader. Mirror of `fs_backend_btrfs.rs`:
//! same architectural position, same protocol choice (BlockIO not DiskIo),
//! same error-translation discipline.
//!
//! # Why this exists
//!
//! XFS is the default `/boot` + `/` filesystem on the entire RHEL family
//! (RHEL, Rocky, AlmaLinux, Oracle Linux, Amazon Linux) — the single largest
//! de-bundling gap left by the ext4/btrfs/FAT backends. The third-party EfiFs
//! `xfs_x64.efi` driver is GRUB-derived (GPLv3, can't be Microsoft-signed under
//! Path D) and loading it via `BS->LoadImage` triggers shim 15.8's `ShimLock`
//! uninstall — the same failure mode SDS-2 was written to avoid. A native Rust
//! reader embedded in the LamBoot binary sidesteps both problems.
//!
//! # Architectural position
//!
//! * `partitions.rs::probe_superblock` identifies XFS volumes by the `XFSB`
//!   magic at byte offset 0 and returns `FsInfo { fs_type: Xfs, .. }`.
//! * `fs.rs::Volume::from_backend` is the generic constructor for every
//!   non-FAT backend.
//!
//! Call site: `main.rs` discoverable-partition scan, immediately after the
//! btrfs dispatch loop. When the unified probe returns `FsType::Xfs`,
//! `XfsBackend::new` is called and the resulting `Volume` is pushed onto
//! `extra_volumes`. The source-generic [`crate::fs_backend_lvm_dispatch`] Seam-C
//! path also mounts XFS over an LV or `.iso` loopback region.
//!
//! # Simpler than the btrfs backend
//!
//! lamxfs exposes `uuid()` and `label()` accessors directly (lambutter v0.3.x
//! does not), so this backend derives its own identity straight from the
//! mounted superblock rather than reusing the partition prober's pre-peeked
//! UUID. The over-source constructor therefore needs no UUID parameter.
//!
//! # Security
//!
//! No `unsafe` blocks in this module. lamxfs itself `#![forbid(unsafe_code)]`
//! at the crate level; every on-disk count/offset/shift is range-checked and a
//! `MAX_FILE_BYTES` cap refuses a hostile inode size before allocation, so a
//! crafted volume cannot drive a denial-of-boot.

use alloc::{string::String, vec::Vec};

use lamxfs::{Error as XfsErr, SuperblockReason, Xfs};
use uefi::Handle;

use crate::{
    block_source::{BlockIoSource, BlockSource, SourceReader},
    fs_backend::{
        BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path, PathBuf, Uuid,
    },
};

/// Max symlink-resolution depth before we conclude the chain is a loop. Matches
/// `BtrfsBackend::MAX_SYMLINK_DEPTH` (Linux's `SYMLOOP_MAX`, 40) so a legitimate
/// distro `/boot` layout never trips the limit while a cycle still terminates.
const MAX_SYMLINK_DEPTH: u8 = 40;

/// Backend tag surfaced via `FsBackend::tag()`. Update lockstep with the Cargo
/// dep version — exact-pinned `=0.1.0`, same discipline as `BTRFS_BACKEND_TAG`.
pub(crate) const XFS_BACKEND_TAG: BackendTag = "lamxfs@0.1.0";

/// A mounted XFS volume. Owns the lamxfs reader plus the superblock UUID read
/// at open time.
///
/// Generic over the [`BlockSource`] beneath it: `BlockIoSource` for a whole
/// partition, or (via the Seam-C dispatcher) a logical volume or an `.iso`
/// loopback region.
pub(crate) struct XfsBackend<S: BlockSource> {
    fs: Xfs<SourceReader<S>>,
    fs_uuid: Option<Uuid>,
}

impl XfsBackend<BlockIoSource> {
    /// Construct an XFS backend over a whole partition identified as XFS by the
    /// unified `probe_superblock`. Ownership of the BlockIO protocol transfers
    /// into the backend for its lifetime — tree walks are read-frequent and a
    /// per-call open would be wasteful.
    pub(crate) fn new(handle: Handle) -> Result<Self, FsError> {
        Self::from_source(BlockIoSource::open(handle)?)
    }
}

impl<S: BlockSource + 'static> XfsBackend<S> {
    /// Construct an XFS backend over any [`BlockSource`]. The source is wrapped
    /// in the shared [`SourceReader`]; its byte length (which lamxfs accepts for
    /// device-bounds parity) is read straight off the source. The filesystem
    /// UUID is read from the mounted superblock — no caller-supplied identity.
    pub(crate) fn from_source(source: S) -> Result<Self, FsError> {
        let reader = SourceReader::new(source);
        let device_size_bytes = reader.byte_len();
        let fs = Xfs::open(reader, device_size_bytes).map_err(translate_xfs_error)?;
        // `sb_uuid` is always set by mkfs.xfs; an all-zero field means the bytes
        // were not really a superblock — omit rather than report a bogus UUID.
        let raw = fs.uuid();
        let fs_uuid = (raw != [0u8; 16]).then(|| Uuid::from_bytes(raw));
        Ok(Self { fs, fs_uuid })
    }

    /// Resolve `path` to an inode, transparently following a symlink at the
    /// final component up to `MAX_SYMLINK_DEPTH` hops.
    ///
    /// lamxfs (like lambutter) does NOT follow symlinks during resolution —
    /// `resolve` returns the symlink's own inode and `read_file` errors with
    /// `NotARegularFile`. The FsBackend contract is POSIX-like (transparent
    /// following), so this adapter bridges the two. Only leaf-component symlinks
    /// are followed; the distro `/boot` layouts we target put symlinks only at
    /// the kernel-name leaf, so leaf-only is sufficient (same scope as btrfs).
    fn resolve_following(&mut self, path: &Path) -> Result<lamxfs::Inode, FsError> {
        let mut current: PathBuf = Path::try_from_str(path.as_str())?;
        for _ in 0..=MAX_SYMLINK_DEPTH {
            let xp = xfs_path(current.as_path());
            let inode = self.fs.resolve(xp).map_err(translate_xfs_error)?;
            let md = self.fs.metadata(&inode).map_err(translate_xfs_error)?;
            if !md.is_symlink() {
                return Ok(inode);
            }
            // read_link returns owned bytes, ending the `xp` borrow of `current`
            // before the reassignment below.
            let target_bytes = self.fs.read_link(xp).map_err(translate_xfs_error)?;
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

impl<S: BlockSource + 'static> FsBackend for XfsBackend<S> {
    fn tag(&self) -> BackendTag {
        XFS_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        // lamxfs exposes the `sb_fname` label directly (unlike lambutter).
        self.fs.label()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_xfs_error)?;
        // Clamp the up-front allocation to the read-contract cap: md.size() is
        // read straight from the on-disk inode and is attacker-controlled, so a
        // crafted multi-GiB size must not drive an OOM-abort allocation. (lamxfs
        // enforces its own MAX_FILE_BYTES too; this is the backend-side gate.)
        let size = crate::fs_backend::checked_full_read_len(md.size())?;
        let mut buf = alloc::vec![0u8; size];
        let n = self
            .fs
            .read_file_at(&inode, 0, &mut buf)
            .map_err(translate_xfs_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let inode = self.resolve_following(path)?;
        let mut buf = alloc::vec![0u8; len];
        let n = self
            .fs
            .read_file_at(&inode, offset, &mut buf)
            .map_err(translate_xfs_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        // Does NOT follow a leaf symlink — `exists` is "is there an entry here"
        // (`lstat` semantics), consistent with the btrfs backend.
        match self.fs.resolve(xfs_path(path)) {
            Ok(_) => Ok(true),
            Err(XfsErr::NotFound { .. }) => Ok(false),
            Err(other) => Err(translate_xfs_error(other)),
        }
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        // Follows symlinks (POSIX `stat`, not `lstat`).
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_xfs_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let inode = self.resolve_following(path)?;
        let dir_md = self.fs.metadata(&inode).map_err(translate_xfs_error)?;
        if !dir_md.is_dir() {
            return Err(FsError::NotDirectory);
        }
        // lamxfs read_dir() takes a path and re-resolves it WITHOUT following a
        // leaf symlink. For the common case (the directory itself is not a
        // symlink) this resolves to the same inode `resolve_following` returned.
        // A symlink-to-directory at the leaf would error here — acceptable for
        // v0.1, identical to the btrfs backend's documented limitation.
        let entries = self
            .fs
            .read_dir(xfs_path(path))
            .map_err(translate_xfs_error)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = core::str::from_utf8(&entry.name)
                .map_err(|_| FsError::InvalidPath("non-utf8 xfs filename"))?;
            if name == "." || name == ".." {
                continue;
            }
            // Per-entry metadata for DirEntry.size + canonical kind. This is
            // ADVISORY: it reflects the name→inode binding the directory claims,
            // not one re-resolved by name. A metadata failure on one entry must
            // not abort the whole listing (an attacker could otherwise craft one
            // bad entry to hide its siblings) — fall back to the dirent's own
            // kind (reliable when the `ftype` feature is on) with unknown size.
            let child = lamxfs::Inode {
                ino: entry.inode_number,
            };
            let (kind, size) = match self.fs.metadata(&child) {
                Ok(md) => (translate_metadata(&md).kind, md.size()),
                Err(_) => (kind_from_entry(entry.kind), 0),
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
// Helpers
// ---------------------------------------------------------------------------

/// Translate a LamBoot `&Path` to a `lamxfs::Path`. Both use UTF-8 +
/// forward-slash conventions; lamxfs's resolver ignores empty and `.`
/// components, so construction is infallible (no absolute-ness check).
pub(crate) fn xfs_path(path: &Path) -> lamxfs::Path<'_> {
    lamxfs::Path::new(path.as_str().as_bytes())
}

/// Translate `lamxfs::Error` into `FsError`. Stable `&'static str` tokens are
/// preserved for trust-log emission.
#[expect(
    clippy::needless_pass_by_value,
    reason = "called as a function pointer via .map_err; XfsErr is a small enum \
              and the conversion semantically consumes it at the FsBackend boundary"
)]
pub(crate) fn translate_xfs_error(err: XfsErr) -> FsError {
    match err {
        XfsErr::NotFound { .. } => FsError::NotFound,
        XfsErr::NotARegularFile => FsError::IsDirectory,
        XfsErr::NotASymlink => FsError::Unsupported("not_a_symlink"),
        XfsErr::FileTooLarge { size, max } => FsError::FileTooLarge { size, max },
        XfsErr::UnsupportedFeature(feature) => FsError::UnsupportedFeature {
            feature,
            backend: XFS_BACKEND_TAG,
        },
        XfsErr::BadSuperblock(reason) => match reason {
            SuperblockReason::BadMagic => FsError::Corrupt("xfs bad superblock magic"),
            SuperblockReason::BadCrc => FsError::Corrupt("xfs superblock crc mismatch"),
            SuperblockReason::BadGeometry => FsError::Corrupt("xfs bad superblock geometry"),
            SuperblockReason::UnsupportedVersion => FsError::UnsupportedFeature {
                feature: "xfs_sb_version",
                backend: XFS_BACKEND_TAG,
            },
            SuperblockReason::ExternalLog => FsError::UnsupportedFeature {
                feature: "xfs_external_log",
                backend: XFS_BACKEND_TAG,
            },
            SuperblockReason::RealtimeDevice => FsError::UnsupportedFeature {
                feature: "xfs_realtime_device",
                backend: XFS_BACKEND_TAG,
            },
            // NEEDSREPAIR incompat bit: the volume is flagged for xfs_repair and
            // must not be trusted by a read-only reader.
            SuperblockReason::DirtyLog => FsError::Corrupt("xfs needs repair (dirty log)"),
        },
        // A v5 self-describing-metadata CRC mismatch is on-disk corruption.
        XfsErr::BadCrc { what, .. } => FsError::Corrupt(what),
        // A structural violation found mid-walk (bad magic, key order, count, …)
        // and a block-read failure surfaced through lamxfs both present, from
        // lamboot's view, as unreadable on-disk data — each carries a stable
        // `token` preserved for the trust log.
        XfsErr::Inconsistent { token, .. } | XfsErr::Io { token, .. } => FsError::Corrupt(token),
        XfsErr::OutOfMemory { .. } => FsError::Unsupported("out_of_memory"),
    }
}

pub(crate) fn translate_metadata(md: &lamxfs::Metadata) -> Metadata {
    let kind = if md.is_file() {
        FileKind::Regular
    } else if md.is_dir() {
        FileKind::Directory
    } else if md.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    Metadata {
        kind,
        size: md.size(),
        mode: md.mode(),
        // Owner-write bit clear ⇒ read-only by owner. Matches the ext4/btrfs
        // interpretation; the backend itself is read-only regardless.
        read_only: (md.mode() & 0o200) == 0,
        source_backend: XFS_BACKEND_TAG,
    }
}

/// Map a lamxfs directory-entry kind to a `FileKind` (the fallback when a
/// per-entry metadata read fails during `read_dir`).
fn kind_from_entry(kind: lamxfs::EntryKind) -> FileKind {
    match kind {
        lamxfs::EntryKind::Regular => FileKind::Regular,
        lamxfs::EntryKind::Directory => FileKind::Directory,
        lamxfs::EntryKind::Symlink => FileKind::Symlink,
        lamxfs::EntryKind::Other => FileKind::Other,
    }
}
