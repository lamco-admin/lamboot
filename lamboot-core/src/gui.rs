use alloc::{format, string::String, vec, vec::Vec};
use core::time::Duration;

use uefi::{
    proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput},
    Result,
};

use crate::{
    discovery::{BootEntry, EntryKind, Icon},
    health,
    input::{InputEvent, InputManager, Key},
    policy::Policy,
};

const BACKGROUND_COLOR: BltPixel = BltPixel::new(0x1e, 0x1e, 0x2e);
const TEXT_COLOR: BltPixel = BltPixel::new(0xff, 0xff, 0xff);
const ACCENT_COLOR: BltPixel = BltPixel::new(0x89, 0xb4, 0xfa);
const SELECTED_COLOR: BltPixel = BltPixel::new(0x45, 0x47, 0x5a);
const TILE_COLOR: BltPixel = BltPixel::new(0x31, 0x32, 0x44);
const SUBTITLE_COLOR: BltPixel = BltPixel::new(0x80, 0x80, 0x90);
const WARNING_COLOR: BltPixel = BltPixel::new(0xfa, 0xb3, 0x87);
const DIVIDER_COLOR: BltPixel = BltPixel::new(0x40, 0x40, 0x55);

const LOGO_WIDTH: usize = 128;
// Logo dimensions (width used for draw_logo, height for layout reference)
#[expect(
    dead_code,
    reason = "documents the logo asset dimensions for layout calculations"
)]
const LOGO_HEIGHT: usize = 116;
const LOGO_COMPRESSED: &[u8] = include_bytes!("logo.compressed");

const FONT_SMALL: &[u8] = include_bytes!("font_terminus_16.raw");
const FONT_SMALL_W: usize = 8;
const FONT_SMALL_H: usize = 16;

const FONT_LARGE: &[u8] = include_bytes!("font_terminus_32.raw");
const FONT_LARGE_W: usize = 16;
const FONT_LARGE_H: usize = 32;

// Layout constants
const BOOT_TILE_H: usize = 60;
const TOOL_TILE_H: usize = 40;
const TILE_GAP: usize = 6;
const HEADER_H: usize = 140; // Logo (116) + padding
const FOOTER_H: usize = 50;
const MARGIN: usize = 20;
const COL_GAP: usize = 20;

#[derive(Clone, Copy)]
enum Font {
    Small,
    Large,
}

impl Font {
    const fn width(self) -> usize {
        match self {
            Font::Small => FONT_SMALL_W,
            Font::Large => FONT_LARGE_W,
        }
    }
}

const _: () = {
    assert!(FONT_SMALL.len() == 256 * FONT_SMALL_H);
    assert!(FONT_LARGE.len() == 256 * 2 * FONT_LARGE_H);
};

// ─── Framebuffer (unchanged) ───────────────────────────────────────

struct Framebuffer {
    pixels: Vec<BltPixel>,
    width: usize,
    height: usize,
}

