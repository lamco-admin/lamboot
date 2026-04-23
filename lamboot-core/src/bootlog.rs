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

use crate::{fs::Volume, fs_backend::PathBuf, fs_writer::EspWriter};

const LOG_PATH: &str = "/EFI/LamBoot/reports/boot.log";
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
            write_through: true,
            written: 0,
        }
    }

    pub(crate) fn log(&mut self, esp: Option<&mut Volume>, level: &str, message: &str) {
        let timestamp = get_timestamp();
        let line = format!("[{timestamp}] {level}: {message}\n");

        if self.write_through {
            if let Some(esp) = esp {
                self.append_to_esp(esp, &line);
            }
        }

        self.buffer.push_str(&line);
    }

    pub(crate) fn info(&mut self, esp: Option<&mut Volume>, message: &str) {
        self.log(esp, "INFO", message);
    }

    pub(crate) fn warn(&mut self, esp: Option<&mut Volume>, message: &str) {
        self.log(esp, "WARN", message);
    }

    pub(crate) fn set_buffered(&mut self) {
        self.write_through = false;
    }

    pub(crate) fn flush(&mut self, esp: &mut Volume) {
        if self.buffer.is_empty() {
            return;
        }
        let Some(mut writer) = EspWriter::new(esp) else {
            log::warn!("bootlog flush skipped: target volume is not FAT");
            return;
        };
        let Ok(path) = PathBuf::from_str(LOG_PATH) else {
            return;
        };
        if let Err(e) = writer.write(path.as_path(), self.buffer.as_bytes()) {
            log::warn!("Failed to write boot log: {e}");
        }
    }

    pub(crate) fn start(&mut self, esp: &mut Volume, version: &str, arch: &str) {
        let timestamp = get_timestamp();
        let header = format!(
            "=== LamBoot {version} ({arch}) boot log ===\n\
             === {timestamp} ===\n\n"
        );
        self.buffer.clone_from(&header);
        self.written = 0;

        let Some(mut writer) = EspWriter::new(esp) else {
            log::warn!("bootlog start skipped: target volume is not FAT");
            return;
        };
        let Ok(path) = PathBuf::from_str(LOG_PATH) else {
            return;
        };
        if let Err(e) = writer.write(path.as_path(), header.as_bytes()) {
            log::warn!("Failed to initialize boot log: {e}");
            return;
        }
        self.written = header.len();
    }

    fn append_to_esp(&mut self, esp: &mut Volume, line: &str) {
        if self.written + line.len() > MAX_LOG_SIZE {
            return;
        }
        let Some(mut writer) = EspWriter::new(esp) else {
            return;
        };
        let Ok(path) = PathBuf::from_str(LOG_PATH) else {
            return;
        };
        if writer.append(path.as_path(), line.as_bytes()).is_ok() {
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
