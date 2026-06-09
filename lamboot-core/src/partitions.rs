//! Layer: 4 — Policy & State.
//!
//! Discoverable partition scanning via UEFI PartitionInfo + BlockIO.
//!
//! Implements UAPI.2 Discoverable Partitions (GPT type GUIDs → Root/XBOOTLDR for
//! automatic root= generation and XBOOTLDR mounting) and, for robustness across
//! BIOS-converted and PartitionInfo-sparse firmware, also surfaces legacy-MBR
//! primary partitions and BlockIO-only logical partitions for superblock
//! probing by the native filesystem backends. See `scan_discoverable_partitions`.

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
    boot_types::EntryKind,
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

/// Scan every partition the firmware (or LamBoot) can see, from three sources,
/// de-duplicated by handle:
///
///   1. **GPT** via `PartitionInfo` — DPS-tagged Root/XBOOTLDR plus every other
///      GPT partition as `Opaque` (the ESP is skipped: probing its BlockIO
///      exclusive while `mount_esp` holds it hangs OVMF).
///   2. **MBR** via `PartitionInfo` — a BIOS-installed disk keeps an `msdos`
///      table whose `/boot` is a primary partition. Classified `Opaque` for
///      superblock probing; the EFI/extended/protective/empty slots are skipped
///      (see `partition_classify_pure::mbr_os_type_is_mountable`).
///   3. **BlockIO-only** logical partitions the firmware described with neither
///      `PartitionInfo` nor `SimpleFileSystem` — for firmware that installs
///      `PartitionInfo` on the ESP alone (observed: ASUS G10AJ).
///
/// Callers consume the result by *superblock probe* (`probe_superblock` + the
/// native ext4/btrfs/xfs/exfat/zfs/LVM/media loops), so the partition-table
/// type and the partition-type GUID are irrelevant to *mounting* — they only
/// drive Root (`auto_append_root`) and XBOOTLDR (`mount_xbootldr`) roles, both
/// GPT-only. That is why MBR and BlockIO-only partitions are safely `Opaque`.
pub(crate) fn scan_discoverable_partitions() -> Vec<DiscoveredPartition> {
    let mut results = Vec::new();
    let mut seen: Vec<uefi::Handle> = Vec::new();

    // Sources 1 + 2: firmware-parsed partition tables (GPT and MBR).
    if let Ok(handles) = uefi::boot::find_handles::<PartitionInfo>() {
        for handle in handles {
            let Ok(part_info) = uefi::boot::open_protocol_exclusive::<PartitionInfo>(handle) else {
                continue;
            };
            // Copy the Copy field out of the packed struct (by value, no
            // reference) before matching, to avoid an unaligned reference.
            let partition_type = part_info.partition_type;
            let classified = match partition_type {
                PartitionType::GPT => classify_gpt_partition(&part_info),
                PartitionType::MBR => classify_mbr_partition(&part_info),
                _ => None,
            };
            if let Some((partition_type, unique_guid)) = classified {
                results.push(DiscoveredPartition {
                    partition_type,
                    unique_guid,
                    handle,
                });
                seen.push(handle);
            }
        }
    } else {
        log::debug!("No PartitionInfo handles found");
    }

    // Source 3: BlockIO-only logical partitions not already described above.
    for handle in scan_block_io_logical_partitions_without_fs() {
        if seen.contains(&handle) {
            continue;
        }
        log::info!("BlockIO-only partition (no PartitionInfo) — retained for superblock probing");
        results.push(DiscoveredPartition {
            partition_type: PartType::Opaque,
            unique_guid: Guid::ZERO,
            handle,
        });
        seen.push(handle);
    }

    results
}

