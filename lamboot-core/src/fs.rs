//! Layer-2 filesystem coordinator.
//!
//! Owns `Volume` — the caller-facing handle for reading any mounted
//! filesystem — plus the per-boot cache. Every Layer-3+ consumer that
//! used to talk to `EspVolume` in v0.8.3 now talks to `Volume`.
//!
//! The `FsBackend` trait lives in `fs_backend.rs`; concrete backends
//! (`FatBackend`, `Ext4Backend`) live in `fs_backend_fat.rs` and
//! `fs_backend_ext4.rs`. Writes are routed through `fs_writer::EspWriter`
//! — never through this module.

#![expect(
    dead_code,
    reason = "Volume exposes the full spec-mandated API (read_dir, open_stream, \
              invalidate_all, metadata, etc.). Not every method is called today \
              — SDS-2/3/5/7 bring the remaining consumers. Keeping the API \
              complete avoids breaking signature additions."
)]

use alloc::{
    boxed::Box,
    collections::VecDeque,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};

use log::{debug, info};
use uefi::{proto::media::fs::SimpleFileSystem, Handle};

use crate::{
    fs_backend::{
        BackendTag, DirEntry, FsBackend, FsError, FsStream, Metadata, Path, PathBuf, Uuid,
    },
    fs_backend_fat::FatBackend,
};

// ---------------------------------------------------------------------------
// VolumeIdentity
// ---------------------------------------------------------------------------

/// Stable per-boot identity for a mounted volume.
///
/// Producers (the mount path in `partitions.rs`) set everything at construction.
/// Consumers treat this as read-only metadata for logs, BLS source tagging,
/// and debugging.
#[derive(Clone, Debug)]
pub(crate) struct VolumeIdentity {
    /// GPT partition GUID (unique per partition, if the volume lives on a
    /// GPT-partitioned disk).
    pub partition_guid: Option<uefi::Guid>,
    /// Filesystem UUID if the backend can expose one (ext4 superblock UUID,
    /// FAT serial widened to 16 bytes).
    pub fs_uuid: Option<Uuid>,
    /// Filesystem label if present and non-empty.
    pub label: Option<String>,
    /// Zero-based index assigned at mount order. Stable within a boot.
    pub index: u32,
    /// Short stable backend tag (`"fat"`, `"ext4-view@0.9.3"`, ...).
    pub backend_tag: BackendTag,
}

impl VolumeIdentity {
    /// Render a short display label suitable for logs:
    /// `[fat #0 UUID=xxxx label=EFI]` or similar.
    pub(crate) fn describe(&self) -> String {
        use core::fmt::Write as _;
        let mut out = format!("[{} #{}", self.backend_tag, self.index);
        if let Some(uuid) = self.fs_uuid {
            let _ = write!(out, " fs-uuid={uuid}");
        }
        if let Some(guid) = self.partition_guid {
            let _ = write!(out, " part={guid}");
        }
        if let Some(ref lbl) = self.label {
            let _ = write!(out, " label={lbl}");
        }
        out.push(']');
        out
    }
}

// ---------------------------------------------------------------------------
// VolumeCache — per-volume LRU
// ---------------------------------------------------------------------------

/// Cache limit — hard-coded in v0.9.0, promoted to `policy.toml` in a later
/// release per SPEC-FS-BACKEND-TRAIT §4.3.
const CACHE_BYTES_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct CacheKey {
    path: String,
    offset: u64,
    len: usize,
}

impl CacheKey {
    fn full_read(path: &Path) -> Self {
        Self {
            path: path.as_str().to_string(),
            offset: 0,
            len: usize::MAX,
        }
    }

    fn range(path: &Path, offset: u64, len: usize) -> Self {
        Self {
            path: path.as_str().to_string(),
            offset,
            len,
        }
    }

    fn matches(&self, other: &CacheKey) -> bool {
        self.path == other.path && self.offset == other.offset && self.len == other.len
    }
}

struct CacheEntry {
    key: CacheKey,
    data: Arc<[u8]>,
}

