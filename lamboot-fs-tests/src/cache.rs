//! Host-side mirror of `lamboot_core::fs::VolumeCache`.
//!
//! The real cache in `lamboot-core` has the same structure and algorithm
//! but lives in a no_std UEFI binary crate that can't host `cargo test`.
//! This mirror is kept structurally identical to the real one so the §10.2
//! LRU tests validate the behavior both implementations must exhibit.
//!
//! If the real `VolumeCache` algorithm ever changes, this mirror changes
//! to match. The test suite is the executable spec.

// Test-support infrastructure: these items are exercised by the integration
// tests in `tests/`, a separate compilation unit, so the lib build sees them
// as dead. Removing them would delete live test scaffolding.
#![allow(dead_code)]

use alloc::{
    collections::VecDeque,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};

use crate::fs_types::Path;

const CACHE_BYTES_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CacheKey {
    pub path: String,
    pub offset: u64,
    pub len: usize,
}

impl CacheKey {
    pub(crate) fn full_read(path: &Path) -> Self {
        Self {
            path: path.as_str().to_string(),
            offset: 0,
            len: usize::MAX,
        }
    }

    pub(crate) fn range(path: &Path, offset: u64, len: usize) -> Self {
        Self {
            path: path.as_str().to_string(),
            offset,
            len,
        }
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
        Self::with_limit(CACHE_BYTES_LIMIT)
    }

    pub(crate) fn with_limit(limit_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            used_bytes: 0,
            limit_bytes,
        }
    }

    pub(crate) fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<Arc<[u8]>> {
        let pos = self.entries.iter().position(|e| e.key == *key)?;
        let entry = self.entries.remove(pos)?;
        let data = entry.data.clone();
        self.entries.push_front(entry);
        Some(data)
    }

    pub(crate) fn insert(&mut self, key: CacheKey, data: Vec<u8>) {
        let arc: Arc<[u8]> = Arc::from(data.as_slice());
        self.insert_arc(key, arc);
    }

    fn insert_arc(&mut self, key: CacheKey, data: Arc<[u8]>) {
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

    pub(crate) fn invalidate_path(&mut self, path: &Path) {
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

impl Default for VolumeCache {
    fn default() -> Self {
        Self::new()
    }
}
