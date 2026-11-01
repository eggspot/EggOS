//! The multibyte encodings of Darwin's locales: in C and POSIX each byte is the character of
//! its value and a wide character above 0xFF has no bytes; in the UTF-8 locales a character is
//! a UTF-8 sequence, refusing overlong forms, surrogates, values above U+10FFFF and lead bytes
//! of five bytes or more, as a Mac does. A [`Decoder`] keeps a partial sequence between calls in
//! 64 bits, which `mbstate_t` holds.

use super::settings::Locale;

/// How a locale's `LC_CTYPE` writes characters as bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Byte,
    Utf8,
}

impl Encoding {
    pub fn of(locale: Locale) -> Self {
        if locale.is_utf8() {
            Self::Utf8
        } else {
            Self::Byte
        }
    }

    /// The most bytes a character takes, `MB_CUR_MAX`.
    pub fn longest(self) -> i32 {
        match self {
            Self::Byte => 1,
            Self::Utf8 => 4,
        }
    }

    /// The bytes of `scalar` (a `wchar_t`) into `out`, returning how many; `None` when the
    /// encoding has none for it.
    pub fn encode(self, scalar: u32, out: &mut [u8; 4]) -> Option<usize> {
        match self {
            Self::Byte => {
                out[0] = u8::try_from(scalar).ok()?;
                Some(1)
            }
            Self::Utf8 => Some(char::from_u32(scalar)?.encode_utf8(out).len()),
        }
    }
}

/// One step of [`Decoder::decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A whole character, completed by the first `used` bytes of the input.
    Character { scalar: u32, used: usize },
    /// The input ended inside a character, which the decoder keeps.
    Incomplete,
    /// The bytes are not a character; the decoder starts afresh.
    Invalid,
}

/// A partial UTF-8 sequence: its value so far, the bytes still to come and its whole length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Decoder {
    value: u32,
    remaining: u8,
    length: u8,
}

impl Decoder {
    pub fn from_bits(bits: u64) -> Self {
        Self {
            value: bits as u32,
            remaining: (bits >> 32) as u8,
            length: (bits >> 40) as u8,
        }
    }

    pub fn bits(self) -> u64 {
        u64::from(self.value) | (u64::from(self.remaining) << 32) | (u64::from(self.length) << 40)
    }

    /// Whether no character is under way, `mbsinit`'s answer.
    pub fn is_initial(self) -> bool {
        self.remaining == 0
    }

    /// Reads the next character of `input` in `encoding`, continuing the one under way.
    pub fn decode(&mut self, encoding: Encoding, input: &[u8]) -> Step {
        if encoding == Encoding::Byte {
            return match input.first() {
                Some(&byte) => Step::Character {
                    scalar: u32::from(byte),
                    used: 1,
                },
                None => Step::Incomplete,
            };
        }
        for (index, &byte) in input.iter().enumerate() {
            if self.remaining == 0 {
                let (length, bits) = match byte {
                    0x00..=0x7f => {
                        return Step::Character {
                            scalar: u32::from(byte),
                            used: index + 1,
                        };
                    }
                    0xc0..=0xdf => (2, byte & 0x1f),
                    0xe0..=0xef => (3, byte & 0x0f),
                    0xf0..=0xf7 => (4, byte & 0x07),
                    _ => return Step::Invalid,
                };
                *self = Self {
                    value: u32::from(bits),
                    remaining: length - 1,
                    length,
                };
                continue;
            }
            if byte & 0xc0 != 0x80 {
                *self = Self::default();
                return Step::Invalid;
            }
            self.value = (self.value << 6) | u32::from(byte & 0x3f);
            self.remaining -= 1;
            if self.remaining == 0 {
                let (scalar, length) = (self.value, self.length);
                *self = Self::default();
                if !is_scalar(scalar, length) {
                    return Step::Invalid;
                }
                return Step::Character {
                    scalar,
                    used: index + 1,
                };
            }
        }
        Step::Incomplete
    }
}

