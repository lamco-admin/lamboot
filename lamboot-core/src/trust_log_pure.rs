//! Pure (no UEFI) portions of the trust-log subsystem.
//!
//! SDS-4 PR-4: split out so lamboot-fs-tests can include this file
//! verbatim via `#[path]` and run host unit tests against the event
//! schema, the accumulator, and the JSON-Lines serializer. The
//! Volume-dependent wrapper (flush-to-ESP) lives in `trust_log.rs`
//! next door and is exercised only by the QEMU harness.
//!
//! Record format — one JSON object per line, UTF-8, with fields emitted
//! in this stable order:
//!
//! ```text
//! seq, event, path, size, sha256, verified_via, status[, verifier_tag][, note]
//! ```
//!
//! `verifier_tag` and `note` are elided when empty to keep the log
//! compact; all other fields always appear. Field order is part of the
//! schema — consumers doing line-based diffs against reference logs
//! rely on it.
//!
//! The re-hosted pub items intentionally use `pub` (not `pub(crate)`)
//! so external integration tests can import them. Under `lamboot-core`
//! the same items are semantically crate-private; `#![allow(unreachable_pub)]`
//! silences the lint in both consumer views.

#![allow(
    dead_code,
    reason = "reachability differs between UEFI bootloader and host test crate"
)]
#![allow(
    unreachable_pub,
    reason = "host tests need pub; lamboot-core treats items as crate-internal"
)]

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

// ---------------------------------------------------------------------------
// SDS-4 §6.3 stable `verified_via` vocabulary
//
// Consumers of the trust log MAY match on any of these tokens. Adding
// a new token is semver-additive; renaming or removing one is a major-
// version break. Prefer these constants over string literals to keep
// the vocabulary DRY at the call sites.
// ---------------------------------------------------------------------------

/// ShimLock::Verify accepted via MOK-enrolled certificate. Normal
/// distro boot under shim+MOK.
pub const V_SHIM_MOK: &str = "shim_mok";
/// Firmware's EFI_SECURITY2_ARCH_PROTOCOL fallback accepted (e.g. LamBoot
/// loaded directly from firmware DB without shim in the chain).
pub const V_FIRMWARE_DB_FALLBACK: &str = "firmware_db_fallback";
/// Secure Boot is disabled. LamBoot attempted no verification.
pub const V_DEGRADED_TRUST_SB_OFF: &str = "degraded_trust_sb_off";
/// SDS-4 v1.1 addition — LamBoot loaded directly by firmware DB
/// (no shim in chain), SB on. Same trust model as SB-off from
/// LamBoot's perspective (admin's db key covers the tree).
pub const V_DEGRADED_TRUST_SB_DIRECT: &str = "degraded_trust_sb_direct";
/// ShimLock::Verify rejected an image. Catch-all reason for the
/// rejection lands in the `note` field.
pub const V_SHIM_REJECTED: &str = "shim_rejected";
/// Native PE loader produced the image. Used on `image_loaded_native`
/// when the `verified_via` upstream is lost or not applicable.
pub const V_NATIVE_PE_LOADER: &str = "native_pe_loader";
/// Firmware's LoadImage path produced the image. Used on
/// `image_loaded_firmware` as the audit flag distinguishing the
/// v0.8.3 path from SDS-3's native path.
pub const V_FIRMWARE_LOADIMAGE: &str = "firmware_loadimage";

/// Full list of stable `verified_via` tokens. Host tests assert any
/// event emitted by the live boot flow carries a value from this set.
pub const ALL_VERIFIED_VIA: &[&str] = &[
    V_SHIM_MOK,
    V_FIRMWARE_DB_FALLBACK,
    V_DEGRADED_TRUST_SB_OFF,
    V_DEGRADED_TRUST_SB_DIRECT,
    V_SHIM_REJECTED,
    V_NATIVE_PE_LOADER,
    V_FIRMWARE_LOADIMAGE,
];

/// One evidence record, built up as LamBoot progresses and flushed at key points.
#[derive(Debug, Clone)]
pub struct TrustEvent {
    pub event: &'static str,
    pub path: String,
    pub size: usize,
    pub sha256_hex: String,
    pub verified_via: &'static str,
    pub status: String,
    pub note: String,
    /// SDS-4: shim version string or other verifier identity token,
    /// surfaces in `image_verified` and `image_loaded_native` so
    /// audit consumers can correlate decisions to specific verifier
    /// releases. Best-effort; `""` when unavailable.
    pub verifier_tag: String,
}

impl TrustEvent {
    pub fn new(event: &'static str) -> Self {
        Self {
            event,
            path: String::new(),
            size: 0,
            sha256_hex: String::new(),
            verified_via: "",
            status: String::new(),
            note: String::new(),
            verifier_tag: String::new(),
        }
    }

    /// SDS-4: attach a verifier-identity string (e.g. `"shim-15.8"`).
    /// Surfaces in `image_verified` + `image_loaded_native` for audit
    /// correlation to specific verifier releases.
    pub fn with_verifier_tag(mut self, tag: &str) -> Self {
        self.verifier_tag = tag.to_string();
        self
    }

