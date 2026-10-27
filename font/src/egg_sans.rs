//! Egg Sans 16, the UI font (docs/ux/font-ui-16.txt, docs/ux/toolkit-v0.3.md): one
//! proportional bitmap face of 16 rows, no anti-aliasing, printable ASCII and Latin-1. The
//! design file is the source: its glyphs are read from it while the crate compiles, so a change
//! to the drawing is a change to the file alone.

/// Rows of a glyph cell.
pub const HEIGHT: usize = 16;
/// The row the letters stand on, shared with Egg Mono.
pub const BASELINE: usize = 12;
/// The widest advance a glyph may have.
pub const WIDEST: usize = 16;
/// The glyphs the face holds: U+0020 to U+007E, then U+00A0 to U+00FF.
pub const GLYPH_COUNT: usize = 191;

const ASCII_GLYPHS: usize = 95;
const FIRST_LATIN_1: u32 = 0xa0;
const REPLACEMENT: u32 = '?' as u32;

/// One glyph: its advance, which includes the space after it, and its rows, top first; bit 15
/// of each row is column 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    /// How far the next glyph starts to the right, in pixels.
    pub advance: u8,
    /// The ink of each row.
    pub rows: [u16; HEIGHT],
}

impl Glyph {
    /// Whether the pixel at `column`, `row` is ink.
    pub const fn is_ink(&self, column: usize, row: usize) -> bool {
        column < WIDEST && row < HEIGHT && self.rows[row] & (0x8000 >> column) != 0
    }
}

static GLYPHS: [Glyph; GLYPH_COUNT] = parse(include_bytes!("../../docs/ux/font-ui-16.txt"));

/// The glyph that draws `character`: its own for ASCII and Latin-1, else `?`'s.
pub fn glyph(character: char) -> &'static Glyph {
    &GLYPHS[index(character as u32).unwrap_or(index(REPLACEMENT).unwrap_or(0))]
}

/// Whether the face has a glyph of its own for `character`.
pub fn covers(character: char) -> bool {
    index(character as u32).is_some()
}

/// The width of `text` set in Egg Sans: the sum of its glyphs' advances.
pub fn width(text: &str) -> u32 {
    text.chars()
        .map(|character| u32::from(glyph(character).advance))
        .sum()
}

const fn index(codepoint: u32) -> Option<usize> {
    match codepoint {
        0x20..=0x7e => Some((codepoint - 0x20) as usize),
        FIRST_LATIN_1..=0xff => Some((codepoint - FIRST_LATIN_1) as usize + ASCII_GLYPHS),
        _ => None,
    }
}

const fn codepoint_at(index: usize) -> u32 {
    if index < ASCII_GLYPHS {
        index as u32 + 0x20
    } else {
        (index - ASCII_GLYPHS) as u32 + FIRST_LATIN_1
    }
}

const fn line_end(text: &[u8], start: usize) -> usize {
    let mut at = start;
    while at < text.len() && text[at] != b'\n' {
        at += 1;
    }
    at
}

