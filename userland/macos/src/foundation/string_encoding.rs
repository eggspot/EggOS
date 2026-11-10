//! NSString's byte encodings (D232), for `dataUsingEncoding:`, `initWithData:encoding:` and
//! `initWithBytes:length:encoding:`: ASCII, ISO Latin 1, UTF-8, UTF-16 and UTF-32 in either
//! byte order. UTF-16 and UTF-32 without an order write a byte-order mark and little-endian
//! units, as a Mac does; read without a mark they are big-endian (RFC 2781). A character an
//! encoding cannot hold makes the conversion fail, or becomes `?` when it may be lossy.

use super::string_text::Text;

/// `NSASCIIStringEncoding`.
pub const ASCII: usize = 1;
/// `NSUTF8StringEncoding`.
pub const UTF8: usize = 4;
/// `NSISOLatin1StringEncoding`.
pub const LATIN1: usize = 5;
/// `NSUTF16StringEncoding` (`NSUnicodeStringEncoding`).
pub const UTF16: usize = 10;
/// `NSUTF16BigEndianStringEncoding`.
pub const UTF16_BIG: usize = 0x9000_0100;
/// `NSUTF16LittleEndianStringEncoding`.
pub const UTF16_LITTLE: usize = 0x9400_0100;
/// `NSUTF32StringEncoding`.
pub const UTF32: usize = 0x8c00_0100;
/// `NSUTF32BigEndianStringEncoding`.
pub const UTF32_BIG: usize = 0x9800_0100;
/// `NSUTF32LittleEndianStringEncoding`.
pub const UTF32_LITTLE: usize = 0x9c00_0100;

const REPLACEMENT: u32 = 0xfffd;
const LOSSY_BYTE: u8 = b'?';

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Big,
    Little,
}

#[derive(Clone, Copy)]
enum Form {
    Bytes(u32),
    Utf8,
    Utf16(Option<Order>),
    Utf32(Option<Order>),
}

fn form(encoding: usize) -> Option<Form> {
    match encoding {
        ASCII => Some(Form::Bytes(0x7f)),
        LATIN1 => Some(Form::Bytes(0xff)),
        UTF8 => Some(Form::Utf8),
        UTF16 => Some(Form::Utf16(None)),
        UTF16_BIG => Some(Form::Utf16(Some(Order::Big))),
        UTF16_LITTLE => Some(Form::Utf16(Some(Order::Little))),
        UTF32 => Some(Form::Utf32(None)),
        UTF32_BIG => Some(Form::Utf32(Some(Order::Big))),
        UTF32_LITTLE => Some(Form::Utf32(Some(Order::Little))),
        _ => None,
    }
}

/// Whether the layer converts `encoding`.
pub fn is_known(encoding: usize) -> bool {
    form(encoding).is_some()
}

fn scalars<'a>(text: &'a Text<'a>) -> impl Iterator<Item = u32> + 'a {
    let mut index = 0;
    core::iter::from_fn(move || {
        if index >= text.len() {
            return None;
        }
        let unit = u32::from(text.unit(index));
        index += 1;
        match unit {
            0xd800..=0xdbff if index < text.len() => {
                let low = u32::from(text.unit(index));
                match low {
                    0xdc00..=0xdfff => {
                        index += 1;
                        Some(0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00))
                    }
                    _ => Some(REPLACEMENT),
                }
            }
            0xd800..=0xdfff => Some(REPLACEMENT),
            _ => Some(unit),
        }
    })
}

fn ordered_16(unit: u16, order: Order) -> [u8; 2] {
    match order {
        Order::Big => unit.to_be_bytes(),
        Order::Little => unit.to_le_bytes(),
    }
}

fn ordered_32(scalar: u32, order: Order) -> [u8; 4] {
    match order {
        Order::Big => scalar.to_be_bytes(),
        Order::Little => scalar.to_le_bytes(),
    }
}

/// Writes `text` in `encoding` to `out` in pieces; false (having written part of it) if the
/// encoding is unknown or, unless `lossy`, a character does not fit it.
pub fn encode(text: &Text<'_>, encoding: usize, lossy: bool, out: &mut impl FnMut(&[u8])) -> bool {
    let Some(form) = form(encoding) else {
        return false;
    };
    match form {
        Form::Utf8 => text.write_utf8(out),
        Form::Bytes(largest) => {
            if let Text::Ascii(bytes) = text {
                out(bytes);
                return true;
            }
            for scalar in scalars(text) {
                match scalar <= largest {
                    true => out(&[scalar as u8]),
                    false if lossy => out(&[LOSSY_BYTE]),
                    false => return false,
                }
            }
        }
        Form::Utf16(order) => {
            if order.is_none() {
                out(&ordered_16(0xfeff, Order::Little));
            }
            let order = order.unwrap_or(Order::Little);
            for unit in text.units() {
                out(&ordered_16(unit, order));
            }
        }
        Form::Utf32(order) => {
            if order.is_none() {
                out(&ordered_32(0xfeff, Order::Little));
            }
            let order = order.unwrap_or(Order::Little);
            for scalar in scalars(text) {
                out(&ordered_32(scalar, order));
            }
        }
    }
    true
}

fn put_scalar(scalar: u32, out: &mut impl FnMut(u16)) -> bool {
    match scalar {
        0xd800..=0xdfff => false,
        0..=0xffff => {
            out(scalar as u16);
            true
        }
        0x1_0000..=0x10_ffff => {
            let shifted = scalar - 0x10000;
            out(0xd800 | (shifted >> 10) as u16);
            out(0xdc00 | (shifted & 0x3ff) as u16);
            true
        }
        _ => false,
    }
}

