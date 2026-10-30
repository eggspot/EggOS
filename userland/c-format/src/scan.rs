//! `scanf`'s family (D174, D190), shared by the Windows layer's `__stdio_common_vsscanf` and
//! the macOS layer's `scanf`, `fscanf` and `sscanf`: white space in the format skips any in
//! the input, other characters must match, and each conversion (`%d`, `%i`, `%u`, `%o`, `%x`,
//! `%p`, the `%e` family, `%c`, `%s`, `%[...]`, `%n`, `%%`) with its optional `*`, width and
//! size reads a field from a [`Source`] and hands it to the caller, which stores it where the
//! next argument points. A field is the longest prefix of a number the input holds; what the
//! number does not use (the `x` of `0x`, the `e` of `1e`) goes back to the input, as Darwin's
//! does. The [`Dialect`] says what a size means: Darwin's `l` is 64 bits and makes `%lc`,
//! `%ls` and `%l[` wide; Windows' `l` is 32 bits.

use crate::Dialect;
use crate::number::{self, parse_float, parse_integer};

/// Where an integer conversion stores its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Byte,
    Short,
    Int,
    Wide,
}

/// Where a floating-point conversion stores its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Single,
    Double,
    /// A `long double`: a double on arm64 and in Microsoft's C, x87's 80 bits on x86_64 Darwin.
    Extended,
}

/// One converted field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field<'a> {
    Signed(i64, Size),
    Unsigned(u64, Size),
    Float(f64, Precision),
    /// `%c`'s characters, stored without a NUL.
    Characters(&'a [u8]),
    /// `%s`' and `%[`'s characters, stored with a NUL.
    Text(&'a [u8]),
    /// `%lc`'s multibyte characters, stored as wide characters without a NUL.
    WideCharacters(&'a [u8]),
    /// `%ls`' and `%l[`'s multibyte characters, stored as wide characters with a NUL.
    WideText(&'a [u8]),
    /// `%n`'s count of the input read so far.
    Count(usize, Size),
}

/// What [`scan`] ends with when the input ended before the first conversion: `EOF`.
pub const END_OF_INPUT: i32 = -1;

/// The input `scan` reads.
pub trait Source {
    /// The next byte, still unread; `None` at the end of the input.
    fn peek(&mut self) -> Option<u8>;
    /// Reads the byte [`Source::peek`] returned.
    fn advance(&mut self);
    /// Reads at most `limit` bytes while `accept` takes them and returns them.
    fn take_while(&mut self, limit: usize, accept: &mut dyn FnMut(u8) -> bool) -> &[u8];
    /// Returns the last `count` bytes [`Source::take_while`] read to the input, to be read
    /// again.
    fn give_back(&mut self, count: usize);
}

/// A [`Source`] over bytes in memory, for `sscanf`.
#[derive(Debug)]
pub struct Bytes<'a> {
    input: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, at: 0 }
    }
}

impl Source for Bytes<'_> {
    fn peek(&mut self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    fn advance(&mut self) {
        self.at = (self.at + 1).min(self.input.len());
    }

    fn take_while(&mut self, limit: usize, accept: &mut dyn FnMut(u8) -> bool) -> &[u8] {
        let start = self.at;
        while self.at - start < limit && self.input.get(self.at).is_some_and(|byte| accept(*byte)) {
            self.at += 1;
        }
        &self.input[start..self.at]
    }

    fn give_back(&mut self, count: usize) {
        self.at = self.at.saturating_sub(count);
    }
}

fn is_space(byte: u8) -> bool {
    number::is_space(u32::from(byte))
}

fn skip_space(source: &mut dyn Source) -> usize {
    let mut skipped = 0;
    while source.peek().is_some_and(is_space) {
        source.advance();
        skipped += 1;
    }
    skipped
}

struct Spec {
    suppress: bool,
    width: Option<usize>,
    size: Size,
    precision: Precision,
    wide: bool,
    conversion: u8,
}

