//! Layer: 2 — Storage & Filesystems.
//!
//! FAT-only write path.
//!
//! v0.9.x Layer-2 design: backends are read-only. Writes go through
//! `EspWriter`, which is constructed from a `Volume` only if that volume's
//! backend is FAT. This makes "no ext4 writes, ever" a compile-time
//! property of the type system — you cannot hold an `EspWriter` whose
//! underlying volume is ext4.
//!
//! Rationale: on-disk writes to ext4 at boot time introduce journal-replay
//! hazards that trade away the security posture we gain from never-mounting
//! ext4 read-write in UEFI. The FAT ESP, in contrast, is a small workspace
//! for trust logs, boot reports, and NVRAM mirrors.
//!
//! Crash-consistency caveat: the underlying FAT primitives are NOT atomic.
//! `write` is delete-then-create (a crash between the two loses the file),
//! and `append`/`rename` read-modify-write the whole file. A crash, power
//! loss, or firmware hang (the ASUS G10AJ-class FAT-driver stall this code
//! works around is a real interruption source) mid-write can truncate or
//! lose the target. Treat these as best-effort. For the trust log, prefer
//! [`EspWriter::true_append`] (positional `SetPosition(u64::MAX)` + write of
//! the new tail only): it does not rewrite the existing file, so an
//! interrupted append loses at most the new tail, never the prior log. A
//! crash-consistent write-temp-then-atomic-replace primitive is future work
//! (it must be validated against the quirky-firmware FAT path before
//! replacing the delete-then-create `write`).

#![expect(
    dead_code,
    reason = "EspWriter::delete and read_many are infrastructure for SDS-7 \
              (lamboot-migrate) and diagnostic subcommands. Keeping them ready \
              prevents a breaking API addition when those land."
)]

use alloc::vec::Vec;

use crate::{
    fs::Volume,
    fs_backend::{FsError, Path},
    fs_backend_fat::{
        fat_append, fat_delete, fat_ensure_dir, fat_rename, fat_true_append, fat_write, FatBackend,
    },
};

/// Helper: fetch the volume's cached SimpleFileSystem or return
/// `FsError::Unsupported` if the volume isn't FAT. Single point so all
/// EspWriter methods get the same error story.
fn sfs_or_err(
    volume: &mut Volume,
) -> Result<&mut uefi::proto::media::fs::SimpleFileSystem, FsError> {
    volume
        .fat_sfs_mut()
        .ok_or(FsError::Unsupported("write requires a FAT volume"))
}

/// Write-only handle to a FAT-backed `Volume`.
///
/// Construction validates that the volume's backend is FAT. Every method
/// forwards to the FAT backend's low-level file operations; the `Volume`'s
/// read cache is invalidated on every write to keep cached reads honest.
pub(crate) struct EspWriter<'v> {
    volume: &'v mut Volume,
    handle: uefi::Handle,
}

impl<'v> EspWriter<'v> {
    /// Build an `EspWriter` on a volume whose backend is FAT.
    ///
    /// Returns `None` if the backend is not FAT. This is the only place in
    /// Layer 2 that branches on backend identity — every other consumer is
    /// backend-agnostic. The branching is intentional and documented.
    pub(crate) fn new(volume: &'v mut Volume) -> Option<Self> {
        if volume.backend_tag() != FatBackend::TAG {
            return None;
        }
        let handle = volume.fat_handle()?;
        Some(Self { volume, handle })
    }

    /// Overwrite `path` with `data`, creating the file if it doesn't exist.
    pub(crate) fn write(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_write(sfs, path, data)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// Append `data` to `path`, creating if missing. Implemented as
    /// read-existing + concat + overwrite for UEFI portability.
    pub(crate) fn append(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_append(sfs, path, data)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// True positional append via FileProtocol::SetPosition(u64::MAX) +
    /// Write. Writes ONLY `data` at the current EOF — no whole-file
    /// rewrite. Use for append-mode logs (trust_log, audit_log).
    pub(crate) fn true_append(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_true_append(sfs, path, data)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// Atomically rename `old_name` → `new_name` within `dir_path`.
    /// Implemented as read-content → write-new → delete-old. Used by BLS
    /// boot counting (`entry+3-0.conf` → `entry+2-1.conf`).
    pub(crate) fn rename(
        &mut self,
        dir_path: &Path,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_rename(sfs, dir_path, old_name, new_name)?;
        let old_full = dir_path.join(old_name)?;
        let new_full = dir_path.join(new_name)?;
        self.volume.invalidate_path(old_full.as_path());
        self.volume.invalidate_path(new_full.as_path());
        Ok(())
    }

    /// Create `path` as a directory if it doesn't already exist. Idempotent.
    pub(crate) fn ensure_dir(&mut self, path: &Path) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_ensure_dir(sfs, path)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// Delete the regular file at `path`. Returns `FsError::IsDirectory`
    /// if `path` names a directory.
    pub(crate) fn delete(&mut self, path: &Path) -> Result<(), FsError> {
        let sfs = sfs_or_err(self.volume)?;
        fat_delete(sfs, path)?;
        self.volume.invalidate_path(path);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Free helper for legacy call sites
// ---------------------------------------------------------------------------

/// Read a set of files in one go (useful for copying configs before writes).
/// Lives here rather than the `FsBackend` trait because it's a pure
/// convenience over `Volume::read`.
pub(crate) fn read_many<'p>(
    volume: &mut Volume,
    paths: impl IntoIterator<Item = &'p Path>,
) -> Vec<(&'p Path, Result<Vec<u8>, FsError>)> {
    paths
        .into_iter()
        .map(|p| {
            let result = volume.read(p);
            (p, result)
        })
        .collect()
}
