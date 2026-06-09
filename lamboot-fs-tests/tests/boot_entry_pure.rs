//! Host unit tests for the NVRAM self-install byte codecs.
//!
//! The module lives at `lamboot-core/src/boot_entry_pure.rs` and is included
//! verbatim into this test crate via `#[path]`.
//! `boot_entry::ensure_named_boot_entry` builds the FilePathList device path
//! from firmware protocols and then delegates the `EFI_LOAD_OPTION` /
//! `BootOrder` byte layout here, so this is the authoritative test of the
//! firmware-variable encoding — a wrong byte corrupts a real `Boot####` var.

use lamboot_fs_tests::boot_entry_pure::{
    boot_order_with_front, boot_var_name, decode_boot_order, encode_boot_order, encode_load_option,
    first_free_boot_number, parse_load_option_description, LOAD_OPTION_ACTIVE,
};

#[test]
fn load_option_header_is_little_endian_and_well_formed() {
    // device path = a trivial 4-byte End node stand-in; description "LamBoot".
    let dp = [0x7f, 0xff, 0x04, 0x00];
    let opt = encode_load_option(LOAD_OPTION_ACTIVE, "LamBoot", &dp, &[]);

    // Attributes (u32 LE) = 0x00000001.
    assert_eq!(&opt[0..4], &[0x01, 0x00, 0x00, 0x00]);
    // FilePathListLength (u16 LE) = device-path length (4).
    assert_eq!(&opt[4..6], &[0x04, 0x00]);
    // Description: "LamBoot" as UTF-16LE then a NUL u16.
    let mut desc = Vec::new();
    for u in "LamBoot".encode_utf16() {
        desc.extend_from_slice(&u.to_le_bytes());
    }
    desc.extend_from_slice(&[0, 0]);
    assert_eq!(&opt[6..6 + desc.len()], desc.as_slice());
    // FilePathList immediately follows the NUL-terminated description.
    assert_eq!(&opt[6 + desc.len()..], &dp);
}

#[test]
fn description_round_trips_through_encode_then_parse() {
    let dp = [0x7f, 0xff, 0x04, 0x00];
    let opt = encode_load_option(LOAD_OPTION_ACTIVE, "LamBoot", &dp, &[]);
    assert_eq!(
        parse_load_option_description(&opt).as_deref(),
        Some("LamBoot")
    );
}

#[test]
fn parse_description_rejects_a_too_short_buffer() {
    assert_eq!(parse_load_option_description(&[0x01, 0x00, 0x00]), None);
}

#[test]
fn boot_order_round_trips() {
    let order = [0x0002u16, 0x0000, 0x0001];
    let bytes = encode_boot_order(&order);
    assert_eq!(bytes, vec![0x02, 0x00, 0x00, 0x00, 0x01, 0x00]);
    assert_eq!(decode_boot_order(&bytes), order);
}

#[test]
fn decode_boot_order_ignores_a_trailing_odd_byte() {
    // Defensive: a malformed odd-length payload must not panic.
    assert_eq!(decode_boot_order(&[0x02, 0x00, 0x09]), vec![0x0002]);
}

#[test]
fn front_loads_a_new_entry_without_duplicating_or_reordering() {
    let order = [0x0000u16, 0x0001];
    // New entry goes to the front.
    assert_eq!(
        boot_order_with_front(&order, 0x0002),
        vec![0x0002, 0x0000, 0x0001]
    );
    // Already present → unchanged (no duplicate, existing order preserved).
    assert_eq!(boot_order_with_front(&order, 0x0001), vec![0x0000, 0x0001]);
}

#[test]
fn first_free_boot_number_picks_the_lowest_gap() {
    assert_eq!(first_free_boot_number(&[0, 1, 3], 0xFF), Some(2));
    assert_eq!(first_free_boot_number(&[], 0xFF), Some(0));
    // Exhausted range yields None rather than panicking.
    assert_eq!(first_free_boot_number(&[0, 1, 2], 2), None);
}

#[test]
fn boot_var_name_is_four_wide_uppercase_hex() {
    assert_eq!(boot_var_name(2), "Boot0002");
    assert_eq!(boot_var_name(0x1A), "Boot001A");
    assert_eq!(boot_var_name(0xFFFF), "BootFFFF");
}
