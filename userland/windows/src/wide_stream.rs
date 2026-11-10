//! Wide characters on the C runtime's byte streams (D229): `fgetwc`, `fputwc` and `ungetwc`
//! move UTF-16 units, which a stream holds as the locale's multibyte characters (UTF-8,
//! EggOS's ANSI code page, through the stream's own text mode) or as UTF-16LE: a stream opened
//! with `ccs=UTF-16LE` or `ccs=UNICODE`, a descriptor `_setmode` put in `_O_WTEXT` or
//! `_O_U16TEXT`, or a binary stream. UTF-16 in text mode writes `\n` as `\r\n` and reads
//! `\r\n` as `\n`, with Ctrl+Z ending the text; a terminal always takes UTF-8.

use crate::low_level_io::Translation;
use crate::stream::{Backing, Stream};

const CARRIAGE_RETURN: u16 = 0x0d;
const LINE_FEED: u16 = 0x0a;
const END_OF_TEXT: u16 = 0x1a;
const REPLACEMENT: u16 = 0xfffd;
const UTF8_BYTE_ORDER_MARK: [u8; 3] = [0xef, 0xbb, 0xbf];
const UTF16_BYTE_ORDER_MARK: [u8; 2] = [0xff, 0xfe];
const PENDING_UNITS: usize = 2;

/// A mode whose text after the comma names no encoding the runtime knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownEncoding;

/// The encoding `fopen`'s mode names after `ccs=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ccs {
    Utf8,
    Utf16,
}

impl Ccs {
    /// The encoding the mode `letters` name after a comma (`r, ccs=UTF-8`; `UTF-16LE` and
    /// `UNICODE` are UTF-16), without regard to case: `Ok(None)` without a comma, `Err` for
    /// anything else after one.
    pub fn of_mode(letters: impl Iterator<Item = u32>) -> Result<Option<Ccs>, UnknownEncoding> {
        let mut after = letters.skip_while(|letter| *letter != u32::from(b','));
        if after.next().is_none() {
            return Ok(None);
        }
        let mut text = [0u8; 16];
        let mut length = 0;
        for letter in after.filter(|letter| *letter != u32::from(b' ')) {
            let byte = u8::try_from(letter).map_err(|_| UnknownEncoding)?;
            *text.get_mut(length).ok_or(UnknownEncoding)? = byte.to_ascii_uppercase();
            length += 1;
        }
        match &text[..length] {
            b"CCS=UTF-8" => Ok(Some(Ccs::Utf8)),
            b"CCS=UTF-16LE" | b"CCS=UNICODE" => Ok(Some(Ccs::Utf16)),
            _ => Err(UnknownEncoding),
        }
    }

    /// The byte order mark a file opened for writing with this encoding starts with.
    pub fn byte_order_mark(self) -> &'static [u8] {
        match self {
            Ccs::Utf8 => &UTF8_BYTE_ORDER_MARK,
            Ccs::Utf16 => &UTF16_BYTE_ORDER_MARK,
        }
    }

    /// The encoding a file's first bytes `head` name by their byte order mark, and the mark's
    /// length; `None` without one.
    pub fn of_byte_order_mark(head: &[u8]) -> Option<(Ccs, usize)> {
        if head.starts_with(&UTF8_BYTE_ORDER_MARK) {
            Some((Ccs::Utf8, UTF8_BYTE_ORDER_MARK.len()))
        } else if head.starts_with(&UTF16_BYTE_ORDER_MARK) {
            Some((Ccs::Utf16, UTF16_BYTE_ORDER_MARK.len()))
        } else {
            None
        }
    }
}

/// How a stream holds wide characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, through the stream's own text mode.
    Multibyte,
    /// UTF-16LE, with `\r\n` for `\n` and Ctrl+Z ending the text when `translated`.
    Utf16 { translated: bool },
}

