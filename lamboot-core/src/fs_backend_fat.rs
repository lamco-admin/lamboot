//! FAT adapter for `FsBackend`.
//!
//! Wraps UEFI's `SimpleFileSystem` + `File` protocols. Preserves v0.8.3
//! behavior exactly — same path resolution, same file-mode semantics —
//! but accessed through the uniform `FsBackend` interface.
//!
//! Design choices (see SPEC-FS-BACKEND-TRAIT §8):
//!   * Per-call protocol re-open. Keeps `FatBackend` free of borrow-lifetime
//!     plumbing at negligible runtime cost (open_protocol_exclusive is ~µs
//!     at boot time, called dozens of times across a whole boot).
//!   * Forward-slash to backslash translation happens at the I/O boundary
//!     only. Callers never see backslashes.
//!   * `supports_streaming() = true` — FAT is the reference streaming
//!     backend for the PE loader in SDS-3.

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use uefi::{
    proto::media::{
        file::{File as _, FileAttribute, FileInfo, FileMode, RegularFile},
        fs::SimpleFileSystem,
    },
    CString16, Handle, Status,
};

use crate::fs_backend::{
    BackendTag, DirEntry, FileKind, FsBackend, FsError, FsStream, Metadata, Path, Uuid,
};

/// FAT filesystem backend.
pub(crate) struct FatBackend {
    handle: Handle,
    uuid: Option<Uuid>,
    label: Option<String>,
}

impl FatBackend {
    pub(crate) const TAG: BackendTag = "fat";

    /// Bind a FAT backend to a `SimpleFileSystem` handle. Queries the volume
    /// for label and serial at construction time so later calls don't need
    /// to re-open just to produce identity.
    ///
    /// The return type is `Result` even though the current implementation is
    /// infallible: future probes (volume-label UTF-16 decode, fsinfo serial
    /// extraction) may legitimately fail and must surface that failure
    /// without silently masking it. Callers today should treat `Err` as a
    /// reason to skip the volume.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "fallible signature reserved for upcoming FAT serial + UTF-16 label probes; \
                  keeping it now prevents a breaking signature change when those land"
    )]
    pub(crate) fn new(handle: Handle) -> Result<Self, FsError> {
        let (uuid, label) = probe_identity(handle).unwrap_or((None, None));
        Ok(Self {
            handle,
            uuid,
            label,
        })
    }

    /// The underlying UEFI handle — exposed only for the write path
    /// (`fs_writer::EspWriter`), which also needs `SimpleFileSystem`.
    pub(crate) fn handle(&self) -> Handle {
        self.handle
    }

    fn open_root(&self) -> Result<uefi::proto::media::file::Directory, FsError> {
        let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(self.handle)?;
        let root = fs.open_volume()?;
        Ok(root)
    }
}

/// Translate a canonical forward-slash path to a UEFI backslash CString16.
///
/// `/EFI/LamBoot/policy.toml` → `\EFI\LamBoot\policy.toml`
/// `/`                         → `\`
fn to_uefi_path(path: &Path) -> Result<CString16, FsError> {
    let s = path.as_str();
    if s == "/" {
        return CString16::try_from("\\").map_err(|_| FsError::InvalidPath("non-BMP codepoint"));
    }
    let translated: String = s.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    CString16::try_from(translated.as_str()).map_err(|_| FsError::InvalidPath("non-BMP codepoint"))
}

fn open_file_read(
    root: &mut uefi::proto::media::file::Directory,
    path: &Path,
) -> Result<RegularFile, FsError> {
    let cstr = to_uefi_path(path)?;
    let handle = root.open(&cstr, FileMode::Read, FileAttribute::empty())?;
    handle.into_regular_file().ok_or(FsError::IsDirectory)
}

