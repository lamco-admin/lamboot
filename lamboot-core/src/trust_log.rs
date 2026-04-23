//! Trust-evidence log (v0.8.3 initial version).
//!
//! Writes structured JSON-lines records to `\loader\boot-trust.log` on the ESP
//! documenting every image-authentication decision LamBoot made during the
//! current boot. This is LamBoot's first-pass differentiator against GRUB,
//! systemd-boot, and rEFInd — none of which surface trust decisions to the
//! user or to host-side audit tools. Path G (v0.9.x) will extend this with
//! crypto-signed entries, policy records, and TPM event correlation.
//!
//! Record format (one line per record, UTF-8):
//! ```json
//! {"t":"<UTC-ish tick>","event":"<type>","path":"<path or \"\">",
//!  "size":<bytes>,"sha256":"<hex>","verified_via":"<source>",
//!  "status":"<efi status name>","note":"<optional>"}
//! ```
//!
//! `verified_via` values:
//!   - `firmware_db`     — firmware's native security protocol accepted
//!   - `shim_mok`        — shim ShimLock::Verify accepted (MOK-enrolled cert)
//!   - `shim_vendor`     — shim ShimLock::Verify accepted via embedded vendor cert
//!   - `security_override` — our SecurityOverride wrapper delegated firmware→success
//!   - `rejected`        — no trust source accepted
//!   - `sb_disabled`     — Secure Boot off, trust not evaluated
//!
//! The log is best-effort: write failures are silently tolerated so that trust
//! recording never blocks boot. The log is truncated on each boot.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::{fs::Volume, fs_backend::PathBuf, fs_writer::EspWriter};

const LOG_PATH: &str = "/loader/boot-trust.log";

/// One evidence record, built up as LamBoot progresses and flushed at key points.
#[derive(Debug, Clone)]
pub(crate) struct TrustEvent {
    pub event: &'static str,
    pub path: String,
    pub size: usize,
    pub sha256_hex: String,
    pub verified_via: &'static str,
    pub status: String,
    pub note: String,
}

impl TrustEvent {
    pub(crate) fn new(event: &'static str) -> Self {
        Self {
            event,
            path: String::new(),
            size: 0,
            sha256_hex: String::new(),
            verified_via: "",
            status: String::new(),
            note: String::new(),
        }
    }

    pub(crate) fn with_path(mut self, p: &str) -> Self {
        self.path = p.to_string();
        self
    }
    #[expect(
        dead_code,
        reason = "v0.8.3 does not record image sizes; Path G (v0.9.x) will populate via PE inspection"
    )]
    pub(crate) fn with_size(mut self, s: usize) -> Self {
        self.size = s;
        self
    }
    pub(crate) fn with_verified_via(mut self, v: &'static str) -> Self {
        self.verified_via = v;
        self
    }
    pub(crate) fn with_status(mut self, s: impl core::fmt::Debug) -> Self {
        self.status = format!("{s:?}");
        self
    }
    pub(crate) fn with_note(mut self, n: &str) -> Self {
        self.note = n.to_string();
        self
    }
    #[expect(
        dead_code,
        reason = "v0.8.3 does not capture per-image hashes; Path G (v0.9.x) will add SHA-256 digests per verification"
    )]
    pub(crate) fn with_sha256(mut self, hex: &str) -> Self {
        self.sha256_hex = hex.to_string();
        self
    }
}

/// Accumulator for a boot's worth of trust events. Flushed to the log at
/// key points (after driver loads, before kernel handoff, etc.) so partial
/// state is captured even if we crash later.
pub(crate) struct TrustLog {
    events: Vec<TrustEvent>,
    /// All events ever recorded this boot, preserved across flushes so that
    /// multiple flushes produce a cumulative on-disk log rather than each
    /// flush overwriting the last. Cleared only when a new boot begins.
    committed: Vec<TrustEvent>,
    boot_seq: u64,
}

impl TrustLog {
    pub(crate) fn new() -> Self {
        Self {
            events: Vec::new(),
            committed: Vec::new(),
            boot_seq: 0,
        }
    }

    pub(crate) fn record(&mut self, ev: TrustEvent) {
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

    /// Flush all accumulated events to `\loader\boot-trust.log` on the ESP.
    /// Preserves all events recorded this boot across multiple flushes by
    /// keeping an internal `committed` list and rewriting the full cumulative
    /// log on each flush. (write_file overwrites; proper append via
    /// FileProtocol::Write at current EOF is a Path G v0.9.x task.)
    /// Best-effort: errors are logged but do not propagate.
    pub(crate) fn flush(&mut self, esp: &mut Volume) {
        if self.events.is_empty() {
            return;
        }

        // Merge newly-recorded events into the cumulative list for this boot.
        self.committed.append(&mut self.events);

        let mut buf = String::new();
        for (i, ev) in self.committed.iter().enumerate() {
            buf.push('{');
            push_kv(
                &mut buf,
                "seq",
                &format!("{}", i as u64 + self.boot_seq),
                false,
            );
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
            if !ev.note.is_empty() {
                buf.push(',');
                push_kv(&mut buf, "note", &ev.note, true);
            }
            buf.push_str("}\n");
        }

        let Some(mut writer) = EspWriter::new(esp) else {
            log::warn!("trust-log write skipped: target volume is not FAT");
            return;
        };
        let Ok(path) = PathBuf::from_str(LOG_PATH) else {
            log::warn!("trust-log write skipped: log path failed canonicalization");
            return;
        };
        match writer.write(path.as_path(), buf.as_bytes()) {
            Ok(()) => log::debug!(
                "trust-log flushed ({} events cumulative)",
                self.committed.len()
            ),
            Err(e) => log::warn!("trust-log write failed: {e}"),
        }
    }
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