/// The encoding of a stream opened with `ccs`, in text mode or not (`text`), whose descriptor
/// is in `descriptor`'s translation or is a terminal.
pub fn encoding(ccs: Option<Ccs>, descriptor: Translation, text: bool, terminal: bool) -> Encoding {
    match (terminal, ccs, descriptor) {
        (true, _, _) | (_, Some(Ccs::Utf8), _) | (_, None, Translation::Utf8) => {
            Encoding::Multibyte
        }
        (_, Some(Ccs::Utf16), _) => Encoding::Utf16 { translated: text },
        (_, None, Translation::Utf16) => Encoding::Utf16 { translated: true },
        (_, None, Translation::Binary) => Encoding::Utf16 { translated: false },
        (_, None, Translation::Text) if !text => Encoding::Utf16 { translated: false },
        (_, None, Translation::Text) => Encoding::Multibyte,
    }
}

/// A stream's wide-character state: units read ahead or pushed back, a high surrogate
/// written and waiting for its pair, and whether a Ctrl+Z ended UTF-16 text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WideState {
    pending: [u16; PENDING_UNITS],
    pending_count: usize,
    waiting_high: Option<u16>,
    ended: bool,
}

impl WideState {
    /// Forgets the units read ahead and pushed back and a Ctrl+Z's end, as a seek does.
    pub fn forget(&mut self) {
        self.pending_count = 0;
        self.ended = false;
    }

    /// Pushes `unit` back (`ungetwc`) so the next read returns it first; `false` when two
    /// units already wait.
    pub fn unget(&mut self, unit: u16) -> bool {
        if self.pending_count == PENDING_UNITS {
            return false;
        }
        self.pending[self.pending_count] = unit;
        self.pending_count += 1;
        true
    }

    fn take_pending(&mut self) -> Option<u16> {
        self.pending_count = self.pending_count.checked_sub(1)?;
        Some(self.pending[self.pending_count])
    }

    /// Writes the UTF-16 `unit` (`fputwc`); `false` when the stream refused it.
    pub fn put<B: Backing>(
        &mut self,
        stream: &mut Stream<B>,
        encoding: Encoding,
        unit: u16,
    ) -> bool {
        match encoding {
            Encoding::Utf16 { translated } => {
                let line_ended = unit == LINE_FEED;
                let bytes = unit.to_le_bytes();
                match (translated, line_ended) {
                    (true, true) => {
                        let mut both = [0u8; 4];
                        both[..2].copy_from_slice(&CARRIAGE_RETURN.to_le_bytes());
                        both[2..].copy_from_slice(&bytes);
                        stream.write_untranslated(&both, true)
                    }
                    _ => stream.write_untranslated(&bytes, line_ended),
                }
            }
            Encoding::Multibyte => {
                let c = match (self.waiting_high.take(), unit) {
                    (None, 0xd800..=0xdbff) => {
                        self.waiting_high = Some(unit);
                        return true;
                    }
                    (Some(high), 0xdc00..=0xdfff) => char::decode_utf16([high, unit])
                        .next()
                        .and_then(Result::ok)
                        .unwrap_or(char::REPLACEMENT_CHARACTER),
                    (Some(_), _) => {
                        if !write_char(stream, char::REPLACEMENT_CHARACTER) {
                            return false;
                        }
                        return self.put(stream, encoding, unit);
                    }
                    (None, _) => {
                        char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
                    }
                };
                write_char(stream, c)
            }
        }
    }

    /// Reads one UTF-16 unit (`fgetwc`); `None` at the end of the file or on an error, which
    /// the stream's indicators tell apart.
    pub fn get<B: Backing>(&mut self, stream: &mut Stream<B>, encoding: Encoding) -> Option<u16> {
        if let Some(unit) = self.take_pending() {
            return Some(unit);
        }
        if self.ended {
            stream.mark_end_of_file();
            return None;
        }
        match encoding {
            Encoding::Multibyte => {
                let c = read_char(stream)?;
                let mut units = [0u16; 2];
                let encoded = c.encode_utf16(&mut units);
                if let [high, low] = *encoded {
                    self.unget(low);
                    return Some(high);
                }
                Some(encoded[0])
            }
            Encoding::Utf16 { translated } => {
                let unit = read_unit(stream)?;
                if !translated {
                    return Some(unit);
                }
                match unit {
                    END_OF_TEXT => {
                        self.ended = true;
                        stream.mark_end_of_file();
                        None
                    }
                    CARRIAGE_RETURN => match read_unit(stream) {
                        Some(LINE_FEED) => Some(LINE_FEED),
                        Some(next) => {
                            self.unget(next);
                            Some(CARRIAGE_RETURN)
                        }
                        None => Some(CARRIAGE_RETURN),
                    },
                    _ => Some(unit),
                }
            }
        }
    }
}

