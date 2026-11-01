//! Character classes and case mappings as a Mac's `isalpha` family, `toupper` and `towupper`
//! read them: Darwin's class bits (`_CTYPE_A` and the rest, a hexadecimal digit's value in the
//! low byte), the C locale's ASCII table that `__DefaultRuneLocale` holds whatever the locale,
//! Latin-1's Unicode classes in C.UTF-8 and en_US.UTF-8 (UTF-8 alone keeps ASCII's), and
//! Latin-1's case mappings in every UTF-8 locale. Characters above U+00FF have no classes yet.

use super::settings::Locale;

pub const ALPHA: u32 = 0x100;
pub const CONTROL: u32 = 0x200;
pub const DIGIT: u32 = 0x400;
pub const GRAPH: u32 = 0x800;
pub const LOWER: u32 = 0x1000;
pub const PUNCT: u32 = 0x2000;
pub const SPACE: u32 = 0x4000;
pub const UPPER: u32 = 0x8000;
pub const HEX_DIGIT: u32 = 0x1_0000;
pub const BLANK: u32 = 0x2_0000;
pub const PRINT: u32 = 0x4_0000;
/// The bit the Unicode locales add to the decimal digits.
pub const NUMBER: u32 = 0x40_0000;

const LETTERS: usize = 26;
const UPPER_A: usize = 0x41;
const LOWER_A: usize = 0x61;
const CASE_SHIFT: i32 = 0x20;
const LATIN_Y_DIAERESIS: i32 = 0xff;
const CAPITAL_Y_DIAERESIS: i32 = 0x178;
const MICRO_SIGN: i32 = 0xb5;
const CAPITAL_MU: i32 = 0x39c;

const fn ascii(byte: u8) -> u32 {
    match byte {
        b'\t' => BLANK | SPACE | CONTROL,
        0x0a..=0x0d => SPACE | CONTROL,
        0x00..=0x08 | 0x0e..=0x1f | 0x7f => CONTROL,
        b' ' => BLANK | PRINT | SPACE,
        b'0'..=b'9' => PRINT | HEX_DIGIT | DIGIT | GRAPH | (byte - b'0') as u32,
        b'A'..=b'F' => PRINT | HEX_DIGIT | UPPER | GRAPH | ALPHA | (byte - b'A' + 10) as u32,
        b'G'..=b'Z' => PRINT | UPPER | GRAPH | ALPHA,
        b'a'..=b'f' => PRINT | HEX_DIGIT | LOWER | GRAPH | ALPHA | (byte - b'a' + 10) as u32,
        b'g'..=b'z' => PRINT | LOWER | GRAPH | ALPHA,
        0x21..=0x2f | 0x3a..=0x40 | 0x5b..=0x60 | 0x7b..=0x7e => PRINT | PUNCT | GRAPH,
        _ => 0,
    }
}

const fn latin1(byte: u8) -> u32 {
    match byte {
        0x80..=0x9f | 0xad => CONTROL,
        0xa0 => PRINT | SPACE,
        0xa1 | 0xa7 | 0xab | 0xb6 | 0xb7 | 0xbb | 0xbf => PRINT | PUNCT | GRAPH,
        0xaa | 0xba => PRINT | GRAPH | ALPHA,
        0xb5 | 0xdf..=0xf6 | 0xf8..=0xff => PRINT | LOWER | GRAPH | ALPHA,
        0xc0..=0xd6 | 0xd8..=0xde => PRINT | UPPER | GRAPH | ALPHA,
        0x00..=0x7f => ascii(byte),
        _ => PRINT | GRAPH,
    }
}

const fn default_classes() -> [u32; 256] {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 0x80 {
        table[byte] = ascii(byte as u8);
        byte += 1;
    }
    table
}

const fn default_mapping(first: usize, shift: i32) -> [i32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        table[index] = index as i32;
        index += 1;
    }
    let mut letter = 0;
    while letter < LETTERS {
        table[first + letter] = (first + letter) as i32 + shift;
        letter += 1;
    }
    table
}

/// `__DefaultRuneLocale`'s classes of each byte: the C locale's, ASCII only.
pub const DEFAULT_CLASSES: [u32; 256] = default_classes();
/// `__DefaultRuneLocale`'s lowercase of each byte.
pub const DEFAULT_LOWER: [i32; 256] = default_mapping(UPPER_A, CASE_SHIFT);
/// `__DefaultRuneLocale`'s uppercase of each byte.
pub const DEFAULT_UPPER: [i32; 256] = default_mapping(LOWER_A, -CASE_SHIFT);

fn has_unicode_classes(locale: Locale) -> bool {
    matches!(locale, Locale::CUtf8 | Locale::UnitedStates)
}