    pub fn with_path(mut self, p: &str) -> Self {
        self.path = p.to_string();
        self
    }
    pub fn with_size(mut self, s: usize) -> Self {
        self.size = s;
        self
    }
    pub fn with_verified_via(mut self, v: &'static str) -> Self {
        self.verified_via = v;
        self
    }
    pub fn with_status(mut self, s: impl core::fmt::Debug) -> Self {
        self.status = format!("{s:?}");
        self
    }
    pub fn with_note(mut self, n: &str) -> Self {
        self.note = n.to_string();
        self
    }
    /// SDS-4: SHA-256 hex digest of the image bytes. Consumed in
    /// `image_verified` (Step 10) and re-emitted in `image_loaded_native`
    /// (Step 13) so the §6.4 invariant check is auditable from the log.
    pub fn with_sha256(mut self, hex: &str) -> Self {
        self.sha256_hex = hex.to_string();
        self
    }
}

/// Accumulator for a boot's worth of trust events. Flushed to the log at
/// key points (after driver loads, before kernel handoff, etc.) so partial
/// state is captured even if we crash later.
pub struct TrustLog {
    /// Events recorded since the last flush. Drained into `committed`
    /// each flush; callers that only want the pending tail should use
    /// [`TrustLog::pending_events`].
    events: Vec<TrustEvent>,
    /// All events ever recorded this boot, preserved across flushes so that
    /// multiple flushes produce a cumulative on-disk log rather than each
    /// flush overwriting the last. Cleared only when a new boot begins.
    committed: Vec<TrustEvent>,
    boot_seq: u64,
}

impl TrustLog {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            committed: Vec::new(),
            boot_seq: 0,
        }
    }

    pub fn record(&mut self, ev: TrustEvent) {
        log::info!(
            "trust: {}{} verified_via={} status={}",
            ev.event,
            if ev.path.is_empty() {
                String::new()
            } else {
                format!(" path={}", ev.path)
            },
            ev.verified_via,
            ev.status,
        );
        self.events.push(ev);
    }

    /// Merge pending events into the committed list and return the JSON-Lines
    /// serialization of the cumulative log. Pure — no I/O. The UEFI-side
    /// `flush()` wrapper in `trust_log.rs` invokes this and writes the
    /// returned buffer via `EspWriter`.
    pub fn serialize_merged(&mut self) -> String {
        self.committed.append(&mut self.events);
        serialize_events(&self.committed, self.boot_seq)
    }

    /// Read-only view of all events committed so far this boot. Host-test
    /// seam for asserting accumulation across multiple flush boundaries.
    pub fn committed_events(&self) -> &[TrustEvent] {
        &self.committed
    }

    /// Read-only view of events recorded since the last flush. Same intent
    /// as above for the pending tail.
    pub fn pending_events(&self) -> &[TrustEvent] {
        &self.events
    }
}

impl Default for TrustLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure JSON-Lines serializer for a slice of trust events. Emits fields
/// in the stable order documented at the module level. `boot_seq` is the
/// sequence offset — SDS-4 sets this to 0 for the first (and, in v0.9.x,
/// only) boot within a run; reserved for future multi-boot concatenation
/// if we ever rotate logs without truncating.
pub fn serialize_events(committed: &[TrustEvent], boot_seq: u64) -> String {
    let mut buf = String::new();
    for (i, ev) in committed.iter().enumerate() {
        buf.push('{');
        push_kv(&mut buf, "seq", &format!("{}", i as u64 + boot_seq), false);
        buf.push(',');
        push_kv(&mut buf, "event", ev.event, true);
        buf.push(',');
        push_kv(&mut buf, "path", &ev.path, true);
        buf.push(',');
        push_kv(&mut buf, "size", &format!("{}", ev.size), false);
        buf.push(',');
        push_kv(&mut buf, "sha256", &ev.sha256_hex, true);
        buf.push(',');
        push_kv(&mut buf, "verified_via", ev.verified_via, true);
        buf.push(',');
        push_kv(&mut buf, "status", &ev.status, true);
        // SDS-4: optional fields. Omit when empty so the log
        // stays compact on events that don't carry this data.
        if !ev.verifier_tag.is_empty() {
            buf.push(',');
            push_kv(&mut buf, "verifier_tag", &ev.verifier_tag, true);
        }
        if !ev.note.is_empty() {
            buf.push(',');
            push_kv(&mut buf, "note", &ev.note, true);
        }
        buf.push_str("}\n");
    }
    buf
}

fn push_kv(buf: &mut String, key: &str, val: &str, quote: bool) {
    buf.push('"');
    buf.push_str(key);
    buf.push_str("\":");
    if quote {
        buf.push('"');
        for c in val.chars() {
            match c {
                '"' => buf.push_str("\\\""),
                '\\' => buf.push_str("\\\\"),
                '\n' => buf.push_str("\\n"),
                '\r' => buf.push_str("\\r"),
                '\t' => buf.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    use core::fmt::Write;
                    let _ = write!(buf, "\\u{:04x}", c as u32);
                }
                c => buf.push(c),
            }
        }
        buf.push('"');
    } else {
        buf.push_str(val);
    }
}
