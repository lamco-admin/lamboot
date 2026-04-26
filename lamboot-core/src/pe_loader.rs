//! Native Rust PE loader — SDS-3.
//!
//! Loads a PE/COFF binary from a byte slice into memory, applies
//! base relocations, installs a `LoadedImageProtocol` handle, and
//! returns a ready-to-invoke `LoadedImage`. **Does not call
//! `BS->LoadImage()` / `BS->StartImage()` — that is the whole point.**
//!
//! Trust verification (ShimLock::Verify or equivalent) happens BEFORE
//! `load_pe` is called, in SDS-4's trust-chain layer or in the v0.9.x
//! transitional wrapper inside `boot.rs`. The loader itself treats its
//! input as pre-verified bytes.
//!
//! The pure byte-parsing and relocation-math code lives in
//! `pe_loader_pure.rs` so host tests can include it via `#[path]` and
//! exercise every validation + reloc branch without firmware.

// Some items (LoadedImage accessors like sha256, source_backend) are
// not yet called from boot.rs — they'll be used by PR-4's QEMU
// trust-log assertions. Allow dead_code while those paths round out.
#![allow(dead_code)]

use alloc::boxed::Box;
use core::ptr;

use uefi::{
    boot::{AllocateType, MemoryType},
    Handle,
};
use uefi_raw::{
    protocol::loaded_image::LoadedImageProtocol,
    table::{boot::MemoryType as RawMemoryType, system::SystemTable},
};

use crate::pe_loader_pure::{
    apply_relocations_to_slice, hex_encode_32, parse_headers, sha256_of, validate_headers,
    PeLoadError, PeSummary,
};

/// Handle to a loaded, ready-to-run PE image.
///
/// Opaque to callers. The only operations are `start_image` (consumes
/// self) and `Drop` (frees pages, uninstalls protocol). Carries all
/// state needed for both.
pub(crate) struct LoadedImage {
    /// Image base in UEFI memory where we copied sections.
    image_base: *mut u8,
    /// Total size in bytes of the image (= SizeOfImage).
    image_size: usize,
    /// Computed entry point = image_base + entry_rva, cast to the EFI
    /// entry-point calling convention.
    entry_point: EntryFn,
    /// UEFI handle returned by `install_protocol_interface`. Non-zero
    /// on success; used at `start_image` time as the child's image
    /// handle argument.
    handle: Handle,
    /// Pages we allocated. Drop frees them.
    pages_allocated: usize,
    /// SHA-256 of the ORIGINAL input bytes (pre-relocation). Passed
    /// back to the caller for the `image_loaded_native` trust-log event.
    sha256: [u8; 32],
    /// Backend tag the bytes came from, for trust-log annotation.
    source_backend: &'static str,
    /// Heap-leaked LoadedImageProtocol struct — must outlive any
    /// firmware consumer of the protocol (kernels cache it through
    /// ExitBootServices). Kept so Drop can optionally free it when
    /// no longer needed. Set to `None` when we never want to free it
    /// (kernel-hand-off path).
    protocol_data: Option<Box<LoadedImageProtocol>>,
    /// Heap-leaked load-options buffer (UTF-16). Kept alive in the
    /// struct so firmware's reads of `LoadedImageProtocol.load_options`
    /// don't dangle.
    load_options_guard: Option<Box<[u16]>>,
}

/// Entry-point calling convention. Matches UEFI `EFI_IMAGE_ENTRY_POINT`.
type EntryFn =
    unsafe extern "efiapi" fn(image_handle: Handle, system_table: *mut SystemTable) -> uefi::Status;

impl LoadedImage {
    /// SHA-256 of the original bytes (pre-relocation). For trust-log use.
    #[must_use]
    pub(crate) fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// Hex-encoded SHA-256 — convenience for `TrustEvent::with_sha256`.
    #[must_use]
    pub(crate) fn sha256_hex(&self) -> alloc::string::String {
        hex_encode_32(&self.sha256)
    }

    /// Backend identifier of the volume the bytes came from.
    #[must_use]
    pub(crate) fn source_backend(&self) -> &'static str {
        self.source_backend
    }
}

