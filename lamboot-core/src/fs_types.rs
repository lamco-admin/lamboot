//! Pure value types for the filesystem abstraction.
//!
//! Split out of `fs_backend.rs` so they can be host-tested without bringing
//! in any UEFI-specific dependencies. Every item here is `no_std` but has
//! zero reference to the `uefi` crate.
//!
//! * `Path` / `PathBuf` — canonical absolute forward-slash paths.
//! * `PathError` — construction errors for `PathBuf`.
//! * `Uuid` — 128-bit filesystem UUID (distinct from `uefi::Guid`).
//! * `FileKind` / `DirEntry` / `Metadata` — stat-style metadata.
//! * `BackendTag` — short `&'static str` identifier for a backend.

#![expect(
    dead_code,
    reason = "SDS-1 pure value types. Uuid::from_fat_serial and bytes are used by \
              SDS-2 (ext4 superblock UUID), Path::parent/file_name/try_from_str \
              by SDS-5 (BLS path splitting) and SDS-7 (lamboot-migrate). \
              Tested on host in the lamboot-fs-tests crate."
)]

use alloc::{
    borrow::ToOwned,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::fmt;

/// Short stable string used to identify a backend in logs and trust events.
/// Always returned as `&'static str` from `FsBackend::tag()`.
pub(crate) type BackendTag = &'static str;

// ---------------------------------------------------------------------------
// Path & PathBuf
// ---------------------------------------------------------------------------

/// An owned, canonical, absolute, forward-slash-separated, UTF-8 path.
///
/// Canonicalization performed at construction:
///   * Redundant separators collapsed (`/a//b` -> `/a/b`)
///   * `.` segments dropped (`/a/./b` -> `/a/b`)
///   * `..` segments resolved (`/a/../b` -> `/b`)
///   * Traversal above the filesystem root rejected with `PathError::AboveRoot`
///   * Empty input rejected with `PathError::Empty`
///   * Non-absolute input rejected with `PathError::NotAbsolute`
///
/// This type is intentionally not `std::path::PathBuf` (we are `no_std`) and
/// not `String` (the newtype lets the compiler enforce the contract).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PathBuf(pub(crate) String);

impl PathBuf {
    pub(crate) fn from_str(s: &str) -> Result<Self, PathError> {
        if s.is_empty() {
            return Err(PathError::Empty);
        }
        if !s.starts_with('/') {
            return Err(PathError::NotAbsolute);
        }

        let mut stack: Vec<&str> = Vec::new();
        for segment in s.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if stack.pop().is_none() {
                        return Err(PathError::AboveRoot);
                    }
                }
                other => stack.push(other),
            }
        }

        let mut out = String::with_capacity(s.len());
        if stack.is_empty() {
            out.push('/');
        } else {
            for seg in &stack {
                out.push('/');
                out.push_str(seg);
            }
        }
        Ok(Self(out))
    }

    pub(crate) fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::ops::Deref for PathBuf {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.as_path()
    }
}

/// A borrowed canonical path (the `&str` equivalent of `PathBuf`).
#[derive(Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub(crate) struct Path(str);

impl Path {
    fn new(s: &str) -> &Self {
        // SAFETY: `Path` is `#[repr(transparent)]` over `str`, so the
        // layouts are identical. Every construction path canonicalizes
        // before calling this.
        unsafe { &*(core::ptr::from_ref::<str>(s) as *const Path) }
    }

    /// Create a `&Path` directly from a string that the caller asserts is
    /// already canonical.
    pub(crate) fn from_canonical_str(s: &str) -> &Self {
        Path::new(s)
    }

    pub(crate) fn try_from_str(s: &str) -> Result<PathBuf, PathError> {
        PathBuf::from_str(s)
    }

    pub(crate) fn as_str(&self) -> &str {
        // SAFETY: `Path` is `#[repr(transparent)]` over `str`.
        unsafe { &*(core::ptr::from_ref::<Path>(self) as *const str) }
    }

    /// Final component (file or directory name), or `""` for `"/"`.
    pub(crate) fn file_name(&self) -> &str {
        match self.as_str().rsplit_once('/') {
            Some((_, last)) => last,
            None => "",
        }
    }

    /// Parent path, or `"/"` if the path is `/foo` or `/`.
    pub(crate) fn parent(&self) -> PathBuf {
        let s = self.as_str();
        if s == "/" {
            return PathBuf(String::from("/"));
        }
        match s.rsplit_once('/') {
            Some((head, _)) if !head.is_empty() => PathBuf(head.to_string()),
            _ => PathBuf(String::from("/")),
        }
    }

    /// Join a child component onto this path, canonicalizing.
    pub(crate) fn join(&self, child: &str) -> Result<PathBuf, PathError> {
        if child.starts_with('/') {
            return PathBuf::from_str(child);
        }
        let s = self.as_str();
        let combined = if s.ends_with('/') {
            format!("{s}{child}")
        } else {
            format!("{s}/{child}")
        };
        PathBuf::from_str(&combined)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ToOwned for Path {
    type Owned = PathBuf;
    fn to_owned(&self) -> PathBuf {
        PathBuf(self.as_str().to_string())
    }
}

impl core::borrow::Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathError {
    Empty,
    NotAbsolute,
    AboveRoot,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => f.write_str("path is empty"),
            PathError::NotAbsolute => f.write_str("path is not absolute"),
            PathError::AboveRoot => f.write_str("path traverses above root"),
        }
    }
}

// ---------------------------------------------------------------------------
// UUID
// ---------------------------------------------------------------------------

/// A 128-bit filesystem UUID (ext4 superblock UUID, FAT serial widened, etc.).
/// Distinct from `uefi::Guid`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Uuid(pub [u8; 16]);

impl Uuid {
    /// Widen a 32-bit FAT volume serial to a 16-byte UUID by zero-padding.
    pub(crate) fn from_fat_serial(serial: u32) -> Self {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&serial.to_le_bytes());
        Self(bytes)
    }

    pub(crate) fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
        )
    }
}

// ---------------------------------------------------------------------------
// Metadata & FileKind
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileKind {
    Regular,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Metadata {
    pub kind: FileKind,
    pub size: u64,
    pub mode: u32,
    pub read_only: bool,
    pub source_backend: BackendTag,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirEntry {
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
}
