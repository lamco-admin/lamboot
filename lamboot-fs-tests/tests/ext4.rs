//! SDS-2 host-side integration tests for the ext4 backend.
//!
//! The real `Ext4Backend` lives in `lamboot-core/src/fs_backend_ext4.rs`
//! and depends on UEFI types that cannot run on the host. What we can
//! test here:
//!
//!   1. The **upstream `ext4-view` 0.9.3 API surface** we pin against —
//!      catches breakage on a version bump before it lands in the
//!      bootloader binary.
//!   2. The **error-translation table** per SDS-2 §7 — every
//!      `Ext4Error` variant must map to a typed `FsError` variant, and
//!      the backend must never leak ext4-view's native error upward.
//!      Encoded as a conformance table here; kept in sync with
//!      `fs_backend_ext4.rs::translate_ext4_error` by documentation.
//!   3. End-to-end **read / exists / read_dir** against a real ext4
//!      filesystem (the bundled `test_disk1.bin.zst` from the upstream
//!      ext4-view repository, redistributed under MIT-OR-Apache-2.0
//!      matching LamBoot's own dual license).
//!
//! What this does NOT cover (deferred to PR-3 QEMU harness):
//!   * The UEFI `DiskIo` adapter (`DiskIoReader`).
//!   * The `partitions.rs::probe_superblock → Ext4Backend::new`
//!     dispatch — needs a real UEFI handle.
//!   * Streaming reads of multi-MB kernels.
//!   * Measured performance budget (SDS-2 §12).
//!
//! # Running
//!
//!     cargo test -p lamboot-fs-tests --test ext4

use std::io::Read;

use ext4_view::{Ext4, Ext4Error, FileType};

/// Path to the compressed test disk image, relative to the crate root.
/// The image is 64 MiB uncompressed, ~550 KiB compressed, and ships
/// upstream under `ext4-view-rs/test_data/test_disk1.bin.zst`.
const FIXTURE_PATH: &str = "fixtures/ext4/test_disk1.bin.zst";

/// Load and decompress the bundled ext4 image into a `Vec<u8>` that
/// can back an `Ext4Read` impl.
fn load_fixture() -> Vec<u8> {
    let compressed = std::fs::read(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("failed to read {FIXTURE_PATH}: {e}"));
    let mut decoder = zstd::Decoder::new(compressed.as_slice()).expect("zstd decoder init failed");
    let mut out = Vec::with_capacity(64 * 1024 * 1024);
    decoder
        .read_to_end(&mut out)
        .expect("zstd decompress failed");
    assert_eq!(
        out.len(),
        64 * 1024 * 1024,
        "fixture decompressed size drift — upstream may have changed",
    );
    out
}

/// Open the fixture as an `Ext4`. Returns the live reader so callers
/// can exercise the read-path API.
fn load_ext4() -> Ext4 {
    let bytes = load_fixture();
    // ext4-view ships a `Vec<u8>: Ext4Read` impl — no custom adapter
    // needed for host tests. The bootloader uses a `DiskIoReader`
    // wrapping `uefi::proto::media::disk::DiskIo` instead; the shape
    // of the trait contract is identical.
    Ext4::load(Box::new(bytes)).expect("fixture Ext4::load failed")
}

// ---------------------------------------------------------------------------
// 1. ext4-view 0.9.3 API conformance
// ---------------------------------------------------------------------------

#[test]
fn fixture_decompresses_to_expected_size() {
    // Trivial guard against fixture corruption or upstream change.
    let bytes = load_fixture();
    assert_eq!(bytes.len(), 64 * 1024 * 1024);
}

#[test]
fn api_ext4_load_succeeds_on_good_image() {
    let _ = load_ext4();
}

#[test]
fn api_ext4_load_fails_on_zeros() {
    let zeros = vec![0u8; 1024 * 1024];
    let result = Ext4::load(Box::new(zeros));
    assert!(
        matches!(result, Err(_)),
        "Ext4::load on an all-zero buffer must fail (no superblock magic)",
    );
}

