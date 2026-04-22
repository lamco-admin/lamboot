//! Serial/text console fallback for environments without GOP (graphics).
//!
//! When GraphicsOutput protocol is unavailable (serial console, SOL, headless VMs),
//! falls back to UEFI SimpleTextOutput for a text-mode boot menu.

use alloc::format;

use uefi::{prelude::*, proto::console::gop::GraphicsOutput, Result};

use crate::{
    discovery::{BootEntry, Icon},
    health,
    input::{InputEvent, InputManager, Key},
    policy::Policy,
};

/// Check if graphics mode is available
pub(crate) fn has_graphics() -> bool {
    uefi::boot::get_handle_for_protocol::<GraphicsOutput>().is_ok()
}

/// Text-mode boot menu for serial console environments
pub(crate) struct TextMenu {
    selected_index: usize,
    input: InputManager,
}

impl TextMenu {
    pub(crate) fn new() -> Self {
        // Use a default resolution for input — doesn't matter for text mode
        let input = InputManager::new(80, 25);
        Self {
            selected_index: 0,
            input,
        }
    }

    /// Run the text-mode boot menu
    pub(crate) fn run_menu(&mut self, entries: &[BootEntry], policy: &Policy) -> Result<BootEntry> {
        let total_selectable = entries.len() + 2; // +2 for system actions

        let has_bootable = entries.iter().any(|e| e.icon != Icon::Tools);
        let timeout_ms = if has_bootable {
            policy.default_timeout_ms
        } else {
            0
        };
        let mut user_interacted = false;
        let mut elapsed_frames = 0u32;

        self.draw_menu(entries, None);

        loop {
            if !user_interacted && timeout_ms > 0 {
                let elapsed_ms = elapsed_frames * 100;
                if elapsed_ms >= timeout_ms && has_bootable {
                    if let Some(ref default_id) = policy.default_entry {
                        if let Some(idx) = entries.iter().position(|e| &e.id == default_id) {
                            self.selected_index = idx;
                        }
                    }
                    if self.selected_index < entries.len() {
                        println_console(&format!(
                            "\nAuto-booting: {}",
                            entries[self.selected_index].name
                        ));
                        return Ok(entries[self.selected_index].clone());
                    }
                }

                let remaining = (timeout_ms - elapsed_ms.min(timeout_ms)) / 1000;
                if elapsed_frames.is_multiple_of(10) {
                    self.draw_menu(entries, Some(remaining));
                }
            }

            match self.input.poll() {
                InputEvent::KeyPress(key) => {
                    user_interacted = true;
                    match key {
                        Key::Up => {
                            if self.selected_index > 0 {
                                self.selected_index -= 1;
                            }
                            self.draw_menu(entries, None);
                        }
                        Key::Down => {
                            if self.selected_index < total_selectable - 1 {
                                self.selected_index += 1;
                            }
                            self.draw_menu(entries, None);
                        }
                        Key::Enter | Key::Char('\r') => {
                            if self.selected_index < entries.len() {
                                return Ok(entries[self.selected_index].clone());
                            }
                            // System action
                            let action = self.selected_index - entries.len();
                            if action == 0 {
                                println_console("\nRebooting to firmware setup...");
                                health::reboot_to_firmware_setup();
                            } else {
                                println_console("\nRebooting...");
                                health::reboot();
                            }
                        }
                        Key::F2 => {
                            println_console("\nRebooting to firmware setup...");
                            health::reboot_to_firmware_setup();
                        }
                        Key::F12 => {
                            println_console("\nRebooting...");
                            health::reboot();
                        }
                        Key::Char(c) if c.is_ascii_digit() => {
                            let idx = (c as u8 - b'0') as usize;
                            if idx < entries.len() {
                                self.selected_index = idx;
                                return Ok(entries[self.selected_index].clone());
                            }
                        }
                        _ => {}
                    }
                }
                InputEvent::None => {
                    uefi::boot::stall(core::time::Duration::from_millis(100));
                    elapsed_frames += 1;
                }
                _ => {}
            }
        }
    }

    fn draw_menu(&self, entries: &[BootEntry], timeout_remaining: Option<u32>) {
        uefi::system::with_stdout(|stdout| {
            let _ = stdout.clear();
        });

        println_console(&format!(
            "=== LamBoot v{} ({}) ===",
            crate::VERSION,
            crate::BUILD_TARGET
        ));
        println_console("");

        if entries.is_empty() {
            println_console("  No bootable entries found.");
            println_console("  LamBoot searched: BLS entries, UKIs, Windows, GRUB, rEFInd");
            println_console("");
        } else {
            for (i, entry) in entries.iter().enumerate() {
                let marker = if i == self.selected_index { ">" } else { " " };
                let status = match entry.preflight.as_ref().map(|p| p.status) {
                    Some(crate::preflight::PreflightStatus::Warning) => " [!]",
                    Some(crate::preflight::PreflightStatus::Error) => " [X]",
                    _ => "",
                };
                println_console(&format!(" {marker} {i}) {}{status}", entry.name));
            }
        }

        // System actions
        println_console("");
        println_console("--- System Actions ---");
        let fw_marker = if self.selected_index == entries.len() {
            ">"
        } else {
            " "
        };
        let rb_marker = if self.selected_index == entries.len() + 1 {
            ">"
        } else {
            " "
        };
        println_console(&format!(" {fw_marker} F2) Reboot to Firmware Setup"));
        println_console(&format!(" {rb_marker} F12) Reboot"));

        println_console("");
        if let Some(secs) = timeout_remaining {
            println_console(&format!("Auto-boot in {secs}s... Press any key to stop."));
        } else {
            println_console("Up/Down: Navigate  Enter: Select  F2: Firmware  F12: Reboot");
        }
    }
}

/// Print a line to the UEFI console
fn println_console(msg: &str) {
    uefi::system::with_stdout(|stdout| {
        let buf = uefi::CString16::try_from(msg).unwrap_or_default();
        let _ = stdout.output_string(&buf);
        let newline = cstr16!("\r\n");
        let _ = stdout.output_string(newline);
    });
}
