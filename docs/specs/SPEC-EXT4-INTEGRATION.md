# SPEC-EXT4-INTEGRATION: Native ext4 Read Backend

**SDS number:** SDS-2 (v0.9.x foundation)
**Version:** 1.0
**Date:** 2026-04-22
**Status:** Ready for implementation review
**Target:** `lamboot-core/src/fs_backend_ext4.rs` (full implementation), `lamboot-core/Cargo.toml` (add `ext4-view` dep), test harness under `tests/ext4-images/` (new)
**Depends on:** SDS-1 (`SPEC-FS-BACKEND-TRAIT.md`) — MUST be landed first
**Blocks:** SDS-3 (PE loader needs ext4-sourced byte slices to validate against real kernels), SDS-5 (BLS multi-FS discovery reads from ext4 /boot via this backend), SDS-6 (deprecation story for `ext4_x64.efi` UEFI driver references this backend as replacement)
**External dependency:** [`ext4-view`](https://crates.io/crates/ext4-view) v0.9.3, MIT OR Apache-2.0, Google-authored, `no_std`+`alloc`, read-only by design

---

## 1. Overview

This specification defines how the `ext4-view` Rust crate is integrated into LamBoot as a Layer-2 filesystem backend, how LamBoot feeds block I/O to it via UEFI's `DiskIO` protocol, and the full lifecycle from volume-mount through file read to drop. It also defines feature-compatibility promises, cache strategy, version-pinning policy, and the upstream-contribution stance.

### 1.1 Why ext4-view

The v1.0 promise — "Secure Boot on stock Ubuntu/Debian/Fedora just works" — requires LamBoot to read `/boot` on ext4 **without** loading a UEFI filesystem driver via `LoadImage`+`StartImage`, because that load triggers shim 15.8's `ShimLock` uninstall (see `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md`). A native Rust ext4 reader embedded in the LamBoot binary sidesteps the entire shim-uninstall problem.

`ext4-view` is the correct choice, not a compromise:

- **Purpose-built for UEFI bootloaders.** Author Nicholas Bishop ([nbishop.net/notes-ext4-view](https://nbishop.net/notes-ext4-view.html)) explicitly designed it for reading ext4 partitions from UEFI applications. The crate workspace includes `xtask/uefibench/` — a working UEFI application that uses ext4-view + uefi-rs to walk real ext4 filesystems at boot time. That reference implementation is the skeleton we adapt.
- **Feature-complete for stock distros.** Verified from crate source (`/tmp/ext4-view-rs/src/features.rs`, `src/superblock.rs::check_incompat_features()`) and cross-referenced against `/etc/mke2fs.conf` stock ext4 defaults: every feature `mkfs.ext4` enables by default (`has_journal, extent, huge_file, flex_bg, metadata_csum, metadata_csum_seed, 64bit, dir_nlink, extra_isize, orphan_file`, plus base `filetype, dir_index` HTree) is either REQUIRED, supported, or read-only-compatible. Journal recovery for not-cleanly-unmounted filesystems landed in v0.9.0. TEA-hash directory support landed in v0.9.3.
- **Read-only by design.** Matches LamBoot's security property: "LamBoot's code literally cannot modify your `/boot`." Write support is an explicit non-goal of ext4-view and we inherit that.
- **`no_std` + `alloc` only.** No `std` dependency. No runtime. Matches LamBoot's target.
- **Minimal transitive dependencies.** `bitflags 2.0` + `crc 3.0`. That's it. No async, no log, no proc-macros.
- **Maintained.** 564 commits through 2026, codecov-tracked, 100% API-documented, MSRV bumps communicated in CHANGELOG.
- **License-compatible.** MIT OR Apache-2.0 — same dual-license as LamBoot itself.

### 1.2 What this SDS delivers

- `lamboot-core/src/fs_backend_ext4.rs` fully implemented, satisfying the `FsBackend` trait defined in SDS-1.
- A concrete `Ext4Read` adapter wrapping UEFI's `DiskIo` protocol, owned by the backend.
- A feature-probe path in the volume-mount code (Layer 1 / `partitions.rs`) that recognizes ext4 filesystems from their superblock and constructs an `Ext4Backend` for them.
- A version-pinning policy for the `ext4-view` crate and a documented upgrade/CVE-response procedure.
- A cache-sizing decision backed by measurement on representative disks.
- A test-harness layout: mock `Ext4Read`, disk-image test fixtures, QEMU integration suite.
- A CHANGELOG entry template and upstream-contribution posture.

### 1.3 What this SDS does NOT deliver

- **Write support.** Not in ext4-view's scope; won't be in ours. Any write to `/boot` (kernel install, boot-counter rename) goes through the distro's own tooling at install time, not LamBoot's code at boot time.
- **ext2 / ext3 variant handling beyond what ext4-view provides natively.** ext4-view claims these work; we rely on its claim, surface any failure as `FsError::UnsupportedFeature` with ext4-view's error message appended.
- **Block-level write-through corruption recovery.** If a block is unreadable, that is a hardware or filesystem-corruption event; we emit a trust-log event and return `FsError::Corrupt`. No journal-replay-on-mount. No `fsck`.
- **Btrfs, XFS, f2fs, or ZFS backends.** Separate SDSes (or permanent descope per `NATIVE-FS-AND-PE-LOADER-STRATEGY §5.5–5.7`).
- **Multi-device filesystems.** ext4 doesn't have this; not a concern. But for completeness: LamBoot assumes exactly one `DiskIo` per ext4 volume.
- **Discarding or updating the journal.** Journal replay for an unclean filesystem is documented in ext4-view's `src/journal/`; we rely on it, don't modify it.
- **Extended-attribute reading (xattrs).** ext4-view doesn't yet expose xattrs. Not needed for BLS discovery or kernel loading. Filed as a future enhancement.

### 1.4 Constraints

All SDS-1 constraints apply. Additionally:

- The `ext4-view` crate version MUST be pinned exactly in `Cargo.toml` (not semver-range). v0.9.0 ships v0.9.3 pinned; subsequent LamBoot releases explicitly bump.
- LamBoot MUST handle `ext4_view::Ext4Error::Incompatible` by returning `FsError::UnsupportedFeature` with the feature name, emitting a trust-log event, and NOT panicking.
- The `Ext4Read` adapter MUST use UEFI `DiskIo::read_disk(media_id, offset, dst)` (byte-granular) rather than `BlockIO::read_blocks(lba, dst)` (block-granular). Rationale in §5.1.
- Superblock probe at mount time MUST run BEFORE any other filesystem scan to avoid slow false-positive paths.
- The ext4 backend MUST NOT attempt to load any UEFI filesystem driver. Ever. The whole point is bypassing that path.

---

## 2. External interface (what consumers see)

Consumers never touch `ext4-view` types directly. They interact via the SDS-1 `FsBackend` trait.

```rust
// Anywhere in the codebase (main.rs, discovery.rs, boot.rs, bls.rs, …):
let entries = volume.read_dir(&Path::from_str("/loader/entries")?)?;
for entry in entries {
    let bytes = volume.read(&entry.path())?;
    // parse BLS, UKI, etc.
}
```

The `Volume` dispatches to `Ext4Backend` if the volume's filesystem is ext4; to `FatBackend` if FAT; etc. The caller does not branch on backend type.

### 2.1 Backend `tag()` value

`Ext4Backend::tag()` returns `"ext4-view@0.9.3"` — the `@version` suffix is important because it propagates into trust-log events, giving downstream auditors a reproducible record of exactly which library version produced which read. On library upgrade we update the string literal alongside the Cargo dep.

```rust
impl FsBackend for Ext4Backend {
    fn tag(&self) -> &'static str { "ext4-view@0.9.3" }
    // …
}
```

The tag is checked by SDS-4 trust-chain assertions and the acceptance tests in §10.

---

## 3. Module layout

```
lamboot-core/
├── Cargo.toml                          — add ext4-view dep (§4)
└── src/
    └── fs_backend_ext4.rs              — THIS SDS delivers this file
        ├── struct Ext4Backend
        ├── impl FsBackend for Ext4Backend
        ├── struct DiskIoReader (the Ext4Read adapter)
        ├── impl ext4_view::Ext4Read for DiskIoReader
        ├── fn probe_ext4_superblock(handle: Handle) -> Option<ProbeResult>
        └── mod translate — Path / FsError / Metadata / DirEntry translation
```

No other files change except:

- `lamboot-core/src/fs.rs` already (post-SDS-1) re-exports the trait types; no changes here.
- `lamboot-core/src/partitions.rs` — add `probe_ext4_superblock` call in the volume-mount pipeline. §6 below.
- `lamboot-core/src/main.rs` — no change. The orchestration is FsBackend-polymorphic.

---

## 4. Cargo dependency

Add to `[dependencies]` in `lamboot-core/Cargo.toml`:

```toml
# Native ext4 read-only backend (SDS-2).
# Pinned exactly — upgrades require a CHANGELOG entry and re-run of the
# disk-image regression suite (see SDS-2 §10).
ext4-view = { version = "=0.9.3", default-features = false }
```

Notes:

- `default-features = false` disables the `std` feature. ext4-view's no_std mode requires `alloc`, which the LamBoot target already provides.
- The exact-version pin (`=0.9.3`) is intentional. Semver ranges would allow cargo to pick a newer patch that might change the feature-unsupported-error shape, which downstream callers match on.
- MSRV of ext4-view 0.9.3 is **Rust 1.85** (per its Cargo.toml). LamBoot's current MSRV must be ≥ 1.85 before this SDS can merge. Verify with `rustup show active-toolchain`; if below, bump `rust-toolchain.toml` in the same PR.

Transitive deps that come in: `bitflags 2.x`, `crc 3.x`. Both are already known acceptable (`bitflags` is a common transitive dep of uefi-rs; `crc` is unique to ext4-view).

### 4.1 Crate licensing in the release tarball

`ext4-view` is MIT OR Apache-2.0. We choose to redistribute under the same dual license — no additional obligations. No action needed beyond the existing `LICENSE-MIT` + `LICENSE-APACHE` + `LICENSE` top-level files.

For completeness, add a line to `LICENSE` noting that bundled crate dependencies retain their own licenses (standard Rust practice). This SDS delivers that amendment.

---

## 5. The `Ext4Read` adapter

The `ext4_view::Ext4Read` trait requires exactly one method:

```rust
fn read(&mut self, start_byte: u64, dst: &mut [u8])
    -> Result<(), Box<dyn core::error::Error + Send + Sync + 'static>>;
```

"Exactly `dst.len()` bytes will be read; an error will be returned if there is not enough data to fill `dst`."

Our implementation wraps UEFI's `DiskIo` protocol. Following upstream's `xtask/uefibench` pattern:

```rust
struct DiskIoReader {
    media_id: u32,
    disk: uefi::boot::ScopedProtocol<DiskIo>,
}

impl ext4_view::Ext4Read for DiskIoReader {
    fn read(&mut self, start_byte: u64, dst: &mut [u8])
        -> Result<(), Box<dyn core::error::Error + Send + Sync + 'static>>
    {
        self.disk
            .read_disk(self.media_id, start_byte, dst)
            .map_err(|e| Box::new(e) as Box<dyn core::error::Error + Send + Sync + 'static>)
    }
}
```

### 5.1 Why `DiskIo`, not `BlockIO`

`ext4-view` consumes byte-granular reads at arbitrary offsets. `BlockIO::read_blocks` takes LBA + must be block-aligned + dst must be a whole number of blocks. Using `BlockIO` would require a read-through adapter that:

1. Computes `lba = start_byte / block_size`
2. Reads aligned blocks into a scratch buffer
3. Copies the requested byte range out
4. Handles reads that span multiple blocks

`DiskIo` does all of this internally (it's the UEFI spec's byte-granular layer over `BlockIO`). Using `DiskIo` is:

- **Simpler** — 3 lines of adapter code vs ~30 for a `BlockIO` read-through.
- **Tested** — UEFI spec-compliant impls have decades of fleet experience.
- **Upstream-compatible** — matches ext4-view's own `uefibench`.
- **No performance loss** — modern firmware's `DiskIo` is a thin wrapper over `BlockIO`, and both paths hit the same caching.

### 5.2 Media-ID handling

`DiskIo` does not know its own media-ID; you obtain it from the `BlockIOMedia` struct reachable via the sibling `BlockIO` protocol. Per upstream `uefibench`:

```rust
fn get_media_id(handle: Handle) -> uefi::Result<u32> {
    let bio = unsafe {
        boot::open_protocol::<BlockIO>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )?
    };
    Ok(bio.media().media_id())
}
```

Note the `unsafe` is required because `GetProtocol` mode is the documented safe-to-share attribute but the uefi-rs wrapper still marks it `unsafe` out of caution. SAFETY comment on our copy:

```rust
// SAFETY: GetProtocol mode on BlockIO does not grant exclusive access;
// per UEFI spec 12.4, multiple agents may hold GetProtocol handles on
// the same device simultaneously. The DiskIo protocol held by this
// backend is opened with open_protocol_exclusive separately, which is
// what actually controls the read path.
```

Media-ID is captured at backend construction and held for the backend's lifetime. If the underlying device is removed mid-boot (unusual; requires hot-plug), read calls fail with `MEDIA_CHANGED`; we surface this as `FsError::Io`.

### 5.3 DiskIo scope

The `ScopedProtocol<DiskIo>` lives inside `DiskIoReader`, which lives inside `Ext4Backend`, which lives inside `Volume`. Drop-chain: `Volume` drops → `Ext4Backend` drops → `DiskIoReader` drops → `ScopedProtocol<DiskIo>` drops → UEFI protocol is closed. No leaks.

Contrast with SDS-1 `FatBackend`'s per-call open-protocol pattern: ext4 reads are high-frequency (reading a 30-MB kernel might trigger dozens of superblock/group-descriptor/inode/extent-tree reads), so holding the protocol open for the backend's lifetime is measurably cheaper. Tested in §10.

---

## 6. Mount-time probe and backend construction

### 6.1 Probe function

In `partitions.rs` (Layer 1), after SDS-1's dispatch logic has enumerated `BlockIO` handles:

```rust
/// Attempt to identify an ext4 filesystem on the given handle.
/// Returns `Some(ProbeResult)` if the superblock is readable and has
/// ext4 magic; `None` otherwise. Does NOT fully validate the filesystem —
/// that happens when `Ext4::load()` is called.
pub(crate) fn probe_ext4_superblock(handle: Handle) -> Option<ProbeResult> {
    // Open DiskIo
    let disk = uefi::boot::open_protocol_exclusive::<DiskIo>(handle).ok()?;
    let media_id = get_media_id(handle).ok()?;

    // ext4 superblock is at byte offset 1024 (skipping boot sector).
    // We read the first 1024 bytes of the superblock — enough to check
    // magic and basic feature flags.
    let mut sb_probe = [0u8; 1024];
    disk.read_disk(media_id, 1024, &mut sb_probe).ok()?;

    // Magic number at offset 56 in the superblock (0x38 = s_magic).
    let magic_lo = sb_probe[56] as u16;
    let magic_hi = sb_probe[57] as u16;
    let magic = magic_lo | (magic_hi << 8);
    if magic != 0xEF53 {
        return None;
    }

    // Read the fs_uuid at superblock offset 104 (s_uuid).
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&sb_probe[104..120]);

    // Read the label at superblock offset 120 (s_volume_name, 16 bytes).
    let label_bytes = &sb_probe[120..136];
    let label = core::str::from_utf8(label_bytes)
        .ok()
        .map(|s| s.trim_end_matches('\0').to_string())
        .filter(|s| !s.is_empty());

    Some(ProbeResult {
        fs_kind: "ext4",
        fs_uuid: Some(Uuid(uuid)),
        label,
    })
}
```

This probe is ~30 lines, reads exactly 1 KiB from disk, and is the first thing called during ext4 volume candidates. No ext4-view code runs yet — we don't want to pay for full filesystem load on every handle that might be some other FS.

### 6.2 Backend construction

If the probe returns `Some`, `partitions.rs` constructs the backend:

```rust
pub(crate) fn build_ext4_backend(handle: Handle, probe: ProbeResult)
    -> Result<Ext4Backend, FsError>
{
    let disk = uefi::boot::open_protocol_exclusive::<DiskIo>(handle)
        .map_err(|e| FsError::Io { source: e })?;
    let media_id = get_media_id(handle).map_err(|e| FsError::Io { source: e })?;
    let reader = DiskIoReader { media_id, disk };
    let ext4 = ext4_view::Ext4::load(Box::new(reader))
        .map_err(translate::ext4_error)?;
    Ok(Ext4Backend {
        ext4,
        fs_uuid: probe.fs_uuid,
        label: probe.label,
    })
}
```

`ext4_view::Ext4::load(Box<dyn Ext4Read>)` reads and validates the superblock, block group descriptors, and root inode. If the filesystem is corrupt, unsupported-feature'd, or encrypted, `Ext4::load` returns `Ext4Error::{Corrupt, Incompatible, Encrypted, …}`, which we translate to `FsError::{Corrupt, UnsupportedFeature, Unsupported}` respectively (see §7 translate table).

### 6.3 Error handling at probe + build time

Probe failure returns `None`. The volume is then passed to the next backend's probe (FAT, btrfs, …). If all probes fail, the volume is skipped silently (SDS-1 §5 policy).

Build-time failure returns `FsError`. The Layer-1 caller logs a trust-log event (`volume_build_failed` with backend tag + FsError token) and skips the volume.

---

## 7. Error translation (ext4-view → LamBoot)

ext4-view's error type is `ext4_view::Ext4Error`. Its variants map to our `FsError` per this table:

| `Ext4Error` variant | `FsError` variant | Trust-log behavior |
|---|---|---|
| `NotFound` | `FsError::NotFound` | No event (caller decides) |
| `IsSpecialFile` | `FsError::Unsupported("special_file")` | One-shot per (path, boot) |
| `NotADirectory` | `FsError::NotDirectory` | No event |
| `IsADirectory` | `FsError::IsDirectory` | No event |
| `Encrypted(_)` | `FsError::Unsupported("encrypted")` | Emit `fs_encrypted` event always (user needs to know) |
| `Incompatible(inner)` | `FsError::UnsupportedFeature { feature: translate(inner), backend: "ext4-view@0.9.3" }` | One-shot per (feature, boot) |
| `Corrupt(inner)` | `FsError::Corrupt(translate(inner))` | Emit `fs_corrupt` event always |
| `Io(inner)` | `FsError::BackendOther(Box::new(inner))` | Emit `fs_io_error` event always |
| anything else | `FsError::BackendOther(Box::new(e))` | Emit `fs_backend_other` event |

Implementation:

```rust
mod translate {
    use crate::fs_backend::FsError;
    use ext4_view::Ext4Error;

    pub(super) fn ext4_error(e: Ext4Error) -> FsError {
        match e {
            Ext4Error::NotFound => FsError::NotFound,
            Ext4Error::NotADirectory => FsError::NotDirectory,
            Ext4Error::IsADirectory => FsError::IsDirectory,
            Ext4Error::Encrypted(_) => FsError::Unsupported("encrypted"),
            Ext4Error::Incompatible(i) => FsError::UnsupportedFeature {
                feature: incompatible_feature_name(&i),
                backend: "ext4-view@0.9.3",
            },
            Ext4Error::Corrupt(_) => FsError::Corrupt("ext4 structural corruption"),
            Ext4Error::Io(_) => FsError::BackendOther(Box::new(e)),
            _ => FsError::BackendOther(Box::new(e)),
        }
    }

    fn incompatible_feature_name(_i: &ext4_view::Incompatible) -> &'static str {
        // ext4-view's Incompatible type is opaque as of v0.9.3 (no Display).
        // Surface a generic label; trust-log records the full ext4_view error
        // via BackendOther on the parent call.
        "ext4_incompatible_feature"
    }
}
```

Note that `ext4_view::Incompatible` is opaque per `CHANGELOG.md` v0.9.0 ("Made the Incompatible type opaque. It is no longer possible to match on specific types of incompatibility."). We accept this and surface a generic label; the exact feature name is lost but the crate's Display impl (via BackendOther in some variants) preserves it for logging.

---

## 8. `Ext4Backend` — full `FsBackend` implementation

```rust
pub(crate) struct Ext4Backend {
    ext4: ext4_view::Ext4,
    fs_uuid: Option<Uuid>,
    label: Option<String>,
}

impl FsBackend for Ext4Backend {
    fn tag(&self) -> &'static str { "ext4-view@0.9.3" }
    fn uuid(&self) -> Option<Uuid> { self.fs_uuid }
    fn label(&self) -> Option<&str> { self.label.as_deref() }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, FsError> {
        let p = translate::path_to_ext4(path)?;
        self.ext4.read(p).map_err(translate::ext4_error)
    }

    fn read_at(&mut self, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let p = translate::path_to_ext4(path)?;
        let mut file = self.ext4.open(p).map_err(translate::ext4_error)?;
        use ext4_view::Seek as _;
        file.seek(ext4_view::SeekFrom::Start(offset)).map_err(translate::ext4_error)?;
        let mut buf = alloc::vec![0u8; len];
        let n = ext4_view::Read::read(&mut file, &mut buf).map_err(translate::ext4_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn exists(&mut self, path: &Path) -> Result<bool, FsError> {
        let p = translate::path_to_ext4(path)?;
        self.ext4.exists(p).map_err(translate::ext4_error)
    }

    fn metadata(&mut self, path: &Path) -> Result<Metadata, FsError> {
        let p = translate::path_to_ext4(path)?;
        let md = self.ext4.metadata(p).map_err(translate::ext4_error)?;
        Ok(translate::metadata(&md))
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let p = translate::path_to_ext4(path)?;
        let iter = self.ext4.read_dir(p).map_err(translate::ext4_error)?;
        let mut out = Vec::new();
        for entry in iter {
            let entry = entry.map_err(translate::ext4_error)?;
            let name = entry.path().to_str()
                .map_err(|_| FsError::InvalidPath("non-utf8 ext4 filename"))?
                .to_string();
            if name == "." || name == ".." { continue; }
            let kind = translate::file_type(entry.file_type());
            out.push(DirEntry {
                name,
                kind,
                backend_tag: "ext4-view@0.9.3",
            });
        }
        Ok(out)
    }

    fn supports_streaming(&self) -> bool { true }

    fn open_stream(&mut self, path: &Path) -> Result<Box<dyn FsStream>, FsError> {
        let p = translate::path_to_ext4(path)?;
        let file = self.ext4.open(p).map_err(translate::ext4_error)?;
        let len = self.metadata(path)?.size;
        Ok(Box::new(Ext4Stream { file, len: Some(len) }))
    }
}

struct Ext4Stream {
    file: ext4_view::File,
    len: Option<u64>,
}

impl FsStream for Ext4Stream {
    fn len(&self) -> Option<u64> { self.len }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        use ext4_view::Read as _;
        Read::read(&mut self.file, buf).map_err(translate::ext4_error)
    }

    fn seek(&mut self, offset: u64) -> Result<(), FsError> {
        use ext4_view::Seek as _;
        self.file.seek(ext4_view::SeekFrom::Start(offset)).map_err(translate::ext4_error)?;
        Ok(())
    }
}
```

### 8.1 Path translation

ext4-view accepts `&str` / `&[u8]` paths with forward-slash separators — identical to our `Path` convention. Translation is `Path → &str` with no modification:

```rust
pub(super) fn path_to_ext4(path: &Path) -> Result<&str, FsError> {
    // Path is already UTF-8 + forward-slash + absolute per SDS-1.
    // Direct pass-through.
    Ok(path.as_str())
}
```

### 8.2 Metadata translation

```rust
pub(super) fn metadata(md: &ext4_view::Metadata) -> crate::fs_backend::Metadata {
    crate::fs_backend::Metadata {
        kind: file_kind_from_ext4(md.file_type()),
        size: md.len(),
        mode: md.mode().bits() as u32,
        read_only: (md.mode().bits() & 0o200) == 0,  // no write bit for owner
        source_backend: "ext4-view@0.9.3",
    }
}
```

---

## 9. Feature compatibility map

Verified from ext4-view v0.9.3 source (`src/features.rs`). Match against distro defaults:

| ext4 feature | ext4-view handling | Ubuntu default | Debian default | Fedora default | RHEL default | Arch default |
|---|---|---|---|---|---|---|
| `filetype` | **required** — accepted | ✓ | ✓ | ✓ | ✓ | ✓ |
| `extents` | supported | ✓ | ✓ | ✓ | ✓ | ✓ |
| `64bit` | supported | ✓ | ✓ | ✓ | ✓ | ✓ |
| `flex_bg` | supported | ✓ | ✓ | ✓ | ✓ | ✓ |
| `metadata_csum_seed` | supported | ✓ | ✓ | ✓ | ✓ | ✓ |
| `has_journal` | supported (replay for unclean) | ✓ | ✓ | ✓ | ✓ | ✓ |
| `huge_file` | read-only-compat | ✓ | ✓ | ✓ | ✓ | ✓ |
| `metadata_csum` | read-only-compat | ✓ | ✓ | ✓ | ✓ | ✓ |
| `sparse_super` | read-only-compat (base) | ✓ | ✓ | ✓ | ✓ | ✓ |
| `large_file` | read-only-compat (base) | ✓ | ✓ | ✓ | ✓ | ✓ |
| `extra_isize` | read-only-compat | ✓ | ✓ | ✓ | ✓ | ✓ |
| `orphan_file` | read-only-compat | ✓ | ✓ | ✓ | ✓ | ✓ |
| `dir_index` (HTree) | supported (v0.9.3 adds TEA) | ✓ | ✓ | ✓ | ✓ | ✓ |
| `compression` | **rejected** | — | — | — | — | — |
| `separate_journal_device` | **rejected** | — | — | — | — | — |
| `meta_bg` | **rejected** | — | — | — | — | — |
| `mmp` (multi-mount protection) | **rejected** | — | — | — | — | — |
| `ea_inode` (large xattrs in inode) | **rejected** | — | — | — | — | — |
| `inline_data` | **rejected** | — | — | — | — | — |
| `large_dir` | **rejected** | — | — | — | — | — |
| `encrypt` | surfaces as Encrypted | — | — | — | — | — |

**Conclusion:** Every default `mkfs.ext4` configuration across the distros we target mounts cleanly via ext4-view 0.9.3. Users with exotic filesystems (`inline_data` manually enabled, `encrypt` directories, `mmp` on shared storage) fall through to a clean `FsError::UnsupportedFeature` with a log event — not a crash, not silent failure.

### 9.1 Regression test: feature survey

Part of SDS-2 delivery: a harness that `debugfs -R 'show_super_stats'`s each VM in the fleet and cross-references the `Features:` line against the supported-set. Automated as `tools/verify-ext4-features.sh` run against a fresh VM-disk image before release. Fails loudly if any target VM's ext4 uses a feature not in our supported-set — prompts an SDS update.

---

## 10. Test plan

### 10.1 Unit tests (mock `Ext4Read`)

Living in `lamboot-core/tests/fs_backend_ext4_tests.rs` (host-side, `#[cfg(test)]` behind a feature gate; uefi-target tests aren't runnable).

Required cases:

1. **Mount success.** Feed a known-good in-memory ext4 image (from `/tmp/ext4-view-rs/test_data/test_disk1.bin.zst`) through a `Vec<u8>`-backed `Ext4Read`. Assert `Ext4Backend` constructs cleanly and `label` / `uuid` match reference values.
2. **Mount failure — not ext4.** Feed 64 KiB of zeros; assert `probe_ext4_superblock` returns `None`.
3. **Mount failure — corrupt.** Flip bytes 0x56-0x57 (magic) to `0xDEAD`; assert probe returns `None`. Flip bytes within the feature flags; assert `Ext4::load` returns `Incompatible` which we translate to `FsError::UnsupportedFeature`.
4. **`read()` happy path.** Open a file `/regular.txt`, read, assert bytes match reference.
5. **`read_at(offset, len)`.** Read a byte range from `/large.bin`; assert contents match `losetup`+`dd` reference.
6. **`read_dir()`.** List `/dir` children; assert set matches `ls -A1` reference.
7. **`exists()`.** `exists("/dir")` → `true`, `exists("/nope")` → `false`, no error in either case.
8. **`metadata()`.** Assert `size`, `kind`, `mode` for a regular file, a directory, a symlink.
9. **Error translation — not found.** `read("/nope")` → `FsError::NotFound`.
10. **Error translation — is directory.** `read("/dir")` → `FsError::IsDirectory`.
11. **Error translation — unsupported feature.** Feed an image with `inline_data` set; assert `FsError::UnsupportedFeature { feature: "ext4_incompatible_feature", backend: "ext4-view@0.9.3" }`.
12. **Streaming.** `open_stream("/large.bin")` → reads 4 KiB at a time, 1024 iterations; assert SHA-256 of concatenated chunks matches the non-streaming full read's SHA-256.

### 10.2 QEMU integration tests

Living in `run-qemu-ext4-backend-test.sh` (new harness). Constructs a disk with GPT → partition 1 FAT (ESP) + partition 2 ext4, writes known test files into the ext4 partition, boots LamBoot, runs a built-in test-module (or emits trust-log events we parse post-boot).

1. **LamBoot detects ext4 partition at mount time.** `trust_log` contains a `volume_mounted` event with `backend_tag="ext4-view@0.9.3"` for partition 2.
2. **Volume UUID matches the `mkfs.ext4 -U <uuid>` value.**
3. **LamBoot reads a known file from ext4.** The file `/testfile.bin` (contents: 1 MiB of `/dev/urandom` with known SHA-256) is readable via the backend; SHA-256 matches.
4. **LamBoot reads `/boot/loader/entries/test.conf` if present** (for SDS-5 cross-check — verifies BLS discovery can see through an ext4 backend).
5. **Clean unmount on exit.** No leaked `DiskIo` handles.

### 10.3 Real-disk fleet regression

Running LamBoot from a fixed commit against a curated fleet of disk images captured from real distro installs. Lives at `tests/ext4-images/`:

- `ubuntu-25.10-default.img.zst` (~40 MB compressed)
- `debian-13-default.img.zst`
- `fedora-43-default.img.zst`
- `rhel-10-default.img.zst`
- `arch-default.img.zst`
- `popos-24.04-default.img.zst`

For each image: mount it via a host-side ext4-view test binary, walk `/boot/`, verify each file-read SHA-256 matches what a mounted loopback `cat` produces. Runs as part of `./run-tests.sh` pre-release.

Fleet fails → SDS-2 cannot ship. A test-coverage gap means we don't know if LamBoot will work on that distro.

### 10.4 Fuzzing

`cargo fuzz` harness for the `Ext4Read` adapter and the probe function. Seeds from `/tmp/ext4-view-rs/test_data/`. Goals:

- No panic on any input.
- No OOM on pathological superblock claiming huge block count.
- No infinite loop on malformed directory structure.

10 minutes of fuzzing at minimum before each release candidate.

---

## 11. Version-pinning and upgrade policy

### 11.1 Pin

`ext4-view = "=0.9.3"` (exact pin). Rationale: opaque error types changed between 0.9.0 and 0.9.3; we match on these. Unpinned semver ranges could silently change behavior.

### 11.2 Upgrade procedure

When a new `ext4-view` version is considered:

1. Read upstream CHANGELOG. Note MSRV bumps, breaking changes, feature adds.
2. Update `Cargo.toml` pin + `Ext4Backend::tag()` string literal in lockstep.
3. Re-run SDS-2 §10 test suite (all four tiers).
4. Update the feature-compatibility map (§9) if upstream adds/removes supported features.
5. Entry in `CHANGELOG.md` under the next version's "Changed (dependency)" subsection.
6. If the upstream version bumped a feature from `Incompatible` to `Supported`, add it to §9 table.
7. Commit with message `deps: ext4-view X.Y.Z → A.B.C`, reference SDS-2 §11.2.

### 11.3 CVE response

If `ext4-view` publishes a security advisory:

1. Same-day: `cargo audit` confirms affected.
2. Same-day: LamBoot source release (patch-level bump, e.g. v0.9.0 → v0.9.1) with the updated pin.
3. Same-day: rebuild tarball, re-sign, publish release.
4. Same-day: SECURITY.md addendum if LamBoot was actually vulnerable in the target-use context.

### 11.4 Upstream-contribution posture

We do not fork `ext4-view`. If we need a feature the crate lacks:

1. Open a GitHub issue on `nicholasbishop/ext4-view-rs` describing the use case (LamBoot bootloader context).
2. If feasible, open a PR implementing the feature in a minimally-invasive way.
3. If PR merged, bump pin as per §11.2. If not, fork as a LAST RESORT, publish under `lamco-admin/ext4-view-rs-fork`, and document the divergence in `docs/analysis/`.

No silent forks. No private patches.

---

## 12. Performance budget

Design target: reading `/boot/vmlinuz-X` (~30 MB) over ext4-view, from native-loader context, should complete in under **2 seconds** on a representative modern system (NVMe, Proxmox host, VM guest).

Rough budget:

| Phase | Budget |
|---|---|
| Superblock + block group descriptor load | 50 ms |
| Inode + extent tree resolution | 50 ms |
| File data read (30 MB at NVMe-typical 2 GB/s) | 15 ms |
| ext4-view per-block overhead | 100 ms |
| LamBoot `FsBackend` dispatch + cache | 10 ms |
| **Total** | **~225 ms** |

Why 2 s target is 10× budget: conservative for older hardware (SATA SSD, USB3 thumb drive); real-world boot-time acceptability is sub-3s to menu-up.

If measured performance is outside budget:

- Cache block-group descriptors in the `VolumeCache` (SDS-1 §4.3 lets us do this without backend changes).
- Investigate ext4-view v0.9.2's block-cache (upstream note: "Added a block cache to improve performance when running in an environment where the OS doesn't provide a block cache." — verify it's enabled by default).
- File against `nicholasbishop/ext4-view-rs` if a structural improvement is warranted.

Performance budget is measured in `run-qemu-ext4-backend-test.sh` with a `perf`-style stopwatch; failure produces a warning, not a test-fail (performance regressions deserve a human look).

---

## 13. Risks and open questions

| Risk / question | Severity | Mitigation |
|---|---|---|
| ext4-view may have edge-case incompatibility with some distro-specific ext4 variant | Low | §9 feature-survey shows stock defaults are fully covered. Fleet regression in §10.3 catches distro-specific surprises before release. |
| v0.9.3 opaque error types lose information compared to older versions | Low | Accept; surface via `FsError::BackendOther` for logging; structural handling at `FsError::{UnsupportedFeature, Corrupt, NotFound}` is unaffected. |
| MSRV bump from 1.85 could impose constraint elsewhere in LamBoot | Low | LamBoot is already on recent nightly for build-std; stable 1.85 is no issue. |
| Performance budget 225 ms is a guess | Medium | Measure in §10.2 tests; if violated, cache tuning per §12. |
| Journal recovery on unclean filesystem could take long | Low | ext4-view does NOT replay the journal (it reads through). Unclean filesystems are still readable; only writes would need journal replay, and LamBoot doesn't write ext4. |
| Encrypted directories return `Encrypted` error | Low (documented) | `/boot` is effectively never encrypted on any distro. Non-`/boot` ext4 volumes that have encrypted subtrees fail with a clear `FsError::Unsupported("encrypted")` + trust-log event. |
| Fuzzing may find ext4-view bugs | Medium | Policy: report upstream, not patch locally. If critical, pin to pre-bug version. |
| `ext4-view` doesn't yet expose xattrs | Low | Not needed for v1.0. File as enhancement. |

---

## 14. Acceptance criteria

SDS-2 implementation is complete when all of the following are true:

- [ ] `ext4-view = "=0.9.3"` in `lamboot-core/Cargo.toml` with `default-features = false`.
- [ ] MSRV in `rust-toolchain.toml` ≥ 1.85 (ext4-view requirement).
- [ ] `lamboot-core/src/fs_backend_ext4.rs` exists, implements `FsBackend` for `Ext4Backend` + `FsStream` for `Ext4Stream`.
- [ ] `probe_ext4_superblock` function in `partitions.rs` recognizes ext4 volumes and feeds them into `Ext4Backend` construction.
- [ ] `Ext4Backend::tag()` returns `"ext4-view@0.9.3"`.
- [ ] All §10.1 unit tests pass.
- [ ] All §10.2 QEMU integration tests pass.
- [ ] §10.3 fleet regression passes against at least: Ubuntu 25.10, Debian 13, Fedora 43, EndeavourOS, Pop!_OS — the distros in the v0.8.3 fleet test.
- [ ] §10.4 fuzzing runs 10+ minutes with no panic.
- [ ] `lamboot-core/src/fs_backend_ext4.rs` has no `#[allow(clippy::…)]` annotations; any `#[expect]` has a documented reason.
- [ ] `LICENSE` top-level file has the bundled-crate-license note added per §4.1.
- [ ] `CHANGELOG.md` entry under Unreleased section describing SDS-2.
- [ ] Pre-commit hook (fmt + clippy + check) passes.
- [ ] Fleet-test VM 122 (Fedora Sway) — which failed v0.8.3 because LamBoot couldn't read ext4 /boot — now boots successfully through LamBoot via native ext4 reading. This is the release-validating acceptance test.

---

## 15. Sign-off

This spec is **implementation-ready** when:
- Founder has reviewed and explicitly acknowledged.
- SDS-1 (`SPEC-FS-BACKEND-TRAIT.md`) has landed AND been implemented (SDS-2 cannot merge before SDS-1's traits exist in code).
- No unresolved questions remain in §13 above "Medium" severity.

Implementation PR(s) must cite this SDS in their description and must not deviate from §2, §5, §7, §8 without a documented amendment to this file.

---

## Appendix A — Working code pattern from upstream

The ext4-view repository's `xtask/uefibench/src/main.rs` contains a working UEFI application that mounts an ext4 filesystem via `DiskIo`. LamBoot's `fs_backend_ext4.rs` is directly modeled on this pattern:

```rust
// Reference: ext4-view-rs/xtask/uefibench/src/main.rs
struct Disk {
    media_id: u32,
    io: ScopedProtocol<DiskIo>,
}

impl Ext4Read for Disk {
    fn read(&mut self, start_byte: u64, dst: &mut [u8])
        -> Result<(), Box<dyn Error + Send + Sync + 'static>>
    {
        Ok(self.io.read_disk(self.media_id, start_byte, dst).map_err(Box::new)?)
    }
}
```

This is the upstream-blessed integration pattern. LamBoot's `DiskIoReader` type is structurally identical; the difference is that `DiskIoReader` is owned by an `Ext4Backend` which is owned by a `Volume` which is enumerated by `partitions.rs` and dispatched through the `FsBackend` trait.

## Appendix B — References

- `docs/specs/SPEC-FS-BACKEND-TRAIT.md` — SDS-1, the parent spec.
- `docs/analysis/NATIVE-FS-AND-PE-LOADER-STRATEGY-2026-04-21.md` §5.1 — ext4-view adoption rationale.
- `docs/analysis/CONFIG-4-TRUST-CHAIN-GAP-2026-04-21-AMENDED.md` — why native ext4 reading matters (shim 15.8 ShimLock-uninstall issue).
- [`ext4-view` crate](https://crates.io/crates/ext4-view) v0.9.3
- [`ext4-view` GitHub](https://github.com/nicholasbishop/ext4-view-rs) — source inspection reference.
- [`ext4-view` `uefibench`](https://github.com/nicholasbishop/ext4-view-rs/tree/main/xtask/uefibench) — upstream-blessed UEFI integration pattern.
- [`ext4-view` `Ext4Read` trait](https://docs.rs/ext4-view/0.9.3/ext4_view/trait.Ext4Read.html) — the trait we adapt.
- [UEFI Spec §12.7 DiskIo Protocol](https://uefi.org/specs/UEFI/2.10/12_Protocols_Media_Access.html#disk-i-o-protocol) — the protocol our adapter wraps.
- ext4 disk format reference: [kernel.org ext4 wiki](https://ext4.wiki.kernel.org/index.php/Ext4_Disk_Layout)
