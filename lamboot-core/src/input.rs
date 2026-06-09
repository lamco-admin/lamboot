//! Layer: 0 — Platform Introspection.

use core::time::Duration;

use uefi::{
    boot,
    proto::{unsafe_protocol, usb::io::UsbIo},
    system, Handle, Status,
};

// ---------------------------------------------------------------------------
// Absolute Pointer Protocol (UEFI 2.4+ §11.6) — FFI binding.
//
// uefi-rs 0.37 doesn't ship this protocol. Many USB HID drivers expose
// mice via AbsolutePointer (touchscreen-style absolute coordinates)
// instead of SimplePointer (relative). We need both.
//
// GUID: 8D59D32B-C655-4AE9-9B15-F25904992A43
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct AbsolutePointerState {
    current_x: u64,
    current_y: u64,
    current_z: u64,
    active_buttons: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct AbsolutePointerMode {
    absolute_min_x: u64,
    absolute_min_y: u64,
    absolute_min_z: u64,
    absolute_max_x: u64,
    absolute_max_y: u64,
    absolute_max_z: u64,
    attributes: u32,
}

#[repr(C)]
struct AbsolutePointerProtocolRaw {
    reset: unsafe extern "efiapi" fn(this: *mut AbsolutePointerProtocolRaw, ext: u8) -> Status,
    get_state: unsafe extern "efiapi" fn(
        this: *mut AbsolutePointerProtocolRaw,
        state: *mut AbsolutePointerState,
    ) -> Status,
    wait_for_input: *mut core::ffi::c_void,
    mode: *const AbsolutePointerMode,
}

#[repr(transparent)]
#[unsafe_protocol("8d59d32b-c655-4ae9-9b15-f25904992a43")]
struct AbsolutePointer(AbsolutePointerProtocolRaw);

impl AbsolutePointer {
    fn reset(&mut self, extended: bool) -> Result<(), Status> {
        let this: *mut AbsolutePointerProtocolRaw = &raw mut self.0;
        let s = unsafe { (self.0.reset)(this, u8::from(extended)) };
        if s == Status::SUCCESS {
            Ok(())
        } else {
            Err(s)
        }
    }
    fn read_state(&mut self) -> Result<Option<AbsolutePointerState>, Status> {
        let mut st = AbsolutePointerState::default();
        let this: *mut AbsolutePointerProtocolRaw = &raw mut self.0;
        let st_ptr: *mut AbsolutePointerState = &raw mut st;
        let s = unsafe { (self.0.get_state)(this, st_ptr) };
        match s {
            Status::SUCCESS => Ok(Some(st)),
            Status::NOT_READY => Ok(None),
            other => Err(other),
        }
    }
    fn mode(&self) -> &AbsolutePointerMode {
        unsafe { &*self.0.mode }
    }
}

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
    /// All SimpleTextInput handles discovered at init. Firmware
    /// typically installs one per attached keyboard. The default
    /// ConIn handle (system::with_stdin) is one of these; on
    /// hardware where firmware-default ConIn is wired to the
    /// wrong device (observed on ASUS G10AJ), polling all handles
    /// catches the real keyboard.
    keyboard_handles: alloc::vec::Vec<Handle>,
    /// All SimplePointer handles discovered at init. Same rationale
    /// — pve2 shows 2 SimplePointer handles; the firmware default
    /// path via get_handle_for_protocol picks only the first.
    pointer_handles: alloc::vec::Vec<Handle>,
    /// AbsolutePointer (UEFI 2.4+) handles. Many USB HID drivers
    /// expose mice/touchscreens here instead of SimplePointer.
    /// Coordinates are absolute (mapped to screen on poll).
    absolute_pointer_handles: alloc::vec::Vec<Handle>,
}

