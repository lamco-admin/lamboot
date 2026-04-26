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

use crate::{
    discovery::EntryKind,
    fs::{partition_guid_for, Volume, VolumeIdentity},
    fs_backend::FsBackend as _,
    fs_backend_fat::FatBackend,
};

/// Well-known GPT partition type GUIDs (UAPI.2 Discoverable Partitions)
#[cfg(target_arch = "x86_64")]
const ROOT_PARTITION_TYPE: Guid = guid!("4f68bce3-e8cd-4db1-96e7-fbcaf984b709");

#[cfg(target_arch = "aarch64")]
const ROOT_PARTITION_TYPE: Guid = guid!("b921b045-1df0-41c3-af44-4c6f280d3fae");

const XBOOTLDR_PARTITION_TYPE: Guid = guid!("bc13c2ff-59e6-4262-a352-b275fd6f7172");

/// EFI System Partition type GUID (UEFI spec). Excluded from the Opaque
/// return set because the ESP is already mounted via `mount_esp(image)`
/// elsewhere, and re-opening its BlockIO handle during superblock probing
/// conflicts with the live mount — observed on VM 120 OVMF to hang
/// LamBoot in `open_protocol_exclusive::<BlockIO>` without surfacing an
/// error. The ESP is never an ext4-backend candidate, so skipping it is
/// safe and eliminates the hang.
const ESP_PARTITION_TYPE: Guid = guid!("c12a7328-f81f-11d2-ba4b-00a0c93ec93b");

/// Information about a discovered partition
#[derive(Debug)]
pub(crate) struct DiscoveredPartition {
    pub partition_type: PartType,
    pub unique_guid: Guid,
    pub handle: uefi::Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartType {
    /// UAPI.2 Discoverable Partition for the architecture's Linux root.
    /// Consumed by [`auto_append_root`] to synthesize `root=PARTUUID=`.
    Root,
    /// UAPI.2 XBOOTLDR (Boot Loader Specification extended boot partition).
    /// Consumed by [`mount_xbootldr`].
    Xbootldr,
    /// A GPT partition present on the disk but carrying a partition-type
    /// GUID that isn't one of the DPS ones we recognize above. We still
    /// return it so callers that want to try mounting by filesystem probe
    /// (main.rs phase 6 — SDS-2 native ext4 mount) can iterate every
    /// partition the firmware knows about, not just DPS-tagged ones.
    ///
    /// Rationale: most Linux distros (Ubuntu, Debian, Fedora when not using
    /// `systemd-repart`) create their root partition with the generic
    /// "Linux filesystem" type GUID `0fc63daf-8483-4772-8e79-3d69d8477de4`
    /// rather than the DPS root GUID. Under v0.8.3 those partitions were
    /// reachable through the UEFI `ext4_x64.efi` filesystem driver, which
    /// registered `SimpleFileSystem` for any ext4 blockdev. Under v0.9.x
    /// SDS-6 Auto-mode correctly skips the legacy driver; without this
    /// `Opaque` path, the native ext4 backend would never get a chance
    /// to mount real-world distro roots, silently regressing Config 4.
    Opaque,
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

        // ESP: skip entirely. Probing its handle while mount_esp already
        // holds it hangs LamBoot on OVMF (see ESP_PARTITION_TYPE doc).
        if type_guid == ESP_PARTITION_TYPE {
            continue;
        }

        let part_type = if type_guid == ROOT_PARTITION_TYPE {
            PartType::Root
        } else if type_guid == XBOOTLDR_PARTITION_TYPE {
            PartType::Xbootldr
        } else {
            PartType::Opaque
        };

        // Log at INFO so boot.log captures scanner results and real-hardware
        // debugging stays tractable. For DPS-tagged partitions we name the
        // semantic role; for Opaque (generic Linux filesystem GUID, EFI
        // System, BIOS boot, Windows Recovery, etc.) we just record the
        // type GUID so the trace reader can correlate with a GUID table.
        match part_type {
            PartType::Root | PartType::Xbootldr => {
                log::info!("Discoverable partition: {part_type:?} PARTUUID={unique_guid}");
            }
            PartType::Opaque => {
                log::info!(
                    "GPT partition (opaque type={type_guid}) PARTUUID={unique_guid} — \
                     retained for superblock probing"
                );
            }
        }

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

/// Try to mount the XBOOTLDR partition as a `Volume` backed by the FAT
/// adapter. Returns `None` if no XBOOTLDR partition exists, the handle is
/// inaccessible, or the filesystem is not FAT (ext4 XBOOTLDR support lands
/// with SDS-2). The XBOOTLDR partition may contain BLS entries and kernels.
pub(crate) fn mount_xbootldr(partitions: &[DiscoveredPartition]) -> Option<Volume> {
    let xbootldr = partitions
        .iter()
        .find(|p| p.partition_type == PartType::Xbootldr)?;

    // Open SimpleFileSystem eagerly so we fail fast on non-FAT / missing driver.
    if uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(xbootldr.handle).is_err() {
        log::warn!("XBOOTLDR partition found but SimpleFileSystem is unavailable");
        return None;
    }

    let backend = FatBackend::new(xbootldr.handle).ok()?;
    let identity = VolumeIdentity {
        partition_guid: partition_guid_for(xbootldr.handle).or(Some(xbootldr.unique_guid)),
        fs_uuid: backend.uuid(),
        label: backend.label().map(alloc::string::ToString::to_string),
        // XBOOTLDR is always numbered after the ESP (which is index 0).
        // Subsequent extra FAT volumes from `enumerate_volumes()` use higher
        // indices; a minor overlap is acceptable because the index is only
        // used for display.
        index: u32::MAX,
        backend_tag: FatBackend::TAG,
    };
    log::info!("Mounted XBOOTLDR partition {}", identity.describe());
    Some(Volume::from_fat(identity, backend))
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
        // Superblock starts at byte offset 1024 of the partition; we read
        // from the LBA containing that offset, so it lives at
        // (1024 % block_size) within our buffer.
        let sb_offset = 1024 % block_size;
        if let Some(sb) = ext4_buf.get(sb_offset..) {
            if let Some(info) = parse_ext4_superblock(sb) {
                log::info!(
                    "Probed ext4: UUID={}",
                    info.uuid.as_deref().unwrap_or("unknown")
                );
                return Some(info);
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

/// Parse an ext4 superblock from a byte slice starting at the superblock
/// offset (byte 1024 of the partition). Returns `None` if the magic is
/// absent or the slice is too short to hold the UUID field. Pure — no
/// UEFI dependencies — so it can be exercised under host tests and
/// `cargo-fuzz` (see `fuzz/fuzz_targets/probe_ext4.rs`).
pub(crate) fn parse_ext4_superblock(sb: &[u8]) -> Option<FsInfo> {
    // Need bytes up through UUID tail: offset 0x78.
    if sb.len() < 0x78 {
        return None;
    }
    // Magic 0xEF53 at offset 0x38, little-endian.
    let magic = u16::from_le_bytes([sb[0x38], sb[0x39]]);
    if magic != 0xEF53 {
        return None;
    }
    // UUID at offset 0x68, 16 bytes.
    let uuid = format_fs_uuid(&sb[0x68..0x78]);
    Some(FsInfo {
        fs_type: FsType::Ext4,
        uuid: Some(uuid),
    })
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
