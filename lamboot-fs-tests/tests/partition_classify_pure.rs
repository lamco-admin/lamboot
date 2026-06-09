//! Host unit tests for legacy-MBR partition `os_type` mountability.
//!
//! The module lives at `lamboot-core/src/partition_classify_pure.rs` and is
//! included verbatim into this test crate via `#[path]`.
//! `partitions::scan_discoverable_partitions` reads the real `os_type` byte out
//! of the firmware's `MbrPartitionRecord` and delegates the keep/skip decision
//! here, so this is the authoritative test of which legacy-MBR partition types
//! LamBoot superblock-probes when booting a BIOS-installed (msdos-table) disk.
//!
//! The motivating case: RHEL 9.7 (VM 166) — `sda` is an msdos disk with `sda1`
//! = `0x83` Linux (XFS `/boot`) and `sda2` = `0x8E` Linux LVM. Both must be
//! retained; the EFI/extended/protective/empty slots must not.

use lamboot_fs_tests::partition_classify_pure::mbr_os_type_is_mountable;

#[test]
fn linux_filesystem_partition_is_mountable() {
    // 0x83 Linux — the RHEL/CentOS/Rocky/Alma /boot on a BIOS install.
    assert!(mbr_os_type_is_mountable(0x83));
}

#[test]
fn linux_lvm_partition_is_mountable() {
    // 0x8E Linux LVM — RHEL root lives inside; the LVM probe consumes it.
    assert!(mbr_os_type_is_mountable(0x8E));
}

#[test]
fn data_filesystem_types_are_mountable() {
    // Generous allow-list: the superblock probe makes the real decision.
    for os_type in [
        0x07u8, /* NTFS/exFAT */
        0x0C,   /* FAT32-LBA */
        0x0B,   /* FAT32 */
        0x82,   /* swap-as-data */
    ] {
        assert!(
            mbr_os_type_is_mountable(os_type),
            "os_type {os_type:#04x} should be a probe candidate"
        );
    }
}

#[test]
fn empty_slot_is_skipped() {
    assert!(!mbr_os_type_is_mountable(0x00));
}

#[test]
fn extended_partition_containers_are_skipped() {
    // The EBR — not a filesystem — sits at the start of these.
    for os_type in [0x05u8, 0x0F, 0x85] {
        assert!(
            !mbr_os_type_is_mountable(os_type),
            "extended container {os_type:#04x} must be skipped"
        );
    }
}

#[test]
fn gpt_protective_is_skipped() {
    // 0xEE — the disk is really GPT; the GPT path owns it.
    assert!(!mbr_os_type_is_mountable(0xEE));
}

#[test]
fn efi_system_partition_is_skipped() {
    // 0xEF — already mounted as the ESP; probing it exclusive risks the
    // VM-120 firmware hang.
    assert!(!mbr_os_type_is_mountable(0xEF));
}
