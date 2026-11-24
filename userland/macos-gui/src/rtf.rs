//! Reading RTF (D731), the subset text editors write for styled text: groups, the colour
//! table, character formatting (bold, italic, underline, strikethrough, size, colour), the
//! paragraph alignments, paragraph and line breaks, tabs, the named punctuation, `\'hh` bytes
//! in Windows-1252 and `\uN` characters with their fallbacks skipped. Font tables, style
//! sheets, pictures, document information and every `\*` destination are skipped. The reader
//! allocates nothing: each UTF-16 unit of text goes to a [`Sink`] with the style in force.

const MOST_COLOURS: usize = 256;
const MOST_DEPTH: usize = 64;
const DEFAULT_HALF_POINTS: i32 = 24;
const WORD_LONGEST: usize = 32;
const LINE_SEPARATOR: u16 = 0x2028;
const LINE_FEED: u16 = 0x0a;
const TAB: u16 = 0x09;
const NO_BREAK_SPACE: u16 = 0xa0;
const NON_BREAKING_HYPHEN: u16 = 0x2011;

/// A paragraph's alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    Justified,
}

/// The character formatting in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// The font size in half points (`\fs`), 24 (12 pt) when the document gives none.
    pub half_points: i32,
    /// The text colour as `0x00RRGGBB`, or `None` for the automatic colour.
    pub colour: Option<u32>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            half_points: DEFAULT_HALF_POINTS,
            colour: None,
        }
    }
}

/// Where the text goes.
pub trait Sink {
    /// One UTF-16 unit of text with the style in force.
    fn unit(&mut self, unit: u16, style: &Style);
    /// The paragraph just ended (its line feed already given) had `alignment`.
    fn paragraph(&mut self, alignment: Alignment);
}

/// Why a document could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// It does not start with `{\rtf`.
    NotRtf,
    /// Its groups nest deeper than the reader follows, or close more than they open.
    Unbalanced,
}

#[derive(Debug, Clone, Copy)]
struct State {
    style: Style,
    skip: bool,
    colour_table: bool,
    fallback: usize,
    fallback_length: usize,
}

const WINDOWS_1252_HIGH: [u16; 32] = [
    0x20ac, 0x81, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x8d, 0x017d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x9d, 0x017e, 0x0178,
];

fn windows_1252(byte: u8) -> u16 {
    match byte {
        0x80..=0x9f => WINDOWS_1252_HIGH[usize::from(byte - 0x80)],
        _ => u16::from(byte),
    }
}

fn skipped_destination(word: &[u8]) -> bool {
    matches!(
        word,
        b"fonttbl"
            | b"stylesheet"
            | b"info"
            | b"pict"
            | b"header"
            | b"footer"
            | b"headerl"
            | b"headerr"
            | b"footerl"
            | b"footerr"
            | b"listtable"
            | b"listoverridetable"
            | b"rsidtbl"
            | b"generator"
            | b"fldinst"
            | b"themedata"
            | b"colorschememapping"
            | b"latentstyles"
            | b"datastore"
            | b"xmlnstbl"
            | b"object"
            | b"filetbl"
            | b"revtbl"
    )
}

fn named(word: &[u8]) -> Option<u16> {
    Some(match word {
        b"tab" => TAB,
        b"line" => LINE_SEPARATOR,
        b"emdash" => 0x2014,
        b"endash" => 0x2013,
        b"bullet" => 0x2022,
        b"lquote" => 0x2018,
        b"rquote" => 0x2019,
        b"ldblquote" => 0x201c,
        b"rdblquote" => 0x201d,
        b"emspace" => 0x2003,
        b"enspace" => 0x2002,
        _ => return None,
    })
}

struct Reader<'b, 's, S: Sink> {
    bytes: &'b [u8],
    at: usize,
    stack: [State; MOST_DEPTH],
    depth: usize,
    colours: [u32; MOST_COLOURS],
    colour_count: usize,
    pending_colour: u32,
    alignment: Alignment,
    sink: &'s mut S,
}

impl<S: Sink> Reader<'_, '_, S> {
    fn state(&mut self) -> &mut State {
        &mut self.stack[self.depth]
    }

    fn emit(&mut self, unit: u16) {
        let state = self.stack[self.depth];
        if state.skip || state.colour_table {
            return;
        }
        if state.fallback > 0 {
            self.state().fallback -= 1;
            return;
        }
        self.sink.unit(unit, &state.style);
    }

