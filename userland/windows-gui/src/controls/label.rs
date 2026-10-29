//! `STATIC` text (section 4 of the design): the whole rectangle filled, then the text as
//! `DrawTextW` lays it out for the style.

use super::Palette;
use crate::surface::{Rect, Surface};
use crate::text::{self, Cell, format};

/// The most cells a label shows.
pub const MAX_CELLS: usize = 1024;

/// `STATIC`'s styles this layer reads.
pub mod style {
    pub const TYPE: u32 = 0x1f;
    pub const CENTER: u32 = 0x1;
    pub const RIGHT: u32 = 0x2;
    pub const SIMPLE: u32 = 0xb;
    pub const LEFT_NO_WORD_WRAP: u32 = 0xc;
    pub const NO_PREFIX: u32 = 0x80;
    pub const NOTIFY: u32 = 0x100;
}

/// How a label looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelLook {
    pub style: u32,
    pub enabled: bool,
}

/// The `DrawTextW` format a label's style asks for.
pub fn format_of(style: u32) -> u32 {
    let alignment = match style & style::TYPE {
        style::CENTER => format::CENTER,
        style::RIGHT => format::RIGHT,
        _ => 0,
    };
    let lines = match style & style::TYPE {
        style::SIMPLE | style::LEFT_NO_WORD_WRAP => format::SINGLELINE,
        _ => format::WORDBREAK,
    };
    let prefix = match style & style::NO_PREFIX {
        0 => 0,
        _ => format::NOPREFIX,
    };
    alignment | lines | prefix | format::END_ELLIPSIS
}

/// Paints the label `units` over the whole of `rect`.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    units: &[u16],
    look: LabelLook,
    palette: &Palette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, palette.surface);
    let flags = format_of(look.style);
    let mut cells = [Cell::Space; MAX_CELLS];
    let count = text::decode(units, text::reading(flags), &mut cells);
    let colour = match look.enabled {
        true => palette.text,
        false => palette.text_disabled,
    };
    text::draw_text(surface, &cells[..count], rect, flags, colour, None);
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    #[test]
    fn a_label_fills_its_rectangle_and_draws_its_text_from_the_top_left() {
        let mut pixels = vec![0u32; 64 * 24];
        let mut surface = Surface::new(&mut pixels, 64, 24).unwrap();
        let units: Vec<u16> = "Name:".encode_utf16().collect();
        let look = LabelLook {
            style: 0,
            enabled: true,
        };
        paint(&mut surface, Rect::new(0, 0, 64, 24), &units, look, &LIGHT);
        assert_eq!(surface.get(63, 23), Some(0x00f0_f0f0));
        assert!((0..8).any(|x| (0..16).any(|y| surface.get(x, y) == Some(0))));
        assert!((0..64).all(|x| surface.get(x, 20) == Some(0x00f0_f0f0)));
    }

    #[test]
    fn styles_choose_alignment_wrapping_and_prefixes() {
        assert_eq!(format_of(0), format::WORDBREAK | format::END_ELLIPSIS);
        assert_eq!(
            format_of(style::CENTER | style::NO_PREFIX),
            format::CENTER | format::WORDBREAK | format::NOPREFIX | format::END_ELLIPSIS
        );
        assert_eq!(
            format_of(style::LEFT_NO_WORD_WRAP),
            format::SINGLELINE | format::END_ELLIPSIS
        );
    }
}
