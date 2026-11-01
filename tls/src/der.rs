//! Reading DER (ITU-T X.690), the encoding of certificates: elements with one-byte tags and
//! definite, minimally encoded lengths.

/// The tag of a SEQUENCE.
pub const SEQUENCE: u8 = 0x30;
/// The tag of a SET.
pub const SET: u8 = 0x31;
/// The tag of a BOOLEAN.
pub const BOOLEAN: u8 = 0x01;
/// The tag of an INTEGER.
pub const INTEGER: u8 = 0x02;
/// The tag of a BIT STRING.
pub const BIT_STRING: u8 = 0x03;
/// The tag of an OCTET STRING.
pub const OCTET_STRING: u8 = 0x04;
/// The tag of an OBJECT IDENTIFIER.
pub const OBJECT_IDENTIFIER: u8 = 0x06;
/// The tag of a UTCTime.
pub const UTC_TIME: u8 = 0x17;
/// The tag of a GeneralizedTime.
pub const GENERALIZED_TIME: u8 = 0x18;

/// The tag of the constructed, context-specific element `[number]`.
pub const fn explicit(number: u8) -> u8 {
    0xa0 | number
}

/// The tag of the primitive, context-specific element `[number]`.
pub const fn implicit(number: u8) -> u8 {
    0x80 | number
}

/// Bytes that are not the DER expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// One element: its tag, its contents, and all its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element<'a> {
    /// The tag.
    pub tag: u8,
    /// The contents, after the tag and length.
    pub contents: &'a [u8],
    /// The whole element.
    pub whole: &'a [u8],
}

/// Reads elements one after another.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader of `data`.
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Whether everything was read.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The next element's tag, without reading it.
    pub fn peek(&self) -> Option<u8> {
        self.data.first().copied()
    }

    /// The next element.
    pub fn read(&mut self) -> Result<Element<'a>, Malformed> {
        let data = self.data;
        let (&tag, rest) = data.split_first().ok_or(Malformed)?;
        if tag & 0x1f == 0x1f {
            return Err(Malformed);
        }
        let (&first, rest) = rest.split_first().ok_or(Malformed)?;
        let (length, rest) = match first {
            0..=0x7f => (usize::from(first), rest),
            0x81..=0x84 => {
                let count = usize::from(first & 0x7f);
                let bytes = rest.get(..count).ok_or(Malformed)?;
                if bytes[0] == 0 {
                    return Err(Malformed);
                }
                let length = bytes
                    .iter()
                    .fold(0usize, |length, &byte| (length << 8) | usize::from(byte));
                if length < 0x80 {
                    return Err(Malformed);
                }
                (length, &rest[count..])
            }
            _ => return Err(Malformed),
        };
        let contents = rest.get(..length).ok_or(Malformed)?;
        let used = data.len() - rest.len() + length;
        self.data = &data[used..];
        Ok(Element {
            tag,
            contents,
            whole: &data[..used],
        })
    }

    /// The next element's contents, when it has `tag`.
    pub fn expect(&mut self, tag: u8) -> Result<&'a [u8], Malformed> {
        let element = self.read()?;
        match element.tag == tag {
            true => Ok(element.contents),
            false => Err(Malformed),
        }
    }

    /// The next element's contents when it has `tag`; nothing read otherwise.
    pub fn optional(&mut self, tag: u8) -> Result<Option<&'a [u8]>, Malformed> {
        match self.peek() == Some(tag) {
            true => self.expect(tag).map(Some),
            false => Ok(None),
        }
    }

    /// Fails unless everything was read.
    pub fn finish(&self) -> Result<(), Malformed> {
        match self.is_empty() {
            true => Ok(()),
            false => Err(Malformed),
        }
    }
}

/// An INTEGER's contents as a positive big-endian number without its leading zero; `None` for a
/// negative or empty one.
pub fn positive_integer(contents: &[u8]) -> Option<&[u8]> {
    match contents {
        [] => None,
        [first, ..] if first & 0x80 != 0 => None,
        [0, rest @ ..] if !rest.is_empty() => Some(rest),
        _ => Some(contents),
    }
}

/// A BIT STRING's contents as whole bytes: `None` unless it has no unused bits.
pub fn bit_string_bytes(contents: &[u8]) -> Option<&[u8]> {
    match contents.split_first() {
        Some((0, bytes)) => Some(bytes),
        _ => None,
    }
}