/// Classify a GPT partition from its `PartitionInfo`. `None` skips the ESP —
/// probing its BlockIO exclusive while `mount_esp` holds it hangs LamBoot on
/// OVMF (see `ESP_PARTITION_TYPE`). All non-ESP GPT partitions are retained:
/// DPS-tagged ones by role, the rest as `Opaque` for superblock probing.
fn classify_gpt_partition(part_info: &PartitionInfo) -> Option<(PartType, Guid)> {
    let gpt_entry = part_info.gpt_partition_entry()?;
    // Copy packed fields to avoid unaligned reference errors.
    let type_guid: Guid = { gpt_entry.partition_type_guid }.0;
    let unique_guid: Guid = { gpt_entry.unique_partition_guid };

    if type_guid == ESP_PARTITION_TYPE {
        return None;
    }

    let part_type = if type_guid == ROOT_PARTITION_TYPE {
        PartType::Root
    } else if type_guid == XBOOTLDR_PARTITION_TYPE {
        PartType::Xbootldr
    } else {
        PartType::Opaque
    };

    // Log at INFO so boot.log captures scanner results. DPS-tagged partitions
    // name the semantic role; Opaque records the type GUID for GUID-table
    // correlation by the trace reader.
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
    Some((part_type, unique_guid))
}

/// Classify a legacy-MBR partition from its `PartitionInfo`. `None` skips the
/// slots that carry no probeable filesystem (empty / extended container / GPT
/// protective) or are owned elsewhere (EFI System = the ESP). All others are
/// `Opaque`; the superblock probe decides the real filesystem. MBR has no
/// per-partition GUID, so `unique_guid` is `ZERO` — used only for display
/// (`auto_append_root` consumes GPT Root partitions, never MBR).
fn classify_mbr_partition(part_info: &PartitionInfo) -> Option<(PartType, Guid)> {
    let rec = part_info.mbr_partition_record()?;
    // Copy the single-byte newtype out of the packed record by value.
    let os_type = { rec.os_type }.0;
    if !crate::partition_classify_pure::mbr_os_type_is_mountable(os_type) {
        return None;
    }
    log::info!("MBR partition (os_type={os_type:#04x}) — retained for superblock probing");
    Some((PartType::Opaque, Guid::ZERO))
}

/// Enumerate logical-partition BlockIO handles the firmware described with
/// neither `PartitionInfo` nor `SimpleFileSystem`. These carry a filesystem the
/// firmware can't read (ext4/xfs/btrfs/LVM/…) on firmware that skips the
/// `PartitionInfo` install for non-ESP partitions (observed: ASUS G10AJ exposes
/// `PartitionInfo` for the ESP only).
///
/// Skipping `SimpleFileSystem` handles keeps the ESP and any firmware-mounted
/// FAT out: re-opening the ESP BlockIO exclusive while `mount_esp` holds it
/// hangs LamBoot (see `ESP_PARTITION_TYPE`), and firmware-FAT volumes are
/// already surfaced by `enumerate_volumes`.
fn scan_block_io_logical_partitions_without_fs() -> Vec<uefi::Handle> {
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};

    let mut out = Vec::new();
    let Ok(handles) = uefi::boot::find_handles::<BlockIO>() else {
        return out;
    };
    // Handles the firmware can already read as a filesystem — skip them.
    let sfs_handles: Vec<uefi::Handle> =
        uefi::boot::find_handles::<SimpleFileSystem>().unwrap_or_default();

    let image_handle = uefi::boot::image_handle();
    for handle in handles {
        if sfs_handles.contains(&handle) {
            continue;
        }
        let params = OpenProtocolParams {
            handle,
            agent: image_handle,
            controller: None,
        };
        // SAFETY: GetProtocol is non-exclusive and does not invalidate the
        // BY_DRIVER firmware FS agents. The ScopedProtocol is dropped before the
        // next iteration, so the close is synchronous and the pointer is never
        // retained past the drop. Same discipline as scan_block_io_partitions_for_lvm.
        let scoped = unsafe {
            uefi::boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol)
        };
        let Ok(block_io) = scoped else {
            continue;
        };
        // Skip whole disks; we only want logical partitions.
        if !block_io.media().is_logical_partition() {
            continue;
        }
        drop(block_io);
        out.push(handle);
    }
    out
}

