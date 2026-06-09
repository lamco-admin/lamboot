//! Layer: 2 — Storage & Filesystems.
//!
//! `LamfoldBackend` — the one generic adapter from a lamfold `FoldFrontend`
//! (node-addressed, over a `BlockSource`) to lamboot-core's path-based
//! [`FsBackend`]. One adapter serves every Family-A frontend (iso / udf /
//! squashfs / erofs / cramfs / romfs / cpio); each dispatch arm boxes a concrete
//! `LamfoldBackend<SourceReader<…>, Erofs<…>>` etc., so the generic never leaks
//! into `dyn FsBackend`. Read-only by construction — `read_only` is always
//! `true` and `EspWriter` rejects every lamfold tag. `SPEC-LAMFOLD-INTEGRATION`
//! §3.

use alloc::{boxed::Box, vec, vec::Vec};
use core::marker::PhantomData;

use lamfold::{BlockCache, FoldError, FoldFrontend, SubstrateCtx, Verifier};

use crate::{
    fs_backend::{FsBackend, FsError},
    fs_types::{BackendTag, DirEntry, FileKind, Metadata, Path, Uuid},
};

/// A mounted lamfold frontend adapted to [`FsBackend`]. Owns the frontend (which
/// owns its bridged `BlockSource`), an immutable decompressed-block cache, and
/// the shepherd — `NoVerifier` for unverified media, a `MerkleVerifier` when the
/// dispatcher supplies a trust root.
///
/// `S` is the frontend's source type. It appears only inside `F: FoldFrontend<S>`,
/// so a `PhantomData<S>` is required to pin it — without it the impl's `S` would
/// be unconstrained (E0207).
pub(crate) struct LamfoldBackend<S, F> {
    fs: F,
    cache: BlockCache,
    verifier: Box<dyn Verifier>,
    tag: BackendTag,
    _src: PhantomData<S>,
}

impl<S, F> LamfoldBackend<S, F>
where
    S: lamfold::BlockSource + 'static,
    F: FoldFrontend<S>,
{
    pub(crate) fn new(
        fs: F,
        cache: BlockCache,
        verifier: Box<dyn Verifier>,
        tag: BackendTag,
    ) -> Self {
        Self {
            fs,
            cache,
            verifier,
            tag,
            _src: PhantomData,
        }
    }
}

impl<S, F> FsBackend for LamfoldBackend<S, F>
where
    S: lamfold::BlockSource + 'static,
    F: FoldFrontend<S>,
{
    fn tag(&self) -> BackendTag {
        self.tag
    }

    // Per-format volume UUIDs (iso PVD, erofs sb) are surfaced in a later pass
    // (SPEC-LAMFOLD-INTEGRATION §6); the boot path does not key on them today.
    fn uuid(&self) -> Option<Uuid> {
        None
    }

    fn label(&self) -> Option<&str> {
        None
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let mut cx = SubstrateCtx {
            cache: &mut self.cache,
            verifier: &*self.verifier,
        };
        lamfold::read_path(&mut self.fs, &mut cx, path.as_str()).map_err(map_fold_err)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let mut cx = SubstrateCtx {
            cache: &mut self.cache,
            verifier: &*self.verifier,
        };
        let node = lamfold::resolve(&mut self.fs, &mut cx, path.as_str()).map_err(map_fold_err)?;
        let mut buf = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            let n = self
                .fs
                .read_at(node, offset + filled as u64, &mut buf[filled..], &mut cx)
                .map_err(map_fold_err)?;
            if n == 0 {
                break; // short read at EOF — return what we have
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        match self.metadata(path) {
            Ok(_) => Ok(true),
            Err(FsError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let tag = self.tag;
        let mut cx = SubstrateCtx {
            cache: &mut self.cache,
            verifier: &*self.verifier,
        };
        let md =
            lamfold::metadata_path(&mut self.fs, &mut cx, path.as_str()).map_err(map_fold_err)?;
        Ok(Metadata {
            kind: map_kind(md.kind),
            size: md.size,
            mode: md.mode,
            read_only: true,
            source_backend: tag,
        })
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let mut cx = SubstrateCtx {
            cache: &mut self.cache,
            verifier: &*self.verifier,
        };
        let entries =
            lamfold::read_dir_path(&mut self.fs, &mut cx, path.as_str()).map_err(map_fold_err)?;
        let mut out = Vec::with_capacity(entries.len());
        for e in entries {
            // lamfold's `DirEntry` carries no size; resolve it from the node (we
            // already hold its id). A failed stat degrades to 0 rather than
            // failing the whole listing.
            let size = self.fs.metadata(e.node, &mut cx).map_or(0, |m| m.size);
            out.push(DirEntry {
                name: e.name,
                kind: map_kind(e.kind),
                size,
            });
        }
        Ok(out)
    }
}

fn map_kind(k: lamfold::FileKind) -> FileKind {
    match k {
        lamfold::FileKind::Regular => FileKind::Regular,
        lamfold::FileKind::Directory => FileKind::Directory,
        lamfold::FileKind::Symlink => FileKind::Symlink,
        lamfold::FileKind::Other => FileKind::Other,
    }
}

/// `FoldError` → `FsError`. A verification refusal becomes the distinct, loud
/// [`FsError::IntegrityFailed`] — never folded into `Corrupt`; decompression
/// failures *are* treated as corruption. `pub(crate)` so the dispatcher can map
/// a frontend's `open()` error the same way.
#[expect(
    clippy::needless_pass_by_value,
    reason = "used directly as a `map_err` closure, which hands over the error by value"
)]
pub(crate) fn map_fold_err(e: FoldError) -> FsError {
    match e {
        FoldError::NotFound => FsError::NotFound,
        FoldError::NotDirectory => FsError::NotDirectory,
        FoldError::IsDirectory => FsError::IsDirectory,
        FoldError::InvalidPath(s) => FsError::InvalidPath(s),
        FoldError::Corrupt(s) | FoldError::Decompress(s) => FsError::Corrupt(s),
        FoldError::Unsupported(s) => FsError::Unsupported(s),
        FoldError::FileTooLarge { size, max } => FsError::FileTooLarge { size, max },
        FoldError::VerifyFailed(s) => FsError::IntegrityFailed(s),
        // lamfold's only `Io` source here is our own SourceReader bridge; the
        // firmware-level cause is already lost, so surface a device error.
        FoldError::Io(_) => FsError::Io {
            source: uefi::Error::from(uefi::Status::DEVICE_ERROR),
        },
    }
}
