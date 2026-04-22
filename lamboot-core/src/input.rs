use core::time::Duration;

use uefi::{boot, proto::usb::io::UsbIo, system, Handle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputEvent {
    KeyPress(Key),
    MouseMove { x: i32, y: i32 },
    MouseClick { x: i32, y: i32, button: MouseButton },
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Left,
    Right,
    #[expect(
        dead_code,
        reason = "enter key is produced by Char('\\r') in UEFI SimpleTextInput"
    )]
    Enter,
    Escape,
    F1,
    F2,
    F12,
    Char(char),
    #[expect(dead_code, reason = "reserved for future editor/shell input")]
    Backspace,
    #[expect(dead_code, reason = "reserved for future editor/shell input")]
    Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseButton {
    Left,
    #[expect(dead_code, reason = "reserved for context menu support")]
    Right,
    #[expect(dead_code, reason = "reserved for middle-click paste")]
    Middle,
}

/// QEMU USB tablet HID report format (6 bytes):
///   [0]     buttons (bit 0 = left, bit 1 = right, bit 2 = middle)
///   [1..3]  X absolute position (u16 LE, 0..0x7FFF)
///   [3..5]  Y absolute position (u16 LE, 0..0x7FFF)
///   [5]     Z wheel (i8, relative)
const TABLET_REPORT_SIZE: usize = 6;
const TABLET_MAX_COORD: i32 = 0x7FFF;

/// USB HID class constants
const USB_CLASS_HID: u8 = 0x03;
const USB_SUBCLASS_BOOT: u8 = 0x01;

pub(crate) struct InputManager {
    mouse_x: i32,
    mouse_y: i32,
    mouse_button_down: bool,
    screen_width: i32,
    screen_height: i32,
    /// Handle to the USB tablet device (found during init, None if no tablet)
    tablet_handle: Option<Handle>,
    /// Interrupt IN endpoint address for the tablet
    tablet_endpoint: u8,
}

impl InputManager {
    pub(crate) fn new(screen_width: usize, screen_height: usize) -> Self {
        let (tablet_handle, tablet_endpoint) = find_usb_tablet();

        if tablet_handle.is_some() {
            log::info!("Input: USB HID tablet found (endpoint 0x{tablet_endpoint:02x})");
        } else {
            log::info!("Input: no USB tablet found, keyboard only");
        }

        Self {
            mouse_x: (screen_width / 2) as i32,
            mouse_y: (screen_height / 2) as i32,
            mouse_button_down: false,
            screen_width: screen_width as i32,
            screen_height: screen_height as i32,
            tablet_handle,
            tablet_endpoint,
        }
    }

    /// Poll for input: USB tablet first (QEMU/VNC), then standard UEFI
    /// pointer protocols (physical hardware with proper mouse drivers),
    /// then keyboard.
    pub(crate) fn poll(&mut self) -> InputEvent {
        if let Some(event) = self.poll_usb_tablet() {
            return event;
        }
        if let Some(event) = self.poll_standard_pointer() {
            return event;
        }
        Self::poll_keyboard()
    }

    fn poll_keyboard() -> InputEvent {
        let result = system::with_stdin(|stdin| stdin.read_key().ok().flatten());
        match result {
            Some(key) => {
                use uefi::proto::console::text::Key as UefiKey;

                match key {
                    UefiKey::Printable(c) => {
                        let ch: char = c.into();
                        InputEvent::KeyPress(Key::Char(ch))
                    }
                    UefiKey::Special(scan_code) => {
                        use uefi::proto::console::text::ScanCode;

                        match scan_code {
                            ScanCode::UP => InputEvent::KeyPress(Key::Up),
                            ScanCode::DOWN => InputEvent::KeyPress(Key::Down),
                            ScanCode::LEFT => InputEvent::KeyPress(Key::Left),
                            ScanCode::RIGHT => InputEvent::KeyPress(Key::Right),
                            ScanCode::ESCAPE => InputEvent::KeyPress(Key::Escape),
                            ScanCode::FUNCTION_1 => InputEvent::KeyPress(Key::F1),
                            ScanCode::FUNCTION_2 => InputEvent::KeyPress(Key::F2),
                            ScanCode::FUNCTION_12 => InputEvent::KeyPress(Key::F12),
                            _ => InputEvent::None,
                        }
                    }
                }
            }
            _ => InputEvent::None,
        }
    }

