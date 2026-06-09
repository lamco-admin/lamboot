//! Layer: 5 (cross-cutting) — Trust & Audit.
//!
//! Gated diagnostic audit-log writes.
//!
//! Used by the post-selection boot path (boot.rs, pe_loader.rs,
//! pe_loader_pure.rs, initrd.rs, main.rs) to drop breadcrumb markers
//! at each major phase. Every breadcrumb is a `report::append_audit_log`
//! write to `/EFI/LamBoot/reports/audit.log` on the ESP — but ONLY when
//! the `DIAG_ENABLED` flag is set, which is wired to
//! `policy.diagnostics_verbose` at boot init.
//!
//! Default: OFF.
//! Enabled: when operators set `[diagnostics] verbose = true` in
//! `/EFI/LamBoot/policy.toml` to investigate a boot freeze.
//!
//! ## Coalescing on conout_fat_coupling firmware
//!
//! On firmware where a ConOut write stalls the next FAT op (ASUS G10AJ
//! class — see firmware_quirks::conout_fat_coupling), even the
//! `true_append` per-breadcrumb writes are FAT ops in the post-selection
//! band and risk coupling. When `DIAG_COALESCE` is set (wired to
//! conout_fat_coupling), breadcrumbs accumulate in a RAM buffer instead of
//! writing per-call, and the whole buffer is emitted in a SINGLE
//! `true_append` at each pre-handoff `flush()` checkpoint. This keeps
//! verbose diagnostics safe to enable on coupling-prone firmware: enabling
//! them can no longer contribute per-call FAT-write churn to the band. The
//! trade-off is that a hang BEFORE the pre-handoff flush loses the in-RAM
//! trail — but a coalesced breadcrumb cannot itself be the cause of such a
//! hang.
//!
//! Cost when disabled: one atomic load per call site. Negligible.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{fs::Volume, report};

static DIAG_ENABLED: AtomicBool = AtomicBool::new(false);
static DIAG_COALESCE: AtomicBool = AtomicBool::new(false);

/// Bound on the in-RAM breadcrumb accumulator (coalesce mode). Once full we
/// stop appending: the earliest trail shows how far the boot reached, and a
/// bounded buffer means a single bounded write at flush time.
const DIAG_BUF_CAP: usize = 32 * 1024;

struct DiagBuf(UnsafeCell<Option<alloc::string::String>>);

// SAFETY: UEFI execution before ExitBootServices is single-threaded and
// non-reentrant — the boot path is the only accessor of this buffer, and
// there is no interrupt/async context that could observe a torn state.
unsafe impl Sync for DiagBuf {}

static DIAG_BUF: DiagBuf = DiagBuf(UnsafeCell::new(None));

pub(crate) fn set_enabled(v: bool) {
    DIAG_ENABLED.store(v, Ordering::Relaxed);
}

/// Route breadcrumbs through the RAM buffer + single pre-handoff flush
/// instead of per-call FAT writes. Wired to
/// `firmware_quirks.conout_fat_coupling` at boot init.
pub(crate) fn set_coalesce(v: bool) {
    DIAG_COALESCE.store(v, Ordering::Relaxed);
}

#[inline]
pub(crate) fn append(esp: &mut Volume, msg: &str) {
    if !DIAG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if DIAG_COALESCE.load(Ordering::Relaxed) {
        buffer_push(msg);
    } else {
        let _ = report::append_audit_log(esp, msg);
    }
}

#[inline]
pub(crate) fn append_fmt(esp: &mut Volume, args: core::fmt::Arguments<'_>) {
    if !DIAG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let s = alloc::format!("{args}");
    if DIAG_COALESCE.load(Ordering::Relaxed) {
        buffer_push(&s);
    } else {
        let _ = report::append_audit_log(esp, &s);
    }
}

fn buffer_push(msg: &str) {
    // SAFETY: single-threaded pre-ExitBootServices boot context; see DiagBuf.
    let slot = unsafe { &mut *DIAG_BUF.0.get() };
    let buf = slot.get_or_insert_with(alloc::string::String::new);
    if buf.len() + msg.len() <= DIAG_BUF_CAP {
        buf.push_str(msg);
    }
}

/// Emit the coalesced buffer in a SINGLE `true_append` write, then clear it.
/// No-op when not coalescing, or when the buffer is empty. Called at safe
/// pre-handoff checkpoints (before each `start_image`) so verbose diagnostics
/// produce at most one FAT write per boot on coupling-prone firmware.
pub(crate) fn flush(esp: &mut Volume) {
    if !DIAG_COALESCE.load(Ordering::Relaxed) {
        return;
    }
    // SAFETY: single-threaded pre-ExitBootServices boot context; see DiagBuf.
    let slot = unsafe { &mut *DIAG_BUF.0.get() };
    if let Some(buf) = slot.as_mut() {
        if !buf.is_empty() {
            let _ = report::append_audit_log(esp, buf);
            buf.clear();
        }
    }
}
