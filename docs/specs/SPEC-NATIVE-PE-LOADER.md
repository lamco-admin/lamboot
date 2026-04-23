# SPEC-NATIVE-PE-LOADER: Native Rust PE Loader

**SDS number:** SDS-3 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/pe_loader.rs` (new, full implementation), `lamboot-core/src/boot.rs` (branched to call native loader when backend + feature flag permit), `lamboot-core/Cargo.toml` (add `goblin`, `sha2` deps)
**Depends on:** SDS-1 (`SPEC-FS-BACKEND-TRAIT.md`) and SDS-2 (`SPEC-EXT4-INTEGRATION.md`) MUST be landed first. Byte sources for the loader come from `Volume::read` and `Volume::open_stream` defined in SDS-1; ext4-native reads defined in SDS-2.
**Blocks:** SDS-4 (Native Trust Chain describes the verify step sitting between byte-source and this loader), SDS-6 (UEFI FS Driver Deprecation describes the path this loader replaces).
**External dependencies:** [`goblin`](https://crates.io/crates/goblin) v0.10.5 (PE parsing, `no_std` mode), [`sha2`](https://crates.io/crates/sha2) v0.10.8 (`no_std` mode, for trust-log SHA-256), uefi-rs ≥ 0.37 (already present; we use `boot::allocate_pages`, `boot::image_handle`, `table::system_table_raw`).

---

## 1. Overview

This specification defines LamBoot's native PE loader — the code that loads a kernel, UKI, or EFI driver from a byte slice into memory, applies relocations, and transfers control to the entry point — **without calling `BS->LoadImage()` / `BS->StartImage()`**. It is the mechanism by which LamBoot v1.0 bypasses the shim 15.8 `ShimLock`-uninstall problem at kernel-load time and the architectural heart of the "native boot" promise.

### 1.1 Why this is necessary

The documented v0.8.3 failure mode: LamBoot under Secure Boot loads a UEFI filesystem driver (e.g. ext4_x64.efi) via `LoadImage`+`StartImage`. Shim 15.8 uninstalls its `ShimLock` protocol the moment that child `StartImage` returns. When LamBoot subsequently tries to `LoadImage` a MOK-signed kernel, the Security2Arch firmware hook invokes our `SecurityOverride::hook_security2`, which tries to delegate to `ShimLock::Verify` — but `ShimLock` is gone, so the delegation fails and firmware rejects the image with `ACCESS_DENIED`.

SDS-2's native ext4 backend removes the need to ever `LoadImage` a UEFI filesystem driver. That fix alone is insufficient though: even once LamBoot can read the kernel from ext4 natively, **calling `BS->LoadImage(kernel_buffer)` still invokes the firmware's Security2Arch hook**, which still has the same `ShimLock`-availability problem if anything else in the session has uninstalled it (or if the kernel's authenticode bytes are in the shape firmware rejects despite shim approval — the well-documented "PE gaps" issue).

The ONLY way to definitively exit the firmware-LoadImage+shim-interaction failure modes is to load the kernel ourselves: parse the PE structure in-process, allocate memory, copy sections, apply base relocations, and invoke the entry point directly. Shim's `ShimLock::Verify` is called ONCE — before our ext4 driver has been loaded — to stamp the kernel's SHA-256 with a "shim-approved" verdict that we then record to the trust log. After that point, the firmware's Security2Arch path is never consulted again for this image.

### 1.2 What this SDS delivers

- `lamboot-core/src/pe_loader.rs` with three public functions:
  - `pub fn load_pe(bytes: &[u8]) -> Result<LoadedImage, PeLoadError>` — parse + allocate + copy sections + apply relocations, return a handle you can invoke.
  - `pub fn load_pe_stream(stream: Box<dyn FsStream>) -> Result<LoadedImage, PeLoadError>` — streaming variant for large images where buffering the whole thing is undesirable (kernel sizes grow; UKIs can be 100+ MB).
  - `pub unsafe fn start_image(image: LoadedImage, load_options: &[u16]) -> Status` — transfer control to the entry point.
- `LoadedImage` opaque handle carrying: base address, entry point address, image size, interior allocation record for Drop, `LoadedImageProtocol` installation handle so the child image can look up its own info.
- A formal list of the PE/COFF features we SUPPORT and REFUSE, mapped against the binaries we must actually load (Ubuntu/Debian/Fedora/Arch kernels, sd-stub UKIs, LamBoot's own signed drivers + modules).
- Relocation algorithm: exact byte-level description of how we walk `IMAGE_DIRECTORY_ENTRY_BASERELOC` and apply `IMAGE_REL_BASED_{ABSOLUTE,HIGH,LOW,HIGHLOW,DIR64}` types.
- Memory-allocation strategy: preferred-base-first via `boot::allocate_pages(AllocateType::Address(…))`, falling back to any-address + relocation when the preferred base conflicts.
- Entry-point-invocation contract: calling convention, argument shape, what we do with the return value.
- Trust-log surface: `image_loaded_native` event with SHA-256, entry point, size, source-backend tag.
- Precise failure-mode enumeration.
- Test plan: corpus of real kernels + UKIs + drivers, fuzzing, QEMU integration.

### 1.3 What this SDS does NOT deliver

- **Authenticode verification.** The PE loader accepts a byte slice and loads it. Verifying that slice is SDS-4's concern (`SPEC-NATIVE-TRUST-CHAIN.md`). The loader never tries to validate signatures itself; it is a pure loader.
- **Symbol resolution / imports.** EFI-stub kernels and UKIs have no imports. LamBoot's own drivers have no imports. UEFI applications in general may have imports, but none of the binaries we load do. The loader REJECTS any PE that has a non-empty import table (see §5.2) — not because we couldn't support it, but because the current LamBoot use cases don't need it and refusing keeps attack surface minimal.
- **DLLs.** Loading a PE in DLL-subsystem mode is refused. Kernels/UKIs/apps/drivers are subsystem `EFI_APPLICATION` (10), `EFI_BOOT_SERVICE_DRIVER` (11), or `EFI_RUNTIME_DRIVER` (12).
- **TLS callbacks.** PE files with non-empty TLS directory are refused (§5.2).
- **Exception-handling data (`.pdata`).** We don't consult it; just copy it into the image like any other section.
- **Delayed-load imports.** Same as imports — not present in our target binaries.
- **32-bit PE (PE32 vs PE32+).** x86_64 UEFI binaries are PE32+; aarch64 UEFI binaries are PE32+. We refuse PE32 (`IMAGE_NT_OPTIONAL_HDR32_MAGIC`).
- **WriteImage semantics.** The loader is read-allocate-execute. Any modification of the image happens between section copy and `start_image`, via relocation application only. No self-modifying code support (none of our targets need it).
- **Resource directory interpretation.** Resources (icons, strings) in a PE are copied with the section but not consulted.
- **LamBoot-itself-bootstrapping via this loader.** LamBoot is still loaded by firmware via `BS->LoadImage` (firmware can't use our loader; we're the ones it's loading). This loader is only for images LamBoot wants to start DURING its own boot flow.

### 1.4 Constraints

All SDS-1 and SDS-2 constraints apply. Additionally:

- `#![no_std]` + `alloc`. No `std`. No threads. No async.
- Max 256 sections per PE (well above any realistic kernel; bounds the parsing work and prevents maliciously-constructed headers claiming billions of sections).
- Max 256 MiB image size for the initial load path (covers the largest UKIs we've observed; hard-fails larger images with an unambiguous trust-log event).
- All `unsafe` blocks carry SAFETY comments. The loader has MORE `unsafe` than most LamBoot modules by necessity — raw pointer writes during relocation, transmuting entry-point addresses to function pointers, etc. Every block is documented.
- The loader MUST NOT call `BS->LoadImage` or `BS->StartImage`. Ever. That is the entire point.
- The loader MUST install a `LoadedImageProtocol` handle for the child image, because kernels and UKIs query it to discover their own load address + load options + device path. Without this, Linux kernels crash at early boot.
- The loader MUST mark memory as `EfiLoaderCode` (not `EfiLoaderData`) for executable pages, `EfiLoaderData` for read-only data. This matches what `BS->LoadImage` would have done and is what kernels expect.

---

## 2. External interface

### 2.1 Public types

```rust
/// Handle to a loaded, ready-to-run PE image.
///
/// Carries the resources needed for both `start_image` and Drop-time cleanup.
/// Opaque; consumers treat it as a move-only value.
pub(crate) struct LoadedImage {
    /// Image base in memory where we copied sections to.
    pub(super) image_base: *mut u8,
    /// Total size in bytes of the image in memory (= SizeOfImage).
    pub(super) image_size: usize,
    /// Computed entry point address = image_base + OptionalHeader::AddressOfEntryPoint.
    pub(super) entry_point: EntryFn,
    /// UEFI handle for the installed LoadedImageProtocol instance.
    pub(super) handle: uefi::Handle,
    /// SHA-256 of the ORIGINAL bytes (before relocation patching). For trust log.
    pub(super) sha256: [u8; 32],
    /// Number of pages we allocated. Needed at Drop for `free_pages`.
    pub(super) pages_allocated: usize,
    /// Which FsBackend tag sourced the bytes. For trust log and audit.
    pub(super) source_backend: &'static str,
}

type EntryFn = unsafe extern "efiapi" fn(
    image_handle: uefi::Handle,
    system_table: *mut uefi_raw::table::system::SystemTable,
) -> uefi::Status;
```

`LoadedImage` is `!Copy`, `!Clone`, `!Send`. It is a single resource representing a single in-memory image.

### 2.2 Public functions

```rust
/// Parse a PE image from `bytes`, allocate memory, copy sections, apply
/// relocations, install LoadedImageProtocol. Does NOT invoke the entry point.
///
/// Trust-log side effects: caller is responsible for emitting an
/// `image_loaded_native` event. The returned `LoadedImage.sha256` is suitable
/// to pass into the event.
pub(crate) fn load_pe(
    bytes: &[u8],
    load_options: Option<&[u16]>,
    source_backend: &'static str,
) -> Result<LoadedImage, PeLoadError>;

/// Streaming variant. Same semantics; sources bytes from the stream instead.
/// Internally buffers only one section at a time plus the PE headers,
/// reducing peak memory pressure for large UKIs.
pub(crate) fn load_pe_stream(
    stream: Box<dyn FsStream>,
    load_options: Option<&[u16]>,
    source_backend: &'static str,
) -> Result<LoadedImage, PeLoadError>;

/// Invoke the entry point. The image's lifecycle is handed off: if the
/// entry point returns, we are still running and the `Status` is its exit
/// code. Consumers that Drop the returned `LoadedImage` AFTER `start_image`
/// returns will free the image's memory.
///
/// SAFETY: Caller MUST ensure the underlying image has been trust-verified
/// before invocation (via ShimLock or equivalent). The PE loader itself does
/// not verify signatures; it just loads.
pub(crate) unsafe fn start_image(image: LoadedImage) -> uefi::Status;
```

### 2.3 Public error type

```rust
#[derive(Debug)]
pub(crate) enum PeLoadError {
    /// Byte slice too short to be a PE (less than DOS+NT headers).
    TooShort { have: usize, need: usize },
    /// DOS "MZ" signature missing.
    BadDosSignature,
    /// NT signature offset points outside bounds.
    NtOffsetOutOfBounds(u32),
    /// NT "PE\0\0" signature missing.
    BadNtSignature,
    /// Machine type not supported by this build.
    UnsupportedMachine(u16),
    /// PE32 (32-bit) image offered to a 64-bit loader.
    UnsupportedPe32,
    /// Magic in optional header not PE32+.
    BadOptionalHeaderMagic(u16),
    /// Subsystem not one we load.
    UnsupportedSubsystem(u16),
    /// Size_of_image exceeds our sanity cap.
    ImageTooLarge { claimed: u64, max: u64 },
    /// Number_of_sections exceeds our sanity cap.
    TooManySections { claimed: u16, max: u16 },
    /// Image has a non-empty import table. Not supported.
    HasImports,
    /// Image has a non-empty TLS table. Not supported.
    HasTlsCallbacks,
    /// Section table or per-section header references bytes outside the slice.
    SectionOutOfBounds { section_index: usize, detail: &'static str },
    /// Relocation directory is malformed (block extends past section end,
    /// offset overflows the block, etc.).
    RelocationMalformed(&'static str),
    /// A relocation type we don't know how to apply.
    UnsupportedRelocationType(u16),
    /// Memory allocation failed (UEFI OUT_OF_RESOURCES).
    AllocationFailed(uefi::Error),
    /// Entry point falls outside the allocated image range.
    EntryPointOutOfBounds,
    /// LoadedImageProtocol installation failed.
    ProtocolInstallFailed(uefi::Error),
    /// I/O error from the backing stream (streaming variant only).
    StreamIo(crate::fs_backend::FsError),
}

impl PeLoadError {
    /// Stable token for trust-log `status` field.
    pub(crate) fn as_log_token(&self) -> &'static str { /* … */ }
}
```

All PE-load failures produce a `PeLoadError`. Consumers wrap this in higher-level error types at the call site (usually `boot::boot_linux` or the Orchestration layer).

---

## 3. Module layout

```
lamboot-core/src/
├── pe_loader.rs              — THIS SDS delivers. ~600 LOC.
│   ├── pub fn load_pe
│   ├── pub fn load_pe_stream
│   ├── pub unsafe fn start_image
│   ├── struct LoadedImage (opaque)
│   ├── enum PeLoadError
│   ├── fn parse_headers
│   ├── fn validate_headers
│   ├── fn allocate_pages
│   ├── fn copy_sections
│   ├── fn apply_relocations
│   ├── fn install_loaded_image_protocol
│   └── impl Drop for LoadedImage
├── boot.rs                   — EDITED. New branch: if the volume's
│                               backend supports native loading AND
│                               the policy enables it, call pe_loader
│                               instead of uefi::boot::load_image.
│                               Old path retained as fallback.
└── main.rs                   — no change (the branch is in boot.rs).
```

No other files change.

---

## 4. Cargo dependencies

Add to `[dependencies]`:

```toml
# PE parsing (SDS-3). Exact-pinned for the same reasons as ext4-view.
goblin = { version = "=0.10.5", default-features = false, features = ["pe64", "endian_fd"] }

# SHA-256 for trust-log. no_std + no asm.
sha2 = { version = "=0.10.8", default-features = false }
```

Feature rationale:
- `goblin`'s default features pull in `std`; we disable. `pe64` enables 64-bit PE parsing (the only kind we support). `endian_fd` enables scroll-based endian-aware parsing which all PE parsing needs.
- `sha2` has an `asm` feature that requires nightly + architecture-specific assembly. LamBoot builds stable; disable.

---

## 5. PE image requirements

LamBoot's PE loader accepts a specific subset of the PE/COFF format. An image meeting these requirements is "acceptable"; an image violating any of them is REFUSED with a specific `PeLoadError`.

### 5.1 Required properties

| Property | Value | `PeLoadError` if violated |
|---|---|---|
| DOS signature at offset 0 | `"MZ"` (0x5A4D) | `BadDosSignature` |
| e_lfanew (at offset 0x3C) | ≤ byte length − 4 | `NtOffsetOutOfBounds` |
| NT signature at e_lfanew | `"PE\0\0"` | `BadNtSignature` |
| Machine | `0x8664` (x86_64) or `0xAA64` (aarch64) matching build target | `UnsupportedMachine` |
| Optional header magic | `0x20B` (PE32+) | `UnsupportedPe32` or `BadOptionalHeaderMagic` |
| Subsystem | 10 (EFI_APPLICATION), 11 (EFI_BOOT_SERVICE_DRIVER), or 12 (EFI_RUNTIME_DRIVER) | `UnsupportedSubsystem` |
| SizeOfImage | ≤ 256 MiB (§1.4) | `ImageTooLarge` |
| NumberOfSections | ≤ 256 (§1.4) | `TooManySections` |
| Each section's (VirtualAddress, VirtualSize) | Falls within SizeOfImage | `SectionOutOfBounds` |
| Each section's (PointerToRawData, SizeOfRawData) | Falls within input byte length | `SectionOutOfBounds` |
| Import directory size | 0 | `HasImports` |
| TLS directory size | 0 | `HasTlsCallbacks` |
| AddressOfEntryPoint | 0 ≤ entry < SizeOfImage AND within a section marked IMAGE_SCN_MEM_EXECUTE | `EntryPointOutOfBounds` |

### 5.2 Why import/TLS refusal

Every kernel and UKI we've catalogued has empty import and TLS directories. Refusing both narrows the attack surface: no import-resolution code to get wrong, no TLS-callback dispatch to implement. If a new LamBoot use case requires an image with imports, SDS-3 gets an amendment (adding an `ImportResolver` subsystem as a new Layer-3 module); until then, the check stays.

### 5.3 Target binary characterization

Empirical characterization of what we actually load:

| Binary | Typical size | Sections | Reloc type distribution (x86_64) | Subsystem |
|---|---|---|---|---|
| Ubuntu vmlinuz 6.17 | 17 MB | 4 (`.setup .compat .text .data`) | DIR64 heavy, ABSOLUTE padding | 10 (EFI_APPLICATION) |
| Debian vmlinuz 6.19 | 18 MB | 4 | Same | 10 |
| Fedora vmlinuz 6.19 | 18 MB | 4 | Same | 10 |
| systemd-stub UKI (Fedora) | 30-60 MB | 6-10 (`.linux .initrd .osrel .cmdline .uname .pcrsig` + base) | DIR64 heavy | 10 |
| LamBoot lambootx64.efi | 215 KB | 5 | DIR64 heavy | 10 |
| rEFInd ext4 driver | 60 KB | 5 | DIR64 | 11 |

All are PE32+, x86_64, subsystem 10 or 11, no imports, no TLS. All conform to §5.1. **Our loader is designed for exactly this characterization**; non-conforming images fail cleanly with a specific error.

### 5.4 Authenticode certificate handling

Signed images have a certificate table at the end of the file (referenced by `OptionalHeader::DataDirectory[IMAGE_DIRECTORY_ENTRY_SECURITY]`). **Our loader ignores this data entirely.** Signature verification is SDS-4's concern, happens before `load_pe` is called, and uses `ShimLock::Verify(bytes)` on the pre-load byte slice. The loader does NOT strip, parse, or modify the certificate table — it's past the end of any section, so it's never copied into the image anyway.

The "PE gap" issue (certain signed binaries have `SizeOfRawData` values that don't cover the full signed region, producing `sbverify` warnings like `data remaining[16558472 vs 17000520]: gaps between PE/COFF sections`) is **irrelevant to our loader**. We read sections by `(PointerToRawData, SizeOfRawData)` and don't care about bytes between sections. The firmware `LoadImage` path that chokes on PE gaps is exactly what we're bypassing.

---

## 6. Parsing with goblin

```rust
use goblin::pe::{PE, header::Header, optional_header::OptionalHeader};

fn parse_headers(bytes: &[u8]) -> Result<PE, PeLoadError> {
    if bytes.len() < 64 { return Err(PeLoadError::TooShort { have: bytes.len(), need: 64 }); }
    PE::parse(bytes).map_err(|e| match e {
        goblin::error::Error::Malformed(_) => PeLoadError::BadDosSignature,
        goblin::error::Error::BadMagic(_) => PeLoadError::BadOptionalHeaderMagic(0),
        _ => PeLoadError::BadNtSignature,
    })
}
```

goblin's `PE::parse` handles DOS+NT signature checks, Machine, Subsystem, OptionalHeader parsing, section table enumeration, and data-directory extraction. We then run the additional validation from §5.1 as a second pass.

### 6.1 Extracting what we need

From the parsed `PE`:
- `pe.header.coff_header.machine`
- `pe.header.optional_header.unwrap().standard_fields.magic` (== 0x20B)
- `pe.header.optional_header.unwrap().windows_fields.image_base` (preferred load address)
- `pe.header.optional_header.unwrap().windows_fields.size_of_image`
- `pe.header.optional_header.unwrap().standard_fields.address_of_entry_point`
- `pe.header.optional_header.unwrap().windows_fields.subsystem`
- `pe.sections` — iterable of section headers
- `pe.header.optional_header.unwrap().data_directories.get_base_relocation_table()` — for relocations
- `pe.header.optional_header.unwrap().data_directories.get_import_table()` — must be empty
- `pe.header.optional_header.unwrap().data_directories.get_tls_table()` — must be empty

### 6.2 Forbidden directories check

```rust
let od = pe.header.optional_header.ok_or(PeLoadError::BadOptionalHeaderMagic(0))?;
if let Some(imp) = od.data_directories.get_import_table() {
    if imp.size != 0 { return Err(PeLoadError::HasImports); }
}
if let Some(tls) = od.data_directories.get_tls_table() {
    if tls.size != 0 { return Err(PeLoadError::HasTlsCallbacks); }
}
```

(Structure checks are against `.size`, not presence — goblin returns `Some(DataDirectory { size: 0 })` for "directory entry exists but is empty." That's fine.)

---

## 7. Memory allocation

### 7.1 Page-count calculation

PE `SizeOfImage` is page-aligned per the PE spec. We allocate exactly that many 4 KiB UEFI pages:

```rust
let size_of_image = od.windows_fields.size_of_image as usize;
let pages = size_of_image.div_ceil(4096);
```

If `pages > 65_536` (= 256 MiB), we reject (`ImageTooLarge`).

### 7.2 Preferred-base allocation

We try the PE's preferred base first:

```rust
let preferred_base = od.windows_fields.image_base;
let base = match uefi::boot::allocate_pages(
    uefi::boot::AllocateType::Address(preferred_base),
    uefi::boot::MemoryType::LOADER_CODE,
    pages,
) {
    Ok(addr) => { rebased = false; addr }
    Err(_) => {
        // Preferred base unavailable; allocate anywhere and relocate.
        let addr = uefi::boot::allocate_pages(
            uefi::boot::AllocateType::AnyPages,
            uefi::boot::MemoryType::LOADER_CODE,
            pages,
        ).map_err(PeLoadError::AllocationFailed)?;
        rebased = true;
        addr
    }
};
```

Why `LOADER_CODE` not `LOADER_DATA`: UEFI spec requires loader-allocated regions that contain code to be `LOADER_CODE` so firmware knows to mark pages executable. Section-by-section W^X re-mapping is possible (using `DXE_SERVICES::GetMemorySpaceDescriptor` + `SetMemorySpaceAttributes`) but is firmware-specific and prone to failure modes. Matching `BS->LoadImage`'s default allocation type is the minimum-surprise choice.

### 7.3 Zeroing

`allocate_pages` on OVMF/edk2 returns zero-initialized memory on most platforms but the spec doesn't require it. We explicitly zero the full range before copying sections:

```rust
// SAFETY: `base` is a freshly allocated range of `pages * 4096` bytes.
// It is owned by us until `free_pages` in Drop; no aliases exist.
unsafe { core::ptr::write_bytes(base as *mut u8, 0, pages * 4096); }
```

This is important because PE sections have `VirtualSize > SizeOfRawData` for BSS-style zero-init regions. Without pre-zeroing we could leak kernel uninitialized memory into the loaded image.

### 7.4 Drop

`LoadedImage::drop` calls `boot::free_pages(self.image_base as u64, self.pages_allocated)`. This is only reached if the consumer drops the `LoadedImage` without calling `start_image` OR if `start_image`'s entry point returns. In both cases, we reclaim the pages.

---

## 8. Section copy

For each section in `pe.sections`:

```rust
let virt_addr = section.virtual_address as usize;   // relative to image_base
let virt_size = section.virtual_size as usize;
let raw_ptr = section.pointer_to_raw_data as usize; // offset within input bytes
let raw_size = section.size_of_raw_data as usize;

// Clamp raw_size to what's actually present in virt_size. PE may claim
// raw_size > virt_size for alignment padding; we only need virt_size bytes.
let copy_len = raw_size.min(virt_size);

// Bounds-check before trusting values.
if raw_ptr + copy_len > bytes.len() {
    return Err(PeLoadError::SectionOutOfBounds {
        section_index: i, detail: "raw range exceeds input"
    });
}
if virt_addr + virt_size > size_of_image {
    return Err(PeLoadError::SectionOutOfBounds {
        section_index: i, detail: "virtual range exceeds SizeOfImage"
    });
}

let src = &bytes[raw_ptr..raw_ptr + copy_len];
// SAFETY: dst is inside our allocation (checked above); src is a valid slice;
// ranges do not overlap (input bytes come from a Vec/slice, destination is
// a freshly allocated page range).
unsafe {
    let dst = (base as *mut u8).add(virt_addr);
    core::ptr::copy_nonoverlapping(src.as_ptr(), dst, copy_len);
}
```

Sections with `raw_size == 0` but `virt_size > 0` (BSS) are skipped — the pre-zeroed pages already satisfy them.

---

## 9. Relocation algorithm

Reference: [shim's `pe-relocate.c`](https://github.com/rhboot/shim/blob/main/pe-relocate.c), distilled.

### 9.1 Block iteration

The base relocation directory (`DataDirectories[5]`) points at a sequence of relocation blocks. Each block starts with:

```
struct IMAGE_BASE_RELOCATION {
    uint32_t VirtualAddress;  // RVA relative to image base
    uint32_t SizeOfBlock;     // total size including this header
}
```

...followed by `(SizeOfBlock - 8) / 2` entries, each a 16-bit value:

```
bits 15..12: type  (IMAGE_REL_BASED_*)
bits 11..0:  offset within the block's 4 KiB page
```

To apply a fixup: compute address = `image_base + block.VirtualAddress + entry.offset`. Apply the type-specific patch using `delta = actual_base - preferred_base`.

### 9.2 Supported types

| Type (name / number) | Field width | Action |
|---|---|---|
| `IMAGE_REL_BASED_ABSOLUTE` / 0 | — | No-op. Used for 2-byte block-padding. |
| `IMAGE_REL_BASED_HIGH` / 1 | 16 bits | `*u16_at_addr = ((*u16_at_addr << 16) + delta) >> 16` (high half of 32-bit) — kernels don't emit this |
| `IMAGE_REL_BASED_LOW` / 2 | 16 bits | `*u16_at_addr = (*u16_at_addr + delta) & 0xFFFF` (low half) — kernels don't emit this |
| `IMAGE_REL_BASED_HIGHLOW` / 3 | 32 bits | `*u32_at_addr = *u32_at_addr + delta` — used in 32-bit PEs only; x86_64 shouldn't emit |
| `IMAGE_REL_BASED_DIR64` / 10 | 64 bits | `*u64_at_addr = *u64_at_addr + delta` — **the type kernels + UKIs emit** |

All other types (`SECTION`, `REL`, `THUMB_MOV32`, various ARM/RISC-V-specific) are refused with `UnsupportedRelocationType(n)`.

### 9.3 Skip when rebased == false

If we got the preferred base (§7.2), `delta == 0` and **no relocation is needed**. We short-circuit the entire relocation pass — saves time and prevents subtle bugs around "applying a zero delta."

### 9.4 Implementation sketch

```rust
fn apply_relocations(
    image_base: *mut u8,
    image_size: usize,
    preferred_base: u64,
    actual_base: u64,
    reloc_table: &DataDirectory,
    reloc_bytes: &[u8],
) -> Result<(), PeLoadError> {
    let delta = actual_base.wrapping_sub(preferred_base);
    if delta == 0 { return Ok(()); }

    let mut cursor = 0usize;
    while cursor + 8 <= reloc_bytes.len() {
        // Parse block header.
        let virt_addr = u32::from_le_bytes(reloc_bytes[cursor..cursor+4].try_into().unwrap());
        let size_of_block = u32::from_le_bytes(reloc_bytes[cursor+4..cursor+8].try_into().unwrap()) as usize;

        if size_of_block < 8 {
            return Err(PeLoadError::RelocationMalformed("SizeOfBlock < 8"));
        }
        if cursor + size_of_block > reloc_bytes.len() {
            return Err(PeLoadError::RelocationMalformed("block extends past directory"));
        }

        let entry_count = (size_of_block - 8) / 2;
        for i in 0..entry_count {
            let entry_offset = cursor + 8 + i * 2;
            let entry = u16::from_le_bytes(reloc_bytes[entry_offset..entry_offset+2].try_into().unwrap());
            let reloc_type = (entry >> 12) as u16;
            let within_page = (entry & 0x0FFF) as u32;

            let target_rva = virt_addr + within_page;
            if target_rva as usize + 8 > image_size {
                return Err(PeLoadError::RelocationMalformed("fixup target out of bounds"));
            }

            match reloc_type {
                0 => {} // IMAGE_REL_BASED_ABSOLUTE — no-op
                10 => {
                    // IMAGE_REL_BASED_DIR64
                    // SAFETY: target_rva bounds-checked to be <= image_size - 8.
                    // Aligned reads/writes would be nicer but PE doesn't guarantee
                    // alignment; use unaligned operations.
                    unsafe {
                        let target = image_base.add(target_rva as usize) as *mut u64;
                        let v = core::ptr::read_unaligned(target);
                        core::ptr::write_unaligned(target, v.wrapping_add(delta));
                    }
                }
                3 => {
                    // IMAGE_REL_BASED_HIGHLOW (32-bit) — shouldn't appear in x86_64 but
                    // implement for completeness. delta truncated to u32.
                    unsafe {
                        let target = image_base.add(target_rva as usize) as *mut u32;
                        let v = core::ptr::read_unaligned(target);
                        core::ptr::write_unaligned(target, v.wrapping_add(delta as u32));
                    }
                }
                1 | 2 => {
                    // HIGH / LOW — kernels don't emit these; supporting them is
                    // trivial but yagni. Refuse with specific error.
                    return Err(PeLoadError::UnsupportedRelocationType(reloc_type));
                }
                _ => return Err(PeLoadError::UnsupportedRelocationType(reloc_type)),
            }
        }

        cursor += size_of_block;
    }
    Ok(())
}
```

### 9.5 Alignment

x86_64 can read/write 64-bit values at any 2-byte alignment without a fault. But to be portable to aarch64 (where unaligned access can trap depending on firmware configuration), we use `read_unaligned`/`write_unaligned`. Per-call cost is negligible; kernels typically have <100K relocations and we're not in the hot path for the running system.

---

## 10. `LoadedImageProtocol` installation

Linux kernels query `LoadedImageProtocol` on their own image handle to discover:

- `image_base` — where they were loaded (needed for self-relocation decisions the kernel makes internally)
- `image_size` — extent of the loaded image
- `load_options` / `load_options_size` — cmdline bytes as UTF-16
- `device_handle` — which device the image came from
- `file_path` — the path on that device

Without a correct `LoadedImageProtocol`, the EFI stub in Linux often panics very early or booots with garbage cmdline.

### 10.1 Installing the protocol

```rust
let loaded_image_data = LoadedImageProtocolData {
    revision: 0x1000,  // LOADED_IMAGE_PROTOCOL_REVISION
    parent_handle: uefi::boot::image_handle(),
    system_table: system_table_raw_ptr(),
    device_handle: /* the Volume's device handle or None if synthetic */,
    file_path: /* FilePath DevicePath or EndEntire for synthetic */,
    reserved: core::ptr::null_mut(),
    load_options_size: (load_options.len() * 2) as u32,
    load_options: load_options.as_ptr() as *mut _,
    image_base: base as *mut _,
    image_size: size_of_image as u64,
    image_code_type: MemoryType::LOADER_CODE,
    image_data_type: MemoryType::LOADER_DATA,
    unload: None,  // we're the loader; child doesn't unload itself
};

// SAFETY: the protocol data is allocated via Box::leak and lives until
// the child image exits. `install_protocol_interface` returns a handle
// that represents this protocol instance.
let handle = unsafe {
    uefi::boot::install_protocol_interface::<LoadedImage>(
        None,  // create new handle
        &loaded_image_data as *const _ as *mut _,
    )
}.map_err(PeLoadError::ProtocolInstallFailed)?;
```

The exact Rust type for `LoadedImageProtocolData` is provided by `uefi-rs`'s `LoadedImage` protocol wrapper; we access the raw underlying `uefi_raw::protocol::loaded_image::LoadedImage` to construct it manually because uefi-rs's safe wrapper assumes firmware allocated the struct.

### 10.2 Device path construction

For kernels loaded from ext4 `/boot/vmlinuz-X`, the device path is:

```
HardDrive(part_num, GPT-GUID-of-partition) / FilePath("\\vmlinuz-X")
```

We construct this from the `Volume`'s partition GUID (SDS-1 `VolumeIdentity.partition_guid`) plus the path the consumer provided. If the path-component is longer than 255 characters, we truncate (Linux's EFI stub handles truncated file_path fine; it's informational).

### 10.3 Protocol lifecycle

The installed protocol lives until the child image either:

- Calls `ExitBootServices` (at which point the protocol is logically invalidated, but the kernel has already cached what it needs),
- Or calls `Exit()` which returns control to us, at which point our `Drop` for `LoadedImage` uninstalls the protocol.

For kernels that successfully jump to real runtime (normal case), the protocol is never uninstalled — it persists past `ExitBootServices` as a memory leak that's reclaimed by the kernel reusing all firmware memory anyway.

---

## 11. Entry-point invocation

```rust
pub(crate) unsafe fn start_image(image: LoadedImage) -> uefi::Status {
    // Calling convention: extern "efiapi" matches UEFI ABI (x86_64 MS ABI
    // with shadow space; aarch64 AAPCS64).
    //
    // SAFETY: Caller has verified the image bytes are trusted (via ShimLock
    // or policy). image.entry_point was validated to be inside the allocated
    // image range and in an executable section. image.handle was installed
    // above and represents a valid LoadedImageProtocol.
    let status = (image.entry_point)(image.handle, system_table_raw_ptr());

    // If we reach here, the child image returned control to us. Release
    // resources via Drop.
    drop(image);
    status
}
```

### 11.1 What the kernel does with control

It looks up its `LoadedImageProtocol` on the image handle, reads load options, relocates internally if needed, calls `ExitBootServices`, transitions to kernel mode. None of this is LamBoot's concern after `start_image` returns (usually it doesn't return — `ExitBootServices` makes return impossible).

### 11.2 Drivers returning to us

EFI drivers (subsystem 11/12) often return `EFI_SUCCESS` from their entry point after installing their service protocols. The returned status tells us whether the driver initialized successfully. This is the normal flow for LamBoot's own ext4 UEFI driver (though SDS-2 makes it mostly unnecessary).

---

## 12. Trust-log integration

Every successful `load_pe` call produces data for a `image_loaded_native` trust-log event. The loader returns the SHA-256 and size in `LoadedImage`; the caller emits the event:

```rust
let loaded = pe_loader::load_pe(&bytes, Some(&cmdline_u16), volume.identity().backend_tag)?;
trust_log.record(TrustEvent::new("image_loaded_native")
    .with_path(path.as_str())
    .with_size(bytes.len())
    .with_sha256_hex(&loaded.sha256_hex())
    .with_verified_via(volume.identity().backend_tag)
    .with_status("SUCCESS"));
```

The `image_loaded_native` event is new in v0.9.x and is spec'd by SDS-4. Fields: `event=image_loaded_native`, `path`, `size`, `sha256`, `verified_via=<backend_tag>`, `status=SUCCESS`. Failure path emits `image_load_failed` with a specific `PeLoadError::as_log_token()`.

---

## 13. Testing plan

### 13.1 Unit tests (host, mock UEFI boot services)

A `mocks::boot` module that satisfies just enough of the uefi-rs `boot::` API to test the loader without real firmware. Critical tests:

1. **Valid minimal PE.** Construct a PE from scratch (DOS+NT+OptHdr+one .text section, entry point at section start, no imports/TLS/reloc). `load_pe` returns `Ok(LoadedImage)`. `sha256` matches `sha2::Sha256` of the input bytes.
2. **DOS signature wrong.** Bytes[0..2] ≠ "MZ" → `BadDosSignature`.
3. **PE signature wrong.** Bytes at e_lfanew ≠ "PE\0\0" → `BadNtSignature`.
4. **PE32 refused.** Optional header magic = 0x10B → `UnsupportedPe32`.
5. **Wrong machine.** Machine = 0x1C0 (ARM32) on x86_64 build → `UnsupportedMachine`.
6. **Wrong subsystem.** Subsystem = 3 (WindowsCUI) → `UnsupportedSubsystem`.
7. **SizeOfImage too large.** Claimed 1 GB → `ImageTooLarge`.
8. **NumberOfSections too large.** Claimed 10000 → `TooManySections`.
9. **Has imports.** Import directory size ≠ 0 → `HasImports`.
10. **Has TLS.** TLS directory size ≠ 0 → `HasTlsCallbacks`.
11. **Section raw range out of bounds.** PointerToRawData + SizeOfRawData > byte length → `SectionOutOfBounds`.
12. **Section virtual range out of bounds.** VirtualAddress + VirtualSize > SizeOfImage → `SectionOutOfBounds`.
13. **Entry point out of bounds.** AddressOfEntryPoint ≥ SizeOfImage → `EntryPointOutOfBounds`.
14. **Allocation failure.** Mock allocator returns OutOfResources → `AllocationFailed`.
15. **Relocation happy path.** PE with one DIR64 reloc; actual_base ≠ preferred_base; the pointed-to u64 is patched by exactly `delta`.
16. **Relocation skip on zero delta.** actual_base == preferred_base; reloc bytes UNCHANGED in memory (even if malformed).
17. **Malformed reloc block.** SizeOfBlock < 8 → `RelocationMalformed`.
18. **Unsupported reloc type.** Type = 7 (THUMB_MOV32) → `UnsupportedRelocationType(7)`.
19. **SHA-256 stability.** Same input → same SHA regardless of allocation base.

### 13.2 Real-binary corpus tests

A test harness that loads every binary in `tests/pe-corpus/`:

- `ubuntu-vmlinuz-6.17.0-22-generic` (captured from VM 120)
- `debian-vmlinuz-6.19.11+deb14-amd64` (captured from VM 100)
- `fedora-vmlinuz-6.19.12-200.fc43.x86_64` (captured from VM 201)
- `sd-stub-uki-sample.efi`
- `lamboot-lambootx64.efi` (our own binary)
- `refind-ext4_x64.efi`

For each: parse headers successfully, validate §5.1, compute SHA-256, run `apply_relocations` on a known dummy delta, verify no panics. Actual invocation happens in QEMU integration (§13.3).

### 13.3 QEMU integration

A new `run-qemu-native-pe-load.sh` that builds LamBoot with an added diagnostic module, boots in QEMU, and exercises:

1. Load a trivially-small test PE from the ESP via native loader; start_image returns `SUCCESS`; boot flow continues.
2. Load Ubuntu's kernel from an ext4 partition via SDS-2 + native loader; kernel EFI stub runs; captures its own EFI-stub log via a known magic string, visible on serial.
3. Load a sd-stub UKI the same way.
4. Load a PE that fails validation (crafted); verify specific `PeLoadError` surfaces in the trust log.

### 13.4 Fuzzing

`cargo fuzz` target: `load_pe(Vec<u8>)`. Seeds from the corpus. Targets:
- No panic on any input ≤ 16 MiB.
- No infinite loop on malformed reloc chain.
- No unbounded allocation (allocator wrapper caps at 256 MiB; the loader must honor this).

Minimum 30 minutes fuzzing per release candidate.

### 13.5 Live-boot acceptance

The release-validating test: **VM 122 Fedora Sway**, the v0.8.3 FAIL case, boots successfully after SDS-2 + SDS-3 are implemented. LamBoot discovers Fedora's BLS entries on ext4 /boot (SDS-2 + SDS-5), loads the kernel via native PE loader (SDS-3), boots Fedora — without ever calling `BS->LoadImage` on anything.

---

## 14. Performance budget

For a 30 MB kernel:

| Phase | Budget |
|---|---|
| goblin parse headers | 5 ms |
| Validation (§5.1) | <1 ms |
| Allocate 7680 pages | 20 ms |
| Zero 30 MB | 30 ms |
| Copy sections | 30 ms |
| Apply relocations (~100K DIR64) | 50 ms |
| SHA-256 of input | 80 ms (software, no AVX) |
| Install LoadedImageProtocol | 5 ms |
| **Total** | **~225 ms** |

Plus SDS-2's ~225 ms to read the bytes = ~450 ms kernel-to-entry-point. Adequate for sub-3s boot.

If SHA-256 dominates (it will at first, software impl), consider: compute it lazily only when the trust log needs it (sometimes it doesn't — policy-off configs skip the log). Or adopt `sha2` with CPU-detected AVX2 once LamBoot switches to nightly stable-ish. Not a v0.9.0 concern.

---

## 15. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| Preferred-base collision forces relocation frequently | Low | PE kernels are linked at very high addresses (0xFFFFFFFF80000000-ish in kernel space); UEFI boot services don't usually allocate there. Rebasing is cheap (~50 ms). |
| `install_protocol_interface` on LoadedImage must handle uefi-rs version mismatch | Medium | Pin uefi-rs; test on the exact version. If uefi-rs doesn't expose raw install, use `table::boot_services_raw()` and call through the raw vtable directly. |
| Alignment traps on aarch64 | Low | We use `read_unaligned`/`write_unaligned` consistently (§9.5). |
| Kernel reads `LoadedImage.device_handle` and crashes if it's null | Medium | Always populate a real device handle from the Volume that sourced the bytes. Synthetic handles (for in-memory testing) use `boot::image_handle()` as a benign fallback. |
| `ExitBootServices` timing with pending protocol uninstall | Low | Protocol is uninstalled at `LoadedImage::drop`, which only runs if the entry point returns. Kernels call ExitBootServices before returning (they don't return), so the leak is acceptable. |
| Fuzzing finds goblin bugs | Medium | Report upstream. Pin to pre-bug version. No local patching. |
| 30-minute fuzz budget insufficient | Low | Can extend in CI to 8-hour runs. |
| Alternative: adopt sd-boot's pe_loader.c directly via FFI | Rejected | Adds C dependency, defeats memory-safe story. Pure-Rust is the v1.0 promise. |

---

## 16. Acceptance criteria

SDS-3 implementation is complete when all of the following are true:

- [ ] `goblin = "=0.10.5"` + `sha2 = "=0.10.8"` in `lamboot-core/Cargo.toml`, both `default-features = false`.
- [ ] `lamboot-core/src/pe_loader.rs` implements `load_pe`, `load_pe_stream`, `start_image`, `LoadedImage`, `PeLoadError`.
- [ ] All §13.1 unit tests pass.
- [ ] §13.2 real-binary corpus parses without error; §5.1 validation passes for every binary.
- [ ] §13.3 QEMU integration passes: trivial PE loads + runs, Fedora kernel loads + runs via native path.
- [ ] §13.4 fuzzing runs 30 min with no panics or unbounded allocations.
- [ ] §13.5 acceptance: VM 122 Fedora Sway boots through LamBoot using native path (SDS-2 + SDS-3).
- [ ] No `#[allow(clippy::…)]` introduced; every `unsafe` block has a `SAFETY:` comment.
- [ ] Pre-commit hook (fmt + clippy + check) passes.
- [ ] `CHANGELOG.md` entry under Unreleased.

---

## 17. Sign-off

This spec is **implementation-ready** when:
- Founder has reviewed and explicitly acknowledged.
- SDS-1 AND SDS-2 have landed AND been implemented.
- No open question in §15 above "Medium" severity.

Implementation PR(s) must cite this SDS in their description and must not deviate from §2, §5, §9, §10 without a documented amendment.

---

## Appendix A — PE format references

- PE/COFF spec (Microsoft): https://learn.microsoft.com/windows/win32/debug/pe-format
- shim's `pe-relocate.c`: https://github.com/rhboot/shim/blob/main/pe-relocate.c
- goblin PE module: https://docs.rs/goblin/0.10.5/goblin/pe/
- ext4-view uefibench pattern: https://github.com/nicholasbishop/ext4-view-rs/tree/main/xtask/uefibench
- systemd-boot pe-loader (C, reference for design choices): https://github.com/systemd/systemd/tree/main/src/boot

## Appendix B — Related LamBoot references

- `docs/specs/SPEC-FS-BACKEND-TRAIT.md` — byte source types (Volume, FsStream)
- `docs/specs/SPEC-EXT4-INTEGRATION.md` — ext4 native reader
- `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` (SDS-4, next) — where `ShimLock::Verify` happens
- `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` §5.7 — adoption rationale
- `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` — why this loader exists