pub(crate) struct VolumeCache {
    entries: VecDeque<CacheEntry>,
    used_bytes: usize,
    limit_bytes: usize,
}

impl VolumeCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            used_bytes: 0,
            limit_bytes: CACHE_BYTES_LIMIT,
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<Arc<[u8]>> {
        let pos = self.entries.iter().position(|e| e.key.matches(key))?;
        let entry = self.entries.remove(pos)?;
        let data = entry.data.clone();
        self.entries.push_front(entry);
        Some(data)
    }

    fn insert(&mut self, key: CacheKey, data: Arc<[u8]>) {
        let data_len = data.len();
        if data_len > self.limit_bytes {
            return;
        }
        while self.used_bytes + data_len > self.limit_bytes {
            if let Some(evicted) = self.entries.pop_back() {
                self.used_bytes = self.used_bytes.saturating_sub(evicted.data.len());
            } else {
                break;
            }
        }
        self.used_bytes += data_len;
        self.entries.push_front(CacheEntry { key, data });
    }

    fn invalidate_path(&mut self, path: &Path) {
        let canonical = path.as_str();
        let mut freed = 0usize;
        self.entries.retain(|entry| {
            if entry.key.path == canonical {
                freed += entry.data.len();
                false
            } else {
                true
            }
        });
        self.used_bytes = self.used_bytes.saturating_sub(freed);
    }
}

// ---------------------------------------------------------------------------
// Volume — the caller-facing handle
// ---------------------------------------------------------------------------

/// A mounted filesystem, routed through a concrete backend.
///
/// `Volume` owns the backend. Dropping a `Volume` drops the backend, which
/// in turn releases any UEFI protocol handles it holds.
///
/// Not `Clone`, not `Copy`, not `Send`. Each volume holds exclusive access
/// to its UEFI protocol handles through its backend.
pub(crate) struct Volume {
    identity: VolumeIdentity,
    backend: Box<dyn FsBackend>,
    cache: VolumeCache,
    /// FAT-only shortcut — set when the volume was constructed from a
    /// `FatBackend`. Exposes the underlying `SimpleFileSystem` handle so
    /// `EspWriter` and `boot::chainload_efi` can reuse it without a dyn
    /// downcast. `None` for every non-FAT backend.
    fat_handle: Option<Handle>,
}

impl Volume {
    /// Construct a volume from a FAT backend + identity. Stashes the FAT
    /// handle so `fat_handle()` later can return it without downcasting.
    pub(crate) fn from_fat(identity: VolumeIdentity, backend: FatBackend) -> Self {
        let handle = backend.handle();
        Self {
            identity,
            backend: Box::new(backend),
            cache: VolumeCache::new(),
            fat_handle: Some(handle),
        }
    }

    /// Construct a volume from any non-FAT backend + identity.
    pub(crate) fn from_backend(identity: VolumeIdentity, backend: Box<dyn FsBackend>) -> Self {
        Self {
            identity,
            backend,
            cache: VolumeCache::new(),
            fat_handle: None,
        }
    }

    // ---------- identity ----------

    pub(crate) fn identity(&self) -> &VolumeIdentity {
        &self.identity
    }

    pub(crate) fn backend_tag(&self) -> BackendTag {
        self.identity.backend_tag
    }

    pub(crate) fn fs_uuid(&self) -> Option<Uuid> {
        self.identity.fs_uuid
    }

    pub(crate) fn partition_guid(&self) -> Option<uefi::Guid> {
        self.identity.partition_guid
    }

    pub(crate) fn label(&self) -> Option<&str> {
        self.identity.label.as_deref()
    }

    /// Return the underlying FAT `SimpleFileSystem` handle if the backend
    /// is `FatBackend`. `None` otherwise. Consulted by `EspWriter` (sole
    /// writer path) and `boot::chainload_efi` (child-image LoadImage).
    pub(crate) fn fat_handle(&self) -> Option<Handle> {
        self.fat_handle
    }

    // ---------- read surface: &Path-based (primary) ----------

