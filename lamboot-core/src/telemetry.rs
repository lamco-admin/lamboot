//! Boot telemetry — per-phase timing measurements.
//!
//! Measures wall-clock time for each boot phase using the UEFI RTC.
//! Stored as a UEFI variable for host-side monitoring and included
//! in the boot report JSON.

use alloc::{format, string::String, vec::Vec};

/// A single phase timing measurement
struct PhaseTiming {
    name: &'static str,
    ms: u32,
}

/// Boot telemetry collector
pub(crate) struct BootTelemetry {
    phases: Vec<PhaseTiming>,
    boot_start: u32,
}

impl BootTelemetry {
    pub(crate) fn new() -> Self {
        Self {
            phases: Vec::new(),
            boot_start: timestamp_ms(),
        }
    }

    /// Record the duration of a phase.
    pub(crate) fn record(&mut self, name: &'static str, start_ms: u32) {
        let elapsed = timestamp_ms().wrapping_sub(start_ms);
        self.phases.push(PhaseTiming { name, ms: elapsed });
    }

    /// Total boot time from construction to now.
    pub(crate) fn total_ms(&self) -> u32 {
        timestamp_ms().wrapping_sub(self.boot_start)
    }

    /// Format as JSON fragment for inclusion in boot report.
    pub(crate) fn to_json(&self) -> String {
        let mut parts = Vec::new();
        for p in &self.phases {
            parts.push(format!("\"{}\":{}", p.name, p.ms));
        }
        parts.push(format!("\"total\":{}", self.total_ms()));
        format!("{{{}}}", parts.join(","))
    }

    /// Store telemetry as a UEFI NVRAM variable for host-side reading.
    pub(crate) fn store_variable(&self) {
        // Compact binary: [phase_count: u8] [name_len: u8, name: bytes, ms: u32_le]...
        let mut data = Vec::new();
        let count = self.phases.len().min(255) as u8;
        data.push(count);

        for p in self.phases.iter().take(count as usize) {
            let name_bytes = p.name.as_bytes();
            let name_len = name_bytes.len().min(31) as u8;
            data.push(name_len);
            data.extend_from_slice(&name_bytes[..name_len as usize]);
            data.extend_from_slice(&p.ms.to_le_bytes());
        }
        // Append total
        data.extend_from_slice(&self.total_ms().to_le_bytes());

        let guid = uefi::guid!("4c414d42-4f4f-5400-0000-000000000001");
        let vendor = uefi::runtime::VariableVendor(guid);
        let attrs = uefi::runtime::VariableAttributes::BOOTSERVICE_ACCESS
            | uefi::runtime::VariableAttributes::RUNTIME_ACCESS;

        let _ =
            uefi::runtime::set_variable(uefi::cstr16!("LamBootTelemetry"), &vendor, attrs, &data);
    }
}

/// Get current time in milliseconds from UEFI RTC.
/// Returns a monotonically-ish value suitable for duration measurement.
pub(crate) fn timestamp_ms() -> u32 {
    match uefi::runtime::get_time() {
        Ok(time) => {
            // Convert to ms since midnight for sub-second comparison
            let h = time.hour() as u32;
            let m = time.minute() as u32;
            let s = time.second() as u32;
            let ns = time.nanosecond();
            h * 3_600_000 + m * 60_000 + s * 1_000 + ns / 1_000_000
        }
        Err(_) => 0,
    }
}
