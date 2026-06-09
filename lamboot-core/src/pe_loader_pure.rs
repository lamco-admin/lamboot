//! Layer: 3 (pure) — Parsers & Shared Types.
//!
//! Pure PE-loader logic — parse, validate, relocation math, SHA-256.
//!
//! No UEFI, no allocation of pages, no protocol installation. This
//! module is included verbatim into `lamboot-fs-tests` via `#[path]`
//! so host tests can exercise every validation + relocation branch
//! without firmware. The Volume-dependent side (`allocate_pages`,
//! `install_protocol_interface`, `start_image`) lives in `pe_loader.rs`.
//!
//! SDS-3 PR-1 / v1.1 amendment #3.
//!
//! `unreachable_pub` + `dead_code` are allowed rather than `expect`-ed
//! because this module is consumed by two code generators with
//! different reachability views: lamboot-core is a UEFI binary with no
//! public API (so the lints would fire), and lamboot-fs-tests's host
//! test target reaches them as an external consumer (so
//! `#[expect]` would go unfulfilled, producing a different warning).
//! `allow` is the only annotation that suits both consumers.

#![allow(dead_code)]
#![allow(unreachable_pub)]
// The `unwrap`s below are infallible: every `try_into` targets a statically
// 2/4/8-byte slice whose bounds are validated immediately before the read.
#![allow(clippy::unwrap_used)]

use alloc::{string::String, vec, vec::Vec};

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// PE image constraints (SDS-3 §1.4 / §5.1)
// ---------------------------------------------------------------------------

/// Maximum image size accepted — 256 MiB. Covers the largest UKIs we
/// have catalogued (Fedora sd-stub UKI ~60 MB) with ~4× headroom.
pub const MAX_IMAGE_SIZE: u64 = 256 * 1024 * 1024;

/// Maximum number of sections accepted. Realistic kernels have 4-10
/// sections; 256 is a defensive cap against malicious headers
/// claiming billions.
pub const MAX_SECTIONS: u16 = 256;

// ---------------------------------------------------------------------------
// PE/COFF constants
// ---------------------------------------------------------------------------

/// Optional-header magic for PE32+ (64-bit). SDS-3 §5.1 requires this.
pub const PE32_PLUS_MAGIC: u16 = 0x20B;
/// Optional-header magic for PE32 (32-bit). SDS-3 refuses these.
pub const PE32_MAGIC: u16 = 0x10B;

/// Machine types LamBoot accepts, matched per build target at
/// validation time.
pub const MACHINE_X86_64: u16 = 0x8664;
pub const MACHINE_AARCH64: u16 = 0xAA64;

/// PE subsystem values we load. Per SDS-3 §5.1.
pub const SUBSYSTEM_EFI_APPLICATION: u16 = 10;
pub const SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER: u16 = 11;
pub const SUBSYSTEM_EFI_RUNTIME_DRIVER: u16 = 12;

/// Base-relocation types we apply. Per SDS-3 §9.2.
pub const REL_ABSOLUTE: u16 = 0;
pub const REL_HIGH: u16 = 1;
pub const REL_LOW: u16 = 2;
pub const REL_HIGHLOW: u16 = 3;
pub const REL_DIR64: u16 = 10;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Every PE-load failure surfaces as a `PeLoadError`. Variants are
/// exhaustively enumerated in SDS-3 §2.3.
#[derive(Debug)]
pub enum PeLoadError {
    TooShort {
        have: usize,
        need: usize,
    },
    BadDosSignature,
    NtOffsetOutOfBounds(u32),
    BadNtSignature,
    UnsupportedMachine(u16),
    UnsupportedPe32,
    BadOptionalHeaderMagic(u16),
    UnsupportedSubsystem(u16),
    ImageTooLarge {
        claimed: u64,
        max: u64,
    },
    TooManySections {
        claimed: u16,
        max: u16,
    },
    HasImports,
    HasTlsCallbacks,
    SectionOutOfBounds {
        section_index: usize,
        detail: &'static str,
    },
    RelocationMalformed(&'static str),
    UnsupportedRelocationType(u16),
    EntryPointOutOfBounds,
    /// The hand-rolled PE header parse rejected the bytes.
    ParseFailed(&'static str),
}

impl PeLoadError {
    /// Stable token for trust-log `status` field. Never panics.
    pub fn as_log_token(&self) -> &'static str {
        match self {
            Self::TooShort { .. } => "pe_too_short",
            Self::BadDosSignature => "pe_bad_dos_sig",
            Self::NtOffsetOutOfBounds(_) => "pe_nt_offset_oob",
            Self::BadNtSignature => "pe_bad_nt_sig",
            Self::UnsupportedMachine(_) => "pe_unsupported_machine",
            Self::UnsupportedPe32 => "pe_unsupported_pe32",
            Self::BadOptionalHeaderMagic(_) => "pe_bad_opt_magic",
            Self::UnsupportedSubsystem(_) => "pe_unsupported_subsystem",
            Self::ImageTooLarge { .. } => "pe_image_too_large",
            Self::TooManySections { .. } => "pe_too_many_sections",
            Self::HasImports => "pe_has_imports",
            Self::HasTlsCallbacks => "pe_has_tls",
            Self::SectionOutOfBounds { .. } => "pe_section_oob",
            Self::RelocationMalformed(_) => "pe_reloc_malformed",
            Self::UnsupportedRelocationType(_) => "pe_reloc_unsupported",
            Self::EntryPointOutOfBounds => "pe_entry_oob",
            Self::ParseFailed(_) => "pe_parse_failed",
        }
    }
}

impl core::fmt::Display for PeLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_log_token())
    }
}

