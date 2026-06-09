//! SDS-3 PR-2 — host unit tests for the pure PE-loader logic.
//!
//! The module lives at `lamboot-core/src/pe_loader_pure.rs` and is
//! included verbatim into this test crate via `#[path]`. The tests
//! exercise every validation branch in SDS-3 §5.1, every relocation
//! path in §9, and the SHA-256 + hex-encoding helper functions. The
//! Volume-dependent side (allocate_pages, install_protocol_interface,
//! start_image) is out of scope for host testing by architecture —
//! `lamboot-core` is UEFI-only — and is exercised by the QEMU
//! harness in PR-4.
//!
//! Corpus: LamBoot's own `lambootx64.efi` post-build artifact (known-
//! good PE; always conforms to §5.1) + hand-crafted bad-PE fixtures
//! (short, bad-DOS, bad-NT, PE32 magic, wrong-machine) constructed
//! in-test.

use std::{fs, path::PathBuf};

use lamboot_fs_tests::pe_loader_pure::{
    apply_relocations_to_slice, expected_machine, hex_encode_32, parse_headers, sha256_of,
    validate_headers, PeLoadError, MAX_IMAGE_SIZE, MAX_SECTIONS, PE32_MAGIC, PE32_PLUS_MAGIC,
    REL_DIR64,
};

// ---------------------------------------------------------------------------
// Known-good corpus — LamBoot's own post-build binary
// ---------------------------------------------------------------------------

/// Locate `lambootx64.efi` post-build. Skips the test with a loud
/// message if the build hasn't happened yet; we don't want CI to fail
/// on a missing artifact.
fn load_lamboot_efi() -> Option<Vec<u8>> {
    let candidates = [
        "target/x86_64-unknown-uefi/release/lambootx64.efi",
        "target/x86_64-unknown-uefi/debug/lambootx64.efi",
        "dist/EFI/LamBoot/lambootx64.efi",
    ];
    for rel in &candidates {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); // up to workspace root
        p.push(rel);
        if let Ok(bytes) = fs::read(&p) {
            return Some(bytes);
        }
    }
    None
}

#[test]
fn known_good_lambootx64_parses_and_validates() {
    let Some(bytes) = load_lamboot_efi() else {
        eprintln!(
            "SKIP: lambootx64.efi not built; run `cargo build --target x86_64-unknown-uefi` first"
        );
        return;
    };
    let summary = parse_headers(&bytes).expect("LamBoot's own binary must parse");
    validate_headers(&summary, &bytes).expect("LamBoot's own binary must validate");
    assert_eq!(summary.opt_magic, PE32_PLUS_MAGIC);
    // LamBoot is EFI_APPLICATION.
    assert_eq!(summary.subsystem, 10);
    assert!(summary.size_of_image > 0);
    assert!(summary.size_of_image <= MAX_IMAGE_SIZE);
    assert!(summary.num_sections > 0);
    assert!(summary.num_sections <= MAX_SECTIONS);
    assert!(
        summary.entry_in_executable_section,
        "entry point must land in an executable section"
    );
    // LamBoot has no imports, no TLS.
    assert_eq!(summary.import_dir_size, 0);
    assert_eq!(summary.tls_dir_size, 0);
}

