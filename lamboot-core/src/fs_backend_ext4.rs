//! ext4 backend skeleton.
//!
//! SDS-1 lands the trait and a minimal skeleton that returns `Unsupported`
//! for every read method. Full implementation is SDS-2 (`ext4-view` crate
//! integration over UEFI `DiskIo`).
//!
//! Keeping the skeleton in-tree ensures:
//!   * The FsBackend trait remains object-safe for a second implementor.
//!   * SDS-2 lands as a pure drop-in replacement (no new module, no new
//!     imports anywhere else).
//!   * CI catches trait-signature regressions the moment they happen —
//!     if SDS-2's work breaks the trait, this skeleton stops compiling.

#![expect(
    dead_code,
    reason = "SDS-1 ext4 skeleton. Every item here is consumed by SDS-2 in-place, \
              keeping the trait object-safe for a second implementor without \
              any breaking-signature change."
)]

use alloc::{boxed::Box, vec::Vec};

use uefi::Handle;

use crate::fs_backend::{BackendTag, DirEntry, FsBackend, FsError, FsStream, Metadata, Path, Uuid};

pub(crate) struct Ext4Backend {
    _block_handle: Handle,
}

impl Ext4Backend {
    pub(crate) const TAG: BackendTag = "ext4-view@skeleton";

    /// Bind an ext4 backend to a UEFI `BlockIO`/`DiskIo` handle.
    ///
    /// SDS-1 skeleton: accepts the handle, does not attempt to parse the
    /// superblock. SDS-2 will perform mount-time superblock validation here
    /// and fail construction if the filesystem is not a supported ext4
    /// revision — so the `Result` return is the correct long-term shape,
    /// kept here to lock the contract before SDS-2 lands.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "SDS-2 implementation will return Err on unsupported ext4 revisions; \
                  maintain the fallible signature so SDS-2 is a pure in-place swap"
    )]
    pub(crate) fn new(block_handle: Handle) -> Result<Self, FsError> {
        Ok(Self {
            _block_handle: block_handle,
        })
    }
}

impl FsBackend for Ext4Backend {
    fn tag(&self) -> BackendTag {
        Ext4Backend::TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        None
    }

    fn label(&self) -> Option<&str> {
        None
    }

    fn read(&mut self, _path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }

    fn read_at(&mut self, _path: &Path, _offset: u64, _len: usize) -> Result<Vec<u8>, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }

    fn exists(&mut self, _path: &Path) -> Result<bool, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }

    fn metadata(&mut self, _path: &Path) -> Result<Metadata, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }

    fn read_dir(&mut self, _path: &Path) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    fn open_stream(&mut self, _path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        Err(FsError::Unsupported("ext4 backend: SDS-2 not implemented"))
    }
}