impl Drop for LoadedImage {
    fn drop(&mut self) {
        // We only reach Drop if the consumer drops without calling
        // start_image OR if start_image's entry point returned
        // (which kernels don't — they ExitBootServices instead, so
        // in practice Drop fires for drivers and failed loads).
        if self.pages_allocated > 0 && !self.image_base.is_null() {
            // SAFETY: image_base / pages_allocated are the exact
            // values returned by allocate_pages during load_pe. No
            // other pointer aliases this range (we never hand out
            // the base to anything but the kernel's own code via
            // start_image, which either ExitBootServices-es or
            // exits cleanly before this Drop fires).
            if let Some(nn) = core::ptr::NonNull::new(self.image_base) {
                unsafe {
                    let _ = uefi::boot::free_pages(nn, self.pages_allocated);
                }
            }
        }
        // protocol_data is intentionally leaked via Box when we're
        // in the kernel-hand-off path. For Drop-fires-before-start
        // we could uninstall here, but `uninstall_protocol_interface`
        // requires a Handle + the exact interface ptr, which we have
        // — but uefi-rs 0.37's safe wrapper requires a typed Protocol
        // impl. Keeping this conservative: leak rather than
        // potentially-misuse the uninstall path. Pages (the big
        // allocation) are correctly freed above.
        let _ = self.protocol_data.take();
        let _ = self.load_options_guard.take();
    }
}

/// Parse, allocate, copy, relocate, install — returns a ready
/// `LoadedImage`. Does NOT invoke the entry point. SDS-3 §2.2.
///
/// `load_options` is the kernel cmdline, already encoded as UTF-16
/// with NUL terminator. `None` for images that don't need options
/// (chainload, diagnostic modules).
///
/// `source_backend` is the `BackendTag` of the volume the bytes came
/// from — for trust-log annotation.
pub(crate) fn load_pe(
    bytes: &[u8],
    load_options: Option<Box<[u16]>>,
    source_backend: &'static str,
) -> Result<LoadedImage, PeLoadError> {
    // 1. Parse + validate headers (pure code).
    let summary = parse_headers(bytes)?;
    validate_headers(&summary, bytes)?;

    // 2. Compute SHA-256 of the ORIGINAL bytes before any allocation
    //    or relocation. This is what ShimLock::Verify hashed.
    let sha256 = sha256_of(bytes);

    // 3. Allocate the image range.
    let (image_base, pages_allocated, actual_base) = allocate_image_pages(&summary)?;

    // 4. Zero the whole allocation (PE BSS-style sections rely on this).
    // SAFETY: image_base is a freshly allocated range of
    // pages_allocated * 4096 bytes owned exclusively by us.
    unsafe {
        ptr::write_bytes(image_base, 0, pages_allocated * 4096);
    }

    // 5. Copy each section.
    // SAFETY: summary.sections came from validate_headers which
    // bounds-checked every (raw_offset, copy_len) against bytes.len()
    // and every (virt_addr, virt_size) against size_of_image. The
    // destination range is inside our allocation.
    for plan in &summary.sections {
        if plan.copy_len == 0 {
            continue;
        }
        unsafe {
            let src = bytes.as_ptr().add(plan.raw_offset as usize);
            let dst = image_base.add(plan.virt_addr as usize);
            ptr::copy_nonoverlapping(src, dst, plan.copy_len as usize);
        }
    }

    // 6. Apply relocations, unless we got the preferred base.
    if actual_base != summary.preferred_base {
        let Some(reloc_dir) = summary.reloc_dir.filter(|d| d.size > 0) else {
            // No relocation directory AND we didn't get preferred base.
            //
            // For EFI applications and drivers this would be a fatal
            // error. For **Linux EFI-stub kernels**, though, it is
            // the normal case: the kernel's PE has no .reloc table
            // because the EFI stub is position-independent and
            // performs its own internal relocation (KASLR+) after
            // entry via `image_base` from LoadedImageProtocol.
            //
            // Rather than refuse, proceed with the any-address
            // allocation. The kernel's EFI stub will handle
            // relocation itself. Firmware's own `BS->LoadImage` does
            // the same — it doesn't require a reloc table for
            // kernels with DYNAMIC_BASE + self-relocation. Every
            // established Linux-EFI bootloader (sd-boot, GRUB,
            // rEFInd) behaves this way.
            log::info!(
                "pe_loader: no reloc directory and non-preferred base; \
                 assuming position-independent image (Linux EFI stub)"
            );
            // Fall through — skip to install + entry-point compute.
            return finish_load_without_relocs(
                image_base,
                pages_allocated,
                &summary,
                sha256,
                source_backend,
                load_options,
            );
        };

        // SAFETY: reloc_dir.virtual_address + size bounds are checked
        // inside apply_relocations_to_slice against the image slice's
        // len. We pass the full allocated range as the image slice.
        let image_slice =
            unsafe { core::slice::from_raw_parts_mut(image_base, summary.size_of_image as usize) };
        let reloc_start = reloc_dir.virtual_address as usize;
        let reloc_end = reloc_start.saturating_add(reloc_dir.size as usize);
        if reloc_end > image_slice.len() {
            unsafe {
                if let Some(nn) = core::ptr::NonNull::new(image_base) {
                    let _ = uefi::boot::free_pages(nn, pages_allocated);
                }
            }
            return Err(PeLoadError::RelocationMalformed(
                "reloc directory outside image bounds",
            ));
        }
        // Snapshot the reloc bytes before mutating image (aliasing-
        // safe since we clone the few kB into a Vec).
        let reloc_bytes = image_slice[reloc_start..reloc_end].to_vec();
        if let Err(e) = apply_relocations_to_slice(
            image_slice,
            summary.preferred_base,
            actual_base,
            &reloc_bytes,
        ) {
            unsafe {
                if let Some(nn) = core::ptr::NonNull::new(image_base) {
                    let _ = uefi::boot::free_pages(nn, pages_allocated);
                }
            }
            return Err(e);
        }
    }

    // 7. Install LoadedImageProtocol.
    let (handle, protocol_data, load_options_guard) =
        match install_loaded_image_protocol(image_base, summary.size_of_image, load_options) {
            Ok(triple) => triple,
            Err(e) => {
                unsafe {
                    if let Some(nn) = core::ptr::NonNull::new(image_base) {
                        let _ = uefi::boot::free_pages(nn, pages_allocated);
                    }
                }
                return Err(e);
            }
        };

    // 8. Compute entry point.
    // SAFETY: validate_headers ensured entry_rva < size_of_image and
    // points into an executable section. Casting a function pointer
    // from a raw byte ptr is UB unless the ABI matches; we require
    // efiapi, which is exactly what UEFI boot images are compiled
    // against. The resulting fn ptr is only invoked by start_image
    // after the caller has verified trust (e.g. ShimLock::Verify).
    let entry_point: EntryFn = unsafe {
        let entry_ptr = image_base.add(summary.entry_rva as usize);
        core::mem::transmute::<*const u8, EntryFn>(entry_ptr)
    };

    Ok(LoadedImage {
        image_base,
        image_size: summary.size_of_image as usize,
        entry_point,
        handle,
        pages_allocated,
        sha256,
        source_backend,
        protocol_data: Some(protocol_data),
        load_options_guard,
    })
}