fn is_scalar(value: u32, length: u8) -> bool {
    let shortest = match length {
        2 => 0x80,
        3 => 0x800,
        _ => 0x1_0000,
    };
    value >= shortest && char::from_u32(value).is_some()
}

/// What a string conversion did: the characters (or bytes) it wrote, uncounted NUL aside, how
/// much of its input it consumed, and whether it reached the input's NUL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Converted {
    pub count: usize,
    pub read: usize,
    pub finished: bool,
}

/// `mbsrtowcs`: decodes the NUL-terminated bytes `byte_at` reads into `out`, or only counts
/// when `out` is `None`, until the NUL (written too) or until `out` is full; `None` at a
/// sequence that is not a character.
pub fn decode_text(
    encoding: Encoding,
    decoder: &mut Decoder,
    byte_at: impl Fn(usize) -> u8,
    mut out: Option<&mut [u32]>,
) -> Option<Converted> {
    let mut converted = Converted::default();
    let mut position = 0;
    loop {
        if out.as_ref().is_some_and(|out| converted.count == out.len()) {
            return Some(converted);
        }
        let byte = byte_at(position);
        position += 1;
        match decoder.decode(encoding, &[byte]) {
            Step::Incomplete => {}
            Step::Invalid => return None,
            Step::Character { scalar, .. } => {
                if let Some(out) = out.as_deref_mut() {
                    out[converted.count] = scalar;
                }
                converted.read = position;
                if scalar == 0 {
                    converted.finished = true;
                    return Some(converted);
                }
                converted.count += 1;
            }
        }
    }
}