/// The classes of `rune` in `locale`'s `LC_CTYPE`, which `___maskrune` masks.
pub fn classes(locale: Locale, rune: i32) -> u32 {
    let Ok(byte) = u8::try_from(rune) else {
        return 0;
    };
    match (byte, has_unicode_classes(locale)) {
        (0x00..=0x7f, false) => DEFAULT_CLASSES[usize::from(byte)],
        (b'0'..=b'9', true) => ascii(byte) | NUMBER,
        (_, true) => latin1(byte),
        (_, false) => 0,
    }
}

/// The uppercase of `rune` in `locale`, which `toupper` and `towupper` answer.
pub fn to_upper(locale: Locale, rune: i32) -> i32 {
    match rune {
        0x61..=0x7a => rune - CASE_SHIFT,
        _ if !locale.is_utf8() => rune,
        MICRO_SIGN => CAPITAL_MU,
        LATIN_Y_DIAERESIS => CAPITAL_Y_DIAERESIS,
        0xe0..=0xf6 | 0xf8..=0xfe => rune - CASE_SHIFT,
        _ => rune,
    }
}

/// The lowercase of `rune` in `locale`, which `tolower` and `towlower` answer.
pub fn to_lower(locale: Locale, rune: i32) -> i32 {
    match rune {
        0x41..=0x5a => rune + CASE_SHIFT,
        _ if !locale.is_utf8() => rune,
        0xc0..=0xd6 | 0xd8..=0xde => rune + CASE_SHIFT,
        CAPITAL_Y_DIAERESIS => LATIN_Y_DIAERESIS,
        _ => rune,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_table_is_the_c_locales_ascii() {
        assert_eq!(DEFAULT_CLASSES[usize::from(b'a')], 0x0005_190a);
        assert_eq!(DEFAULT_CLASSES[usize::from(b'0')], 0x0005_0c00);
        assert_eq!(DEFAULT_CLASSES[usize::from(b'F')], 0x0005_890f);
        assert_eq!(DEFAULT_CLASSES[usize::from(b'\t')], 0x0002_4200);
        assert_eq!(DEFAULT_CLASSES[usize::from(b' ')], 0x0006_4000);
        assert_eq!(DEFAULT_CLASSES[usize::from(b'~')], 0x0004_2800);
        assert_eq!(DEFAULT_CLASSES[0x7f], CONTROL);
        assert!(DEFAULT_CLASSES[0x80..].iter().all(|&classes| classes == 0));
        assert_eq!((DEFAULT_UPPER[0x61], DEFAULT_LOWER[0x5a]), (0x41, 0x7a));
        assert_eq!((DEFAULT_UPPER[0xe9], DEFAULT_LOWER[0xc9]), (0xe9, 0xc9));
    }

    #[test]
    fn latin1_has_unicodes_classes_in_the_unicode_locales_only() {
        let united = Locale::UnitedStates;
        assert_eq!(classes(united, 0xe9), PRINT | LOWER | GRAPH | ALPHA);
        assert_eq!(classes(united, 0xc9), PRINT | UPPER | GRAPH | ALPHA);
        assert_eq!(classes(united, 0xa0), PRINT | SPACE);
        assert_eq!(classes(united, 0xd7), PRINT | GRAPH);
        assert_eq!(classes(united, 0xbf), PRINT | PUNCT | GRAPH);
        assert_eq!(classes(united, 0xad), CONTROL);
        assert_eq!(classes(united, b'7'.into()), 0x0045_0c07);
        assert_eq!(classes(Locale::CUtf8, 0xaa), PRINT | GRAPH | ALPHA);
        for locale in [Locale::C, Locale::Posix, Locale::Utf8] {
            assert_eq!(classes(locale, 0xe9), 0);
            assert_eq!(classes(locale, b'7'.into()), 0x0005_0c07);
        }
        for rune in [-1, 0x100, 0x4e00] {
            assert_eq!(classes(united, rune), 0);
        }
    }

    #[test]
    fn case_maps_latin1_in_utf8_locales_and_ascii_in_c() {
        for locale in [Locale::Utf8, Locale::CUtf8, Locale::UnitedStates] {
            assert_eq!(to_upper(locale, 0xe9), 0xc9);
            assert_eq!(to_lower(locale, 0xc9), 0xe9);
            assert_eq!(to_upper(locale, 0xff), 0x178);
            assert_eq!(to_upper(locale, 0xb5), 0x39c);
            assert_eq!(to_lower(locale, 0x178), 0xff);
            assert_eq!(
                (to_upper(locale, 0xf7), to_lower(locale, 0xd7)),
                (0xf7, 0xd7)
            );
            assert_eq!(to_upper(locale, 0xdf), 0xdf);
        }
        assert_eq!(
            (to_upper(Locale::C, 0xe9), to_lower(Locale::C, 0xc9)),
            (0xe9, 0xc9)
        );
        assert_eq!(
            (
                to_upper(Locale::C, b'q'.into()),
                to_lower(Locale::Posix, b'Q'.into())
            ),
            (0x51, 0x71)
        );
        assert_eq!(
            (to_upper(Locale::C, -1), to_lower(Locale::UnitedStates, -1)),
            (-1, -1)
        );
    }
}