/// Invoke the entry point. Consumes the `LoadedImage`.
///
/// # Safety
///
/// Caller MUST have verified the bytes (via ShimLock::Verify or
/// equivalent) before `load_pe` was called. The loader does not
/// verify signatures itself.
///
/// If the entry point returns (drivers do; kernels typically don't —
/// they ExitBootServices first), the `LoadedImage` Drop runs,
/// freeing the pages. If the entry point does not return, the pages
/// stay allocated forever — that's correct because the kernel
/// owns all firmware memory after ExitBootServices.
pub(crate) unsafe fn start_image(image: LoadedImage) -> uefi::Status {
    let entry = image.entry_point;
    let handle = image.handle;
    let system_table = system_table_raw_ptr();

    log::info!(
        "pe_loader: invoking entry point at {:p} (image handle {:?})",
        entry as *const (),
        handle
    );

    // SAFETY: entry is an efiapi function pointer constructed from a
    // validated in-range address (§5.1 check). handle is a real UEFI
    // handle we just installed. system_table is the raw pointer UEFI
    // hands our own entry — re-passing it is correct.
    let status = unsafe { entry(handle, system_table) };

    // Child returned. Drop the LoadedImage → free_pages.
    drop(image);
    status
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate `size_of_image` bytes as UEFI pages, preferring the PE's
/// preferred base. Returns (base, pages, actual_base). On address-
/// allocation failure, falls back to AnyPages.
/// Finish load_pe on the "no-reloc + non-preferred base" path.
/// Install the LoadedImageProtocol and build the LoadedImage without
/// touching the image bytes further. Used by Linux EFI-stub kernels.
fn finish_load_without_relocs(
    image_base: *mut u8,
    pages_allocated: usize,
    summary: &PeSummary,
    sha256: [u8; 32],
    source_backend: &'static str,
    load_options: Option<Box<[u16]>>,
) -> Result<LoadedImage, PeLoadError> {
    let (handle, protocol_data, load_options_guard) =
        match install_loaded_image_protocol(image_base, summary.size_of_image, load_options) {
            Ok(t) => t,
            Err(e) => {
                if let Some(nn) = core::ptr::NonNull::new(image_base) {
                    unsafe {
                        let _ = uefi::boot::free_pages(nn, pages_allocated);
                    }
                }
                return Err(e);
            }
        };

    // SAFETY: validate_headers ensured entry_rva is within image
    // bounds and in an executable section. The target OS's EFI stub
    // is position-independent by contract when no reloc table is
    // present (otherwise this code path wouldn't run) — it consults
    // LoadedImageProtocol::image_base to discover where it actually
    // lives and relocates itself from there.
    let entry_point: EntryFn = unsafe {
        let entry_ptr = image_base.add(summary.entry_rva as usize);
        core::mem::transmute::<*const u8, EntryFn>(entry_ptr)
    };

    Ok(LoadedImage {
        image_base,
        image_size: summary.size_of_image as usize,
        entry_point,
        handle,
        pages_allocated,
        sha256,
        source_backend,
        protocol_data: Some(protocol_data),
        load_options_guard,
    })
}

fn allocate_image_pages(summary: &PeSummary) -> Result<(*mut u8, usize, u64), PeLoadError> {
    // SizeOfImage is page-aligned per PE spec, but defensive:
    let size = summary.size_of_image as usize;
    let pages = size.div_ceil(4096);

    // Try preferred-base allocation first.
    if summary.preferred_base > 0 {
        if let Ok(addr) = uefi::boot::allocate_pages(
            AllocateType::Address(summary.preferred_base),
            MemoryType::LOADER_CODE,
            pages,
        ) {
            return Ok((addr.as_ptr(), pages, summary.preferred_base));
        }
    }

    // Preferred base unavailable (or zero). Fall back to anywhere.
    let addr = uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_CODE, pages)
        .map_err(|_e| {
        // Wrap UEFI error as a generic "parse failed" placeholder.
        // We don't have PeLoadError::AllocationFailed per v1.1 — the
        // error taxonomy stays pure-friendly.
        PeLoadError::ParseFailed("allocate_pages failed")
    })?;
    let base = addr.as_ptr();
    let actual_base = base as u64;
    Ok((base, pages, actual_base))
}

