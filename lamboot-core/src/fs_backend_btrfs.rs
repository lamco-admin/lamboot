//! Layer: 2 — Storage & Filesystems.
//!
//! Native read-only btrfs backend.
//!
//! Implements `FsBackend` over the [`lambutter`] crate — a Lamco-authored
//! `no_std` read-only btrfs reader. Mirror of `fs_backend_ext4.rs`:
//! same architectural position, same protocol choice (BlockIO not DiskIo),
//! same error-translation discipline.
//!
//! # Why this exists
//!
//! Replaces the third-party EfiFs `btrfs_x64.efi` driver (GRUB-derived,
//! GPLv3, can't be Microsoft-signed under Path D). Loading EfiFs through
//! `BS->LoadImage` + `StartImage` also triggers shim 15.8's `ShimLock`
//! uninstall — same failure mode that motivated SDS-2. A native Rust
//! reader embedded in the LamBoot binary sidesteps both problems.
//!
//! # Architectural position
//!
//! * `partitions.rs::probe_superblock` identifies btrfs volumes by
//!   superblock magic at offset 0x10000. SDS-1 landed the unified probe;
//!   this module consumes its `FsInfo { fs_type: Btrfs, .. }` output.
//! * `fs.rs::Volume::from_backend` is the generic constructor for every
//!   non-FAT backend.
//!
//! Call site: `main.rs` Phase 6.5 discoverable-partition scan, immediately
//! after the ext4 dispatch loop. When the unified probe returns
//! `FsType::Btrfs`, `BtrfsBackend::new` is called and the resulting
//! `Volume` is pushed onto `extra_volumes`.
//!
//! # Lambutter version pinning
//!
//! Currently a path dep on `~/lambutter-dev` while lambutter is pre-publish.
//! When lambutter v0.3.x publishes to crates.io, this dep flips to
//! `lambutter = { version = "=0.3.x", ... }` — exact-pin, lockstep bump on
//! lambutter releases (same discipline as SDS-2 §11 for ext4-view).
//!
//! # Protocol choice — BlockIO, not DiskIo
//!
//! Identical reasoning to `fs_backend_ext4.rs`: `DiskIo` is not installed
//! on partition handles in plain OVMF; `BlockIO` is. The BlockIO adapter
//! delegates block-alignment to `fs_backend_ext4::compute_aligned_read`,
//! which is the same maths regardless of upper-layer parser.
//!
//! # Security
//!
//! No `unsafe` blocks in this module. lambutter itself denies `unsafe_code`
//! at the crate level — every byte read goes through validated tree-walk
//! + metadata-CRC32C verification before being returned.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::error::Error;

use lambutter::{BlockRead, Btrfs, Error as LbErr, SuperblockReason};
use uefi::{
    boot::{self, ScopedProtocol},
    proto::media::block::BlockIO,
    Handle,
};

use crate::{
    fs_backend::{
        BackendTag, DirEntry, FileKind, FsBackend, FsError, Metadata, Path, PathBuf, Uuid,
    },
    fs_backend_ext4::compute_aligned_read,
    partitions::FsInfo,
};

/// Max symlink-resolution depth before we conclude the chain is a loop or
/// pathologically nested. POSIX `SYMLOOP_MAX` is 8 minimum, Linux uses 40.
/// Pick the more permissive Linux value so legitimate distro layouts
/// (Tumbleweed's /boot/vmlinuz → /boot/vmlinuz-X-Y →
/// /usr/lib/modules/X-Y/vmlinuz is only 2 hops) never trip the limit.
const MAX_SYMLINK_DEPTH: u8 = 40;

/// Backend tag surfaced via `FsBackend::tag()`. Update lockstep with the
/// Cargo dep version per the same discipline as `EXT4_BACKEND_TAG`.
pub(crate) const BTRFS_BACKEND_TAG: BackendTag = "lambutter@0.3.0-path";

/// A mounted btrfs volume. Owns the lambutter reader (which holds the
/// BlockIO protocol) plus probe-time identification metadata.
pub(crate) struct BtrfsBackend {
    fs: Btrfs<BlockIoReader>,
    fs_uuid: Option<Uuid>,
}