fn stat_file(
    root: &mut uefi::proto::media::file::Directory,
    path: &Path,
) -> Result<Metadata, FsError> {
    let cstr = to_uefi_path(path)?;
    // Open with Read mode (works for both regular files and directories).
    let handle = root.open(&cstr, FileMode::Read, FileAttribute::empty())?;
    let mut buf = vec![0u8; 512];
    // Directory and RegularFile both have get_info via the File trait
    let kind = if handle.is_regular_file()? {
        FileKind::Regular
    } else if handle.is_directory()? {
        FileKind::Directory
    } else {
        FileKind::Other
    };

    // Narrow to a concrete handle type so we can call get_info reliably.
    let (size, read_only) = match kind {
        FileKind::Regular => {
            let mut regular = handle
                .into_regular_file()
                .ok_or(FsError::Corrupt("file kind mismatch after probe"))?;
            let info = regular
                .get_info::<FileInfo>(&mut buf)
                .map_err(|err| FsError::from(err.to_err_without_payload()))?;
            let ro = info.attribute().contains(FileAttribute::READ_ONLY);
            (info.file_size(), ro)
        }
        FileKind::Directory => {
            let mut dir = handle
                .into_directory()
                .ok_or(FsError::Corrupt("file kind mismatch after probe"))?;
            let info = dir
                .get_info::<FileInfo>(&mut buf)
                .map_err(|err| FsError::from(err.to_err_without_payload()))?;
            let ro = info.attribute().contains(FileAttribute::READ_ONLY);
            (0, ro)
        }
        _ => (0, false),
    };

    let mode = match kind {
        FileKind::Directory => 0o755,
        FileKind::Regular if read_only => 0o444,
        FileKind::Regular => 0o644,
        _ => 0o000,
    };

    Ok(Metadata {
        kind,
        size,
        mode,
        read_only,
        source_backend: FatBackend::TAG,
    })
}

fn probe_identity(handle: Handle) -> Option<(Option<Uuid>, Option<String>)> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle).ok()?;
    let mut root = fs.open_volume().ok()?;
    let mut buf = vec![0u8; 512];
    let info = root
        .get_info::<uefi::proto::media::file::FileSystemVolumeLabel>(&mut buf)
        .ok()?;
    let label = info.volume_label().to_string();
    Some((None, if label.is_empty() { None } else { Some(label) }))
}

// ---------------------------------------------------------------------------
// FsBackend impl
// ---------------------------------------------------------------------------

impl FsBackend for FatBackend {
    fn tag(&self) -> BackendTag {
        FatBackend::TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let mut root = self.open_root()?;
        let mut file = open_file_read(&mut root, path)?;
        let mut info_buf = vec![0u8; 512];
        let info = file
            .get_info::<FileInfo>(&mut info_buf)
            .map_err(|err| FsError::from(err.to_err_without_payload()))?;
        let size = info.file_size() as usize;
        let mut out = vec![0u8; size];
        let got = file.read(&mut out)?;
        out.truncate(got);
        Ok(out)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let mut root = self.open_root()?;
        let mut file = open_file_read(&mut root, path)?;
        file.set_position(offset)?;
        let mut out = vec![0u8; len];
        let got = file.read(&mut out)?;
        out.truncate(got);
        Ok(out)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let mut root = self.open_root()?;
        let cstr = to_uefi_path(path)?;
        match root.open(&cstr, FileMode::Read, FileAttribute::empty()) {
            Ok(_) => Ok(true),
            Err(err) if err.status() == Status::NOT_FOUND => Ok(false),
            Err(err) => Err(FsError::from(err)),
        }
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let mut root = self.open_root()?;
        stat_file(&mut root, path)
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let mut root = self.open_root()?;
        let cstr = to_uefi_path(path)?;

        let handle = root.open(&cstr, FileMode::Read, FileAttribute::DIRECTORY)?;
        let mut dir = handle.into_directory().ok_or(FsError::NotDirectory)?;

        let mut entries = Vec::new();
        let mut buf = vec![0u8; 1024];
        loop {
            let info = match dir.read_entry(&mut buf) {
                Ok(Some(info)) => info,
                Ok(None) => break,
                Err(err) => return Err(FsError::from(err.to_err_without_payload())),
            };
            let name = info.file_name().to_string();
            if name == "." || name == ".." {
                continue;
            }
            let kind = if info.attribute().contains(FileAttribute::DIRECTORY) {
                FileKind::Directory
            } else {
                FileKind::Regular
            };
            entries.push(DirEntry {
                name,
                kind,
                size: info.file_size(),
            });
        }
        Ok(entries)
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        let mut root = self.open_root()?;
        let mut file = open_file_read(&mut root, path)?;
        let mut info_buf = vec![0u8; 512];
        let len = file
            .get_info::<FileInfo>(&mut info_buf)
            .map(|i| i.file_size())
            .ok();
        Ok(Box::new(FatStream {
            file,
            len,
            position: 0,
        }))
    }
}

// ---------------------------------------------------------------------------
// FatStream
// ---------------------------------------------------------------------------

pub(crate) struct FatStream {
    file: RegularFile,
    len: Option<u64>,
    position: u64,
}

