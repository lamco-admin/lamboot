//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only ZFS backend.
//!
//! Implements `FsBackend` over the [`lamzfs`] crate — a Lamco-authored `no_std`
//! read-only ZFS pool reader. ZFS is structurally unlike the other backends in
//! two ways, both handled here:
//!
//! 1. **A pool spans multiple member devices.** Every other backend reads one
//!    partition; a ZFS mirror or raidz pool reads several. [`mount_pool`] takes a
//!    `Vec<PoolMember>` (one per member partition) — `main.rs` groups the
//!    ZFS-labeled partitions by pool GUID (via `lamzfs::peek_pool_id`) before
//!    calling it.
//! 2. **A pool contains many datasets, each its own filesystem.** A child
//!    dataset (`bpool/BOOT/<id>`) is a separate object set with its own ZPL root,
//!    so it must be presented as its own `Volume`. The pool is imported once,
//!    shared behind an `Rc<RefCell<…>>` (the same shared-ownership pattern as
//!    `block_source::FileBlockSource`), and one `ZfsBackend` is produced per
//!    dataset — each a thin view that resolves its in-dataset path against the
//!    shared pool.
//!
//! # Why this exists
//!
//! Ubuntu and Debian root-on-ZFS keep kernels and initrds in an unencrypted
//! `bpool` boot pool. Reading them needs neither the OpenZFS kernel module nor a
//! GPL UEFI filesystem driver (which `BS->LoadImage` cannot load under shim
//! without uninstalling `ShimLock`) — a native Rust reader embedded in LamBoot
//! reads the pool directly.
//!
//! # Identity
//!
//! A ZFS pool carries a 64-bit pool GUID, not a 128-bit filesystem UUID, so
//! [`FsBackend::uuid`] reports `None`; a dataset volume is identified by its
//! label (the dataset path) and `VolumeIdentity::index`.
//!
//! # Security
//!
//! No `unsafe` blocks. lamzfs is `#![forbid(unsafe_code)]` at the crate level;
//! every on-disk offset/size/indirection is bounds-, overflow-, and cycle-
//! checked, and `MAX_FILE_BYTES` refuses a hostile logical size before
//! allocation, so a crafted pool cannot drive a denial-of-boot. A block whose
//! checksum fails surfaces as [`FsError::IntegrityFailed`] — a loud security
//! status, never folded into a generic parse error.