fn write_char<B: Backing>(stream: &mut Stream<B>, c: char) -> bool {
    let mut bytes = [0u8; 4];
    let encoded = c.encode_utf8(&mut bytes).as_bytes();
    stream.write(encoded) == encoded.len()
}

fn read_unit<B: Backing>(stream: &mut Stream<B>) -> Option<u16> {
    let low = stream.get_untranslated_byte()?;
    let high = stream.get_untranslated_byte()?;
    Some(u16::from_le_bytes([low, high]))
}

fn read_char<B: Backing>(stream: &mut Stream<B>) -> Option<char> {
    let lead = stream.get_byte()?;
    let (length, initial) = match lead {
        0x00..=0x7f => return Some(char::from(lead)),
        0xc2..=0xdf => (2, u32::from(lead & 0x1f)),
        0xe0..=0xef => (3, u32::from(lead & 0x0f)),
        0xf0..=0xf4 => (4, u32::from(lead & 0x07)),
        _ => return Some(char::REPLACEMENT_CHARACTER),
    };
    let mut value = initial;
    for _ in 1..length {
        let Some(byte) = stream.get_byte() else {
            return Some(char::REPLACEMENT_CHARACTER);
        };
        if byte & 0xc0 != 0x80 {
            stream.unget(byte);
            return Some(char::REPLACEMENT_CHARACTER);
        }
        value = (value << 6) | u32::from(byte & 0x3f);
    }
    let shortest = match length {
        2 => 0x80,
        3 => 0x800,
        _ => 0x1_0000,
    };
    Some(match value >= shortest {
        true => char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER),
        false => char::REPLACEMENT_CHARACTER,
    })
}

