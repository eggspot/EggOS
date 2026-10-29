//! NSString's text, independent of the objects that hold it: the two storages (ASCII bytes or
//! UTF-16 units), UTF-8 conversion both ways, comparison and search over UTF-16 units, number
//! parsing, and the quoting collection descriptions use, as macOS prints them: a text is left
//! bare only when it is non-empty ASCII letters and digits, otherwise it is quoted with `\"`,
//! `\\`, `\n`, `\t` and `\Uxxxx` for everything outside printable ASCII.

use core::cmp::Ordering;

/// A string's characters.
#[derive(Debug, Clone, Copy)]
pub enum Text<'a> {
    Ascii(&'a [u8]),
    Wide(&'a [u16]),
}

impl<'a> Text<'a> {
    /// The length in UTF-16 units, NSString's `length`.
    pub fn len(&self) -> usize {
        match self {
            Text::Ascii(bytes) => bytes.len(),
            Text::Wide(units) => units.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The UTF-16 unit at `index` (below the length).
    pub fn unit(&self, index: usize) -> u16 {
        match self {
            Text::Ascii(bytes) => u16::from(bytes[index]),
            Text::Wide(units) => units[index],
        }
    }

    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.len()).map(|index| self.unit(index))
    }

    /// The units from `start` to `end`.
    pub fn slice(&self, start: usize, end: usize) -> Text<'a> {
        match *self {
            Text::Ascii(bytes) => Text::Ascii(&bytes[start..end]),
            Text::Wide(units) => Text::Wide(&units[start..end]),
        }
    }

    /// The text as UTF-8, written in pieces to `out`; an unpaired surrogate becomes U+FFFD.
    pub fn write_utf8(&self, out: &mut impl FnMut(&[u8])) {
        match self {
            Text::Ascii(bytes) => out(bytes),
            Text::Wide(units) => {
                let mut index = 0;
                while index < units.len() {
                    let unit = units[index];
                    let (code, used) = match unit {
                        0xd800..=0xdbff => match units.get(index + 1) {
                            Some(&low @ 0xdc00..=0xdfff) => (
                                0x10000
                                    + ((u32::from(unit) - 0xd800) << 10)
                                    + (u32::from(low) - 0xdc00),
                                2,
                            ),
                            _ => (0xfffd, 1),
                        },
                        0xdc00..=0xdfff => (0xfffd, 1),
                        _ => (u32::from(unit), 1),
                    };
                    let mut buffer = [0u8; 4];
                    out(encode_utf8(code, &mut buffer));
                    index += used;
                }
            }
        }
    }

    /// The UTF-8 length of the text.
    pub fn utf8_len(&self) -> usize {
        let mut length = 0;
        self.write_utf8(&mut |piece| length += piece.len());
        length
    }

    pub fn compare(&self, other: &Text<'_>) -> Ordering {
        self.units().cmp(other.units())
    }

    pub fn equals(&self, other: &Text<'_>) -> bool {
        self.len() == other.len() && self.units().eq(other.units())
    }

    /// FNV-1a over the UTF-16 units, the same for either storage.
    pub fn hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for unit in self.units() {
            for byte in unit.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    /// Where `needle` first occurs at or after `from`.
    pub fn find(&self, needle: &Text<'_>, from: usize) -> Option<usize> {
        if needle.is_empty() || needle.len() > self.len() {
            return None;
        }
        (from..=self.len() - needle.len()).find(|&start| {
            (0..needle.len()).all(|offset| self.unit(start + offset) == needle.unit(offset))
        })
    }

    pub fn starts_with(&self, prefix: &Text<'_>) -> bool {
        prefix.len() <= self.len()
            && (0..prefix.len()).all(|index| self.unit(index) == prefix.unit(index))
    }

    pub fn ends_with(&self, suffix: &Text<'_>) -> bool {
        let start = match self.len().checked_sub(suffix.len()) {
            Some(start) => start,
            None => return false,
        };
        (0..suffix.len()).all(|index| self.unit(start + index) == suffix.unit(index))
    }

    /// The leading integer, as `intValue` reads it: spaces, a sign, digits; 0 if none, clamped.
    pub fn integer_value(&self) -> i64 {
        let mut index = self.skip_spaces();
        let negative = match self.unit_at(index) {
            Some(0x2d) => {
                index += 1;
                true
            }
            Some(0x2b) => {
                index += 1;
                false
            }
            _ => false,
        };
        let mut value: i64 = 0;
        while let Some(digit @ 0x30..=0x39) = self.unit_at(index) {
            let digit = i64::from(digit - 0x30);
            value = value.saturating_mul(10).saturating_add(digit);
            index += 1;
        }
        if negative {
            value.saturating_neg()
        } else {
            value
        }
    }

    /// The leading decimal number, as `doubleValue` reads it; 0 if none.
    pub fn double_value(&self) -> f64 {
        let mut index = self.skip_spaces();
        let negative = match self.unit_at(index) {
            Some(0x2d) => {
                index += 1;
                true
            }
            Some(0x2b) => {
                index += 1;
                false
            }
            _ => false,
        };
        let mut value = 0f64;
        while let Some(digit @ 0x30..=0x39) = self.unit_at(index) {
            value = value * 10.0 + f64::from(digit - 0x30);
            index += 1;
        }
        if self.unit_at(index) == Some(0x2e) {
            index += 1;
            let mut scale = 0.1;
            while let Some(digit @ 0x30..=0x39) = self.unit_at(index) {
                value += f64::from(digit - 0x30) * scale;
                scale /= 10.0;
                index += 1;
            }
        }
        if matches!(self.unit_at(index), Some(0x65 | 0x45)) {
            let exponent = self
                .slice(index + 1, self.len())
                .integer_value()
                .clamp(-400, 400);
            let mut factor = 1f64;
            for _ in 0..exponent.unsigned_abs() {
                factor *= 10.0;
            }
            value = if exponent < 0 {
                value / factor
            } else {
                value * factor
            };
        }
        if negative { -value } else { value }
    }

    /// `boolValue`: true for a leading `Y`, `y`, `T`, `t` or a nonzero digit (after spaces, a
    /// sign and zeros).
    pub fn bool_value(&self) -> bool {
        let mut index = self.skip_spaces();
        if matches!(self.unit_at(index), Some(0x2d | 0x2b)) {
            index += 1;
        }
        while self.unit_at(index) == Some(0x30) {
            index += 1;
        }
        matches!(
            self.unit_at(index),
            Some(0x59 | 0x79 | 0x54 | 0x74 | 0x31..=0x39)
        )
    }

    fn unit_at(&self, index: usize) -> Option<u16> {
        (index < self.len()).then(|| self.unit(index))
    }

    fn skip_spaces(&self) -> usize {
        let mut index = 0;
        while matches!(self.unit_at(index), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            index += 1;
        }
        index
    }
}

/// `code` (a Unicode scalar or U+FFFD) as UTF-8 in `buffer`.
pub fn encode_utf8(code: u32, buffer: &mut [u8; 4]) -> &[u8] {
    match code {
        0..=0x7f => {
            buffer[0] = code as u8;
            &buffer[..1]
        }
        0x80..=0x7ff => {
            buffer[0] = 0xc0 | (code >> 6) as u8;
            buffer[1] = 0x80 | (code & 0x3f) as u8;
            &buffer[..2]
        }
        0x800..=0xffff => {
            buffer[0] = 0xe0 | (code >> 12) as u8;
            buffer[1] = 0x80 | ((code >> 6) & 0x3f) as u8;
            buffer[2] = 0x80 | (code & 0x3f) as u8;
            &buffer[..3]
        }
        _ => {
            buffer[0] = 0xf0 | (code >> 18) as u8;
            buffer[1] = 0x80 | ((code >> 12) & 0x3f) as u8;
            buffer[2] = 0x80 | ((code >> 6) & 0x3f) as u8;
            buffer[3] = 0x80 | (code & 0x3f) as u8;
            &buffer[..4]
        }
    }
}

/// Decodes UTF-8 `bytes` into UTF-16 units given to `out`; false (having stopped) at the first
/// invalid sequence, as `stringWithUTF8String:` returns nil for one.
pub fn decode_utf8(bytes: &[u8], out: &mut impl FnMut(u16)) -> bool {
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        let (length, minimum, initial) = match first {
            0x00..=0x7f => (1, 0, u32::from(first)),
            0xc2..=0xdf => (2, 0x80, u32::from(first & 0x1f)),
            0xe0..=0xef => (3, 0x800, u32::from(first & 0x0f)),
            0xf0..=0xf4 => (4, 0x10000, u32::from(first & 0x07)),
            _ => return false,
        };
        if index + length > bytes.len() {
            return false;
        }
        let mut code = initial;
        for &next in &bytes[index + 1..index + length] {
            if next & 0xc0 != 0x80 {
                return false;
            }
            code = (code << 6) | u32::from(next & 0x3f);
        }
        if code < minimum || code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
            return false;
        }
        if code >= 0x10000 {
            let shifted = code - 0x10000;
            out(0xd800 | (shifted >> 10) as u16);
            out(0xdc00 | (shifted & 0x3ff) as u16);
        } else {
            out(code as u16);
        }
        index += length;
    }
    true
}