    fn end_paragraph(&mut self) {
        let state = self.stack[self.depth];
        if state.skip || state.colour_table {
            return;
        }
        self.sink.unit(LINE_FEED, &state.style);
        self.sink.paragraph(self.alignment);
    }

    fn colour_table_byte(&mut self, byte: u8) {
        if byte == b';' && self.colour_count < MOST_COLOURS {
            self.colours[self.colour_count] = self.pending_colour;
            self.colour_count += 1;
            self.pending_colour = 0;
        }
    }

    fn word(&mut self, word: &[u8], parameter: Option<i32>) {
        let value = parameter.unwrap_or(1);
        let on = value != 0;
        if self.stack[self.depth].colour_table {
            let channel = value.clamp(0, 255) as u32;
            let colour = self.pending_colour;
            self.pending_colour = match word {
                b"red" => (colour & 0x00_ffff) | (channel << 16),
                b"green" => (colour & 0xff_00ff) | (channel << 8),
                b"blue" => (colour & 0xff_ff00) | channel,
                _ => colour,
            };
            return;
        }
        match word {
            b"b" => self.state().style.bold = on,
            b"i" => self.state().style.italic = on,
            b"ul" => self.state().style.underline = on,
            b"ulnone" => self.state().style.underline = false,
            b"strike" => self.state().style.strikethrough = on,
            b"fs" => self.state().style.half_points = value.max(1),
            b"cf" => {
                let index = usize::try_from(value).unwrap_or(0);
                let known = index < self.colour_count;
                self.state().style.colour = match index {
                    0 => None,
                    index => self.colours.get(index).copied().filter(|_| known),
                };
            }
            b"plain" => self.state().style = Style::default(),
            b"pard" => self.alignment = Alignment::Left,
            b"ql" => self.alignment = Alignment::Left,
            b"qc" => self.alignment = Alignment::Center,
            b"qr" => self.alignment = Alignment::Right,
            b"qj" => self.alignment = Alignment::Justified,
            b"par" | b"sect" | b"page" => self.end_paragraph(),
            b"uc" => self.state().fallback_length = usize::try_from(value).unwrap_or(0),
            b"u" => {
                let unit = value as i16 as u16;
                self.emit(unit);
                let length = self.state().fallback_length;
                self.state().fallback = length;
            }
            b"colortbl" => self.state().colour_table = true,
            word if skipped_destination(word) => self.state().skip = true,
            word => {
                if let Some(unit) = named(word) {
                    self.emit(unit);
                }
            }
        }
    }

