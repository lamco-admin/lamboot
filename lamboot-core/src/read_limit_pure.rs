//! Layer: 2 (pure) — Storage read-size guard.
//!
//! The single full-file-read allocation cap and its bounds check, split out so
//! `lamboot-fs-tests` can include it verbatim via `#[path]` and exercise the
//! boundary (0, 1, the documented worst-case initrd, the exact cap, cap+1,
//! multi-GiB, `u64::MAX`) on the host. `fs_backend::checked_full_read_len`
//! delegates here and maps [`TooLarge`] into its `FsError::FileTooLarge`, so the
//! cap value and the over-read rejection live — and are tested — in one place.
//!
//! Cap rationale (the FsBackend hardening boundary): a backend `read()` sizes a
//! `vec![0u8; n]` from on-disk/firmware metadata (an inode file-size field, a
//! FAT directory entry) that is attacker-controlled on a probed volume. An
//! unvalidated multi-GiB size would abort the boot against bounded UEFI
//! boot-services memory (a denial-of-boot). This is DELIBERATELY decoupled from
//! `pe_loader_pure::MAX_IMAGE_SIZE` (256 MiB, which bounds a PE image we
//! RELOCATE): an **initrd** is gated ONLY by this cap — handed straight to the
//! kernel via `InitrdHandle::register`, never the PE loader. The original
//! 256 MiB value was merely mirrored from `MAX_IMAGE_SIZE` and rejected real
//! enterprise/HPC initrds (driver + firmware blobs, ~300–400 MiB) outright →
//! unbootable with no fallback. 512 MiB clears that real-world maximum with
//! headroom while still capping a hostile multi-GiB size field.

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

/// Upper bound on a single full-file read allocation. See the module rationale.
pub const MAX_BOOT_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// A metadata-reported size that exceeded [`MAX_BOOT_FILE_BYTES`] or did not fit
/// `usize` (a 32-bit build). Carries the offending size and the cap for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLarge {
    pub size: u64,
    pub max: u64,
}

/// Validate a metadata-reported file size for a full-file read allocation.
///
/// `Ok(size as usize)` when `size <= MAX_BOOT_FILE_BYTES` (the cap is
/// inclusive), else `Err(TooLarge)`. The `try_from` arm makes a 32-bit build
/// fail loudly instead of truncating an `as usize` cast into a small allocation
/// and a subsequent under-read.
pub fn checked_read_len(size: u64) -> Result<usize, TooLarge> {
    if size > MAX_BOOT_FILE_BYTES {
        return Err(TooLarge {
            size,
            max: MAX_BOOT_FILE_BYTES,
        });
    }
    usize::try_from(size).map_err(|_| TooLarge {
        size,
        max: MAX_BOOT_FILE_BYTES,
    })
}
