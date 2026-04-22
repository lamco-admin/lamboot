use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use uefi::{
    prelude::*,
    proto::media::{
        file::{Directory, File, FileAttribute, FileInfo, FileMode, RegularFile},
        fs::SimpleFileSystem,
    },
    CString16, Result,
};

pub(crate) struct EspVolume {
    root: Directory,
}

impl EspVolume {
    pub(crate) fn new(root: Directory) -> Self {
        Self { root }
    }

    /// Check if a file exists at the given path
    pub(crate) fn exists(&mut self, path: &str) -> bool {
        self.open_file(path).is_ok()
    }

    /// Open a file for reading
    pub(crate) fn open_file(&mut self, path: &str) -> Result<RegularFile> {
        let path_cstr = CString16::try_from(path).map_err(|_| Status::INVALID_PARAMETER)?;

        // Clone directory by re-opening root
        // We need to work around Directory not being Clone
        let handle = self
            .root
            .open(&path_cstr, FileMode::Read, FileAttribute::empty())?
            .into_regular_file()
            .ok_or(Status::INVALID_PARAMETER)?;

        Ok(handle)
    }

    /// Read entire file contents into a vector
    pub(crate) fn read_to_vec(&mut self, path: &str) -> Result<Vec<u8>> {
        let mut file = self.open_file(path)?;

        // Get file size
        let mut info_buffer = [0u8; 512];
        let info = file
            .get_info::<FileInfo>(&mut info_buffer)
            .discard_errdata()?;
        let file_size = info.file_size() as usize;

        // Read file contents
        let mut buffer = alloc::vec![0; file_size];
        file.read(&mut buffer)?;

        Ok(buffer)
    }