/// Format a GUID as a lowercase UUID string for root=PARTUUID= usage.
pub(crate) fn format_partuuid(guid: &Guid) -> String {
    format!("{guid}")
}

/// Append root=PARTUUID=... to Linux entries that lack a root= parameter.
pub(crate) fn auto_append_root(
    entries: &mut [crate::boot_types::BootEntry],
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

/// Try to mount the XBOOTLDR partition as a `Volume`. Returns `None` if no
/// XBOOTLDR partition exists or it can be read by neither the firmware nor the
/// native FAT reader. The XBOOTLDR partition may contain BLS entries and
/// kernels.
///
/// Two-tier read path:
///   1. **Firmware `SimpleFileSystem`** (primary) — but only if `open_volume()`
///      actually succeeds. Some OVMF/i440FX configs expose an SFS handle for a
///      secondary FAT partition yet return `EFI_UNSUPPORTED` from
///      `open_volume()` (root cause: memory obs `4d6f6a9a`); that handle yields
///      a dead `FatBackend`, so we must probe before trusting it.
///   2. **Native `FatRo`** (fallback) — reads the FAT bytes directly over
///      BlockIO when the firmware refuses. Closes LamBoot's last firmware
///      dependency in the `/boot`-read path. See `fs_backend_fat_ro`.
pub(crate) fn mount_xbootldr(partitions: &[DiscoveredPartition]) -> Option<Volume> {
    let xbootldr = partitions
        .iter()
        .find(|p| p.partition_type == PartType::Xbootldr)?;

    // Tier 1: firmware FAT, only if it can actually open the volume.
    if firmware_fat_readable(xbootldr.handle) {
        if let Ok(backend) = FatBackend::new(xbootldr.handle) {
            let identity = VolumeIdentity {
                partition_guid: partition_guid_for(xbootldr.handle).or(Some(xbootldr.unique_guid)),
                fs_uuid: backend.uuid(),
                label: backend.label().map(alloc::string::ToString::to_string),
                // XBOOTLDR is always numbered after the ESP (which is index 0).
                // Subsequent extra FAT volumes from `enumerate_volumes()` use
                // higher indices; a minor overlap is acceptable because the
                // index is only used for display.
                index: u32::MAX,
                backend_tag: FatBackend::TAG,
            };
            log::info!(
                "Mounted XBOOTLDR partition (firmware FAT) {}",
                identity.describe()
            );
            return Some(Volume::from_fat(identity, backend));
        }
    }

    // Tier 2: native read-only FAT over BlockIO.
    match crate::fs_backend_fat_ro::FatRoBackend::open(xbootldr.handle) {
        Ok(backend) => {
            let identity = VolumeIdentity {
                partition_guid: partition_guid_for(xbootldr.handle).or(Some(xbootldr.unique_guid)),
                fs_uuid: backend.uuid(),
                label: backend.label().map(alloc::string::ToString::to_string),
                index: u32::MAX,
                backend_tag: crate::fs_backend_fat_ro::FAT_RO_BACKEND_TAG,
            };
            log::info!(
                "Mounted XBOOTLDR partition (native FatRo) {}",
                identity.describe()
            );
            Some(Volume::from_backend(
                identity,
                alloc::boxed::Box::new(backend),
            ))
        }
        Err(e) => {
            log::warn!("XBOOTLDR: firmware refused and FatRo failed: {e}");
            None
        }
    }
}

/// Probe whether the firmware's `SimpleFileSystem` can actually serve this
/// handle. A present SFS handle is not sufficient — `open_volume()` can still
/// return `EFI_UNSUPPORTED` for a secondary FAT partition on some firmware.
/// The exclusive open is released before returning so the subsequent
/// `FatBackend::new` (or `FatRo` fallback) can re-open cleanly.
fn firmware_fat_readable(handle: uefi::Handle) -> bool {
    let Ok(mut sfs) = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle) else {
        return false;
    };
    sfs.open_volume().is_ok()
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
    /// exFAT on removable/utility media (USB stick, SD card). Never the ESP.
    ExFat,
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

    // Try exFAT (boot sector at offset 0, "EXFAT   " magic at byte offset 3).
    // The legacy FAT BPB area (bytes 11..90) is zeroed on a real exFAT volume,
    // so this magic never collides with FAT32 (which carries "FAT32   " at 82).
    // Reuse the sector-0 read from the XFS probe above.
    if xfs_buf.len() >= 0x68 && xfs_buf[3..11] == *b"EXFAT   " {
        // 32-bit VolumeSerialNumber at offset 100, shown blkid-style XXXX-XXXX.
        let serial = u32::from_le_bytes([xfs_buf[100], xfs_buf[101], xfs_buf[102], xfs_buf[103]]);
        let serial_str = format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF);
        log::info!("Probed exFAT: serial={serial_str}");
        return Some(FsInfo {
            fs_type: FsType::ExFat,
            uuid: Some(serial_str),
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

    // Try ZFS: the L0 vdev label's uberblock array spans bytes 128–256 KiB, a
    // ring of slots each starting with `ub_magic` = 0x0000_0000_00ba_b10c (a u64
    // in the pool's native byte order). The active uberblock rotates by
    // `txg % slot_count` and the on-disk txg starts at 4, so slot 0 is usually
    // empty on a freshly created pool — scan the whole array at the minimum
    // 1 KiB slot stride (`VDEV_UBERBLOCK_SHIFT` floor) and accept the partition
    // if any slot carries the magic in either endianness (an x86 pool is
    // little-endian: `0c b1 ba 00 …`).
    const ZFS_UBERBLOCK_MAGIC: u64 = 0x0000_0000_00ba_b10c;
    const ZFS_UB_ARRAY_BYTES: usize = 128 * 1024;
    const ZFS_UB_MIN_SLOT: usize = 1024;
    let zfs_lba = 0x20000u64 / block_size as u64;
    let zfs_sectors = ZFS_UB_ARRAY_BYTES.div_ceil(block_size);
    let mut zfs_buf = alloc::vec![0u8; zfs_sectors * block_size];
    if block_io
        .read_blocks(media_id, zfs_lba, &mut zfs_buf)
        .is_ok()
    {
        let mut off = 0usize;
        while off + 8 <= zfs_buf.len() {
            let m = [
                zfs_buf[off],
                zfs_buf[off + 1],
                zfs_buf[off + 2],
                zfs_buf[off + 3],
                zfs_buf[off + 4],
                zfs_buf[off + 5],
                zfs_buf[off + 6],
                zfs_buf[off + 7],
            ];
            if u64::from_le_bytes(m) == ZFS_UBERBLOCK_MAGIC
                || u64::from_be_bytes(m) == ZFS_UBERBLOCK_MAGIC
            {
                log::info!("Probed ZFS");
                return Some(FsInfo {
                    fs_type: FsType::Zfs,
                    uuid: None, // the pool GUID is surfaced by the lamzfs import
                });
            }
            off += ZFS_UB_MIN_SLOT;
        }
    }

    None
}

/// Enumerate raw block-IO handles that represent partitions (not whole
/// disks, not the ESP), regardless of whether the firmware installed
/// `PartitionInfo` on them.
///
/// `scan_discoverable_partitions` (above) relies on the `PartitionInfo`
/// protocol, which most firmware installs on every partition handle but
/// not all. Observed empty-set in the wild on ASUS G10AJ (pve2):
/// firmware exposes `BlockIO` for every partition but `PartitionInfo`
/// for only the ESP, so the GPT-based enumerator returns 0 candidates.
/// This fallback finds those partitions by walking the `BlockIO`
/// protocol space directly and filtering to logical partitions only.
///
/// Used by the LVM-probe loop in main.rs so LVM detection works on
/// firmware that lacks the PartitionInfo install. Currently NOT
/// promoted to the ext4/btrfs loops — those still use the GPT-typed
/// path because they want the GPT GUID for partition-uuid lookups
/// and for the EFI-fallback path. The LVM probe doesn't need either.
pub(crate) fn scan_block_io_partitions_for_lvm() -> Vec<uefi::Handle> {
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};

    let mut out = Vec::new();
    let Ok(handles) = uefi::boot::find_handles::<BlockIO>() else {
        return out;
    };

    // CRITICAL: must NOT use open_protocol_exclusive::<BlockIO> here.
    // BlockIO is held BY_DRIVER by the firmware's filesystem drivers
    // (the SFS driver opens its parent BlockIO BY_DRIVER); an
    // exclusive open of BlockIO on those handles cascades a
    // disconnect of the BY_DRIVER agent, killing the in-flight ESP
    // mount mid-boot. Observed on pve2 (ASUS G10AJ firmware):
    // disconnects produced an empty boot menu with no recovery.
    //
    // The non-disturbing alternative is `open_protocol` with
    // `OpenProtocolAttributes::GetProtocol` (UEFI value 0x02), which
    // grants short-term shared access without affecting other
    // holders.
    let image_handle = uefi::boot::image_handle();
    for handle in handles {
        let params = OpenProtocolParams {
            handle,
            agent: image_handle,
            controller: None,
        };
        // SAFETY: GetProtocol mode is non-exclusive and does not
        // invalidate other agents. The ScopedProtocol is dropped
        // before the loop iteration ends so the protocol close is
        // synchronous; firmware handles + protocol references remain
        // valid for the lifetime of the open. We never store the
        // pointer past the drop.
        let scoped = unsafe {
            uefi::boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol)
        };
        let Ok(block_io) = scoped else {
            continue;
        };
        let media = block_io.media();
        // Whole disks have `logical_partition = false`; partitions
        // have it true. Skip whole disks — we don't want to probe a
        // disk's MBR area for LABELONE (LABELONE is at byte 512 of a
        // PV, and whole disks have MBR there).
        if !media.is_logical_partition() {
            continue;
        }
        // Probe the LABELONE signature WHILE we still hold the
        // GetProtocol handle — saves a second open. Same logic as
        // probe_lvm_pv but inlined here so we don't have to
        // round-trip through another open.
        let media_id = media.media_id();
        let block_size = u64::from(media.block_size());
        if block_size == 0 {
            continue;
        }
        let lba = 512u64 / block_size;
        let sectors = 1024usize.div_ceil(block_size as usize);
        let mut buf = alloc::vec![0u8; sectors * block_size as usize];
        if block_io.read_blocks(media_id, lba, &mut buf).is_err() {
            continue;
        }
        let offset = 512usize % block_size as usize;
        if offset + 8 > buf.len() {
            continue;
        }
        if &buf[offset..offset + 8] == b"LABELONE" {
            // Found one. Drop the GetProtocol handle (synchronous)
            // before returning so the caller can subsequently
            // open_protocol_exclusive::<BlockIO> on this same handle
            // — safe because (a) no fs driver was ever attached to a
            // raw LVM partition (no LVM-aware FS driver in firmware),
            // and (b) our GetProtocol slot is released here.
            drop(block_io);
            out.push(handle);
        }
    }
    out
}