impl Framebuffer {
    fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![BACKGROUND_COLOR; width * height],
            width,
            height,
        }
    }

    fn clear(&mut self) {
        self.pixels.fill(BACKGROUND_COLOR);
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: BltPixel) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        let x_start = x.min(self.width);
        for row in y.min(self.height)..y_end {
            let row_offset = row * self.width;
            for col in x_start..x_end {
                self.pixels[row_offset + col] = color;
            }
        }
    }

    fn draw_rect_border(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        thickness: usize,
        color: BltPixel,
    ) {
        self.fill_rect(x, y, w, thickness, color);
        if h > thickness {
            self.fill_rect(x, y + h - thickness, w, thickness, color);
        }
        self.fill_rect(x, y, thickness, h, color);
        if w > thickness {
            self.fill_rect(x + w - thickness, y, thickness, h, color);
        }
    }

    fn draw_char(&mut self, ch: char, x: i32, y: i32, color: BltPixel, font: Font) {
        let (data, glyph_w, glyph_h, bytes_per_row) = match font {
            Font::Small => (FONT_SMALL, FONT_SMALL_W, FONT_SMALL_H, 1usize),
            Font::Large => (FONT_LARGE, FONT_LARGE_W, FONT_LARGE_H, 2usize),
        };
        let idx = if (ch as u32) < 256 { ch as usize } else { 0xFE };
        let glyph_offset = idx * bytes_per_row * glyph_h;

        for row in 0..glyph_h {
            let py = y + row as i32;
            if py < 0 || py as usize >= self.height {
                continue;
            }
            let row_offset = glyph_offset + row * bytes_per_row;
            for col in 0..glyph_w {
                let byte_idx = col / 8;
                let bit_idx = 7 - (col % 8);
                if (data[row_offset + byte_idx] >> bit_idx) & 1 == 1 {
                    let px = x + col as i32;
                    if px >= 0 && (px as usize) < self.width {
                        self.pixels[py as usize * self.width + px as usize] = color;
                    }
                }
            }
        }
    }

    fn draw_text(&mut self, text: &str, x: i32, y: i32, color: BltPixel, font: Font) {
        let char_width = font.width() as i32;
        for (i, ch) in text.chars().enumerate() {
            let char_x = x + (i as i32 * char_width);
            if char_x >= self.width as i32 {
                break;
            }
            if char_x + char_width < 0 {
                continue;
            }
            self.draw_char(ch, char_x, y, color, font);
        }
    }

    fn draw_logo(&mut self, dest_x: usize, dest_y: usize) {
        let data = LOGO_COMPRESSED;
        if data.len() < 14 || &data[0..4] != b"LBCL" {
            return;
        }

        let palette_count = u16::from_le_bytes([data[8], data[9]]) as usize;
        let run_count = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;

        let palette_start = 14;
        let runs_start = palette_start + palette_count * 3;

        if runs_start + run_count * 4 > data.len() {
            return;
        }

        let mut px_offset: usize = 0;
        for i in 0..run_count {
            let run_off = runs_start + i * 4;
            let count = u16::from_le_bytes([data[run_off], data[run_off + 1]]) as usize;
            let pal_idx = data[run_off + 2] as usize;
            let alpha = data[run_off + 3];

            if alpha == 0 {
                px_offset += count;
                continue;
            }

            let pal_off = palette_start + pal_idx * 3;
            let r = data[pal_off];
            let g = data[pal_off + 1];
            let b = data[pal_off + 2];

            for _ in 0..count {
                let col = px_offset % LOGO_WIDTH;
                let row = px_offset / LOGO_WIDTH;
                px_offset += 1;

                let px = dest_x + col;
                let py = dest_y + row;
                if px >= self.width || py >= self.height {
                    continue;
                }

                let idx = py * self.width + px;
                if alpha == 255 {
                    self.pixels[idx] = BltPixel::new(b, g, r);
                } else {
                    let bg = self.pixels[idx];
                    let inv = 255 - alpha;
                    let blend = |fg: u8, bg_ch: u8| -> u8 {
                        ((u16::from(fg) * u16::from(alpha) + u16::from(bg_ch) * u16::from(inv))
                            / 255) as u8
                    };
                    self.pixels[idx] =
                        BltPixel::new(blend(b, bg.red), blend(g, bg.green), blend(r, bg.blue));
                }
            }
        }
    }

    fn present(&self, gop: &mut GraphicsOutput) {
        let _ = gop.blt(BltOp::BufferToVideo {
            buffer: &self.pixels,
            src: BltRegion::Full,
            dest: (0, 0),
            dims: (self.width, self.height),
        });
    }
}

// ─── System info passed from main ──────────────────────────────────

/// System information displayed in the GUI header
#[derive(Default)]
pub(crate) struct SystemInfoDisplay {
    pub vmid: Option<String>,
    pub hypervisor: Option<String>,
    pub system_product: Option<String>,
    pub secure_boot: bool,
    pub driver_count: usize,
}

// ─── GUI Context ───────────────────────────────────────────────────

/// Which column the cursor is in (two-column mode)
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Left,
    Right,
}

pub(crate) struct GuiContext {
    width: usize,
    height: usize,
    /// Index within the active column
    sel_idx: usize,
    /// Active column
    sel_col: Column,
    scroll_left: usize,
    scroll_right: usize,
    mouse_x: i32,
    mouse_y: i32,
    input: InputManager,
    fb: Framebuffer,
    sys_info: SystemInfoDisplay,
}

