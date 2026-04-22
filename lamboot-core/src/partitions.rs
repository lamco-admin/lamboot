//! Discoverable partition scanning via UEFI PartitionInfo protocol.
//!
//! Implements UAPI.2 Discoverable Partitions Specification:
//! Scans GPT partition type GUIDs to find root partitions, XBOOTLDR,
//! and ESP — enabling automatic root= generation and XBOOTLDR mounting.

use alloc::{format, string::String, vec::Vec};

use uefi::{
    guid,
    proto::media::{
        block::BlockIO,
        fs::SimpleFileSystem,
        partition::{PartitionInfo, PartitionType},
    },
    Guid,
};

use crate::{discovery::EntryKind, fs::EspVolume};

/// Well-known GPT partition type GUIDs (UAPI.2 Discoverable Partitions)
#[cfg(target_arch = "x86_64")]
const ROOT_PARTITION_TYPE: Guid = guid!("4f68bce3-e8cd-4db1-96e7-fbcaf984b709");

#[cfg(target_arch = "aarch64")]
const ROOT_PARTITION_TYPE: Guid = guid!("b921b045-1df0-41c3-af44-4c6f280d3fae");

const XBOOTLDR_PARTITION_TYPE: Guid = guid!("bc13c2ff-59e6-4262-a352-b275fd6f7172");

/// Information about a discovered partition
#[derive(Debug)]
pub(crate) struct DiscoveredPartition {
    pub partition_type: PartType,
    pub unique_guid: Guid,
    pub handle: uefi::Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartType {
    Root,
    Xbootldr,
}

/// Scan all partitions for discoverable types (root, XBOOTLDR).
/// Uses the UEFI PartitionInfo protocol — firmware already parsed GPT.
pub(crate) fn scan_discoverable_partitions() -> Vec<DiscoveredPartition> {
    let mut results = Vec::new();

    let Ok(handles) = uefi::boot::find_handles::<PartitionInfo>() else {
        log::debug!("No PartitionInfo handles found");
        return results;
    };

    for handle in handles {
        let Ok(part_info) = uefi::boot::open_protocol_exclusive::<PartitionInfo>(handle) else {
            continue;
        };

        // Only GPT partitions — copy from packed struct to avoid alignment issues
        let pt = { part_info.partition_type };
        if pt != PartitionType::GPT {
            continue;
        }

        let Some(gpt_entry) = part_info.gpt_partition_entry() else {
            continue;
        };

        // Copy packed fields to avoid unaligned reference errors
        let type_guid: Guid = { gpt_entry.partition_type_guid }.0;
        let unique_guid: Guid = { gpt_entry.unique_partition_guid };

        let part_type = if type_guid == ROOT_PARTITION_TYPE {
            PartType::Root
        } else if type_guid == XBOOTLDR_PARTITION_TYPE {
            PartType::Xbootldr
        } else {
            continue;
        };

        log::info!("Discoverable partition: {part_type:?} PARTUUID={unique_guid}");

        results.push(DiscoveredPartition {
            partition_type: part_type,
            unique_guid,
            handle,
        });
    }

    results
}

/// Format a GUID as a lowercase UUID string for root=PARTUUID= usage.
pub(crate) fn format_partuuid(guid: &Guid) -> String {
    format!("{guid}")
}

/// Append root=PARTUUID=... to Linux entries that lack a root= parameter.
pub(crate) fn auto_append_root(
    entries: &mut [crate::discovery::BootEntry],
    partitions: &[DiscoveredPartition],
) {
    // Find the first root partition
    let root_part = partitions
        .iter()
        .find(|p| p.partition_type == PartType::Root);
    let Some(root) = root_part else {
        return;
    };

    let partuuid = format_partuuid(&root.unique_guid);

    for entry in entries.iter_mut() {
        if let EntryKind::LinuxLegacy {
            ref mut options, ..
        } = entry.kind
        {
            if !options.contains("root=") {
                let append = format!(" root=PARTUUID={partuuid} rw");
                options.push_str(&append);
                log::info!("Auto root=: appended PARTUUID={partuuid} for {}", entry.id);
            }
        }
    }
}

/// Try to mount XBOOTLDR partition and return it as an EspVolume.
/// The XBOOTLDR partition may contain BLS entries and kernels.
pub(crate) fn mount_xbootldr(partitions: &[DiscoveredPartition]) -> Option<EspVolume> {
    let xbootldr = partitions
        .iter()
        .find(|p| p.partition_type == PartType::Xbootldr)?;

    // Try to open SimpleFileSystem on the XBOOTLDR handle
    let Ok(mut fs) = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(xbootldr.handle)
    else {
        log::warn!("XBOOTLDR partition found but cannot open filesystem");
        return None;
    };

    let Ok(root) = fs.open_volume() else {
        log::warn!("XBOOTLDR partition found but cannot open volume");
        return None;
    };

    log::info!("Mounted XBOOTLDR partition");
    Some(EspVolume::new(root))
}

// ============================================================================
// Filesystem Superblock Probing
// ============================================================================

/// Filesystem identification from superblock magic numbers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsType {
    Ext4,
    Btrfs,
    Xfs,
    F2fs,
    Zfs,
    #[expect(
        dead_code,
        reason = "fallback for unrecognized but mountable filesystems"
    )]
    Unknown,
}