const _: () = assert!(REPLACEMENT == char::REPLACEMENT_CHARACTER as u16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Buffering, Mode, Origin};

    #[derive(Debug, Default)]
    struct Memory {
        bytes: Vec<u8>,
        at: usize,
    }

    impl Backing for Memory {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, u16> {
            let taken = self.bytes.len().saturating_sub(self.at).min(buffer.len());
            buffer[..taken].copy_from_slice(&self.bytes[self.at..self.at + taken]);
            self.at += taken;
            Ok(taken)
        }

        fn write(&mut self, bytes: &[u8]) -> Result<(), u16> {
            self.bytes.truncate(self.at);
            self.bytes.extend_from_slice(bytes);
            self.at = self.bytes.len();
            Ok(())
        }

        fn seek(&mut self, offset: i64, origin: Origin) -> Result<u64, u16> {
            let base = match origin {
                Origin::Start => 0,
                Origin::Current => self.at as i64,
                Origin::End => self.bytes.len() as i64,
            };
            self.at = (base + offset) as usize;
            Ok(self.at as u64)
        }
    }

    fn stream(bytes: &[u8], letters: &str) -> Stream<Memory> {
        let mode = Mode::parse(letters.chars().map(u32::from)).unwrap();
        let memory = Memory {
            bytes: bytes.to_vec(),
            at: 0,
        };
        Stream::new(memory, mode, Buffering::Full)
    }

    fn written(encoding: Encoding, letters: &str, text: &str) -> Vec<u8> {
        let mut file = stream(b"", letters);
        let mut state = WideState::default();
        for unit in text.encode_utf16() {
            assert!(state.put(&mut file, encoding, unit));
        }
        file.flush().unwrap();
        file.backing().bytes.clone()
    }

    fn read(encoding: Encoding, letters: &str, bytes: &[u8]) -> Vec<u16> {
        let mut file = stream(bytes, letters);
        let mut state = WideState::default();
        core::iter::from_fn(|| state.get(&mut file, encoding)).collect()
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn ccs_names_the_encoding_after_the_comma() {
        let of = |letters: &str| Ccs::of_mode(letters.chars().map(u32::from));
        assert_eq!(of("r"), Ok(None));
        assert_eq!(of("w, ccs=UTF-8"), Ok(Some(Ccs::Utf8)));
        assert_eq!(of("r,ccs=utf-16le"), Ok(Some(Ccs::Utf16)));
        assert_eq!(of("a+, ccs=UNICODE"), Ok(Some(Ccs::Utf16)));
        assert_eq!(of("w, ccs=UTF-7"), Err(UnknownEncoding));
        assert_eq!(of("w, mode=x"), Err(UnknownEncoding));
        assert_eq!(
            Ccs::of_byte_order_mark(&[0xef, 0xbb, 0xbf, b'a']),
            Some((Ccs::Utf8, 3))
        );
        assert_eq!(
            Ccs::of_byte_order_mark(&[0xff, 0xfe]),
            Some((Ccs::Utf16, 2))
        );
        assert_eq!(Ccs::of_byte_order_mark(b"ab"), None);
        assert_eq!(Ccs::Utf16.byte_order_mark(), &[0xff, 0xfe]);
    }

    #[test]
    fn the_encoding_follows_ccs_then_the_descriptor_then_the_stream_s_mode() {
        let utf16 = |translated| Encoding::Utf16 { translated };
        assert_eq!(
            encoding(None, Translation::Text, true, false),
            Encoding::Multibyte
        );
        assert_eq!(
            encoding(None, Translation::Text, false, false),
            utf16(false)
        );
        assert_eq!(encoding(None, Translation::Utf16, true, false), utf16(true));
        assert_eq!(
            encoding(None, Translation::Utf16, true, true),
            Encoding::Multibyte
        );
        assert_eq!(
            encoding(None, Translation::Utf8, true, false),
            Encoding::Multibyte
        );
        assert_eq!(
            encoding(Some(Ccs::Utf16), Translation::Text, true, false),
            utf16(true)
        );
        assert_eq!(
            encoding(Some(Ccs::Utf8), Translation::Binary, false, false),
            Encoding::Multibyte
        );
    }

    #[test]
    fn multibyte_streams_hold_utf8_through_text_mode() {
        let text = "é😀\nz";
        assert_eq!(
            written(Encoding::Multibyte, "w", text),
            "é😀\r\nz".as_bytes()
        );
        assert_eq!(
            read(Encoding::Multibyte, "r", "é😀\r\nz".as_bytes()),
            text.encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            read(Encoding::Multibyte, "rb", &[0xc3, b'a', 0xff]),
            [REPLACEMENT, 0x61, REPLACEMENT]
        );
    }

    #[test]
    fn utf16_streams_translate_line_ends_in_units() {
        let text = "a\nb\rc";
        assert_eq!(
            written(Encoding::Utf16 { translated: true }, "w", text),
            utf16("a\r\nb\rc")
        );
        assert_eq!(
            written(Encoding::Utf16 { translated: false }, "wb", text),
            utf16(text)
        );
        assert_eq!(
            read(
                Encoding::Utf16 { translated: true },
                "r",
                &utf16("a\r\nb\rc\u{1a}d")
            ),
            "a\nb\rc".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn pushed_back_units_come_back_first_and_a_seek_forgets_them() {
        let mut file = stream(b"xy", "r");
        let mut state = WideState::default();
        assert!(state.unget(0x41));
        assert!(state.unget(0x42));
        assert!(!state.unget(0x43));
        assert_eq!(state.get(&mut file, Encoding::Multibyte), Some(0x42));
        assert_eq!(state.get(&mut file, Encoding::Multibyte), Some(0x41));
        assert_eq!(
            state.get(&mut file, Encoding::Multibyte),
            Some(u16::from(b'x'))
        );
        assert!(state.unget(0x41));
        state.forget();
        assert_eq!(
            state.get(&mut file, Encoding::Multibyte),
            Some(u16::from(b'y'))
        );
    }
}