impl BtrfsBackend {
    /// Construct a btrfs backend from a block-device handle identified as
    /// btrfs by the unified `probe_superblock`. Ownership of the BlockIO
    /// protocol transfers into the backend for its lifetime — same
    /// rationale as ext4: tree walks are read-frequent and per-call open
    /// would be wasteful.
    pub(crate) fn new(handle: Handle, info: &FsInfo) -> Result<Self, FsError> {
        let block_io = boot::open_protocol_exclusive::<BlockIO>(handle)?;
        let media = block_io.media();
        let media_id = media.media_id();
        let block_size = media.block_size() as u64;
        if block_size == 0 {
            return Err(FsError::Unsupported("block_size=0"));
        }
        let last_block = media.last_block();
        let device_size_bytes = block_size
            .checked_mul(last_block.saturating_add(1))
            .ok_or(FsError::Unsupported("device_size_overflow"))?;

        let reader = BlockIoReader {
            block_io,
            media_id,
            block_size,
        };

        let fs = Btrfs::open(reader, device_size_bytes).map_err(translate_lb_error)?;

        // lambutter does not expose superblock UUID accessor (intentional
        // minimal v0.3.x API). Reuse the UUID the partition prober already
        // extracted from byte offset 0x20 of the superblock; both readers
        // looked at the same bytes so consistency is structural.
        let fs_uuid = info.uuid.as_deref().and_then(parse_btrfs_uuid_string);

        Ok(Self { fs, fs_uuid })
    }
}

impl BtrfsBackend {
    /// Resolve `path` to an inode, transparently following any symlinks
    /// at the final component up to `MAX_SYMLINK_DEPTH` hops.
    ///
    /// lambutter v0.1.x deliberately does NOT follow symlinks during path
    /// resolution — `Btrfs::resolve` returns the inode of the symlink
    /// itself, and `read_file` errors with `NotARegularFile` on a symlink.
    /// That matches the crate's design philosophy ("read_link returns raw
    /// bytes; caller handles resolution") but the FsBackend contract is
    /// POSIX-like (transparent following). This adapter bridges the two.
    ///
    /// Only leaf-component symlinks are followed; intermediate-component
    /// symlinks in the path (e.g. `/boot/<symlink-dir>/foo`) are NOT
    /// resolved. The distro `/boot` layouts we target (Tumbleweed, Fedora,
    /// CachyOS, Garuda) only put symlinks at the kernel-name leaf, so leaf-
    /// only is sufficient for v0.10.0. Full per-component traversal is a
    /// future enhancement when a real layout demands it.
    fn resolve_following(&mut self, path: &Path) -> Result<lambutter::Inode, FsError> {
        let mut current: PathBuf = Path::try_from_str(path.as_str())?;
        for _ in 0..=MAX_SYMLINK_DEPTH {
            let lb = lb_path(&current)?;
            let inode = self.fs.resolve(lb).map_err(translate_lb_error)?;
            let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
            if !md.is_symlink() {
                return Ok(inode);
            }
            let target_bytes = self.fs.read_link(lb).map_err(translate_lb_error)?;
            let target_str = core::str::from_utf8(&target_bytes)
                .map_err(|_| FsError::InvalidPath("non-utf8 symlink target"))?;
            // Absolute target replaces; relative target joins to parent of
            // the current path (the symlink lives in its parent's dir).
            current = if target_str.starts_with('/') {
                Path::try_from_str(target_str)?
            } else {
                current.parent().join(target_str)?
            };
        }
        Err(FsError::Unsupported("symlink_chain_too_deep"))
    }
}

impl FsBackend for BtrfsBackend {
    fn tag(&self) -> BackendTag {
        BTRFS_BACKEND_TAG
    }

    fn uuid(&self) -> Option<Uuid> {
        self.fs_uuid
    }