use alloc::{
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;

use lamzfs::{EntryKind, Zfs};

use crate::{
    block_source::{BlockIoSource, SourceReader},
    fs_backend::{
        checked_full_read_len, BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path,
        Uuid,
    },
};

/// Backend tag surfaced via `FsBackend::tag()`. Update lockstep with the Cargo
/// dep version — exact-pinned `=0.1.0`, same discipline as `EXFAT_BACKEND_TAG`.
pub(crate) const ZFS_BACKEND_TAG: BackendTag = "lamzfs@0.1.0";

/// The reader under one ZFS member: a whole partition's `BlockIO`, bridged into
/// `lamzfs::BlockRead` by the shared [`SourceReader`] (see `block_source.rs`).
type ZfsReader = SourceReader<BlockIoSource>;

/// One imported pool, shared by every dataset view cut from it. `Rc<RefCell<…>>`
/// because the pool owns all members' exclusive `BlockIO` handles and is read by
/// several `ZfsBackend`s (one per dataset) over the boot — the same shared
/// single-threaded ownership `FileBlockSource` uses for a holding backend.
type SharedPool = Rc<RefCell<Zfs<ZfsReader>>>;

/// A mounted ZFS *dataset*, presented as a single-rooted filesystem. Holds a
/// shared handle to the imported pool and the dataset's path components within
/// it (`[]` = the pool root dataset).
pub(crate) struct ZfsBackend {
    pool: SharedPool,
    dataset: Vec<String>,
    label: Option<String>,
}

impl ZfsBackend {
    /// Import the pool from its member devices and produce one backend per
    /// dataset that opens as a ZPL filesystem (zvols and unopenable datasets are
    /// skipped). The members must all belong to one pool — `main.rs` groups them
    /// by `lamzfs::peek_pool_id` first.
    pub(crate) fn mount_pool(
        members: Vec<lamzfs::PoolMember<ZfsReader>>,
    ) -> Result<Vec<ZfsBackend>, FsError> {
        let mut zfs = Zfs::import(members).map_err(translate_zfs_error)?;
        let pool_name = zfs.pool_name().to_string();
        // A crafted/odd DSL tree must not abort the mount: fall back to just the
        // root dataset if enumeration fails.
        let datasets = zfs.datasets().unwrap_or_else(|_| alloc::vec![Vec::new()]);
        let pool = Rc::new(RefCell::new(zfs));

        let mut backends = Vec::new();
        for ds in datasets {
            let ds_ref: Vec<&str> = ds.iter().map(String::as_str).collect();
            // Only present datasets that actually open as a filesystem (a zvol or
            // container without a ZPL root errors here and is skipped).
            if pool.borrow_mut().read_dir(&ds_ref, &[]).is_err() {
                continue;
            }
            backends.push(ZfsBackend {
                label: Some(dataset_label(&pool_name, &ds)),
                dataset: ds,
                pool: Rc::clone(&pool),
            });
        }
        if backends.is_empty() {
            return Err(FsError::Unsupported("zfs_no_readable_datasets"));
        }
        Ok(backends)
    }

    /// The dataset path as `&[&str]`, for the lamzfs dataset-scoped calls.
    fn dataset_ref(&self) -> Vec<&str> {
        self.dataset.iter().map(String::as_str).collect()
    }
}

impl FsBackend for ZfsBackend {
    fn tag(&self) -> BackendTag {
        ZFS_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        // A ZFS pool has a 64-bit GUID, not a 128-bit filesystem UUID.
        None
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let comps = components(path);
        let ds = self.dataset_ref();
        // lamzfs caps the full-file allocation itself, but route through the
        // backend read-contract chokepoint too so the cap is uniform across
        // backends and a 32-bit build fails loudly rather than truncating.
        let size = self
            .pool
            .borrow_mut()
            .stat(&ds, &comps)
            .map_err(translate_zfs_error)?
            .size;
        let _ = checked_full_read_len(size)?;
        self.pool
            .borrow_mut()
            .read(&ds, &comps)
            .map_err(translate_zfs_error)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let comps = components(path);
        let ds = self.dataset_ref();
        self.pool
            .borrow_mut()
            .read_at(&ds, &comps, offset, len)
            .map_err(translate_zfs_error)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let comps = components(path);
        let ds = self.dataset_ref();
        self.pool
            .borrow_mut()
            .exists(&ds, &comps)
            .map_err(translate_zfs_error)
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let comps = components(path);
        let ds = self.dataset_ref();
        let stat = self
            .pool
            .borrow_mut()
            .stat(&ds, &comps)
            .map_err(translate_zfs_error)?;
        Ok(translate_stat(&stat))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let comps = components(path);
        let ds = self.dataset_ref();
        let entries = self
            .pool
            .borrow_mut()
            .read_dir(&ds, &comps)
            .map_err(translate_zfs_error)?;
        // The dirent's own kind is authoritative; size needs a per-entry stat
        // (lamzfs `read_dir` does not carry sizes). A failure on one entry must
        // not abort the listing (a crafted entry could otherwise hide its
        // siblings) — fall back to size 0. The child path is this directory's
        // components plus the entry name, built directly (no Path round-trip).
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let kind = kind_from_entry(entry.kind);
            let size = if kind == FileKind::Regular {
                let mut child = comps.clone();
                child.push(entry.name.as_str());
                self.pool
                    .borrow_mut()
                    .stat(&ds, &child)
                    .map_or(0, |s| s.size)
            } else {
                0
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

/// Split a LamBoot `&Path` into the in-dataset components lamzfs expects. Both
/// use UTF-8 + forward slashes; empty and `.` components are dropped (so
/// `/BOOT//x` and `/BOOT/x` resolve identically, and `/` yields `[]` = the
/// dataset root).
fn components(path: &Path) -> Vec<&str> {
    path.as_str()
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// The volume label for a dataset: the pool name for the root dataset, else the
/// dataset path joined (e.g. `BOOT/ubuntu_x1`).
fn dataset_label(pool_name: &str, dataset: &[String]) -> String {
    if dataset.is_empty() {
        pool_name.to_string()
    } else {
        dataset.join("/")
    }
}

/// Translate a `lamzfs::Error` into `FsError`, preserving stable `&'static str`
/// tokens for trust-log emission. A checksum mismatch becomes
/// [`FsError::IntegrityFailed`] (a security status), never a generic parse error.
#[expect(
    clippy::needless_pass_by_value,
    reason = "called as a function pointer via .map_err; lamzfs::Error is a small \
              enum and the conversion semantically consumes it at the FsBackend boundary"
)]
fn translate_zfs_error(err: lamzfs::Error) -> FsError {
    use lamzfs::Error as E;
    match err {
        E::NotFound { .. } => FsError::NotFound,
        E::NotARegularFile => FsError::IsDirectory,
        E::NotADirectory => FsError::NotDirectory,
        E::NotASymlink => FsError::Unsupported("zfs_not_a_symlink"),
        E::FileTooLarge { size, max } => FsError::FileTooLarge { size, max },
        E::UnsupportedFeature(feature) | E::UnsupportedTopology(feature) => {
            FsError::UnsupportedFeature {
                feature,
                backend: ZFS_BACKEND_TAG,
            }
        }
        E::UnsupportedCompression { .. } => FsError::UnsupportedFeature {
            feature: "zfs_compression",
            backend: ZFS_BACKEND_TAG,
        },
        E::Unsupported(token) => FsError::Unsupported(token),
        E::ChecksumMismatch { what, .. } => FsError::IntegrityFailed(what),
        // A structural inconsistency, a bad compressed block, or a member I/O
        // failure all present as unreadable on-disk data; each carries a stable
        // token preserved for the trust log.
        E::Io { token, .. } | E::Inconsistent { token, .. } | E::BadCompression { token, .. } => {
            FsError::Corrupt(token)
        }
        E::BadLabel(_) => FsError::Corrupt("zfs_bad_label"),
        E::NoValidUberblock => FsError::Corrupt("zfs_no_valid_uberblock"),
        E::OutOfMemory { site } => FsError::Unsupported(site),
    }
}

/// Map a `lamzfs::Stat` to a `Metadata`. ZFS has POSIX modes on disk, but the
/// reader does not decode them; synthesize read-only-media values (dirs 0o555,
/// files 0o444) as the FAT-RO / exFAT backends do — the backend is read-only
/// regardless.
fn translate_stat(stat: &lamzfs::Stat) -> Metadata {
    let kind = kind_from_entry(stat.kind);
    let mode = match kind {
        FileKind::Directory => 0o555,
        _ => 0o444,
    };
    Metadata {
        kind,
        size: stat.size,
        mode,
        read_only: true,
        source_backend: ZFS_BACKEND_TAG,
    }
}

/// Map a lamzfs entry kind to a `FileKind`.
fn kind_from_entry(kind: EntryKind) -> FileKind {
    match kind {
        EntryKind::Regular => FileKind::Regular,
        EntryKind::Directory => FileKind::Directory,
        EntryKind::Symlink => FileKind::Symlink,
        EntryKind::Other => FileKind::Other,
    }
}
