//! Layer: 4 (pure) — legacy-MBR partition `os_type` classification.
//!
//! Split out of `partitions::scan_discoverable_partitions` so `lamboot-fs-tests`
//! can include it verbatim via `#[path]` and exercise every legacy-MBR
//! partition-type byte on the host — no UEFI needed.
//!
//! A BIOS-installed Linux disk keeps an `msdos` (MBR) partition table whose
//! `/boot` is a primary partition (RHEL/CentOS/Rocky/Alma and any pre-UEFI
//! install). When such a host is converted to UEFI, LamBoot must still discover
//! those partitions and superblock-probe them — but it must NOT probe the slots
//! that carry no filesystem at their start, or the one already owned elsewhere.

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

/// Does an MBR partition `os_type` byte denote a partition that may carry a
/// mountable filesystem worth superblock-probing?
///
/// Excluded (return `false`):
/// - `0x00` — empty / unused table slot.
/// - `0x05`, `0x0F`, `0x85` — extended-partition CONTAINERS. The EBR, not a
///   filesystem, sits at their start; the logical volumes inside are surfaced
///   by the firmware as their own partition handles.
/// - `0xEE` — GPT protective MBR. The disk is really GPT; the GPT path owns it.
/// - `0xEF` — EFI System Partition on an MBR disk. Already mounted as the ESP;
///   re-opening its BlockIO exclusive while `mount_esp` holds it can hang the
///   firmware (the VM-120 `open_protocol_exclusive::<BlockIO>` hang).
///
/// Everything else (`0x83` Linux, `0x8E` Linux LVM, `0x07` NTFS/exFAT, `0x0C`
/// FAT32-LBA, …) is a probe candidate: the superblock probe decides the actual
/// filesystem, so a generous allow-list here costs at most one BlockIO read on a
/// non-filesystem partition and never a misclassification.
#[must_use]
pub const fn mbr_os_type_is_mountable(os_type: u8) -> bool {
    !matches!(os_type, 0x00 | 0x05 | 0x0F | 0x85 | 0xEE | 0xEF)
}