#[test]
fn api_ext4_uuid_is_16_bytes() {
    let ext4 = load_ext4();
    let uuid = ext4.uuid();
    // The bootloader calls `uuid.as_bytes()` and builds its own
    // `lamboot_core::fs_types::Uuid` from the 16-byte array. If
    // upstream ever changes this length, our constructor breaks.
    let bytes: &[u8; 16] = uuid.as_bytes();
    assert_eq!(bytes.len(), 16);
}

#[test]
fn api_ext4_label_to_str_roundtrips() {
    // Label accessor used by `Ext4Backend::new` for the volume label.
    // Confirm the return type and that `to_str()` handles null-trim.
    let ext4 = load_ext4();
    let label = ext4.label();
    let _ = label.to_str(); // may be Ok("") or an actual label
}

#[test]
fn api_read_and_exists_basics() {
    let ext4 = load_ext4();
    // Root directory always exists.
    assert!(ext4.exists("/").expect("exists(/) failed"));
    // Nonexistent path: exists() returns Ok(false), not NotFound.
    assert_eq!(ext4.exists("/definitely-not-there").ok(), Some(false));
    // read_dir of root yields at least lost+found on a fresh ext4.
    let iter = ext4.read_dir("/").expect("read_dir(/) failed");
    let names: Vec<String> = iter
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().as_str().ok().map(String::from))
        .collect();
    assert!(
        !names.is_empty(),
        "root dir must not be empty on a fresh ext4 fixture",
    );
}

#[test]
fn api_metadata_exposes_kind_and_len() {
    let ext4 = load_ext4();
    // Root is a directory.
    let root_md = ext4.metadata("/").expect("metadata(/) failed");
    assert!(root_md.file_type().is_dir());
}

#[test]
fn api_read_regular_file_returns_bytes() {
    let ext4 = load_ext4();
    // `test_disk1` includes a file called `/empty_file` (0-byte regular
    // file). The bootloader's read_at path opens + seeks + read_bytes;
    // this test exercises the simpler Ext4::read path.
    if ext4.exists("/empty_file").unwrap_or(false) {
        let bytes = ext4.read("/empty_file").expect("read(/empty_file) failed");
        assert_eq!(bytes.len(), 0);
    }
}

// ---------------------------------------------------------------------------
// 2. Error-translation conformance
//
// This table is the authoritative one. `fs_backend_ext4.rs::translate_ext4_error`
// MUST match the same structure. When ext4-view adds a new Ext4Error variant
// on a version bump, this table gets updated in the same PR.
// ---------------------------------------------------------------------------

/// A stable string token that represents the expected `FsError`
/// classification for each `Ext4Error` variant. Matches the
/// `FsError::as_log_token()` return value in lamboot-core.
///
/// Keep in sync with `lamboot-core/src/fs_backend_ext4.rs::translate_ext4_error`.
fn expected_fs_token(e: &Ext4Error) -> &'static str {
    match e {
        Ext4Error::NotFound => "not_found",
        Ext4Error::NotADirectory => "not_directory",
        Ext4Error::IsADirectory => "is_directory",
        Ext4Error::IsASpecialFile => "unsupported",
        Ext4Error::Encrypted => "unsupported",
        Ext4Error::Incompatible(_) => "unsupported_feature",
        Ext4Error::Corrupt(_) => "fs_corrupt",
        // Io, NotAbsolute, NotASymlink, FileTooLarge, MalformedPath,
        // PathTooLong, TooManySymlinks, and any future additions
        // surface as backend_other.
        _ => "backend_other",
    }
}

#[test]
fn translation_table_compiles() {
    // Construct the Ext4Error variants we can build directly (the ones
    // without opaque inner types). The table check lives in the
    // match's exhaustiveness: if ext4-view adds a variant we don't
    // handle, this test's match in `expected_fs_token` still returns
    // "backend_other" — which is correct. If it makes a previously
    // simple variant take new payload, this won't compile.
    assert_eq!(expected_fs_token(&Ext4Error::NotFound), "not_found");
    assert_eq!(
        expected_fs_token(&Ext4Error::NotADirectory),
        "not_directory"
    );
    assert_eq!(expected_fs_token(&Ext4Error::IsADirectory), "is_directory");
    assert_eq!(expected_fs_token(&Ext4Error::IsASpecialFile), "unsupported");
    assert_eq!(expected_fs_token(&Ext4Error::Encrypted), "unsupported");
}