    /// Poll the USB HID tablet for position and button state.
    /// Uses synchronous interrupt transfer with a 1ms timeout (non-blocking).
    fn poll_usb_tablet(&mut self) -> Option<InputEvent> {
        let handle = self.tablet_handle?;

        let mut usb_io = boot::open_protocol_exclusive::<UsbIo>(handle).ok()?;

        let mut report = [0u8; TABLET_REPORT_SIZE];
        let result = usb_io.sync_interrupt_receive(self.tablet_endpoint, &mut report, 1);

        match result {
            Ok(len) if len >= TABLET_REPORT_SIZE => {}
            _ => return None, // No data or timeout — no new input
        }

        // Parse QEMU tablet HID report
        let buttons = report[0];
        let raw_x = u16::from_le_bytes([report[1], report[2]]) as i32;
        let raw_y = u16::from_le_bytes([report[3], report[4]]) as i32;

        // Scale absolute coordinates to screen pixels
        let new_x = raw_x * self.screen_width / TABLET_MAX_COORD;
        let new_y = raw_y * self.screen_height / TABLET_MAX_COORD;

        self.mouse_x = new_x.max(0).min(self.screen_width - 1);
        self.mouse_y = new_y.max(0).min(self.screen_height - 1);

        // Button state tracking
        let left_pressed = buttons & 1 != 0;
        if left_pressed && !self.mouse_button_down {
            self.mouse_button_down = true;
            return Some(InputEvent::MouseClick {
                x: self.mouse_x,
                y: self.mouse_y,
                button: MouseButton::Left,
            });
        } else if !left_pressed && self.mouse_button_down {
            self.mouse_button_down = false;
        }

        Some(InputEvent::MouseMove {
            x: self.mouse_x,
            y: self.mouse_y,
        })
    }

    /// Fallback: poll standard UEFI SimplePointer protocol.
    /// Works on physical hardware with native mouse drivers (PS/2, USB mouse
    /// with UsbMouseDxe). Does NOT work on QEMU/OVMF which lacks mouse drivers.
    fn poll_standard_pointer(&mut self) -> Option<InputEvent> {
        use uefi::proto::console::pointer::Pointer;

        let handle = boot::get_handle_for_protocol::<Pointer>().ok()?;
        let mut mouse = boot::open_protocol_exclusive::<Pointer>(handle).ok()?;
        let state = mouse.read_state().ok()??;

        let dx = state.relative_movement[0];
        let dy = state.relative_movement[1];
        self.mouse_x += dx;
        self.mouse_y += dy;
        self.mouse_x = self.mouse_x.max(0).min(self.screen_width - 1);
        self.mouse_y = self.mouse_y.max(0).min(self.screen_height - 1);

        if state.button[0] && !self.mouse_button_down {
            self.mouse_button_down = true;
            return Some(InputEvent::MouseClick {
                x: self.mouse_x,
                y: self.mouse_y,
                button: MouseButton::Left,
            });
        } else if !state.button[0] && self.mouse_button_down {
            self.mouse_button_down = false;
        }

        if dx != 0 || dy != 0 {
            return Some(InputEvent::MouseMove {
                x: self.mouse_x,
                y: self.mouse_y,
            });
        }

        None
    }

    #[expect(dead_code, reason = "public API for GUI hit-testing")]
    pub(crate) fn mouse_pos(&self) -> (i32, i32) {
        (self.mouse_x, self.mouse_y)
    }

    pub(crate) fn wait_for_key(&mut self) -> Key {
        loop {
            if let InputEvent::KeyPress(key) = self.poll() {
                return key;
            }
            uefi::boot::stall(Duration::from_millis(10));
        }
    }
}

/// Find a USB HID tablet device by scanning all UsbIo handles.
/// Returns (handle, interrupt_endpoint) or (None, 0) if not found.
///
/// Matches any HID device that is NOT a boot-protocol mouse/keyboard
/// (QEMU tablet uses InterfaceSubClass=0, InterfaceProtocol=0).
fn find_usb_tablet() -> (Option<Handle>, u8) {
    let Ok(handles) = boot::find_handles::<UsbIo>() else {
        return (None, 0);
    };

    for handle in handles {
        let Ok(mut usb_io) = boot::open_protocol_exclusive::<UsbIo>(handle) else {
            continue;
        };

        // Check interface descriptor for HID class
        let Ok(iface) = usb_io.interface_descriptor() else {
            continue;
        };

        if iface.interface_class != USB_CLASS_HID {
            continue;
        }

        // Skip boot-protocol devices (keyboards=1, mice=2) — we want the tablet
        if iface.interface_subclass == USB_SUBCLASS_BOOT {
            continue;
        }

        // Find interrupt IN endpoint
        for ep_idx in 0..iface.num_endpoints {
            let Ok(ep) = usb_io.endpoint_descriptor(ep_idx) else {
                continue;
            };

            // Interrupt endpoint (type bits [1:0] = 0x03) with IN direction (bit 7)
            let is_interrupt = (ep.attributes & 0x03) == 0x03;
            let is_in = (ep.endpoint_address & 0x80) != 0;

            if is_interrupt && is_in {
                let endpoint = ep.endpoint_address & 0x0F; // strip direction bit
                log::info!(
                    "USB tablet: class={:02x} sub={:02x} proto={:02x} ep=0x{:02x} maxpkt={}",
                    iface.interface_class,
                    iface.interface_subclass,
                    iface.interface_protocol,
                    ep.endpoint_address,
                    ep.max_packet_size,
                );
                return (Some(handle), endpoint);
            }
        }
    }

    (None, 0)
}
