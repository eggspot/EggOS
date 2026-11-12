//! How msvcp's `basic_ostream` writes numbers and `basic_istream` reads them (D405), from the
//! stream's format flags (`ios_base::fmtflags`, MSVC's values), precision, width and fill, as
//! MSVC's `num_put` and `num_get` do in the classic locale: integers in their base with the
//! sign or prefix the flags ask for (a negative number in hex or octal as its two's complement,
//! as `printf`'s `%x` gives it), floating point through the C runtime's `printf` rules, and the
//! field padded on the side the adjustment names.

use alloc::vec::Vec;

/// `ios_base`'s format flags, as MSVC numbers them.
pub mod flags {
    pub const SKIPWS: i32 = 0x1;
    pub const UNITBUF: i32 = 0x2;
    pub const UPPERCASE: i32 = 0x4;
    pub const SHOWBASE: i32 = 0x8;
    pub const SHOWPOINT: i32 = 0x10;
    pub const SHOWPOS: i32 = 0x20;
    pub const LEFT: i32 = 0x40;
    pub const RIGHT: i32 = 0x80;
    pub const INTERNAL: i32 = 0x100;
    pub const DEC: i32 = 0x200;
    pub const OCT: i32 = 0x400;
    pub const HEX: i32 = 0x800;
    pub const SCIENTIFIC: i32 = 0x1000;
    pub const FIXED: i32 = 0x2000;
    pub const BOOLALPHA: i32 = 0x4000;
    pub const ADJUSTFIELD: i32 = LEFT | RIGHT | INTERNAL;
    pub const BASEFIELD: i32 = DEC | OCT | HEX;
    pub const FLOATFIELD: i32 = SCIENTIFIC | FIXED;
}

/// An integer as the stream holds it: its bits, whether its type is signed and its width.
#[derive(Debug, Clone, Copy)]
pub struct Integer {
    pub bits: u64,
    pub signed: bool,
    pub width: u32,
}

impl Integer {
    fn negative(&self) -> bool {
        self.signed && self.bits >> (self.width - 1) & 1 == 1
    }

    fn magnitude(&self) -> u64 {
        let mask = match self.width {
            64 => u64::MAX,
            width => (1u64 << width) - 1,
        };
        let bits = self.bits & mask;
        match self.negative() {
            true => (!bits).wrapping_add(1) & mask,
            false => bits,
        }
    }

    fn unsigned_bits(&self) -> u64 {
        match self.width {
            64 => self.bits,
            width => self.bits & ((1u64 << width) - 1),
        }
    }
}

fn digits(mut value: u64, base: u64, upper: bool, out: &mut Vec<u8>) {
    let alphabet: &[u8; 16] = match upper {
        true => b"0123456789ABCDEF",
        false => b"0123456789abcdef",
    };
    let start = out.len();
    loop {
        out.push(alphabet[(value % base) as usize]);
        value /= base;
        if value == 0 {
            break;
        }
    }
    out[start..].reverse();
}

/// `value`'s text under `format` flags and where its sign or base prefix ends (the place
/// `internal` padding goes).
pub fn integer(value: Integer, format: i32) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let upper = format & flags::UPPERCASE != 0;
    let base = match format & flags::BASEFIELD {
        flags::OCT => 8,
        flags::HEX => 16,
        _ => 10,
    };
    if base == 10 {
        let negative = value.negative();
        if negative {
            out.push(b'-');
        } else if value.signed && format & flags::SHOWPOS != 0 {
            out.push(b'+');
        }
        let prefix = out.len();
        digits(value.magnitude(), 10, upper, &mut out);
        return (out, prefix);
    }
    let bits = value.unsigned_bits();
    if format & flags::SHOWBASE != 0 && bits != 0 {
        out.push(b'0');
        if base == 16 {
            out.push(if upper { b'X' } else { b'x' });
        }
    }
    let prefix = out.len();
    digits(bits, base, upper, &mut out);
    (out, prefix)
}

