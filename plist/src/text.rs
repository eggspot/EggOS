//! A string's or data object's contents, read in place: a binary list's Latin-1 and big-endian
//! UTF-16 strings and raw bytes, and an XML list's UTF-8 text with its character references and
//! Base64 data.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextForm<'b> {
    Latin1(&'b [u8]),
    Utf16BigEndian(&'b [u8]),
    Xml(&'b [u8]),
}

/// A string's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Text<'b>(TextForm<'b>);

impl<'b> Text<'b> {
    pub(crate) fn latin1(bytes: &'b [u8]) -> Self {
        Self(TextForm::Latin1(bytes))
    }

    pub(crate) fn utf16_big_endian(bytes: &'b [u8]) -> Self {
        Self(TextForm::Utf16BigEndian(bytes))
    }

    pub(crate) fn xml(bytes: &'b [u8]) -> Self {
        Self(TextForm::Xml(bytes))
    }

    /// The characters; a broken UTF-8 or UTF-16 sequence reads as U+FFFD.
    pub fn chars(&self) -> Chars<'b> {
        Chars {
            form: self.0,
            at: 0,
        }
    }

    /// The text as UTF-8 in the list's own bytes, when it is stored that way (ASCII, or XML
    /// text without references).
    pub fn as_utf8(&self) -> Option<&'b [u8]> {
        match self.0 {
            TextForm::Latin1(bytes) if bytes.is_ascii() => Some(bytes),
            TextForm::Xml(bytes) if !bytes.contains(&b'&') => {
                core::str::from_utf8(bytes).ok().map(str::as_bytes)
            }
            _ => None,
        }
    }

    /// Whether the text is `utf8`.
    pub fn equals(&self, utf8: &[u8]) -> bool {
        if let Some(own) = self.as_utf8() {
            return own == utf8;
        }
        core::str::from_utf8(utf8).is_ok_and(|other| self.chars().eq(other.chars()))
    }

    /// How many bytes the text takes in UTF-8.
    pub fn utf8_len(&self) -> usize {
        self.chars().map(char::len_utf8).sum()
    }

    /// Writes the text into `out` as UTF-8; `None` when it does not fit.
    pub fn write_utf8<'o>(&self, out: &'o mut [u8]) -> Option<&'o [u8]> {
        let mut length = 0;
        for character in self.chars() {
            let end = length + character.len_utf8();
            character.encode_utf8(out.get_mut(length..end)?);
            length = end;
        }
        Some(&out[..length])
    }
}

/// The characters of a [`Text`].
#[derive(Debug, Clone)]
pub struct Chars<'b> {
    form: TextForm<'b>,
    at: usize,
}

fn utf8_at(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let first = *bytes.get(at)?;
    let width = match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Some((char::REPLACEMENT_CHARACTER, 1)),
    };
    match bytes
        .get(at..at + width)
        .and_then(|sequence| core::str::from_utf8(sequence).ok())
        .and_then(|text| text.chars().next())
    {
        Some(character) => Some((character, width)),
        None => Some((char::REPLACEMENT_CHARACTER, 1)),
    }
}

const NAMED_ENTITIES: [(&[u8], char); 5] = [
    (b"&amp;", '&'),
    (b"&lt;", '<'),
    (b"&gt;", '>'),
    (b"&quot;", '"'),
    (b"&apos;", '\''),
];
const LONGEST_REFERENCE: usize = 12;