/// Enumerate physical optical drives (CD/DVD/BD) holding an ISO9660 disc,
/// returning each drive's `BlockIO` handle paired with its volume label
/// (boot-from-ISO scenario d). Mirrors `scan_block_io_partitions_for_lvm`'s
/// non-exclusive walk: an exclusive `BlockIO` open would cascade a BY_DRIVER
/// disconnect of the firmware FS driver mid-boot (observed on pve2), so this
/// opens with `GetProtocol`.
///
/// Optical media is distinguished from other removable devices by its **2048-
/// byte logical sector size** — universal for CD/DVD/BD, whereas USB sticks
/// report 512. That single discriminator also keeps a dd'd isohybrid USB (a
/// valid ISO9660 image, but 512-byte sectors) out of this list, so it is never
/// double-listed against the ESP partition the firmware already surfaced. A
/// whole-disk, media-present, 2048-byte handle whose LBA 16 carries the ISO9660
/// `CD001` primary volume descriptor is an optical disc; its label is taken from
/// the PVD Volume Identifier.
pub(crate) fn scan_optical_handles() -> Vec<(uefi::Handle, alloc::string::String)> {
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};

    let mut out = Vec::new();
    let Ok(handles) = uefi::boot::find_handles::<BlockIO>() else {
        return out;
    };
    let image_handle = uefi::boot::image_handle();
    for handle in handles {
        let params = OpenProtocolParams {
            handle,
            agent: image_handle,
            controller: None,
        };
        // SAFETY: GetProtocol is non-exclusive and does not disturb the
        // BY_DRIVER agent (the firmware optical FS driver). The ScopedProtocol
        // is dropped before the next iteration, so the close is synchronous and
        // we never retain the pointer past the drop.
        let scoped = unsafe {
            uefi::boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol)
        };
        let Ok(block_io) = scoped else {
            continue;
        };
        let media = block_io.media();
        // Optical signature: media present, whole disk (not a logical
        // partition), 2048-byte logical sectors.
        if !media.is_media_present() || media.is_logical_partition() || media.block_size() != 2048 {
            continue;
        }
        let media_id = media.media_id();
        // The ISO9660 Primary Volume Descriptor lives at LBA 16 (byte 32768);
        // one 2048-byte block holds it.
        let mut buf = alloc::vec![0u8; 2048];
        if block_io.read_blocks(media_id, 16, &mut buf).is_err() {
            continue;
        }
        // PVD: descriptor type 0x01 at offset 0, standard id "CD001" at 1..6.
        if buf[0] != 0x01 || &buf[1..6] != b"CD001" {
            continue;
        }
        // Volume Identifier: 32 d-characters at offset 40, space/NUL-padded.
        let label = parse_pvd_label(&buf[40..72]);
        drop(block_io);
        out.push((handle, label));
    }
    out
}