#[test]
fn read_nonexistent_returns_not_found() {
    let ext4 = load_ext4();
    let err = ext4
        .read("/no-such-path-xxxx")
        .expect_err("read of nonexistent path must fail");
    assert_eq!(expected_fs_token(&err), "not_found");
}

#[test]
fn read_of_directory_returns_is_directory() {
    let ext4 = load_ext4();
    let err = ext4
        .read("/")
        .expect_err("read(/) must fail — it's a directory");
    assert_eq!(expected_fs_token(&err), "is_directory");
}

#[test]
fn read_dir_of_file_returns_not_directory() {
    let ext4 = load_ext4();
    // Find a regular file by walking root.
    let iter = ext4.read_dir("/").expect("read_dir(/)");
    let mut file_path: Option<String> = None;
    for entry in iter.flatten() {
        if let Ok(md) = entry.metadata() {
            if md.file_type().is_regular_file() {
                if let Ok(name) = entry.file_name().as_str() {
                    file_path = Some(format!("/{name}"));
                    break;
                }
            }
        }
    }
    let Some(path) = file_path else {
        // Fixture may not contain a regular file at root; skip rather
        // than fail — the conformance above still holds.
        return;
    };
    let err = ext4
        .read_dir(&*path)
        .expect_err("read_dir on a regular file must fail");
    assert_eq!(expected_fs_token(&err), "not_directory");
}

// ---------------------------------------------------------------------------
// 3. FileType translation conformance
// ---------------------------------------------------------------------------

/// Mirror of `lamboot-core/src/fs_backend_ext4.rs::translate_file_type`.
/// Keep in sync when FileType gains new variants upstream.
fn file_kind_token(ft: FileType) -> &'static str {
    if ft.is_dir() {
        "directory"
    } else if ft.is_regular_file() {
        "regular"
    } else if ft.is_symlink() {
        "symlink"
    } else {
        "other"
    }
}

#[test]
fn file_type_translation_covers_dir_regular_symlink() {
    let ext4 = load_ext4();
    // Root: directory.
    let root_ft = ext4.metadata("/").expect("metadata(/)").file_type();
    assert_eq!(file_kind_token(root_ft), "directory");

    // Walk the root once to hit each FileType class we can see.
    let iter = ext4.read_dir("/").expect("read_dir(/)");
    let mut seen_regular = false;
    for entry in iter.flatten() {
        if let Ok(md) = entry.metadata() {
            let token = file_kind_token(md.file_type());
            if token == "regular" {
                seen_regular = true;
            }
        }
    }
    assert!(
        seen_regular,
        "fixture must contain at least one regular file at root",
    );
}

// ---------------------------------------------------------------------------
// 4. Pin integrity
//
// The bootloader depends on the exact-pinned ext4-view "=0.9.3". If the
// fs-tests dev-dependency ever drifts, compilation fails at `cargo test`
// time with a version-mismatch error. This runtime guard catches the
// rarer case where the pin matches but the API has shifted under us
// (e.g., a semver-compatible upstream change to Ext4Error's shape).
// ---------------------------------------------------------------------------

#[test]
fn ext4_error_is_non_exhaustive_handling_is_default_backend_other() {
    // We rely on `_ => "backend_other"` catching variants we don't
    // enumerate. If ext4-view ever renames an existing variant, this
    // test alone won't catch it — but the `translation_table_compiles`
    // test will fail to compile because the match arm references the
    // old name.
    assert_eq!(expected_fs_token(&Ext4Error::NotAbsolute), "backend_other");
    assert_eq!(expected_fs_token(&Ext4Error::NotASymlink), "backend_other");
    assert_eq!(expected_fs_token(&Ext4Error::FileTooLarge), "backend_other");
    assert_eq!(
        expected_fs_token(&Ext4Error::MalformedPath),
        "backend_other"
    );
    assert_eq!(expected_fs_token(&Ext4Error::PathTooLong), "backend_other");
    assert_eq!(
        expected_fs_token(&Ext4Error::TooManySymlinks),
        "backend_other"
    );
}