    pub(crate) fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let key = CacheKey::full_read(path);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(cached.to_vec());
        }
        let data = self.backend.read(path)?;
        let arc: Arc<[u8]> = Arc::from(data.as_slice());
        self.cache.insert(key, arc.clone());
        Ok(arc.to_vec())
    }

    pub(crate) fn read_to_string(&mut self, path: &Path) -> Result<String, FsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| FsError::InvalidPath("non-UTF-8 file content"))
    }

    pub(crate) fn read_at(
        &mut self,
        path: &Path,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, FsError> {
        let key = CacheKey::range(path, offset, len);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(cached.to_vec());
        }
        let data = self.backend.read_at(path, offset, len)?;
        let arc: Arc<[u8]> = Arc::from(data.as_slice());
        self.cache.insert(key, arc.clone());
        Ok(arc.to_vec())
    }

    pub(crate) fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        self.backend.exists(path)
    }

    pub(crate) fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        self.backend.metadata(path)
    }

    pub(crate) fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        self.backend.read_dir(path)
    }

    pub(crate) fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        self.backend.open_stream(path)
    }

    pub(crate) fn supports_streaming(&self) -> bool {
        self.backend.supports_streaming()
    }

    // ---------- read surface: &str convenience wrappers ----------
    //
    // Accept forward-slash literals (preferred) or legacy backslash
    // literals (auto-normalized). The backslash tolerance exists only
    // for the v0.8.3 → v0.9.x migration period.

    pub(crate) fn read_str(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let pb = PathBuf::from_str(&normalize_path_string(path))?;
        self.read(pb.as_path())
    }

    pub(crate) fn read_to_string_str(&mut self, path: &str) -> Result<String, FsError> {
        let pb = PathBuf::from_str(&normalize_path_string(path))?;
        self.read_to_string(pb.as_path())
    }

    pub(crate) fn read_at_str(
        &mut self,
        path: &str,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, FsError> {
        let pb = PathBuf::from_str(&normalize_path_string(path))?;
        self.read_at(pb.as_path(), offset, len)
    }

    pub(crate) fn exists_str(&mut self, path: &str) -> bool {
        let Ok(pb) = PathBuf::from_str(&normalize_path_string(path)) else {
            return false;
        };
        self.exists(pb.as_path()).unwrap_or(false)
    }

    pub(crate) fn read_dir_str(&mut self, path: &str) -> Result<Vec<String>, FsError> {
        let pb = PathBuf::from_str(&normalize_path_string(path))?;
        Ok(self
            .read_dir(pb.as_path())?
            .into_iter()
            .map(|e| e.name)
            .collect())
    }

    // ---------- cache hooks ----------

    pub(crate) fn invalidate_path(&mut self, path: &Path) {
        self.cache.invalidate_path(path);
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.cache = VolumeCache::new();
    }

    // ---------- glob helper (preserves v0.8.3 semantics) ----------

    /// Simple prefix/suffix/contains glob. Returns forward-slash paths.
    /// Accepts either forward-slash or backslash path patterns.
    pub(crate) fn glob(&mut self, pattern: &str) -> Vec<String> {
        let normalized = normalize_path_string(pattern);
        let Some(slash_pos) = normalized.rfind('/') else {
            return Vec::new();
        };
        let dir = if slash_pos == 0 {
            "/"
        } else {
            &normalized[..slash_pos]
        };
        let file_pattern = &normalized[slash_pos + 1..];

        let Ok(dir_pb) = PathBuf::from_str(dir) else {
            return Vec::new();
        };
        let Ok(entries) = self.read_dir(dir_pb.as_path()) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for entry in entries {
            if matches_glob(&entry.name, file_pattern) {
                if dir == "/" {
                    out.push(format!("/{}", entry.name));
                } else {
                    out.push(format!("{dir}/{}", entry.name));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Path-string normalization helper
// ---------------------------------------------------------------------------

/// Normalize a legacy call-site path string to the canonical forward-slash
/// absolute form that `PathBuf::from_str` accepts.
///
/// * Converts backslashes to forward-slashes.
/// * Prepends `/` if the input had no leading slash.
pub(crate) fn normalize_path_string(s: &str) -> String {
    if s.is_empty() {
        return String::from("/");
    }
    let mut translated: String = s.chars().map(|c| if c == '\\' { '/' } else { c }).collect();
    if !translated.starts_with('/') {
        translated.insert(0, '/');
    }
    translated
}

// ---------------------------------------------------------------------------
// Glob matcher — matches v0.8.3 behavior exactly
// ---------------------------------------------------------------------------

fn matches_glob(name: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "*.*" {
        return true;
    }
    if pattern.starts_with('*') && pattern.ends_with('*') && pattern.len() >= 2 {
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

// ---------------------------------------------------------------------------
// Top-level mount helpers
// ---------------------------------------------------------------------------

/// Mount the ESP from the loaded image's device as a `Volume`.
///
/// Retains the v0.8.3 contract: takes the image handle, consults
/// `LoadedImage`, opens `SimpleFileSystem` on the device, wraps the FAT
/// backend in a `Volume`.
pub(crate) fn mount_esp(image: Handle) -> Result<Volume, FsError> {
    let loaded =
        uefi::boot::open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(image)?;
    let device_handle = loaded.device().ok_or(FsError::Io {
        source: uefi::Error::from(uefi::Status::NOT_FOUND),
    })?;

    // Confirm `SimpleFileSystem` is available before we construct the
    // backend — firmware can install `LoadedImage` without exposing FS
    // protocol if the ESP isn't mountable (vanishingly rare but possible).
    {
        let _probe = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(device_handle)?;
    }

    let backend = FatBackend::new(device_handle)?;
    let identity = VolumeIdentity {
        partition_guid: partition_guid_for(device_handle),
        fs_uuid: backend.uuid(),
        label: backend.label().map(ToString::to_string),
        index: 0,
        backend_tag: FatBackend::TAG,
    };
    info!("Mounted ESP {}", identity.describe());
    Ok(Volume::from_fat(identity, backend))
}

/// Enumerate every `SimpleFileSystem` the firmware exposes and wrap each
/// as a FAT `Volume`, numbering starting from `starting_index` (typically
/// `1` — index `0` is reserved for the primary ESP returned by `mount_esp`).
/// Volumes already held exclusively are skipped silently.
pub(crate) fn enumerate_volumes() -> Vec<Volume> {
    enumerate_fat_volumes(1)
}

fn enumerate_fat_volumes(starting_index: u32) -> Vec<Volume> {
    let mut volumes = Vec::new();

    let Ok(handles) = uefi::boot::find_handles::<SimpleFileSystem>() else {
        return volumes;
    };

    let mut next_index = starting_index;
    for handle in handles {
        // Skip handles we can't open exclusively (ESP is open elsewhere, etc).
        if uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle).is_err() {
            continue;
        }
        let Ok(backend) = FatBackend::new(handle) else {
            continue;
        };
        let identity = VolumeIdentity {
            partition_guid: partition_guid_for(handle),
            fs_uuid: backend.uuid(),
            label: backend.label().map(ToString::to_string),
            index: next_index,
            backend_tag: FatBackend::TAG,
        };
        debug!("Enumerated FAT volume {}", identity.describe());
        volumes.push(Volume::from_fat(identity, backend));
        next_index += 1;
    }

    volumes
}

/// Helper used by mount paths to look up the GPT partition GUID for a
/// device handle. Returns `None` if `PartitionInfo` isn't available.
pub(crate) fn partition_guid_for(handle: Handle) -> Option<uefi::Guid> {
    use uefi::proto::media::partition::{PartitionInfo, PartitionType};
    let part_info = uefi::boot::open_protocol_exclusive::<PartitionInfo>(handle).ok()?;
    let pt = { part_info.partition_type };
    if pt != PartitionType::GPT {
        return None;
    }
    let entry = part_info.gpt_partition_entry()?;
    // Copying the packed field into a local to avoid an unaligned borrow.
    let unique = { entry.unique_partition_guid };
    Some(unique)
}
