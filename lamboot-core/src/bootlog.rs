//! Persistent boot log — writes boot trace to ESP for post-boot retrieval.
//!
//! Two modes:
//!   - **Write-through** (early boot): each message is appended to the ESP file
//!     immediately, surviving crashes. Used during init, driver loading, and discovery.
//!   - **Buffered** (menu phase): messages accumulate in memory and flush on demand.
//!     Reduces I/O during the interactive menu where crashes are unlikely.
//!
//! After boot: `cat /boot/efi/EFI/LamBoot/reports/boot.log`

use alloc::{format, string::String};

use crate::fs::EspVolume;

const LOG_PATH: &str = "\\EFI\\LamBoot\\reports\\boot.log";
const MAX_LOG_SIZE: usize = 64 * 1024; // 64 KB cap

pub(crate) struct BootLog {
    buffer: String,
    write_through: bool,
    /// Accumulated size written to ESP in this boot (for size cap)
    written: usize,
}

impl BootLog {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
            write_through: true, // start in write-through mode
            written: 0,
        }
    }

    /// Log a message. In write-through mode, also writes to ESP immediately.
    pub(crate) fn log(&mut self, esp: Option<&mut EspVolume>, level: &str, message: &str) {
        let timestamp = get_timestamp();
        let line = format!("[{timestamp}] {level}: {message}\n");

        if self.write_through {
            if let Some(esp) = esp {
                self.append_to_esp(esp, &line);
            }
        }

        self.buffer.push_str(&line);
    }

    /// Log an info-level message
    pub(crate) fn info(&mut self, esp: Option<&mut EspVolume>, message: &str) {
        self.log(esp, "INFO", message);
    }

    /// Log a warning-level message
    pub(crate) fn warn(&mut self, esp: Option<&mut EspVolume>, message: &str) {
        self.log(esp, "WARN", message);
    }

    /// Switch to buffered mode (call when entering the menu)
    pub(crate) fn set_buffered(&mut self) {
        self.write_through = false;
    }

    /// Flush any buffered content to ESP (call before booting)
    pub(crate) fn flush(&mut self, esp: &mut EspVolume) {
        if !self.buffer.is_empty() {
            // In buffered mode, the buffer has content not yet on disk.
            // In write-through mode, everything is already on disk,
            // but we write the full buffer anyway to ensure completeness.
            esp.write_file(LOG_PATH, self.buffer.as_bytes())
                .unwrap_or_else(|_| log::warn!("Failed to write boot log"));
        }
    }

    /// Start a new boot log (truncate the previous one)
    pub(crate) fn start(&mut self, esp: &mut EspVolume, version: &str, arch: &str) {
        let timestamp = get_timestamp();
        let header = format!(
            "=== LamBoot {version} ({arch}) boot log ===\n\
             === {timestamp} ===\n\n"
        );
        self.buffer.clone_from(&header);
        self.written = 0;
        // Truncate and write header
        esp.write_file(LOG_PATH, header.as_bytes())
            .unwrap_or_else(|_| log::warn!("Failed to initialize boot log"));
        self.written = header.len();
    }

    fn append_to_esp(&mut self, esp: &mut EspVolume, line: &str) {
        if self.written + line.len() > MAX_LOG_SIZE {
            return; // size cap reached
        }

        // Read existing content, append, write back
        let mut content = esp.read_to_string(LOG_PATH).unwrap_or_default();
        content.push_str(line);
        if esp.write_file(LOG_PATH, content.as_bytes()).is_ok() {
            self.written += line.len();
        }
    }
}

fn get_timestamp() -> String {
    match uefi::runtime::get_time() {
        Ok(time) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            time.year(),
            time.month(),
            time.day(),
            time.hour(),
            time.minute(),
            time.second(),
        ),
        Err(_) => String::from("??:??:??"),
    }
}