    fn control(&mut self) {
        let Some(&first) = self.bytes.get(self.at) else {
            return;
        };
        self.at += 1;
        match first {
            b'\\' | b'{' | b'}' => self.emit(u16::from(first)),
            b'~' => self.emit(NO_BREAK_SPACE),
            b'_' => self.emit(NON_BREAKING_HYPHEN),
            b'-' => {}
            b'*' => self.state().skip = true,
            b'\'' => {
                let hex = self.bytes.get(self.at..self.at + 2);
                let byte = hex
                    .and_then(|hex| core::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                self.at += 2;
                if let Some(byte) = byte {
                    self.emit(windows_1252(byte));
                }
            }
            b'\n' | b'\r' => self.end_paragraph(),
            letter if letter.is_ascii_alphabetic() => {
                let mut word = [0u8; WORD_LONGEST];
                let mut length = 0;
                word[0] = letter;
                length += 1;
                let letter_at = |at: usize| self.bytes.get(at).filter(|b| b.is_ascii_alphabetic());
                while let Some(&next) = letter_at(self.at) {
                    if length < WORD_LONGEST {
                        word[length] = next;
                        length += 1;
                    }
                    self.at += 1;
                }
                let negative = self.bytes.get(self.at) == Some(&b'-');
                if negative {
                    self.at += 1;
                }
                let mut parameter: Option<i32> = None;
                while let Some(&digit) = self.bytes.get(self.at).filter(|b| b.is_ascii_digit()) {
                    let so_far = parameter.unwrap_or(0).saturating_mul(10);
                    parameter = Some(so_far.saturating_add(i32::from(digit - b'0')));
                    self.at += 1;
                }
                if negative {
                    parameter = parameter.map(|value| -value);
                }
                if self.bytes.get(self.at) == Some(&b' ') {
                    self.at += 1;
                }
                self.word(&word[..length], parameter);
            }
            _ => {}
        }
    }

    fn read(&mut self) -> Result<(), Error> {
        while let Some(&byte) = self.bytes.get(self.at) {
            self.at += 1;
            match byte {
                b'{' => {
                    if self.depth + 1 == MOST_DEPTH {
                        return Err(Error::Unbalanced);
                    }
                    self.stack[self.depth + 1] = State {
                        fallback: 0,
                        ..self.stack[self.depth]
                    };
                    self.depth += 1;
                }
                b'}' => {
                    if self.depth == 0 {
                        return Err(Error::Unbalanced);
                    }
                    self.depth -= 1;
                    if self.depth == 0 {
                        return Ok(());
                    }
                }
                b'\\' => self.control(),
                b'\n' | b'\r' => {}
                byte if self.stack[self.depth].colour_table => self.colour_table_byte(byte),
                byte => self.emit(windows_1252(byte)),
            }
        }
        Ok(())
    }
}

/// Reads the RTF document `bytes`, handing its text to `sink`.
pub fn read(bytes: &[u8], sink: &mut impl Sink) -> Result<(), Error> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(Error::NotRtf);
    }
    let base = State {
        style: Style::default(),
        skip: false,
        colour_table: false,
        fallback: 0,
        fallback_length: 1,
    };
    let mut reader = Reader {
        bytes,
        at: 0,
        stack: [base; MOST_DEPTH],
        depth: 0,
        colours: [0; MOST_COLOURS],
        colour_count: 0,
        pending_colour: 0,
        alignment: Alignment::Left,
        sink,
    };
    reader.read()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collected {
        text: String,
        styles: Vec<Style>,
        paragraphs: Vec<Alignment>,
    }

    impl Sink for Collected {
        fn unit(&mut self, unit: u16, style: &Style) {
            self.text.push(char::from_u32(u32::from(unit)).unwrap_or('?'));
            self.styles.push(*style);
        }

        fn paragraph(&mut self, alignment: Alignment) {
            self.paragraphs.push(alignment);
        }
    }

    fn collected(document: &str) -> Collected {
        let mut out = Collected::default();
        read(document.as_bytes(), &mut out).unwrap();
        out
    }

    #[test]
    fn plain_text_and_paragraphs_come_through_and_tables_are_skipped() {
        let out = collected(
            r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}{\*\generator Test;}\f0 Hello\par World}",
        );
        assert_eq!(out.text, "Hello\nWorld");
        assert_eq!(out.paragraphs, [Alignment::Left]);
    }

    #[test]
    fn character_formatting_follows_groups_and_the_colour_table() {
        let out = collected(
            r"{\rtf1{\colortbl;\red255\green0\blue0;}a{\b b\i0}{\cf1\fs48 c}\ul d\ulnone e}",
        );
        assert_eq!(out.text, "abcde");
        assert!(!out.styles[0].bold);
        assert!(out.styles[1].bold);
        assert_eq!(out.styles[2].colour, Some(0x00ff_0000));
        assert_eq!(out.styles[2].half_points, 48);
        assert!(out.styles[3].underline);
        assert!(!out.styles[4].underline);
    }

    #[test]
    fn escapes_unicode_and_alignment_are_read() {
        let out = collected(r"{\rtf1\qc caf\'e9 荤?\'80\par\pard\qr \{x\}\par}");
        assert_eq!(out.text, "café €€\n{x}\n");
        assert_eq!(out.paragraphs, [Alignment::Center, Alignment::Right]);
    }

    #[test]
    fn a_document_that_is_not_rtf_or_nests_too_deep_is_refused() {
        let mut out = Collected::default();
        assert_eq!(read(b"hello", &mut out), Err(Error::NotRtf));
        let deep = format!("{{\\rtf1{}", "{".repeat(MOST_DEPTH));
        assert_eq!(read(deep.as_bytes(), &mut out), Err(Error::Unbalanced));
    }

    #[test]
    fn unicode_fallbacks_follow_uc() {
        let out = collected(r"{\rtf1\uc2\u233 xxy\uc0\u233 z}");
        assert_eq!(out.text, "éyéz");
    }
}
