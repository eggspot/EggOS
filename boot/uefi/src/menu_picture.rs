//! The boot menu's picture (D653; installer dual boot v0.1 section 6, "Look"): where each row,
//! name, marker and the hint go on a screen, in which colours, drawn with Egg Mono 8x16 into
//! any [`Canvas`]. The loader's canvas is the framebuffer; the host tests use a recording one.

use eggos_font::{GLYPH_HEIGHT, GLYPH_WIDTH};

use crate::menu::{Menu, OUR_DESCRIPTION};

pub const ROW_WIDTH: u32 = 400;
pub const ROW_HEIGHT: u32 = 40;
pub const NAME_INSET: u32 = 40;
pub const MARKER_INSET: u32 = 16;
pub const TEXT_TOP: u32 = 12;
pub const FIRST_ROW_ABOVE_MIDDLE: u32 = 96;
pub const TITLE_ABOVE_MIDDLE: u32 = 160;
pub const HINT_GAP: u32 = 16;
pub const DOUBLED_FROM_WIDTH: u32 = 2560;

/// Where pixels go: a rectangle filled with one `0xRRGGBB` colour.
pub trait Canvas {
    fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, colour: u32);
}

/// The menu's colours, `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub text: u32,
    pub dim: u32,
    pub selected: u32,
    pub selected_outline: Option<u32>,
    pub marker: u32,
}

/// The boot screen's colours, or high contrast's.
pub fn palette(high_contrast: bool) -> Palette {
    match high_contrast {
        false => Palette {
            background: 0x0E_17_26,
            text: 0xE6_EA_F0,
            dim: 0x8A_94_A6,
            selected: 0x2E_4A_6B,
            selected_outline: None,
            marker: 0xF2_B3_3D,
        },
        true => Palette {
            background: 0x00_00_00,
            text: 0xFF_FF_FF,
            dim: 0xB8_C0_CC,
            selected: 0x15_3A_6B,
            selected_outline: Some(0xFF_FF_FF),
            marker: 0xFF_D2_4D,
        },
    }
}

/// 2 on a screen 2560 wide or more, which doubles everything; else 1.
pub fn scale_of(width: u32) -> u32 {
    if width >= DOUBLED_FROM_WIDTH { 2 } else { 1 }
}

/// Row `index`'s top-left corner.
pub fn row_origin(width: u32, height: u32, index: usize) -> (u32, u32) {
    let scale = scale_of(width);
    let x = width.saturating_sub(ROW_WIDTH * scale) / 2;
    let first = (height / 2).saturating_sub(FIRST_ROW_ABOVE_MIDDLE * scale);
    (x, first + index as u32 * ROW_HEIGHT * scale)
}

fn text_width(text: &str, size: u32) -> u32 {
    text.len() as u32 * GLYPH_WIDTH as u32 * size
}

fn draw_text(canvas: &mut impl Canvas, x: u32, y: u32, text: &str, colour: u32, size: u32) -> u32 {
    let mut left = x;
    for byte in text.bytes() {
        let rows = eggos_font::rows(byte);
        for (row, bits) in rows.iter().enumerate().take(GLYPH_HEIGHT) {
            for column in 0..GLYPH_WIDTH {
                if bits & (0x80 >> column) != 0 {
                    canvas.fill(left + column as u32 * size, y + row as u32 * size, size, size, colour);
                }
            }
        }
        left += GLYPH_WIDTH as u32 * size;
    }
    left
}

fn outline(canvas: &mut impl Canvas, x: u32, y: u32, width: u32, height: u32, colour: u32) {
    canvas.fill(x, y, width, 1, colour);
    canvas.fill(x, y + height - 1, width, 1, colour);
    canvas.fill(x, y, 1, height, colour);
    canvas.fill(x + width - 1, y, 1, height, colour);
}

/// Draws the whole menu on a `width` x `height` screen.
pub fn draw(menu: &Menu, width: u32, height: u32, canvas: &mut impl Canvas) {
    let colours = palette(menu.settings.high_contrast);
    let scale = scale_of(width);
    canvas.fill(0, 0, width, height, colours.background);
    let title_size = 2 * scale;
    let title_x = width.saturating_sub(text_width(OUR_DESCRIPTION, title_size)) / 2;
    let title_y = (height / 2).saturating_sub(TITLE_ABOVE_MIDDLE * scale);
    draw_text(canvas, title_x, title_y, OUR_DESCRIPTION, colours.text, title_size);
    let mut count = 0;
    for (index, entry) in menu.entries().enumerate() {
        count += 1;
        let (x, y) = row_origin(width, height, index);
        let text_y = y + TEXT_TOP * scale;
        if index == menu.selected {
            canvas.fill(x, y, ROW_WIDTH * scale, ROW_HEIGHT * scale, colours.selected);
            if let Some(line) = colours.selected_outline {
                outline(canvas, x, y, ROW_WIDTH * scale, ROW_HEIGHT * scale, line);
            }
            draw_text(canvas, x + MARKER_INSET * scale, text_y, ">", colours.marker, scale);
        }
        let after = draw_text(canvas, x + NAME_INSET * scale, text_y, entry.name.as_str(), colours.text, scale);
        if index == menu.default {
            draw_text(canvas, after, text_y, " (default)", colours.dim, scale);
        }
    }
    let (_, below) = row_origin(width, height, count);
    let hint = menu.hint();
    let hint_x = width.saturating_sub(text_width(hint.as_str(), scale)) / 2;
    draw_text(canvas, hint_x, below + HINT_GAP * scale, hint.as_str(), colours.dim, scale);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Key, Settings};

    #[derive(Default)]
    struct Recording {
        fills: Vec<(u32, u32, u32, u32, u32)>,
    }

    impl Canvas for Recording {
        fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, colour: u32) {
            self.fills.push((x, y, width, height, colour));
        }
    }

    fn three_entries() -> Menu {
        let mut menu = Menu::new(Settings::new());
        menu.add_firmware_settings();
        menu.add_firmware_settings();
        menu.ready();
        menu
    }

    #[test]
    fn rows_sit_where_section_6_puts_them_at_1280_by_800() {
        assert_eq!(row_origin(1280, 800, 0), (440, 304));
        assert_eq!(row_origin(1280, 800, 1), (440, 344));
        assert_eq!(row_origin(1280, 800, 2), (440, 384));
        assert_eq!(row_origin(1280, 800, 3).1 + HINT_GAP, 440);
        assert_eq!(row_origin(2560, 1600, 0), (880, 608));
    }

    #[test]
    fn the_picture_fills_the_background_then_the_selected_row() {
        let mut menu = three_entries();
        let mut canvas = Recording::default();
        draw(&menu, 1280, 800, &mut canvas);
        assert_eq!(canvas.fills[0], (0, 0, 1280, 800, 0x0E_17_26));
        assert!(canvas.fills.contains(&(440, 304, 400, 40, 0x2E_4A_6B)));
        assert!(canvas.fills.iter().any(|fill| fill.4 == 0xF2_B3_3D && fill.1 >= 316 && fill.1 < 332));
        assert!(canvas.fills.iter().any(|fill| fill.4 == 0x8A_94_A6 && fill.1 >= 440));
        menu.press(Key::Down);
        menu.settings.high_contrast = true;
        let mut canvas = Recording::default();
        draw(&menu, 1280, 800, &mut canvas);
        assert_eq!(canvas.fills[0], (0, 0, 1280, 800, 0));
        assert!(canvas.fills.contains(&(440, 344, 400, 40, 0x15_3A_6B)));
        assert!(canvas.fills.contains(&(440, 344, 400, 1, 0xFF_FF_FF)));
    }
}
