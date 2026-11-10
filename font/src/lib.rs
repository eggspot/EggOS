//! Egg Mono 8x16, the system font (docs/ux/font-8x16.txt, D109; its Latin-1 supplement
//! docs/ux/font-8x16-latin-1.txt, D185): one bitmap face of 8x16 cells, no anti-aliasing. A
//! glyph is addressed by a `u8` glyph id: 0x20 to 0x7E are the printable ASCII characters
//! themselves, 0xA0 to 0xFF the Latin-1 characters themselves, [`EURO`] the euro sign, and the
//! box drawing, block and symbol extras take 0x80 upwards in the order of [`EXTRAS`]; every
//! other id draws `?`. Latin Extended-A (U+0100 to U+017F, docs/ux/font-8x16-latin-ext-a.txt,
//! D221) has no `u8` id: [`glyph_rows`] draws it by character. The desktop, the terminal and the
//! Windows layer's drawing share it (D129). [`egg_sans`] is the proportional UI font of the
//! toolkit (docs/ux/font-ui-16.txt, D132). [`ascii_fold`] and [`contains_folded`] match text
//! without regard to case and accents, as the launcher's and Settings' searches do (D221).

#![cfg_attr(not(test), no_std)]

mod egg_mono;
pub mod egg_sans;
mod latin_1;
mod latin_ext_a;

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

/// The glyph id of the first Latin-1 character, U+00A0; the ids up to 0xFF are the Latin-1
/// characters themselves.
pub const FIRST_LATIN_1: u8 = 0xa0;

/// The glyph id of `€` (U+20AC): the one id below [`FIRST_EXTRA`] that no character of its own
/// takes.
pub const EURO: u8 = 0x7f;

const EURO_CHARACTER: u32 = 0x20ac;

/// The glyph id that draws `character`: itself for printable ASCII and Latin-1, [`EURO`] for
/// the euro sign, an extra's id, else [`REPLACEMENT`].
pub fn glyph_id(character: u32) -> u8 {
    match character {
        0x20..=0x7e | 0xa0..=0xff => character as u8,
        EURO_CHARACTER => EURO,
        0x2500..=0x25ff | 0x2190..=0x2193 => EXTRAS
            .iter()
            .position(|&extra| extra as u32 == character)
            .map_or(REPLACEMENT, |index| FIRST_EXTRA + index as u8),
        _ => REPLACEMENT,
    }
}

/// The character glyph `id` draws, the inverse of [`glyph_id`]: `?` for an id the font has
/// no glyph of its own for.
pub fn character(id: u8) -> char {
    match id {
        0x20..=0x7e | FIRST_LATIN_1..=0xff => char::from(id),
        EURO => '\u{20ac}',
        FIRST_EXTRA..=UP_TRIANGLE => EXTRAS[usize::from(id - FIRST_EXTRA)],
        _ => char::from(REPLACEMENT),
    }
}

/// Whether Egg Mono has a glyph of its own for `character`.
pub fn covers(character: u32) -> bool {
    glyph_id(character) != REPLACEMENT
        || character == u32::from(REPLACEMENT)
        || (latin_ext_a::FIRST..=latin_ext_a::LAST).contains(&character)
}

/// The rows that draw `character`: those of its glyph id ([`glyph_id`]), or its Latin
/// Extended-A glyph, which has no id (D221).
pub fn glyph_rows(character: u32) -> &'static GlyphRows {
    match character {
        latin_ext_a::FIRST..=latin_ext_a::LAST => {
            &latin_ext_a::LATIN_EXTENDED_A[(character - latin_ext_a::FIRST) as usize]
        }
        _ => rows(glyph_id(character)),
    }
}

const FIRST_FOLDED: u32 = 0xc0;
const LAST_FOLDED: u32 = 0x17f;

static FOLDS: &[u8; 384] = b"a a a a a a aec e e e e i i i i d n o o o o o   o u u u u y thss\
a a a a a a aec e e e e i i i i d n o o o o o   o u u u u y thy \
a a a a a a c c c c c c c c d d d d e e e e e e e e e e g g g g \
g g g g h h h h i i i i i i i i i i ijijj j k k k l l l l l l l \
l l l n n n n n n n n n o o o o o o oeoer r r r r r s s s s s s \
s s t t t t t t u u u u u u u u u u u u w w y y y z z z z z z s ";

/// A character folded for matching ([`ascii_fold`]): one or two lower-case ASCII letters, or
/// the character itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Folded {
    bytes: [u8; 4],
    length: u8,
}

impl Folded {
    /// The folded text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    /// The folded text's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

/// `character` folded for matching without regard to case and accents (D221): every letter
/// the fonts draw from U+00C0 to U+017F to one or two lower-case ASCII letters (`É` to `e`,
/// `ß` to `ss`, `Œ` to `oe`, `Ł` to `l`), ASCII capitals to small letters, anything else
/// unchanged.
pub fn ascii_fold(character: char) -> Folded {
    let code = character as u32;
    let mut bytes = [0u8; 4];
    if character.is_ascii() {
        bytes[0] = (code as u8).to_ascii_lowercase();
        return Folded { bytes, length: 1 };
    }
    if (FIRST_FOLDED..=LAST_FOLDED).contains(&code) {
        let at = 2 * (code - FIRST_FOLDED) as usize;
        let fold = [FOLDS[at], FOLDS[at + 1]];
        if fold[0] != b' ' {
            bytes[..2].copy_from_slice(&fold);
            let length = if fold[1] == b' ' { 1 } else { 2 };
            return Folded { bytes, length };
        }
    }
    let length = character.encode_utf8(&mut bytes).len() as u8;
    Folded { bytes, length }
}

fn folded(text: &str) -> impl Iterator<Item = u8> + Clone + '_ {
    text.chars().flat_map(|character| {
        let fold = ascii_fold(character);
        (0..fold.length).map(move |index| fold.bytes[usize::from(index)])
    })
}

/// Whether `query` is in `text` once both are folded ([`ascii_fold`]): `e` finds `Éditeur`,
/// `strasse` finds `Straße`, and `é` finds `Text Editor`. Allocates nothing.
pub fn contains_folded(text: &str, query: &str) -> bool {
    let mut from = folded(text);
    loop {
        let mut candidate = from.clone();
        if folded(query).all(|byte| candidate.next() == Some(byte)) {
            return true;
        }
        if from.next().is_none() {
            return false;
        }
    }
}

/// `a` and `b` in the order of their folded texts ([`ascii_fold`]), then of their bytes, so
/// `Édition` sorts under E and the order is total.
pub fn cmp_folded(a: &str, b: &str) -> core::cmp::Ordering {
    folded(a).cmp(folded(b)).then_with(|| a.cmp(b))
}

/// The rows of glyph `id`; those of `?` for an id the font does not have.
pub fn rows(id: u8) -> &'static GlyphRows {
    match id {
        0x20..=0x7e => &egg_mono::ASCII[usize::from(id - 0x20)],
        FIRST_EXTRA..=UP_TRIANGLE => &egg_mono::EXTRAS[usize::from(id - FIRST_EXTRA)],
        FIRST_LATIN_1..=0xff => &latin_1::LATIN_1[usize::from(id - FIRST_LATIN_1)],
        EURO => &latin_1::LATIN_1[latin_1::GLYPH_COUNT - 1],
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
