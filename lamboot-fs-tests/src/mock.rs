//! In-memory mock backend — mirrors the contract of
//! `lamboot_core::fs_backend::FsBackend` without the UEFI dependency.
//!
//! Tests against this mock validate the semantics spec-required for every
//! backend implementor: error codes for missing files, directory-vs-file
//! distinctions, read-at offset handling, read-dir child filtering.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

use crate::fs_types::{BackendTag, DirEntry, FileKind, Metadata, Path, PathBuf};

/// Errors a host-side backend can produce. Mirrors the real `FsError`
/// variants that don't require a `uefi::Error` payload.
#[derive(Debug, Eq, PartialEq)]
pub enum MockError {
    NotFound,
    IsDirectory,
    NotDirectory,
    InvalidPath,
    Unsupported(&'static str),
    InjectedFailure(&'static str),
}

#[derive(Clone, Debug)]
enum Node {
    File(Vec<u8>),
    Directory,
}

/// Simple mock backend. The contract mirrors `FsBackend`:
/// - `&mut self` methods only.
/// - `read` returns full file bytes, `read_at` returns a byte range.
/// - `exists` never errors for "absent"; only for I/O injection.
/// - `read_dir` excludes `.` and `..`.
pub struct MockBackend {
    tag: BackendTag,
    nodes: BTreeMap<String, Node>,
    call_counts: BTreeMap<&'static str, u32>,
    fail_reads: bool,
}

impl MockBackend {
    pub fn new(tag: BackendTag) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(String::from("/"), Node::Directory);
        Self {
            tag,
            nodes,
            call_counts: BTreeMap::new(),
            fail_reads: false,
        }
    }

    pub fn add_file(&mut self, path: &str, data: &[u8]) {
        let canonical = PathBuf::from_str(path)
            .expect("test paths must be valid canonical paths")
            .as_str()
            .to_string();
        // Ensure each ancestor directory exists so read_dir works.
        let mut seg = String::from("/");
        for component in canonical
            .trim_start_matches('/')
            .rsplit_once('/')
            .map(|(head, _)| head.split('/').collect::<Vec<_>>())
            .unwrap_or_default()
        {
            if component.is_empty() {
                continue;
            }
            if seg != "/" {
                seg.push('/');
            }
            seg.push_str(component);
            self.nodes.entry(seg.clone()).or_insert(Node::Directory);
        }
        self.nodes.insert(canonical, Node::File(data.to_vec()));
    }

    pub fn add_dir(&mut self, path: &str) {
        let canonical = PathBuf::from_str(path)
            .expect("test paths must be valid canonical paths")
            .as_str()
            .to_string();
        self.nodes.insert(canonical, Node::Directory);
    }

    pub fn inject_read_failure(&mut self, fail: bool) {
        self.fail_reads = fail;
    }

    pub fn call_count(&self, method: &'static str) -> u32 {
        self.call_counts.get(method).copied().unwrap_or(0)
    }

    fn bump(&mut self, method: &'static str) {
        *self.call_counts.entry(method).or_insert(0) += 1;
    }

    pub fn tag(&self) -> BackendTag {
        self.tag
    }

    pub fn read(&mut self, path: &Path) -> Result<Vec<u8>, MockError> {
        self.bump("read");
        if self.fail_reads {
            return Err(MockError::InjectedFailure("read disabled"));
        }
        match self.nodes.get(path.as_str()) {
            Some(Node::File(bytes)) => Ok(bytes.clone()),
            Some(Node::Directory) => Err(MockError::IsDirectory),
            None => Err(MockError::NotFound),
        }
    }

    pub fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, MockError> {
        self.bump("read_at");
        if self.fail_reads {
            return Err(MockError::InjectedFailure("read_at disabled"));
        }
        let bytes = match self.nodes.get(path.as_str()) {
            Some(Node::File(b)) => b,
            Some(Node::Directory) => return Err(MockError::IsDirectory),
            None => return Err(MockError::NotFound),
        };
        let off = offset as usize;
        if off >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = (off + len).min(bytes.len());
        Ok(bytes[off..end].to_vec())
    }

    pub fn exists(&mut self, path: &Path) -> Result<bool, MockError> {
        self.bump("exists");
        if self.fail_reads {
            return Err(MockError::InjectedFailure("exists disabled"));
        }
        Ok(self.nodes.contains_key(path.as_str()))
    }

    pub fn metadata(&mut self, path: &Path) -> Result<Metadata, MockError> {
        self.bump("metadata");
        let node = self
            .nodes
            .get(path.as_str())
            .ok_or(MockError::NotFound)?
            .clone();
        Ok(match node {
            Node::File(bytes) => Metadata {
                kind: FileKind::Regular,
                size: bytes.len() as u64,
                mode: 0o644,
                read_only: false,
                source_backend: self.tag,
            },
            Node::Directory => Metadata {
                kind: FileKind::Directory,
                size: 0,
                mode: 0o755,
                read_only: false,
                source_backend: self.tag,
            },
        })
    }

    pub fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, MockError> {
        self.bump("read_dir");
        match self.nodes.get(path.as_str()) {
            Some(Node::Directory) => {}
            Some(Node::File(_)) => return Err(MockError::NotDirectory),
            None => return Err(MockError::NotFound),
        }
        let prefix = if path.as_str() == "/" {
            String::from("/")
        } else {
            let mut s = path.as_str().to_string();
            s.push('/');
            s
        };
        let mut out = Vec::new();
        for (full, node) in &self.nodes {
            if full == path.as_str() {
                continue;
            }
            if !full.starts_with(&prefix) {
                continue;
            }
            let remainder = &full[prefix.len()..];
            if remainder.is_empty() || remainder.contains('/') {
                continue;
            }
            let (kind, size) = match node {
                Node::File(b) => (FileKind::Regular, b.len() as u64),
                Node::Directory => (FileKind::Directory, 0),
            };
            out.push(DirEntry {
                name: remainder.to_string(),
                kind,
                size,
            });
        }
        Ok(out)
    }
}