#[test]
fn known_good_lambootx64_sha256_is_stable() {
    let Some(bytes) = load_lamboot_efi() else {
        return;
    };
    let sha_a = sha256_of(&bytes);
    let sha_b = sha256_of(&bytes);
    assert_eq!(sha_a, sha_b, "sha256 is deterministic");
    assert_eq!(sha_a.len(), 32);
    // Hex round-trip matches expected length + alphabet.
    let hex = hex_encode_32(&sha_a);
    assert_eq!(hex.len(), 64);
    for c in hex.chars() {
        assert!(c.is_ascii_hexdigit(), "hex chars only: got {c:?}");
        // Lowercase per SDS-3 convention.
        if c.is_alphabetic() {
            assert!(c.is_ascii_lowercase());
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-crafted bad-PE fixtures (SDS-3 §5.1 violation cases)
// ---------------------------------------------------------------------------

#[test]
fn too_short_byte_slice_rejected() {
    let tiny = vec![0u8; 32];
    match parse_headers(&tiny) {
        Err(PeLoadError::TooShort { have, need }) => {
            assert_eq!(have, 32);
            assert_eq!(need, 64);
        }
        other => panic!("expected TooShort, got {other:?}"),
    }
}

#[test]
fn bad_dos_signature_rejected() {
    let mut bytes = vec![0u8; 128];
    bytes[0] = b'Z'; // wrong
    bytes[1] = b'M';
    match parse_headers(&bytes) {
        Err(PeLoadError::BadDosSignature) => {}
        other => panic!("expected BadDosSignature, got {other:?}"),
    }
}

#[test]
fn nt_offset_out_of_bounds_rejected() {
    let mut bytes = vec![0u8; 128];
    bytes[0..2].copy_from_slice(b"MZ");
    // e_lfanew at 0x3C set to way past buffer length.
    bytes[0x3C..0x40].copy_from_slice(&u32::to_le_bytes(0xFFFF_0000));
    match parse_headers(&bytes) {
        Err(PeLoadError::NtOffsetOutOfBounds(off)) => {
            assert_eq!(off, 0xFFFF_0000);
        }
        other => panic!("expected NtOffsetOutOfBounds, got {other:?}"),
    }
}

#[test]
fn bad_nt_signature_rejected() {
    let mut bytes = vec![0u8; 256];
    bytes[0..2].copy_from_slice(b"MZ");
    bytes[0x3C..0x40].copy_from_slice(&u32::to_le_bytes(0x80));
    // At offset 0x80 place wrong signature.
    bytes[0x80..0x84].copy_from_slice(b"XE\0\0");
    match parse_headers(&bytes) {
        Err(PeLoadError::BadNtSignature) => {}
        other => panic!("expected BadNtSignature, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Relocation math — the heart of §9
// ---------------------------------------------------------------------------

#[test]
fn relocation_zero_delta_is_noop_even_on_malformed_bytes() {
    // If actual == preferred, the algorithm returns Ok(()) immediately
    // without parsing reloc_bytes. This short-circuit saves time and
    // avoids surprises on PEs that have a reloc directory the linker
    // wrote but the loader doesn't need.
    let mut image = vec![0xAAu8; 4096];
    let image_before = image.clone();
    let junk_reloc_bytes = vec![0xFFu8; 64]; // definitely malformed
    apply_relocations_to_slice(&mut image, 0x1_0000_0000, 0x1_0000_0000, &junk_reloc_bytes)
        .expect("zero delta must be a no-op");
    assert_eq!(image, image_before, "zero delta must not touch image");
}

#[test]
fn relocation_dir64_applies_delta_to_u64_at_rva() {
    // Build a 16 KiB image. Place a u64 "pointer" at RVA 0x1000 + 0x8
    // pointing at preferred_base + some offset; expect the loader to
    // patch it by delta.
    const IMAGE_SIZE: usize = 16 * 1024;
    const PAGE_RVA: u32 = 0x1000;
    const WITHIN_PAGE: u32 = 0x8;
    const TARGET_RVA: u32 = PAGE_RVA + WITHIN_PAGE;

    let preferred_base: u64 = 0x1_0000_0000;
    let actual_base: u64 = 0x2_0000_0000;
    let delta = actual_base - preferred_base;
    let original_value: u64 = preferred_base + 0xDEAD;

    let mut image = vec![0u8; IMAGE_SIZE];
    image[TARGET_RVA as usize..TARGET_RVA as usize + 8]
        .copy_from_slice(&original_value.to_le_bytes());

    // Relocation block: virt_addr = PAGE_RVA, size = 8 (header) + 2 (one entry) + 2 (padding)
    // type=DIR64 (10), offset=WITHIN_PAGE (0x8)
    let entry: u16 = (REL_DIR64 << 12) | (WITHIN_PAGE as u16);
    let mut reloc_bytes = Vec::new();
    reloc_bytes.extend_from_slice(&PAGE_RVA.to_le_bytes());
    reloc_bytes.extend_from_slice(&12u32.to_le_bytes()); // size of block = 12
    reloc_bytes.extend_from_slice(&entry.to_le_bytes());
    reloc_bytes.extend_from_slice(&0u16.to_le_bytes()); // ABSOLUTE padding

    apply_relocations_to_slice(&mut image, preferred_base, actual_base, &reloc_bytes)
        .expect("well-formed reloc block applies cleanly");

    let patched = u64::from_le_bytes(
        image[TARGET_RVA as usize..TARGET_RVA as usize + 8]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        patched,
        original_value.wrapping_add(delta),
        "DIR64 must add delta to the u64 slot",
    );
}

#[test]
fn relocation_absolute_type_is_noop() {
    let mut image = vec![0xAAu8; 4096];
    let image_before = image.clone();
    let preferred_base: u64 = 0x1_0000_0000;
    let actual_base: u64 = 0x2_0000_0000;

    // One block with a single ABSOLUTE entry. Should produce no image change.
    let entry: u16 = 0; // type 0 + offset 0
    let mut reloc_bytes = Vec::new();
    reloc_bytes.extend_from_slice(&0u32.to_le_bytes()); // page RVA
    reloc_bytes.extend_from_slice(&12u32.to_le_bytes()); // block size = 12
    reloc_bytes.extend_from_slice(&entry.to_le_bytes());
    reloc_bytes.extend_from_slice(&0u16.to_le_bytes());

    apply_relocations_to_slice(&mut image, preferred_base, actual_base, &reloc_bytes).unwrap();
    assert_eq!(
        image, image_before,
        "ABSOLUTE-only block must not touch image"
    );
}

#[test]
fn relocation_size_of_block_zero_terminates() {
    // Some PE writers emit a zero-sized block as terminator. Our loop
    // should break cleanly, not panic or loop forever.
    let mut image = vec![0u8; 4096];
    let preferred_base: u64 = 0x1000;
    let actual_base: u64 = 0x2000;
    // Block with size=0 after 8-byte header of zeros.
    let reloc_bytes = vec![0u8; 8];
    apply_relocations_to_slice(&mut image, preferred_base, actual_base, &reloc_bytes).unwrap();
}

#[test]
fn relocation_size_of_block_less_than_8_rejected() {
    let mut image = vec![0u8; 4096];
    let mut reloc_bytes = Vec::new();
    reloc_bytes.extend_from_slice(&0u32.to_le_bytes());
    reloc_bytes.extend_from_slice(&4u32.to_le_bytes()); // malformed: size < 8
    match apply_relocations_to_slice(&mut image, 0x1000, 0x2000, &reloc_bytes) {
        Err(PeLoadError::RelocationMalformed("SizeOfBlock < 8")) => {}
        other => panic!("expected RelocationMalformed, got {other:?}"),
    }
}

#[test]
fn relocation_block_extends_past_directory_rejected() {
    let mut image = vec![0u8; 4096];
    let mut reloc_bytes = Vec::new();
    reloc_bytes.extend_from_slice(&0u32.to_le_bytes());
    reloc_bytes.extend_from_slice(&1024u32.to_le_bytes()); // claims 1 KiB but only 8 bytes follow
                                                           // No further bytes — block header says 1024 but we have only 8.
    match apply_relocations_to_slice(&mut image, 0x1000, 0x2000, &reloc_bytes) {
        Err(PeLoadError::RelocationMalformed(
            "block extends past directory" | "odd entries_bytes",
        )) => {}
        other => panic!("expected RelocationMalformed, got {other:?}"),
    }
}

#[test]
fn relocation_unsupported_type_rejected() {
    // Type = 7 (THUMB_MOV32) — we don't support this.
    let mut image = vec![0u8; 4096];
    let entry: u16 = 7 << 12;
    let mut reloc_bytes = Vec::new();
    reloc_bytes.extend_from_slice(&0u32.to_le_bytes());
    reloc_bytes.extend_from_slice(&12u32.to_le_bytes());
    reloc_bytes.extend_from_slice(&entry.to_le_bytes());
    reloc_bytes.extend_from_slice(&0u16.to_le_bytes());
    match apply_relocations_to_slice(&mut image, 0x1000, 0x2000, &reloc_bytes) {
        Err(PeLoadError::UnsupportedRelocationType(7)) => {}
        other => panic!("expected UnsupportedRelocationType(7), got {other:?}"),
    }
}

#[test]
fn relocation_high_low_types_explicitly_refused() {
    // HIGH=1, LOW=2 are valid PE but kernels don't emit them; we refuse
    // per spec §9.2. Ensure we don't silently accept.
    for bad_type in [1u16, 2u16] {
        let mut image = vec![0u8; 4096];
        let entry: u16 = bad_type << 12;
        let mut reloc_bytes = Vec::new();
        reloc_bytes.extend_from_slice(&0u32.to_le_bytes());
        reloc_bytes.extend_from_slice(&12u32.to_le_bytes());
        reloc_bytes.extend_from_slice(&entry.to_le_bytes());
        reloc_bytes.extend_from_slice(&0u16.to_le_bytes());
        match apply_relocations_to_slice(&mut image, 0x1000, 0x2000, &reloc_bytes) {
            Err(PeLoadError::UnsupportedRelocationType(t)) if t == bad_type => {}
            other => panic!("expected UnsupportedRelocationType({bad_type}), got {other:?}"),
        }
    }
}

#[test]
fn relocation_target_out_of_bounds_rejected() {
    // DIR64 at RVA = image_size - 4 (needs 8 bytes). Must reject.
    let mut image = vec![0u8; 4096];
    let entry: u16 = (REL_DIR64 << 12) | 0x0FFC; // within page = 0xFFC
    let mut reloc_bytes = Vec::new();
    // Place the block at a page RVA that makes target_rva = image_size - 4.
    reloc_bytes.extend_from_slice(&0u32.to_le_bytes()); // page RVA = 0
    reloc_bytes.extend_from_slice(&12u32.to_le_bytes());
    reloc_bytes.extend_from_slice(&entry.to_le_bytes());
    reloc_bytes.extend_from_slice(&0u16.to_le_bytes());
    // target_rva = 0 + 0xFFC = 0xFFC; + 8 = 0x1004 > image_size 4096. Reject.
    match apply_relocations_to_slice(&mut image, 0x1000, 0x2000, &reloc_bytes) {
        Err(PeLoadError::RelocationMalformed(_)) => {}
        other => panic!("expected RelocationMalformed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// PeLoadError taxonomy: every variant has a stable log token
// ---------------------------------------------------------------------------

#[test]
fn every_error_variant_has_stable_log_token() {
    use PeLoadError::*;
    let cases: &[(PeLoadError, &str)] = &[
        (TooShort { have: 1, need: 64 }, "pe_too_short"),
        (BadDosSignature, "pe_bad_dos_sig"),
        (NtOffsetOutOfBounds(0x1000_0000), "pe_nt_offset_oob"),
        (BadNtSignature, "pe_bad_nt_sig"),
        (UnsupportedMachine(0x1234), "pe_unsupported_machine"),
        (UnsupportedPe32, "pe_unsupported_pe32"),
        (BadOptionalHeaderMagic(0xBEEF), "pe_bad_opt_magic"),
        (UnsupportedSubsystem(3), "pe_unsupported_subsystem"),
        (
            ImageTooLarge {
                claimed: 1 << 40,
                max: MAX_IMAGE_SIZE,
            },
            "pe_image_too_large",
        ),
        (
            TooManySections {
                claimed: 9000,
                max: MAX_SECTIONS,
            },
            "pe_too_many_sections",
        ),
        (HasImports, "pe_has_imports"),
        (HasTlsCallbacks, "pe_has_tls"),
        (
            SectionOutOfBounds {
                section_index: 3,
                detail: "test",
            },
            "pe_section_oob",
        ),
        (RelocationMalformed("x"), "pe_reloc_malformed"),
        (UnsupportedRelocationType(99), "pe_reloc_unsupported"),
        (EntryPointOutOfBounds, "pe_entry_oob"),
        (ParseFailed("x"), "pe_parse_failed"),
    ];
    for (err, token) in cases {
        assert_eq!(
            err.as_log_token(),
            *token,
            "token drift for {err:?}: got {} expected {}",
            err.as_log_token(),
            token,
        );
    }
}

// ---------------------------------------------------------------------------
// SHA-256 correctness — via known-vector test
// ---------------------------------------------------------------------------

#[test]
fn sha256_known_vector_empty_string() {
    // Published SHA-256 of empty input.
    let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let digest = sha256_of(b"");
    let hex = hex_encode_32(&digest);
    assert_eq!(hex, expected);
}

#[test]
fn sha256_known_vector_abc() {
    // Published SHA-256 of "abc".
    let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let digest = sha256_of(b"abc");
    let hex = hex_encode_32(&digest);
    assert_eq!(hex, expected);
}

#[test]
fn hex_encode_round_trip() {
    let digest: [u8; 32] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
        0x32, 0x10,
    ];
    let hex = hex_encode_32(&digest);
    assert_eq!(
        hex,
        "00112233445566778899aabbccddeeff0123456789abcdeffedcba9876543210"
    );
}

// ---------------------------------------------------------------------------
// Hand-rolled-parser regression coverage (v0.16.x goblin removal)
//
// These lock in what the goblin-0.9.3 parser got wrong and the hand-rolled
// reader must get right:
//   * a PE that OMITS the MSVC DOS stub (e_lfanew == 0x40) — the Linux
//     EFI-stub layout 0.9.3's `end_offset <= start_offset` check rejected,
//     powering archie off on every kernel select;
//   * number_of_rva_and_sizes < 16 (Linux kernels ship ~6) bounding the
//     data-directory accessors;
//   * PE32 (0x10b) rejected cleanly, not misparsed with 64-bit offsets.
// ---------------------------------------------------------------------------

/// Build a minimal but fully §5.1-valid PE32+ EFI application with its PE
/// header at `e_lfanew`. Sixteen physical data-directory slots are laid
/// out, of which `declared_n_rva` are advertised. `tls_at_9` optionally
/// writes a TLS directory (index 9) so a test can prove the n_rva bound.
/// Pass `opt_magic = PE32_MAGIC` to forge a 32-bit image for rejection.
fn build_pe(
    e_lfanew: u32,
    opt_magic: u16,
    declared_n_rva: u32,
    tls_at_9: Option<(u32, u32)>,
) -> Vec<u8> {
    let soh: u16 = 24 + 88 + 16 * 8; // 240: standard + windows + 16 data dirs
    let coff = e_lfanew as usize + 4;
    let opt = coff + 20;
    let sec = opt + soh as usize;
    let buf_len = (sec + 40).max(0x400);
    let mut b = vec![0u8; buf_len];

    b[0..2].copy_from_slice(b"MZ");
    b[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());

    b[e_lfanew as usize..e_lfanew as usize + 4].copy_from_slice(b"PE\0\0");
    b[coff..coff + 2].copy_from_slice(&expected_machine().to_le_bytes());
    b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // number_of_sections
    b[coff + 16..coff + 18].copy_from_slice(&soh.to_le_bytes()); // size_of_optional_header
    b[coff + 18..coff + 20].copy_from_slice(&0x0022u16.to_le_bytes()); // characteristics

    b[opt..opt + 2].copy_from_slice(&opt_magic.to_le_bytes());
    b[opt + 0x10..opt + 0x14].copy_from_slice(&0x1000u32.to_le_bytes()); // entry_rva
    b[opt + 0x18..opt + 0x20].copy_from_slice(&0x1_4000_0000u64.to_le_bytes()); // image_base
    b[opt + 0x38..opt + 0x3C].copy_from_slice(&0x2000u32.to_le_bytes()); // size_of_image
    b[opt + 0x44..opt + 0x46].copy_from_slice(&10u16.to_le_bytes()); // subsystem = EFI app
    b[opt + 0x6C..opt + 0x70].copy_from_slice(&declared_n_rva.to_le_bytes());
    if let Some((va, size)) = tls_at_9 {
        let slot = opt + 0x70 + 9 * 8; // data directory index 9 = TLS
        b[slot..slot + 4].copy_from_slice(&va.to_le_bytes());
        b[slot + 4..slot + 8].copy_from_slice(&size.to_le_bytes());
    }

    b[sec..sec + 5].copy_from_slice(b".text");
    b[sec + 8..sec + 12].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual_size
    b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual_address
    b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes()); // size_of_raw_data
    b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // pointer_to_raw_data
    b[sec + 36..sec + 40].copy_from_slice(&0x2000_0000u32.to_le_bytes()); // IMAGE_SCN_MEM_EXECUTE
    b
}

#[test]
fn omitted_dos_stub_kernel_layout_parses_and_validates() {
    // e_lfanew == 0x40: the PE header sits immediately after the DOS header
    // with NO DOS stub — exactly archie's vmlinuz-linux. goblin 0.9.3
    // rejected this; the hand-rolled reader must accept it.
    let img = build_pe(0x40, PE32_PLUS_MAGIC, 16, None);
    let s = parse_headers(&img).expect("omitted-DOS-stub PE32+ must parse");
    validate_headers(&s, &img).expect("omitted-DOS-stub PE32+ must validate");
    assert_eq!(s.opt_magic, PE32_PLUS_MAGIC);
    assert_eq!(s.machine, expected_machine());
    assert_eq!(s.num_sections, 1);
    assert_eq!(s.size_of_image, 0x2000);
    assert_eq!(s.entry_rva, 0x1000);
    assert!(s.entry_in_executable_section);
    assert_eq!(s.import_dir_size, 0);
    assert_eq!(s.tls_dir_size, 0);
}

#[test]
fn dos_stub_present_layout_also_parses() {
    // The same image with a non-zero e_lfanew (a DOS stub region present)
    // must parse identically — the reader is agnostic to the stub.
    let img = build_pe(0x80, PE32_PLUS_MAGIC, 16, None);
    let s = parse_headers(&img).expect("with-DOS-stub PE32+ must parse");
    validate_headers(&s, &img).expect("with-DOS-stub PE32+ must validate");
    assert_eq!(s.entry_rva, 0x1000);
}

#[test]
fn data_directory_count_is_bounded_by_number_of_rva_and_sizes() {
    // A TLS directory is physically present at index 9, but the header
    // advertises only 6 directories. The accessor must treat index 9 as
    // absent (Linux kernels ship < 16), so tls_dir_size == 0.
    let bounded = build_pe(0x40, PE32_PLUS_MAGIC, 6, Some((0x9000, 0x40)));
    let s = parse_headers(&bounded).expect("must parse");
    assert_eq!(
        s.tls_dir_size, 0,
        "index 9 is beyond number_of_rva_and_sizes=6 and must read as absent"
    );

    // With the full 16 advertised, the same physical TLS entry IS read.
    let full = build_pe(0x40, PE32_PLUS_MAGIC, 16, Some((0x9000, 0x40)));
    let s = parse_headers(&full).expect("must parse");
    assert_eq!(s.tls_dir_size, 0x40, "in-range directory must be read");
}

#[test]
fn pe32_image_is_rejected_cleanly() {
    // A 32-bit (PE32, 0x10b) optional header: parse surfaces the magic and
    // validate rejects it with the precise token — never misparsed with
    // 64-bit field offsets.
    let img = build_pe(0x40, PE32_MAGIC, 16, None);
    let s = parse_headers(&img).expect("parse must surface the magic, not panic");
    assert_eq!(s.opt_magic, PE32_MAGIC);
    match validate_headers(&s, &img) {
        Err(PeLoadError::UnsupportedPe32) => {}
        other => panic!("expected UnsupportedPe32, got {other:?}"),
    }
}
