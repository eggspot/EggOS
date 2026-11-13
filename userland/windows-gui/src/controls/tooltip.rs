//! comctl32's tooltip (D355, `docs/ux/windows-tooltips-tray-v0.5.md` sections 3 to 5 and 7):
//! a tip's lines (one line until `TTM_SETMAXTIPWIDTH`, then wrapped at spaces), its size, its
//! title row and icon, where it goes in the window's content, and its painting.

use super::{Palette, framed};
use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_HEIGHT, CELL_WIDTH};
use eggos_font as font;

/// The most lines a tip shows.
pub const MOST_LINES: usize = 16;
const PADDING: i32 = 8;
const TEXT_TOP: i32 = 4;
const TITLE_GAP: i32 = 4;
const ICON_BOX: i32 = 16;
const ICON_GAP: i32 = 4;
const POINTER_DROP: i32 = 20;
const TOOL_GAP: i32 = 4;
const MINIMUM_WIDTH: i32 = 24;
const CIRCLE_14: [i32; 5] = [4, 2, 1, 1, 0];
const SPACE: u16 = 0x20;
const LINE_FEED: u16 = 0x0a;
const CARRIAGE_RETURN: u16 = 0x0d;
const AMPERSAND: u16 = 0x26;
const LIGHT_FILL: u32 = 0x00ff_ffff;
const LIGHT_BORDER: u32 = 0x0076_7676;
const LIGHT_TEXT: u32 = 0x0000_0000;
const DARK_FILL: u32 = 0x001c_2430;
const DARK_BORDER: u32 = 0x002a_3340;
const DARK_TEXT: u32 = 0x00e6_eaf0;
const INFO: u32 = 0x0000_78d7;
const WARNING: u32 = 0x00ff_b900;
const ERROR: u32 = 0x00e8_1123;

/// The icon of a titled tip (`TTI_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TipIcon {
    #[default]
    None,
    Info,
    Warning,
    Error,
}

impl TipIcon {
    /// `TTM_SETTITLEW`'s `wParam`: the standard icons and their large forms.
    pub fn of(value: usize) -> TipIcon {
        match value {
            1 | 4 => TipIcon::Info,
            2 | 5 => TipIcon::Warning,
            3 | 6 => TipIcon::Error,
            _ => TipIcon::None,
        }
    }
}

/// What a tip says: its text, its title and icon, and the width it wraps at (-1: one line).
#[derive(Debug, Clone, Copy)]
pub struct TipText<'a> {
    pub text: &'a [u16],
    pub title: &'a [u16],
    pub icon: TipIcon,
    pub max_width: i32,
}

/// A tip's lines (ranges of its text) and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipLayout {
    pub width: i32,
    pub height: i32,
    lines: [(usize, usize); MOST_LINES],
    count: usize,
}

impl TipLayout {
    /// The text's lines, as ranges of its units.
    pub fn lines(&self) -> &[(usize, usize)] {
        &self.lines[..self.count]
    }
}

fn is_break(unit: u16) -> bool {
    unit == LINE_FEED || unit == CARRIAGE_RETURN
}

/// Lays out `tip`: with no maximum width one line (breaks drawn as spaces); with one, lines
/// broken at `\n` and wrapped at spaces within `max_width / 8` cells, at most 16.
pub fn layout(tip: &TipText<'_>) -> TipLayout {
    let mut lines = [(0, 0); MOST_LINES];
    let mut count = 0;
    let text = tip.text;
    match tip.max_width {
        width if width < 0 => {
            lines[0] = (0, text.len());
            count = usize::from(!text.is_empty());
        }
        width => {
            let room = (width / CELL_WIDTH).max(1) as usize;
            let mut start = 0;
            while start < text.len() && count < MOST_LINES {
                let paragraph_end = (start..text.len())
                    .find(|&at| text[at] == LINE_FEED)
                    .unwrap_or(text.len());
                let mut end = paragraph_end;
                while end > start && text[end - 1] == CARRIAGE_RETURN {
                    end -= 1;
                }
                let (line_end, next) = match end - start <= room {
                    true => (end, paragraph_end + 1),
                    false => {
                        let limit = start + room;
                        let cut = (start + 1..=limit)
                            .rev()
                            .find(|&at| text[at] == SPACE)
                            .unwrap_or(limit);
                        let skip = usize::from(text.get(cut) == Some(&SPACE));
                        (cut, cut + skip)
                    }
                };
                lines[count] = (start, line_end);
                count += 1;
                start = next;
            }
        }
    }
    let widest = lines[..count]
        .iter()
        .map(|&(start, end)| shown_cells(&text[start..end]))
        .max()
        .unwrap_or(0) as i32;
    let titled = !tip.title.is_empty() || tip.icon != TipIcon::None;
    let title_width = match tip.icon {
        TipIcon::None => tip.title.len() as i32 * CELL_WIDTH,
        _ => ICON_BOX + ICON_GAP + tip.title.len() as i32 * CELL_WIDTH,
    };
    let text_height = count as i32 * CELL_HEIGHT;
    let (width, height) = match titled {
        true => (
            2 * PADDING + title_width.max(widest * CELL_WIDTH),
            2 * TEXT_TOP + CELL_HEIGHT + TITLE_GAP + text_height,
        ),
        false => (
            2 * PADDING + widest * CELL_WIDTH,
            2 * TEXT_TOP + text_height,
        ),
    };
    TipLayout {
        width: width.max(MINIMUM_WIDTH),
        height: height.max(2 * TEXT_TOP + CELL_HEIGHT),
        lines,
        count,
    }
}