/// Information extracted from a filesystem superblock
#[derive(Debug)]
pub(crate) struct FsInfo {
    pub fs_type: FsType,
    pub uuid: Option<String>,
}

/// Probe a block device handle for filesystem superblock metadata.
/// Reads raw sectors via BlockIO protocol to identify ext4/btrfs
/// and extract UUID and volume label.
pub(crate) fn probe_superblock(handle: uefi::Handle) -> Option<FsInfo> {
    let Ok(block_io) = uefi::boot::open_protocol_exclusive::<BlockIO>(handle) else {
        return None;
    };

    let media = block_io.media();
    let media_id = media.media_id();
    let block_size = media.block_size() as usize;

    if block_size == 0 {
        return None;
    }

    // Read enough data for superblock probing (need first 128KB for btrfs)
    // ext4 superblock is at byte offset 1024 (usually within first 2 sectors)
    // btrfs superblock is at byte offset 0x10000 (64KB)

    // Try ext4 first (superblock at offset 1024)
    let ext4_lba = 1024 / block_size as u64;
    let ext4_sectors = 4096usize.div_ceil(block_size); // Read enough for the superblock
    let mut ext4_buf = alloc::vec![0u8; ext4_sectors * block_size];

    if block_io
        .read_blocks(media_id, ext4_lba, &mut ext4_buf)
        .is_ok()
    {
        // ext4 magic at offset 0x38 from superblock start (which is at 1024 from disk start)
        // Since we read from LBA containing offset 1024, the superblock starts at
        // offset (1024 % block_size) within our buffer
        let sb_offset = 1024 % block_size;
        if sb_offset + 0x80 <= ext4_buf.len() {
            let magic =
                u16::from_le_bytes([ext4_buf[sb_offset + 0x38], ext4_buf[sb_offset + 0x39]]);
            if magic == 0xEF53 {
                // Extract UUID at offset 0x68 (16 bytes)
                let uuid_bytes = &ext4_buf[sb_offset + 0x68..sb_offset + 0x78];
                let uuid = format_fs_uuid(uuid_bytes);

                log::info!("Probed ext4: UUID={uuid}");
                return Some(FsInfo {
                    fs_type: FsType::Ext4,
                    uuid: Some(uuid),
                });
            }
        }
    }

    // Try btrfs (superblock at offset 0x10000 = 64KB)
    let btrfs_lba = 0x10000u64 / block_size as u64;
    let btrfs_sectors = 4096usize.div_ceil(block_size);
    let mut btrfs_buf = alloc::vec![0u8; btrfs_sectors * block_size];

    if block_io
        .read_blocks(media_id, btrfs_lba, &mut btrfs_buf)
        .is_ok()
    {
        let sb_offset = 0x10000 % block_size;
        if sb_offset + 0x200 <= btrfs_buf.len() {
            // btrfs magic "_BHRfS_M" at offset 0x40 from superblock start
            let magic = &btrfs_buf[sb_offset + 0x40..sb_offset + 0x48];
            if magic == b"_BHRfS_M" {
                // UUID at offset 0x20 (16 bytes)
                let uuid_bytes = &btrfs_buf[sb_offset + 0x20..sb_offset + 0x30];
                let uuid = format_fs_uuid(uuid_bytes);

                log::info!("Probed btrfs: UUID={uuid}");
                return Some(FsInfo {
                    fs_type: FsType::Btrfs,
                    uuid: Some(uuid),
                });
            }
        }
    }

    // Try XFS (superblock at offset 0, magic "XFSB" at offset 0)
    let mut xfs_buf = alloc::vec![0u8; block_size.max(512)];
    if block_io.read_blocks(media_id, 0, &mut xfs_buf).is_ok()
        && xfs_buf.len() >= 0x60
        && xfs_buf[0..4] == *b"XFSB"
    {
        let uuid_bytes = &xfs_buf[0x20..0x30];
        let uuid = format_fs_uuid(uuid_bytes);
        log::info!("Probed XFS: UUID={uuid}");
        return Some(FsInfo {
            fs_type: FsType::Xfs,
            uuid: Some(uuid),
        });
    }

    // Try F2FS (superblock at offset 1024, magic 0xF2F52010 at offset 0)
    if ext4_buf.len() >= 1024 % block_size + 4 {
        let sb_offset = 1024 % block_size;
        let magic = u32::from_le_bytes([
            ext4_buf[sb_offset],
            ext4_buf[sb_offset + 1],
            ext4_buf[sb_offset + 2],
            ext4_buf[sb_offset + 3],
        ]);
        if magic == 0xF2F5_2010 {
            // UUID at offset 0x6C (16 bytes)
            if sb_offset + 0x7C <= ext4_buf.len() {
                let uuid_bytes = &ext4_buf[sb_offset + 0x6C..sb_offset + 0x7C];
                let uuid = format_fs_uuid(uuid_bytes);
                log::info!("Probed F2FS: UUID={uuid}");
                return Some(FsInfo {
                    fs_type: FsType::F2fs,
                    uuid: Some(uuid),
                });
            }
        }
    }

    // Try ZFS (uberblock at offset 0x20000 = 128KB, magic 0x00BAB10C at offset 0)
    let zfs_lba = 0x20000u64 / block_size as u64;
    let zfs_sectors = 4096usize.div_ceil(block_size);
    let mut zfs_buf = alloc::vec![0u8; zfs_sectors * block_size];
    if block_io
        .read_blocks(media_id, zfs_lba, &mut zfs_buf)
        .is_ok()
        && zfs_buf.len() >= 4
    {
        let magic = u32::from_be_bytes([zfs_buf[0], zfs_buf[1], zfs_buf[2], zfs_buf[3]]);
        if magic == 0x00BA_B10C {
            log::info!("Probed ZFS");
            return Some(FsInfo {
                fs_type: FsType::Zfs,
                uuid: None, // ZFS UUID requires parsing the label nvpair data
            });
        }
    }

    None
}

/// Format a filesystem UUID (standard byte order, not GPT mixed-endian)
fn format_fs_uuid(bytes: &[u8]) -> String {
    if bytes.len() < 16 {
        return String::from("unknown");
    }
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

// vmgenid detection delegated to fw_cfg module
pub(crate) fn read_vmgenid() -> Option<[u8; 16]> {
    crate::fw_cfg::read_vmgenid()
}
