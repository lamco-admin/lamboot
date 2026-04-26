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

use alloc::{string::String, vec, vec::Vec};

use goblin::pe::{data_directories::DataDirectory, options::ParseOptions, PE};
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
    /// goblin couldn't make sense of the bytes.
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

/// Mirror of goblin's `DataDirectory` with the two fields we need.
/// Plain data so the pure module has no goblin-typed return surface.
#[derive(Debug, Clone, Copy)]
pub struct DataDir {
    pub virtual_address: u32,
    pub size: u32,
}

impl From<DataDirectory> for DataDir {
    fn from(d: DataDirectory) -> Self {
        Self {
            virtual_address: d.virtual_address,
            size: d.size,
        }
    }
}

// ---------------------------------------------------------------------------
// parse_headers — the goblin-delegating front door
// ---------------------------------------------------------------------------

/// Parse the PE headers via goblin and flatten to a `PeSummary`.
///
/// Short-circuits TooShort / BadDosSignature / BadNtSignature before
/// calling goblin so we always produce a specific `PeLoadError` rather
/// than goblin's opaque `Error::Malformed`. After goblin succeeds we
/// verify magic, machine, subsystem at the summary level.
pub fn parse_headers(bytes: &[u8]) -> Result<PeSummary, PeLoadError> {
    // DOS header is 64 bytes; NT signature lives 4 bytes past
    // e_lfanew (which is at DOS+0x3C). Minimum viable PE is
    // DOS (64) + NT sig (4) + COFF header (20) + optional header
    // (>= 112 for PE32+) + section table (0 sections permitted).
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

    // Hand off to goblin for the rest. `ParseOptions::default()`
    // respects the PE layout strictly; we don't need resolve_rva.
    let pe = PE::parse_with_opts(bytes, &ParseOptions::default())
        .map_err(|_| PeLoadError::ParseFailed("goblin refused the bytes"))?;

    let coff = pe.header.coff_header;
    let num_sections = coff.number_of_sections;

    // Optional header presence is required by the goblin parser for
    // PE32+; absence means goblin would have errored already.
    let oh = pe
        .header
        .optional_header
        .ok_or(PeLoadError::BadOptionalHeaderMagic(0))?;

    let opt_magic = oh.standard_fields.magic;
    let preferred_base = oh.windows_fields.image_base;
    let size_of_image = oh.windows_fields.size_of_image as u64;
    let entry_rva = oh.standard_fields.address_of_entry_point as u32;
    let subsystem = oh.windows_fields.subsystem;

    let import_dir_size = oh.data_directories.get_import_table().map_or(0, |d| d.size);
    let tls_dir_size = oh.data_directories.get_tls_table().map_or(0, |d| d.size);
    let reloc_dir = oh
        .data_directories
        .get_base_relocation_table()
        .copied()
        .map(DataDir::from);

    // Early reject: num_sections > MAX_SECTIONS before we allocate a
    // SectionPlan vec of that size, which is hostile-input-bounded.
    if num_sections > MAX_SECTIONS {
        return Err(PeLoadError::TooManySections {
            claimed: num_sections,
            max: MAX_SECTIONS,
        });
    }

    // Flatten goblin's section iterator into a Vec<SectionPlan> with
    // clamped copy_len. Bounds-check happens in `validate_headers`
    // with the full input slice in hand (we need `bytes.len()` for
    // that check; keep it there for cleanest separation of parse vs
    // validate).
    let mut sections: Vec<SectionPlan> = Vec::with_capacity(num_sections as usize);
    let mut entry_in_executable_section = false;
    for (i, s) in pe.sections.iter().enumerate() {
        let raw_size = s.size_of_raw_data;
        let virt_size = s.virtual_size;
        let copy_len = raw_size.min(virt_size);
        // IMAGE_SCN_MEM_EXECUTE = 0x2000_0000
        let executable = (s.characteristics & 0x2000_0000) != 0;
        sections.push(SectionPlan {
            index: i,
            virt_addr: s.virtual_address,
            virt_size,
            raw_offset: s.pointer_to_raw_data,
            copy_len,
            executable,
        });
        // Entry-in-executable check: entry RVA in [virt_addr, virt_addr + virt_size)
        if executable
            && entry_rva >= s.virtual_address
            && (entry_rva as u64) < (s.virtual_address as u64 + virt_size as u64)
        {
            entry_in_executable_section = true;
        }
    }

    Ok(PeSummary {
        machine: coff.machine,
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
        if entries_bytes % 2 != 0 {
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