fn parse_spec(format: &[u8], mut at: usize, dialect: Dialect) -> Option<(Spec, usize)> {
    let suppress = format.get(at) == Some(&b'*');
    if suppress {
        at += 1;
    }
    let mut width = None;
    while let Some(&digit) = format.get(at).filter(|byte| byte.is_ascii_digit()) {
        width = Some(
            width
                .unwrap_or(0usize)
                .saturating_mul(10)
                .saturating_add(usize::from(digit - b'0')),
        );
        at += 1;
    }
    let darwin = matches!(dialect, Dialect::Darwin);
    let (mut size, mut precision, mut wide) = (Size::Int, Precision::Single, false);
    let rest = &format[at.min(format.len())..];
    let taken = match rest {
        [b'h', b'h', ..] => {
            size = Size::Byte;
            2
        }
        [b'h', ..] => {
            size = Size::Short;
            1
        }
        [b'l', b'l', ..] => {
            size = Size::Wide;
            2
        }
        [b'l', ..] => {
            precision = Precision::Double;
            if darwin {
                size = Size::Wide;
                wide = true;
            }
            1
        }
        [b'L', ..] => {
            precision = match darwin {
                true => Precision::Extended,
                false => Precision::Double,
            };
            1
        }
        [b'q', ..] if darwin => {
            size = Size::Wide;
            1
        }
        [b'I', b'6', b'4', ..] if !darwin => {
            size = Size::Wide;
            3
        }
        [b'I', b'3', b'2', ..] if !darwin => 3,
        [b'I', ..] if !darwin => {
            size = Size::Wide;
            1
        }
        [b'j' | b'z' | b't', ..] => {
            size = Size::Wide;
            1
        }
        [b'w', ..] if !darwin => 1,
        _ => 0,
    };
    at += taken;
    let mut conversion = *format.get(at)?;
    if darwin && matches!(conversion, b'C' | b'S') {
        wide = true;
        conversion = conversion.to_ascii_lowercase();
    }
    if conversion == b'p' {
        size = Size::Wide;
    }
    Some((
        Spec {
            suppress,
            width,
            size,
            precision,
            wide: wide && matches!(conversion, b'c' | b's' | b'['),
            conversion,
        },
        at + 1,
    ))
}

fn in_set(set: &[u8], byte: u8) -> bool {
    let (negated, set) = match set.first() {
        Some(b'^') => (true, &set[1..]),
        _ => (false, set),
    };
    let mut found = false;
    let mut index = 0;
    while index < set.len() {
        let low = set[index];
        if set.get(index + 1) == Some(&b'-') && index + 2 < set.len() {
            let high = set[index + 2];
            found |= (low..=high).contains(&byte);
            index += 3;
        } else {
            found |= low == byte;
            index += 1;
        }
    }
    found != negated
}

