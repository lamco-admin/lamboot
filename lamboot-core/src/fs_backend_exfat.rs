//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only exFAT backend.
//!
//! Implements `FsBackend` over the [`lamexfat`] crate — a Lamco-authored
//! `no_std` read-only exFAT reader. Same architectural position as
//! `fs_backend_xfs.rs`, but simpler: exFAT has no symlinks, so there is no
//! leaf-following resolver — every operation takes the path directly.
//!
//! # Why this exists
//!
//! exFAT is never the UEFI ESP (the ESP is FAT32 by spec) and never a Linux
//! root, so it does not overlap the ext4/btrfs/xfs backends. It serves
//! **removable/utility media**: a kernel + initrd or a boot image dropped onto
//! an exFAT-formatted USB stick or SD card — including the `> 4 GiB` payloads
//! FAT32 cannot hold. A native Rust reader embedded in the LamBoot binary reads
//! that media without a GPLv3 EfiFs driver (which `BS->LoadImage` cannot load
//! under shim 15.8 without uninstalling `ShimLock`).
//!
//! # Architectural position
//!
//! * `partitions.rs::probe_superblock` identifies exFAT volumes by the
//!   `"EXFAT   "` magic at byte offset 3 of sector 0 and returns
//!   `FsInfo { fs_type: ExFat, .. }`.
//! * `main.rs`'s discoverable-partition scan mounts the volume via
//!   `ExFatBackend::new` and pushes the resulting `Volume` onto `extra_volumes`,
//!   immediately after the XFS loop.
//!
//! The backend is `BlockSource`-generic so the same code mounts an exFAT volume
//! over any source, but the Seam-C LV/`.iso` dispatcher is intentionally NOT
//! wired for exFAT: logical volumes and `.iso` loopback regions are never exFAT
//! (exFAT is removable media, not an LVM member or an optical image).
//!
//! # Identity
//!
//! exFAT carries a 32-bit VolumeSerialNumber, not a 128-bit UUID, so
//! [`FsBackend::uuid`] reports `None`; the volume is identified by its label and
//! GPT partition GUID. The serial is surfaced for the trust log by the prober.
//!
//! # Security
//!
//! No `unsafe` blocks in this module. lamexfat itself is `#![forbid(unsafe_code)]`
//! at the crate level; every on-disk count/offset/cluster-chain is bounds- and
//! cycle-checked, and a `MAX_FILE_BYTES` cap refuses a hostile `DataLength`
//! before allocation, so a crafted volume cannot drive a denial-of-boot.

use alloc::{format, vec::Vec};

use lamexfat::{Error as ExfatErr, ExFat, VbrReason};
use uefi::Handle;

use crate::{
    block_source::{BlockIoSource, BlockSource, SourceReader},
    fs_backend::{
        checked_full_read_len, BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path,
        Uuid,
    },
};

/// Backend tag surfaced via `FsBackend::tag()`. Update lockstep with the Cargo
/// dep version — exact-pinned `=0.1.0`, same discipline as `XFS_BACKEND_TAG`.
pub(crate) const EXFAT_BACKEND_TAG: BackendTag = "lamexfat@0.1.0";

/// A mounted exFAT volume. Generic over the [`BlockSource`] beneath it:
/// `BlockIoSource` for a whole partition.
pub(crate) struct ExFatBackend<S: BlockSource> {
    fs: ExFat<SourceReader<S>>,
}

impl ExFatBackend<BlockIoSource> {
    /// Construct an exFAT backend over a whole partition identified as exFAT by
    /// the unified `probe_superblock`. Ownership of the BlockIO protocol
    /// transfers into the backend for its lifetime.
    pub(crate) fn new(handle: Handle) -> Result<Self, FsError> {
        Self::from_source(BlockIoSource::open(handle)?)
    }
}

impl<S: BlockSource + 'static> ExFatBackend<S> {
    /// Construct an exFAT backend over any [`BlockSource`]. The source is wrapped
    /// in the shared [`SourceReader`]; its byte length (which lamexfat accepts
    /// for device-bounds parity) is read straight off the source.
    pub(crate) fn from_source(source: S) -> Result<Self, FsError> {
        let reader = SourceReader::new(source);
        let device_size_bytes = reader.byte_len();
        let fs = ExFat::open(reader, device_size_bytes).map_err(translate_exfat_error)?;
        Ok(Self { fs })
    }
}