fn reference_at(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let rest = &bytes[at..];
    if let Some((entity, character)) = NAMED_ENTITIES
        .iter()
        .find(|(entity, _)| rest.starts_with(entity))
    {
        return Some((*character, entity.len()));
    }
    let digits = rest.strip_prefix(b"&#")?;
    let end = digits
        .iter()
        .take(LONGEST_REFERENCE)
        .position(|&byte| byte == b';')?;
    let number = core::str::from_utf8(&digits[..end]).ok()?;
    let value = match number.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => number.parse::<u32>().ok()?,
    };
    Some((char::from_u32(value)?, 2 + end + 1))
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self.form {
            TextForm::Latin1(bytes) => {
                let byte = *bytes.get(self.at)?;
                self.at += 1;
                Some(char::from(byte))
            }
            TextForm::Utf16BigEndian(bytes) => {
                let unit = |at: usize| {
                    bytes
                        .get(at..at + 2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                };
                let first = unit(self.at)?;
                self.at += 2;
                if !(0xd800..0xdc00).contains(&first) {
                    return Some(char::from_u32(u32::from(first)).unwrap_or('\u{fffd}'));
                }
                match unit(self.at) {
                    Some(second @ 0xdc00..0xe000) => {
                        self.at += 2;
                        let value = 0x10000
                            + ((u32::from(first) - 0xd800) << 10)
                            + (u32::from(second) - 0xdc00);
                        Some(char::from_u32(value).unwrap_or('\u{fffd}'))
                    }
                    _ => Some(char::REPLACEMENT_CHARACTER),
                }
            }
            TextForm::Xml(bytes) => {
                if *bytes.get(self.at)? == b'&' {
                    let (character, used) = reference_at(bytes, self.at).unwrap_or(('&', 1));
                    self.at += used;
                    return Some(character);
                }
                let (character, used) = utf8_at(bytes, self.at)?;
                self.at += used;
                Some(character)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataForm<'b> {
    Raw(&'b [u8]),
    Base64(&'b [u8]),
}

/// A data object's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Data<'b>(DataForm<'b>);

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

impl<'b> Data<'b> {
    pub(crate) fn raw(bytes: &'b [u8]) -> Self {
        Self(DataForm::Raw(bytes))
    }

    pub(crate) fn base64(text: &'b [u8]) -> Self {
        Self(DataForm::Base64(text))
    }

    /// The bytes in the list's own storage, when they are stored raw (binary lists).
    pub fn as_raw(&self) -> Option<&'b [u8]> {
        match self.0 {
            DataForm::Raw(bytes) => Some(bytes),
            DataForm::Base64(_) => None,
        }
    }

    /// Whether the data is well formed (XML Base64 holds only its alphabet, white space and
    /// final padding).
    pub fn is_valid(&self) -> bool {
        match self.0 {
            DataForm::Raw(_) => true,
            DataForm::Base64(text) => {
                let mut padding = false;
                text.iter().all(|&byte| match byte {
                    b'=' => {
                        padding = true;
                        true
                    }
                    _ if byte.is_ascii_whitespace() => true,
                    _ => !padding && sextet(byte).is_some(),
                })
            }
        }
    }

    /// The decoded bytes; Base64 that is not well formed gives none.
    pub fn bytes(&self) -> Bytes<'b> {
        Bytes {
            form: match self.is_valid() {
                true => self.0,
                false => DataForm::Raw(&[]),
            },
            at: 0,
            bits: 0,
            held: 0,
        }
    }

    /// How many bytes the data decodes to.
    pub fn len(&self) -> usize {
        match self.0 {
            DataForm::Raw(bytes) => bytes.len(),
            DataForm::Base64(_) => self.bytes().count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Writes the bytes into `out`; `None` when they do not fit or are not well formed.
    pub fn write<'o>(&self, out: &'o mut [u8]) -> Option<&'o [u8]> {
        if !self.is_valid() {
            return None;
        }
        let mut length = 0;
        for byte in self.bytes() {
            *out.get_mut(length)? = byte;
            length += 1;
        }
        Some(&out[..length])
    }
}

/// The decoded bytes of a [`Data`].
#[derive(Debug, Clone)]
pub struct Bytes<'b> {
    form: DataForm<'b>,
    at: usize,
    bits: u32,
    held: u32,
}

impl Iterator for Bytes<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        match self.form {
            DataForm::Raw(bytes) => {
                let byte = *bytes.get(self.at)?;
                self.at += 1;
                Some(byte)
            }
            DataForm::Base64(text) => loop {
                if self.held >= 8 {
                    self.held -= 8;
                    return Some((self.bits >> self.held) as u8);
                }
                let byte = *text.get(self.at)?;
                self.at += 1;
                if byte == b'=' {
                    return None;
                }
                if let Some(value) = sextet(byte) {
                    self.bits = (self.bits << 6 | u32::from(value)) & 0xff_ffff;
                    self.held += 6;
                }
            },
        }
    }
}
