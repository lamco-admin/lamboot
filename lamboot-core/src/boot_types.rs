//! Layer: 3 (types) — shared boot-entry and preflight data types.
//!
//! Pure data types with no I/O and no firmware calls, shared across the
//! discovery, preflight, presentation, and boot layers. Extracted from
//! `discovery.rs` and `preflight.rs` so that lower layers (`partitions`,
//! `report`) and the presentation layer (`console`, `gui`) can name a
//! `BootEntry` without importing the Layer-7 aggregation logic — the type
//! definitions were the only thing those modules needed, and depending
//! upward on `discovery` for them violated the layer contract.
//!
//! Layer rationale: these types sit at the parser tier (3) because they are
//! the structured output of discovery/parsing — bytes-in, struct-out — and
//! carry no behavior beyond pure constructors. Every higher layer may name
//! them; this module names nothing above `fs_types` (Layer 2).

use alloc::{string::String, vec::Vec};

use crate::fs_types::BackendTag;

/// A bootable entry discovered on some volume — the unit the menu presents
/// and the boot layer consumes.
#[derive(Debug, Clone)]
pub(crate) struct BootEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub icon: Icon,
    /// BLS source info for boot counting (None for non-BLS entries)
    pub bls_filename: Option<String>,
    /// Preflight validation results (None if not yet run)
    pub preflight: Option<PreflightResult>,
    /// SDS-5: back-reference into the `volumes` slice passed to
    /// `discover_all_entries`. Consumed by the boot layer (SDS-3+) to
    /// pick the correct `Volume` as byte source for the kernel read.
    /// For pre-SDS-3 boot paths that still read from ESP, a non-zero
    /// value surfaces as a "kernel not on ESP" preflight warning.
    pub source_volume_index: usize,
    /// SDS-5: backend identifier of the volume this entry came from,
    /// used for trust-log annotation + menu UI grouping.
    pub source_backend_tag: BackendTag,
}

/// What kind of thing a `BootEntry` boots, with its kind-specific payload.
#[derive(Debug, Clone)]
pub(crate) enum EntryKind {
    Chainload {
        path: String,
    },
    Uki {
        path: String,
        options: String,
    },
    LinuxLegacy {
        kernel_path: String,
        initrd_paths: Vec<String>,
        options: String,
    },
    /// Boot an OS from an ISO9660 medium (SPEC-BOOT-FROM-ISO). The source —
    /// either an `.iso` file on a mounted volume or a physical optical drive —
    /// is carried by [`IsoSource`]; the loopback/optical mount + Path-A
    /// resolution happen at boot time.
    Iso {
        source: IsoSource,
    },
}

/// Where the ISO9660 bytes for an [`EntryKind::Iso`] come from.
#[derive(Debug, Clone)]
pub(crate) enum IsoSource {
    /// An `.iso` file on the volume named by `BootEntry::source_volume_index`.
    /// Booted via `FileBlockSource` (Path A1 `loopback.cfg`, else A2 file form).
    File { iso_path: String },
    /// A physical optical drive — the `BlockIO` handle *is* the byte source.
    /// `handle` is an opaque firmware token (Copy, valid for the firmware's
    /// boot-services lifetime), not a firmware call, so it is layer-legal here.
    /// `label` is the ISO9660 PVD volume label, read once at discovery and used
    /// to render the Path-A2 direct-media command line (`CDLABEL=`/`archisolabel=`).
    Optical { handle: uefi::Handle, label: String },
}

/// Menu icon hint for a `BootEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Windows,
    Linux,
    Efi,
    #[expect(dead_code, reason = "used when recovery partition discovery is added")]
    Recovery,
    #[expect(dead_code, reason = "used when network boot support is added")]
    Network,
    Tools,
}

/// Aggregate preflight result for a boot entry. Advisory only — preflight
/// never blocks boot; the result annotates the menu entry.
#[derive(Debug, Clone)]
pub(crate) struct PreflightResult {
    pub status: PreflightStatus,
    pub checks: Vec<CheckResult>,
}

impl PreflightResult {
    pub(crate) fn from_checks(checks: Vec<CheckResult>) -> Self {
        let status = if checks.iter().any(|c| c.severity == Severity::Error) {
            PreflightStatus::Error
        } else if checks.iter().any(|c| c.severity == Severity::Warning) {
            PreflightStatus::Warning
        } else {
            PreflightStatus::Ok
        };
        Self { status, checks }
    }

    pub(crate) fn first_issue(&self) -> Option<&CheckResult> {
        self.checks.iter().find(|c| c.severity != Severity::Ok)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PreflightStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct CheckResult {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Ok,
    Warning,
    Error,
}
