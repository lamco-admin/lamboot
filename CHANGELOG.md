# Changelog

All notable changes to LamBoot are documented here. Format inspired by Keep a Changelog; semantic versioning is loose during pre-1.0.

## [0.16.5] — Dependency pins: lamfat 0.4.2 + lamzfs 0.1.1

A dependency-sourcing patch with no change to boot behavior. The native FAT and
ZFS readers move to freshly republished crate versions.

### Changed

- **`FatRo` now consumes `lamfat` `=0.4.2`** — a republish of the read-only
  FAT reader without the `fscommon` dependency. Read behavior is byte-for-byte
  identical; this trims a transitive dependency from the FAT-backend layer.
- **`lamzfs` pinned to `=0.1.1`**, which moves its LZ4 block decompressor from
  `lz4_flex` 0.11 to 0.13. The ZFS boot-pool reader decodes the same blocks;
  this is a dependency refresh with no behavior change.

### Infrastructure

- SBAT version string aligned to `0.16.5` (generation column unchanged at `1`:
  human-facing string only, no revocation).

## [0.16.4] — Kernel-upgrade BLS entries fixed on XFS /boot (kernel hook)

A kernel upgrade on an XFS `/boot` (RHEL/Rocky/Alma/CentOS — surfaced by a RHEL
9.7 → 9.8 upgrade) produced an **unbootable, red-X** BLS entry for the new
kernel: `lamboot-kernel-hook` (the incremental writer invoked by
`kernel-install` / the Debian kernel hooks) had two bugs that only bit XFS and
`kernel-install`-driven distros. The bootloader binary is unchanged; this is a
host-side tooling fix.

### Fixed

- **`lamboot-kernel-hook` now writes XFS `/boot` entries in place, not on the
  ESP.** Its `resolve_bls_dir` placement set was `vfat|ext2|ext3|ext4|btrfs` —
  missing `xfs`, so it never picked up `lamboot-install`'s v0.16.0 "xfs →
  `boot_in_place`" change and wrote the new kernel's entry to the ESP instead of
  the XFS `/boot`.
- **The `linux` path now references the `/boot` kernel copy, not kernel-install's
  source image.** On Fedora/RHEL/CentOS, `kernel-install add` passes
  `/lib/modules/<version>/vmlinuz` (the package staging path); the hook copied
  that into the BLS `linux` field (mangled to `//lib/modules/...`) instead of the
  bootable `/boot/vmlinuz-<version>` copy → LamBoot couldn't find the kernel
  (preflight red X). It now prefers a `/boot`-located image, else
  `/boot/vmlinuz-<version>`; `bls_path` is hardened against the `//` double-slash.
- **Placement parity is now build-enforced.** `tools/check-fs-coverage-parity.py`
  additionally asserts the hook's in-place set equals
  `is_filesystem_natively_covered` minus zfs (the lamzfs read-in-place deferral).
  The hook can no longer silently drift from the installer — the exact regression
  class behind this fix. (zfs stays ESP-placed in both, by design.)

## [0.16.3] — Robust partition discovery (MBR) + bootloader NVRAM self-install

Robustness work surfaced by the first RHEL-family validation (RHEL 9.7, a BIOS
install converted to UEFI). LamBoot now discovers and boots a Linux install whose
`/boot` is a plain filesystem on a **legacy-MBR** disk — the canonical layout of
a BIOS-installed RHEL/Rocky/Alma/CentOS — and manages its own persistent UEFI
boot entry directly from the firmware environment.

### Added

- **Bootloader-side NVRAM self-install of the `LamBoot` boot entry.** LamBoot now
  ensures a labeled `Boot####` entry pointing at `\EFI\LamBoot\lambootx64.efi`
  exists, creating it (and front-loading `BootOrder`) from the UEFI environment
  if absent. This is the OS-independent pathway for the persistent entry: no
  `efibootmgr`, no OS, and on SELinux-enforcing distros no confined-service block
  in the way (`Boot####`/`BootOrder` are non-authenticated, so no Secure Boot
  keys are involved). Idempotent — it keys on the exact `LamBoot` description, so
  it never duplicates or churns NVRAM and coexists with the OS-side `efibootmgr`
  pathway. New module `boot_entry` (UEFI I/O) over `boot_entry_pure` (the
  byte-exact `EFI_LOAD_OPTION`/`BootOrder` codecs, host-tested).
- **`[boot-entry] self_install` policy gate** (default **on**; set `false` to
  opt out for operators who manage firmware boot order externally).

### Fixed

