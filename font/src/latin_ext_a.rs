use super::GlyphRows;
use super::latin_1::{Codepoints, parse};

pub(super) const FIRST: u32 = 0x100;
pub(super) const LAST: u32 = 0x17f;
pub(super) const GLYPH_COUNT: usize = (LAST - FIRST + 1) as usize;

pub(super) static LATIN_EXTENDED_A: [GlyphRows; GLYPH_COUNT] = parse(
    include_bytes!("../../docs/ux/font-8x16-latin-ext-a.txt"),
    Codepoints {
        first: FIRST,
        last: LAST,
        then: LAST,
    },
);