fn shown_cells(line: &[u16]) -> usize {
    line.iter().filter(|&&unit| unit != CARRIAGE_RETURN).count()
}

/// Where a tip goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Under the pointer, 20 down, flipped above when it does not fit.
    Pointer((i32, i32)),
    /// Centred on the tool, below it (above for a balloon, `above_first`).
    Centred { tool: Rect, above_first: bool },
    /// Exactly at a point (`TTF_TRACK` with `TTF_ABSOLUTE`), not clamped.
    Absolute((i32, i32)),
}

/// The tip's rectangle in content `width` by `height`, for a tip `size` placed as `placement`.
pub fn place(placement: Placement, size: (i32, i32), content: (i32, i32)) -> Rect {
    let (w, h) = size;
    let (cw, ch) = content;
    let (mut x, mut y) = match placement {
        Placement::Absolute(at) => return Rect::sized(at.0, at.1, w, h),
        Placement::Pointer((px, py)) => {
            let mut y = py + POINTER_DROP;
            if y + h > ch {
                y = py - TOOL_GAP - h;
            }
            (px, y)
        }
        Placement::Centred { tool, above_first } => {
            let x = tool.left + (tool.width() - w) / 2;
            let below = tool.bottom + TOOL_GAP;
            let above = tool.top - TOOL_GAP - h;
            let y = match above_first {
                true if above >= 0 => above,
                true => below,
                false if below + h > ch => above,
                false => below,
            };
            (x, y)
        }
    };
    if x + w > cw {
        x = cw - w;
    }
    x = x.max(0);
    y = y.max(0);
    Rect::sized(x, y, w, h)
}

fn colours(palette: &Palette) -> (u32, u32, u32) {
    match palette.rounded {
        true => (DARK_FILL, DARK_BORDER, DARK_TEXT),
        false => (LIGHT_FILL, LIGHT_BORDER, LIGHT_TEXT),
    }
}

fn draw_units(surface: &mut Surface<'_>, (x, y): (i32, i32), units: &[u16], colour: u32) {
    for (column, character) in (0..).zip(char::decode_utf16(units.iter().copied())) {
        let id = match character {
            Ok(character) if is_break(character as u16) && (character as u32) < 0x80 => {
                font::glyph_id(u32::from(' '))
            }
            Ok(character) => font::glyph_id(u32::from(character)),
            Err(_) => font::REPLACEMENT,
        };
        text::glyph(surface, x + column * CELL_WIDTH, y, id, false, colour, None);
    }
}

fn draw_icon(surface: &mut Surface<'_>, (x, y): (i32, i32), icon: TipIcon, glyph: u32) {
    let (fill, mark) = match icon {
        TipIcon::Info => (INFO, 'i'),
        TipIcon::Warning => (WARNING, '!'),
        TipIcon::Error => (ERROR, 'x'),
        TipIcon::None => return,
    };
    surface.rounded(Rect::sized(x + 1, y + 1, 14, 14), &CIRCLE_14, fill);
    let id = font::glyph_id(u32::from(mark));
    text::glyph(surface, x + 4, y, id, false, glyph, None);
}

/// Strips `&` as a mnemonic would (`&&` is `&`) into `out`: the units a tip without
/// `TTS_NOPREFIX` shows.
pub fn without_prefixes<'a>(text: &[u16], out: &'a mut [u16]) -> &'a [u16] {
    let mut length = 0;
    let mut at = 0;
    while at < text.len() && length < out.len() {
        let unit = text[at];
        at += 1;
        if unit == AMPERSAND && at < text.len() {
            out[length] = text[at];
            at += 1;
        } else if unit != AMPERSAND {
            out[length] = unit;
        } else {
            continue;
        }
        length += 1;
    }
    &out[..length]
}