fn marked(bytes: &[u8], width: usize, given: Option<Order>) -> (Order, &[u8]) {
    if let Some(order) = given {
        return (order, bytes);
    }
    let big: &[u8] = if width == 2 {
        &[0xfe, 0xff]
    } else {
        &[0, 0, 0xfe, 0xff]
    };
    let little: &[u8] = if width == 2 {
        &[0xff, 0xfe]
    } else {
        &[0xff, 0xfe, 0, 0]
    };
    if bytes.starts_with(big) {
        (Order::Big, &bytes[width..])
    } else if bytes.starts_with(little) {
        (Order::Little, &bytes[width..])
    } else {
        (Order::Big, bytes)
    }
}

fn decode_utf16(bytes: &[u8], order: Order, out: &mut impl FnMut(u16)) -> bool {
    if bytes.len() % 2 != 0 {
        return false;
    }
    for pair in bytes.chunks_exact(2) {
        let pair = [pair[0], pair[1]];
        out(match order {
            Order::Big => u16::from_be_bytes(pair),
            Order::Little => u16::from_le_bytes(pair),
        });
    }
    true
}

fn decode_utf32(bytes: &[u8], order: Order, out: &mut impl FnMut(u16)) -> bool {
    if bytes.len() % 4 != 0 {
        return false;
    }
    for quad in bytes.chunks_exact(4) {
        let quad = [quad[0], quad[1], quad[2], quad[3]];
        let scalar = match order {
            Order::Big => u32::from_be_bytes(quad),
            Order::Little => u32::from_le_bytes(quad),
        };
        if !put_scalar(scalar, out) {
            return false;
        }
    }
    true
}

/// Decodes `bytes` in `encoding` into UTF-16 units given to `out`; false (having stopped) if
/// the encoding is unknown or the bytes are not valid in it.
pub fn decode(bytes: &[u8], encoding: usize, out: &mut impl FnMut(u16)) -> bool {
    let Some(form) = form(encoding) else {
        return false;
    };
    match form {
        Form::Utf8 => super::string_text::decode_utf8(bytes, out),
        Form::Bytes(largest) => {
            for &byte in bytes {
                if u32::from(byte) > largest {
                    return false;
                }
                out(u16::from(byte));
            }
            true
        }
        Form::Utf16(given) => {
            let (order, rest) = marked(bytes, 2, given);
            decode_utf16(rest, order, out)
        }
        Form::Utf32(given) => {
            let (order, rest) = marked(bytes, 4, given);
            decode_utf32(rest, order, out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(text: &str, encoding: usize, lossy: bool) -> Option<Vec<u8>> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut out = Vec::new();
        let done = encode(&Text::Wide(&units), encoding, lossy, &mut |piece| {
            out.extend_from_slice(piece)
        });
        done.then_some(out)
    }

    fn decoded(bytes: &[u8], encoding: usize) -> Option<String> {
        let mut units = Vec::new();
        let valid = decode(bytes, encoding, &mut |unit| units.push(unit));
        valid.then(|| String::from_utf16(&units).unwrap())
    }

    #[test]
    fn every_encoding_round_trips() {
        let text = "Egg é \u{1F95A}";
        for encoding in [
            UTF8,
            UTF16,
            UTF16_BIG,
            UTF16_LITTLE,
            UTF32,
            UTF32_BIG,
            UTF32_LITTLE,
        ] {
            let bytes = encoded(text, encoding, false).unwrap();
            assert_eq!(
                decoded(&bytes, encoding).as_deref(),
                Some(text),
                "{encoding:#x}"
            );
        }
        let latin = encoded("café", LATIN1, false).unwrap();
        assert_eq!(latin, b"caf\xe9");
        assert_eq!(decoded(&latin, LATIN1).as_deref(), Some("café"));
    }

    #[test]
    fn unordered_forms_write_a_mark_and_little_endian_units() {
        assert_eq!(encoded("A", UTF16, false).unwrap(), [0xff, 0xfe, 0x41, 0]);
        assert_eq!(encoded("A", UTF16_BIG, false).unwrap(), [0, 0x41]);
        assert_eq!(
            encoded("A", UTF32, false).unwrap(),
            [0xff, 0xfe, 0, 0, 0x41, 0, 0, 0]
        );
        assert_eq!(decoded(&[0, 0x41], UTF16).as_deref(), Some("A"));
        assert_eq!(decoded(&[0xff, 0xfe, 0x41, 0], UTF16).as_deref(), Some("A"));
        assert_eq!(decoded(&[0xfe, 0xff, 0, 0x41], UTF16).as_deref(), Some("A"));
    }

    #[test]
    fn characters_an_encoding_lacks_fail_or_become_question_marks() {
        assert_eq!(encoded("é", ASCII, false), None);
        assert_eq!(encoded("aé\u{1F95A}", ASCII, true).unwrap(), b"a??");
        assert_eq!(encoded("\u{1F95A}", LATIN1, true).unwrap(), b"?");
        assert_eq!(encoded("x", 99, false), None);
    }

    #[test]
    fn invalid_bytes_are_refused() {
        assert_eq!(decoded(b"\x80", ASCII), None);
        assert_eq!(decoded(b"\xc3", UTF8), None);
        assert_eq!(decoded(&[0, 0x41, 0], UTF16_BIG), None);
        assert_eq!(decoded(&[0, 0, 0xd8, 0], UTF32_BIG), None);
        assert_eq!(decoded(&[0, 0x11, 0, 0], UTF32_BIG), None);
        assert_eq!(decoded(b"abc", 99), None);
        assert!(!is_known(99));
        assert!(is_known(UTF16_LITTLE));
    }
}
