//! Host unit tests for the full-file read-size guard.
//!
//! The module lives at `lamboot-core/src/read_limit_pure.rs` and is included
//! verbatim via `#[path]`. `fs_backend::checked_full_read_len` delegates here
//! and maps `TooLarge` into `FsError::FileTooLarge`, so this is the
//! authoritative test of the cap value and the over-read rejection that every
//! backend `read()` funnels through before `vec![0u8; n]`.
//!
//! Dual contract: flexibility — a legitimately empty file, a 1-byte read, and a
//! realistic ~400 MiB enterprise/HPC initrd (the regression the cap was raised
//! to 512 MiB for) must all be accepted, and the exact cap is inclusive;
//! security — cap+1, a multi-GiB hostile inode size, and `u64::MAX` must all be
//! rejected before allocation (and a 32-bit build must reject `u64::MAX` loudly
//! rather than truncate via `as usize`).

use lamboot_fs_tests::read_limit_pure::{checked_read_len, TooLarge, MAX_BOOT_FILE_BYTES};

#[test]
fn cap_is_exactly_512_mib() {
    // The cap was deliberately raised from 256 MiB; pin the value so a refactor
    // tightening it back (and re-breaking large initrds) fails here.
    assert_eq!(MAX_BOOT_FILE_BYTES, 512 * 1024 * 1024);
    assert_eq!(MAX_BOOT_FILE_BYTES, 536_870_912);
}

#[test]
fn zero_bytes_ok() {
    assert_eq!(checked_read_len(0), Ok(0));
}

#[test]
fn one_byte_ok() {
    assert_eq!(checked_read_len(1), Ok(1));
}

#[test]
fn initrd_400_mib_ok() {
    let size = 400u64 * 1024 * 1024;
    assert_eq!(checked_read_len(size), Ok(size as usize));
}

#[test]
fn exact_cap_ok_inclusive() {
    let size = MAX_BOOT_FILE_BYTES;
    assert_eq!(checked_read_len(size), Ok(size as usize));
}

#[test]
fn cap_plus_one_too_large() {
    let size = MAX_BOOT_FILE_BYTES + 1;
    assert_eq!(
        checked_read_len(size),
        Err(TooLarge {
            size,
            max: MAX_BOOT_FILE_BYTES
        })
    );
}

#[test]
fn multi_gib_hostile_size_too_large() {
    let size = 8u64 * 1024 * 1024 * 1024; // 8 GiB crafted inode/dir-entry size
    assert_eq!(
        checked_read_len(size),
        Err(TooLarge {
            size,
            max: MAX_BOOT_FILE_BYTES
        })
    );
}

#[test]
fn u64_max_too_large_no_silent_truncation() {
    assert_eq!(
        checked_read_len(u64::MAX),
        Err(TooLarge {
            size: u64::MAX,
            max: MAX_BOOT_FILE_BYTES
        })
    );
}
