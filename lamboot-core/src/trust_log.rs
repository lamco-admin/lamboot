//! Trust-evidence log — UEFI-side wrapper.
//!
//! The schema, event builder, accumulator, stable vocabulary, and JSON-Lines
//! serializer all live in [`crate::trust_log_pure`] (no UEFI deps). This file
//! keeps only the piece that actually needs UEFI protocols: writing the
//! serialized log out to `\loader\boot-trust.log` on the ESP via `EspWriter`.
//!
//! Re-exports the pure items under `crate::trust_log::*` so existing call
//! sites keep working unchanged.
//!
//! SDS-4 PR-4: split pre-existing single-file module into pure + wrapper so
//! lamboot-fs-tests can cover event ordering, field schema, and stable-token
//! vocabulary under `cargo test`. The wrapper is exercised by the QEMU
//! harness.
//!
//! Record format:
//! ```json
//! {"seq":<n>,"event":"<type>","path":"<path>","size":<bytes>,
//!  "sha256":"<hex>","verified_via":"<source>","status":"<efi status>"
//!  [,"verifier_tag":"<tag>"][,"note":"<note>"]}
//! ```
//!
//! Write failures are silently tolerated so that trust recording never blocks
//! boot. The log is truncated on each boot.

// Canonical module boundary: consumers in the bootloader import from
// `crate::trust_log`, not `crate::trust_log_pure` (which is an impl
// detail split out purely so host tests can include it via `#[path]`).
// Re-export the full public surface; the lint allow covers items that
// are currently unused by the UEFI binary but must stay reachable via
// this path for future call sites and audit consistency.
#[expect(
    unused_imports,
    reason = "re-export is the stable module boundary even when the UEFI binary \
              doesn't currently reference every item"
)]
pub(crate) use crate::trust_log_pure::{
    serialize_events, TrustEvent, TrustLog, ALL_VERIFIED_VIA, V_DEGRADED_TRUST_SB_DIRECT,
    V_DEGRADED_TRUST_SB_OFF, V_FIRMWARE_DB_FALLBACK, V_FIRMWARE_LOADIMAGE, V_NATIVE_PE_LOADER,
    V_SHIM_MOK, V_SHIM_REJECTED,
};
use crate::{fs::Volume, fs_backend::PathBuf, fs_writer::EspWriter};

const LOG_PATH: &str = "/loader/boot-trust.log";

impl TrustLog {
    /// Flush all accumulated events to `\loader\boot-trust.log` on the ESP.
    /// Preserves all events recorded this boot across multiple flushes by
    /// keeping an internal `committed` list and rewriting the full cumulative
    /// log on each flush. (`EspWriter::write` overwrites; proper append via
    /// `FileProtocol::Write` at current EOF is a Path G v0.9.x task.)
    /// Best-effort: errors are logged but do not propagate.
    pub(crate) fn flush(&mut self, esp: &mut Volume) {
        if self.pending_events().is_empty() {
            return;
        }

        let buf = self.serialize_merged();
        let cumulative = self.committed_events().len();

        let Some(mut writer) = EspWriter::new(esp) else {
            log::warn!("trust-log write skipped: target volume is not FAT");
            return;
        };
        let Ok(path) = PathBuf::from_str(LOG_PATH) else {
            log::warn!("trust-log write skipped: log path failed canonicalization");
            return;
        };
        match writer.write(path.as_path(), buf.as_bytes()) {
            Ok(()) => log::debug!("trust-log flushed ({cumulative} events cumulative)"),
            Err(e) => log::warn!("trust-log write failed: {e}"),
        }
    }
}

// Re-export the pure serializer under the crate::trust_log module path
// so the one non-flush external consumer (host tests via #[path]) doesn't
// have to care about the split. The path-pure module is the authoritative
// source; this file adds only the UEFI wrapper.
#[cfg(doc)]
pub(crate) use serialize_events as _serialize_events_for_docs;
