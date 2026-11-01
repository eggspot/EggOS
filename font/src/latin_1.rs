use super::{GLYPH_HEIGHT, GLYPH_WIDTH, GlyphRows};

pub(super) const GLYPH_COUNT: usize = 97;

const FIRST: u32 = 0xa0;
const LAST: u32 = 0xff;
const EURO: u32 = 0x20ac;

pub(super) static LATIN_1: [GlyphRows; GLYPH_COUNT] =
    parse(include_bytes!("../../docs/ux/font-8x16-latin-1.txt"));

const fn codepoint_at(index: usize) -> u32 {
    if index as u32 <= LAST - FIRST {
        index as u32 + FIRST
    } else {
        EURO
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
        _ => panic!("font-8x16-latin-1.txt: a glyph header's codepoint is not 4 hex digits"),
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

const fn is_header(text: &[u8], start: usize, end: usize) -> bool {
    end >= start + 6 && text[start] == b'U' && text[start + 1] == b'+'
}

const fn parse(text: &[u8]) -> [GlyphRows; GLYPH_COUNT] {
    let mut glyphs = [[0u8; GLYPH_HEIGHT]; GLYPH_COUNT];
    let mut count = 0;
    let mut start = 0;
    while start < text.len() {
        let end = content_end(text, start, line_end(text, start));
        if !is_header(text, start, end) {
            start = line_end(text, start) + 1;
            continue;
        }
        assert!(
            count < GLYPH_COUNT,
            "font-8x16-latin-1.txt: more than 97 glyphs"
        );
        assert!(
            header_codepoint(text, start) == codepoint_at(count),
            "font-8x16-latin-1.txt: a glyph is out of codepoint order"
        );
        let mut rows = [0u8; GLYPH_HEIGHT];
        let mut row = 0;
        start = line_end(text, start) + 1;
        while row < GLYPH_HEIGHT {
            assert!(
                start < text.len(),
                "font-8x16-latin-1.txt: a glyph has fewer than 16 rows"
            );
            let end = content_end(text, start, line_end(text, start));
            assert!(
                end - start == GLYPH_WIDTH,
                "font-8x16-latin-1.txt: a row is not 8 columns"
            );
            let mut column = 0;
            while column < GLYPH_WIDTH {
                match text[start + column] {
                    b'#' => rows[row] |= 0x80 >> column,
                    b'.' => {}
                    _ => panic!("font-8x16-latin-1.txt: a row holds something other than . and #"),
                }
                column += 1;
            }
            row += 1;
            start = line_end(text, start) + 1;
        }
        glyphs[count] = rows;
        count += 1;
    }
    assert!(
        count == GLYPH_COUNT,
        "font-8x16-latin-1.txt: fewer than 97 glyphs"
    );
    glyphs
}