// ---------------------------------------------------------------------------
// Summary — the subset of parsed PE that subsequent passes consume
// ---------------------------------------------------------------------------

/// Flat summary of a parsed PE. Produced by `parse_headers`, consumed
/// by `validate_headers`, `compute_section_copy_plan`, and the
/// allocation/copy/relocate passes in `pe_loader.rs`.
#[derive(Debug)]
pub struct PeSummary {
    pub machine: u16,
    pub subsystem: u16,
    pub preferred_base: u64,
    pub size_of_image: u64,
    pub entry_rva: u32,
    pub num_sections: u16,
    pub opt_magic: u16,
    pub sections: Vec<SectionPlan>,
    pub reloc_dir: Option<DataDir>,
    pub import_dir_size: u32,
    pub tls_dir_size: u32,
    /// The specific executable RVA range (derived from the section
    /// containing the entry point). Used to validate that the entry
    /// point lands in an IMAGE_SCN_MEM_EXECUTE section.
    pub entry_in_executable_section: bool,
}

/// Copy plan for a single section — the byte ranges the allocate+copy
/// pass will use. Bounds have already been validated.
#[derive(Debug, Clone, Copy)]
pub struct SectionPlan {
    pub index: usize,
    /// Offset into the loaded image (= destination) where this
    /// section's bytes go.
    pub virt_addr: u32,
    /// Bytes of the section in the virtual image.
    pub virt_size: u32,
    /// Offset into the input byte slice where this section's bytes
    /// come from.
    pub raw_offset: u32,
    /// Bytes to copy. Clamped to `min(raw_size, virt_size)` to handle
    /// BSS-like sections where `raw_size > virt_size`.
    pub copy_len: u32,
    /// True when section's `Characteristics` has `IMAGE_SCN_MEM_EXECUTE`.
    pub executable: bool,
}

/// One PE data-directory entry (RVA + size). The pure module carries
/// its own plain type so it has no third-party-parser dependency.
#[derive(Debug, Clone, Copy)]
pub struct DataDir {
    pub virtual_address: u32,
    pub size: u32,
}

// ---------------------------------------------------------------------------
// Fixed-width little-endian readers + data-directory accessor
//
// Bounds-checked and non-panicking on ANY input (the fuzz target asserts
// no panic on arbitrary bytes). A short read surfaces as `ParseFailed` so
// the stable `pe_parse_failed` trust-log token is preserved.
// ---------------------------------------------------------------------------