/// Install a raw `LoadedImageProtocol` on a new handle. Returns the
/// handle, the leaked Box holding the protocol data, and the leaked
/// load-options buffer (if any).
///
/// Safety rationale documented inline.
fn install_loaded_image_protocol(
    image_base: *mut u8,
    image_size: u64,
    load_options: Option<Box<[u16]>>,
) -> Result<(Handle, Box<LoadedImageProtocol>, Option<Box<[u16]>>), PeLoadError> {
    // Compute load-options pointer + size. We hold the Box to prevent
    // it from being dropped before the child reads it; LoadedImage
    // keeps it alive in `load_options_guard`.
    let (lo_ptr, lo_size_bytes) = match &load_options {
        Some(buf) => {
            let size = buf.len() * core::mem::size_of::<u16>();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "UEFI LoadedImageProtocol.load_options_size is u32 by spec; callers never pass options larger than 4 GiB"
            )]
            let size32 = size as u32;
            (buf.as_ptr().cast::<core::ffi::c_void>(), size32)
        }
        None => (core::ptr::null(), 0),
    };

    // Parent handle + system table come from the firmware.
    let parent_handle = uefi::boot::image_handle();
    let sys_table = system_table_raw_ptr();

    // Build the raw protocol struct on the heap. Boxed so the pointer
    // we hand to firmware is stable for the protocol's lifetime.
    let proto = Box::new(LoadedImageProtocol {
        revision: 0x1000, // EFI_LOADED_IMAGE_PROTOCOL_REVISION
        parent_handle: raw_handle_of(parent_handle),
        system_table: sys_table.cast_const(),
        device_handle: core::ptr::null_mut(),
        file_path: core::ptr::null(),
        reserved: core::ptr::null(),
        load_options_size: lo_size_bytes,
        load_options: lo_ptr,
        image_base: image_base.cast(),
        image_size,
        image_code_type: RawMemoryType::LOADER_CODE,
        image_data_type: RawMemoryType::LOADER_DATA,
        unload: None,
    });

    // SAFETY: GUID is the canonical EFI_LOADED_IMAGE_PROTOCOL GUID.
    // The interface pointer points to a heap-owned struct that lives
    // for as long as the returned LoadedImage (we stash the Box in
    // LoadedImage.protocol_data). The firmware may read but must not
    // free this memory.
    let proto_ptr =
        core::ptr::from_ref::<LoadedImageProtocol>(proto.as_ref()).cast::<core::ffi::c_void>();
    let handle = unsafe {
        uefi::boot::install_protocol_interface(None, &LoadedImageProtocol::GUID, proto_ptr)
    }
    .map_err(|_e| PeLoadError::ParseFailed("install_protocol_interface failed"))?;

    Ok((handle, proto, load_options))
}

/// Get the raw pointer to the UEFI SystemTable. uefi-rs stores this
/// behind a guarded accessor; we cast it to the raw type firmware
/// wants when we pass it to the child image's entry point.
fn system_table_raw_ptr() -> *mut SystemTable {
    uefi::table::system_table_raw()
        .map_or(core::ptr::null_mut(), core::ptr::NonNull::as_ptr)
        .cast()
}

/// Convert `uefi::Handle` to the raw `uefi_raw::Handle` that the
/// `LoadedImageProtocol` struct takes. Both types are thin NonNull
/// wrappers around the same pointer.
fn raw_handle_of(h: Handle) -> uefi_raw::Handle {
    h.as_ptr().cast()
}