/// The `printf` conversion `num_put` uses for a floating-point value under `format`: flags,
/// `.*` for the precision unless it is hexadecimal, and the conversion letter.
pub fn float_specification(format: i32, long_double: bool) -> Vec<u8> {
    let mut spec = alloc::vec![b'%'];
    if format & flags::SHOWPOS != 0 {
        spec.push(b'+');
    }
    if format & flags::SHOWPOINT != 0 {
        spec.push(b'#');
    }
    let field = format & flags::FLOATFIELD;
    if field != flags::FLOATFIELD {
        spec.extend_from_slice(b".*");
    }
    if long_double {
        spec.push(b'L');
    }
    let upper = format & flags::UPPERCASE != 0;
    spec.push(match (field, upper) {
        (flags::FIXED, false) => b'f',
        (flags::FIXED, true) => b'F',
        (flags::SCIENTIFIC, false) => b'e',
        (flags::SCIENTIFIC, true) => b'E',
        (flags::FLOATFIELD, false) => b'a',
        (flags::FLOATFIELD, true) => b'A',
        (_, false) => b'g',
        (_, true) => b'G',
    });
    spec.push(0);
    spec
}

/// Where a formatted number's sign (and `0x`) ends, for `internal` padding.
pub fn float_prefix(text: &[u8]) -> usize {
    let mut at = usize::from(matches!(text.first(), Some(b'-' | b'+')));
    if text[at..].starts_with(b"0x") || text[at..].starts_with(b"0X") {
        at += 2;
    }
    at
}

/// `body` padded with `fill` to `width` as `format`'s adjustment says, `prefix` bytes of it
/// (sign, base) staying in front for `internal`.
pub fn pad(body: &[u8], prefix: usize, format: i32, width: i64, fill: u8) -> Vec<u8> {
    let wanted = usize::try_from(width).unwrap_or(0);
    if wanted <= body.len() {
        return body.to_vec();
    }
    let padding = wanted - body.len();
    let mut out = Vec::with_capacity(wanted);
    match format & flags::ADJUSTFIELD {
        flags::LEFT => {
            out.extend_from_slice(body);
            out.resize(wanted, fill);
        }
        flags::INTERNAL => {
            out.extend_from_slice(&body[..prefix]);
            out.resize(prefix + padding, fill);
            out.extend_from_slice(&body[prefix..]);
        }
        _ => {
            out.resize(padding, fill);
            out.extend_from_slice(body);
        }
    }
    out
}

/// What reading an integer gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// The value's bits.
    Value(u64),
    /// Digits, but too large for the type: the type's limit, and the stream fails.
    Overflow(u64),
    /// No number: the stream fails.
    Nothing,
}

