//! `scanf`'s family over bytes (D174), as the Universal C Runtime's `__stdio_common_vsscanf`
//! reads: white space in the format skips any in the input, other characters must match, and
//! each conversion (`%d`, `%i`, `%u`, `%o`, `%x`, `%p`, the `%e` family, `%c`, `%s`, `%[...]`,
//! `%n`, `%%`) with its optional `*`, width and size reads a field and hands it to the caller,
//! which stores it where the next argument points.

use crate::c_text;
use crate::float;

/// Where an integer conversion stores its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Byte,
    Short,
    Int,
    Wide,
}

/// One converted field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field<'a> {
    Signed(i64, Size),
    Unsigned(u64, Size),
    /// A floating-point value, and whether it is stored as a `double`.
    Float(f64, bool),
    /// `%c`'s characters, stored without a NUL.
    Characters(&'a [u8]),
    /// `%s`' and `%[`'s characters, stored with a NUL.
    Text(&'a [u8]),
    /// `%n`'s count of the input read so far.
    Count(usize, Size),
}

/// What [`scan`] ends with: the fields stored, or `EOF` (-1) when the input ended before the
/// first conversion.
pub const END_OF_INPUT: i32 = -1;

fn is_space(byte: u8) -> bool {
    byte == b' ' || (0x09..=0x0d).contains(&byte)
}

struct Spec {
    suppress: bool,
    width: Option<usize>,
    size: Size,
    double: bool,
    conversion: u8,
}

fn parse_spec(format: &[u8], mut at: usize) -> Option<(Spec, usize)> {
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
    let (mut size, mut double) = (Size::Int, false);
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
            double = true;
            1
        }
        [b'L', ..] => {
            double = true;
            1
        }
        [b'I', b'6', b'4', ..] => {
            size = Size::Wide;
            3
        }
        [b'I', b'3', b'2', ..] => 3,
        [b'I' | b'j' | b'z' | b't', ..] => {
            size = Size::Wide;
            1
        }
        [b'w', ..] => 1,
        _ => 0,
    };
    at += taken;
    let conversion = *format.get(at)?;
    if conversion == b'p' {
        size = Size::Wide;
    }
    Some((
        Spec {
            suppress,
            width,
            size,
            double,
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

/// Reads `input` as `format` says, handing each field not suppressed with `*` to `store`, which
/// answers whether it stored it (no argument for it ends the scan); the fields stored, `%n`'s
/// not counted, or [`END_OF_INPUT`] when the input ended before the first conversion.
pub fn scan(input: &[u8], format: &[u8], store: &mut dyn FnMut(Field<'_>) -> bool) -> i32 {
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
            while input.get(position).copied().is_some_and(is_space) {
                position += 1;
            }
            at += 1;
            continue;
        }
        if byte != b'%' {
            match input.get(position) {
                Some(&found) if found == byte => {
                    position += 1;
                    at += 1;
                    continue;
                }
                Some(_) => return stored,
                None => return ended(converted, stored),
            }
        }
        let Some((spec, mut next)) = parse_spec(format, at + 1) else {
            return stored;
        };
        if !matches!(spec.conversion, b'c' | b'[' | b'n') {
            while input.get(position).copied().is_some_and(is_space) {
                position += 1;
            }
        }
        if spec.conversion != b'n' && position >= input.len() {
            return ended(converted, stored);
        }
        let limit = spec.width.map_or(input.len(), |width| {
            position.saturating_add(width).min(input.len())
        });
        let field = &input[position..limit];
        let (taken, value) = match spec.conversion {
            b'%' => match field.first() {
                Some(b'%') => (1, None),
                _ => return stored,
            },
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                let base = match spec.conversion {
                    b'd' | b'u' => 10,
                    b'i' => 0,
                    b'o' => 8,
                    _ => 16,
                };
                let Some(number) = c_text::parse_integer(field, base) else {
                    return stored;
                };
                if number.consumed == 0 {
                    return stored;
                }
                let value = match spec.conversion {
                    b'd' | b'i' => Field::Signed(number.signed(64).0, spec.size),
                    _ => Field::Unsigned(number.unsigned(64).0, spec.size),
                };
                (number.consumed, Some(value))
            }
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
                let (value, consumed) = float::parse_float(field);
                if consumed == 0 {
                    return stored;
                }
                (consumed, Some(Field::Float(value, spec.double)))
            }
            b'c' => {
                let width = spec.width.unwrap_or(1);
                if input.len() - position < width {
                    return ended(converted, stored);
                }
                let characters = &input[position..position + width];
                (width, Some(Field::Characters(characters)))
            }
            b's' => {
                let length = field.iter().take_while(|byte| !is_space(**byte)).count();
                (length, Some(Field::Text(&field[..length])))
            }
            b'[' => {
                let Some(end) = set_end(format, next) else {
                    return stored;
                };
                let set = &format[next..end];
                next = end + 1;
                let length = field.iter().take_while(|byte| in_set(set, **byte)).count();
                if length == 0 {
                    return stored;
                }
                (length, Some(Field::Text(&field[..length])))
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

    fn fields(input: &str, format: &str) -> (i32, Vec<String>) {
        let mut seen = Vec::new();
        let count = scan(input.as_bytes(), format.as_bytes(), &mut |field| {
            seen.push(format!("{field:?}"));
            true
        });
        (count, seen)
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
                "Float(2.5, true)",
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
}