/// Paints the tip `tip`, laid out as `layout`, at `rect`; a balloon has rounded corners in
/// both palettes.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    tip: &TipText<'_>,
    layout: &TipLayout,
    balloon: bool,
    palette: &Palette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    let (fill, border, ink) = colours(palette);
    framed(surface, rect, palette.rounded || balloon, border, fill);
    let titled = !tip.title.is_empty() || tip.icon != TipIcon::None;
    let mut top = rect.top + TEXT_TOP;
    if titled {
        let mut left = rect.left + PADDING;
        if tip.icon != TipIcon::None {
            let glyph = if palette.rounded {
                0x000b_0e13
            } else {
                0x00ff_ffff
            };
            draw_icon(surface, (left, top), tip.icon, glyph);
            left += ICON_BOX + ICON_GAP;
        }
        draw_units(surface, (left, top), tip.title, ink);
        top += CELL_HEIGHT + TITLE_GAP;
    }
    for (row, &(start, end)) in layout.lines().iter().enumerate() {
        let y = top + row as i32 * CELL_HEIGHT;
        draw_units(
            surface,
            (rect.left + PADDING, y),
            &tip.text[start..end],
            ink,
        );
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn sized(text: &str, max_width: i32) -> (i32, i32) {
        let text = units(text);
        let tip = TipText {
            text: &text,
            title: &[],
            icon: TipIcon::None,
            max_width,
        };
        let laid = layout(&tip);
        (laid.width, laid.height)
    }

    #[test]
    fn tips_are_sized_as_the_design_s_table_says() {
        assert_eq!(sized("Save", -1), (48, 24));
        assert_eq!(sized("Save (Ctrl+S)", -1), (120, 24));
        assert_eq!(sized("Line one\nLine two", -1), (152, 24));
        assert_eq!(sized("Line one\nLine two", 80), (80, 40));
        assert_eq!(sized("A long sentence that wraps", 96), (96, 56));
    }

    #[test]
    fn a_title_with_an_icon_adds_its_row() {
        let text = units("Saved");
        let title = units("Done");
        let tip = TipText {
            text: &text,
            title: &title,
            icon: TipIcon::Info,
            max_width: -1,
        };
        let laid = layout(&tip);
        assert_eq!((laid.width, laid.height), (8 + 20 + 32 + 8, 44));
    }

    #[test]
    fn a_tip_goes_under_the_pointer_and_flips_above_at_the_bottom() {
        let below = place(Placement::Pointer((10, 10)), (48, 24), (200, 100));
        assert_eq!(below, Rect::sized(10, 30, 48, 24));
        let flipped = place(Placement::Pointer((190, 90)), (48, 24), (200, 100));
        assert_eq!(flipped, Rect::sized(152, 62, 48, 24));
        let tool = Rect::sized(50, 50, 20, 20);
        let balloon = Placement::Centred {
            tool,
            above_first: true,
        };
        assert_eq!(
            place(balloon, (40, 24), (200, 200)),
            Rect::sized(40, 22, 40, 24)
        );
        let exact = place(Placement::Absolute((-5, 3)), (40, 24), (200, 200));
        assert_eq!(exact, Rect::sized(-5, 3, 40, 24));
    }

    #[test]
    fn mnemonic_ampersands_are_stripped() {
        let mut out = [0u16; 16];
        let shown = without_prefixes(&units("&Save && Close"), &mut out);
        assert_eq!(shown, &units("Save & Close")[..]);
    }

    #[test]
    fn a_light_tip_is_white_with_a_grey_border() {
        let text = units("Save");
        let tip = TipText {
            text: &text,
            title: &[],
            icon: TipIcon::None,
            max_width: -1,
        };
        let laid = layout(&tip);
        let mut pixels = vec![0u32; 64 * 32];
        let mut surface = Surface::new(&mut pixels, 64, 32).unwrap();
        paint(
            &mut surface,
            Rect::sized(0, 0, 48, 24),
            &tip,
            &laid,
            false,
            &LIGHT,
        );
        assert_eq!(surface.get(0, 0), Some(LIGHT_BORDER));
        assert_eq!(surface.get(47, 12), Some(LIGHT_BORDER));
        assert_eq!(surface.get(4, 12), Some(LIGHT_FILL));
    }
}