#[inline]
fn rd_u16(b: &[u8], off: usize) -> Result<u16, PeLoadError> {
    let s = b
        .get(off..off + 2)
        .ok_or(PeLoadError::ParseFailed("u16 read past end of image"))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn rd_u32(b: &[u8], off: usize) -> Result<u32, PeLoadError> {
    let s = b
        .get(off..off + 4)
        .ok_or(PeLoadError::ParseFailed("u32 read past end of image"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn rd_u64(b: &[u8], off: usize) -> Result<u64, PeLoadError> {
    let s = b
        .get(off..off + 8)
        .ok_or(PeLoadError::ParseFailed("u64 read past end of image"))?;
    Ok(u64::from_le_bytes(s.try_into().unwrap()))
}

/// Read PE data directory `idx`. Matches the getter semantics LamBoot
/// relied on: absent when `idx >= number_of_rva_and_sizes` (Linux kernels
/// ship fewer than the full 16), when the 8-byte slot is out of bounds, or
/// when the entry is all-zero.
fn read_data_dir(bytes: &[u8], start: usize, n_rva: u32, idx: u32) -> Option<DataDir> {
    if idx >= n_rva {
        return None;
    }
    let off = start + (idx as usize) * 8;
    let virtual_address = rd_u32(bytes, off).ok()?;
    let size = rd_u32(bytes, off + 4).ok()?;
    if virtual_address == 0 && size == 0 {
        return None;
    }
    Some(DataDir {
        virtual_address,
        size,
    })
}

// ---------------------------------------------------------------------------
// parse_headers — hand-rolled PE/COFF header reader
// ---------------------------------------------------------------------------

/// Parse the PE headers and flatten to a `PeSummary`.
///
/// Reads only the load-bearing COFF + PE32+ optional-header fields from
/// their fixed offsets; the MSVC DOS stub and Rich header are never
/// touched (skipping them is what makes this robust across the Linux
/// EFI-stub / UKI header zoo). PE32 and non-PE images are surfaced via
/// `opt_magic` for `validate_headers` to reject with a precise token.
pub fn parse_headers(bytes: &[u8]) -> Result<PeSummary, PeLoadError> {
    parse_headers_with_diag(bytes, None)
}

/// Diagnostic variant — accepts an optional callback invoked at major
/// internal checkpoints. Added v0.11.0 for pve2/ASUS G10AJ freeze
/// investigation; release callers pass None.
pub fn parse_headers_with_diag(
    bytes: &[u8],
    mut diag: Option<&mut dyn FnMut(&str)>,
) -> Result<PeSummary, PeLoadError> {
    macro_rules! diag {
        ($msg:expr) => {
            if let Some(ref mut cb) = diag {
                cb($msg);
            }
        };
    }
    diag!("PRH1 enter\n");
    if bytes.len() < 64 {
        return Err(PeLoadError::TooShort {
            have: bytes.len(),
            need: 64,
        });
    }
    if bytes[0..2] != *b"MZ" {
        return Err(PeLoadError::BadDosSignature);
    }
    let e_lfanew = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]);
    if (e_lfanew as usize).saturating_add(4) > bytes.len() {
        return Err(PeLoadError::NtOffsetOutOfBounds(e_lfanew));
    }
    let nt_off = e_lfanew as usize;
    if bytes[nt_off..nt_off + 4] != *b"PE\0\0" {
        return Err(PeLoadError::BadNtSignature);
    }
    diag!("PRH2 sigs_ok before_header_parse\n");

    // Hand-rolled PE/COFF header read. We deliberately parse ONLY the
    // fields LamBoot loads with — straight from their spec-fixed offsets —
    // and never touch the MSVC DOS stub or the Rich header. Those two
    // structures are exactly what goblin 0.9.3's `Header::parse` insisted
    // on validating: its `end_offset <= start_offset` DOS-stub check
    // rejected the Linux EFI-stub layout (e_lfanew == 0x40, no DOS stub)
    // that every modern x86_64/aarch64 kernel uses, and its unconditional
    // Rich-header scan underflowed on the same images. Reading only the
    // load-bearing fields is both more robust across the header zoo and
    // drops goblin (plus plain + scroll) from the dependency set — which
    // is what lets lamboot-core sit in the Debian archive.
    const SIZEOF_PE_MAGIC: usize = 4;
    const SIZEOF_COFF_HEADER: usize = 20;

    // COFF header (20 bytes) immediately follows the 4-byte PE signature.
    let coff_off = nt_off + SIZEOF_PE_MAGIC;
    let machine = rd_u16(bytes, coff_off)?; // +0
    let num_sections = rd_u16(bytes, coff_off + 2)?; // +2
    let size_of_optional_header = rd_u16(bytes, coff_off + 16)? as usize; // +16

    // The optional header begins immediately after the COFF header.
    let opt_off = coff_off + SIZEOF_COFF_HEADER;
    let opt_magic = rd_u16(bytes, opt_off)?; // +0x00

    // PE32+ (0x20B) only. PE32 (0x10B) and any other magic are surfaced
    // via `opt_magic` so `validate_headers` rejects them with the precise
    // token (UnsupportedPe32 / BadOptionalHeaderMagic). We must NOT read
    // the 64-bit field offsets out of a 32-bit (or junk) optional header,
    // so short-circuit here, carrying only the fields well-defined
    // regardless of magic (machine + section count). Section/field parsing
    // below is therefore reached only for genuine PE32+ images.
    if opt_magic != PE32_PLUS_MAGIC {
        diag!("PRH3 non_pe32plus_magic\n");
        return Ok(PeSummary {
            machine,
            subsystem: 0,
            preferred_base: 0,
            size_of_image: 0,
            entry_rva: 0,
            num_sections,
            opt_magic,
            sections: Vec::new(),
            reloc_dir: None,
            import_dir_size: 0,
            tls_dir_size: 0,
            entry_in_executable_section: false,
        });
    }
    diag!("PRH3 after_header_parse\n");

    // PE32+ optional-header fields, by spec offset from `opt_off`:
    //   0x10 address_of_entry_point (u32)
    //   0x18 image_base             (u64)
    //   0x38 size_of_image          (u32)
    //   0x44 subsystem              (u16)
    //   0x6C number_of_rva_and_sizes(u32)
    // The data directories begin at 0x70 (24-byte standard fields +
    // 88-byte windows fields).
    let entry_rva = rd_u32(bytes, opt_off + 0x10)?;
    let preferred_base = rd_u64(bytes, opt_off + 0x18)?;
    let size_of_image = u64::from(rd_u32(bytes, opt_off + 0x38)?);
    let subsystem = rd_u16(bytes, opt_off + 0x44)?;
    let number_of_rva_and_sizes = rd_u32(bytes, opt_off + 0x6C)?;
    let data_dir_start = opt_off + 0x70;

    // Directory indices: import = 1, base relocation = 5, TLS = 9.
    let import_dir_size =
        read_data_dir(bytes, data_dir_start, number_of_rva_and_sizes, 1).map_or(0, |d| d.size);
    let reloc_dir = read_data_dir(bytes, data_dir_start, number_of_rva_and_sizes, 5);
    let tls_dir_size =
        read_data_dir(bytes, data_dir_start, number_of_rva_and_sizes, 9).map_or(0, |d| d.size);
    diag!("PRH4 after_optional_header\n");

    if num_sections > MAX_SECTIONS {
        return Err(PeLoadError::TooManySections {
            claimed: num_sections,
            max: MAX_SECTIONS,
        });
    }
    diag!("PRH5 before_manual_section_parse\n");

    // Section table: 40-byte entries immediately after the optional
    // header. Bounds-check the whole table up front so the per-field reads
    // in the loop below cannot go out of range.
    let sections_offset_start = opt_off + size_of_optional_header;
    let needed = (num_sections as usize).saturating_mul(40);
    if sections_offset_start.saturating_add(needed) > bytes.len() {
        return Err(PeLoadError::ParseFailed(
            "section table extends past file end",
        ));
    }

    let mut sections: Vec<SectionPlan> = Vec::with_capacity(num_sections as usize);
    let mut entry_in_executable_section = false;
    for i in 0..num_sections as usize {
        let base = sections_offset_start + i * 40;
        // Name (8 bytes at base) — skipped, we don't use it.
        let virt_size = u32::from_le_bytes(bytes[base + 8..base + 12].try_into().unwrap());
        let virt_addr = u32::from_le_bytes(bytes[base + 12..base + 16].try_into().unwrap());
        let raw_size = u32::from_le_bytes(bytes[base + 16..base + 20].try_into().unwrap());
        let raw_offset = u32::from_le_bytes(bytes[base + 20..base + 24].try_into().unwrap());
        // pointer_to_relocations [24..28], pointer_to_linenumbers [28..32],
        // number_of_relocations [32..34], number_of_linenumbers [34..36] — skipped.
        let characteristics = u32::from_le_bytes(bytes[base + 36..base + 40].try_into().unwrap());

        let copy_len = raw_size.min(virt_size);
        let executable = (characteristics & 0x2000_0000) != 0;
        sections.push(SectionPlan {
            index: i,
            virt_addr,
            virt_size,
            raw_offset,
            copy_len,
            executable,
        });
        if executable
            && entry_rva >= virt_addr
            && (entry_rva as u64) < (virt_addr as u64 + virt_size as u64)
        {
            entry_in_executable_section = true;
        }
    }
    diag!("PRH6 after_manual_section_parse\n");

    Ok(PeSummary {
        machine,
        subsystem,
        preferred_base,
        size_of_image,
        entry_rva,
        num_sections,
        opt_magic,
        sections,
        reloc_dir,
        import_dir_size,
        tls_dir_size,
        entry_in_executable_section,
    })
}

// ---------------------------------------------------------------------------
// validate_headers — SDS-3 §5.1 conformance check
// ---------------------------------------------------------------------------

/// Return the machine type LamBoot accepts for its build target.
/// `#[cfg]`-gated so running the host test suite on x86_64 matches
/// the x86_64 UEFI build's expectation; aarch64 builds probe the
/// other magic.
#[must_use]
pub const fn expected_machine() -> u16 {
    #[cfg(target_arch = "x86_64")]
    {
        MACHINE_X86_64
    }
    #[cfg(target_arch = "aarch64")]
    {
        MACHINE_AARCH64
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        MACHINE_X86_64
    }
}

/// Apply every §5.1 constraint. Returns the summary unchanged on
/// success so call sites can chain: `let s = parse_headers(b)?;
/// validate_headers(&s, b)?;`.
pub fn validate_headers(s: &PeSummary, bytes: &[u8]) -> Result<(), PeLoadError> {
    // Machine must match build target.
    if s.machine != expected_machine() {
        return Err(PeLoadError::UnsupportedMachine(s.machine));
    }

    // Optional header magic: PE32+ only.
    if s.opt_magic == PE32_MAGIC {
        return Err(PeLoadError::UnsupportedPe32);
    }
    if s.opt_magic != PE32_PLUS_MAGIC {
        return Err(PeLoadError::BadOptionalHeaderMagic(s.opt_magic));
    }

    // Subsystem: EFI application / boot driver / runtime driver only.
    match s.subsystem {
        SUBSYSTEM_EFI_APPLICATION
        | SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER
        | SUBSYSTEM_EFI_RUNTIME_DRIVER => {}
        other => return Err(PeLoadError::UnsupportedSubsystem(other)),
    }

    // Image-size cap.
    if s.size_of_image > MAX_IMAGE_SIZE {
        return Err(PeLoadError::ImageTooLarge {
            claimed: s.size_of_image,
            max: MAX_IMAGE_SIZE,
        });
    }

    // Imports + TLS must be empty.
    if s.import_dir_size != 0 {
        return Err(PeLoadError::HasImports);
    }
    if s.tls_dir_size != 0 {
        return Err(PeLoadError::HasTlsCallbacks);
    }

    // Section bounds.
    for plan in &s.sections {
        // Virtual range within SizeOfImage.
        let virt_end = (plan.virt_addr as u64).saturating_add(plan.virt_size as u64);
        if virt_end > s.size_of_image {
            return Err(PeLoadError::SectionOutOfBounds {
                section_index: plan.index,
                detail: "virtual range exceeds SizeOfImage",
            });
        }
        // Raw range within input bytes (only if copy_len > 0).
        if plan.copy_len > 0 {
            let raw_end = (plan.raw_offset as usize).saturating_add(plan.copy_len as usize);
            if raw_end > bytes.len() {
                return Err(PeLoadError::SectionOutOfBounds {
                    section_index: plan.index,
                    detail: "raw range exceeds input",
                });
            }
        }
    }

    // Entry point bounds + executable section.
    if (s.entry_rva as u64) >= s.size_of_image {
        return Err(PeLoadError::EntryPointOutOfBounds);
    }
    if !s.entry_in_executable_section {
        return Err(PeLoadError::EntryPointOutOfBounds);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Relocation application (SDS-3 §9)
// ---------------------------------------------------------------------------

/// Apply base relocations to an in-memory image slice.
///
/// `image` is the section-copied destination buffer of length
/// `size_of_image`. `reloc_bytes` is the raw byte range at the
/// relocation directory's RVA (virtual_address within `image`).
/// `preferred_base` / `actual_base` produce `delta = actual - preferred`.
///
/// No-op if delta == 0. Returns RelocationMalformed / UnsupportedRelocationType
/// on violation. Never panics on malformed input.
pub fn apply_relocations_to_slice(
    image: &mut [u8],
    preferred_base: u64,
    actual_base: u64,
    reloc_bytes: &[u8],
) -> Result<(), PeLoadError> {
    let delta = actual_base.wrapping_sub(preferred_base);
    if delta == 0 {
        return Ok(());
    }

    let image_size = image.len();
    let mut cursor = 0usize;

    while cursor < reloc_bytes.len() {
        if cursor + 8 > reloc_bytes.len() {
            return Err(PeLoadError::RelocationMalformed("block header truncated"));
        }
        let virt_addr =
            u32::from_le_bytes(reloc_bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        let size_of_block =
            u32::from_le_bytes(reloc_bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;

        if size_of_block == 0 {
            // Terminator. Some PE writers emit one; stop.
            break;
        }
        if size_of_block < 8 {
            return Err(PeLoadError::RelocationMalformed("SizeOfBlock < 8"));
        }
        if cursor + size_of_block > reloc_bytes.len() {
            return Err(PeLoadError::RelocationMalformed(
                "block extends past directory",
            ));
        }

        let entries_bytes = size_of_block - 8;
        if !entries_bytes.is_multiple_of(2) {
            return Err(PeLoadError::RelocationMalformed("odd entries_bytes"));
        }
        let entry_count = entries_bytes / 2;

        for i in 0..entry_count {
            let entry_offset = cursor + 8 + i * 2;
            let entry = u16::from_le_bytes(
                reloc_bytes[entry_offset..entry_offset + 2]
                    .try_into()
                    .unwrap(),
            );
            let reloc_type = entry >> 12;
            let within_page = (entry & 0x0FFF) as usize;
            let target_rva = virt_addr.saturating_add(within_page);

            match reloc_type {
                REL_ABSOLUTE => {
                    // No-op. Used for 2-byte block-padding.
                }
                REL_DIR64 => {
                    if target_rva + 8 > image_size {
                        return Err(PeLoadError::RelocationMalformed(
                            "DIR64 fixup target out of bounds",
                        ));
                    }
                    let slot = &mut image[target_rva..target_rva + 8];
                    let v = u64::from_le_bytes(slot.try_into().unwrap());
                    slot.copy_from_slice(&v.wrapping_add(delta).to_le_bytes());
                }
                REL_HIGHLOW => {
                    if target_rva + 4 > image_size {
                        return Err(PeLoadError::RelocationMalformed(
                            "HIGHLOW fixup target out of bounds",
                        ));
                    }
                    let slot = &mut image[target_rva..target_rva + 4];
                    let v = u32::from_le_bytes(slot.try_into().unwrap());
                    // HIGHLOW is 32-bit; truncating delta is the correct semantic here.
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "HIGHLOW relocation applies delta as u32; PE spec defines truncation"
                    )]
                    let delta32 = delta as u32;
                    slot.copy_from_slice(&v.wrapping_add(delta32).to_le_bytes());
                }
                REL_HIGH | REL_LOW => {
                    // x86_64 kernels don't emit these; refuse per §9.2.
                    return Err(PeLoadError::UnsupportedRelocationType(reloc_type));
                }
                other => return Err(PeLoadError::UnsupportedRelocationType(other)),
            }
        }

        cursor += size_of_block;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// SHA-256 — used in trust-log events for image_loaded_native
// ---------------------------------------------------------------------------

/// Compute the SHA-256 of the full input bytes. Pure software via
/// `sha2` (no asm feature). Used for `image_loaded_native` trust-log
/// events per SDS-3 §12.
#[must_use]
pub fn sha256_of(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Format a 32-byte digest as lowercase hex. Convenience for the
/// trust-log `sha256` field, which stores hex strings.
#[must_use]
pub fn hex_encode_32(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = vec![0u8; 64];
    for (i, b) in digest.iter().enumerate() {
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0x0F) as usize];
    }
    // SAFETY: every byte written is from the HEX alphabet, so the
    // result is valid ASCII / UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}