/// Whether a collection description must quote `text`.
pub fn needs_quotes(text: &Text<'_>) -> bool {
    text.is_empty()
        || text
            .units()
            .any(|unit| !(unit < 0x80 && (unit as u8).is_ascii_alphanumeric()))
}

/// Writes `text` as a collection description shows it: bare or quoted and escaped.
pub fn write_quoted(text: &Text<'_>, out: &mut impl FnMut(&[u8])) {
    if !needs_quotes(text) {
        text.write_utf8(out);
        return;
    }
    out(b"\"");
    for unit in text.units() {
        match unit {
            0x22 => out(b"\\\""),
            0x5c => out(b"\\\\"),
            0x0a => out(b"\\n"),
            0x09 => out(b"\\t"),
            0x20..=0x7e => out(&[unit as u8]),
            _ => {
                let digits = b"0123456789abcdef";
                out(&[
                    b'\\',
                    b'U',
                    digits[usize::from(unit >> 12)],
                    digits[usize::from((unit >> 8) & 0xf)],
                    digits[usize::from((unit >> 4) & 0xf)],
                    digits[usize::from(unit & 0xf)],
                ]);
            }
        }
    }
    out(b"\"");
}

/// The ASCII upper or lower case of a unit; other units are kept.
pub fn ascii_case(unit: u16, upper: bool) -> u16 {
    match (unit, upper) {
        (0x61..=0x7a, true) => unit - 0x20,
        (0x41..=0x5a, false) => unit + 0x20,
        _ => unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(text: &str) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut out = Vec::new();
        write_quoted(&Text::Wide(&units), &mut |piece| {
            out.extend_from_slice(piece)
        });
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn descriptions_quote_as_macos_does() {
        assert_eq!(quoted("two"), "two");
        assert_eq!(quoted("A9"), "A9");
        assert_eq!(quoted("9"), "9");
        assert_eq!(quoted("3.5"), "\"3.5\"");
        assert_eq!(quoted("a_b"), "\"a_b\"");
        assert_eq!(quoted("-1"), "\"-1\"");
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("a b"), "\"a b\"");
        assert_eq!(quoted("é"), "\"\\U00e9\"");
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("a\nb"), "\"a\\nb\"");
        assert_eq!(quoted("h\u{e9}"), "\"h\\U00e9\"");
    }

    #[test]
    fn utf8_round_trips_through_utf16() {
        let text = "h\u{e9}llo \u{1F95A} w";
        let mut units = Vec::new();
        assert!(decode_utf8(text.as_bytes(), &mut |unit| units.push(unit)));
        assert_eq!(units, text.encode_utf16().collect::<Vec<_>>());
        let wide = Text::Wide(&units);
        assert_eq!(wide.len(), 10);
        let mut out = Vec::new();
        wide.write_utf8(&mut |piece| out.extend_from_slice(piece));
        assert_eq!(out, text.as_bytes());
        assert_eq!(wide.utf8_len(), text.len());
        assert!(!decode_utf8(b"\xc3", &mut |_| {}));
        assert!(!decode_utf8(b"\xed\xa0\x80", &mut |_| {}));
        assert!(!decode_utf8(b"\xc0\xaf", &mut |_| {}));
    }

    #[test]
    fn both_storages_compare_hash_and_search_alike() {
        let wide: Vec<u16> = "hello".encode_utf16().collect();
        let ascii = Text::Ascii(b"hello");
        let wide = Text::Wide(&wide);
        assert!(ascii.equals(&wide));
        assert_eq!(ascii.hash(), wide.hash());
        assert_eq!(ascii.compare(&Text::Ascii(b"help")), Ordering::Less);
        assert_eq!(
            Text::Ascii(b"Zed").compare(&Text::Ascii(b"a b")),
            Ordering::Less
        );
        assert_eq!(ascii.find(&Text::Ascii(b"l"), 0), Some(2));
        assert_eq!(ascii.find(&Text::Ascii(b"l"), 3), Some(3));
        assert_eq!(ascii.find(&Text::Ascii(b"x"), 0), None);
        assert!(wide.starts_with(&Text::Ascii(b"he")));
        assert!(wide.ends_with(&Text::Ascii(b"llo")));
        assert!(!wide.ends_with(&Text::Ascii(b"hhello")));
    }

    #[test]
    fn numbers_parse_as_nsstring_reads_them() {
        assert_eq!(Text::Ascii(b"  -42abc").integer_value(), -42);
        assert_eq!(Text::Ascii(b"x").integer_value(), 0);
        assert_eq!(
            Text::Ascii(b"99999999999999999999").integer_value(),
            i64::MAX
        );
        assert_eq!(Text::Ascii(b"3.25").double_value(), 3.25);
        assert_eq!(Text::Ascii(b"-1.5e2").double_value(), -150.0);
        assert!(Text::Ascii(b"YES").bool_value());
        assert!(Text::Ascii(b" 007").bool_value());
        assert!(!Text::Ascii(b"no").bool_value());
        assert_eq!(ascii_case(u16::from(b'a'), true), u16::from(b'A'));
        assert_eq!(ascii_case(0xe9, true), 0xe9);
    }
}