impl GuiContext {
    pub(crate) fn new() -> Result<Self> {
        let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()?;
        let gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle)?;

        let mode = gop.current_mode_info();
        let (width, height) = mode.resolution();

        let input = InputManager::new(width, height);
        let fb = Framebuffer::new(width, height);
        Ok(Self {
            width,
            height,
            sel_idx: 0,
            sel_col: Column::Left,
            scroll_left: 0,
            scroll_right: 0,
            mouse_x: (width / 2) as i32,
            mouse_y: (height / 2) as i32,
            input,
            fb,
            sys_info: SystemInfoDisplay::default(),
        })
    }

    pub(crate) fn set_system_info(&mut self, info: SystemInfoDisplay) {
        self.sys_info = info;
    }

    // ─── Layout helpers ─────────────────────────────────────────

    fn content_bottom(&self) -> usize {
        self.height.saturating_sub(FOOTER_H)
    }

    fn content_height(&self) -> usize {
        self.content_bottom().saturating_sub(HEADER_H)
    }

    fn left_col_w(&self) -> usize {
        (self.width - 2 * MARGIN - COL_GAP) * 55 / 100
    }

    fn right_col_x(&self) -> usize {
        MARGIN + self.left_col_w() + COL_GAP
    }

    fn right_col_w(&self) -> usize {
        self.width - self.right_col_x() - MARGIN
    }

    fn max_visible_boot(&self) -> usize {
        (self.content_height() / (BOOT_TILE_H + TILE_GAP)).max(1)
    }

    // ─── Main menu loop ─────────────────────────────────────────

    pub(crate) fn run_menu(&mut self, entries: &[BootEntry], policy: &Policy) -> Result<BootEntry> {
        // Split entries: boot entries (left), tools (right)
        let boot_entries: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.icon != Icon::Tools)
            .map(|(i, _)| i)
            .collect();
        let tool_entries: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.icon == Icon::Tools)
            .map(|(i, _)| i)
            .collect();
        // Right column: tools + 2 system actions
        let right_count = tool_entries.len() + 2; // +2 for firmware setup, reboot

        let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()?;
        let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle)?;

        let has_bootable = !boot_entries.is_empty();
        let timeout_ms = if has_bootable {
            policy.default_timeout_ms
        } else {
            0
        };
        let mut timed_out = false;
        let mut user_interacted = false;
        let mut elapsed_frames = 0u32;

        loop {
            self.fb.clear();
            self.draw_header();

            self.draw_left_column(entries, &boot_entries);
            self.draw_right_column(entries, &tool_entries);

            // Footer
            self.draw_footer(
                entries,
                &boot_entries,
                &tool_entries,
                timeout_ms,
                elapsed_frames,
                user_interacted,
            );

            self.draw_cursor(self.mouse_x, self.mouse_y);
            self.fb.present(&mut gop);

            // Auto-boot timeout
            if !user_interacted && timeout_ms > 0 {
                let elapsed_ms = elapsed_frames * 16;
                if elapsed_ms >= timeout_ms {
                    timed_out = true;
                }
            }

            if timed_out && !user_interacted && has_bootable {
                // Find default entry or use first boot entry
                let boot_idx = if let Some(ref default_id) = policy.default_entry {
                    boot_entries
                        .iter()
                        .position(|&i| entries[i].id == *default_id)
                        .unwrap_or(0)
                } else {
                    0
                };
                let entry_idx = boot_entries[boot_idx];
                self.draw_boot_progress(&entries[entry_idx].name, &mut gop);
                return Ok(entries[entry_idx].clone());
            }

            let event = self.input.poll();

            match event {
                InputEvent::KeyPress(key) => {
                    user_interacted = true;
                    match key {
                        Key::Up if self.sel_idx > 0 => {
                            self.sel_idx -= 1;
                            self.adjust_scroll();
                        }
                        Key::Down => {
                            let max = match self.sel_col {
                                Column::Left => boot_entries.len(),
                                Column::Right => right_count,
                            };
                            if self.sel_idx + 1 < max {
                                self.sel_idx += 1;
                                self.adjust_scroll();
                            }
                        }
                        Key::Left if self.sel_col == Column::Right => {
                            self.sel_col = Column::Left;
                            self.sel_idx = self.sel_idx.min(boot_entries.len().saturating_sub(1));
                        }
                        Key::Right if self.sel_col == Column::Left => {
                            self.sel_col = Column::Right;
                            self.sel_idx = self.sel_idx.min(right_count.saturating_sub(1));
                        }
                        Key::Enter | Key::Char('\r') => {
                            match self.sel_col {
                                Column::Left => {
                                    if self.sel_idx < boot_entries.len() {
                                        let idx = boot_entries[self.sel_idx];
                                        self.draw_boot_progress(&entries[idx].name, &mut gop);
                                        return Ok(entries[idx].clone());
                                    }
                                }
                                Column::Right => {
                                    if self.sel_idx < tool_entries.len() {
                                        let idx = tool_entries[self.sel_idx];
                                        self.draw_boot_progress(&entries[idx].name, &mut gop);
                                        return Ok(entries[idx].clone());
                                    }
                                    // System actions
                                    let action = self.sel_idx - tool_entries.len();
                                    self.execute_system_action(action, &mut gop);
                                }
                            }
                        }
                        Key::F2 => {
                            self.execute_system_action(0, &mut gop);
                        }
                        Key::F12 => {
                            self.execute_system_action(1, &mut gop);
                        }
                        Key::F1 => {
                            self.show_help(&mut gop);
                        }
                        _ => {}
                    }
                }
                InputEvent::MouseMove { x, y } => {
                    // Mouse movement alone does NOT cancel auto-boot timer
                    self.mouse_x = x;
                    self.mouse_y = y;
                    // Update selection based on mouse position
                    if let Some((col, idx)) = self.hit_test(x, y, &boot_entries, &tool_entries) {
                        self.sel_col = col;
                        self.sel_idx = idx;
                    }
                }
                InputEvent::MouseClick { x, y, .. } => {
                    user_interacted = true;
                    if let Some((col, idx)) = self.hit_test(x, y, &boot_entries, &tool_entries) {
                        self.sel_col = col;
                        self.sel_idx = idx;
                        // Boot or execute the clicked entry
                        match col {
                            Column::Left if idx < boot_entries.len() => {
                                let ei = boot_entries[idx];
                                self.draw_boot_progress(&entries[ei].name, &mut gop);
                                return Ok(entries[ei].clone());
                            }
                            Column::Right if idx < tool_entries.len() => {
                                let ei = tool_entries[idx];
                                self.draw_boot_progress(&entries[ei].name, &mut gop);
                                return Ok(entries[ei].clone());
                            }
                            Column::Right => {
                                let action = idx - tool_entries.len();
                                self.execute_system_action(action, &mut gop);
                            }
                            Column::Left => {}
                        }
                    }
                }
                InputEvent::None => {
                    uefi::boot::stall(Duration::from_millis(16));
                    elapsed_frames += 1;
                }
            }
        }
    }

    fn adjust_scroll(&mut self) {
        match self.sel_col {
            Column::Left => {
                let max_vis = self.max_visible_boot();
                if self.sel_idx < self.scroll_left {
                    self.scroll_left = self.sel_idx;
                } else if self.sel_idx >= self.scroll_left + max_vis {
                    self.scroll_left = self.sel_idx + 1 - max_vis;
                }
            }
            Column::Right => {
                let max_vis = self.max_visible_boot();
                if self.sel_idx < self.scroll_right {
                    self.scroll_right = self.sel_idx;
                } else if self.sel_idx >= self.scroll_right + max_vis {
                    self.scroll_right = self.sel_idx + 1 - max_vis;
                }
            }
        }
    }

    // ─── Header ─────────────────────────────────────────────────

    fn draw_header(&mut self) {
        // Logo on the left
        let logo_x = MARGIN;
        let logo_y = 8;
        self.fb.draw_logo(logo_x, logo_y);

        // Title beside the logo
        let text_x = (MARGIN + LOGO_WIDTH + 16) as i32;
        self.fb.draw_text(
            "LamBoot",
            text_x,
            logo_y as i32 + 20,
            ACCENT_COLOR,
            Font::Large,
        );

        let version = format!("v{} ({})", crate::VERSION, crate::BUILD_TARGET);
        self.fb.draw_text(
            &version,
            text_x,
            logo_y as i32 + 56,
            SUBTITLE_COLOR,
            Font::Small,
        );

        // System info on the right side
        let mut info_y = logo_y as i32 + 10;
        let right_edge = (self.width - MARGIN) as i32;

        if let Some(ref vmid) = self.sys_info.vmid {
            let label = format!("VM {vmid}");
            let lx = right_edge - (label.len() as i32 * FONT_LARGE_W as i32);
            self.fb
                .draw_text(&label, lx, info_y, ACCENT_COLOR, Font::Large);
            info_y += 40;
        }

        // System details below VMID
        let mut line_parts = Vec::new();
        let hv_owned;
        if let Some(ref hv) = self.sys_info.hypervisor {
            hv_owned = hv.clone();
            line_parts.push(hv_owned);
        }
        let prod_owned;
        if let Some(ref prod) = self.sys_info.system_product {
            prod_owned = prod.clone();
            line_parts.push(prod_owned);
        }
        if !line_parts.is_empty() {
            let line: String = line_parts.join(" | ");
            let lx = right_edge - (line.len() as i32 * FONT_SMALL_W as i32);
            self.fb
                .draw_text(&line, lx, info_y, SUBTITLE_COLOR, Font::Small);
            info_y += 18;
        }

        // Build + security info
        let sb = if self.sys_info.secure_boot {
            "SB: on"
        } else {
            "SB: off"
        };
        let drv = self.sys_info.driver_count;
        let build_line = format!("{sb} | {drv} driver(s)");
        let lx = right_edge - (build_line.len() as i32 * FONT_SMALL_W as i32);
        self.fb
            .draw_text(&build_line, lx, info_y, SUBTITLE_COLOR, Font::Small);

        // Divider line below header
        self.fb.fill_rect(
            MARGIN,
            HEADER_H - 2,
            self.width - 2 * MARGIN,
            1,
            DIVIDER_COLOR,
        );
    }

    // ─── Left column: boot entries ──────────────────────────────

    fn draw_left_column(&mut self, entries: &[BootEntry], boot_indices: &[usize]) {
        let x = MARGIN;
        let w = self.left_col_w();
        let top = HEADER_H;
        let max_vis = self.max_visible_boot();
        let vis_end = (self.scroll_left + max_vis).min(boot_indices.len());

        // Column header
        self.fb.draw_text(
            "Boot Entries",
            x as i32,
            top as i32 - FONT_SMALL_H as i32 - 2,
            SUBTITLE_COLOR,
            Font::Small,
        );

        if boot_indices.is_empty() {
            self.fb.draw_text(
                "No bootable entries found.",
                x as i32 + 8,
                top as i32 + 10,
                WARNING_COLOR,
                Font::Small,
            );
            return;
        }

        for (vi, bi) in (self.scroll_left..vis_end).enumerate() {
            let entry = &entries[boot_indices[bi]];
            let y = top + vi * (BOOT_TILE_H + TILE_GAP);
            let selected = self.sel_col == Column::Left && bi == self.sel_idx;

            self.draw_boot_tile(x, y, w, entry, selected);
        }

        // Scroll indicators
        if self.scroll_left > 0 {
            self.fb.draw_text(
                "^ more",
                (x + w / 2 - 24) as i32,
                top as i32 - 18,
                ACCENT_COLOR,
                Font::Small,
            );
        }
        if vis_end < boot_indices.len() {
            let y = top + max_vis * (BOOT_TILE_H + TILE_GAP);
            self.fb.draw_text(
                "v more",
                (x + w / 2 - 24) as i32,
                y as i32,
                ACCENT_COLOR,
                Font::Small,
            );
        }
    }

    fn draw_boot_tile(&mut self, x: usize, y: usize, w: usize, entry: &BootEntry, selected: bool) {
        let fill = if selected { SELECTED_COLOR } else { TILE_COLOR };
        self.fb.fill_rect(x, y, w, BOOT_TILE_H, fill);
        if selected {
            self.fb
                .draw_rect_border(x, y, w, BOOT_TILE_H, 2, ACCENT_COLOR);
        }

        // Icon badge
        let icon_text = icon_label(entry.icon);
        self.fb.draw_text(
            icon_text,
            x as i32 + 10,
            y as i32 + 8,
            ACCENT_COLOR,
            Font::Small,
        );

        // Name
        let name_x = x as i32 + 60;
        let max_px = w.saturating_sub(90);
        let name = truncate_to_fit(&entry.name, max_px, Font::Small);
        self.fb
            .draw_text(&name, name_x, y as i32 + 10, TEXT_COLOR, Font::Small);

        // Subtitle
        let subtitle = entry_type_label(&entry.kind, entry.icon);
        self.fb
            .draw_text(subtitle, name_x, y as i32 + 30, SUBTITLE_COLOR, Font::Small);

        // Preflight indicator
        if let Some(ref pf) = entry.preflight {
            let (indicator, color) = match pf.status {
                crate::preflight::PreflightStatus::Ok => ("", TEXT_COLOR),
                crate::preflight::PreflightStatus::Warning => ("!", WARNING_COLOR),
                crate::preflight::PreflightStatus::Error => ("X", BltPixel::new(0xf3, 0x8b, 0xa8)),
            };
            if !indicator.is_empty() {
                let ix = (x + w) as i32 - 20;
                self.fb
                    .draw_text(indicator, ix, y as i32 + 10, color, Font::Small);
            }
        }
    }

    // ─── Right column: tools + system actions ───────────────────

    fn draw_right_column(&mut self, entries: &[BootEntry], tool_indices: &[usize]) {
        let x = self.right_col_x();
        let w = self.right_col_w();
        let top = HEADER_H;

        // Column header
        self.fb.draw_text(
            "Tools & Actions",
            x as i32,
            top as i32 - FONT_SMALL_H as i32 - 2,
            SUBTITLE_COLOR,
            Font::Small,
        );

        // Tool entries
        let mut y = top;
        for (i, &ti) in tool_indices.iter().enumerate() {
            let entry = &entries[ti];
            let selected = self.sel_col == Column::Right && i == self.sel_idx;
            self.draw_tool_tile(x, y, w, &entry.name, "Diagnostic module", selected);
            y += TOOL_TILE_H + TILE_GAP;
        }

        // Divider before system actions
        y += 4;
        self.fb.fill_rect(x, y, w, 1, DIVIDER_COLOR);
        y += 8;

        // System actions
        let actions = [("Firmware Setup", "[F2]"), ("Reboot", "[F12]")];
        for (i, (label, key)) in actions.iter().enumerate() {
            let action_idx = tool_indices.len() + i;
            let selected = self.sel_col == Column::Right && action_idx == self.sel_idx;
            let fill = if selected { SELECTED_COLOR } else { TILE_COLOR };
            self.fb.fill_rect(x, y, w, TOOL_TILE_H, fill);
            if selected {
                self.fb
                    .draw_rect_border(x, y, w, TOOL_TILE_H, 2, ACCENT_COLOR);
            }
            self.fb
                .draw_text(label, x as i32 + 10, y as i32 + 12, TEXT_COLOR, Font::Small);
            let kx = (x + w) as i32 - (key.len() as i32 * FONT_SMALL_W as i32) - 10;
            self.fb
                .draw_text(key, kx, y as i32 + 12, ACCENT_COLOR, Font::Small);
            y += TOOL_TILE_H + TILE_GAP;
        }
    }

    fn draw_tool_tile(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        name: &str,
        subtitle: &str,
        selected: bool,
    ) {
        let fill = if selected { SELECTED_COLOR } else { TILE_COLOR };
        self.fb.fill_rect(x, y, w, TOOL_TILE_H, fill);
        if selected {
            self.fb
                .draw_rect_border(x, y, w, TOOL_TILE_H, 2, ACCENT_COLOR);
        }

        // Strip "Tool: " prefix for cleaner display
        let display_name = name.strip_prefix("Tool: ").unwrap_or(name);
        let truncated = truncate_to_fit(display_name, w.saturating_sub(20), Font::Small);
        self.fb.draw_text(
            &truncated,
            x as i32 + 10,
            y as i32 + 4,
            TEXT_COLOR,
            Font::Small,
        );
        self.fb.draw_text(
            subtitle,
            x as i32 + 10,
            y as i32 + 22,
            SUBTITLE_COLOR,
            Font::Small,
        );
    }

    // ─── Footer ─────────────────────────────────────────────────

    fn draw_footer(
        &mut self,
        entries: &[BootEntry],
        boot_indices: &[usize],
        tool_indices: &[usize],
        timeout_ms: u32,
        elapsed_frames: u32,
        user_interacted: bool,
    ) {
        let footer_y = self.content_bottom() + 4;

        // Divider
        self.fb
            .fill_rect(MARGIN, footer_y, self.width - 2 * MARGIN, 1, DIVIDER_COLOR);

        // Status message (left side)
        let status = if !user_interacted && timeout_ms > 0 {
            let elapsed_ms = elapsed_frames * 16;
            if elapsed_ms < timeout_ms {
                let remaining = (timeout_ms - elapsed_ms) / 1000;
                format!("Auto-boot in {remaining}s...")
            } else {
                String::from("Booting...")
            }
        } else {
            // Show preflight issue for selected entry
            self.get_selected_status(entries, boot_indices, tool_indices)
        };
        self.fb.draw_text(
            &status,
            MARGIN as i32 + 8,
            footer_y as i32 + 10,
            ACCENT_COLOR,
            Font::Small,
        );

        // Keyboard hints (right side)
        let hints = "Enter: Boot  F1: Help  F2: Setup  F12: Reboot";
        let hx = (self.width - MARGIN) as i32 - (hints.len() as i32 * FONT_SMALL_W as i32);
        self.fb
            .draw_text(hints, hx, footer_y as i32 + 10, SUBTITLE_COLOR, Font::Small);
    }

    fn get_selected_status(
        &self,
        entries: &[BootEntry],
        boot_indices: &[usize],
        tool_indices: &[usize],
    ) -> String {
        let entry_idx = match self.sel_col {
            Column::Left => boot_indices.get(self.sel_idx).copied(),
            Column::Right => {
                if self.sel_idx < tool_indices.len() {
                    tool_indices.get(self.sel_idx).copied()
                } else {
                    None // system action selected
                }
            }
        };

        if let Some(idx) = entry_idx {
            if let Some(ref pf) = entries[idx].preflight {
                if let Some(issue) = pf.first_issue() {
                    return issue.message.clone();
                }
            }
        }

        String::from("Use arrow keys to navigate. Press Enter to boot.")
    }

    // ─── Mouse hit-testing ────────────────────────────────────────

    fn hit_test(
        &self,
        mx: i32,
        my: i32,
        boot_indices: &[usize],
        tool_indices: &[usize],
    ) -> Option<(Column, usize)> {
        let top = HEADER_H as i32;

        // Left column: boot entries
        let lx = MARGIN as i32;
        let lw = self.left_col_w() as i32;
        if mx >= lx && mx < lx + lw {
            let max_vis = self.max_visible_boot().min(boot_indices.len());
            for vi in 0..max_vis {
                let ey = top + (vi as i32) * (BOOT_TILE_H + TILE_GAP) as i32;
                if my >= ey && my < ey + BOOT_TILE_H as i32 {
                    return Some((Column::Left, self.scroll_left + vi));
                }
            }
        }

        // Right column: tools + system actions
        let rx = self.right_col_x() as i32;
        let rw = self.right_col_w() as i32;
        if mx >= rx && mx < rx + rw {
            let mut ry = top as usize;
            for i in 0..tool_indices.len() {
                if my >= ry as i32 && my < (ry + TOOL_TILE_H) as i32 {
                    return Some((Column::Right, i));
                }
                ry += TOOL_TILE_H + TILE_GAP;
            }
            ry += 12; // gap + divider
            for i in 0..2 {
                if my >= ry as i32 && my < (ry + TOOL_TILE_H) as i32 {
                    return Some((Column::Right, tool_indices.len() + i));
                }
                ry += TOOL_TILE_H + TILE_GAP;
            }
        }

        None
    }

    // ─── Shared helpers ─────────────────────────────────────────

    fn execute_system_action(&mut self, action: usize, gop: &mut GraphicsOutput) {
        match action {
            0 => {
                self.fb.clear();
                self.fb.draw_text(
                    "Rebooting to firmware setup...",
                    50,
                    (self.height / 2) as i32,
                    ACCENT_COLOR,
                    Font::Large,
                );
                self.fb.present(gop);
                health::reboot_to_firmware_setup();
            }
            1 => {
                self.fb.clear();
                self.fb.draw_text(
                    "Rebooting...",
                    50,
                    (self.height / 2) as i32,
                    ACCENT_COLOR,
                    Font::Large,
                );
                self.fb.present(gop);
                health::reboot();
            }
            _ => {}
        }
    }

    fn draw_boot_progress(&mut self, name: &str, gop: &mut GraphicsOutput) {
        self.fb.clear();
        let msg = format!("Booting: {name}...");
        let px_width = msg.len() * FONT_LARGE_W;
        let x = (self.width as i32 - px_width as i32) / 2;
        let y = (self.height as i32 - FONT_LARGE_H as i32) / 2;
        self.fb.draw_text(&msg, x, y, TEXT_COLOR, Font::Large);
        self.fb.present(gop);
    }

    fn draw_cursor(&mut self, x: i32, y: i32) {
        let ux = x as usize;
        let uy = y as usize;
        if ux >= 10 {
            self.fb
                .fill_rect(ux - 10, uy.saturating_sub(1), 20, 2, TEXT_COLOR);
        }
        if uy >= 10 {
            self.fb
                .fill_rect(ux.saturating_sub(1), uy - 10, 2, 20, TEXT_COLOR);
        }
    }

    fn show_help(&mut self, gop: &mut GraphicsOutput) {
        self.fb.clear();
        self.fb
            .draw_text("LamBoot Help", 50, 50, ACCENT_COLOR, Font::Large);

        let lines = [
            "Arrow Keys: Navigate entries",
            "Left/Right: Switch columns",
            "Enter: Boot selected entry",
            "Mouse: Click to select and boot",
            "F1: This help screen",
            "F2: Reboot to firmware setup",
            "F12: Reboot system",
            "Esc: Cancel auto-boot timeout",
        ];

        for (i, line) in lines.iter().enumerate() {
            self.fb
                .draw_text(line, 50, (120 + i * 30) as i32, TEXT_COLOR, Font::Small);
        }

        let build_info = format!("LamBoot v{} ({})", crate::VERSION, crate::BUILD_TARGET);
        self.fb
            .draw_text(&build_info, 50, 400, SUBTITLE_COLOR, Font::Small);

        self.fb.draw_text(
            "Press any key to continue...",
            50,
            440,
            ACCENT_COLOR,
            Font::Small,
        );
        self.fb.present(gop);

        self.input.wait_for_key();
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

fn icon_label(icon: Icon) -> &'static str {
    match icon {
        Icon::Windows => "Win",
        Icon::Linux => "Linux",
        Icon::Efi => "EFI",
        Icon::Recovery => "Rcvr",
        Icon::Network => "Net",
        Icon::Tools => "Tool",
    }
}

fn entry_type_label(kind: &EntryKind, icon: Icon) -> &'static str {
    match (kind, icon) {
        (EntryKind::LinuxLegacy { .. }, _) => "Linux kernel + initrd",
        (EntryKind::Uki { .. }, _) => "Unified Kernel Image",
        (EntryKind::Chainload { .. }, Icon::Windows) => "Windows Boot Manager",
        (EntryKind::Chainload { .. }, Icon::Tools) => "Diagnostic module",
        (EntryKind::Chainload { .. }, _) => "EFI bootloader",
    }
}

fn truncate_to_fit(text: &str, max_pixels: usize, font: Font) -> String {
    let char_w = font.width();
    let max_chars = max_pixels / char_w;
    if text.len() <= max_chars {
        return String::from(text);
    }
    let keep = max_chars.saturating_sub(3);
    let mut result = String::with_capacity(keep + 3);
    for ch in text.chars().take(keep) {
        result.push(ch);
    }
    result.push_str("...");
    result
}
