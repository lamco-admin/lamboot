//! Layer: 2 — Storage & Filesystems.
//!
//! Shared ownership of an [`FsBackend`] open, so a `FileBlockSource` can read a
//! file-region (an `.iso`) out of the *same* backend a [`Volume`](crate::fs::Volume)
//! keeps serving the rest of boot — without re-opening it (which would re-arm the
//! ESP `SimpleFileSystem`-disconnect hazard documented at `main.rs`) and without
//! borrowing it (which fails the `'static` bound `dispatch_fs_over_source` and
//! the lamfold mounter require). See `SPEC-BOOT-FROM-ISO-DESIGN` §1.
//!
//! `no_std` boot is single-threaded, so `Rc<RefCell<…>>` is the right shared
//! cell — `!Send`/`!Sync`, matching `Volume` (which is already not `Send`).

use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::cell::RefCell;

use crate::{
    fs_backend::{FsBackend, FsError},
    fs_types::{BackendTag, DirEntry, Metadata, Path},
};

/// A single `FsBackend` open shared between the holding `Volume` and any
/// `FileBlockSource` reading a file-region out of it.
pub(crate) type SharedBackend = Rc<RefCell<Box<dyn FsBackend>>>;

/// How a [`Volume`](crate::fs::Volume) holds its backend. Most volumes own it
/// outright ([`Owned`](BackendOwn::Owned), today's path); a volume that hosts an
/// `.iso` a `FileBlockSource` reads through is promoted once to
/// [`Shared`](BackendOwn::Shared) by `Volume::share_backend`, after which the
/// `Volume` and the source both read through the same `Rc`.
pub(crate) enum BackendOwn {
    Owned(Box<dyn FsBackend>),
    Shared(SharedBackend),
}

/// A do-nothing backend used **only** as a transient placeholder while
/// `Volume::share_backend` swaps an `Owned` backend out to wrap it in an `Rc`.
/// It is never read through: the swap installs the real `Shared` backend in the
/// same call before returning.
pub(crate) struct NullBackend;

impl FsBackend for NullBackend {
    fn tag(&self) -> BackendTag {
        "null"
    }
    fn uuid(&self) -> Option<crate::fs_types::Uuid> {
        None
    }
    fn label(&self) -> Option<&str> {
        None
    }
    fn read(&mut self, _path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound)
    }
    fn read_at(&mut self, _path: &Path, _offset: u64, _len: usize) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound)
    }
    fn exists(&mut self, _path: &Path) -> Result<bool, FsError> {
        Ok(false)
    }
    fn metadata(&mut self, _path: &Path) -> Result<Metadata, FsError> {
        Err(FsError::NotFound)
    }
    fn read_dir(&mut self, _path: &Path) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotFound)
    }
}