    /// Read a byte range from a file (seek to offset, read len bytes)
    pub(crate) fn read_file_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut file = self.open_file(path)?;
        file.set_position(offset)?;
        let mut buffer = alloc::vec![0; len];
        let bytes_read = file.read(&mut buffer)?;
        buffer.truncate(bytes_read);
        Ok(buffer)
    }

    /// Read file as UTF-8 string
    pub(crate) fn read_to_string(&mut self, path: &str) -> Result<String> {
        let bytes = self.read_to_vec(path)?;
        String::from_utf8(bytes).map_err(|_| Status::INVALID_PARAMETER.into())
    }

    /// Write data to a file (creates or overwrites)
    pub(crate) fn write_file(&mut self, path: &str, data: &[u8]) -> Result {
        let path_cstr = CString16::try_from(path).map_err(|_| Status::INVALID_PARAMETER)?;

        // Delete existing file first to avoid stale tail bytes when new
        // content is shorter than old (UEFI CreateReadWrite doesn't truncate)
        if let Ok(existing) =
            self.root
                .open(&path_cstr, FileMode::ReadWrite, FileAttribute::empty())
        {
            if let Some(regular_file) = existing.into_regular_file() {
                let _ = regular_file.delete();
            }
        }

        let file = self.root.open(
            &path_cstr,
            FileMode::CreateReadWrite,
            FileAttribute::empty(),
        )?;

        if let Some(mut regular_file) = file.into_regular_file() {
            regular_file.write(data).discard_errdata()?;
            Ok(())
        } else {
            Err(Status::INVALID_PARAMETER.into())
        }
    }

    /// Rename a file within the same directory.
    /// Used for BLS boot counting (+N[-M] suffix changes).
    /// Implemented as read-content → write-new → delete-old (portable across UEFI impls).
    pub(crate) fn rename_file(&mut self, dir_path: &str, old_name: &str, new_name: &str) -> Result {
        // Read old file contents
        let old_path = format!("{dir_path}\\{old_name}");
        let contents = self.read_to_vec(&old_path)?;

        // Write to new filename
        let new_path = format!("{dir_path}\\{new_name}");
        self.write_file(&new_path, &contents)?;

        // Delete old file
        let old_cstr =
            CString16::try_from(old_path.as_str()).map_err(|_| Status::INVALID_PARAMETER)?;
        let file_handle = self
            .root
            .open(&old_cstr, FileMode::ReadWrite, FileAttribute::empty())?;
        if let Some(regular_file) = file_handle.into_regular_file() {
            regular_file
                .delete()
                .map_err(|_| uefi::Error::from(Status::DEVICE_ERROR))?;
        }

        Ok(())
    }

    /// List files in a directory matching a simple glob pattern
    pub(crate) fn glob(&mut self, pattern: &str) -> Vec<String> {
        let mut results = Vec::new();

        // Simple pattern matching - supports basic wildcards
        // For MVP, we'll do simple prefix matching
        if let Some(dir_path) = pattern.rfind('\\') {
            let dir = &pattern[..dir_path];
            let file_pattern = &pattern[dir_path + 1..];

            if let Ok(entries) = self.read_dir(dir) {
                for entry in entries {
                    if matches_pattern(&entry, file_pattern) {
                        results.push(format!("{dir}\\{entry}"));
                    }
                }
            }
        }

        results
    }

    /// Read directory entries
    pub(crate) fn read_dir(&mut self, path: &str) -> Result<Vec<String>> {
        let path_cstr = CString16::try_from(path).map_err(|_| Status::INVALID_PARAMETER)?;

        let mut dir = self
            .root
            .open(&path_cstr, FileMode::Read, FileAttribute::DIRECTORY)?
            .into_directory()
            .ok_or(Status::INVALID_PARAMETER)?;

        let mut entries = Vec::new();
        let mut buffer = alloc::vec![0u8; 512];

        while let Ok(Some(info)) = dir.read_entry(&mut buffer) {
            let name = info.file_name().to_string();
            if name != "." && name != ".." {
                entries.push(name);
            }
        }

        Ok(entries)
    }

    /// Ensure a directory exists (creates if missing)
    pub(crate) fn ensure_dir(&mut self, path: &str) -> Result {
        let path_cstr = CString16::try_from(path).map_err(|_| Status::INVALID_PARAMETER)?;
        let handle = self.root.open(
            &path_cstr,
            FileMode::CreateReadWrite,
            FileAttribute::DIRECTORY,
        )?;
        drop(handle);
        Ok(())
    }
}

/// Mount the ESP (EFI System Partition) from the loaded image's device
pub(crate) fn mount_esp(image: Handle) -> Result<EspVolume> {
    let loaded_image_proto =
        uefi::boot::open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(image)?;
    let device_handle = loaded_image_proto.device().ok_or(Status::NOT_FOUND)?;

    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(device_handle)?;
    let root = fs.open_volume()?;

    Ok(EspVolume::new(root))
}

/// Enumerate all volumes with SimpleFileSystem protocol.
/// Returns volumes for every partition the firmware can read (FAT, and any
/// filesystem for which an EFI driver has been loaded).
/// Skips volumes that fail to open (e.g. due to exclusive access).
pub(crate) fn enumerate_volumes() -> Vec<EspVolume> {
    let mut volumes = Vec::new();

    let Ok(handles) = uefi::boot::find_handles::<SimpleFileSystem>() else {
        return volumes;
    };

    for handle in handles {
        // Exclusive open intentionally skips handles already opened (e.g. the ESP)
        let fs_result = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle);
        if let Ok(mut fs) = fs_result {
            if let Ok(root) = fs.open_volume() {
                volumes.push(EspVolume::new(root));
            }
        }
    }

    volumes
}

/// Simple pattern matching for file globbing
fn matches_pattern(name: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "*.*" {
        return true;
    }

    if pattern.starts_with('*') && pattern.ends_with('*') {
        let middle = &pattern[1..pattern.len() - 1];
        return name.contains(middle);
    }

    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }

    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }

    name == pattern
}