/// Trim an ISO9660 PVD Volume Identifier (32 bytes, space/NUL-padded ASCII
/// d-characters) to a clean label. Stops at the first NUL, maps bytes as
/// Latin-1 (d-characters are ASCII), and strips trailing spaces.
fn parse_pvd_label(raw: &[u8]) -> alloc::string::String {
    let s: alloc::string::String = raw
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    s.trim_end().into()
}

/// Probe a partition for an LVM2 Physical Volume label. LVM2 places the
/// 8-byte ASCII signature `LABELONE` at the start of sector 1 (byte
/// offset 512) of a PV. This is a cheap O(1-block-read) check that runs
/// alongside the filesystem-superblock probe.
///
/// Returns `true` if the signature is present. Does NOT parse the rest
/// of the PV header / VG metadata — that work happens in
/// `fs_backend_lvm::LvmExt4Backend::open` only when a caller commits to
/// mounting LVs from this partition.
pub(crate) fn probe_lvm_pv(handle: uefi::Handle) -> bool {
    let Ok(block_io) = uefi::boot::open_protocol_exclusive::<BlockIO>(handle) else {
        return false;
    };
    let media = block_io.media();
    let media_id = media.media_id();
    let block_size = media.block_size() as usize;
    if block_size == 0 {
        return false;
    }
    // Sector 1 contains the label header. With the smallest practical
    // block_size (512) we need LBA 1; with larger block sizes the label
    // is still at byte 512 within LBA 0.
    let lba = 512u64 / block_size as u64;
    let sectors = 1024usize.div_ceil(block_size);
    let mut buf = alloc::vec![0u8; sectors * block_size];
    if block_io.read_blocks(media_id, lba, &mut buf).is_err() {
        return false;
    }
    let offset = 512usize % block_size;
    if offset + 8 > buf.len() {
        return false;
    }
    &buf[offset..offset + 8] == b"LABELONE"
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