    fn label(&self) -> Option<&str> {
        // lambutter v0.1.x does not expose the btrfs superblock label
        // field. When lambutter adds a `label()` accessor (v0.2.x
        // candidate), surface it here. Returning None is correct — the
        // FsBackend trait documents this as optional cosmetic metadata.
        None
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        // Follow symlinks (e.g. /boot/vmlinuz → /boot/vmlinuz-X-Y →
        // /usr/lib/modules/X-Y/vmlinuz on openSUSE), then full-file read
        // by inode via read_file_at to skip a redundant path resolution.
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        // Clamp the up-front allocation to the read-contract cap: md.size is
        // read straight from the on-disk inode and is attacker-controlled, so a
        // crafted multi-GiB size must not drive an OOM-abort allocation.
        let size = crate::fs_backend::checked_full_read_len(md.size)?;
        let mut buf = alloc::vec![0u8; size];
        let n = self
            .fs
            .read_file_at(&inode, 0, &mut buf)
            .map_err(translate_lb_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let inode = self.resolve_following(path)?;
        let mut buf = alloc::vec![0u8; len];
        let n = self
            .fs
            .read_file_at(&inode, offset, &mut buf)
            .map_err(translate_lb_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        // exists() does NOT follow symlinks at the final component — the
        // FsBackend contract treats "does this path resolve" as "is there
        // an entry here", consistent with `stat`/`lstat` distinction.
        // Callers that need symlink-follow can call metadata().
        let p = lb_path(path)?;
        match self.fs.resolve(p) {
            Ok(_) => Ok(true),
            Err(LbErr::NotFound) => Ok(false),
            Err(other) => Err(translate_lb_error(other)),
        }
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        // Follow symlinks (POSIX `stat` semantics, not `lstat`).
        let inode = self.resolve_following(path)?;
        let md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        Ok(translate_metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        // Directory paths are rarely symlinks in our target distros, but
        // resolve through symlinks for consistency with metadata() and
        // POSIX `opendir` semantics.
        let inode = self.resolve_following(path)?;
        let dir_md = self.fs.metadata(&inode).map_err(translate_lb_error)?;
        if !dir_md.is_dir() {
            return Err(FsError::NotDirectory);
        }
        // lambutter's read_dir() takes a path, not an inode. We could
        // re-walk via path here, but the inode we already have is the
        // canonical post-symlink target, so call read_dir on the resolved
        // path. For the no-symlink common case this is the same path.
        let p = lb_path(path)?;
        let entries = self.fs.read_dir(p).map_err(translate_lb_error)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = core::str::from_utf8(&entry.name)
                .map_err(|_| FsError::InvalidPath("non-utf8 btrfs filename"))?;
            if name == "." || name == ".." {
                continue;
            }
            // Per-entry metadata to fill DirEntry.size + canonical kind. The
            // objectid comes from the directory entry itself, so this metadata
            // is ADVISORY — it reflects the name→inode binding the directory
            // claims, not one re-resolved by name. Callers must re-resolve via
            // read()/metadata() before any security decision; DirEntry.size is
            // a display/hint value only.
            //
            // A metadata failure on one entry must NOT abort the whole listing
            // (an attacker could otherwise craft one bad entry to hide its
            // siblings). On error, surface the entry with unknown size/kind
            // rather than dropping the listing.
            let inode = lambutter::Inode {
                objectid: entry.inode_number,
            };
            let (kind, size) = match self.fs.metadata(&inode) {
                Ok(md) => (translate_metadata(&md).kind, md.size),
                Err(_) => (FileKind::Other, 0),
            };
            out.push(DirEntry {
                name: String::from(name),
                kind,
                size,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// BlockIoReader — the lambutter::BlockRead adapter
// ---------------------------------------------------------------------------

/// Wraps a `ScopedProtocol<BlockIO>` so lambutter can pull arbitrary byte
/// ranges from a UEFI block device. Delegates block-alignment maths to
/// `fs_backend_ext4::compute_aligned_read` — identical logic regardless
/// of upper-layer filesystem.
struct BlockIoReader {
    block_io: ScopedProtocol<BlockIO>,
    media_id: u32,
    block_size: u64,
}

impl BlockRead for BlockIoReader {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn read_at(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.is_empty() {
            return Ok(());
        }
        let (first_lba, aligned_len, intra) =
            compute_aligned_read(offset_bytes, buf.len(), self.block_size)
                .ok_or_else(|| Box::<dyn Error + Send + Sync>::from("read overflow"))?;

        let mut aligned = vec![0u8; aligned_len];
        self.block_io
            .read_blocks(self.media_id, first_lba, &mut aligned)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync + 'static>)?;

        buf.copy_from_slice(&aligned[intra..intra + buf.len()]);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Translate a LamBoot `&Path` to a `lambutter::Path`. Both types use
/// UTF-8 + forward-slash + absolute conventions; lambutter additionally
/// validates absolute-ness at construction.
pub(crate) fn lb_path(path: &Path) -> Result<lambutter::Path<'_>, FsError> {
    lambutter::Path::new(path.as_str().as_bytes()).ok_or(FsError::InvalidPath(
        "lambutter::Path rejected (not absolute?)",
    ))
}

/// Translate `lambutter::Error` into `FsError`. Stable tokens go through
/// FsError's `as_log_token()` for trust-log emission.
#[expect(
    clippy::needless_pass_by_value,
    reason = "called as a function pointer via .map_err; LbErr is a small enum \
              and the conversion semantically consumes it at the FsBackend boundary"
)]
pub(crate) fn translate_lb_error(err: LbErr) -> FsError {
    match err {
        LbErr::NotFound => FsError::NotFound,
        LbErr::NotARegularFile => FsError::IsDirectory,
        LbErr::NotASymlink => FsError::Unsupported("not_a_symlink"),
        LbErr::BadSuperblock(reason) => match reason {
            SuperblockReason::BadMagic => FsError::Corrupt("btrfs bad superblock magic"),
            SuperblockReason::BadCsum => FsError::Corrupt("btrfs superblock csum mismatch"),
            SuperblockReason::GenerationSkew => {
                FsError::Corrupt("btrfs superblock generation skew")
            }
            SuperblockReason::NoValidCopy => FsError::Corrupt("btrfs no valid superblock copy"),
            SuperblockReason::UnsupportedCsumType => FsError::UnsupportedFeature {
                feature: "btrfs_csum_type",
                backend: BTRFS_BACKEND_TAG,
            },
            SuperblockReason::UnsupportedIncompat => FsError::UnsupportedFeature {
                feature: "btrfs_incompat_flag",
                backend: BTRFS_BACKEND_TAG,
            },
            SuperblockReason::BadGeometry => FsError::Corrupt("btrfs bad nodesize/sectorsize"),
        },
        LbErr::UnsupportedFeature(_) => FsError::UnsupportedFeature {
            feature: "btrfs_incompat_flag",
            backend: BTRFS_BACKEND_TAG,
        },
        LbErr::UnsupportedProfile(_) => FsError::UnsupportedFeature {
            feature: "btrfs_raid_profile",
            backend: BTRFS_BACKEND_TAG,
        },
        LbErr::UnsupportedChecksum(_) => FsError::UnsupportedFeature {
            feature: "btrfs_csum_type",
            backend: BTRFS_BACKEND_TAG,
        },
        LbErr::CsumMismatch { .. } => FsError::Corrupt("btrfs metadata block csum mismatch"),
        LbErr::CorruptBtree { .. } => FsError::Corrupt("btrfs btree structural violation"),
        LbErr::BadCompression { .. } => FsError::Corrupt("btrfs decompression failed"),
        LbErr::OutOfMemory { .. } => FsError::Unsupported("out_of_memory"),
        // lambutter v0.1.x does not impl `core::error::Error` for its
        // `Error` type (its `std::error::Error` impl is gated on `std`,
        // which we don't have in this UEFI build). The Io variant
        // carries a stable `&'static str` token from lambutter; surface
        // it as `Corrupt` since a btrfs-volume read failure that
        // surfaced through lambutter is, from lamboot's perspective,
        // unreadable on-disk data regardless of root cause.
        LbErr::Io { token, .. } => FsError::Corrupt(token),
    }
}

pub(crate) fn translate_metadata(md: &lambutter::Metadata) -> Metadata {
    let kind = if md.is_file() {
        FileKind::Regular
    } else if md.is_dir() {
        FileKind::Directory
    } else if md.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    Metadata {
        kind,
        size: md.size,
        mode: md.mode,
        // Btrfs owner-write bit: mode & 0o200 set ⇒ writable by owner.
        // Matches the ext4 backend's interpretation; the backend itself
        // is read-only regardless of the underlying bits.
        read_only: (md.mode & 0o200) == 0,
        source_backend: BTRFS_BACKEND_TAG,
    }
}

/// Parse the partition prober's hex-formatted btrfs UUID string (16 bytes
/// rendered as the canonical 8-4-4-4-12 form) back to bytes. Returns
/// `None` on any format violation — we'd rather omit a UUID than report
/// a wrong one.
pub(crate) fn parse_btrfs_uuid_string(s: &str) -> Option<Uuid> {
    let mut bytes = [0u8; 16];
    let mut i = 0usize;
    for chunk in s.as_bytes().split(|&b| b == b'-') {
        for pair in chunk.chunks(2) {
            if pair.len() != 2 || i >= 16 {
                return None;
            }
            let hi = hex_nibble(pair[0])?;
            let lo = hex_nibble(pair[1])?;
            bytes[i] = (hi << 4) | lo;
            i += 1;
        }
    }
    if i != 16 {
        return None;
    }
    Some(Uuid::from_bytes(bytes))
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Host-side tests live in `lamboot-fs-tests`. This module has no testable
// surface in isolation — every entry point requires a UEFI `BlockIO`
// handle. The pure helpers above (`parse_btrfs_uuid_string`, `hex_nibble`,
// `translate_lb_error`, `translate_metadata`) are candidates for
// host-side coverage in a follow-up; for v0.10.0 entry, the live VM-102
// validation we ran on `examples/inspect.rs` is the proof.
// ---------------------------------------------------------------------------
