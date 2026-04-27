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
//! ext4 read-write in UEFI. The FAT ESP, in contrast, is a small,
//! append-only-in-practice workspace for trust logs, boot reports, and
//! NVRAM mirrors — writing there is well-understood and auditable.

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
    fs_backend_fat::{fat_append, fat_delete, fat_ensure_dir, fat_rename, fat_write, FatBackend},
};

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
        fat_write(self.handle, path, data)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// Append `data` to `path`, creating if missing. Implemented as
    /// read-existing + concat + overwrite for UEFI portability.
    pub(crate) fn append(&mut self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        fat_append(self.handle, path, data)?;
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
        fat_rename(self.handle, dir_path, old_name, new_name)?;
        // Invalidate both — neither is guaranteed to match the cache key
        // exactly (caller may have read under a different case) but this
        // is the conservative choice.
        let old_full = dir_path.join(old_name)?;
        let new_full = dir_path.join(new_name)?;
        self.volume.invalidate_path(old_full.as_path());
        self.volume.invalidate_path(new_full.as_path());
        Ok(())
    }

    /// Create `path` as a directory if it doesn't already exist. Idempotent.
    pub(crate) fn ensure_dir(&mut self, path: &Path) -> Result<(), FsError> {
        fat_ensure_dir(self.handle, path)?;
        self.volume.invalidate_path(path);
        Ok(())
    }

    /// Delete the regular file at `path`. Returns `FsError::IsDirectory`
    /// if `path` names a directory.
    pub(crate) fn delete(&mut self, path: &Path) -> Result<(), FsError> {
        fat_delete(self.handle, path)?;
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