fn set_end(format: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    if format.get(at) == Some(&b'^') {
        at += 1;
    }
    if format.get(at) == Some(&b']') {
        at += 1;
    }
    while *format.get(at)? != b']' {
        at += 1;
    }
    Some(at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegerStage {
    Start,
    Signed,
    Zero,
    Prefix,
    Digits,
}

/// The longest prefix of an integer in a base: a sign, `0x` when the base allows it, digits.
struct IntegerShape {
    stage: IntegerStage,
    base: u32,
    hex_prefix: bool,
}

impl IntegerShape {
    fn new(base: u32) -> Self {
        Self {
            stage: IntegerStage::Start,
            base,
            hex_prefix: base == 0 || base == 16,
        }
    }

    fn accepts(&mut self, byte: u8) -> bool {
        let digit_base = match self.base {
            0 => 10,
            base => base,
        };
        let is_digit = char::from(byte)
            .to_digit(36)
            .is_some_and(|value| value < digit_base);
        let next = match (self.stage, byte) {
            (IntegerStage::Start, b'+' | b'-') => IntegerStage::Signed,
            (IntegerStage::Start | IntegerStage::Signed, b'0') => {
                if self.base == 0 {
                    self.base = 8;
                }
                IntegerStage::Zero
            }
            (IntegerStage::Zero, b'x' | b'X') if self.hex_prefix => {
                self.base = 16;
                IntegerStage::Prefix
            }
            _ if is_digit => {
                if self.base == 0 {
                    self.base = 10;
                }
                IntegerStage::Digits
            }
            _ => return false,
        };
        self.stage = next;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatStage {
    Start,
    Signed,
    Word,
    Integer,
    Point,
    Fraction,
    Exponent,
    ExponentSigned,
    ExponentDigits,
}

/// The longest prefix of a decimal floating-point number or of `infinity` or `nan`.
struct FloatShape {
    stage: FloatStage,
    word: &'static [u8],
    letters: usize,
}

impl FloatShape {
    fn new() -> Self {
        Self {
            stage: FloatStage::Start,
            word: b"",
            letters: 0,
        }
    }

    fn accepts(&mut self, byte: u8) -> bool {
        let digit = byte.is_ascii_digit();
        let next = match (self.stage, byte) {
            (FloatStage::Start, b'+' | b'-') => FloatStage::Signed,
            (FloatStage::Start | FloatStage::Signed, b'.') => FloatStage::Point,
            (FloatStage::Start | FloatStage::Signed, b'i' | b'I' | b'n' | b'N') => {
                self.word = match byte.to_ascii_lowercase() {
                    b'i' => b"infinity",
                    _ => b"nan",
                };
                self.letters = 1;
                FloatStage::Word
            }
            (FloatStage::Word, _) => {
                if self.word.get(self.letters) != Some(&byte.to_ascii_lowercase()) {
                    return false;
                }
                self.letters += 1;
                FloatStage::Word
            }
            (FloatStage::Start | FloatStage::Signed | FloatStage::Integer, _) if digit => {
                FloatStage::Integer
            }
            (FloatStage::Integer, b'.') => FloatStage::Fraction,
            (FloatStage::Point | FloatStage::Fraction, _) if digit => FloatStage::Fraction,
            (FloatStage::Integer | FloatStage::Fraction, b'e' | b'E') => FloatStage::Exponent,
            (FloatStage::Exponent, b'+' | b'-') => FloatStage::ExponentSigned,
            (FloatStage::Exponent | FloatStage::ExponentSigned | FloatStage::ExponentDigits, _)
                if digit =>
            {
                FloatStage::ExponentDigits
            }
            _ => return false,
        };
        self.stage = next;
        true
    }
}

/// Whether `byte` continues a UTF-8 sequence rather than starting a character.
fn continues_character(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

/// Takes at most `limit` characters while `accept` takes their first bytes: bytes when
/// `utf8` is false, UTF-8 sequences when it is true.
fn take_characters<'s>(
    source: &'s mut (dyn Source + '_),
    limit: usize,
    utf8: bool,
    accept: &mut dyn FnMut(u8) -> bool,
) -> &'s [u8] {
    let mut characters = 0usize;
    let mut inside = false;
    source.take_while(usize::MAX, &mut |byte| {
        if utf8 && inside && continues_character(byte) {
            return true;
        }
        if characters == limit || !accept(byte) {
            return false;
        }
        characters += 1;
        inside = byte >= 0xc0;
        true
    })
}

/// How [`scan`] reads its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub dialect: Dialect,
    /// Whether multibyte characters are UTF-8 (the locale's `LC_CTYPE`), which `%lc`, `%ls`
    /// and `%l[` read whole; otherwise each byte is a character.
    pub utf8: bool,
}

/// Reads `source` as `format` says, handing each field not suppressed with `*` to `store`,
/// which answers whether it stored it (no argument for it ends the scan); the fields stored,
/// `%n`'s not counted, or [`END_OF_INPUT`] when the input ended before the first conversion.
pub fn scan(
    source: &mut dyn Source,
    format: &[u8],
    rules: Rules,
    store: &mut dyn FnMut(Field<'_>) -> bool,
) -> i32 {
    let mut stored = 0i32;
    let mut position = 0usize;
    let mut at = 0usize;
    let mut converted = false;
    let ended = |converted: bool, stored: i32| match converted {
        true => stored,
        false => END_OF_INPUT,
    };
    while let Some(&byte) = format.get(at) {
        if is_space(byte) {
            position += skip_space(source);
            at += 1;
            continue;
        }
        if byte != b'%' {
            match source.peek() {
                Some(found) if found == byte => {
                    source.advance();
                    position += 1;
                    at += 1;
                    continue;
                }
                Some(_) => return stored,
                None => return ended(converted, stored),
            }
        }
        let Some((spec, mut next)) = parse_spec(format, at + 1, rules.dialect) else {
            return stored;
        };
        if !matches!(spec.conversion, b'c' | b'[' | b'n') {
            position += skip_space(source);
        }
        if spec.conversion != b'n' && source.peek().is_none() {
            return ended(converted, stored);
        }
        let limit = spec.width.unwrap_or(usize::MAX);
        let (taken, value) = match spec.conversion {
            b'%' => {
                if source.peek() != Some(b'%') {
                    return stored;
                }
                source.advance();
                (1, None)
            }
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                let base = match spec.conversion {
                    b'd' | b'u' => 10,
                    b'i' => 0,
                    b'o' => 8,
                    _ => 16,
                };
                let mut shape = IntegerShape::new(base);
                let field = source.take_while(limit, &mut |byte| shape.accepts(byte));
                let length = field.len();
                let number = parse_integer(field, base).filter(|number| number.consumed > 0);
                let Some(number) = number else {
                    source.give_back(length);
                    return stored;
                };
                let value = match spec.conversion {
                    b'd' | b'i' => Field::Signed(number.signed(64).0, spec.size),
                    _ => Field::Unsigned(number.unsigned(64).0, spec.size),
                };
                source.give_back(length - number.consumed);
                (number.consumed, Some(value))
            }
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
                let mut shape = FloatShape::new();
                let field = source.take_while(limit, &mut |byte| shape.accepts(byte));
                let length = field.len();
                let (value, consumed) = parse_float(field);
                source.give_back(length - consumed);
                if consumed == 0 {
                    return stored;
                }
                (consumed, Some(Field::Float(value, spec.precision)))
            }
            b'c' => {
                let width = spec.width.unwrap_or(1);
                let characters = match spec.wide {
                    true => take_characters(source, width, rules.utf8, &mut |_| true),
                    false => source.take_while(width, &mut |_| true),
                };
                let value = match spec.wide {
                    true => Field::WideCharacters(characters),
                    false => Field::Characters(characters),
                };
                (characters.len(), Some(value))
            }
            b's' => {
                let text = match spec.wide {
                    true => take_characters(source, limit, rules.utf8, &mut |byte| !is_space(byte)),
                    false => source.take_while(limit, &mut |byte| !is_space(byte)),
                };
                let value = match spec.wide {
                    true => Field::WideText(text),
                    false => Field::Text(text),
                };
                (text.len(), Some(value))
            }
            b'[' => {
                let Some(end) = set_end(format, next) else {
                    return stored;
                };
                let set = &format[next..end];
                next = end + 1;
                let text = match spec.wide {
                    true => {
                        take_characters(source, limit, rules.utf8, &mut |byte| in_set(set, byte))
                    }
                    false => source.take_while(limit, &mut |byte| in_set(set, byte)),
                };
                if text.is_empty() {
                    return stored;
                }
                let value = match spec.wide {
                    true => Field::WideText(text),
                    false => Field::Text(text),
                };
                (text.len(), Some(value))
            }
            b'n' => (0, Some(Field::Count(position, spec.size))),
            _ => return stored,
        };
        position += taken;
        at = next;
        if spec.conversion != b'%' {
            converted = true;
        }
        let Some(value) = value.filter(|_| !spec.suppress) else {
            continue;
        };
        if !store(value) {
            return stored;
        }
        if !matches!(value, Field::Count(..)) {
            stored += 1;
        }
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARWIN: Rules = Rules {
        dialect: Dialect::Darwin,
        utf8: true,
    };
    const WINDOWS: Rules = Rules {
        dialect: Dialect::Windows {
            wide_strings: false,
        },
        utf8: true,
    };

    fn fields_by(rules: Rules, input: &str, format: &str) -> (i32, Vec<String>) {
        let mut seen = Vec::new();
        let count = scan(
            &mut Bytes::new(input.as_bytes()),
            format.as_bytes(),
            rules,
            &mut |field| {
                seen.push(format!("{field:?}"));
                true
            },
        );
        (count, seen)
    }

    fn fields(input: &str, format: &str) -> (i32, Vec<String>) {
        fields_by(WINDOWS, input, format)
    }

    #[test]
    fn integers_strings_and_floats_are_read_with_their_sizes_and_widths() {
        let (count, seen) = fields("  42 -7 0x1f abc 2.5", "%d %hhd %x %s %lf");
        assert_eq!(count, 5);
        assert_eq!(
            seen,
            [
                "Signed(42, Int)",
                "Signed(-7, Byte)",
                "Unsigned(31, Int)",
                "Text([97, 98, 99])",
                "Float(2.5, Double)",
            ]
        );
        let (count, seen) = fields("12345", "%2d%3d");
        assert_eq!((count, seen.len()), (2, 2));
        assert_eq!(seen[1], "Signed(345, Int)");
    }

    #[test]
    fn literals_must_match_and_suppressed_fields_are_not_counted() {
        assert_eq!(fields("7x9", "%dx%d").0, 2);
        assert_eq!(fields("7y9", "%dx%d").0, 1);
        let (count, seen) = fields("1 2 3", "%d %*d %d%n");
        assert_eq!(count, 2);
        assert_eq!(seen[2], "Count(5, Int)");
        assert_eq!(fields("50%", "%d%%").0, 1);
    }

    #[test]
    fn sets_characters_and_the_end_of_input() {
        let (count, seen) = fields("abc123", "%[a-c]%2c");
        assert_eq!(count, 2);
        assert_eq!(seen, ["Text([97, 98, 99])", "Characters([49, 50])"]);
        assert_eq!(fields("x", "%[^x]").0, 0);
        assert_eq!(fields("", "%d").0, END_OF_INPUT);
        assert_eq!(fields("   ", "%d").0, END_OF_INPUT);
        assert_eq!(fields("5", "%d %d").0, 1);
        assert_eq!(fields("q", "%d").0, 0);
    }

    #[test]
    fn darwin_sizes_are_64_bit_for_l_q_z_and_extended_for_capital_l() {
        let (count, seen) = fields_by(DARWIN, "1 2 3 4 5 6.5", "%ld %lld %qd %zu %hd %Lf");
        assert_eq!(count, 6);
        assert_eq!(
            seen,
            [
                "Signed(1, Wide)",
                "Signed(2, Wide)",
                "Signed(3, Wide)",
                "Unsigned(4, Wide)",
                "Signed(5, Short)",
                "Float(6.5, Extended)",
            ]
        );
        assert_eq!(fields("7", "%ld").1, ["Signed(7, Int)"]);
        assert_eq!(fields_by(DARWIN, "0x10", "%p").1, ["Unsigned(16, Wide)"]);
    }

    #[test]
    fn what_a_number_does_not_use_is_read_again() {
        let (count, seen) = fields_by(DARWIN, "0xg", "%x%c");
        assert_eq!(count, 2);
        assert_eq!(seen, ["Unsigned(0, Int)", "Characters([120])"]);
        let (count, seen) = fields_by(DARWIN, "1e", "%f%c");
        assert_eq!(count, 2);
        assert_eq!(seen, ["Float(1.0, Single)", "Characters([101])"]);
        assert_eq!(fields_by(DARWIN, "0x", "%x").1, ["Unsigned(0, Int)"]);
        let (count, seen) = fields_by(DARWIN, "-x", "%d%c");
        assert_eq!((count, seen.len()), (0, 0));
        let (_, seen) = fields_by(DARWIN, "inf nan -2.5e-1", "%f %lf %e");
        assert_eq!(seen[0], "Float(inf, Single)");
        assert!(seen[1].starts_with("Float(NaN"));
        assert_eq!(seen[2], "Float(-0.25, Single)");
        assert_eq!(fields_by(DARWIN, "017 0x1F 9", "%i %i %i").0, 3);
        assert_eq!(fields_by(DARWIN, "017", "%i").1, ["Signed(15, Int)"]);
    }

    #[test]
    fn wide_conversions_take_whole_utf8_characters() {
        let (count, seen) = fields_by(DARWIN, "h\u{e9}llo w", "%ls %lc");
        assert_eq!(count, 2);
        assert_eq!(
            seen,
            [
                "WideText([104, 195, 169, 108, 108, 111])",
                "WideCharacters([119])"
            ]
        );
        let (_, seen) = fields_by(DARWIN, "\u{e9}a", "%lc");
        assert_eq!(seen, ["WideCharacters([195, 169])"]);
        let (_, seen) = fields_by(DARWIN, "\u{e9}\u{e9}\u{e9}", "%2ls");
        assert_eq!(seen, ["WideText([195, 169, 195, 169])"]);
        assert_eq!(fields("ab", "%ls").1, ["Text([97, 98])"]);
    }

    #[test]
    fn a_short_c_field_stores_what_there_was() {
        assert_eq!(fields_by(DARWIN, "ab", "%5c").1, ["Characters([97, 98])"]);
        let (count, seen) = fields_by(DARWIN, "abc", "%2s%n");
        assert_eq!(count, 1);
        assert_eq!(seen, ["Text([97, 98])", "Count(2, Int)"]);
    }
}