/// `wcsrtombs`: encodes the NUL-terminated wide characters `wide_at` reads into `out`, or only
/// counts when `out` is `None`, until the NUL (written too when it fits) or a character whose
/// bytes do not all fit; `None` at a wide character the encoding has no bytes for.
pub fn encode_text(
    encoding: Encoding,
    wide_at: impl Fn(usize) -> u32,
    mut out: Option<&mut [u8]>,
) -> Option<Converted> {
    let mut converted = Converted::default();
    loop {
        let scalar = wide_at(converted.read);
        let mut bytes = [0; 4];
        let length = encoding.encode(scalar, &mut bytes)?;
        if let Some(out) = out.as_deref_mut() {
            let Some(room) = out.get_mut(converted.count..converted.count + length) else {
                return Some(converted);
            };
            room.copy_from_slice(&bytes[..length]);
        }
        if scalar == 0 {
            converted.finished = true;
            return Some(converted);
        }
        converted.count += length;
        converted.read += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(encoding: Encoding, input: &[u8]) -> Step {
        Decoder::default().decode(encoding, input)
    }

    #[test]
    fn utf8_refuses_what_a_mac_refuses() {
        let utf8 = Encoding::Utf8;
        assert_eq!(decode_all(utf8, b"\xed\xa0\x80"), Step::Invalid);
        assert_eq!(decode_all(utf8, b"\xf8\x88\x80\x80\x80"), Step::Invalid);
        assert_eq!(decode_all(utf8, b"\xe9"), Step::Incomplete);
        assert_eq!(decode_all(utf8, b"\xf4\x90\x80\x80"), Step::Invalid);
        assert_eq!(decode_all(utf8, b"\xc0\x80"), Step::Invalid);
        assert_eq!(decode_all(utf8, b"\xc3A"), Step::Invalid);
        assert_eq!(decode_all(utf8, b"\x80"), Step::Invalid);
        assert_eq!(
            decode_all(utf8, b"\xef\xbf\xbf"),
            Step::Character {
                scalar: 0xffff,
                used: 3
            }
        );
        assert_eq!(
            decode_all(utf8, b"\xf4\x8f\xbf\xbfrest"),
            Step::Character {
                scalar: 0x10_ffff,
                used: 4
            }
        );
        assert_eq!(decode_all(utf8, b""), Step::Incomplete);
    }

    #[test]
    fn a_character_split_across_calls_completes_with_the_bytes_of_the_last() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.decode(Encoding::Utf8, b"\xe2"), Step::Incomplete);
        assert!(!decoder.is_initial());
        let mut decoder = Decoder::from_bits(decoder.bits());
        assert_eq!(decoder.decode(Encoding::Utf8, b"\x82"), Step::Incomplete);
        assert_eq!(
            decoder.decode(Encoding::Utf8, b"\xacx"),
            Step::Character {
                scalar: 0x20ac,
                used: 1
            }
        );
        assert!(decoder.is_initial());
        assert_eq!(decoder.decode(Encoding::Utf8, b"\xc3"), Step::Incomplete);
        assert_eq!(decoder.decode(Encoding::Utf8, b"\0"), Step::Invalid);
        assert!(decoder.is_initial());
    }

    #[test]
    fn the_byte_encoding_maps_each_byte_to_itself() {
        assert_eq!(
            decode_all(Encoding::Byte, b"\xed\xa0"),
            Step::Character {
                scalar: 0xed,
                used: 1
            }
        );
        let mut out = [0; 4];
        assert_eq!(Encoding::Byte.encode(0xe9, &mut out), Some(1));
        assert_eq!(out[0], 0xe9);
        assert_eq!(Encoding::Byte.encode(0x100, &mut out), None);
    }

    #[test]
    fn utf8_encodes_scalars_and_refuses_the_rest() {
        let mut out = [0; 4];
        assert_eq!(Encoding::Utf8.encode(0xe9, &mut out), Some(2));
        assert_eq!(out[..2], [0xc3, 0xa9]);
        assert_eq!(Encoding::Utf8.encode(0x10_ffff, &mut out), Some(4));
        assert_eq!(out, [0xf4, 0x8f, 0xbf, 0xbf]);
        for refused in [0xd800, 0x11_0000, 0x7fff_ffff, u32::MAX] {
            assert_eq!(Encoding::Utf8.encode(refused, &mut out), None);
        }
    }

    #[test]
    fn strings_convert_until_the_nul_or_the_room_runs_out() {
        let text = b"h\xc3\xa9\xe2\x82\xac\0";
        let byte_at = |index: usize| text[index];
        let mut wide = [0u32; 8];
        let mut decoder = Decoder::default();
        let converted =
            decode_text(Encoding::Utf8, &mut decoder, byte_at, Some(&mut wide[..])).unwrap();
        assert_eq!(
            converted,
            Converted {
                count: 3,
                read: 7,
                finished: true
            }
        );
        assert_eq!(wide[..4], [0x68, 0xe9, 0x20ac, 0]);
        let counted = decode_text(Encoding::Utf8, &mut decoder, byte_at, None).unwrap();
        assert_eq!(counted.count, 3);
        let mut short = [0u32; 2];
        let partial =
            decode_text(Encoding::Utf8, &mut decoder, byte_at, Some(&mut short[..])).unwrap();
        assert_eq!(
            (partial.count, partial.read, partial.finished),
            (2, 3, false)
        );
        let bad = b"a\xff\0";
        assert_eq!(
            decode_text(
                Encoding::Utf8,
                &mut Decoder::default(),
                |index| bad[index],
                None
            ),
            None
        );

        let wide_text = [0x68u32, 0xe9, 0x20ac, 0];
        let wide_at = |index: usize| wide_text[index];
        let mut bytes = [0xffu8; 8];
        let written = encode_text(Encoding::Utf8, wide_at, Some(&mut bytes[..])).unwrap();
        assert_eq!(
            (written.count, written.read, written.finished),
            (6, 3, true)
        );
        assert_eq!(&bytes[..7], b"h\xc3\xa9\xe2\x82\xac\0");
        let mut room = [0u8; 3];
        let cut = encode_text(Encoding::Utf8, wide_at, Some(&mut room[..])).unwrap();
        assert_eq!((cut.count, cut.read, cut.finished), (3, 2, false));
        assert_eq!(encode_text(Encoding::Utf8, wide_at, None).unwrap().count, 6);
        assert_eq!(encode_text(Encoding::Byte, wide_at, None), None);
        let surrogate = [0x61u32, 0xd800, 0];
        assert_eq!(
            encode_text(Encoding::Utf8, |index| surrogate[index], None),
            None
        );
    }
}
