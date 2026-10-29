//! The C runtime's text-to-number conversions (D145), `strtol` and its family, for narrow and
//! wide strings alike: blanks, a sign, a base prefix and digits, with C's rules for what a
//! value out of range becomes.

/// What a conversion read from the start of a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integer {
    /// The digits' value, if it fits in 64 bits.
    pub magnitude: Option<u64>,
    pub negative: bool,
    /// How many characters it took; 0 if there was no number, so the end pointer is the start.
    pub consumed: usize,
}

/// The integer at the start of `text` (a string without its NUL) in `base`, which is 0 (decimal,
/// octal after `0` or hexadecimal after `0x`) or 2 to 36; `None` for any other base.
pub fn parse_integer<T: Copy + Into<u32>>(text: &[T], base: u32) -> Option<Integer> {
    if base == 1 || base > 36 {
        return None;
    }
    let at = |index: usize| text.get(index).map(|&unit| unit.into());
    let mut index = 0;
    while at(index).is_some_and(is_space) {
        index += 1;
    }
    let negative = at(index) == Some(u32::from(b'-'));
    if negative || at(index) == Some(u32::from(b'+')) {
        index += 1;
    }
    let mut base = base;
    let hex_prefix = at(index) == Some(u32::from(b'0'))
        && matches!(at(index + 1), Some(x) if x == u32::from(b'x') || x == u32::from(b'X'))
        && at(index + 2).and_then(|unit| digit(unit, 16)).is_some();
    if (base == 0 || base == 16) && hex_prefix {
        base = 16;
        index += 2;
    } else if base == 0 {
        base = if at(index) == Some(u32::from(b'0')) {
            8
        } else {
            10
        };
    }
    let first_digit = index;
    let mut magnitude = Some(0u64);
    while let Some(value) = at(index).and_then(|unit| digit(unit, base)) {
        magnitude = magnitude
            .and_then(|sum| sum.checked_mul(u64::from(base)))
            .and_then(|sum| sum.checked_add(u64::from(value)));
        index += 1;
    }
    let consumed = if index == first_digit { 0 } else { index };
    Some(Integer {
        magnitude: if consumed == 0 { Some(0) } else { magnitude },
        negative: negative && consumed != 0,
        consumed,
    })
}

impl Integer {
    /// The value as a signed integer of `bits` bits (`strtol`, `strtoll`), and whether it was
    /// out of range, in which case it is the type's nearest limit.
    pub fn signed(&self, bits: u32) -> (i64, bool) {
        let limit = 1u64 << (bits - 1);
        match (self.magnitude, self.negative) {
            (Some(magnitude), false) if magnitude < limit => (magnitude as i64, false),
            (Some(magnitude), true) if magnitude <= limit => {
                ((magnitude as i64).wrapping_neg(), false)
            }
            (_, false) => ((limit - 1) as i64, true),
            (_, true) => ((limit as i64).wrapping_neg(), true),
        }
    }

    /// The value as an unsigned integer of `bits` bits (`strtoul`, `strtoull`): a negative
    /// value wraps around, as C says; and whether it was out of range, in which case it is the
    /// type's largest value.
    pub fn unsigned(&self, bits: u32) -> (u64, bool) {
        let largest = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        match self.magnitude {
            Some(magnitude) if magnitude <= largest => {
                let value = if self.negative {
                    magnitude.wrapping_neg() & largest
                } else {
                    magnitude
                };
                (value, false)
            }
            _ => (largest, true),
        }
    }
}

/// Whether `unit` is a C blank: space, tab, line feed, vertical tab, form feed or carriage
/// return.
pub fn is_space(unit: u32) -> bool {
    unit == u32::from(b' ') || (0x09..=0x0d).contains(&unit)
}

fn digit(unit: u32, base: u32) -> Option<u32> {
    char::from_u32(unit)?
        .to_digit(36)
        .filter(|value| *value < base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str, base: u32) -> Integer {
        parse_integer(text.as_bytes(), base).unwrap()
    }

    #[test]
    fn numbers_are_read_after_blanks_with_a_sign_and_a_base() {
        let decimal = parsed("  \t-42abc", 10);
        assert_eq!(
            (decimal.magnitude, decimal.negative, decimal.consumed),
            (Some(42), true, 6)
        );
        assert_eq!(decimal.signed(32), (-42, false));
        let hex = parsed("0x1F", 0);
        assert_eq!((hex.magnitude, hex.consumed), (Some(31), 4));
        assert_eq!(parsed("0X1f", 16).magnitude, Some(31));
        assert_eq!(parsed("017", 0).magnitude, Some(15));
        assert_eq!(parsed("z", 36).magnitude, Some(35));
        assert_eq!(parsed("+7", 0).signed(32), (7, false));
        let wide: Vec<u16> = "  99".encode_utf16().collect();
        assert_eq!(parse_integer(&wide, 10).unwrap().magnitude, Some(99));
    }

    #[test]
    fn no_digits_consume_nothing() {
        for text in ["", "   ", "-", "+x", "abc"] {
            let none = parsed(text, 10);
            assert_eq!((none.magnitude, none.consumed), (Some(0), 0), "{text:?}");
        }
        let zero = parsed("0xg", 0);
        assert_eq!((zero.magnitude, zero.consumed), (Some(0), 1));
        assert_eq!(parse_integer(b"1", 1), None);
        assert_eq!(parse_integer(b"1", 37), None);
    }

    #[test]
    fn values_out_of_range_become_the_limits() {
        assert_eq!(parsed("2147483647", 10).signed(32), (2_147_483_647, false));
        assert_eq!(parsed("2147483648", 10).signed(32), (2_147_483_647, true));
        assert_eq!(
            parsed("-2147483648", 10).signed(32),
            (-2_147_483_648, false)
        );
        assert_eq!(parsed("-2147483649", 10).signed(32), (-2_147_483_648, true));
        assert_eq!(
            parsed("99999999999999999999", 10).signed(64),
            (i64::MAX, true)
        );
        assert_eq!(
            parsed("-9223372036854775808", 10).signed(64),
            (i64::MIN, false)
        );
        assert_eq!(
            parsed("4294967295", 10).unsigned(32),
            (4_294_967_295, false)
        );
        assert_eq!(parsed("4294967296", 10).unsigned(32), (4_294_967_295, true));
        assert_eq!(parsed("-1", 10).unsigned(32), (4_294_967_295, false));
        assert_eq!(parsed("-1", 10).unsigned(64), (u64::MAX, false));
        assert_eq!(
            parsed("18446744073709551616", 10).unsigned(64),
            (u64::MAX, true)
        );
    }
}
