//! Windows' UTF-16 strings and EggOS's UTF-8 ones, converted without allocating: invalid input
//! (an unpaired surrogate, a bad UTF-8 sequence) becomes U+FFFD, as Windows' own conversions do.

pub const REPLACEMENT: char = '\u{fffd}';

/// The characters of UTF-16 `units`.
pub fn utf16_chars(units: &[u16]) -> impl Iterator<Item = char> + '_ {
    char::decode_utf16(units.iter().copied()).map(|unit| unit.unwrap_or(REPLACEMENT))
}

/// The characters of UTF-8 `bytes`.
pub fn utf8_chars(bytes: &[u8]) -> impl Iterator<Item = char> + '_ {
    let mut rest = bytes;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let head = &rest[..rest.len().min(4)];
        match core::str::from_utf8(head) {
            Ok(text) => {
                let c = text.chars().next()?;
                rest = &rest[c.len_utf8()..];
                Some(c)
            }
            Err(error) if error.valid_up_to() > 0 => {
                let valid = core::str::from_utf8(&head[..error.valid_up_to()]).ok()?;
                let c = valid.chars().next()?;
                rest = &rest[c.len_utf8()..];
                Some(c)
            }
            Err(error) => {
                let skip = error.error_len().unwrap_or(rest.len());
                rest = &rest[skip..];
                Some(REPLACEMENT)
            }
        }
    })
}

/// How many UTF-16 units `bytes` (UTF-8) takes.
pub fn utf16_length(bytes: &[u8]) -> usize {
    utf8_chars(bytes).map(char::len_utf16).sum()
}

/// Writes `bytes` (UTF-8) as UTF-16 into `out` as far as it fits; returns the units written.
pub fn write_utf16(bytes: &[u8], out: &mut [u16]) -> usize {
    let mut written = 0;
    for c in utf8_chars(bytes) {
        let mut units = [0; 2];
        let units = c.encode_utf16(&mut units);
        if written + units.len() > out.len() {
            break;
        }
        out[written..written + units.len()].copy_from_slice(units);
        written += units.len();
    }
    written
}

/// Writes UTF-16 `units` as UTF-8 into `out` as far as whole characters fit; returns the bytes
/// written and the units consumed.
pub fn write_utf8(units: &[u16], out: &mut [u8]) -> (usize, usize) {
    let mut written = 0;
    let mut consumed = 0;
    for c in utf16_chars(units) {
        let length = c.len_utf8();
        if written + length > out.len() {
            break;
        }
        c.encode_utf8(&mut out[written..]);
        written += length;
        consumed += match c {
            REPLACEMENT => 1,
            c => c.len_utf16(),
        };
    }
    (written, consumed)
}

/// Whether two ASCII names are equal, ignoring case, as Windows compares DLL and environment
/// variable names.
pub fn equal_ignoring_ascii_case(a: impl Iterator<Item = u8>, b: impl Iterator<Item = u8>) -> bool {
    a.map(|byte| byte.to_ascii_lowercase())
        .eq(b.map(|byte| byte.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_utf16_convert_both_ways_with_replacements_for_bad_input() {
        let text = "Egg \u{1f95a} os é";
        let mut units = [0u16; 32];
        let length = write_utf16(text.as_bytes(), &mut units);
        assert_eq!(length, text.encode_utf16().count());
        assert_eq!(utf16_length(text.as_bytes()), length);
        let mut bytes = [0u8; 32];
        assert_eq!(
            write_utf8(&units[..length], &mut bytes),
            (text.len(), length)
        );
        assert_eq!(&bytes[..text.len()], text.as_bytes());
        let bad: String = utf8_chars(b"a\xffb\xe2\x82").collect();
        assert_eq!(bad, "a\u{fffd}b\u{fffd}");
        let unpaired: String = utf16_chars(&[0x61, 0xd800, 0x62]).collect();
        assert_eq!(unpaired, "a\u{fffd}b");
        assert_eq!(write_utf8(&[0x61, 0xdc00], &mut bytes), (4, 2));
    }

    #[test]
    fn conversions_stop_at_the_last_whole_character_that_fits() {
        let mut units = [0u16; 3];
        assert_eq!(write_utf16("ab\u{1f95a}".as_bytes(), &mut units), 2);
        let mut bytes = [0u8; 5];
        let wide: Vec<u16> = "ab\u{1f95a}".encode_utf16().collect();
        assert_eq!(write_utf8(&wide, &mut bytes), (2, 2));
    }

    #[test]
    fn names_compare_ignoring_ascii_case() {
        assert!(equal_ignoring_ascii_case(
            b"KERNEL32.dll".iter().copied(),
            b"kernel32.DLL".iter().copied()
        ));
        assert!(!equal_ignoring_ascii_case(
            b"kernel32".iter().copied(),
            b"kernel32.dll".iter().copied()
        ));
    }
}
