//! Layer: 4 (pure) — `EFI_LOAD_OPTION` / `BootOrder` byte codecs.
//!
//! The byte-exact, no-firmware pieces of the NVRAM self-install
//! (`boot_entry::ensure_named_boot_entry`), split out so `lamboot-fs-tests` can
//! include them via `#[path]` and exercise the encoding/parsing/ordering on the
//! host. Getting these wrong corrupts a firmware boot variable, so they are
//! tested directly rather than only through QEMU.
//!
//! `EFI_LOAD_OPTION` layout (UEFI spec 3.1.3), all integers little-endian:
//! ```text
//!   UINT32  Attributes
//!   UINT16  FilePathListLength   // bytes of FilePathList
//!   CHAR16  Description[]        // NUL-terminated UTF-16LE
//!   EFI_DEVICE_PATH FilePathList[FilePathListLength]
//!   UINT8   OptionalData[]       // remainder
//! ```

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

use alloc::{string::String, vec::Vec};

/// `LOAD_OPTION_ACTIVE` — the entry is eligible for the boot manager.
pub const LOAD_OPTION_ACTIVE: u32 = 0x0000_0001;

/// Encode an `EFI_LOAD_OPTION` for a `Boot####` variable.
///
/// `device_path` is the already-serialized FilePathList (a full device path,
/// terminated by its End node). `description` is rendered as NUL-terminated
/// UTF-16LE. `optional_data` is appended verbatim (usually empty).
#[must_use]
pub fn encode_load_option(
    attributes: u32,
    description: &str,
    device_path: &[u8],
    optional_data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&attributes.to_le_bytes());
    // FilePathListLength is a u16 by spec; device paths are far below 64 KiB.
    let fpl_len = u16::try_from(device_path.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&fpl_len.to_le_bytes());
    for unit in description.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes()); // NUL terminator
    out.extend_from_slice(device_path);
    out.extend_from_slice(optional_data);
    out
}

/// Extract the UTF-16LE description from a `Boot####` variable's bytes, used for
/// idempotency ("does a `LamBoot` entry already exist?"). Returns `None` if the
/// buffer is too short to contain the fixed header.
#[must_use]
pub fn parse_load_option_description(bytes: &[u8]) -> Option<String> {
    // 4 (attributes) + 2 (FilePathListLength) before the description.
    if bytes.len() < 6 {
        return None;
    }
    let mut units = Vec::new();
    let mut i = 6;
    while i + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        i += 2;
    }
    Some(String::from_utf16_lossy(&units))
}

/// Decode a `BootOrder` variable (array of little-endian `u16`) into numbers.
#[must_use]
pub fn decode_boot_order(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Encode boot numbers back into a `BootOrder` variable payload.
#[must_use]
pub fn encode_boot_order(order: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(order.len() * 2);
    for n in order {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out
}

/// Return `order` with `new` ensured present at the **front** (so LamBoot is
/// tried first). If `new` is already in `order`, returns it unchanged — never
/// duplicates and never reorders an existing entry.
#[must_use]
pub fn boot_order_with_front(order: &[u16], new: u16) -> Vec<u16> {
    if order.contains(&new) {
        return order.to_vec();
    }
    let mut out = Vec::with_capacity(order.len() + 1);
    out.push(new);
    out.extend_from_slice(order);
    out
}

/// Lowest `Boot####` number in `0..=max` not present in `used`, or `None` if the
/// range is exhausted. Bounds the idempotency scan to a sane window.
#[must_use]
pub fn first_free_boot_number(used: &[u16], max: u16) -> Option<u16> {
    (0..=max).find(|n| !used.contains(n))
}

/// Format a boot number as its `Boot####` variable name (uppercase hex, 4 wide).
#[must_use]
pub fn boot_var_name(num: u16) -> String {
    alloc::format!("Boot{num:04X}")
}
