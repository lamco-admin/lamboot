//! Layer 2 — Filesystem abstraction.
//!
//! Defines the `FsBackend` trait that every filesystem backend in LamBoot
//! implements, plus the `FsError` enum and `FsStream` trait. The pure
//! value types (`Path`, `PathBuf`, `Uuid`, `Metadata`, `FileKind`,
//! `DirEntry`, `PathError`) live in `fs_types.rs` and are re-exported
//! from here.
//!
//! Backends are read-only by design — writes go through
//! `fs_writer::EspWriter` and are FAT-ESP-only by construction. See
//! `docs/specs/SPEC-FS-BACKEND-TRAIT.md` for the full rationale.
//!
//! Layering rules:
//!   * A backend MAY call Layer-1 UEFI protocols (`SimpleFileSystem`,
//!     `BlockIO`, `DiskIo`).
//!   * A backend MUST NOT call Layer-3 parsers (`bls`, `uki`, PE loader)
//!     or Layer-4 policy.
//!   * A backend MUST NOT emit trust-log events — callers do, with full
//!     context.

#![expect(
    dead_code,
    reason = "SDS-1 lays Layer-2 infrastructure — several items (FsStream, \
              FsError variants, optional trait methods) are deliberately \
              API-ready and consumed by SDS-2 (ext4 backend), SDS-3 (native \
              PE loader streaming), and SDS-7 (lamboot-migrate). Keeping the \
              contract locked means those SDSes land as in-place implementations \
              rather than breaking API additions."
)]

use alloc::{boxed::Box, vec::Vec};
use core::fmt;

pub(crate) use crate::fs_types::{
    BackendTag, DirEntry, FileKind, Metadata, Path, PathBuf, PathError, Uuid,
};

// ---------------------------------------------------------------------------
// FsError
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum FsError {
    NotFound,
    PermissionDenied,
    IsDirectory,
    NotDirectory,
    InvalidPath(&'static str),
    Unsupported(&'static str),
    UnsupportedFeature {
        feature: &'static str,
        backend: BackendTag,
    },
    Corrupt(&'static str),
    Io {
        source: uefi::Error,
    },
    /// Backend-specific fallback — used only when an adapter genuinely
    /// cannot classify the underlying error.
    BackendOther(Box<dyn core::error::Error + Send + Sync>),
}

impl FsError {
    /// Short stable string for trust-log `status` field and log emission.
    pub(crate) fn as_log_token(&self) -> &'static str {
        match self {
            FsError::NotFound => "not_found",
            FsError::PermissionDenied => "permission_denied",
            FsError::IsDirectory => "is_directory",
            FsError::NotDirectory => "not_directory",
            FsError::InvalidPath(_) => "invalid_path",
            FsError::Unsupported(_) => "unsupported",
            FsError::UnsupportedFeature { .. } => "unsupported_feature",
            FsError::Corrupt(_) => "fs_corrupt",
            FsError::Io { .. } => "io_error",
            FsError::BackendOther(_) => "backend_other",
        }
    }

    /// True iff this error should be treated as "file absent" rather than
    /// an I/O failure — used by callers that probe for optional files.
    pub(crate) fn is_not_found(&self) -> bool {
        matches!(self, FsError::NotFound)
    }

    /// Best-matching UEFI `Status` for call sites that still return
    /// `uefi::Result<T>`. Used by `From<FsError> for uefi::Error` and by
    /// explicit error-mapping call sites.
    pub(crate) fn to_uefi_status(&self) -> uefi::Status {
        use uefi::Status;
        match self {
            FsError::NotFound => Status::NOT_FOUND,
            FsError::PermissionDenied => Status::ACCESS_DENIED,
            FsError::IsDirectory | FsError::NotDirectory | FsError::InvalidPath(_) => {
                Status::INVALID_PARAMETER
            }
            FsError::Unsupported(_) | FsError::UnsupportedFeature { .. } => Status::UNSUPPORTED,
            FsError::Corrupt(_) => Status::VOLUME_CORRUPTED,
            FsError::Io { source } => source.status(),
            FsError::BackendOther(_) => Status::DEVICE_ERROR,
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::NotFound => f.write_str("not found"),
            FsError::PermissionDenied => f.write_str("permission denied"),
            FsError::IsDirectory => f.write_str("is a directory"),
            FsError::NotDirectory => f.write_str("not a directory"),
            FsError::InvalidPath(reason) => write!(f, "invalid path: {reason}"),
            FsError::Unsupported(what) => write!(f, "unsupported: {what}"),
            FsError::UnsupportedFeature { feature, backend } => {
                write!(f, "unsupported feature {feature} on backend {backend}")
            }
            FsError::Corrupt(what) => write!(f, "filesystem corrupt: {what}"),
            FsError::Io { source } => write!(f, "I/O error: {source}"),
            FsError::BackendOther(err) => write!(f, "backend error: {err}"),
        }
    }
}

impl core::error::Error for FsError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            FsError::BackendOther(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

impl From<uefi::Error> for FsError {
    fn from(source: uefi::Error) -> Self {
        FsError::Io { source }
    }
}

impl From<PathError> for FsError {
    fn from(err: PathError) -> Self {
        match err {
            PathError::Empty => FsError::InvalidPath("empty"),
            PathError::NotAbsolute => FsError::InvalidPath("not absolute"),
            PathError::AboveRoot => FsError::InvalidPath("above root"),
        }
    }
}

impl From<FsError> for uefi::Error {
    fn from(err: FsError) -> Self {
        uefi::Error::from(err.to_uefi_status())
    }
}

// ---------------------------------------------------------------------------
// FsBackend trait
// ---------------------------------------------------------------------------

/// The single trait every filesystem backend implements.
///
/// All methods take `&mut self` because backends may mutate caches, file
/// handles, or superblock state across calls. Methods never panic on
/// normal error paths — they return `FsError`.
///
/// Object-safe: no generics on any method, no `Self: Sized` bounds.
pub(crate) trait FsBackend {
    // -- identity --
    fn tag(&self) -> BackendTag;
    fn uuid(&self) -> Option<Uuid>;
    fn label(&self) -> Option<&str>;

    // -- read surface --
    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError>;
    fn exists(&mut self, path: &Path) -> Result<bool, FsError>;
    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError>;
    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError>;

    // -- capabilities --
    fn supports_streaming(&self) -> bool {
        false
    }
    #[expect(
        unused_variables,
        reason = "default impl; backends that support streaming override"
    )]
    fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        Err(FsError::Unsupported("open_stream"))
    }
}

/// Streaming read handle returned by `FsBackend::open_stream` on backends
/// that support it. Lets the PE loader (SDS-3) read multi-megabyte images
/// without buffering the whole file in RAM before validation.
pub(crate) trait FsStream {
    /// Total byte length of the stream, or `None` if unknown.
    fn len(&self) -> Option<u64>;

    /// Fill `buf` starting at the current position; advance position by
    /// the number of bytes actually read. Returns `Ok(0)` at EOF.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Seek to absolute byte offset. Backends that cannot seek backward
    /// return `FsError::Unsupported("seek_backward")`.
    fn seek(&mut self, offset: u64) -> Result<(), FsError>;
}