impl InputManager {
    pub(crate) fn new(screen_width: usize, screen_height: usize) -> Self {
        let (tablet_handle, tablet_endpoint) = find_usb_tablet();

        if tablet_handle.is_some() {
            log::info!("Input: USB HID tablet found (endpoint 0x{tablet_endpoint:02x})");
        } else {
            log::info!("Input: no USB tablet found, keyboard only");
        }

        use uefi::proto::console::{pointer::Pointer, text::Input};

        // ConnectController on every USB IO handle FIRST. UEFI defers
        // driver binding — without this, USB HID keyboards/mice may
        // have their controller handles enumerated but no actual driver
        // attached, so SimpleTextInput::read_key returns NOT_READY
        // forever. Connecting them BEFORE enumerating Input/Pointer
        // handles allows the HID driver to install those protocols.
        if let Ok(usb_handles) = boot::find_handles::<UsbIo>() {
            for handle in usb_handles {
                let _ = uefi::boot::connect_controller(handle, None, None, true);
            }
        }

        // Re-enumerate AFTER connect — new SimpleTextInput / SimplePointer /
        // AbsolutePointer handles may have appeared as drivers attached.
        let keyboard_handles = boot::find_handles::<Input>().unwrap_or_default();
        let pointer_handles = boot::find_handles::<Pointer>().unwrap_or_default();
        let absolute_pointer_handles = boot::find_handles::<AbsolutePointer>().unwrap_or_default();
        // Recursive connect on each input handle too, in case the actual
        // device driver lives one layer below the protocol-holding handle.
        for &handle in keyboard_handles
            .iter()
            .chain(pointer_handles.iter())
            .chain(absolute_pointer_handles.iter())
        {
            let _ = uefi::boot::connect_controller(handle, None, None, true);
        }

        // Reset each pointer device — many UEFI HID drivers refuse to
        // deliver state events until reset() has been called.
        use uefi::{
            boot::{OpenProtocolAttributes, OpenProtocolParams},
            proto::console::pointer::Pointer as PointerProto,
        };
        let image_h = boot::image_handle();
        for &handle in &pointer_handles {
            let params = OpenProtocolParams {
                handle,
                agent: image_h,
                controller: None,
            };
            if let Ok(mut p) = unsafe {
                boot::open_protocol::<PointerProto>(params, OpenProtocolAttributes::GetProtocol)
            } {
                let _ = p.reset(false);
            }
        }
        for &handle in &absolute_pointer_handles {
            let params = OpenProtocolParams {
                handle,
                agent: image_h,
                controller: None,
            };
            if let Ok(mut p) = unsafe {
                boot::open_protocol::<AbsolutePointer>(params, OpenProtocolAttributes::GetProtocol)
            } {
                let _ = p.reset(false);
            }
        }

        log::info!(
            "Input: {} SimpleTextInput, {} SimplePointer, {} AbsolutePointer handle(s) (post-connect, reset)",
            keyboard_handles.len(),
            pointer_handles.len(),
            absolute_pointer_handles.len(),
        );

        Self {
            mouse_x: (screen_width / 2) as i32,
            mouse_y: (screen_height / 2) as i32,
            mouse_button_down: false,
            screen_width: screen_width as i32,
            screen_height: screen_height as i32,
            tablet_handle,
            tablet_endpoint,
            keyboard_handles,
            pointer_handles,
            absolute_pointer_handles,
        }
    }

    /// Poll for input: USB tablet first (QEMU/VNC), then standard UEFI
    /// pointer protocols on every discovered handle (physical hardware,
    /// multi-mouse desktops), then keyboard via both ConIn and every
    /// enumerated SimpleTextInput handle.
    pub(crate) fn poll(&mut self) -> InputEvent {
        if let Some(event) = self.poll_usb_tablet() {
            return event;
        }
        if let Some(event) = self.poll_standard_pointer() {
            return event;
        }
        if let Some(event) = self.poll_absolute_pointer() {
            return event;
        }
        self.poll_keyboard()
    }

    /// Poll AbsolutePointer handles (UEFI 2.4+). Many USB HID mice
    /// expose themselves here instead of SimplePointer. Coordinates
    /// are absolute in the device's own range — we scale to screen.
    fn poll_absolute_pointer(&mut self) -> Option<InputEvent> {
        use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
        let image = boot::image_handle();
        let handles: alloc::vec::Vec<Handle> = self.absolute_pointer_handles.clone();
        for handle in handles {
            let params = OpenProtocolParams {
                handle,
                agent: image,
                controller: None,
            };
            let Ok(mut p) = (unsafe {
                boot::open_protocol::<AbsolutePointer>(params, OpenProtocolAttributes::GetProtocol)
            }) else {
                continue;
            };
            let Ok(Some(state)) = p.read_state() else {
                continue;
            };
            let mode = *p.mode();
            // Scale device coordinates to screen pixels.
            let range_x = mode
                .absolute_max_x
                .saturating_sub(mode.absolute_min_x)
                .max(1);
            let range_y = mode
                .absolute_max_y
                .saturating_sub(mode.absolute_min_y)
                .max(1);
            let rel_x = state.current_x.saturating_sub(mode.absolute_min_x);
            let rel_y = state.current_y.saturating_sub(mode.absolute_min_y);
            let new_x = (rel_x as u128 * self.screen_width as u128 / range_x as u128) as i32;
            let new_y = (rel_y as u128 * self.screen_height as u128 / range_y as u128) as i32;
            self.mouse_x = new_x.max(0).min(self.screen_width - 1);
            self.mouse_y = new_y.max(0).min(self.screen_height - 1);
            let pressed = state.active_buttons & 1 != 0;
            if pressed && !self.mouse_button_down {
                self.mouse_button_down = true;
                return Some(InputEvent::MouseClick {
                    x: self.mouse_x,
                    y: self.mouse_y,
                    button: MouseButton::Left,
                });
            } else if !pressed && self.mouse_button_down {
                self.mouse_button_down = false;
            }
            return Some(InputEvent::MouseMove {
                x: self.mouse_x,
                y: self.mouse_y,
            });
        }
        None
    }