impl<S: BlockSource + 'static> FsBackend for ExFatBackend<S> {
    fn tag(&self) -> BackendTag {
        EXFAT_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        // exFAT has only a 32-bit VolumeSerialNumber, not a 128-bit UUID.
        None
    }

    fn label(&self) -> Option<&str> {
        self.fs.label()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        // Clamp the up-front allocation to the read-contract cap: the on-disk
        // DataLength is attacker-controlled, so a crafted multi-GiB size must not
        // drive an OOM-abort allocation. (lamexfat enforces its own
        // MAX_FILE_BYTES too; this is the backend-side gate, as in the XFS
        // backend.) read_at then streams into the sized buffer.
        let md = self
            .fs
            .metadata(exfat_path(path))
            .map_err(translate_exfat_error)?;
        let size = checked_full_read_len(md.size())?;
        let mut buf = alloc::vec![0u8; size];
        let n = self
            .fs
            .read_at(exfat_path(path), 0, &mut buf)
            .map_err(translate_exfat_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let mut buf = alloc::vec![0u8; len];
        let n = self
            .fs
            .read_at(exfat_path(path), offset, &mut buf)
            .map_err(translate_exfat_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        self.fs
            .exists(exfat_path(path))
            .map_err(translate_exfat_error)
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let md = self
            .fs
            .metadata(exfat_path(path))
            .map_err(translate_exfat_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let entries = self
            .fs
            .read_dir(exfat_path(path))
            .map_err(translate_exfat_error)?;
        // `/dir` -> child paths are `/dir/<name>`; the root is `/` -> `/<name>`.
        let parent = path.as_str().trim_end_matches('/');
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            // The dirent's own kind is authoritative for exFAT (the
            // ATTR_DIRECTORY bit). Size is advisory and needs a per-entry
            // metadata read; a failure on one entry must not abort the listing
            // (an attacker could otherwise craft one bad entry to hide its
            // siblings) — fall back to the dirent kind with unknown size.
            let child = format!("{parent}/{}", entry.name);
            let (kind, size) = match self.fs.metadata(lamexfat::Path::new(child.as_bytes())) {
                Ok(md) => (translate_metadata(&md).kind, md.size()),
                Err(_) => (kind_from_entry(entry.kind), 0),
            };
            out.push(DirEntry {
                name: entry.name,
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

/// Translate a LamBoot `&Path` to a `lamexfat::Path`. Both use UTF-8 +
/// forward-slash conventions; lamexfat's resolver ignores empty and `.`
/// components, so construction is infallible.
pub(crate) fn exfat_path(path: &Path) -> lamexfat::Path<'_> {
    lamexfat::Path::new(path.as_str().as_bytes())
}

/// Translate `lamexfat::Error` into `FsError`. Stable `&'static str` tokens are
/// preserved for trust-log emission.
#[expect(
    clippy::needless_pass_by_value,
    reason = "called as a function pointer via .map_err; ExfatErr is a small enum \
              and the conversion semantically consumes it at the FsBackend boundary"
)]
pub(crate) fn translate_exfat_error(err: ExfatErr) -> FsError {
    match err {
        ExfatErr::NotFound { .. } => FsError::NotFound,
        ExfatErr::NotARegularFile => FsError::IsDirectory,
        ExfatErr::FileTooLarge { size, max } => FsError::FileTooLarge { size, max },
        ExfatErr::UnsupportedFeature(feature) => FsError::UnsupportedFeature {
            feature,
            backend: EXFAT_BACKEND_TAG,
        },
        ExfatErr::BadVbr(reason) => match reason {
            VbrReason::BadMagic => FsError::Corrupt("exfat bad boot-sector magic"),
            VbrReason::BadBootSignature => FsError::Corrupt("exfat bad boot signature"),
            VbrReason::BadGeometry => FsError::Corrupt("exfat bad boot-sector geometry"),
            VbrReason::BadBootChecksum => FsError::Corrupt("exfat boot-region checksum mismatch"),
        },
        ExfatErr::BadUpcaseTable => FsError::Corrupt("exfat bad up-case table"),
        // A bad entry-set checksum/structure and a structural inconsistency
        // mid-walk both present, from lamboot's view, as unreadable on-disk
        // data; each carries a stable `token` preserved for the trust log. A
        // block-read failure surfaced through lamexfat is the same.
        ExfatErr::BadEntrySet { token }
        | ExfatErr::Inconsistent { token }
        | ExfatErr::Io { token, .. } => FsError::Corrupt(token),
        ExfatErr::OutOfMemory { .. } => FsError::Unsupported("out_of_memory"),
    }
}

pub(crate) fn translate_metadata(md: &lamexfat::Metadata) -> Metadata {
    let kind = if md.is_dir() {
        FileKind::Directory
    } else if md.is_file() {
        FileKind::Regular
    } else {
        FileKind::Other
    };
    // exFAT has no POSIX mode; synthesize the conventional read-only-media
    // values (matching the FAT-RO backend: dirs 0o555, files 0o444). The backend
    // is read-only regardless of the on-disk attribute.
    let mode = match kind {
        FileKind::Directory => 0o555,
        _ => 0o444,
    };
    Metadata {
        kind,
        size: md.size(),
        mode,
        read_only: true,
        source_backend: EXFAT_BACKEND_TAG,
    }
}

/// Map a lamexfat directory-entry kind to a `FileKind` (the fallback when a
/// per-entry metadata read fails during `read_dir`).
fn kind_from_entry(kind: lamexfat::EntryKind) -> FileKind {
    match kind {
        lamexfat::EntryKind::Regular => FileKind::Regular,
        lamexfat::EntryKind::Directory => FileKind::Directory,
        lamexfat::EntryKind::Other => FileKind::Other,
    }
}