- **Partition discovery now covers MBR and BlockIO-only partitions, not just
  GPT.** `scan_discoverable_partitions` previously consumed only GPT entries from
  the `PartitionInfo` protocol, so a BIOS-installed disk with an `msdos` table
  (e.g. RHEL's XFS `/boot` as a primary partition) yielded zero discovered
  partitions and its `/boot` was never mounted — LamBoot showed no entries and
  fell into the fallback self-loop. It is now a three-source enumerator,
  de-duplicated by handle: GPT via `PartitionInfo`, **MBR** via
  `PartitionInfo.mbr_partition_record()` (classified by a host-tested `os_type`
  predicate), and **BlockIO-only** logical partitions for firmware that installs
  `PartitionInfo` on the ESP alone. Every native FS backend (lamxfs/ext4/btrfs/
  exfat/zfs/LVM/media) picks them up unchanged.
- **The fallback self-loop guard now actually fires.** The old guard compared the
  on-disk `\EFI\BOOT\BOOTX64.EFI` against the **in-memory, firmware-relocated**
  running image — hashes that never match — so a no-entries boot would offer and
  auto-select the fallback, chainloading LamBoot into itself. Replaced with a
  reliable path check: if LamBoot was launched from the removable fallback path,
  that synthetic is suppressed and the recovery menu shows instead.
- **First-boot NVRAM service no longer retries forever.** A persistently-blocked
  write (e.g. a mislabeled binary on enforcing SELinux) now fails non-fatally
  after a bounded number of attempts with an actionable message, instead of
  re-running every boot.
- **Installer relabels its files for SELinux (`restorecon`).** A hand-deployed
  (non-rpm) install copied through `/tmp` leaves files labeled `user_tmp_t`,
  which `init_t` may not execute — so the first-boot service's exec of
  `lamboot-install` (a symlink into `/usr/share/lamboot`) was denied on enforcing
  RHEL. `lamboot-install` now runs `restorecon` on its installed files +
  payload dir when SELinux is enabled (idempotent no-op on rpm/non-SELinux).
  This is the correct fix; no custom SELinux policy module is shipped.

## [0.16.2] — Fix: installer misclassified its own old fallback as systemd-boot

A point fix for the upgrade path on systems whose firmware boots LamBoot via the
removable-media fallback `\EFI\BOOT\BOOTX64.EFI` (e.g. a Proxmox VM whose ESP
lives on a disk not in the VM's `boot:` order, where named NVRAM entries don't
persist). On those systems `lamboot-install --update --fallback` aborted and the
upgrade never reached the binary the firmware actually loads.

### Fixed

- **`identify_bootloader` now detects LamBoot before systemd-boot.** LamBoot's
  own binary embeds the string `systemd-boot` (its SecurityOverride mirrors
  systemd-boot's ShimLock dance), and the loader-classifier grepped for
  `systemd-boot` *before* the LamBoot self-check — so an existing LamBoot
  fallback was misclassified as a foreign systemd-boot, and `install_fallback`
  refused to refresh it without `--replace-fallback`. The LamBoot check now runs
  first; an old LamBoot fallback is correctly recognized as our own and updated
  in place on `--update --fallback`. Covered by a new
  `tools/tests/installer/identify_bootloader.bats` regression suite.

### Added

- **Install-time warning when the ESP is on a separate disk from the OS root.**
  On a hypervisor whose firmware only boot-indexes the disks in the VM's `boot:`
  order (Proxmox/QEMU OVMF), a named LamBoot NVRAM entry pointing at a
  non-boot-indexed disk is pruned on reboot — so an upgrade silently reverts to
  booting the fallback. `lamboot-install` now detects this (ESP parent disk ≠ OS
  root disk under `systemd-detect-virt`) in `warn_if_esp_on_separate_disk()` and
  warns with the actionable `qm set --boot` fix (or `--fallback`), plus an
  `esp_on_separate_disk` trust event. Tested by
  `tools/tests/installer/warn_esp_disk.bats`.

## [0.16.1] — Fix: goblin 0.9.3 could not boot Linux kernels (native PE loader)

A hotfix for a boot regression introduced in 0.16.0. The `goblin` PE-parser pin was
moved `=0.10.5 → =0.9.3` to align with the Debian archive, but goblin 0.9.3's
`Header::parse` **rejects the Linux EFI-stub layout** — the kernel omits the MSVC DOS
stub (`e_lfanew == 0x40`), and 0.9.3's `DosStub::parse` fails the `end_offset <=
start_offset` check (0.10.5 relaxed it to `<`). Every native-PE Linux-kernel load
therefore failed with `pe_parse_failed`, dead-ending the boot — observed as the VM
powering off on kernel selection. SB-off, directly-signed, and MOK kernels booted via
the native loader are all affected; a default-config Linux boot does not complete.

### Fixed

- **Native PE loader no longer depends on `goblin`.** The single `goblin::Header::parse`
  call is replaced by a hand-rolled `no_std` PE/COFF reader that reads only the
  load-bearing COFF + PE32+ optional-header fields from their fixed offsets and never
  parses the MSVC DOS stub or Rich header — the structures goblin 0.9.3 mis-validated.
  `number_of_rva_and_sizes` bounds the data-directory accessors (Linux kernels ship
  fewer than the full 16). PE32 (`0x10b`) is rejected cleanly. `goblin` — and its
  transitive `plain` / `scroll` deps — leave the dependency tree entirely, which is what
  the Debian archive needs. Verified end-to-end: the exact kernel 0.16.0 rejected now
  loads with `image_loaded_native … SUCCESS`.
- **`boot_linux` gains a native → firmware `BS->LoadImage` fallback.** A native-PE
  parse/load failure on a Linux kernel is no longer terminal — it degrades to a firmware
  boot (logged as `kernel_native_load_fallback`) instead of an unbootable system. The ISO
  path already had this; it now covers BLS kernels too.

### Changed

- **Installer native-filesystem coverage synced with the runtime.** `xfs`/`zfs` are now
  native (lamxfs/lamzfs), so `lamboot-install` no longer installs their legacy GPLv3
  drivers; a separate XFS `/boot` becomes read-in-place. ZFS `/boot` stays ESP-staged
  until `lamzfs` is field-soaked across pool topologies (the driver-skip still applies).
  The `--capabilities` JSON moves `xfs`/`zfs` to `native`. A new
  `tools/check-fs-coverage-parity.py` (pre-commit + CI) pins the installer's list to
  `lamboot-core/src/drivers.rs` so the two cannot silently drift again.

## [0.16.0] — Native XFS / exFAT / ZFS readers + Boot-from-ISO + the lamfold media stack

The biggest de-bundling release since the native ext4/btrfs readers landed. LamBoot
now reads four more on-disk formats natively and read-only — XFS, exFAT, ZFS, and a
six-filesystem read-only *media* stack (EROFS, ISO 9660, SquashFS, cramfs, romfs, UDF) —
without leaning on a firmware filesystem driver or a GPLv3 EfiFs `*.efi` for any of them.
Together these close the last large gaps in the de-bundling map: XFS is the default
`/boot` and `/` across the RHEL family, ZFS covers the Ubuntu/Debian root-on-ZFS `bpool`
pattern, exFAT covers >4 GiB removable boot media, and the media stack covers immutable
and live-media images. A new, **opt-in** boot-from-ISO capability lets an operator boot a
distribution directly from an `.iso` on a mounted volume or from a physical optical disc.
Every new backend is read-only **by construction** — there is no write path and the ESP
writer only ever targets the FAT backend — and nothing in this release changes a
default-config boot: both boot-from-ISO gates default off.

### Added

- **Native read-only XFS backend (`lamxfs`).** A clean-room `no_std` XFS reader replaces
  the GPLv3 EfiFs `xfs_x64.efi` driver path (which cannot be Microsoft-signed and trips
  shim 15.8's `ShimLock` uninstall). It mounts a whole XFS partition and — via the
  source-generic dispatcher — an XFS filesystem on an LVM logical volume (the default RHEL
  root layout) or inside an `.iso` loopback region. It derives its UUID and label from the
  mounted superblock, decodes real on-disk POSIX modes, follows leaf symlinks, caps
  up-front allocations against a hostile inode size, and turns structural and CRC failures
  into stable error tokens. XFS is the only one of the three new whole-partition readers
  that exposes a real filesystem UUID/label and decodes on-disk modes. Read-only by
  construction.
- **Native read-only exFAT backend (`lamexfat`).** A `no_std` exFAT reader for
  removable/utility boot media — a kernel+initrd or a boot image on an exFAT-formatted USB
  stick or SD card, including the >4 GiB payloads FAT32 cannot hold. exFAT is never the
  ESP and never a Linux root, so it does not overlap the ext4/btrfs/xfs/FAT backends; it
  mounts only whole partitions identified by the `EXFAT   ` boot-sector magic and is
  intentionally *not* wired into the LV/`.iso` dispatcher. The reader handles both
  FAT-chained and contiguous (`NoFatChain`) files, up-case-table case-insensitive lookup,
  and entry-set checksums, and caps allocations against a hostile `DataLength`. exFAT
  carries no 128-bit UUID or POSIX modes, so metadata is synthesized read-only and
  `uuid()` returns `None`. Read-only by construction.
- **Native read-only ZFS boot-pool backend (`lamzfs`).** LamBoot can now read kernels and
  initrds directly from an *unencrypted* ZFS boot pool (the Ubuntu/Debian root-on-ZFS
  `bpool` pattern) with no OpenZFS kernel module and no GPL UEFI filesystem driver. A pool
  is imported once from its member partitions — grouped by pool GUID via
  `lamzfs::peek_pool_id` before mount — and each dataset is presented as its own read-only
  volume; reads, stats, ranged reads, existence checks, and directory enumeration are all
  dataset-scoped. Supported vdev topologies are **single disk, mirror, and (per spec)
  single-parity RAIDZ1** with XOR degraded reconstruction; RAIDZ2/RAIDZ3, dRAID, multiple
  top-level vdevs, allocation-class/special vdevs, and native encryption are rejected
  cleanly with a typed error. All six OpenZFS block compressors (off, zle, lzjb, lz4,
  gzip, zstd) decode, and a block whose checksum fails surfaces as an integrity error
  rather than a generic parse failure. The backend is `forbid(unsafe_code)` and read-only
  by construction. ZFS pools carry a 64-bit GUID rather than a 128-bit filesystem UUID, so
  `uuid()` returns `None` and datasets are identified by label and index; on-disk metadata
  modes are synthesized read-only. *Scope note: mirror healing and RAIDZ1 reconstruction
  are supported per the lamzfs v0.1 spec; the live-boot validation in this release
  exercised a single-vdev lz4 pool, so RAIDZ1/mirror-repair behavior should be treated as
  spec-supported, not yet field-soaked.*
- **Native read-only media filesystem stack (`lamfold`): EROFS, ISO 9660, SquashFS,
  cramfs, romfs, UDF.** LamBoot now mounts six read-only media filesystems natively at
  boot through the Lamco-authored `no_std` lamfold stack, without a firmware FS driver.
  Each is identified by a superblock-magic probe (UDF via a targeted sector-256
  Anchor-Volume-Descriptor read) and dispatched over the generic `BlockSource` seam to a
  single `LamfoldBackend` adapter, so a standalone media-FS partition (an EROFS or
  SquashFS root or `/boot`) is mounted as an ordinary read-only volume and scanned for
  kernels/BLS entries. EROFS reads uncompressed, LZ4, and the compressed-COMPACT codec set
  (deflate / zstd / MicroLZMA); SquashFS reads gzip/xz/zstd/lz4/lzo; ISO 9660 reads
  zisofs. Every lamfold backend is read-only by construction (`read_only` is hardcoded
  true and no write path exists). Media mounts are currently *unverified* (read-only, but
  no integrity/trust-root verification of media content yet); a Merkle trust-root path is a
  later pass. *cpio is linked as a lamfold frontend but is deliberately not auto-probed or
  wired into mount dispatch — it is not a usable boot filesystem in this release.*
- **Boot a distribution ISO from disk or a physical optical drive (opt-in, off by
  default).** With `[boot-from-iso] enabled`, an operator can boot a Linux distribution
  directly from an `.iso` file on a mounted volume (ext4/btrfs/xfs/FAT, including the ESP);
  with `optical = true`, from an inserted CD/DVD/BD. ISO discovery scans `/isos` and
  `/boot/isos` for `*.iso`; optical discovery enumerates 2048-byte-sector `BlockIO`
  handles carrying a `CD001` primary volume descriptor (a discriminator that structurally
  excludes a `dd`'d isohybrid USB, so it is never double-listed). A file-hosted ISO is read
  through a `FileBlockSource` over the holding backend's already-open handle (so reading an
  ISO off the ESP never disconnects the FAT driver); an optical disc is read through a
  non-exclusive shared `BlockIO` open (an exclusive open is `ACCESS_DENIED` while the
  firmware ISO9660 driver holds the disc `BY_DRIVER`). The kernel and initrd are read off
  the mounted ISO and handed off through the same measure/verify/initrd-register path as a
  normal Linux boot. Because a distribution kernel is a Linux EFI-stub PE that LamBoot's
  native loader cannot load, the ISO path falls back from the native loader to firmware
  `LoadImage` on a load error. *This is experimental and opt-in;* only Arch 2026.05 and
  Fedora 44 are live-ISO-validated end to end (see below).
- **`loopback.cfg` resolution (Path A1) and a per-distro-family fallback table (Path A2).**
  For a file-hosted ISO, LamBoot first parses the distribution's own
  `/boot/grub/loopback.cfg`, extracting the kernel, first initrd, and command line from the
  first bootable `menuentry` and substituting `${iso_path}` with the real path — so the
  distribution's exact iso-find token (`iso-scan/filename=`, `findiso=`, `root=live:`) is
  reused with no per-distro knowledge. When no usable `loopback.cfg` exists (and always for
  a directly-booted optical disc, which has no `${iso_path}`), LamBoot fingerprints the
  distro family by marker paths and applies a static recipe table covering six families —
  arch, ubuntu-casper, debian-live, fedora, opensuse, alpine — each transitively covering
  its derivatives (Mint, Pop!_OS, EndeavourOS, Manjaro, CachyOS, RHEL, Rocky, Alma, Nobara,
  and more). Each recipe carries a `media_cmdline` ({label}, disc self-locates by volume
  label) and a `file_cmdline` ({iso}, loop-booted from a file). Kernel resolution takes the
  first candidate that resolves (with trailing-`*` prefix-glob support for version-stamped
  names); initrd resolution concatenates every candidate that resolves, microcode first.
  The fedora recipe is keyed on `/LiveOS/squashfs.img` so it survives the Fedora 44 layout
  change. NixOS and Gentoo are intentionally excluded. *Validation status: only Arch 2026.05
  and Fedora 44 are live-booted end to end (A2 live-VM matrix pass 1); the other four
  families and all derivatives are recipe/table-validated and host-unit-tested, not yet
  live-booted.* El Torito chainload (Path B) is not yet wired.

### Changed

- **`FatRo` backend now consumes the published `lamfat` crate.** The native read-only FAT
  backend (for non-ESP FAT volumes the firmware refuses to serve) previously depended on a
  git-pinned commit of `rust-fatfs`; it now depends on `lamfat = "=0.4.0"` from crates.io —
  Lamco's republish of exactly that commit — imported under the local name `fatfs`
  (`package = "lamfat"`) so the backend code is byte-for-byte unchanged. Behavior is
  identical; this removes the last git-pinned dependency from the filesystem-backend layer
  and brings the FAT reader under the same exact-pin drift discipline as the other readers.
  Still read-only by construction — the `FatRo` tag is rejected by the ESP writer.
- **Stable Rust toolchain (nightly pin dropped).** `rust-toolchain.toml` switches its
  channel from `nightly` to `stable`: `lamboot-core`, the diagnostic modules, and both the
  `x86_64-unknown-uefi` and `aarch64-unknown-uefi` targets compile with zero nightly
  feature gates (verified on rustc 1.96.0). The UEFI crates still build
  `core`/`alloc`/`compiler_builtins` from source via the per-crate `-Z build-std` flags;
  those unstable cargo flags are unlocked on a stable compiler through `RUSTC_BOOTSTRAP=1`
  rather than a nightly toolchain. This is what makes a from-source build on a distribution's
  archive rustc possible.
- **Dependency pins aligned to the Debian archive.** The build now pins `goblin =0.9.3`
  (down from 0.10.5 — the PE loader uses only `Header::parse` and the `data_directories`
  getters and bypasses goblin's full parser, so 0.9 suffices and avoids a 0.10 transition
  that breaks seven archive rdeps), `lambutter =0.3.1`, `lamlvm =0.1.1`, and `sha2 =0.10.9`.
  Read behavior is identical; this is a provenance/buildability change so the same source
  compiles unmodified against Debian's library set.

### Fixed

- **Native ZFS backend not reached at boot (driver shadowing + slot-0 uberblock probe).**
  A live OVMF/QEMU boot against a real ZFS pool exposed two runtime-only bugs that
  prevented the native ZFS backend from ever mounting. First, the bundled GPL EfiFs
  filesystem drivers were loaded for any filesystem not marked natively covered; ZFS and
  XFS were missing from that list, so the legacy EfiFs driver attached to the partition
  `BY_DRIVER` and exposed it as a generic `SimpleFileSystem`, shadowing the native backend
  — and the native probe's exclusive open is then refused on firmware that will not
  force-disconnect the driver. The fix marks `zfs_`/`xfs_` driver filenames as natively
  covered so those drivers are skipped (exFAT ships no legacy driver and needs no entry).
  Second, the ZFS superblock probe only inspected slot 0 of the 128 KiB uberblock array,
  but the active uberblock rotates by `txg % slot_count` and the on-disk txg starts at 4,
  so a freshly created pool leaves slot 0 zeroed and the probe missed the pool entirely;
  the probe now scans the whole array at the 1 KiB minimum-slot stride and accepts the
  uberblock magic in either endianness (also fixing a prior big-endian `u32` read that
  never matched a little-endian x86 pool). After both fixes a live boot probes the pool and
  mounts its root, `BOOT`, and `BOOT/test` datasets end to end.
- **Graceful abort instead of panic on UEFI-helpers init failure.** The entry point and the
  four diagnostic modules now return `Status::ABORTED` when `uefi::helpers::init()` fails,
  instead of panicking via `.expect(...)`. On firmware where helper init cannot complete
  this surfaces a clean UEFI error status rather than an unwinding panic. Part of a wider
  lint cleanup that also resolved advisory `unwrap_used`/`expect_used` sites and removed
  three dead functions; no functional change beyond the init path.

### Infrastructure

- **Filesystem readers unified on one `BlockSource` seam.** The storage layer was
  refactored onto a single block-source abstraction with no change to boot or read
  behavior. `block_source.rs` introduces the `BlockSource` trait (Seam A) and one generic
  `SourceReader<S>` adapter (Seam B) whose single `fill_at` loop implements both
  `ext4-view`'s `Ext4Read` and `lambutter`'s `BlockRead`, replacing two byte-for-byte
  duplicated LV adapters. Seam C then folds the three near-identical UEFI `BlockIO` readers
  into one `BlockIoSource`, with the block-alignment math (`compute_aligned_read`) moved
  verbatim and still fuzzed (71.5M execs, 0 crashes). Whole-partition ext4/btrfs now mount
  over `SourceReader<BlockIoSource>`. A source-generic `dispatch_fs_over_source` /
  `probe_source_superblock` API also lands; it is the plumbing the new XFS/media/boot-from-ISO
  paths dispatch through. 168 host tests pass.
- **Lint-clean across UEFI and host crates.** `lamboot-core` and the `lamboot-fs-tests`
  host suite build clean of clippy and compiler warnings: unused goblin imports removed,
  dead functions deleted, infallible-conversion and acronym lints suppressed with
  documented `#[expect(...)]` reasons. No behavior change.
- **Pre-commit hook brought into lockstep with CI.** The placeholder-marker scan
  (`XX` / `FIXME` / `PLACEHOLDER` / `YYYY`) now also covers the shared `lib/` shell
  libraries, and the pre-commit hook now runs the stricter acyclic module-graph check
  (`check-layers.py --graph`) plus the same placeholder scan CI runs. This closes a gap
  where a `block_source` ⇄ `fs_backend_lvm_dispatch` import cycle could pass the local hook
  but fail CI.

## [0.15.2] — Lamco Development LLC relicense + published-crate dependencies

A packaging and provenance release: no change to boot behavior. Copyright moves
to Lamco Development LLC following the LLC's formation and executed IP
assignment, and the two `no_std` reader dependencies are now sourced from their
published crates.io releases instead of in-repo path dependencies.

### Changed

- **Copyright reassigned to Lamco Development LLC.** Source headers and the
  `LICENSE-MIT` / `LICENSE-APACHE` files now name Lamco Development LLC as the
  copyright holder. License terms are unchanged: LamBoot remains dual-licensed
  `MIT OR Apache-2.0`.
- **`lamboot-core` consumes `lambutter` and `lamlvm` from crates.io.** The
  read-only btrfs reader (`lambutter` `=0.3.0`) and the LVM reader (`lamlvm`
  `=0.1.0`) are now independently published library crates under the LLC;
  `lamboot-core` builds against the exact published versions rather than in-repo
  path dependencies. Read behavior is identical: this is a dependency-sourcing
  and provenance change that lets the readers stand as reusable open crates in
  their own right.

### Infrastructure

- SBAT version string aligned to `0.15.2`. The SBAT generation is unchanged
  (column 2 stays `1`): this is a human-facing string only and carries no
  revocation-enforcement semantics.

## [0.15.1] — Native Read-Only FAT Reader (FatRo) + Read-in-Place `/boot`

Closes LamBoot's last firmware dependency in the `/boot` read path, and lets
the installer source the kernel from a separate `/boot` *in place* rather than
mirroring kernels onto the ESP. Until now FAT was the one filesystem LamBoot
leaned on the firmware to serve (ext4 / btrfs / xfs / f2fs / LVM are all read
natively). That dependency is not unconditional: on OVMF/i440FX a *secondary*
FAT partition — a separate `vfat` `/boot`, the typical artifact of a BIOS→UEFI
migrated Linux box — gets a `SimpleFileSystem` handle whose `open_volume()`
returns `EFI_UNSUPPORTED`, and `connect_controller` does not repair it. Such a
`/boot` was unreadable. LamBoot now reads it natively and installs to it
directly.

### Added

- **Native read-only FAT backend (`FatRo`)** via the `fatfs` crate
  (`fs_backend_fat_ro`). `BlockIoFatAdapter` bridges UEFI `BlockIO` into fatfs's
  `no_std` IO traits (reusing the ext4 backend's block-alignment math).
  Read-only **by construction**: a distinct backend tag (`fatfs-ro@…`) that
  `EspWriter` rejects, so the native FAT path has no constructible write
  surface — *its code cannot modify your `/boot`*. BlockIO is opened
  non-exclusively (the firmware FAT driver holds it `BY_DRIVER`; an exclusive
  open would disconnect it).
- **FAT-on-LVM** (`fs_backend_lvm_fat`). `FatRoBackend` is generic over its
  fatfs IO source, so the same reader serves both a raw partition and a logical
  volume. The LVM-FS dispatcher now identifies and mounts FAT on an LV,
  completing the set of LamBoot's native read drivers (ext4 + btrfs + FAT)
  behind one uniform dispatch.
- **Read-in-place BLS placement.** `lamboot-install` auto-detects when `/boot`
  is a separate partition LamBoot reads natively (vfat via `FatRo`; ext2/3/4 +
  btrfs natively) and writes BLS entries **on `/boot`** so the kernel is sourced
  in place — no ESP kernel mirror, no copy-to-ESP band-aid. Non-native
  (`xfs`/`f2fs`/`zfs`) and `/boot`-is-the-ESP layouts keep ESP staging. The
  decision is recorded in the install report (`bls_placement` event), and
  `lamboot-kernel-hook` mirrors it so kernel upgrades keep entries on `/boot`.
- **systemd `kernel-install` plugin** (`kernel-install/90-lamboot.install`).
  Kernel-event integration for Arch / Fedora / openSUSE (Debian-family already
  had the `postinst.d` hooks): a kernel upgrade now regenerates the LamBoot BLS
  entry via `lamboot-kernel-hook`, the single BLS writer.
- **`/boot` mount provenance in the trust log.** The XBOOTLDR mount now emits a
  `volume_mounted` event naming the backend that read `/boot` (`fatfs-ro` vs
  firmware `fat`), matching the ext4/btrfs/LVM mounts — and an
  `xbootldr_mount_failed` event when a `/boot` partition exists but cannot be
  mounted, so an unreadable `/boot` is auditable rather than silent.

### Changed

- **XBOOTLDR mount is now two-tier.** `mount_xbootldr` uses the firmware FAT
  driver only when `open_volume()` actually succeeds, and otherwise falls back
  to the native `FatRo` reader over `BlockIO`.
- **Dead firmware-FAT handles are skipped.** `enumerate_fat_volumes` probes
  `open_volume()` and drops handles the firmware exposes but won't serve, so a
  dead handle can't shadow the live `FatRo` volume.

### Fixed

- **`lamboot-mark-success.service` is installed again.** The source unit had
  gone missing (only a stale `dist/` copy survived), so the crash-counter-reset
  service was never installed and `LamBootCrashCount` climbed every boot toward
  the anti-bootloop threshold. Restored the unit; the installer now fails
  loudly if the mandatory unit is absent instead of skipping silently. Same
  fix applied to the `kernel-install` plugin's previously-silent skip.
- **`lamboot-install --remove` handles read-in-place entries.** BLS entries on
  a separate `/boot` are tracked with a `boot:` manifest prefix; `--remove` now
  resolves and removes them (and `--keep-entries` keeps them) instead of
  orphaning them on `/boot`.
- **`lamboot-install --remove --dry-run` is non-destructive.** It previously
  deleted the install manifest and `rmdir`'d LamBoot's ESP directories even in
  a preview run, leaving the install unremovable on a subsequent real run.
- **The install manifest no longer drops LamBoot's own BLS entries** on a
  covered `--update` re-run (it overwrote with only freshly-generated entries;
  it now re-records our existing entries, leaving native-tooling entries
  untracked).
- **`lamboot-kernel-hook` resolves flavor-named initramfs** (Arch's
  `initramfs-linux.img`, keyed on the kernel image flavor rather than the
  version), so generated entries are not shipped without an `initrd` line.
- **CI:** the cargo jobs (which need the private sibling path-deps) now skip
  cleanly when `SIBLING_REPOS_TOKEN` is unset, instead of hard-failing at
  checkout.

## [0.14.1] — Robustness Restoration + Test Suite

A post-v0.14.0 diff scan (late-0.11.x → 0.14.0) found that several of the
v0.13/v0.14 hardening fixes had narrowed real-world flexibility the installer
and loader were deliberately calibrated for across the support matrix (distros,
firmware/efibootmgr versions, ESP layouts, degraded recovery hosts, and
update/upgrade scenarios). This release restores that flexibility **without
giving back the security property each fix established**, and adds a
dual-contract test suite so neither half can silently regress again. No
happy-path boot behavior changes.

### Fixed

- **`find_lamboot_entry` matches inactive and modern-format NVRAM entries
  again.** The v0.14.0 end-of-line anchor missed entries carrying efibootmgr's
  `\t`-device-path suffix (modern efibootmgr, even without `-v`) and the
  two-space inactive (`%c`=space) flag — causing duplicate entries on
  `--update`/`--remove`. The match is still anchored to the exact `LamBoot`
  label, so a foreign entry whose loader path merely contains `\EFI\LamBoot\`
  is not targeted.
- **`is_genuine_shim` authenticates real shims on degraded hosts.** Added an
  `od` MZ/PE header fallback when `file` is absent (BusyBox/Alpine rescue,
  minimal containers) and a `tr -d '\0'` UTF-16 strip so pre-SBAT shims
  (Ubuntu 20.04 / RHEL 8 era, identified only by their MokManager handoff
  strings) are recognized — marker 2 had been grepping UTF-16 as ASCII and was
  dead. Still rejects GRUB (incl. the `shim_lock` verifier module),
  systemd-boot, and bare PE binaries as the Secure Boot anchor.
- **`read_keyval_field` reproduces `source` semantics for all distro forms.**
  Restored CRLF (Windows-built image) stripping, inline `# comment` handling,
  and take-up-to-the-matching-quote behavior — while still parsing rather than
  sourcing, so `$(...)`, backticks, `${VAR}` and `;`/`|`/`&` in a `--root`
  target's os-release stay inert literal data.
- **`is_esp_partition` re-accepts MBR `0xef` media.** Removable USB boot media
  and prebuilt appliance images report the MBR EFI System type byte, not the GPT
  ESP GUID; these are ESPs and worked before the security narrowing.
- **`detect_secure_boot` no longer over-fails-closed.** A `NOT_FOUND`/
  `UNSUPPORTED` firmware reports off (an unsigned kernel boots); an oversized
  variable (`BUFFER_TOO_SMALL`) reports on; a single transient read retries once
  before failing closed. The `ShimLock` protocol is re-probed at kernel-verify
  time so a shim < 15.8 that uninstalled it (consumed by a legacy FS driver for
  XFS/ZFS `/boot`) degrades `ActiveWithShim`→`ActiveDirect` instead of
  hard-rejecting a kernel that should fall to the firmware `db` path.
- **Full-file read cap raised 256 → 512 MiB.** The 256 MiB value was mirrored
  from `MAX_IMAGE_SIZE` (which bounds a relocated PE image) and was never sized
  against initrd reality; enterprise/HPC initrds carrying driver + firmware
  blobs reach ~300–400 MiB and were rejected outright → unbootable with no
  fallback. The cap still bounds a hostile multi-GiB size field.
- **Crash-loop with no fallback shows a recovery menu** (8 s timeout, defaulting
  to the recovery entry) instead of silently auto-booting the looping entry.
- **`sync-versions.sh --bump` re-resolves `Cargo.lock`** so a `--locked` build
  works immediately after a bump; `sign-lamboot.sh` resolves `objcopy` only when
  an SBAT csv is actually being embedded.

### Added

- **Dual-contract test suite.** 94 bats tests over the installer
  (`tools/tests/installer/`) and 28 host tests over extracted decision cores
  (`lamboot-fs-tests/`), each pinning both a security must-reject and a
  flexibility must-accept-across-the-matrix case. CI gains an `installer-tests`
  (bats) job; the pre-commit hook runs bats when present.

### Changed

- **Internal:** the load-route, full-file-read cap, and Secure Boot
  read-classification decisions were split into pure (no-UEFI) modules
  (`boot_route_pure`, `read_limit_pure`, `sb_classify_pure`) that the host test
  crate includes verbatim; production delegates into them with identical
  behavior.

## [0.14.0] — Security Hardening Release

Closes **43 of 54** actionable findings from a full security/architecture audit — **no CRITICAL or
HIGH remains open** (those landed in v0.13.0). The bulk of this release is
defensive hardening on degraded / attacker-influenced paths; the happy-path boot
is unchanged. It also adds three new installer safety flags. Each non-trivial
change was adversarially reviewed before merge.

### Added

- **`--replace-fallback` (installer).** Overwriting a foreign bootloader at the
  firmware fallback path (`\EFI\BOOT\BOOTX64.EFI`) now requires this dedicated
  opt-in; generic `--force` no longer authorizes it, so a `--force` added for an
  unrelated check can't bury another OS's loader as a side effect.
- **`--force-foreign-esp` (installer).** Writing to a vfat partition that is not
  GPT-typed as an ESP (removable-media / foreign-disk prep) now requires this
  dedicated flag; generic `--force` no longer bypasses the ESP partition-type
  assertion.
- **`sync-versions.sh --bump-sbat-generation`.** Explicit, dry-run-able helper
  that increments the SBAT revocation generation, with a read-back check and the
  mandatory manual follow-up printed. Never run by `--sync`/`--check`/`--bump`.

### Security

- **Honest trust log.** `image_verified` now records `SUCCESS` only when a
  signature was actually validated; `SKIPPED` (SB off), `DEFERRED` (db-direct,
  firmware verifies later), and `REJECTED` (shim refused) cover the rest. TPM
  measurement events are emitted only when the PCR was really extended
  (`*_measurement_skipped` otherwise), so the log never claims a measurement
  that didn't happen.
- **Secure Boot detected once per boot** and threaded through the report, trust
  log, verify path, and load-path routing — they can no longer disagree.
- **Two-phase NVRAM commit (installer).** The UEFI boot entry / `BootOrder` is
  written only after the loader (and, under Secure Boot, shim + grub) is
  confirmed present on the ESP; the pre-install `BootOrder` is recorded for
  `--remove` rollback.
- **Genuine-shim authenticity (installer).** The Secure Boot anchor must be a
  real shim (SBAT product row or MokManager handoff), not any PE32+ parked at a
  shim path — a foreign GRUB/systemd-boot is no longer trusted as the anchor.
- **Installer secret + target hygiene.** First-boot log created `0600`; the MOK
  one-time password goes to the console only, never the log; NVRAM entries are
  matched by exact label (not an unanchored substring); `--root` `os-release` /
  `grub-default` are parsed as data, never `source`d; ESP writes are
  realpath/symlink-guarded against off-volume redirection.
- **SBAT scope** documented as a reviewed decision (`docs/SECURITY-MODEL.md`):
  enforced via shim where present; kernels carry no `.sbat` (ecosystem gap).

### Fixed

- **Degrade instead of brick on the boot-critical path.** A firmware that
  rejects `set_watchdog_timer`, read-only/full NVRAM, a crash loop with no
  configured fallback, and GUI/menu init or run failure are all now recoverable
  (text-menu fallback, deterministic non-interactive recovery boot) instead of
  panicking or rebooting to firmware setup. The crash counter is no longer reset
  on a failed boot, so crash-loop detection actually engages.
- **Bounded all attacker/firmware-controlled allocations and reads.** Full-file
  reads are capped (`FileTooLarge`); SMBIOS/ACPI/UKI/DMAR/IVRS length fields are
  validated before `from_raw_parts`; saturating arithmetic on header-supplied
  offsets/counts.
- **First-boot signing flag matches the install mode** — an unsigned `--root`
  install no longer loops forever re-invoking `--update --signed`.

### Performance

- **Dirty-rect boot menu** — the framebuffer is no longer cleared + recomposited
  + blitted every frame when nothing changed (idle and mouse-move).
- **Cache-miss reads copy the buffer once, not twice** — removes a full-buffer
  memcpy from every kernel / initrd / UKI load.

### Build

- **Signing pipeline hardened.** `sign-lamboot.sh` / `build.sh` assert the
  `.sbat` section is present after signing (PE-parse, not `objdump`), treat a
  GNU-objcopy fallback as a hard error (it corrupts PE), build `--locked`, and
  no longer track per-version release-tarball directories.

## [0.13.0] — Layer Contract Release

Makes the eight-layer architecture **true and machine-enforced** instead of an
aspirational claim, and closes the highest-severity findings from a full
security/architecture audit. No boot-behavior change on the happy path; the
trust-chain fixes change what happens on degraded/attacker-influenced paths.

### Changed

- **The layer architecture is now enforced, not just documented.** Every
  `lamboot-core` module carries a `//! Layer: N` declaration; `tools/layer-map.toml`
  is the machine-readable source of truth; and `tools/check-layers.py` fails the
  build (pre-commit hook **and** the new CI workflow) on a missing declaration or
  an upward dependency. The module dependency graph is a verified acyclic DAG.
  Previously the contract in `docs/ARCHITECTURE-LAYERS.md` was contradicted by
  the code (0 of 45 modules declared a layer; several modules depended upward).
- **Extracted `boot_types.rs` (Layer 3).** The shared `BootEntry`/`EntryKind`/
  `Icon` types and the preflight result types moved out of `discovery.rs`
  (Layer 7) and `preflight.rs` (Layer 4) into a low-layer types module, so
  `partitions`, `report`, `console`, and `gui` no longer depend upward on the
  orchestration layer for a type definition.
- **Moved `select_default_entry` to `policy.rs` (Layer 4).** Default-entry
  selection is policy application, not discovery; it now names only the pure
  `bls_parse::version_compare`, removing the last upward edge from the
  presentation layer into `discovery`.
- **Corrected the layer map to match reality:** `fs.rs` is Layer 2 (it was
  mislabeled Layer 1); `bls_parse.rs` is the pure Layer-3 BLS parser while
  `bls.rs` is a Layer-4 coordinator (the doc had them swapped); the write path
  (`fs_writer`) and the cross-cutting audit modules (`diag`, `version`,
  `trust_log`, `telemetry`) now have explicit homes in the model.
- Documentation swept to match: `docs/ARCHITECTURE-LAYERS.md` rewritten as the
  authoritative, enforced model; `README.md`, `docs/DEVELOPER-GUIDE.md`,
  `CONTRIBUTING.md`, `docs/specs/SPEC-FS-BACKEND-TRAIT.md`,
  and `CLAUDE.md` updated; stale line-count/binary-size/module-count figures
  corrected (≈16,000 lines, 46 modules, ~600 KB).

### Fixed (security — from the 2026-05-31 audit)

- **Native PE path no longer executes db-signed-Secure-Boot kernels without a
  signature check.** Under `ActiveDirect` (SB on, no shim) the native loader
  performed no cryptographic verification yet recorded the load as `SUCCESS`.
  `choose_load_path` now routes `ActiveDirect` kernels to the firmware
  `LoadImage` path, where the firmware `db` signature check actually runs.
- **`detect_secure_boot` now fails closed.** A read error on the `SecureBoot`
  variable previously fell through to `Disabled` (no verification); only a
  genuine "variable absent" now maps to Disabled — any read error is treated as
  active so the load path verifies rather than skips.
- **`chainload_efi` now measures into the TPM and records a trust event** before
  `start_image`, so chainloaded binaries no longer break the measured-boot chain
  or leave a hole in the audit log.
- **The installer no longer executes untrusted distro files.** `detect_distro`
  and the GRUB-cmdline migration parsed the target's `/etc/os-release` and
  `/etc/default/grub` by `source`-ing them, which ran attacker-controlled shell
  as root under `--root`. They are now parsed as data.

### Added

- `tools/check-layers.py` + `tools/layer-map.toml` — the layer-contract gate.
- `.github/workflows/ci.yml` — CI running fmt, clippy, both-target builds, host
  tests, the layer gate, and a placeholder scan (the project previously had no
  CI workflows).

## [0.11.22]

### Fixed

- **`lamboot-install --update` no longer wipes BLS entries on separate-`/boot`
  layouts.** The stale-entry check (`our_bls_entries_are_stale`,
  `remove_stale_our_bls_entries`) tested the BLS `linux` field as a
  running-root-absolute path. On a system with a **separate `/boot` partition**
  the field is `/vmlinuz-X` (file at `/boot/vmlinuz-X`), so every entry was
  falsely judged "kernel missing" → removed; the coverage gap-fill then skipped
  regeneration (its `COVERED_VERSIONS` snapshot predated the removal), leaving
  an **empty boot menu**. The check is now layout-aware (an entry is stale only
  when its kernel is absent at **both** `linux` and `/boot$linux`), and
  `COVERED_VERSIONS`/`EXISTING_BLS` are re-scanned after any removal so a
  genuinely-removed entry is regenerated instead of skipped. Default `--update`
  thus preserves valid entries, removes only truly-dead ones, and gap-fills only
  what is missing.

## [0.11.21]

Makes Proxmox-host kernel integration actually maintain LamBoot's boot menu.
On a Proxmox host with no separate `/boot` partition (LVM/ZFS root), LamBoot
discovers kernels from BLS entries on the **ESP** (`/boot/efi/loader/entries/`).
The prior code wrote entries to the root-filesystem `/boot/loader/entries/`,
which LamBoot never scans on that layout, and omitted `root=` from the options
— so newly installed Proxmox kernels did not appear in (or could not boot from)
LamBoot's menu. The per-kernel writer and the bulk refresh now agree on one
correct, ESP-targeted, byte-compatible entry format.

### Fixed

- **Boot freeze on `conout_fat_coupling` firmware (ASUS G10AJ / pve2).** The
  per-site `log::` gating left a gap: the v0.11.15 LVM-FS dispatch refactor
  added ungated `log::info!`/`log::warn!` in the LV-open path, which fired on
  every LVM-root boot and — on firmware where a ConOut write stalls the next
  FAT op — froze the boot before the menu (independent of `[diagnostics]
  verbose`). The `log` facade is now suppressed globally
  (`set_max_level(Off)`) once `conout_fat_coupling` is detected, so no macro
  can emit ConOut on this firmware. The `true_append` trust/audit logs are
  unaffected. Verified: pve2 boots cleanly via LamBoot.
- **Verbose diagnostics are now safe to enable on `conout_fat_coupling`
  firmware.** `diag::append` breadcrumbs previously did a `true_append` FAT
  write per call; on coupling-prone hardware those per-call writes in the
  post-selection band are a risk. When `conout_fat_coupling` is detected,
  breadcrumbs now coalesce in a bounded RAM buffer and flush in a single
  `true_append` before each handoff (`start_image`), so enabling
  `[diagnostics] verbose = true` to investigate a freeze can no longer
  contribute FAT-write churn to the band. Inert when verbose is off.
- **ESP is the canonical BLS location on Proxmox hosts.** `--refresh` and the
  install-time backfill now write `loader/entries` on the ESP via the single
  writer (`lamboot-kernel-hook`), instead of the inert root-fs
  `/boot/loader/entries/lamboot-*.conf` scheme, which is retired and purged.
- **`root=` is injected when absent.** Both `lamboot-kernel-hook` and
  `get_kernel_cmdline` add `root=<dev> ro` when the cmdline source omits it
  (the Debian/Proxmox convention), so entries are bootable. No-op when `root=`
  is already present, so non-Proxmox installs are unaffected.
- **Layout-correct kernel/initrd paths.** Entries keep the `/boot/` prefix when
  `/boot` lives on the root filesystem and drop it when `/boot` is its own
  partition.

### Added

- **`--repair-bls`** — explicit force-rebuild of the ESP BLS entries (re-derive
  `/etc/kernel/cmdline` from GRUB, overwrite all, prune removed kernels).
  Default `--refresh` and per-kernel hooks remain **additive** and never modify
  existing entries, preserving operator customizations.
- **Proxmox-host detection feedback** — install confirms the detected mode
  (PATH A/C) and warns clearly when run on a Proxmox host without
  `--proxmox-host`, or with it on a non-Proxmox host.

## [0.11.20]

Hardens the first-boot UEFI boot-entry setup so a native (`--root`) install
reliably ends up with a named LamBoot `Boot####` entry instead of silently
falling back to the removable-media path. Surfaced on an Arch install whose
v0.11.9 first-boot run could not find `lamboot-install` in the target
(`/usr/bin` was not searched, and the toolkit was not yet bootstrapped into the
chroot), then marked itself done and disabled the service permanently, so the
box booted via `\EFI\BOOT\BOOTX64.EFI` from then on.

### Fixed

- **First-boot retry (`tools/lamboot-install`, `stage_first_boot_script`):** the
  staged `lamboot-first-boot.service` no longer touches its done-marker and
  disables itself unconditionally. It now confirms the LamBoot `Boot####` entry
  exists via `efibootmgr` and self-disables only on that success; if the entry
  cannot be created yet (toolkit or `efibootmgr` not present), it leaves the
  service enabled and retries on the next boot. The one-time steps (MOK
  enrollment, ShimRetainProtocol) are gated by their own marker so they still
  run exactly once, and the unit's `ConditionPathExists=!.../first-boot-done` is
  dropped so the marker no longer blocks retries.

## [0.11.19]

Generalizes boot-entry integration in `lamboot-install` so a kernel installed
while no entry generator was active is no longer left unbootable. Surfaced on
an openSUSE Tumbleweed host where a freshly installed 7.0.9 kernel had no BLS
entry (the install predated the `90-lamboot.install` plugin, and
`kernel-install` was running at `layout=other`), so LamBoot kept booting the
prior kernel.

### Fixed

- **Per-kernel BLS coverage (`tools/lamboot-install`, phase 5):** generation
  previously deferred entirely to native tooling whenever any non-LamBoot BLS
  entry existed, leaving kernels installed during a no-generator window
  uncovered until `--force`. It now detects the native entry manager and
  per-kernel coverage (matched by the BLS `version` field, so native entries
  are never duplicated) and gap-fills only the kernels that lack an entry.

### Added

- **Future-kernel coverage (`tools/lamboot-install`, phase 7):** on
  systemd-`kernel-install` distros reporting `layout=other`, the installer now
  pins `layout=bls` in `/etc/kernel/install.conf` so the distro's own
  loader-entry plugin keeps entries fresh alongside `90-lamboot.install`.
- **Coverage verification (`tools/lamboot-install`, phase 8):** asserts every
  installed kernel has a boot entry and that an ongoing kernel-event hook is
  present, emitting `kernel_uncovered` / `kernel_hook_missing` events otherwise.
- **`detect_boot_entry_manager`:** records the native entry manager
  (kernel-install + layout / sdbootutil / proxmox-boot-tool / debian-hooks) to
  drive the above. Honors `--no-bls`; `--force` still regenerates all entries.

## [0.11.18]

Closes the third archinstall plugin chroot-fixup bug (#98). Sibling to
#95 (fw_cfg) and #97 (btrfs subvol suffix); the test thread that found
this also produced clean validation of all three prior fixes across
btrfs, btrfs+LVM, and ext4+LUKS+LVM stacks.

### Fixed

- **Bug #98 (`tools/lamboot-install` → `ensure_mkinitcpio_hooks`):**
  archinstall sometimes emits an internally inconsistent
  mkinitcpio.conf when LUKS is in the storage stack — HOOKS uses the
  *legacy* udev-style `encrypt` hook with the `udev` initramfs init,
  but the BLS entries lamboot-install writes use systemd-cryptsetup-
  generator cmdline format (`rd.luks.uuid=...` + `rd.luks.name=...`).
  These two are not interchangeable: the legacy `encrypt` hook reads
  `cryptdevice=UUID=...:NAME` from cmdline and ignores `rd.luks.*`.
  Result: the kernel waits 90 seconds for `/dev/disk/by-uuid/<root>`,
  then drops to emergency. archinstall also placed `block` AFTER
  `encrypt lvm2` in the HOOKS list, which is wrong order even if the
  legacy/systemd mismatch were resolved.

  Phase4c's previous dedup logic — `grep -qE '\b(sd-encrypt|encrypt)\b'`
  — treated either hook as equivalent and skipped the addition. That
  was correct dedup but wrong reconciliation for this case.

  Fix: when needs_encrypt is detected AND the legacy `encrypt` hook is
  present AND `sd-encrypt` is NOT, rewrite the entire HOOKS line to
  the canonical systemd-style equivalent:

  ```
  HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block sd-encrypt lvm2 filesystems fsck)
  ```

  (without `lvm2` when needs_lvm=0). `udev` → `systemd`; `encrypt`
  → `sd-encrypt`; `keymap` and `consolefont` dropped (sd-vconsole
  covers both); `block` reordered to precede `sd-encrypt`. Backup
  of the archinstall original is preserved at
  `${mkinitcpio.conf}.lamboot-${LAMBOOT_VERSION}.bak`.

  Scope discipline: only fires when LUKS is in the stack. Pure
  ext4-on-LVM, btrfs-anywhere, plain partition layouts continue
  through the existing additive sd-encrypt/lvm2 placement logic
  unchanged. Operators with unusual custom HOOKS (mdadm_udev,
  dropbear, custom decryption agents) will have those dropped if they
  also use legacy `encrypt` — but those layouts don't come from
  archinstall defaults anyway.

  Caught on VM 366 ext4+LUKS+LVM archinstall test (v0.11.17). Manual
  unblock was rewriting HOOKS + chroot mkinitcpio -P; v0.11.18 makes
  that automatic.

### Validation

Unit test on a synthetic mkinitcpio.conf matching the VM 366 broken
pattern:

```
in : HOOKS=(base udev autodetect microcode modconf kms keyboard keymap consolefont encrypt lvm2 block filesystems fsck)
out: HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block sd-encrypt lvm2 filesystems fsck)
```

Backup preserved. Diff shows single-line change.

End-to-end regression on a fresh ext4+LUKS+LVM archinstall is pending
operator-driven next test run.

### Files changed

```
tools/lamboot-install   (47 lines — fix + comment block)
```

## [0.11.17]

Single-bug patch release. archinstall plugin now reliably adds lvm2 /
sd-encrypt HOOKs to the chroot's mkinitcpio.conf when the chroot root
is mounted as a btrfs subvolume.

### Fixed

- **Bug #97 (`tools/lamboot-install`):** `phase4c_chroot_initramfs_fixup`
  silently skipped LVM/LUKS detection when the chroot root is a btrfs
  subvolume. `findmnt -o SOURCE,FSTYPE --target $OPT_ROOT` returns
  `/dev/mapper/<vg>-<lv>[/@]` for a btrfs-subvol mount (the `[/@]` is
  findmnt's subvolume annotation, not a real path component). phase4c
  passed the raw value to `lsblk -snro TYPE $root_src`, which treats it
  as a nonexistent path, exits 32 with empty stdout (suppressed by
  `2>/dev/null`). The case-on-types match neither lvm nor crypt;
  phase4c logged "Plain partition root (types: )" and returned 0
  without touching mkinitcpio.conf. Initramfs got the archinstall
  default HOOKS (no lvm2, no sd-encrypt) — kernel couldn't activate
  the LV or unlock the LUKS volume, and systemd waited ~90 seconds at
  `Started Reach target Local File Systems` before dropping to
  emergency.

  Caught on VM 365 v0.11.16 archinstall test (btrfs + LVM + zstd
  compression). Manual unblock was edit mkinitcpio.conf + chroot
  mkinitcpio -P; v0.11.17 makes that automatic on every install.

  Fix: `root_src="${root_src%%[\[]*}"` — bash parameter expansion
  strips from the first `[` onwards before the lsblk call. No subshell,
  no fork.

  Coverage of the bug (and the fix):
    - btrfs + LVM             → now correctly detected, lvm2 added
    - btrfs + LUKS            → now correctly detected, sd-encrypt added
    - btrfs + LUKS + LVM      → both correctly added in spec-order
    - plain btrfs (no extras) → was correctly handled (no hook needed); fix is a no-op there
    - ext4-anywhere paths     → unaffected (no subvol suffix produced by findmnt)

### Files changed

```
tools/lamboot-install   (11 lines — one-line fix + 10 lines of comment)
```

### Not yet end-to-end tested

The bug fix code path is sed-by-eyeball-clean, but the canonical
regression test (fresh archinstall on btrfs+LVM, no manual intervention,
should boot cleanly) is pending operator-driven test. VM 366 staged
with ext4+LUKS+LVM exercises the orthogonal sd-encrypt+lvm2 hook-pair
addition path, also pending.

## [0.11.16]

LamBoot core now consumes the host-supplied JSON blob exposed via
fw_cfg `opt/lamboot/config`. The host side of the Proxmox guest-
integration layer (lamboot-hookscript.pl v0.8.4+) was already correct;
this release closes the in-VM consumer loop.

### Fixed

- **Bug #95** (surfaced during VM 364 v0.11.15 guest-integration test):
  LamBoot's existing `fw_cfg::read_file_string("opt/lamboot/config")`
  call only logged the byte count and threw the content away. The
  pre-0.8.4 SMBIOS OEM string pathway was the only source of
  `fleet_id` / `vmid` in `BootContext`, which produced empty fields
  on every host using the v0.8.4+ hookscript (those write fw_cfg JSON,
  not SMBIOS strings — because `qm set --smbios=...` deadlocks on the
  VM config lock during pre-start).

  Fix: new `lamboot-core/src/fw_cfg_config.rs` module with a hand-rolled
  JSON parser for the flat `schema_version=v1` blob. Extracts
  `fleet_id`, `vmid`, `role`, and `hostname` (exposed as `host_node` to
  disambiguate from the guest's own hostname). 7 unit tests cover
  happy path + missing keys + empty values + arrays + escapes +
  malformed input + substring collisions (e.g. searching `vmid` must
  not match `old_vmid`).

  `main.rs` Phase 2.8 now invokes the parser. fw_cfg-derived values
  override the SMBIOS-derived values from earlier in Phase 2.
  SMBIOS stays as fallback for legacy hosts that haven't migrated
  to the v0.8.4+ hookscript.

  Also removed a redundant `let vmid = smbios::...` shadowing read
  in the menu-loop preamble that would have silently undone the
  fw_cfg override.

### Added

- **`role` + `host_node` fields in `boot.json`**. The boot report now
  surfaces the operator-assigned role from `/etc/lamboot/fleet.toml`
  `[roles]` and the Proxmox node name that started the VM. Useful for
  fleet observability + correlating guest boot records with specific
  hosts in clusters.

### Validated

- VM 364 cold-cycled with the v0.11.16 binary. boot.json now shows
  `fleet_id="lamco-pve2"`, `vmid="364"`,
  `role="lamboot-archinstall-target"`, `host_node="pve2"`. Host
  hookscript wrote JSON at 20:31:28; LamBoot parsed at 20:31:34;
  report written at 20:31:39. Same QEMU process exec'd with the
  `-fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/364.json`
  arg.

### Files changed

```
lamboot-core/src/fw_cfg_config.rs   (new — parser + 7 tests)
lamboot-core/src/main.rs            (Phase 2.8 wiring + remove shadow)
lamboot-core/src/report.rs          (BootContext + JSON output fields)
```

## [0.11.15]

archinstall plugin completeness pass + first-boot/manifest robustness.
Closes the seven bugs surfaced by the VM 364 end-to-end audit of v0.11.14.

### Context

VM 364 (Arch + plain btrfs, no LUKS/LVM, archinstall + niri) booted
LamBoot 0.11.14 correctly via the UEFI fallback path. But thorough
post-install inspection turned up seven issues that block a v1.0
production claim:

| # | Severity | Symptom | Root cause |
|---|---|---|---|
| 1 | crit | NVRAM Boot entry never created | efibootmgr absent from chroot (PKGBUILD depends bypassed) |
| 2 | crit | crash counter climbing monotonically | lamboot-mark-success.service not installed in chroot |
| 3 | crit | future kernel updates won't refresh BLS | pacman alpm hook not staged in chroot |
| 4 | mid | manifest stale referencing pre-rename entries | --update died at Phase 6 before reaching write_manifest |
| 5 | mid | no runtime metrics | host-sampler unit/timer absent |
| 6 | mid | misleading "No such file" log line | first-boot-nvram.sh hard-coded /usr/local/sbin path |
| 7 | mid | plugin version not auditable post-install | nothing preserved __file__ into chroot |

### Fixed

- **Bug 1 — `_pacstrap_chroot_packages`** (new in
  `packaging/installers/archinstall/lamboot_plugin.py`). Runs `arch-chroot
  /mnt pacman -S --needed --noconfirm efibootmgr` before lamboot-install.
  Conditionally adds `lvm2` / `cryptsetup` based on `findmnt` + `lsblk -s`
  inspection of the target's root stack. Defensive fallback if inspection
  fails: pacstrap all three.
- **Bug 2 + Bug 5 — `_TOOLKIT_UNITS_TO_ENABLE` + `_enable_chroot_unit`**.
  Plugin now copies `lamboot-mark-success.service` (mandatory) and
  `lamboot-host-sampler.service/.timer` (optional) into the chroot from
  the live env, then runs `systemctl --root=<target> enable <unit>` for
  each. Wires success-tracking + observability without operator action.
- **Bug 3 — `packaging/aur/lamboot/95-lamboot.hook`** (new shipped file).
  Pacman alpm hook, Path-triggered on `boot/vmlinuz-*` +
  `usr/lib/modules/*/vmlinuz` Install/Upgrade/Remove. PKGBUILD installs
  it to BOTH `/usr/share/libalpm/hooks/95-lamboot.hook` (where pacman
  reads it) AND `/usr/share/lamboot/aur/95-lamboot.hook` (where the
  archinstall plugin reads it for chroot bootstrap before the lamboot
  pkg is installed).
- **Bug 4 — `check_efi_prerequisites` in `tools/lamboot-install`** now
  marks `PARTIAL_FAILURE` and returns 1 (instead of `die`) when
  efibootmgr is missing AND we're under `--update` or `--refresh`. The
  caller (`phase6_efi_boot_entry`) honors the early return and lets
  Phase 7 + `write_manifest` still run. Result: the alpm hook's call
  to `lamboot-install --update --signed --quiet` after a kernel bump
  completes cleanly even on a host where efibootmgr was not pacstrapped,
  and the manifest reflects what's actually on disk.
- **Bug 6 — first-boot-nvram.sh emit** (lines 2649–2658 of
  `tools/lamboot-install`). Replaced hard-coded `/usr/local/sbin/...` +
  `/usr/sbin/...` with `LBI=$(command -v lamboot-install)` plus a
  defensive fallback over `/usr/bin /usr/sbin /usr/local/sbin`. Also
  short-circuits with a clear error when efibootmgr is absent instead
  of running lamboot-install and producing a misleading "deferred to
  operator" message.
- **Bug 7 — plugin self-preserve** at the tail of `_bootstrap_host_toolkit`.
  Copies `__file__` of the running plugin to
  `/usr/share/lamboot/installers/archinstall/lamboot_plugin.py` inside
  the chroot. Makes the running plugin version auditable post-install.

### Coexistence note (Bug 4 followup)

Arch ships `/usr/lib/kernel/install.d/90-loaderentry.install` in its
`systemd` pkg, alongside our `90-lamboot.install`. On Arch the former
is dormant for `pacman -Syu` because Arch's `linux` pkg uses mkinitcpio
presets rather than `kernel-install`, so the two don't actually race
during normal updates. The 95-lamboot.hook alpm hook is what fires on
kernel updates, and lamboot-install's Phase 5 (BLS regen) already
removes stale entries from any prefix before writing arch-*.conf. The
ownership question is therefore "self-healing on every update" rather
than "first writer wins" — which is good enough for v1.0. Distros
where `kernel-install` is the canonical kernel-update mechanism
(Fedora, Debian-derived) will exercise the cross-plugin coexistence
more sharply; deferred to those test tracks.

### Files changed

```
packaging/installers/archinstall/lamboot_plugin.py  (Bugs 1, 2, 5, 7)
tools/lamboot-install                               (Bugs 4, 6)
packaging/aur/lamboot/PKGBUILD                      (Bug 3 ship path)
packaging/aur/lamboot/95-lamboot.hook               (new — Bug 3)
```

### Not yet verified

End-to-end on a fresh archinstall + fresh kernel-update cycle. The
v0.11.14 audit on VM 364 produced this fix set; v0.11.15's own
end-to-end test follows on a fresh VM.

## [0.11.14]

LVM-on-FS dispatch refactor: replaces v0.11.13's trial-and-error
(try-ext4-then-try-btrfs) with a single PV/VG/LV parse + superblock
probe + single backend construction. Scales linearly to new backends.

### Context

v0.11.13 shipped the LVM-lambutter wiring (LvmBtrfsBackend), unblocking
LVM-on-btrfs root layouts. The integration used a cascading match:
ext4-backend `::open` first, on failure btrfs-backend `::open`. Each
`::open` re-did the full PV+VG+LV metadata parse, so the btrfs
fallback path paid double for LVM bookkeeping. The pattern also
doesn't scale to a third backend — adding `LvmXfsBackend` would mean
a 3-deep cascade with 3x metadata parses on the worst-case path.

### Fixed

- **New `fs_backend_lvm_dispatch` module** owns the LVM-on-FS dispatch.
  Three concerns separated:
    - `OpenedLv::open` — opens BlockIO + parses Lvm2 + opens named LV.
      One PV/VG parse per dispatch attempt. Shared infrastructure.
    - `probe_lv_superblock` — reads ~68 KiB from the LV reader, checks
      ext4 magic at offset 1024 and btrfs magic at offset 0x10000,
      rewinds the reader. Returns `LvFsKind`.
    - `open_lvm_lv_backend` — single dispatch entry. Returns
      `LvmDispatch` (boxed FsBackend + identity bundle). One call site
      replaces the v0.11.13 cascading match.
- **`LvmExt4Backend::from_lv_parts(reader, vg_lv)` factory** — receives
  pre-opened `OwnedLvReader`, loads ext4-view, returns wrapped backend.
  Old `LvmExt4Backend::open(handle, lv_name)` API removed; the
  dispatcher is the only mount path.
- **`LvmBtrfsBackend::from_lv_parts(reader, lv_len, vg_lv)` factory**
  — receives pre-opened `OwnedLvReader`, loads lambutter, returns
  wrapped backend. Old `LvmBtrfsBackend::open` API removed.
- **`main.rs` LVM dispatch loop** is now one match: success pushes a
  `Volume` constructed from the boxed `Box<dyn FsBackend>` returned by
  the dispatcher; failure logs the single error token and moves on.
  No backend-type branching at the caller.

### Properties achieved

| | v0.11.13 | v0.11.14 |
|---|---|---|
| PV/VG metadata parses per LV (worst case) | 2 (ext4 fail + btrfs success) | 1 |
| Backend construction attempts per LV | 1–2 | 1 |
| Caller branches on backend type | yes | no |
| Adding a 3rd backend touches | 2+ files cascading | 1 file (the dispatcher), +1 file (the new backend) |
| Failure-vs-wrong-FS distinguishable | no | yes (probe identifies, factory loads) |

### Adding a new LVM-FS backend (e.g. LvmXfsBackend, when native XFS lands)

1. Add `LvFsKind::Xfs` variant to `fs_backend_lvm_dispatch`.
2. Add XFS superblock check in `probe_lv_superblock` (`b"XFSB"` at offset 0,
   before ext4 check).
3. Add match arm in `open_lvm_lv_backend` that calls
   `LvmXfsBackend::from_lv_parts`.
4. Implement `LvmXfsBackend` in a new file with the same shape as
   `LvmBtrfsBackend` (adapter wrapping `OwnedLvReader`, FsBackend impl).

Four small edits. No touch to `main.rs`. No touch to existing
backends. No nested matches.

### Not yet verified

The refactor is functionally equivalent to v0.11.13 from main.rs's
perspective — same Volume objects produced, same VolumeIdentity, same
trust-log events. End-to-end runtime validation on a real LVM-on-btrfs
fixture (openSUSE Tumbleweed with LVM root + snapper-managed
subvolumes per the TODO §3.4 acceptance) is pending operator
scheduling and is not on the immediate archinstall test track.

## [0.11.13]

Residual fixes from the v0.11.12 VM 363 end-to-end audit, plus the
full Proxmox-host hookscript stack (lamboot-bls-amend, host-sampler,
zzzz-lamboot-proxmox dpkg trigger).

### Fixed (residuals from v0.11.12 audit)

- **`find_initrd_arch()` now honors `$OPT_ROOT`.** Under `--root` it
  was probing `/boot/initramfs-linux.img` on the live ISO (not at the
  chroot target) and returning empty, which left the per-version Arch
  BLS entry with an empty `initrd ` line. Exposed on VM 363 post-v0.11.12
  audit.
- **`generate_bls_entry()` omits the `initrd` line entirely when empty**
  instead of writing `initrd ` with a trailing space. Cleaner BLS output
  for UKI-only entries and a safer fallback when find_initrd returns
  nothing.
- **`efibootmgr` promoted to hard depend in PKGBUILD** (was optdepends).
  Under `--root` the deferred first-boot NVRAM-write hook needs it.
- **`lvm2` + `cryptsetup` added to PKGBUILD depends.** LVM/LUKS root
  layouts (the archinstall default LVM and LVM-on-LUKS scenarios) need
  these for initramfs activation + first-boot unlock.
- **archinstall plugin bootstraps host-side toolkit into chroot.**
  After `lamboot-install --root /mnt` completes, the plugin copies
  `/usr/bin/lamboot-install`, `/usr/lib/lamboot/esp-deploy.sh`, the
  `/usr/share/lamboot/{EFI,kernel-install,kernel-hooks}` tree, and the
  `/usr/lib/kernel/install.d/90-lamboot.install` plugin into the
  chroot. Without this, future kernel upgrades on the installed
  system would fire 90-lamboot.install and fail with "lamboot-install:
  command not found" until the operator manually `pacman -S lamboot`'d.

### Added (Proxmox-host hookscript stack)

- **`lamboot-bls-amend.service`** — fires after proxmox-boot-tool
  writes BLS entries; runs `lamboot-install --refresh --quiet` to apply
  LamBoot's sort-key + cmdline-sync overlays. PATH-B integration.
- **`lamboot-host-sampler.service` + `.timer`** — periodic runtime
  metrics sampler (boot state, crash count, ESP free space, Secure
  Boot state, kernel version). Writes one JSONL line every 5 minutes
  to `/var/log/lamboot/sampler.jsonl`, with a 10MB rotate cap. Fires
  via the timer after `lamboot-mark-success.service` lands so the
  crash-count delta semantics are clean.
- **`/usr/bin/lamboot-host-sampler`** — the sampler script itself.
  Best-effort: missing inputs (no efivars, no ESP mount) degrade to
  null JSON fields rather than failing the unit.
- **`/etc/kernel/postinst.d/zzzz-lamboot-proxmox`** — Proxmox-specific
  dpkg trigger. Sorts strictly after `zz-proxmox-boot` (four-z prefix);
  gates on `/etc/lamboot/proxmox-host.conf` so it's a no-op on
  non-Proxmox Debian/Ubuntu systems even when the package ships
  unconditionally. Auto-detects PATH B (proxmox-boot-uuids present)
  vs PATH A and dispatches to the right refresh path.
- **`/etc/lamboot/proxmox-host.conf`** marker file written by
  `lamboot-install --proxmox-host` (any PATH). Records the
  installation PATH, lamboot version, install timestamp. Gates the
  dpkg trigger.
- **New `proxmox_install_marker()` + `proxmox_install_observability()`
  + `proxmox_enable_units()`** functions in lamboot-install. Both
  PATH C (coexist) and PATH A (replace-grub) now install the
  observability stack. Cmdline-sync stays PATH-A-only.
- **`phase7b_proxmox_hooks` now fires for any `--proxmox-host`**, not
  just `--replace-grub`. PATH C operators get the observability stack
  too.
- **Chroot-aware enable**: under `--root` we create the
  `etc/systemd/system/timers.target.wants/` and
  `multi-user.target.wants/` symlinks manually since `systemctl` can't
  operate against a chroot's `/run/systemd`.

### Build pipeline

- **`build.sh`** now stages `systemd/*.{service,timer,path}` from
  source `systemd/` into `dist/systemd/`, stages
  `kernel-hooks/zzzz-lamboot-proxmox` into `dist/kernel-hooks/`, and
  copies `tools/lamboot-host-sampler` into `dist/lamboot-host-sampler`.
- **`package-release.sh`** includes `lamboot-host-sampler` in the
  release tarball staging.
- **PKGBUILD** has `install -Dm0644` lines for the three new units
  and `install -Dm0755` for the sampler and dpkg trigger, all gated
  on file presence so a tarball missing one piece still builds.

### Supported layouts (Proxmox-host)

| PATH | Boot chain | LamBoot reads | Cmdline source | Status |
|---|---|---|---|---|
| C (coexist) | GRUB → LamBoot (opt-in via menu) | /boot via firmware vfat | inherited from GRUB | ✓ ready |
| A (replace-grub) | shim → LamBoot (via dpkg-divert of grubx64.efi) | /boot via lvm2-view+ext4-view | /etc/kernel/cmdline (synced from /etc/default/grub) | ✓ ready |
| B (proxmox-boot-tool) | proxmox-boot-tool → LamBoot (via ESP mirror) | ESP-mirror only | per-BLS-entry options= | ⚠ amend service ready; CLI flag not yet — operator triggers via systemctl |

## [0.11.12]

`lamboot-install --root` now ensures the chroot's initramfs has the
hooks needed to activate the root layout on first boot. Fixes the
"hang at initrd.target / udev waiting for /dev/disk/by-uuid/<root>"
failure exposed on VM 363 after v0.11.11's cmdline-synthesis fix.

### Context

v0.11.11 produced a correct `options root=UUID=<lv-uuid> rw` in BLS
entries. The kernel + initramfs received the cmdline correctly. But on
VM 363 (archinstall + LVM root), the initramfs hung on
`Starting initrd.target` waiting ~90s for `/dev/disk/by-uuid/<lv-uuid>`
to appear, then dropped to emergency mode.

Root cause: **archinstall does NOT add `lvm2` to mkinitcpio HOOKS even
when the operator picks LVM in its disk config.** Without the `lvm2`
hook, the initramfs ships `dm-mod` (autodetect saw `/mnt` on a dm
device) but lacks the `lvm` binary and udev rules needed to scan PVs
and activate the VG. Root LV never appears at `/dev/disk/by-uuid/`.

The same gap applies to LUKS: archinstall doesn't add `sd-encrypt`
when LUKS is picked.

### Fixed

- **New `phase4c_chroot_initramfs_fixup`** runs between
  `phase4_install_files` and `phase5_generate_bls` under `--root`.
  Detects root layout via `findmnt` + `lsblk` walking the device chain:
    - `lvm` in the device chain → needs `lvm2` hook
    - `crypt` in the device chain → needs `sd-encrypt` hook
  Edits the chroot's `/etc/mkinitcpio.conf` to insert missing hooks at
  the canonical placement (after `block`, with `sd-encrypt` before
  `lvm2` so LUKS unlock precedes LVM activation), backs up the original
  to `mkinitcpio.conf.lamboot-<VERSION>.bak`, bind-mounts the chroot's
  kernel filesystems, runs `mkinitcpio -P` in the chroot, then unmounts.
- Idempotent: no-op if the required hooks are already present.
- Backup is only created when a change is actually made.
- Defensive: detects mkinitcpio specifically via
  `${OPT_ROOT}/usr/bin/mkinitcpio`; if absent (Fedora/openSUSE use
  dracut, Debian/Ubuntu use initramfs-tools), warns loudly with the
  specific hooks needed and leaves the chroot alone.
- Skipped under `--proxmox-host` (different integration model — GRUB
  owns the boot chain, LVM-activation is GRUB's problem).

### Supported root layouts (mkinitcpio chroots)

- ext4 plain (no fixup needed)
- ext4 on LVM (lvm2 hook added if missing)
- ext4 on LUKS (sd-encrypt added)
- ext4 on LVM-on-LUKS (both added, sd-encrypt before lvm2)
- btrfs / xfs / f2fs / bcachefs on LVM (lvm2 added if missing)

### LUKS cmdline robustness (also in v0.11.12)

- **LUKS detection moved off the brittle `/dev/mapper/*` heuristic**
  onto device-chain walk via `lsblk -snro NAME,TYPE` — same path the
  initramfs-fixup uses. Catches LVM-on-LUKS and plain LUKS layouts;
  doesn't false-positive on LVM-without-LUKS (which also lands at
  `/dev/mapper/*` paths).
- **`rd.luks.name=<luks-uuid>=<mapper-name>` now emitted alongside
  `rd.luks.uuid=`.** systemd-cryptsetup picks the default name
  `luks-<uuid>` when only `rd.luks.uuid=` is set; the chroot's LVM /
  fstab refers to the chroot-installer-named mapper (`cryptlvm` or
  similar from archinstall), so without `rd.luks.name=` the PV scan
  finds nothing under the expected name and root won't mount.
- **LUKS-header UUID resolved via `cryptsetup luksUUID <underlying>`**
  on the partition under the crypt mapper. Authoritative and immune
  to the same archinstall ordering issue that bit fstab — crypttab
  may not be populated when the plugin fires, but the live LUKS
  mapper is.
- **Crypttab fallback** for layouts where lsblk can't surface a
  partition node (LUKS-on-LV, LUKS-on-mdraid). Handles `UUID=`,
  `/dev/disk/by-uuid/`, and plain device path forms.
- **Loud warning + PARTIAL_FAILURE** if LUKS is detected but no
  UUID is resolvable from either path — operator knows to fix
  cmdline manually before reboot.

### Not yet covered

- dracut chroots (Fedora, openSUSE): warns; manual fixup needed
- initramfs-tools chroots (Debian/Ubuntu, on the rare distro installer
  paths that hit `--root`): warns; manual fixup needed
- mdadm RAID root layouts: not detected; manual fixup needed
- bcachefs-on-LVM with the legacy `bcachefs` hook name: detected as
  LVM but the bcachefs hook addition is out of scope
- TPM-bound LUKS / keyfile unlocks / clevis-tang: cmdline still emits
  `rd.luks.uuid=` and `rd.luks.name=` but no `rd.luks.options=` or
  unlock-mechanism hints; manual cmdline tuning needed for those

## [0.11.11]

`lamboot-install --root` cmdline synthesis now reads the live host's
mount table (via `findmnt`) instead of trusting the chroot's `/etc/fstab`
to be populated. Fixes the empty-`options`-line failure exposed on VM
363 after v0.11.10's BLS path fix got us past the v0.11.9 error.

### Context

v0.11.10 synthesized cmdline from `<root>/etc/fstab`. archinstall fires
the bootloader plugin BEFORE its `genfstab` step, so on VM 363 the
chroot's fstab contained only the skeleton header comments — no root
mount line. The synthesizer correctly bailed (no root in fstab) and the
BLS `options` line shipped empty. The kernel then reported `failed to
mount /sysroot` because no `root=` was passed.

### Fixed

- **New `synthesize_cmdline_from_mount()`** reads the live host's view
  via `findmnt -no SOURCE,UUID,FSTYPE,OPTIONS --target $OPT_ROOT`.
  archinstall has already mounted everything under `/mnt` before
  invoking the bootloader plugin, so the live mount table is the
  authoritative source. Uses `UUID=` form for `root=` (stable across
  reboots), reads filesystem-type-specific options (btrfs `subvol=`
  becomes `rootflags=subvol=`), still adds `rd.luks.uuid=` from
  `<root>/etc/crypttab` when the source is a dm-crypt mapper.
- **`synthesize_cmdline_from_fstab_fallback()`** preserved as a
  fallback for offline image-builder flows on minimal hosts without
  `findmnt`. Only invoked when `command -v findmnt` fails.

### Supported root layouts

Same as v0.11.10 (ext4 / ext4-on-LVM / ext4-on-LUKS / btrfs +
subvolume / xfs / f2fs / bcachefs) but now correctly works regardless
of distro-installer fstab-write timing.

## [0.11.10]

`lamboot-install --root` BLS-write phase now produces correct `linux`/
`initrd` paths AND populates `options` from the chroot's `/etc/fstab`.
Fixes the unbootable-after-archinstall path exposed on VM 362 after
v0.11.9's auto-fallback fix got us through firmware discovery — only
to hit a kernel panic from missing `root=`.

### Context

After v0.11.9, archinstall reached LamBoot's menu but the BLS entry
showed a red X ("file not found /boot/vmlinuz-linux"). Manual
navigation got the kernel to load — and then the kernel panicked
with `VFS: Unable to mount root fs on unknown-block(0,0)` because
the `options` line in the BLS entry was empty.

Three bugs in `phase5_generate_bls`:

1. `kernel_esp_path()`'s `[ "$ESP" = "/boot" ]` check only matched
   literal `/boot`. Under `--root /mnt`, ESP is `/mnt/boot` and the
   case fell through to "keep the full `/boot/` prefix" — so the
   BLS entry shipped `linux /boot/vmlinuz-linux` even though the
   kernel was at the ESP root.
2. `get_kernel_cmdline()` explicitly skipped /proc/cmdline under
   `--root` (correctly — host's cmdline is the live ISO's) and had
   no fallback. Arch doesn't ship `/etc/kernel/cmdline`, so cmdline
   ended up empty.
3. Inconsistency in initrd discovery between the per-version and
   generic BLS entries — separate v0.11.11 work item; the immediate
   v0.11.10 fix only addresses paths + cmdline.

### Fixed

- **`kernel_esp_path()`** now recognizes the `--root` + ESP-at-`<root>/boot`
  case explicitly and writes `linux`/`initrd` paths relative to the
  ESP root (not `/boot`-prefixed). Covers archinstall's default LVM
  layout (ESP mounted at `/boot`) and the equivalent calamares /
  image-builder flows.
- **New `synthesize_cmdline_from_fstab()`** function parses
  `<root>/etc/fstab` and builds:
    - `root=<spec> rw` from the `/` mount line (handles `UUID=`,
      `/dev/mapper/`, plain device paths)
    - `rootflags=subvol=<name>` for btrfs roots (reads the subvol
      from fstab options — covers archinstall's `subvol=@` /
      `subvol=/@` layouts)
    - `rd.luks.uuid=<luks-uuid>` when `<root>/etc/crypttab` is
      populated (LUKS, LUKS+LVM with sd-encrypt initramfs)
- **`get_kernel_cmdline()`** now invokes the synthesizer when running
  under `--root` and no `/etc/kernel/cmdline` exists in the chroot,
  replacing the v0.11.9 behavior of leaving cmdline empty.

### Supported root layouts (per archinstall scenarios)

`ext4` plain, `ext4` on LVM, `ext4` on LUKS, `ext4` on LVM-on-LUKS,
`btrfs` with subvolumes (subvol=/subvolid=), `xfs`, `f2fs`,
`bcachefs`. All synthesize a correct `root=` plus filesystem-specific
extras. ZFS root and mdraid roots are out of scope for v0.11.10 —
they need additional cmdline conventions (`root=ZFS=…`, `rd.md.uuid=…`)
and will be added if a test scenario actually exercises them.

### Verified

- VM 362 (archinstall + LVM root): manually applying the v0.11.10
  fix flow (strip `/boot/` prefix, populate cmdline from fstab) on
  the existing post-v0.11.9 install booted Arch through to userspace
  successfully — confirms the synthesizer produces a correct cmdline
  for the LVM scenario.
- Bash syntax check passes on `tools/lamboot-install`.

### Not yet verified

A fresh archinstall run with v0.11.10-built `lamboot-install` (no
manual BLS edits required) is the proper regression test and needs
a fresh VM. Pending operator scheduling.

## [0.11.9]

`lamboot-install --root` now auto-enables `--fallback`. Fixes the
"installed but firmware can't find it" dead-end exposed on VM 362
2026-05-28 after a clean archinstall run with v0.11.8.

### Context

v0.11.8 fixed the ESP-probe blocker so archinstall could reach the
install phase. But the resulting install had no first-boot path the
firmware could discover:

- Under `--root`, lamboot-install can't run `efibootmgr` (no NVRAM
  access from a chroot), so NVRAM Boot#### writes are deferred to
  first boot via a service that runs after Linux is up.
- That service can only run after Linux boots, Linux can only boot via
  LamBoot, and the firmware can only find LamBoot via NVRAM (doesn't
  exist yet) or the removable-media fallback path
  `\EFI\BOOT\BOOT{X64,AA64}.EFI` (not placed without `--fallback`).
- Result: every distro-installer integration (archinstall, calamares,
  image-builders) silently produced an install that rebooted to "No
  bootable option or device was found."

### Fixed

- **`tools/lamboot-install` auto-enables `--fallback` under `--root`**
  when neither `--fallback` nor `--no-fallback` nor `--no-efi-entry` is
  set explicitly by the operator. The fallback path is the only
  firmware discovery mechanism that does not require NVRAM access or a
  running Linux — under `--root` it is required for the install to be
  reachable on first boot.
- Auto-enable emits a warning line and a JSON event with `reason=
  deferred-NVRAM-no-other-discovery-path` so operators and protocol
  consumers see the decision.
- Auto-enable is skipped for `--remove` and for Proxmox-host PATH C
  (coexist-with-GRUB), where GRUB chainloads LamBoot and the firmware
  fallback isn't ours to touch.
- The existing `install_fallback()` self-loop guard still refuses to
  overwrite a non-LamBoot distro bootloader at the fallback path
  without `--force`, so dual-boot scenarios surface a loud failure with
  options rather than a silent broken install.

### Added

- **`--no-fallback` flag** — explicit opt-out from the auto-enable. For
  operators who arrange first-boot externally (custom NVRAM injection,
  chain-loading from another bootloader, etc.).
- **`OPT_NO_EFI_ENTRY_EXPLICIT` / `OPT_FALLBACK_EXPLICIT` /
  `OPT_FALLBACK_AUTO`** state in lamboot-install — distinguishes
  operator-set from script-set so the auto-enable rule respects
  operator intent.
- **Capabilities-protocol `--no-fallback` flag** declared in the
  `flags` array of `lamboot-install --capabilities` JSON output.

### v0.11.8 disposition

v0.11.8 was NOT published as a release — the dead-end was found
during VM 362 archinstall testing, before tag/push/AUR submission.
All v0.11.8 release-prep work (signed binaries, tarballs, PKGBUILD
`lib/esp-deploy.sh` install line, `sign-lamboot.sh` aarch64 coverage,
`package-release.sh` sha256 path-leak fix) carries forward to v0.11.9
unchanged.

## [0.11.8]

`lamboot-install --root` ESP detection now accepts an empty vfat
mountpoint as a valid ESP. Unblocks the archinstall flow on fresh
installs.

### Context

v0.11.7's archinstall plugin landed correctly and the canonical
`on_add_bootloader` hook fired exactly as designed on VM 362 — the
plugin invoked `lamboot-install --root /mnt --signed --no-prompt
--json` cleanly. But the install errored with:

    ESP not found under --root /mnt (looked for /boot/efi, /efi,
    /boot with EFI/ directory inside)

State of the chroot at that point: `/mnt/boot` is mounted vfat
(sda1, the ESP), but only contains `vmlinuz-linux` and
`initramfs-linux.img` from pacstrap. No `EFI/` subdirectory exists
yet — archinstall expects the bootloader-install step to create it.

The v0.10.1 `find_esp()` `--root` branch only accepted a path if it
already had an `EFI/` subdirectory. That heuristic was chosen
because the v0.10.1 author assumed host-side `mountpoint` / `is_vfat`
checks would be meaningless against an arbitrary chroot. That's
true for a fully-offline image-builder flow, but FALSE for
archinstall: archinstall runs lamboot-install in the live env with
the target ESP mounted at `<root>/boot`, so the host's mount table
DOES reflect it.

### Fixed

- **`tools/lamboot-install` `find_esp` `--root` branch now accepts
  two signals:**
  1. `EFI/` subdir already present — established ESP (existing
     install, offline service flow).
  2. Path is a vfat mountpoint on the host — fresh-install flow
     (archinstall, calamares with pre-mounted ESP, etc.).
  If signal (2) hits and `EFI/` doesn't exist, lamboot-install
  eagerly `mkdir -p`'s it so downstream deploy phases don't trip.
- **Error message updated** to mention both acceptance signals so
  users debugging `--root` failures get the full picture.

### Build pipeline

No archinstall plugin changes in this release — v0.11.7's hook
implementation was correct. Only `tools/lamboot-install` changed.

### Packaging

- **AUR PKGBUILD: install `lib/esp-deploy.sh` to `/usr/lib/lamboot/`.**
  Discovered while bootstrapping the VM 362 end-to-end archinstall
  test: the v0.10.x PKGBUILD was never updated after `lamboot-install`
  was refactored to source `lib/esp-deploy.sh`. Every `pacman -S
  lamboot` would have produced a `/usr/bin/lamboot-install` that
  errored on first invocation with "could not source esp-deploy.sh
  library." The release tarball ships `lib/esp-deploy.sh` correctly;
  only `package()` was missing the install line. No bug filed in AUR
  because v0.11.x has not been published there yet.

## [0.11.7]

archinstall plugin rewritten to use the canonical `on_add_bootloader`
hook. Drops the v0.11.6 `Installer.add_bootloader` monkeypatch. Based
on a comprehensive research session into the archinstall plugin API.

### Context

v0.11.6 monkey-patched `Installer.add_bootloader` to intercept the
Bootloader.Lamboot dispatch, because I had claimed (incorrectly) that
no `on_add_bootloader` hook existed in archinstall 4.x. That claim
was based on an incomplete grep of the source — the hook DOES exist
at line 1822 of `archinstall/lib/installer.py` and is exactly the
intended extension point for third-party bootloaders.

This release replaces the monkeypatch with the canonical hook.

### Fixed

- **Dropped `Installer.add_bootloader` monkeypatch.** The plugin now
  implements `Plugin.on_add_bootloader(self, installer)` and returns
  `True` to signal archinstall to skip its built-in match dispatch.
  This is the supported path; no internal-API patching.

### Architecture

archinstall's `on_add_bootloader(installer)` hook has one subtle gap:
it receives only the Installer instance — NOT the bootloader argument.
The bootloader is a local variable in `add_bootloader()` that is never
attached to the Installer. So a plugin can't directly tell which
bootloader the operator picked.

The plugin handles this via two-stage detection:

1. **Frame inspection** (`inspect.stack()`) — walks up the call stack
   looking for a frame where `bootloader` is a Bootloader enum member.
   Verified working on archinstall 4.3 + Python 3.14. Relies on
   archinstall using `bootloader` as the parameter name, which has
   been stable since the method was introduced.
2. **ArchConfigHandler reconstruction** — fallback that re-parses
   argv + JSON config. Only invoked if frame inspection failed AND
   sys.argv suggests we're inside an archinstall run.
3. **Return False if neither detection works.** This is the
   safe default — we let archinstall's built-in dispatch handle
   whatever the operator picked rather than incorrectly hijacking
   non-Lamboot installs.

### Added — contract tests

`packaging/installers/archinstall/tests/test_plugin_contract.py`:
18 tests covering:
- Loader contract (`__archinstall__version__`, `Plugin` class)
- Enum extension (Lamboot present, real Enum member, iterable)
- Frame-inspection detection (finds bootloader, ignores unrelated locals)
- `_is_lamboot` helper (enum members, strings, None)
- `on_add_bootloader` hook (returns False for non-Lamboot, raises
  on missing binary, returns True after successful install)
- `_run_lamboot_install` (missing binary raises RuntimeError)

Tests use a fake archinstall module so they run on any Python 3.11+
host without a real archinstall install.

### Verified on archinstall 4.3 + archiso 2026.05.01 (live VM 361)

Three scenarios pass:

```
=== scenario 1: operator picked Bootloader.Systemd ===
  result: False  (archinstall handles Systemd via built-in match)

=== scenario 2: operator picked Bootloader.Lamboot, lamboot-install absent ===
  [lamboot] on_add_bootloader: Lamboot selected; running lamboot-install ...
  [lamboot] lamboot-install not found on PATH ... Either: (a) install lamboot AUR pkg ...
  RuntimeError raised (CORRECT)

=== scenario 3: no bootloader in frame ===
  result: False  (safe default, no hijack)

Installer.add_bootloader: add_bootloader   ← no longer wrapped
_lamboot_patched attribute present: False  ← monkeypatch removed
```

### Strategy

This is Track 1 of a two-track plan documented in the analysis doc:

- **Track 1** (this release): plugin with frame inspection, ships now.
- **Track 2** (parallel): file an upstream PR to archinstall adding
  `bootloader` (and `uki_enabled` / `bootloader_removable`) to the
  `on_add_bootloader` signature with `inspect.signature` backward-compat
  at the call site. Once merged, the plugin drops frame inspection.

LamBoot will be the first third-party bootloader plugin published for
archinstall — no precedent exists in the wild. The two example plugins
in archinstall's docs (Torxed and phisch archinstall-aur) implement
`on_pacstrap`, not bootloader.

---

## [0.11.6]

archinstall plugin: critical timing fix. Bootloader.Lamboot selection
now actually installs lamboot. Diagnosed end-to-end on VM 361 / pve2
running archinstall 4.3 + archiso 2026.05.01.

### Fixed

The v0.11.5 plugin loaded correctly, registered the Lamboot Bootloader
enum member, archinstall accepted the operator's selection and even
logged "Adding bootloader lamboot to /dev/sda1". But the lamboot
install hook **never fired** — and the install completed with
"Installation completed without any errors", then rebooted to OVMF's
"No bootable option" screen. Silent failure.

Root cause traced through `scripts/guided.py`:

    installation.minimal_installation(...)       # on_install hook fires HERE
    installation.add_bootloader(config.bootloader_config.bootloader, ...)

The `on_install` hook runs INSIDE `minimal_installation`, BEFORE
`add_bootloader`. At hook-time, the `Installer` instance has no
`bootloader_config` attribute — the selection lives on a separate
`arch_config_handler.config` object that's only passed as an argument
to `add_bootloader()`. The previous plugin's `_selected_bootloader()`
probed `installer.bootloader_config`, `installer.bootloader`, etc. —
all empty — and silently returned. Then `add_bootloader(Bootloader
.Lamboot)` hit a `match` with no `case _:` arm (only
Systemd/Grub/Efistub/Limine/Refind) and fell through silently.

Net: two layers of silent no-op. archinstall reports success.
Operator boots to a black screen.

### The fix: monkeypatch Installer.add_bootloader

archinstall 4.x exposes no `on_add_bootloader` or `on_bootloader`
hook. The cleanest path is to wrap the dispatch site directly. The
plugin's `_patch_add_bootloader()` replaces `Installer.add_bootloader`
with a wrapper that:

1. If `bootloader.name == "Lamboot"`: invokes
   `lamboot-install --root /mnt --signed --no-prompt --json`,
   streams the JSON event log into archinstall's logger, raises
   RuntimeError on failure. **Loud, not silent.**
2. Otherwise: delegates to the original `add_bootloader` unchanged.

Patch is idempotent (checks for a `_lamboot_patched` marker on the
class) and applied at module import time so plugins loaded via
`--plugin PATH`, via entry_point auto-discovery, OR via any future
loader-flow change all get it.

### What this catches that v0.11.5 would silently miss

- **lamboot-install missing**: now raises RuntimeError with a clear
  message ("install the 'lamboot' AUR package in the live env, or add
  it to additional packages so pacstrap pulls it in"). Previously:
  silent no-op + black-screen reboot.
- **lamboot-install failure**: now propagates the rc/exit_name up
  to archinstall, which surfaces it as a visible install error.
- **on_install hook timing**: now irrelevant — the dispatch wrapper
  fires at the right point regardless of when archinstall calls hooks.

### Verified on VM 361 / pve2 / archinstall 4.3 / archiso 2026.05.01

```
$ python3 -c "import archinstall; from archinstall.lib.installer import Installer; print(Installer.add_bootloader.__name__)"
wrapped_add_bootloader

$ # dispatch Bootloader.Lamboot with lamboot-install absent
[lamboot] intercepted add_bootloader(Bootloader.Lamboot); running lamboot-install --root /tmp/...
[lamboot] lamboot-install not found on PATH in the live env nor at /tmp/mock-target/usr/bin/lamboot-install. Install the 'lamboot' AUR package in the live env before running archinstall, OR add 'lamboot' to additional packages so pacstrap pulls it into the target.
RuntimeError: lamboot-install not found on PATH ...
```

Loud failure where v0.11.5 was silent.

### Memory observation

The silent-no-op failure mode is documented in the memory database
as observation `22d9cfdc` (separate from this plugin fix — that one
is about pacman/Bitdefender), with the diagnostic flow recorded for
future debugging.

---

## [0.11.5]

archinstall plugin now auto-loads. No `--plugin` flag required.

### Added

- **`packaging/installers/archinstall/pyproject.toml`** — proper
  Python package metadata declaring the entry point
  `archinstall.plugin: lamboot = lamboot_plugin:Plugin`. Plugin is
  now pip-installable for development workflows.

- **`packaging/installers/archinstall/install-archinstall-plugin.sh`**
  — pip-free installer that writes the `.dist-info/` directory by
  hand. Used by the AUR PKGBUILD and by the live-ISO bootstrap
  workflow (archiso ships archinstall but not pip). Generates a
  spec-compliant METADATA + entry_points.txt + WHEEL + INSTALLER +
  RECORD layout next to the source file. Self-tests via
  `importlib.metadata.entry_points()` after install.

- **AUR PKGBUILD** now calls the install-archinstall-plugin.sh
  installer with `PREFIX=$pkgdir`, so after `pacman -S lamboot` the
  entry point is immediately visible to archinstall.

### Fixed

archinstall 4.x has ONE auto-discovery path: `importlib.metadata
.entry_points()` under group `archinstall.plugin`. The previous
release (v0.11.4) added a `Plugin` class and `__archinstall__version__`
so the plugin LOADED correctly, but archinstall still required
`--plugin PATH` because nothing registered the entry point. v0.11.5
closes that gap: with the package properly installed (via AUR or via
the pip-free installer), archinstall auto-discovers the plugin and
the Bootloader menu lists `lamboot` with no operator action.

### Verified on archinstall 4.3 + archiso 2026.05.01 (VM 361)

```
$ bash install-archinstall-plugin.sh
[ok] installed lamboot_plugin.py + dist-info to /usr/lib/python3.14/site-packages
archinstall.plugin entry points:
  - lamboot = lamboot_plugin:Plugin
[ok] lamboot entry point registered

$ python3 -c "import archinstall; from archinstall.lib.plugins import plugins; print(plugins)"
{'lamboot': <lamboot_plugin.Plugin object at 0x...>}

$ archinstall   # no --plugin flag — lamboot in the Bootloader menu
```

### Usage now

The README's three install paths (AUR, live-ISO pip-free, dev pip)
all produce identical results: `archinstall` (no flags) auto-loads
the plugin. The `--plugin PATH` workflow still works as a fallback.

---

## [0.11.4]

archinstall plugin loader-contract fix (continuation of v0.11.3).

### Fixed

After v0.11.3 corrected the Bootloader enum extension, the plugin
still didn't appear in the menu on archinstall 4.3 because three
additional loader-contract requirements were unmet:

1. **`__archinstall__version__: float`** module variable was missing.
   The loader checks this against the running archinstall's
   MAJOR.MINOR and errors if greater. Plugin now declares `4.0`.
2. **`Plugin` class (capital P)** was named `LambootPlugin`. The
   loader does `getattr(module, "Plugin")()`. Renamed.
3. **`/usr/share/archinstall/plugins/` is NOT auto-scanned** in 4.x —
   the loader needs either an installed Python entry point or an
   explicit `archinstall --plugin PATH` argument. README updated to
   document both code paths and the live-ISO `--plugin` invocation.

### Verified on archinstall 4.3 + Python 3.14

```
$ python3 -c '...load via importlib...; from archinstall...Bootloader; print(...)'
__archinstall__version__: 4.0
has Plugin class: True
Plugin() instantiates: <lamboot_plugin.Plugin object at 0x...>
Lamboot in enum: True
```

### Launch command for live-ISO usage

```sh
archinstall --plugin /usr/share/archinstall/plugins/lamboot_plugin.py
```

After this, the Bootloader menu lists `lamboot`.

---

## [0.11.3]

archinstall plugin rewrite. Verified end-to-end against archinstall 4.3
on Arch Linux 2026.05.01 ISO (VM 361 on pve2). The previous plugin
silently no-op'd on archinstall 3.x/4.x because the `Bootloader` enum
moved out of the top-level `archinstall` namespace; "lamboot" never
showed up as a menu choice on those versions.

### Fixed

- **`packaging/installers/archinstall/lamboot_plugin.py`** — rewritten
  to be version-portable across archinstall 2.x / 3.x / 4.x and Python
  3.11–3.14.

  Three shifts handled uniformly:

  1. **Bootloader enum location.** The import path moved from
     `archinstall.Bootloader` (2.x) to
     `archinstall.lib.models.bootloader.Bootloader` (3.x/4.x) and 4.x
     dropped the top-level alias. New `_resolve_bootloader_class()`
     probes each known path and uses whichever resolves to an `Enum`
     subclass.
  2. **Enum extension.** The old `_member_map_["X"] = "y"` shortcut
     produces a string sentinel, not a real Enum member, so the
     `[b for b in Bootloader]` iteration in the TUI menu skipped it.
     New `_extend_bootloader_enum()` mints a real `Bootloader`
     instance via `object.__new__()`, sets `_name_` and `_value_`,
     and splices into all four internal tables (`_member_map_`,
     `_value2member_map_`, `_member_names_`, plus the class
     attribute). Idempotent; safe against double-registration.
  3. **Selection-attribute attribute name.** 2.x: `installer
     .bootloader`. 3.x: `installer.config['bootloader']`. 4.x:
     `installer.bootloader_config.bootloader`. Plus an env override
     (`ARCHINSTALL_BOOTLOADER=lamboot`) for CI / unattended use. New
     `_selected_bootloader()` checks every documented path; new
     `_is_lamboot_selection()` does case-insensitive name+value match
     so we recognize the selection regardless of which path produced it.

- **Logging shim** — different archinstall versions exposed
  `installer.log(msg, level=...)`, `archinstall.log()`, or
  `archinstall.lib.output.{info,warn,error,debug}`. New `_log()` tries
  each in order, falls through to stderr.

- **Eager `on_register` at import** — some loader paths defer
  `on_register` until after the TUI builds its menu, which would hide
  Lamboot. Now extends the enum at module-import time too (idempotent,
  so safe if loader also calls explicitly).

### Verified on archinstall 4.3 + Python 3.14 (archiso 2026.05.01)

```
[lamboot] Bootloader.Lamboot registered
  (archinstall.archinstall.lib.models.bootloader.Bootloader)

Menu options (UEFI):
  - Systemd-boot
  - Grub
  - Efistub
  - Limine
  - Refind
  - lamboot
```

### Why dispatch falls through cleanly

archinstall's bootloader installation is dispatched by a `match` over
the Bootloader enum with no `case _:` arm. When the operator picks
our extended `Bootloader.Lamboot` member, the match falls through
silently — no bootloader is installed by archinstall. Our
`on_install` hook then runs `lamboot-install --root /mnt` and is the
sole bootloader-creator for the run. This is the intended interaction
and is now documented in `packaging/installers/archinstall/README.md`.

### Umbrella version 0.11.2 → 0.11.3 via `tools/sync-versions.sh --bump`.

---

## [0.11.2]

Single-feature release: optional capability-hints integration with
lamboot-capcheck via a new `--capcheck-json` flag.

### Added

- **`lamboot-install --capcheck-json PATH`** — consume a pre-generated
  lamboot-capcheck audit JSON (schema v1) and derive install hints.
  Contract: lamboot-capcheck is NOT a runtime dependency. The
  operator (or upstream installer integration like archinstall /
  Calamares) generates the JSON separately; lamboot-install only
  reads the file. No subprocess call, jq optional (a grep/awk
  fallback handles the field-set we consume).

  Effects:
  1. **Critical-quirk abort** — if any matched quirk has
     `severity=critical` (e.g. `lenovo-ideapad-300-emergency-shutdown`
     where deleting a UEFI variable bricks the firmware), install
     refuses to proceed. Override with `--force` (with a WARNING).
  2. **`--signed` implication** — if `secure-boot.state.claim
     .deployed_mode == true` (PK + KEK + db populated, SetupMode=0),
     `--signed` is implied automatically. Operator no longer needs
     to remember the flag on SB-enrolled hardware.
  3. **Operator visibility** — non-critical quirks are surfaced with
     id + severity + one-line summary at install start.
  4. **FAIL-status note** — `summary.checks_fail > 0` produces a
     WARNING but does not gate (capcheck FAILs are operator-decision
     territory, not install-blockers).

  Degrades silently when the JSON is missing, unreadable, or
  malformed — never an install-blocker on the bridge itself.

### Validated

- Smoke-tested four code paths on the dev workstation:
  1. Clean audit (debway Q35 OVMF VM, no critical quirks, no SB
     deployed) → consulted, no gate triggered.
  2. Synthetic critical-quirk JSON → ERROR + abort.
  3. Same JSON + `--force` → WARNING + proceed.
  4. Synthetic SB-deployed-mode JSON → `--signed` implied.

- `bash -n tools/lamboot-install` clean.

### Why this is in lamboot-dev, not lamboot-tools-dev

Per CLAUDE.md: lamboot-install is canonical in lamboot-dev (mirrored
to lamboot-tools-dev at release time). The "no lamboot-tools
dependencies, only options" constraint is honored — the integration
is a pure file-read of an externally-produced JSON document, not a
subprocess call or library import of any lamboot-tools binary.

---

## [0.11.1]

Single release rolling up everything after v0.10.0. The v0.11.0 tag
was never cut despite Cargo.toml being bumped in `fb8cd57`
("release: v0.11.0") — 25 commits of post-prep work plus today's
bug-fix sweep all land here.

### Added — bug-fix sweep (VM 118 EndeavourOS Method-B end-to-end test)

- **B12** (`db76516`): `install_kernel_hook_tool` now short-circuits
  when `/etc/kernel/postinst.d/` is absent. Previously set
  `PARTIAL_FAILURE=1` on non-Debian distros where the hook tool is
  irrelevant, causing `lamboot-install` to exit `EXIT_PARTIAL (2)`
  even with successful install + 5 verification passes, which then
  surfaced as a hard failure in `lamboot-migrate to-lamboot` via
  `|| die`. Surfaced by EndeavourOS VM 118 `to-lamboot --unsigned`
  test.

- **B13** (`0a4c7f7`): `lamboot-core::report::write_boot_report`
  emits valid JSON. New `json_escape()` helper covers JSON's required
  escapes (`\\`, `\"`, `\b`, `\f`, `\n`, `\r`, `\t`, `\uXXXX`).
  Applied to every interpolated string field. Previously UEFI-style
  paths like `\boot\vmlinuz-linux` were emitted verbatim, producing
  invalid JSON that `jq` and other strict parsers either error on or
  silently mangle. Validated post-fix on VM 118: `jq -e .` returns
  `JSON VALID`.

- **B14+B15** (`d123bfa`): `CLAUDE.md` documents the per-crate
  `.cargo/config.toml` arrangement post-`313a7d3` (April 22 refactor).
  Workspace-root `cargo build --release` produces host-target builds
  that link-fail on every UEFI crate — by design, so the host
  `lamboot-fs-tests` crate works. Document the cd-into-crate workflow
  + warn against the trap.

- **B16** (`c359427`): `build.sh` now stages `tools/lamboot-kernel-hook`
  to `dist/lamboot-kernel-hook` so ad-hoc deployments that extract
  `dist/` to `/usr/share/lamboot/` include the BLS-generator tool the
  Debian/Ubuntu postinst hooks invoke. `package-release.sh` already
  handled this for official tarballs; this closes the gap for the
  alternate deployment pattern.

### Added — features rolled forward from v0.11.0-prep

- Installer protocol v1 (`98d4180`) + AUR/archinstall/Calamares/openSUSE
  wrappers
- `--proxmox-host` (PATH C coexist) + `--replace-grub` (PATH A subset)
  + `--refresh` (`afeac08`)
- LVM root mount via lamlvm (`8a508ab`)
- `lamboot-grub-cfg` crate (in lamboot-tools-dev, coordinated)
- `bls_ignore_sort_key = true` default policy (`93e4f40`, `3260768`)
- Pop!_OS Bug 24/25 fix — Recovery panic + dir-style menu order
  (`a215f4e`)

### Added — pve2 hardening + firmware quirks subsystem (~18 commits)

- `firmware_quirks` detection module + trust events
  (`84cc0ef`, `90c0868`, `5b9b31d`, `eecf1f6`)
- `conout_fat_coupling` detection (real, confirmed in field)
  + opt-in diag override (`cf20528`, `17420e1`)
- `shim_verify_unusable` flag eradicated as unnecessary (`166105e`)
- `no_partition_info_protocol` flag eradicated as unused (`4776bc0`)
- Native PE loader reconstruction for pve2 (`230c344`, `0382319`)
- Input subsystem improvements: AbsolutePointer (UEFI 2.4+) support
  (`e8d3539`), USB ConnectController + multi-handle (`c46f9d5`,
  `53dcc37`), SimplePointer reset per UEFI §11.5.1 (`475ce2b`)
- Diagnostic gating via `[diagnostics]` policy (`9809d66`)
- Optional shim verify probe (`1c20f24`) + override (`98e18ce`)

### Changed

- `Cargo.toml` umbrella version: `0.11.0` → `0.11.1`
- `packaging/sbat.csv` lamboot row column 5: tracks umbrella
  (`0.11.1`); column 2 (generation) unchanged at 1 — no security
  advisory
- All 6 sync-versions sites in lockstep (per `VERSIONING-POLICY.md` §3.2)

## [0.12.0]

First minor-version milestone since v0.11.0. Cuts the installer-integration,
Proxmox-host, and native-LVM work that developed across the v0.11.x dev cycle
into a numbered release (the v0.11.x sections credited it incrementally; this
consolidates the detail), and surfaces the kernel trust posture directly in
the summary `boot.json`.

### Added — summary boot.json trust posture (issue #5)

- `lamboot-core/src/report.rs` — `boot.json` now carries `verified_via` and
  `loader` fields. Previously a consumer could read the entry *kind*
  (`linux_legacy` / `uki` / `chainload`) but not how the binary was verified
  or which loader ran — that lived only in the per-event
  `\loader\boot-trust.log`, so a native-PE boot and a firmware-LoadImage boot
  were indistinguishable from `boot.json` alone (the confusion behind public
  issue #1). The fields reuse the stable SDS-4 `verified_via` vocabulary
  (`shim_mok`, `degraded_trust_sb_direct`, `degraded_trust_sb_off`,
  `native_pe_loader`, `firmware_loadimage`); chainload targets report an empty
  `verified_via` since LamBoot makes no kernel-trust decision on them. Derived
  by pure mappers (`report_verified_via`, `report_loader`) in
  `trust_log_pure.rs` with host-test coverage. No new ESP writes — the fields
  fold into the existing single pre-handoff `boot.json` write, so the FAT
  write count (and the `conout_fat_coupling` handling) is unchanged.

### Documentation

- `docs/specs/SPEC-LAMBOOT-INSTALL.md` §1.2 / §9.3 — documents the
  signed-binary selection rule (under `--signed`, the ESP's
  `lambootx64.efi` is the signed artifact) and the Secure Boot
  shim-chain copy set (`grubx64.efi`, plus the shim-embedded loader
  name such as SUSE `grub.efi`). The prior spec listed only the
  canonical mapping and implied that filename held the unsigned binary.
  Behavior unchanged; spec brought in line with what the installer
  actually deploys (issue #8).

### Added — installer integration (Tier 1 of the 2026-05-26 roadmap)

- `docs/specs/SPEC-LAMBOOT-INSTALLER-PROTOCOL-V1.md` — authoritative spec
  for the JSON event stream + flag surface consumed by downstream
  installers.
- `tools/lamboot-install` — five protocol additions: `--protocol-version`,
  `--capabilities`, `--json`, `--no-prompt`, `--root PATH`. Exit-code
  vocabulary extended from 4 to 8 (adds
  unsafe/abort/not_applicable/prerequisite_missing). First-boot
  deferral mechanism for NVRAM/MOK/ShimRetainProtocol operations
  under `--root`.
- `packaging/aur/lamboot/` — AUR PKGBUILD + .SRCINFO; pacman
  post-transaction hook mirrors GRUB's `90-grub.hook` pattern.
- `packaging/installers/archinstall/lamboot_plugin.py` — Python plugin
  for the Arch Linux guided installer.
- `packaging/installers/calamares/lamboot/` — Python jobmodule with
  8-phase progress mapping.
- `packaging/installers/opensuse/usr/lib/bootloader/lamboot/` —
  3-script contract (install/config/remove) matching grub2-efi's
  pattern.

### Added — Proxmox VE host install (per `docs/proxmox-host-install/research/INDEX.md`)

- `lamlvm` (separate private dev repo, `lamco-admin/lamlvm-dev`;
  future-public name `lamco-admin/lamlvm` reserved per the lambutter
  convention) — vendored + modernized fork of `main--/rust-lvm2`. `no_std`, `forbid(unsafe_code)`,
  `embedded-io` 0.6 in place of unmaintained `acid_io`, edition 2024,
  arithmetic hardened with `checked_add_signed` / `checked_mul`. Adds
  `OwnedLvReader` so callers don't have to carry the `'a` borrow from
  the parsed `Lvm2` — lets `FsBackend` instances embed an LV reader
  directly. Coverage: linear LV on single PV (Proxmox `pve-root` default
  layout). cargo-fuzz harness suite: 5 targets, 5.36 billion executions
  in pre-publish testing, zero crashes.
- `lamboot-core/src/fs_backend_lvm.rs` — mounts ext4 from an LV via
  `lamlvm::OwnedLvReader` → `ext4_view::Ext4Read`. New
  `partitions::probe_lvm_pv` checks for the `LABELONE` signature at
  byte 512 of every partition; discovery loop in `main.rs` tries a
  curated list of LV names (`root`, `pve-root`, `system`, `ubuntu-lv`,
  `rootlv`) per detected PV and adds the first successful mount to
  `extra_volumes` so BLS discovery sees it.
- `lamboot-grub-cfg` crate (in `lamboot-tools-dev/crates/grub-cfg/`)
  + `lamboot-grub-inspect` CLI — parses `grub.cfg`, `/etc/default/grub`,
  and `grubenv`; resolver ties them together to answer "which entry
  would GRUB boot right now?" applying the full selection rule
  (`next_entry` > `GRUB_DEFAULT=saved` > numeric index > literal id >
  fallback); compare-cmdline reports drift between the resolved
  menuentry and what `/etc/default/grub` would compose now. 36 unit
  tests against real pve2 fixtures.
- `tools/lamboot-install --proxmox-host` (PATH C coexist) — installs
  LamBoot at `/EFI/LamBoot/` alongside Proxmox's shim+grub, adds a
  chainload menuentry to `/etc/grub.d/40_custom` wrapped in idempotent
  markers, runs `update-grub`. GRUB stays the default. No BLS
  generation, no kernel hooks. Pick "LamBoot (chainload)" at the GRUB
  menu or use `grub-reboot` for one-shot test boots. Rollback is
  `lamboot-install --remove` which strips the menuentry and
  regenerates `grub.cfg`.
- `tools/lamboot-install --replace-grub` (PATH A subset) — escalates
  the coexist install with BLS-entry generation under
  `/boot/loader/entries/lamboot-VERSION.conf` for every existing
  Proxmox kernel, writes `/etc/kernel/cmdline` from
  `lamboot-grub-inspect parse-default-grub`, and installs
  `/etc/kernel/install.d/00-lamboot-cmdline-sync` which re-runs
  `lamboot-install --proxmox-host --refresh` after every kernel
  install/remove so BLS entries stay current with `/boot/vmlinuz-*` and
  `/etc/default/grub`. Final drift check via
  `lamboot-grub-inspect compare-cmdline`; surfaces drift as warn +
  PARTIAL exit, never aborts. The dpkg-divert of `grubx64.efi` itself
  is reserved for v0.12+ — `--replace-grub` v0.11.0 keeps the 40_custom
  chainload as the entry point.
- `tools/lamboot-install --refresh` — postinst-hook callback mode;
  skips detect/drivers/discover/install/uefi_entry/systemd/verify,
  runs only the BLS regen + drift check. Not for direct operator use.
- Soft autodetect: running plain `lamboot-install` on a host with
  `/etc/pve/` or `/usr/sbin/proxmox-boot-tool` present prints a loud
  warning that `--proxmox-host` is probably what was meant. Doesn't
  refuse.

### Changed — defaults

- **`dist/EFI/LamBoot/policy.toml` template now sets
  `[bls] bls_ignore_sort_key = true` by default.** Fresh installs of
  v0.11.0 onward collapse the BLS sort-key cohort split that bit two
  real distros (Debian forky/sid VM 100, Pop!_OS 24.04 VM 124) into
  default-boots-wrong-kernel failures. **Existing installs are NOT
  changed** — `lamboot-install`'s `install_policy()` preserves an
  existing `policy.toml` and drops the new template at
  `policy.toml.new` for operator reference. Operators on multi-OS hosts
  who want strict BLS §3.1 spec behavior can set
  `bls_ignore_sort_key = false`; the cohort-split diagnostic (event
  + menu footer) fires regardless of the lever. Rationale logged in
  the template's inline comment block. Default will be revisited if
  Debian BTS #1137582 lands a root-cause fix in upstream systemd.

### Added — release tooling

- `tools/sync-versions.sh` — single command that stamps the umbrella
  version (`Cargo.toml`) and protocol version across all 7 stamp
  sites. Modes: `--check`, `--sync`, `--bump X.Y.Z`,
  `--bump-protocol N`, plus `--dry-run`. Replaces hand-editing 6
  files at every release (the failure mode that produced 5 releases
  of `LAMBOOT_VERSION` drift in `tools/lamboot-install`).

### Fixed

- `tools/lamboot-install` `LAMBOOT_VERSION` corrected from `0.8.4` to
  `0.10.0` (catches up on v0.9.0/v0.9.1/v0.9.2/v0.9.3/v0.10.0 hand-edit
  misses). Future drift prevented by `tools/sync-versions.sh --check`.
- **`examples/policy.toml` Option D template** — `bls_ignore_sort_key`
  was incorrectly placed inside a `[bls]` section in the initial Option D
  template commit (`3260768`). v0.10.0's parser (`policy.rs:300-337`)
  is section-aware: it builds qualified keys as `section.key`, so a key
  inside `[bls]` resolves to `bls.bls_ignore_sort_key` — which has no
  match arm and gets silently dropped. Discovered empirically when VM 100's
  manual `[bls] bls_ignore_sort_key = true` was ignored by v0.10.0 (split
  detected, lever ignored, menu order still wrong). Lever now placed at
  ROOT in `examples/policy.toml`, matching where v0.10.0's parser expects
  it. Template inline comment explicitly warns against the `[bls]`
  placement that bit us.
- **Bug 24 — Pop!_OS Recovery dir kernel panic** (`unable to mount root
  fs on unknown-block(0,0)`). `discovery::discover_systemd_boot_dir_style`
  emitted a `BootEntry` for `/EFI/Recovery-<short>/` whenever
  `vmlinuz.efi` was present, without checking for the sibling `cmdline`
  and `initrd.img` that a normal kernelstub install carries. Recovery
  UKIs have neither (they use casper-style boot with `initrd.gz` and a
  cmdline supplied at menu-select time), so the emitted entry had empty
  cmdline and no initrd → kernel panic on first root mount. Empirically
  observed on VM 124 (Pop!_OS 24.04 LTS). Fix: guard in
  `discover_systemd_boot_dir_style` skips the directory when
  `cmdline.is_empty() && initrd_paths.is_empty()`. Heuristic is general,
  not Pop!_OS-name-specific. Detailed in TROUBLESHOOTING.md §1.5.
- **Bug 25 — non-deterministic menu order for dir-style entries.**
  `discover_systemd_boot_dir_style` returned entries in
  `Volume::read_dir` iteration order (FAT directory traversal order =
  on-disk creation order), not alphabetical. On VM 124 the Recovery dir
  was created before the Pop_OS-uuid dir, so Recovery appeared first.
  Fix: `entries.sort_by(|a, b| a.id.cmp(&b.id))` before return. `id` is
  `sdboot-<dirname>` so alphabetical-by-id = alphabetical-by-dirname,
  and the `-previous` suffix sorts immediately after the parent.
  Detailed in TROUBLESHOOTING.md §1.6.

### Operational (TROUBLESHOOTING.md)

- **Bug 23 — EFI Fallback synthetic entry self-loops on Config 2
  installs at `\EFI\BOOT\BOOTX64.EFI`** — already fixed in v0.9.0
  (commit `712c5b9`) via hash-based self-loop guard in
  `discovery::discover_other_loaders`. Documented retroactively because
  VM 124 (still on v0.8.4 at the time of the validation cycle)
  exhibited it as the "first entry isn't a good image" symptom.
- **Crash-counter wedged at N>0 on direct-binary-replace installs.**
  Operators who deploy LamBoot by manually copying the binary to the
  ESP (without running `lamboot-install --update`) skip the
  `install_mark_success_service()` step (`tools/lamboot-install:2313`).
  Result: `lamboot-mark-success.service` is never installed, the
  `LamBootCrashCount` efivar increments on every boot, and the counter
  eventually approaches the `[security] crash_threshold` (default 3),
  potentially triggering crash-loop fallback for healthy boots.
  Remediation: copy `dist/systemd/lamboot-mark-success.service` to
  `/usr/lib/systemd/system/`, `systemctl daemon-reload`,
  `systemctl enable lamboot-mark-success.service`, optionally
  `systemctl start lamboot-mark-success.service` to reset the counter
  immediately without waiting for the next boot.

### Governance

- `~/lamco-admin/projects/lamboot/VERSIONING-POLICY.md` codifies
  umbrella-only versioning. Only `lamboot`, `lamboot-tools`, and
  `lambutter` get version numbers; component tools carry the parent
  umbrella. Two independent contracts
  (`LAMBOOT_INSTALLER_PROTOCOL_VERSION` + `LAMBOOT_TOOLKIT_SCHEMA_VERSION`)
  bump on their own cadence.
- `~/lamco-admin/pipelines/lamboot/RELEASE-PROCEDURE.md` Phase 0
  step 2 now invokes `tools/sync-versions.sh --bump X.Y.Z` instead
  of hand-editing `Cargo.toml`.

### Research (overnight 2026-05-25)

Ten internal research deliverables (5,709 lines): XFS + ZFS rust-ecosystem
surveys + xfs-view design, Multiboot evaluation (declined),
Proxmox 5-phase deep-dive, Phase D WASMi/health/metrics assessment,
installer-integration roadmap, overnight-research synthesis.
Synthesized in the internal 2026-05-26 roadmap.

## [0.10.0]

Minor-version bump introducing operator-controllable levers for the
BLS sort-key cohort-split failure mode (Bug 22). Driven by the VM 100
(debway, Debian forky/sid) rolling-release upgrade that empirically
demonstrated this class of bug ranks above Bug 21 for default-selection
correctness — and which is the second time in five weeks LamBoot's
autoselect has picked the wrong entry due to upstream distro
inconsistency (predecessor: VM 124 Pop!_OS, commit `a2529e3`,
April 2026).

### Added

- **`policy.toml: default_pattern: Option<String>`** — glob pattern
  over `entry.id`. When set, the default-entry selector picks the
  highest-`version_compare` match across ALL boot-eligible entries,
  regardless of which BLS sort-key cohort they live in. Mirrors
  sd-boot's documented escape hatch (keszybz, systemd#23669) for the
  same class of bug. Resolution precedence at selection time:
  `default_pattern` (glob) > `default_entry` (exact id) > caller fallback
  (first boot-eligible entry by sort order). Implementation in
  `discovery::select_default_entry`; wired into both `gui.rs` and
  `console.rs` default-selection paths.

- **`policy.toml: bls_ignore_sort_key: bool` (default `false`)** —
  when true, `bls_sort_compare` skips the sort-key presence test
  and sort-key value compare, collapsing both BLS cohorts into a
  single machine-id + version + filename ordering. Mirrors Fedora
  bootupd's design choice. Fixes BOTH the menu-display order AND the
  default-entry cursor for the cohort-split failure mode.

- **Cohort-split runtime diagnostic.** `discovery::detect_cohort_split`
  scans the parsed BLS entries during discovery; when entries sharing
  a `machine-id` mix sort-key presence, emits a new trust-log event
  `cohort_split_detected` (with structured `with_sort_key`,
  `without_sort_key`, `highest_in_without` fields) AND surfaces a
  one-line notice on the menu footer (GUI: orange warning color
  beneath the keyboard hints; console: `[!] cohort split: …` line
  above the entry list). The notice includes the policy.toml escape
  hatch hint inline so a stranded operator can resolve the anomaly
  without consulting the trust log or docs.

### Changed

- **`bls_sort_compare` signature**: now takes `(a, b, ignore_sort_key:
  bool)`. Callers in `bls.rs::scan_volume_for_bls` thread
  `policy.bls_ignore_sort_key`; host tests in `lamboot-fs-tests` pass
  `false` to preserve spec-faithful behavior assertions.

- **`discovery::discover_all_entries` return type**: now returns
  `DiscoveryResult { entries, cohort_split }` instead of bare
  `Vec<BootEntry>`. Single caller in `main.rs` updated; allows the
  cohort-split notice to flow through to the menu UI without
  globals or thread-locals.

### Design decisions

Three escape hatches were considered alongside an anomaly-detection
auto-promote heuristic (Option H in the design doc). Option H was
DEFERRED — naive operators get a self-solving diagnostic (Option I)
that tells them which flag to flip, and the two flags (E + F) are
both documented escape hatches in the upstream peer ecosystem
(sd-boot and Fedora bootupd respectively). Forward-compatible with
adding Option H as a third lever (`bls_cohort_repair`) later if
empirical data shows naive operators aren't reading the diagnostic.

### Companion workstreams (operator-side, independent of this release)

- **Debian bug to be filed** against `systemd` or `linux-base`
  describing the mid-upgrade sort-key emission flip surfaced on
  VM 100. Draft preserved in this commit's referenced design doc;
  filing is operator action (lamberson@yahoo.com identity).
- **uapi-group BLS spec addendum** proposal — non-normative
  recommendation that bootloaders MAY treat machine-id-shared
  mixed-cohort entries as inheriting from the homogeneous neighbor
  cohort. First-mover opportunity; cost is low.

## [0.9.3]

Pre-`pve2`-migration release. Surfaces from auditing the Proxmox boot
chain before doing the bare-metal host migration: the v0.9.2
`detect_shim_default_loader()` heuristic would have silently picked
the wrong file on a Proxmox install, with a destructive side effect.

### Fixed

- **Bug 18.5 — shim default-loader heuristic could overwrite the
  Proxmox fallback bootloader.** The v0.9.2 implementation used
  negative filtering — `strings -e l <shim> | grep .efi | grep -v
  "fallback|MokManager|revocation|certificate"` — to find shim's
  embedded default loader. On Proxmox shim the strings include
  `\fbx64.efi` (fallback, single-backslash), `\mmx64.efi` (MokManager,
  single-backslash) and `\\grubx64.efi` (default loader,
  double-backslash). Proxmox's fallback file is named `fbx64.efi`
  rather than `fallback.efi`, so it slipped past the exclusion list,
  and `head -1` picked it as the "default loader." The chain
  setup would then have copied LamBoot to
  `/EFI/LamBoot/fbx64.efi`, **overwriting Proxmox's legitimate
  fallback bootloader binary**. v0.9.2 worked on SUSE only because
  SUSE's fallback file is literally named `fallback.efi`. The fix
  switches to positive identification: grep for `^\\\\[name]\.efi$`
  (the literal `\\` prefix that marks the actual default loader),
  not "any .efi name minus exclusions."

### Notes

- No Rust code changed in this release; the version bump propagates
  through `crate::version` so the `LoaderInfo` UEFI variable, the
  trust-log `boot_start` note, and the GUI footer all consistently
  read `0.9.3`. Single source of truth in action — exactly the
  scenario `version.rs` was built for.

## [0.9.2]

Surface-bugs cleanup release driven by the Config 3 dress-rehearsal pass
on VM 132 (`osusetum-uefi`, openSUSE Tumbleweed snapshot 20260425 with
`grub2-bls` install). All four fixes are needed to put a fresh modern
Tumbleweed install onto LamBoot + shim+MOK + native PE loader and get a
`verified_via=shim_mok` trust-log line out the other side.

### Fixed

- **Bug 16 — `find_distro_shim()` missed the openSUSE `grub2-bls`
  layout.** The search array enumerated `/EFI/ubuntu`, `/EFI/debian`,
  `/EFI/fedora`, `/EFI/opensuse`, `/EFI/proxmox`, etc., but did not
  include `/EFI/systemd/shim.efi` — the canonical location on a
  Tumbleweed snapshot ≥ ~2025-11-13 that chose `grub2-bls` at install
  time (which actually ships systemd-boot; `update-bootloader`'s
  `grub2-bls/` is symlinks into `../systemd-boot/`). On such a system,
  `find_distro_shim()` would silently fall back to
  `/EFI/BOOT/BOOTX64.EFI` (the firmware-fallback path that happens to
  be a copy of the same shim, semantically wrong) before this fix.
  Added `${ESP}/EFI/systemd/shim.efi` to the search array ahead of the
  `BOOTX64.EFI` fallback.

- **Bug 18 — `setup_secure_boot_chain()` hardcoded `grubx64.efi` as
  the shim chainload target.** Upstream shim's default loader is
  `grubx64.efi`, but **SUSE's downstream shim is built with
  `DEFAULT_LOADER L"\\\\grub.efi"`** — so on Tumbleweed,
  `\EFI\LamBoot\grubx64.efi` was never read at boot and the chain
  hung at "default loader not found." Added a `detect_shim_default_loader()`
  helper that extracts the loader name from the shim binary's
  UTF-16LE strings; when the detected name differs from `grubx64.efi`,
  LamBoot is placed at **both** locations so the chain works
  regardless of which shim the firmware ends up calling.

- **Bug 20 — `LoaderInfo` UEFI variable stuck at `"LamBoot 0.2.0"`
  through six releases.** `lamboot-core/src/health.rs` had a literal
  UTF-16LE byte sequence for `"LamBoot 0.2.0"` and a literal
  `LAMBOOT_VERSION: u32 = 2 << 8` — both untouched since v0.2. Every
  shipping LamBoot from v0.3 onward told systemd-boot's `bootctl
  status` it was v0.2.0 even though the trust log (built from
  `env!("CARGO_PKG_VERSION")`) correctly said the real version.
  Introduced `lamboot-core/src/version.rs` as the single source of
  truth for product name + version + packed-u32 + UTF-16LE
  representations; `health.rs` now imports from there, and a
  prominent comment block in `version.rs` documents the rule
  ("never reintroduce a literal version anywhere else") plus the
  Bug 20 history so the next contributor sees why.

### Compatibility

- `lamboot-install` v0.8.4 in this release tarball requires the
  paired `lamboot-tools` ≥ 0.3.0 (for the matching `lamboot-migrate`
  Bug 6/12 fixes and the `lamboot-signing-keys` Bug 15 SBAT VMA
  fix). Mismatched older toolkits will work for ext4 / single-distro
  installs but will not produce a `shim_mok` chain on modern
  Tumbleweed.

## [0.9.1]

Bugfix-cadence release. Closes the v0.9.1 sprint punch list: a
packaging regression in v0.9.0, a documentation gap that bit fleet
operators rolling their own `OVMF_VARS`, a silent-trust-log
correctness bug on the firmware-LoadImage path, and a Pop!_OS
autodiscovery audit that closed with "no gap."

### Fixed — packaging: tarball now ships `lib/esp-deploy.sh`

The v0.9.0 release tarball was missing `lib/esp-deploy.sh` —
`lamboot-install` sources this canonical ESP-layout library at one
of three paths (`<self-dir>/../lib/esp-deploy.sh`,
`/usr/lib/lamboot/esp-deploy.sh`, `<self-dir>/lib/esp-deploy.sh`).
The third path is the tarball case; the file was simply never
copied into the staging tree, so a fresh extract failed with
`ERROR: lamboot-install could not source esp-deploy.sh library.`
and required a manual workaround.

- `package-release.sh` now copies `lib/esp-deploy.sh` into
  `lamboot-<VERSION>/lib/esp-deploy.sh` in the staging tree, where
  `lamboot-install`'s third lib-search candidate finds it.
- `lib/esp-deploy.sh` is now in the `REQUIRED[]` sanity-check
  array, so a future regression of the same shape (file missing
  from `lib/`) will fail the package step before producing a
  tarball.

### Fixed — silent trust log on `[loader].native_pe = "never"`

Operators who set `[loader].native_pe = "never"` — a documented
v0.8.3-rollback knob — got a trust log that went silent after
`boot_attempt`, with no record of the firmware-LoadImage path
being taken. Two issues compounded:

1. The `image_loaded_firmware` event was emitted but **never
   flushed** to `\loader\boot-trust.log` before transferring
   control. The kernel ExitBootServices reclaimed the in-memory
   log, so audit consumers saw nothing.
2. There was no event marking the *routing decision* — only the
   post-load success.

`firmware_load_and_start` now:
- Records a new `legacy_loadimage_used` event before the
  LoadImage call, with `note=size=<N> policy=<auto|always|never>`
  capturing why the firmware path was taken.
- Calls `trust_log.flush(esp)` before `start_image`, mirroring the
  native path's existing flush at boot.rs ~617. Events on this path
  now persist to disk.

The new event uses `verified_via=firmware_loadimage` (existing
stable token); adding the `legacy_loadimage_used` event name is
semver-additive per the trust-log schema vocabulary in
`SECURITY-MODEL.md`.

### Documented — `--add-mok` vs `--add-db` distinction in `OVMF-VARS-PROXMOX.md`

Operators rolling their own `OVMF_VARS` with `virt-fw-vars` (rather
than the pre-built `OVMF_VARS_lamboot.fd`) sometimes used
`--add-mok` alone, expecting that to make LamBoot directly
bootable under SB. It does not — MOK is consulted by shim, not by
firmware. With the cert in MOK only, firmware rejects LamBoot's
binary and falls back to whatever other Boot#### entry exists
(typically the distro's shim), with `BootCurrent` no longer
matching LamBoot. Direct boot under SB requires the cert in
firmware `db` (`virt-fw-vars --add-db`).

`docs/OVMF-VARS-PROXMOX.md` now has a §1.1 "Common pitfall" callout
making the distinction explicit, plus a back-reference from §7
where `virt-fw-vars` is actually used.

### Audit — Pop!_OS auto-discovery: no gap in `autodiscovery.rs`

`POPOS-AUTO-DISCOVERY-FINDINGS-2026-04-25` §1 flagged
`autodiscovery.rs` as a separate audit target alongside the new
`discover_systemd_boot_dir_style` scanner shipped in v0.9.0.
End-to-end audit closed with no code change: `find_initrd()` is
called only from `bls.rs:171` (BLS Type 1 entries). The
`<distro>-<root-uuid>/vmlinuz.efi` layout that Pop!_OS uses is
handled entirely by `discover_systemd_boot_dir_style` in
`discovery.rs`, which constructs `initrd_paths` from the sibling
`initrd.img` file directly and never traverses `autodiscovery.rs`.

## [0.9.0]

The v0.9.x SDS ladder: native ext4 read backend, native PE loader,
native trust chain, BLS multi-FS discovery, legacy UEFI FS driver
deprecation. Plus first-class Pop!_OS / systemd-boot-discoverable-EFI
auto-discovery, the lamboot-tools v0.2.0 toolkit gap closures
(esp-deploy.sh canonical lib + offline `lamboot-esp deploy` +
fallback-foreign detection + BootOrder reorder), and a comprehensive
five-document strategic coverage analysis.

### Added — Pop!_OS / systemd-boot-discoverable-EFI auto-discovery

LamBoot now picks up Pop!_OS kernels at the kernelstub-managed
`\EFI\Pop_OS-<root-uuid>\vmlinuz.efi` layout and the Pop!_OS Recovery
counterpart at `\EFI\Recovery-<short-uuid>\vmlinuz.efi`. Same scanner
covers Garuda variants and any custom systemd-boot installation
that uses the `<distro>-<uuid>` directory pattern.

- New `lamboot-core/src/discovery.rs::discover_systemd_boot_dir_style`
  function. Iterates `/EFI/<dirname>` directories on the ESP, recognizes
  `<name>-<uuid>` patterns via `discovery_pure::looks_like_distro_uuid_dir`,
  reads sibling `vmlinuz.efi` + `cmdline` + `initrd.img` files, emits
  `EntryKind::LinuxLegacy` entries.
- New `lamboot-core/src/discovery_pure.rs` — host-testable pure helpers
  (recognizer + display renderer + path-style converter). Same
  `#[path]`-include pattern used by `bls_parse`, `pe_loader_pure`,
  `trust_log_pure`. 24 host tests covering positive/negative dirname
  recognition + display rendering + path conversion.
- Pop!_OS-specific display: `Pop_OS-<uuid>` renders as `Pop!_OS`
  (System76 escapes the `!` as `_` in directory names).
- Last-known-good kernel surfaced as a sibling entry: when
  `vmlinuz-previous.efi` is present, a `(previous kernel)` BootEntry is
  emitted with paired `initrd.img-previous`.

**Live evidence (VM 124 Pop!_OS COSMIC):** trust log shows
`entries_discovered total=7` with Pop!_OS Recovery selected via
`entry_selected id=sdboot-Recovery-79EB-58C6 kind=LinuxLegacy`,
`image_verified verified_via=degraded_trust_sb_direct sha256=c74f5c68…`,
`image_loaded_native loader=native_pe_loader` — full v0.9.x boot path
exercised on Pop!_OS for the first time.

### Added — EFI Fallback chainload self-loop guard

`discover_other_loaders` now takes the running LamBoot image's
SHA-256 and skips the synthetic "EFI Fallback" entry when
`\EFI\BOOT\BOOTX64.EFI` is itself a copy of LamBoot. Prevents
chainload self-loop when operators use the toolkit's
belt-and-suspenders pattern (`repair.fallback.replace_with_lamboot`)
to write LamBoot's signed binary to the firmware-fallback path.

`run_bootloader` computes the self-hash via `LoadedImageProtocol`
(image_base + image_size) and threads it through
`discovery::discover_all_entries(self_image_sha256: Option<[u8; 32]>)`.

### v0.9.x ladder context

The five SDSes that constitute the v0.9.x foundation are now landed
(see prior ## [Unreleased] sections, retained below for the per-SDS
detail). This release is the first to ship them all together as a
coherent stack.

— end of v0.9.0 entry —

## [Pre-v0.9.0 SDS ladder — historical detail retained]

With v0.8.4 shipped, the toolkit pivot's pause on SDS-2..SDS-6 is
cleared. Per-task polish starts here before diving into SDS-2.

### Added — SDS-2 (native ext4 read backend) — LANDED

Four-PR sequence (see `docs/specs/SPEC-EXT4-INTEGRATION.md` v1.1):

- **PR-1** `feat(fs): SDS-2 native ext4 read backend` (34b8775) —
  core `Ext4Backend` implementation against `ext4-view@0.9.3`
  exact-pinned. Probe unified with ext4 magic at offset 0x438 +
  0xEF53. Partition dispatcher in `main.rs` Phase 6.5 mounts every
  ext4 partition it finds via native backend.
- **PR-2** `test(ext4): host tests vs. ext4-view 0.9.3` (a6a18d0)
  — 14 host tests under `lamboot-fs-tests` pinning ext4-view API
  surface + error translation table.
- **PR-3** `test(ext4): QEMU harness + 6-distro fleet regression`
  (7719efd) — parametric QEMU harness; streaming fixture
  decompression; 6 distro fixtures (ubuntu/debian/fedora/fedora-
  lamboot/endeavouros/popos). BlockIO adapter (not DiskIo — spec
  v1.1 amendment) because DiskIo isn't universal on OVMF partition
  handles.
- **PR-4** `test(ext4): cargo-fuzz harnesses + parser extraction`
  (34b0068) — libFuzzer targets for `parse_ext4_superblock`,
  `compute_aligned_read`, and full `Ext4::load` API-drift canary.

### Added — SDS-5 (BLS multi-filesystem discovery) — LANDED

Four-PR sequence (see `docs/specs/SPEC-BLS-MULTI-FS.md` v1.1):

- **PR-1** `feat(discovery): volume-polymorphic BLS discovery`
  (f6ed798) — `discover_all_entries(volumes, policy, trust_log)`
  replaces ESP-only scanner. Every volume with `/loader/entries/`
  gets scanned via the FsBackend-uniform API. `BootEntry` and
  `BlsEntry` gain `source_volume_index` + `source_backend_tag`.
  Per-volume trust events: `bls_entries_found`,
  `bls_entry_read_failed`, `bls_entry_invalid`. Legacy
  `\EFI\fedora\vmlinuz*` distro-path heuristic descoped — proper
  BLS discovery on every volume supersedes it.
- **PR-2** `test(bls): pure parser extraction + 23 host tests`
  (0221121) — pure parsing split into `bls_parse.rs` so
  `lamboot-fs-tests` can include it verbatim via `#[path]`. 23
  host unit tests covering Fedora-style entries, multi-value
  initrd, multiple `options` lines, GRUB `$variable` stripping
  (including the v0.8.3 `\$var` form bug fixed inline), boot-
  counter filename suffixes, UAPI.10 version comparison, and BLS
  sort ordering (incl. task #51 Pop!_OS regression).
- **PR-3** `feat(discovery): dedup + boot-counter policy`
  (e3bcfb9) — parsed-entry-equality dedup via manual `PartialEq`
  excluding source fields. `bls_duplicate_filename` event on
  filename collision with different content. Boot-counter
  decrement now routes through `selection.source_volume_index`
  (previously always ESP); on non-FAT source, emits
  `boot_counter_skipped_ro` + human-readable warning instead of
  silent failure. Fixes the silent NotFound that occurred when
  selecting a Fedora BLS entry on ext4.
- **PR-4** `test(bls): VM 122 boot-through + harness + docs`
  (this commit) — `run-qemu-ext4-backend-test.sh` extended with
  SDS-5 assertions: `bls_entries_found` event presence, ext4-
  backend scan confirmation, Fedora-fixture entry count > 0.
  Fleet 5/5 PASS; Fedora fixtures (VM 122 + VM 201) discover 4
  BLS entries each on ext4 — **previously invisible in v0.8.3,
  now visible + selectable + bootable-via-shim-chainload**.
  (Native boot-via-ext4 remains gated on SDS-3's PE loader per
  spec v1.1 §8.4 amendment.) `docs/INSTALL-REFERENCE.md` §5
  documents the `$ESP/loader/entries/` write policy.

**Spec v1.1 amendments** (all three SDS-5 specs):

- §5 dedup key → `(filename_lowercase, parsed BlsEntry equality)`.
  SHA-256 content hashing deferred until SDS-3 brings `sha2` in
  naturally; parsed equality is semantically superior anyway.
- §6.3 signature corrected to landed `EspWriter::rename(dir_path,
  old_name, new_name)` — intra-directory, matches UEFI
  `File::rename` semantics.
- §8.4 boot wiring re-homed to SDS-3. SDS-5 delivers
  discovery + visibility; native kernel-read-from-ext4 is SDS-3.

### Added — SDS-3 (native Rust PE loader) — LANDED

Four-PR sequence (see `docs/specs/SPEC-NATIVE-PE-LOADER.md` v1.1):

- **PR-1** `feat(pe_loader): SDS-3 PR-1 — native PE loader core + pure module`
  — new `lamboot-core/src/pe_loader.rs` (Volume-dependent side:
  allocate_pages, zero, copy sections, apply relocations, install
  LoadedImageProtocol, start_image) + `lamboot-core/src/pe_loader_pure.rs`
  (pure parse/validate/relocate/SHA-256). Goblin 0.10.5 + sha2 0.10.8
  exact-pinned. Every `unsafe` block carries a `SAFETY:` comment.
- **PR-2** `test(pe_loader): SDS-3 PR-2 — host unit tests via #[path]`
  — 19 tests covering §5.1 validation + §9 relocation-math + error
  taxonomy + SHA-256 known-vectors + LamBoot's own lambootx64.efi
  as the known-good PE corpus entry.
- **PR-3** `feat(boot): SDS-3 PR-3 — native-vs-firmware branch + policy`
  — boot.rs wires `boot_uki` + `boot_linux` through the native path
  when policy + verify allow. `LoaderNativePeMode::Auto` default
  routes through `pe_loader::load_pe` + `pe_loader::start_image`
  after `ShimLock::Verify` succeeds; fallback to `uefi::boot::load_image`
  when verify is unavailable. New trust events: `image_loaded_native`
  (SHA-256 + backend + path), `image_load_failed` (with
  `PeLoadError::as_log_token()` token), `image_loaded_firmware`
  (audit flag for the fallback path). Chainload targets stay on the
  firmware path because they may have imports the native loader
  rejects by design.
- **PR-4** `test(pe_loader): SDS-3 PR-4 — fuzz + harness + docs`
  — cargo-fuzz target `pe_load` in the standalone fuzz crate (12M+
  runs in 30s smoke, zero crashes); QEMU harness extended with
  image-load event tallies; CONFIGURATION-GUIDE `[loader]` section;
  example policy.toml with `[loader].native_pe`.

**Policy gate** (SDS-3 §2.2 amendment #2): `policy.toml [loader].native_pe`
accepts `"auto" | "always" | "never"` (case-insensitive). Default
`"auto"` — the safe rollout choice with one-flag rollback to firmware.

**Security effect** on a Fedora ext4 `/boot` system under default
policy: the shim-15.8 `ShimLock`-uninstall failure mode is
**structurally unreachable** for the kernel-load step. LamBoot
calls `ShimLock::Verify(kernel_bytes)` once, then loads the kernel
via its own PE loader — `BS->LoadImage` is never invoked for the
kernel. Combined with SDS-6 (no legacy FS driver load for ext4),
the entire v0.8.3 failure path is eliminated for the 95%-of-users
ext4 configuration.

**Spec v1.1 amendments** (SPEC-NATIVE-PE-LOADER.md):
- §2.2 `load_pe_stream` deferred (goblin::PE::parse is &[u8]-only;
  kernels fit RAM with headroom). Tracked on the roadmap.
- §2.2 new `[loader].native_pe` policy gate following SDS-6 pattern.
- §13.1 host tests via #[path] pure-module include, not mocked
  firmware.
- §13.2 corpus from LamBoot's own binary + hand-crafted bad-PE
  fixtures, not captured kernels.

**§16 acceptance** — v0.9.0 scope complete except the release-
validating live boot-through, which is human-in-the-loop on actual
VM 122 hardware (SDS-3 §13.5 — not a CI-time check). Specifically:
  [x] goblin + sha2 pinned, default-features=false
  [x] pe_loader.rs with LoadedImage, PeLoadError, load_pe, start_image
  [x] §13.1 unit tests pass (19/19)
  [x] §13.2 corpus parses — LamBoot's own lambootx64.efi
  [x] §13.3 QEMU integration — `run-qemu-native-pe-harness.sh` boots
       LamBoot with a synthesized BLS entry, auto-selects via
       `loader.conf timeout=1` (with QMP `sendkey ret` as defensive
       backup), and asserts `image_verified` + `image_loaded_native`
       events appear in the serial log. CI-time exerciser of the
       native PE path; closes P-S3-1 in V0.9.0-RELEASE-PLAN.
  [x] §13.4 fuzz — 12M+ runs / 30s / zero crashes; extended pe_load
       657M-run / 30-min clean
  [x] §13.5 VM 122 live boot — validated on Fedora 43 Sway
       VM (separate ext4 /boot): `image_verified sha256=c734236e…
       verified_via=degraded_trust_sb_off` → `image_loaded_native`
       same sha256, loader=native_pe_loader; Fedora 6.19.12-200.fc43
       booted cleanly. Also validated on VM 120 Ubuntu 25.10 under
       shim+MOK SB-on (verified_via=shim_mok, sha256=4839d9d7…).

### Added — SDS-6 (legacy UEFI FS driver deprecation) — LANDED for v0.9.0 scope (the §2.4 tarball-content removal of `ext4_x64.efi` from `dist/` is explicitly v1.0-deferred behind founder sign-off and is NOT a v0.9.0 gate)

- `lamboot-core/src/policy.rs` — new `[drivers].legacy_uefi_drivers`
  policy.toml field with values `"auto" | "always" | "never"`
  (case-insensitive). Default `"auto"`. `DriversLegacyMode` enum
  added to the `Policy` struct with documented semantics.
- `lamboot-core/src/drivers.rs` — per-driver gating. In `"auto"` mode,
  drivers whose target filesystem is natively covered by a compiled-in
  backend are skipped. Current coverage (via `Ext4Backend` from SDS-2):
  `ext4_*.efi`, `ext2_*.efi`, `ext3_*.efi`. FAT is always UEFI-native
  so `fat_*.efi`/`vfat_*.efi` (not currently bundled) would also skip.
  `"always"` restores v0.8.3 load-everything behavior; emits
  `legacy_driver_redundant` when loading a driver whose FS is already
  covered. `"never"` skips all drivers + emits
  `legacy_driver_skipped_policy`.
- **Trust-log event taxonomy expanded** for audit clarity:
    * `legacy_driver_loaded` — replaces the old `driver_loaded`
      for FS drivers. Diagnostic module loads (`\EFI\LamBoot\modules\`)
      keep their distinct code path and are unaffected.
    * `legacy_driver_rejected` — replaces `driver_rejected`.
    * `native_backend_preferred` — emitted per driver skipped
      because a native backend covers its FS.
    * `legacy_driver_skipped_covered` — companion to
      `native_backend_preferred`; annotates the Auto-mode decision.
    * `legacy_driver_skipped_policy` — emitted per skip in Never mode.
    * `legacy_driver_redundant` — emitted in Always mode when a
      natively-covered driver is loaded anyway.
- `examples/policy.toml` — documents the new `[drivers]` section.
- Security effect: under Auto mode on an ext4-only system, LamBoot
  now loads **zero** third-party UEFI filesystem binaries at boot.
  The shim 15.8 `ShimLock`-uninstall failure mode is structurally
  unreachable for that configuration — matches SDS-2's §1.2
  architectural claim.

- `tools/lamboot-install` — new `--with-drivers-legacy=MODE` flag
  (`auto | all | none`); default `auto` skips driver install for
  natively-covered filesystems. `--with-drivers` retained as alias
  for `--with-drivers-legacy=all`. `phase2_assess_drivers` gates
  `NEED_FS_DRIVER` on `is_filesystem_natively_covered(BOOT_FSTYPE)`
  in Auto mode. `none` warns loudly if `/boot` is non-native.
- `tools/lamboot-install --update` — new `prune_natively_covered_drivers`
  pass removes `ext4_x64.efi` / `ext2_x64.efi` / `ext3_x64.efi`
  (and `-signed` + aarch64 variants) from the ESP when they're
  residue from a v0.8.x install and policy is Auto. Re-installing
  them on a v0.9.x install is explicit: `--with-drivers-legacy=all`.
- `docs/INSTALL-REFERENCE.md` — new "SDS-6 Driver Install Policy"
  section documenting the four flag values + prune behavior.

**Spec v1.1 amendments** (SPEC-UEFI-FSDRV-DEPRECATION.md):

- §1 cross-repo scope — SDS-6 is a single logical deliverable
  crossing lamboot-dev drivers.rs + policy AND lamboot-install
  (both in this repo). Tarball content changes (§2.4 removal of
  ext4_x64.efi from dist/) are a v1.0 release artifact deferred
  behind v0.9.0 + founder sign-off.
- §3 native-coverage decision uses compile-time knowledge
  (`filesystem_natively_covered(filename)` static match) rather than
  a runtime probe against `&[Volume]`. Equivalent behavior, simpler
  code, no main.rs phase reorder needed. When v1.1+ adds a
  compile-time-gated `BtrfsBackend`, the match extends via `#[cfg]`.

### Added — SDS-4 (native trust chain) — LANDED

Four-PR sequence (see `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` v1.1):

- **PR-1** `feat(trust): SDS-4 PR-1 — verify_kernel_bytes + VerifiedBytes + §6.4 invariant`
  (2a328ba) — `verify_kernel_bytes(bytes, path, trust_log)` is the
  single canonical kernel-verify site. Returns `VerifiedBytes<'b>`
  (zero-copy borrow + pre-computed SHA-256 + verified_via token) and
  emits the `image_verified` trust event with the digest attached.
  The §6.4 release-mode `assert_eq!` between the bytes we verified
  and the bytes `pe_loader` loaded (via `LoadedImage::sha256()`) is
  the TOCTOU bright line: if they diverge, the bootloader aborts.
  `trust_log.rs` schema extended with `verifier_tag` (e.g. `"shim-15.8"`)
  + `sha256_hex` fields; stable `verified_via` vocabulary constants
  pinned at this layer. Spec v1.1 amendments #1–4 recorded inline.
- **PR-2** `feat(trust): SDS-4 PR-2 — Step 5/6/7/8/11/12 events throughout boot flow`
  (5846d38) — the intermediate trust events — `policy_loaded`,
  `policy_invalid`, `volume_mounted`, `entries_discovered`,
  `entry_selected`, `kernel_measured`, `cmdline_measured` — are
  recorded at every decision point so the audit timeline lines up
  with SDS-4 §7's Step-by-Step boot diagram. `boot_entry` now takes
  `&mut [Volume]` and `split_first_mut`s the ESP slot out so
  `native_load_and_start` can call `trust_log.flush(esp_for_flush)`
  right before `start_image`, guaranteeing the `image_verified` +
  `image_loaded_native` pair is persisted before control transfers.
- **PR-3** `docs(trust): SDS-4 PR-3 — SECURITY-MODEL v0.9.x + claims contract + verify-claims.sh`
  (d0ebeb2) — `docs/SECURITY-MODEL.md` bumped to 0.9.x with an
  authoritative-reference blockquote pointing to the SDS; new
  "What changed in v0.9.x (from v0.8.3)" section explains the
  native-path security story end-to-end. `lamboot-core/src/security_override.rs`
  module-doc narrowed to flag its scope as **legacy UEFI FS driver
  loads only** — kernel-load path no longer reaches it under SDS-4.
  New `tools/verify-claims.sh` (chmod +x, CI-ready) walks §8.1
  permitted-claims and confirms each file + symbol reference still
  resolves, greps user-facing docs for §8.2 prohibited substrings
  with WARN-only behavior (so disclaimers stay allowed).
  Current tree: 10/10 claim references pass, 2 legitimate disclaimers
  flagged.
- **PR-4** `test(trust): SDS-4 PR-4 — host tests + QEMU assertions + pure split`
  (this commit) — `lamboot-core/src/trust_log.rs` split into a pure
  module (`trust_log_pure.rs` — schema, accumulator, stable vocabulary,
  JSON-Lines serializer) + a thin UEFI wrapper (`flush()` via
  `EspWriter`). 19 host tests under `lamboot-fs-tests/tests/trust_log_pure.rs`
  pin the JSON field order, optional-field elision, JSON escape
  behavior, sequence monotonicity, accumulation across flushes,
  verify→load ordering, sha256 round-trip, and every token value in
  `ALL_VERIFIED_VIA`. `run-qemu-ext4-backend-test.sh` gains three
  SDS-4 soft-assertion blocks (§6.4 sha256 cross-check, §6.2
  ordering, §6.3 vocabulary closure) — hard-fail when either event
  is present with a violation, neutral (~) when the harness menu
  timeout means neither fires.

**Spec v1.1 amendments** (SPEC-NATIVE-TRUST-CHAIN.md):

- §6.1 adds `verified_via = degraded_trust_sb_direct` for
  firmware-DB-direct LamBoot loads (SB on, no shim in chain) —
  same trust model as SB-off from LamBoot's perspective (the
  admin's db key covers the tree). Distinct token for audit
  clarity.
- §6.4 invariant promoted from debug-only `debug_assert!` to
  release-mode `assert_eq!` — a TOCTOU-class divergence is a
  bright-line abort, not a log warning.
- §7 `VerifiedBytes` carries a zero-copy byte borrow rather than
  cloning — SHA-256 computes once on the original, `pe_loader`
  consumes the same slice.
- §8.4 `boot_linux` stays one function with policy-branch inside
  rather than splitting into `boot_linux_native` +
  `boot_linux_firmware` — keeps call-site topology single-owner
  for the trust-log flush ordering.

**Acceptance coverage** (SDS-4 §13):
  [x] §13.1 host unit tests — trust_log schema (19 tests passing)
  [x] §13.2 QEMU integration — soft assertions in ext4-backend harness
  [x] §13.3 live-boot pass on VM 122 hardware — validated.
       VM 122 Fedora 43 Sway (separate /boot ext4, SB off): full
       trust chain `image_verified → image_loaded_native` with
       matching sha256 `c734236e…`, verified_via=degraded_trust_sb_off,
       loader=native_pe_loader, kernel 6.19.12-200.fc43 booted
       cleanly. Companion validation on VM 120 Ubuntu 25.10
       (single-root ext4, SB-on shim+MOK): same invariant satisfied
       with verified_via=shim_mok, sha256=4839d9d7…. Two SB states
       × two partition layouts covered.

### Fixed — pre-v0.9.0 polish

- **Policy defaults reconciled (Task #43).** Runtime `Policy::default()`
  in `lamboot-core/src/policy.rs` now matches `examples/policy.toml`:
  `default_timeout_ms = 5000`, `crash_threshold = 3`, `measured_boot =
  true`. Added a compiled-in policy floor (`MAX_CRASH_THRESHOLD = 10`,
  `MAX_WATCHDOG_GRACE_SECONDS = 300`, plus build-time switches
  `FLOOR_REQUIRE_SECURE_BOOT` / `FLOOR_REQUIRE_MEASURED_BOOT`) so
  offline tampering of `policy.toml` cannot weaken security below the
  baseline. `load_policy()` now returns `(Policy, Vec<PolicyClampReport>)`
  and clamped fields surface as `log::warn!` entries at boot. See
  `docs/specs/SPEC-POLICY-DEFAULTS.md` for the full rationale.

- **Up-front BIOS / non-UEFI refusal in `lamboot-install` (Task #45).**
  New `detect_firmware_mode()` function runs in Phase 1 (after distro
  detection) and refuses to install on a BIOS-booted host by checking
  for `/sys/firmware/efi`. Previously, Phase 6 would eventually fail
  against `efivarfs`, but only after Phase 1–5 had already copied files
  to the ESP. New check: refuses with distro-specific migration guidance
  (Ubuntu/Debian/Fedora/Arch all get `lamboot-migrate to-uefi --disk`
  pointers); `--force` overrides for advanced users preparing a disk
  for a different host; chroot is detected and the check is skipped.

- **Fallback self-loop prevention (Task #49).** `install_fallback()` now
  identifies what bootloader currently occupies `\EFI\BOOT\BOOTX64.EFI`
  (via PE string-markers for shim / GRUB / systemd-boot / rEFInd /
  Windows) and refuses `--fallback` if it would displace a distro
  bootloader, explaining the self-loop risk: if LamBoot's primary
  Boot#### entry ever fails, firmware re-invokes `\EFI\BOOT\BOOTX64.EFI`
  which — if that path is LamBoot — produces a loop with no recovery
  unless the operator manually edits the ESP. `--force` overrides for
  removable-disk scenarios. Previously every `--fallback` silently
  replaced whatever was there.

## [0.8.4]

**Release posture:** coordinated `lamboot v0.8.4` + `lamboot-tools v0.2.0`.
v0.8.3 shipped the signing + audit layer; v0.8.4 closes the
Proxmox-toolkit coordination gaps so the companion `lamboot-tools`
v0.2.0 toolkit (11 tools across 3 RPM subpackages) can ship alongside.

Tarball SHA256: `4671691f597627ee354f36e945dc5d68a75709af4524a88c5a6aa9ae87056830`

Proxmox integration test on `pve.a.lamco.io` (VM 120): PASS on all 8
verifications.

See also: `lamco-admin/lamboot-tools v0.2.0`.

### Fixed

- **`tools/lamboot-hookscript.pl` rewritten to fw_cfg file-reference pattern** (commit `2892446`). Pre-0.8.4 hookscript called `qm set --args` during pre-start, which silently failed because Proxmox config-locks the VM config during that lifecycle phase. The rewrite removes all `qm set` calls: `lamboot-pve-setup` from the toolkit sets the permanent `args:` line once on a stopped VM, and this hookscript's pre-start job becomes writing `/var/lib/lamboot/<VMID>.json` for QEMU to expose via fw_cfg. Version header `# version: 0.8.4` allows `lamboot-pve-setup doctor-hookscript` to detect and verify.

### Added

- **`lamboot-install --toolkit-prompt`** (commit `c4a9b4e`). Interactive `Install lamboot-tools for diagnostic and repair utilities? [y/N]` prompt at the end of a successful install, plus `--install-toolkit` / `--no-install-toolkit` flag overrides for non-interactive scripts. Distro-aware install guidance: Fedora/RHEL/EPEL shows `dnf copr enable lamco/lamboot-tools`; Debian/Ubuntu/Arch shows the source-tarball URL with a note that native packaging lands in `lamboot-tools v0.3`. Skipped on `--dry-run`, `--update`, `--quiet`, or partial failure.
- **`/etc/lamboot/fleet.toml` schema v1 consumption** (commits `ada5cb6` + `2892446`). Both `tools/lamboot-monitor.py` and `tools/lamboot-hookscript.pl` now read the shared fleet config defined by the toolkit. Monitor seeds argparse defaults for `--alert-webhook` (HTTPS enforced) and `--log-path` from `[monitor]`. Hookscript reads `[hookscript]` inject flags plus `[roles]` explicit VMID mapping and `[tags]` tag-to-role mapping for per-VM role resolution. Additive with graceful fallback: missing file / missing TOML parser / wrong schema version / malformed TOML all fall back to hardcoded defaults so v0.8.4 is safe to deploy before fleet.toml exists.

### Changed

- **`docs/LAMBOOT-TOOLS-OVERVIEW.md` rewritten** (commit `51ce546`) to reflect the real state of the companion toolkit. Previously described "5 bash CLI utilities"; now describes 11 tools across 3 RPM subpackages (`lamboot-tools`, `lamboot-migrate` dual-pub, `lamboot-toolkit-pve`) with Copr-based install flow and Option 2 packaging architecture.
- **`README.md` adds "Diagnostic and repair utilities" section** (commit `b812fea`) linking `github.com/lamco-admin/lamboot-tools`.
- **Roadmap "Toolkit Pivot" intro updated** (commit `51ce546`) with current toolkit state.
- **`docs/specs/SPEC-LAMBOOT-MIGRATE.md` §14 reconciliation flipped to RESOLVED** (commit `51ce546`). The v1.0.0 implementation in the toolkit closes every gap from §14.1–§14.7, plus `--remove-grub` distro-aware cleanup beyond spec. Each row marked `RESOLVED` / `RESOLVED+` / `KEPT`.

### Added — should-have cross-references

- **`docs/KEY-GENERATION.md` §10 "Operator tooling"** (commit `51ce546`) back-links to `lamboot-signing-keys` in the toolkit; lists `generate`/`rotate`/`enroll`/`sign-binary`/`revoke`/`list`/`show`/`verify`/`import`/`export` subcommands.
- **`docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md` "Operator tooling"** (commit `51ce546`) maps `sign-binary`/`rotate`/`verify` to procedures in the strategy doc.
- **`docs/OVMF-VARS-PROXMOX.md` §12** (commit `51ce546`) notes `lamboot-pve-ovmf-vars` in the toolkit is a mirror of `tools/build-ovmf-vars.sh` here; canonical source stays in this repo.

## [Unreleased — earlier items carried forward]

### Added

- **`lamboot-inspect`** — new diagnostic tool for parsing LamBoot's on-disk artefacts. Stdlib-only Python, six subcommands (`trust-log`, `boot-log`, `summary`, `show`, `verify`, `dump`). Parses the SDS-4 schema-v2 trust log with full validation, renders boot.log with phase timing, produces one-page last-boot summaries, verifies SDS-4 §8.1 website-claims against code-path evidence (CI-ready), and creates diagnostic bundles for bug reports. Includes man page (`lamboot-inspect(1)`), bash + zsh completions, 44-case host test suite, and user guide at `docs/LAMBOOT-INSPECT.md`. Ships in the release tarball alongside `lamboot-install`.

### Changed

- **SDS-7 spec clarified.** `SPEC-LAMBOOT-MIGRATE.md` now correctly frames itself as the v1.0 target for the existing `lamboot-migrate` v0.1.0 tool in the companion toolkit, not a green-field design. Added §14 "Deviations from existing v0.1.0" with a full reconciliation table. **Flipped to RESOLVED status in the v0.8.4 prep section above.**

### Infrastructure

- **Layer-2 FS backend trait** (SDS-1) implemented: `FsBackend` trait with `FatBackend` adapter + `FatStream` streaming, `Volume` dispatch coordinator with 8 MiB LRU cache, `EspWriter` FAT-only-by-construction write path, `Ext4Backend` skeleton (SDS-2 swap-in target). Migrated all 13 consumer files to the new API. Extracted pure value types to `fs_types.rs` for host testing. New `lamboot-fs-tests` host crate with 36 passing tests.

## [0.8.3]

Release posture: **the signing + audit layer.** v0.8.3 ships the production signing pipeline, SecurityOverride (Path F), trust-evidence log, and install-script hardening. It is the foundation every later release builds on.

**What v0.8.3 is good for:** Secure-Boot-off installs (full feature set, all filesystems, no caveats); Secure-Boot-on installs with UKI on the ESP; Secure-Boot-on installs with firmware-DB-signed kernels; Proxmox VM fleets using UKI-based images.

**Known limitation accepted for v0.8.3:** stock `/boot` on ext4 under Secure Boot fails on shim 15.8 (current Ubuntu/Debian shim). Root cause: shim 15.8 uninstalls its `ShimLock` protocol after our UEFI ext4 driver's `StartImage` completes, so later kernel verification has no shim to delegate to. Fully diagnosed with per-hook counters. **Structural fix in v1.0** via a native Rust ext4 reader (`ext4-view`) plus a native PE loader (on top of `goblin`) — no UEFI FS driver load required, no firmware `LoadImage` re-check.

**Effective positioning for v0.8.3:**
- "Modern Rust UEFI bootloader for Linux, with an honest signing and audit layer"
- NOT "Secure Boot on stock Linux just works" — that's v1.0's promise (~14 weeks out)

### Changed (breaking default)

- **`lamboot-install` now makes LamBoot the default boot entry on install.** Previous default was to preserve the existing default and append LamBoot last in `BootOrder` (safe-trial behavior). New default matches `grub-install` / `bootctl install` / `refind-install`: when you install LamBoot, the firmware will pick LamBoot next boot. The previous default is always preserved as the next entry in `BootOrder`, so it remains one keystroke away from the firmware boot menu. Opt back into the old behavior with `--no-make-default`. The explicit `--set-default` / `--make-default` flags are retained for scripts that want to be unambiguous.

### Added

- **Production signing key hierarchy** — PK 4096, KEK 4096, db 2048 per shim-MOK compatibility constraints (see `docs/KEY-GENERATION.md`).
- **Bootloader signing pipeline** — `build.sh` + `tools/sign-lamboot.sh` produce `lambootx64-signed.efi` using `sbsign` with SBAT section added via `llvm-objcopy` (GNU objcopy produces corrupt PE output).
- **Session-cached signing workflow** — `tools/sign-unlock` + `tools/sign-lock` for tmpfs-backed db.key decryption per terminal session.
- **Secure Boot deployment documentation** — `docs/SECURE-BOOT-DEPLOYMENT.md` covering four configurations: (1) SB disabled, (2) firmware db enrollment, (3) shim + MOK, (4) custom OVMF VARS (Proxmox zero-touch).
- **MOK enrollment guide** — `docs/MOK-ENROLLMENT-GUIDE.md` with screen-by-screen MokManager walkthrough.
- **Proxmox OVMF VARS guide** — `docs/OVMF-VARS-PROXMOX.md` for zero-touch fleet deployment per storage backend.
- **Install-script Secure Boot flags** — `--signed`, `--no-shim`, `--no-mok` on `tools/lamboot-install`. MOK auto-enrollment via `mokutil --import`.
- **Path F: SecurityArchProtocol override** — new `lamboot-core/src/security_override.rs` implements the systemd-boot pattern for driver loading under shim+MOK. Resolves MOK-signed driver ACCESS_DENIED on shim < v16 (Ubuntu 25.10, Debian 13, current Fedora).
- **ShimRetainProtocol support** — `request_shim_retain_protocol()` sets the shim 15.8+ variable to prevent ShimLock protocol from being uninstalled after driver `StartImage()` (shim #444 / sd-boot precedent).
- **Trust-evidence log** — `lamboot-core/src/trust_log.rs` writes structured JSON to `\loader\boot-trust.log` on the ESP recording every image-authentication decision. First-of-kind in the Linux bootloader space.
- **Unified Kernel Image (UKI) first-class menu integration** — automatic discovery of UKIs in `\EFI\Linux\*.efi` with PE section metadata extraction (`.osrel`, `.cmdline`, `.uname`).
- **Migration Tier 2 (`--replace`)** — ESP + NVRAM backup, GRUB cmdline extraction, atomic migration to LamBoot with fallback entry.
- **Debian/Ubuntu kernel hooks** — `kernel-hooks/zz-lamboot-postinst` + `zz-lamboot-postrm`, deployed to `/etc/kernel/{postinst,postrm}.d/` on install, keeping BLS entries synchronized with installed kernels.
- **BLS entry lifecycle handling** — install script's `--update` path detects stale LamBoot-owned BLS entries (kernel they reference no longer exists) and regenerates.
- **`SECURITY.md`** — responsible-disclosure policy and security-reporting procedures.
- **`docs/SECURITY-MODEL.md`** — honest threat model: what LamBoot defends against vs. what remains in the ecosystem. Breaks from the bootloader norm of glossing over SB trust-chain gaps.
- **Public repo reservations** — `lamco-admin/lamboot` and `lamco-admin/lamboot-tools` GitHub repositories (initially empty placeholders).

### Changed

- Retired "LamBoot Dev" test keys; replaced with production keys named `CN=LamBoot Platform Key`, `CN=LamBoot Key Exchange Key`, `CN=LamBoot Release Signing Key 2026` under `O=Lamco Development, C=US, ST=IL, emailAddress=office@lamco.io`.
- Driver loading now wraps `uefi::boot::load_image` with SecurityArchProtocol override for MOK-trust delegation.
- `boot.rs::find_volume_for_path` now returns `Option<usize>` — explicit None instead of silently defaulting to the ESP when a BLS-referenced kernel file is absent.

### Fixed

- `build.sh` now copies `kernel-hooks/` into `dist/kernel-hooks/` so the install script's `find_dist_file` locates the Debian/Ubuntu postinst/postrm scripts.
- Install script's `--update` no longer treats previously-generated BLS entries as "native distro" — it distinguishes manifest-tracked entries and regenerates when their kernels are missing.
- Logging clarity: `boot.rs` no longer emits misleading `Kernel found on volume 0` when the kernel was actually absent everywhere.

### Documented limitations

- Canonical-signed Ubuntu kernels may fail to load under LamBoot in Config 3 (shim+MOK) on distros where `/boot` is on ext4 root. Workaround: use UKIs in `\EFI\Linux\`, or Config 4 with Canonical's CA added to custom OVMF VARS. See `docs/SECURITY-MODEL.md` §3.1.
- Trust-evidence log is not yet cryptographically signed (planned for v0.9.x via Path G — own PE loader).
- ShimLock authenticode verification has edge cases with PE-gap-containing kernels that shim doesn't accept even when they chain to trusted CAs.

### Roadmap

- **v0.9.x:** Path G — own PE loader with direct Authenticode verification, structured cert-chain walking, signed trust log entries, Trust Evidence Panel in GUI, policy-driven trust enforcement.
- **v1.0:** Microsoft `rhboot/shim-review` submission (first Rust bootloader), PQ signature support via MOK, `composefs` integration, `bootupd`-style self-update.
- **v2.0:** TrenchBoot / DRTM integration, full UKI management suite, upstream PE-loader crate contribution to `uefi-rs`.

---

## [0.2.0]

Pre-release development; not formally tagged.
