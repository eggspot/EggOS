//! Egg Mono 8x16, the system font (docs/ux/font-8x16.txt, D109): one bitmap face of 8x16 cells,
//! no anti-aliasing. A glyph is addressed by a `u8` glyph id: 0x20 to 0x7E are the printable
//! ASCII characters themselves, and the box drawing, block and symbol extras take 0x80 upwards in
//! the order of [`EXTRAS`]; every other id draws `?`. The desktop, the terminal and the
//! Windows layer's drawing share it (D129). [`egg_sans`] is the proportional UI font of the
//! toolkit (docs/ux/font-ui-16.txt, D132).

#![cfg_attr(not(test), no_std)]

mod egg_mono;
pub mod egg_sans;

#[cfg(test)]
mod tests;

/// Columns of a glyph cell.
pub const GLYPH_WIDTH: usize = 8;
/// Rows of a glyph cell.
pub const GLYPH_HEIGHT: usize = 16;
/// The row an underline is drawn on.
pub const UNDERLINE_ROW: usize = 14;

/// A glyph's rows, top first; bit 7 of each byte is column 0.
pub type GlyphRows = [u8; GLYPH_HEIGHT];

/// The glyph id of the first extra.
pub const FIRST_EXTRA: u8 = 0x80;

/// The glyph id drawn for a character the font does not have.
pub const REPLACEMENT: u8 = b'?';

/// The characters beyond ASCII the font draws, in glyph id order from [`FIRST_EXTRA`]: the
/// design's box drawing, blocks, shapes and arrows, then `▲`, the terminal's scrollback mark.
pub const EXTRAS: [char; 32] = [
    '─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '═', '║', '╔', '╗', '╚', '╝', '█', '▀',
    '▄', '░', '▒', '▓', '▪', '●', '○', '◆', '→', '←', '↑', '↓', '▲',
];

/// The glyph id of `▲`.
pub const UP_TRIANGLE: u8 = FIRST_EXTRA + 31;

/// The glyph id that draws `character`: itself for printable ASCII, an extra's id, else
/// [`REPLACEMENT`].
pub fn glyph_id(character: u32) -> u8 {
    match character {
        0x20..=0x7e => character as u8,
        0x2500..=0x25ff | 0x2190..=0x2193 => EXTRAS
            .iter()
            .position(|&extra| extra as u32 == character)
            .map_or(REPLACEMENT, |index| FIRST_EXTRA + index as u8),
        _ => REPLACEMENT,
    }
}

/// The rows of glyph `id`; those of `?` for an id the font does not have.
pub fn rows(id: u8) -> &'static GlyphRows {
    match id {
        0x20..=0x7e => &egg_mono::ASCII[usize::from(id - 0x20)],
        FIRST_EXTRA..=UP_TRIANGLE => &egg_mono::EXTRAS[usize::from(id - FIRST_EXTRA)],
        _ => &egg_mono::ASCII[usize::from(REPLACEMENT - 0x20)],
    }
}

/// Draws glyph `id` in `foreground` on `background` into the 8x16 cell whose top-left pixel is
/// `pixels[at]`, rows `stride` pixels apart, with an underline when `underline` says so. The
/// cell must lie inside `pixels`.
#[inline]
pub fn draw_glyph(
    pixels: &mut [u32],
    stride: usize,
    at: usize,
    id: u8,
    foreground: u32,
    background: u32,
    underline: bool,
) {
    let glyph = rows(id);
    for (row, &bits) in glyph.iter().enumerate() {
        let bits = if underline && row == UNDERLINE_ROW {
            0xff
        } else {
            bits
        };
        let start = at + row * stride;
        let line = &mut pixels[start..start + GLYPH_WIDTH];
        for (column, pixel) in line.iter_mut().enumerate() {
            *pixel = if bits & (0x80 >> column) != 0 {
                foreground
            } else {
                background
            };
        }
    }
}