impl FsStream for FatStream {
    fn len(&self) -> Option<u64> {
        self.len
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        let got = self.file.read(buf)?;
        self.position += got as u64;
        Ok(got)
    }

    fn seek(&mut self, offset: u64) -> Result<(), FsError> {
        self.file.set_position(offset)?;
        self.position = offset;
        Ok(())
    }
}

// Rename-file convenience retained for `EspWriter` — kept here because the
// operation is inherently FAT-specific and needs the same CString16 conversion
// machinery. Callers get to this through `fs_writer::EspWriter`.
pub(crate) fn fat_rename(
    handle: Handle,
    dir_path: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<(), FsError> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;
    let old_full = dir_path.join(old_name)?;
    let new_full = dir_path.join(new_name)?;

    // Read content of old file.
    let mut old_file = open_file_read(&mut root, old_full.as_path())?;
    let mut info_buf = vec![0u8; 512];
    let size = old_file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|err| FsError::from(err.to_err_without_payload()))?
        .file_size() as usize;
    let mut content = vec![0u8; size];
    let got = old_file.read(&mut content)?;
    content.truncate(got);
    drop(old_file);

    // Write to new path (create or overwrite).
    fat_write(handle, new_full.as_path(), &content)?;

    // Delete old.
    let old_cstr = to_uefi_path(old_full.as_path())?;
    let handle_old = root.open(&old_cstr, FileMode::ReadWrite, FileAttribute::empty())?;
    if let Some(regular) = handle_old.into_regular_file() {
        regular.delete().map_err(|_| FsError::Io {
            source: uefi::Error::from(Status::DEVICE_ERROR),
        })?;
    }
    Ok(())
}

pub(crate) fn fat_write(handle: Handle, path: &Path, data: &[u8]) -> Result<(), FsError> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;
    let cstr = to_uefi_path(path)?;

    // Delete first so shrinking writes leave no tail bytes. UEFI's
    // CreateReadWrite does not truncate existing files.
    if let Ok(existing) = root.open(&cstr, FileMode::ReadWrite, FileAttribute::empty()) {
        if let Some(reg) = existing.into_regular_file() {
            let _ = reg.delete();
        }
    }

    let handle = root.open(&cstr, FileMode::CreateReadWrite, FileAttribute::empty())?;
    let mut regular = handle.into_regular_file().ok_or(FsError::IsDirectory)?;
    regular
        .write(data)
        .map_err(|err| FsError::from(err.to_err_without_payload()))?;
    Ok(())
}

pub(crate) fn fat_append(handle: Handle, path: &Path, data: &[u8]) -> Result<(), FsError> {
    // Read existing content if any, append, write back. Required for the
    // UEFI-portable append semantics; set_position(file_size) + write is
    // subtly buggy on some OVMF builds (observed with boot-trust.log).
    let mut existing = match read_through(handle, path) {
        Ok(bytes) => bytes,
        Err(FsError::NotFound) => Vec::new(),
        Err(e) => return Err(e),
    };
    existing.extend_from_slice(data);
    fat_write(handle, path, &existing)
}

pub(crate) fn fat_ensure_dir(handle: Handle, path: &Path) -> Result<(), FsError> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;
    let cstr = to_uefi_path(path)?;
    let h = root.open(&cstr, FileMode::CreateReadWrite, FileAttribute::DIRECTORY)?;
    drop(h);
    Ok(())
}

/// FAT delete — used by `EspWriter::delete` for explicit file removal
/// (e.g. by SDS-7 `lamboot-migrate` when overwriting prior migration state).
pub(crate) fn fat_delete(handle: Handle, path: &Path) -> Result<(), FsError> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;
    let cstr = to_uefi_path(path)?;
    let h = root.open(&cstr, FileMode::ReadWrite, FileAttribute::empty())?;
    match h.into_regular_file() {
        Some(reg) => reg.delete().map_err(|_| FsError::Io {
            source: uefi::Error::from(Status::DEVICE_ERROR),
        }),
        None => Err(FsError::IsDirectory),
    }
}

fn read_through(handle: Handle, path: &Path) -> Result<Vec<u8>, FsError> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;
    let mut file = open_file_read(&mut root, path)?;
    let mut info_buf = vec![0u8; 512];
    let size = file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|err| FsError::from(err.to_err_without_payload()))?
        .file_size() as usize;
    let mut out = vec![0u8; size];
    let got = file.read(&mut out)?;
    out.truncate(got);
    Ok(out)
}