/// Reads an integer of `width` bits (`signed` or not) from `text`, in the base `format` names
/// (any of 8, 10 and 16 with its prefix when it names none), as `num_get` does; what it read
/// and how many bytes it used.
pub fn read_integer(text: &[u8], format: i32, signed: bool, width: u32) -> (Read, usize) {
    let mut at = 0;
    let negative = match text.first() {
        Some(b'-') => {
            at = 1;
            true
        }
        Some(b'+') => {
            at = 1;
            false
        }
        _ => false,
    };
    let mut base = match format & flags::BASEFIELD {
        flags::OCT => 8,
        flags::HEX => 16,
        flags::DEC => 10,
        _ => 0,
    };
    let has_prefix = text[at..].starts_with(b"0x") || text[at..].starts_with(b"0X");
    if (base == 16 || base == 0) && has_prefix {
        at += 2;
        base = 16;
    } else if base == 0 {
        base = if text.get(at) == Some(&b'0') { 8 } else { 10 };
    }
    let start = at;
    let mut value: u128 = 0;
    while let Some(digit) = text.get(at).and_then(|byte| (*byte as char).to_digit(base)) {
        value = value.saturating_mul(u128::from(base)).saturating_add(u128::from(digit));
        at += 1;
    }
    if at == start {
        return (Read::Nothing, at);
    }
    let limit: u128 = match signed {
        true => (1u128 << (width - 1)) - u128::from(!negative),
        false => (1u128 << width) - 1,
    };
    let fits = value <= limit;
    let mask = (1u128 << width) - 1;
    let bits = match negative {
        true => (value.wrapping_neg()) & mask,
        false => value & mask,
    };
    match fits {
        true => (Read::Value(bits as u64), at),
        false => {
            let edge = match (signed, negative) {
                (true, true) => (1u128 << (width - 1)) & mask,
                (true, false) => (1u128 << (width - 1)) - 1,
                (false, _) => mask,
            };
            (Read::Overflow(edge as u64), at)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(value: i64) -> Integer {
        Integer {
            bits: value as u64,
            signed: true,
            width: 32,
        }
    }

    fn text(value: Integer, format: i32) -> String {
        String::from_utf8(integer(value, format).0).unwrap()
    }

    #[test]
    fn integers_follow_the_base_sign_and_prefix_flags() {
        assert_eq!(text(int(-42), flags::DEC), "-42");
        assert_eq!(text(int(42), flags::DEC | flags::SHOWPOS), "+42");
        assert_eq!(text(int(-1), flags::HEX), "ffffffff");
        assert_eq!(text(int(255), flags::HEX | flags::SHOWBASE | flags::UPPERCASE), "0XFF");
        assert_eq!(text(int(8), flags::OCT | flags::SHOWBASE), "010");
        assert_eq!(text(int(0), flags::HEX | flags::SHOWBASE), "0");
        let unsigned = Integer {
            bits: u64::MAX,
            signed: false,
            width: 64,
        };
        assert_eq!(text(unsigned, flags::DEC | flags::SHOWPOS), "18446744073709551615");
    }

    #[test]
    fn padding_goes_where_the_adjustment_says() {
        let (body, prefix) = integer(int(-7), flags::DEC);
        assert_eq!(pad(&body, prefix, flags::DEC, 5, b'*'), b"***-7");
        assert_eq!(pad(&body, prefix, flags::LEFT, 5, b'*'), b"-7***");
        assert_eq!(pad(&body, prefix, flags::INTERNAL, 5, b'0'), b"-0007");
        assert_eq!(pad(&body, prefix, flags::DEC, 1, b'*'), b"-7");
    }

    #[test]
    fn floats_ask_printf_as_num_put_does() {
        assert_eq!(float_specification(0, false), b"%.*g\0");
        assert_eq!(float_specification(flags::FIXED | flags::SHOWPOS, false), b"%+.*f\0");
        assert_eq!(float_specification(flags::SCIENTIFIC | flags::UPPERCASE, true), b"%.*LE\0");
        assert_eq!(float_specification(flags::FLOATFIELD, false), b"%a\0");
        assert_eq!(float_prefix(b"-0x1.8p+1"), 3);
    }

    #[test]
    fn reading_takes_the_base_and_stops_at_the_first_non_digit() {
        assert_eq!(read_integer(b"123 x", flags::DEC, true, 32), (Read::Value(123), 3));
        let minus_five = -5i32 as u32 as u64;
        assert_eq!(read_integer(b"-5", flags::DEC, true, 32), (Read::Value(minus_five), 2));
        assert_eq!(read_integer(b"ff", flags::HEX, false, 32), (Read::Value(255), 2));
        assert_eq!(read_integer(b"0x10", 0, true, 32), (Read::Value(16), 4));
        assert_eq!(read_integer(b"x", flags::DEC, true, 32), (Read::Nothing, 0));
        assert_eq!(
            read_integer(b"99999999999", flags::DEC, true, 32),
            (Read::Overflow(i32::MAX as u64), 11)
        );
        assert_eq!(
            read_integer(b"-99999999999", flags::DEC, true, 32),
            (Read::Overflow(0x8000_0000), 12)
        );
    }
}