    fn poll_keyboard(&self) -> InputEvent {
        // ConIn first — works on QEMU/OVMF and most embedded firmware.
        let stdin_result = system::with_stdin(|stdin| stdin.read_key().ok().flatten());
        if let Some(ev) = stdin_result.and_then(uefi_key_to_event) {
            return ev;
        }
        // Multi-handle keyboard probe via NON-DISTURBING access (GetProtocol).
        // open_protocol_exclusive on firmware with BY_DRIVER opens cascades a
        // driver disconnect — same root cause as the BlockIO bug we hit during
        // LVM probe. GetProtocol just hands us the protocol pointer without
        // claiming or kicking off existing agents.
        //
        // On ASUS G10AJ (pve2), firmware-default ConIn is not wired to the
        // actual USB keyboard. The real key events arrive on one of the
        // other enumerated SimpleTextInput handles, hence the iteration.
        use uefi::{
            boot::{OpenProtocolAttributes, OpenProtocolParams},
            proto::console::text::Input,
        };
        let image = boot::image_handle();
        for &handle in &self.keyboard_handles {
            let params = OpenProtocolParams {
                handle,
                agent: image,
                controller: None,
            };
            let Ok(mut input) = (unsafe {
                boot::open_protocol::<Input>(params, OpenProtocolAttributes::GetProtocol)
            }) else {
                continue;
            };
            if let Ok(Some(key)) = input.read_key() {
                if let Some(ev) = uefi_key_to_event(key) {
                    return ev;
                }
            }
        }
        InputEvent::None
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

    /// Poll every enumerated SimplePointer handle non-disturbingly
    /// (GetProtocol mode — does not cascade-disconnect existing
    /// BY_DRIVER opens on the handle, unlike open_protocol_exclusive).
    fn poll_standard_pointer(&mut self) -> Option<InputEvent> {
        use uefi::{
            boot::{OpenProtocolAttributes, OpenProtocolParams},
            proto::console::pointer::Pointer,
        };
        let image = boot::image_handle();
        // Copy out to break the borrow against &self.pointer_handles since
        // process_pointer_state needs &mut self.
        let handles: alloc::vec::Vec<Handle> = self.pointer_handles.clone();
        for handle in handles {
            let params = OpenProtocolParams {
                handle,
                agent: image,
                controller: None,
            };
            let Ok(mut mouse) = (unsafe {
                boot::open_protocol::<Pointer>(params, OpenProtocolAttributes::GetProtocol)
            }) else {
                continue;
            };
            let Ok(Some(state)) = mouse.read_state() else {
                continue;
            };
            if let Some(ev) = self.process_pointer_state(&state) {
                return Some(ev);
            }
        }
        None
    }

    fn process_pointer_state(
        &mut self,
        state: &uefi::proto::console::pointer::PointerState,
    ) -> Option<InputEvent> {
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

/// Translate a UEFI SimpleTextInput key into our InputEvent. Returns
/// `None` for keys we don't map.
fn uefi_key_to_event(key: uefi::proto::console::text::Key) -> Option<InputEvent> {
    use uefi::proto::console::text::{Key as UefiKey, ScanCode};
    match key {
        UefiKey::Printable(c) => {
            let ch: char = c.into();
            Some(InputEvent::KeyPress(Key::Char(ch)))
        }
        UefiKey::Special(scan_code) => match scan_code {
            ScanCode::UP => Some(InputEvent::KeyPress(Key::Up)),
            ScanCode::DOWN => Some(InputEvent::KeyPress(Key::Down)),
            ScanCode::LEFT => Some(InputEvent::KeyPress(Key::Left)),
            ScanCode::RIGHT => Some(InputEvent::KeyPress(Key::Right)),
            ScanCode::ESCAPE => Some(InputEvent::KeyPress(Key::Escape)),
            ScanCode::FUNCTION_1 => Some(InputEvent::KeyPress(Key::F1)),
            ScanCode::FUNCTION_2 => Some(InputEvent::KeyPress(Key::F2)),
            ScanCode::FUNCTION_12 => Some(InputEvent::KeyPress(Key::F12)),
            _ => None,
        },
    }
}