const fn content_end(text: &[u8], start: usize, end: usize) -> usize {
    if end > start && text[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

const fn hex_digit(byte: u8) -> u32 {
    match byte {
        b'0'..=b'9' => (byte - b'0') as u32,
        b'A'..=b'F' => (byte - b'A') as u32 + 10,
        _ => panic!("font-ui-16.txt: a glyph header's codepoint is not 4 uppercase hex digits"),
    }
}

const fn header_codepoint(text: &[u8], start: usize) -> u32 {
    let mut value = 0;
    let mut digit = 0;
    while digit < 4 {
        value = value * 16 + hex_digit(text[start + 2 + digit]);
        digit += 1;
    }
    value
}

const fn header_advance(text: &[u8], start: usize, end: usize) -> u8 {
    let mut digits_start = end;
    while digits_start > start && text[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    assert!(
        digits_start < end && digits_start > start && text[digits_start - 1] == b' ',
        "font-ui-16.txt: a glyph header does not end with its advance"
    );
    let mut value = 0usize;
    let mut at = digits_start;
    while at < end {
        value = value * 10 + (text[at] - b'0') as usize;
        at += 1;
    }
    assert!(
        value >= 1 && value <= WIDEST,
        "font-ui-16.txt: an advance is not 1 to 16"
    );
    value as u8
}

const fn is_header(text: &[u8], start: usize, end: usize) -> bool {
    end >= start + 2 && text[start] == b'U' && text[start + 1] == b'+'
}

const fn parse(text: &[u8]) -> [Glyph; GLYPH_COUNT] {
    let mut glyphs = [Glyph {
        advance: 0,
        rows: [0; HEIGHT],
    }; GLYPH_COUNT];
    let mut count = 0;
    let mut start = 0;
    while start < text.len() {
        let end = content_end(text, start, line_end(text, start));
        if !is_header(text, start, end) {
            start = line_end(text, start) + 1;
            continue;
        }
        assert!(count < GLYPH_COUNT, "font-ui-16.txt: more than 191 glyphs");
        assert!(
            header_codepoint(text, start) == codepoint_at(count),
            "font-ui-16.txt: a glyph is out of codepoint order"
        );
        let advance = header_advance(text, start, end);
        let mut rows = [0u16; HEIGHT];
        let mut row = 0;
        start = line_end(text, start) + 1;
        while row < HEIGHT {
            assert!(
                start < text.len(),
                "font-ui-16.txt: a glyph has fewer than 16 rows"
            );
            let end = content_end(text, start, line_end(text, start));
            assert!(
                end - start == advance as usize,
                "font-ui-16.txt: a row is not as wide as its glyph's advance"
            );
            let mut column = 0;
            while column < advance as usize {
                match text[start + column] {
                    b'#' => rows[row] |= 0x8000 >> column,
                    b'.' => {}
                    _ => panic!("font-ui-16.txt: a row holds something other than `.` and `#`"),
                }
                column += 1;
            }
            row += 1;
            start = line_end(text, start) + 1;
        }
        glyphs[count] = Glyph { advance, rows };
        count += 1;
    }
    assert!(
        count == GLYPH_COUNT,
        "font-ui-16.txt: fewer than 191 glyphs"
    );
    glyphs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(character: char) -> Vec<String> {
        let glyph = glyph(character);
        (0..HEIGHT)
            .map(|row| {
                (0..usize::from(glyph.advance))
                    .map(|column| if glyph.is_ink(column, row) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn every_glyph_of_the_design_file_is_read_in_codepoint_order() {
        assert_eq!(glyph('A').advance, 6);
        assert_eq!(picture('A')[3], "..#...");
        assert_eq!(picture('A')[8], "#####.");
        assert_eq!(glyph('W').advance, 8);
        assert_eq!(glyph(' ').advance, 4);
        assert_eq!(glyph('\u{a0}'), glyph(' '));
        assert_eq!(picture('\u{ad}')[8], "###.");
        assert_eq!(glyph('ÿ'), &GLYPHS[GLYPH_COUNT - 1]);
        assert!(
            (0x20..=0x7e)
                .chain(0xa0..=0xff)
                .all(|codepoint| { covers(char::from_u32(codepoint).unwrap()) })
        );
    }

    #[test]
    fn the_design_s_metrics_hold() {
        for digit in '0'..='9' {
            assert_eq!(glyph(digit).advance, 6, "{digit}");
        }
        for character in ('A'..='Z').chain('a'..='z') {
            let glyph = glyph(character);
            let last = usize::from(glyph.advance) - 1;
            assert!(
                (0..HEIGHT).all(|row| !glyph.is_ink(last, row)),
                "{character}"
            );
            assert!(
                (0..usize::from(glyph.advance)).any(|column| glyph.is_ink(column, BASELINE)),
                "{character} stands on the baseline"
            );
        }
        assert!(
            (0..4).any(|column| glyph('É').is_ink(column + 1, 0) || glyph('É').is_ink(column, 1))
        );
        assert!(
            GLYPHS
                .iter()
                .all(|glyph| usize::from(glyph.advance) <= WIDEST)
        );
    }

    #[test]
    fn a_character_outside_the_face_draws_the_question_mark() {
        for unknown in ['\u{7f}', '\u{9f}', '\u{100}', '─', '😀', '\n'] {
            assert!(!covers(unknown));
            assert_eq!(glyph(unknown), glyph('?'));
        }
    }

    #[test]
    fn a_string_s_width_is_the_sum_of_its_advances() {
        assert_eq!(width(""), 0);
        assert_eq!(width("Ai"), 6 + 2);
        assert_eq!(width("10:45"), 6 * 4 + u32::from(glyph(':').advance));
    }
}
