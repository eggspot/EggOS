//! `printf`'s formatting: flags, width, precision and length modifiers and the conversions
//! `d i u o x X c s p f F e E g G %`, reading each argument from an [`Arguments`] source and
//! writing to an [`Output`]. `%n` writes nothing: it is skipped with its argument, so a format
//! string cannot write to memory.

/// Where the variadic arguments come from.
pub trait Arguments {
    /// The next integer or pointer argument, as its 64 bits.
    fn integer(&mut self) -> u64;
    /// The next floating-point argument.
    fn double(&mut self) -> f64;
}

/// Where the formatted bytes go.
pub trait Output {
    fn write(&mut self, bytes: &[u8]);
}

/// Formats `format` with `arguments` into `output` and returns how many bytes it wrote.
///
/// # Safety
/// Each `%s` argument must be null or point to a NUL-terminated string (or, with a precision,
/// to at least that many readable bytes).
pub unsafe fn format(
    format: &[u8],
    arguments: &mut impl Arguments,
    output: &mut impl Output,
) -> usize {
    let mut counter = Counting { output, count: 0 };
    let mut at = 0;
    while at < format.len() {
        let literal_end = format[at..]
            .iter()
            .position(|&b| b == b'%')
            .map_or(format.len(), |offset| at + offset);
        counter.write(&format[at..literal_end]);
        at = literal_end;
        if at >= format.len() {
            break;
        }
        let (spec, next) = Spec::parse(format, at + 1, arguments);
        at = next;
        // SAFETY: the caller's promise about `%s` arguments.
        unsafe { spec.convert(arguments, &mut counter) };
    }
    counter.count
}

struct Counting<'a, O: Output> {
    output: &'a mut O,
    count: usize,
}

impl<O: Output> Counting<'_, O> {
    fn write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.output.write(bytes);
            self.count += bytes.len();
        }
    }

    fn repeat(&mut self, byte: u8, count: usize) {
        let chunk = [byte; 32];
        let mut left = count;
        while left > 0 {
            let taken = left.min(chunk.len());
            self.write(&chunk[..taken]);
            left -= taken;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Length {
    Char,
    Short,
    Int,
    Long,
}

struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alternate: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
    length: Length,
    conversion: u8,
}

const MAX_WIDTH: usize = 4096;

impl Spec {
    fn parse(format: &[u8], mut at: usize, arguments: &mut impl Arguments) -> (Self, usize) {
        let mut spec = Self {
            left: false,
            plus: false,
            space: false,
            alternate: false,
            zero: false,
            width: 0,
            precision: None,
            length: Length::Int,
            conversion: 0,
        };
        while let Some(&flag) = format.get(at) {
            match flag {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alternate = true,
                b'0' => spec.zero = true,
                b'\'' => {}
                _ => break,
            }
            at += 1;
        }
        if format.get(at) == Some(&b'*') {
            let width = arguments.integer() as i32;
            spec.left |= width < 0;
            spec.width = (width.unsigned_abs() as usize).min(MAX_WIDTH);
            at += 1;
        } else {
            (spec.width, at) = number(format, at);
        }
        if format.get(at) == Some(&b'.') {
            at += 1;
            if format.get(at) == Some(&b'*') {
                let precision = arguments.integer() as i32;
                spec.precision = (precision >= 0).then_some((precision as usize).min(MAX_WIDTH));
                at += 1;
            } else {
                let (precision, next) = number(format, at);
                spec.precision = Some(precision);
                at = next;
            }
        }
        while let Some(&modifier) = format.get(at) {
            spec.length = match (modifier, spec.length) {
                (b'h', Length::Short) => Length::Char,
                (b'h', _) => Length::Short,
                (b'l' | b'q' | b'j' | b'z' | b't' | b'L', _) => Length::Long,
                _ => break,
            };
            at += 1;
        }
        spec.conversion = format.get(at).copied().unwrap_or(0);
        (spec, (at + 1).min(format.len()))
    }

    unsafe fn convert<O: Output>(&self, arguments: &mut impl Arguments, out: &mut Counting<'_, O>) {
        match self.conversion {
            b'd' | b'i' => {
                let value = self.signed(arguments.integer());
                let mut digits = [0u8; 24];
                let text = decimal(value.unsigned_abs(), &mut digits);
                let sign: &[u8] = if value < 0 {
                    b"-"
                } else if self.plus {
                    b"+"
                } else if self.space {
                    b" "
                } else {
                    b""
                };
                self.integer(sign, b"", text, out);
            }
            b'u' | b'o' | b'x' | b'X' | b'p' => {
                let value = if self.conversion == b'p' {
                    arguments.integer()
                } else {
                    self.unsigned(arguments.integer())
                };
                let mut digits = [0u8; 24];
                let (text, prefix): (&[u8], &[u8]) = match self.conversion {
                    b'u' => (decimal(value, &mut digits), b""),
                    b'o' => (
                        radix(value, 8, false, &mut digits),
                        if self.alternate && value != 0 {
                            b"0"
                        } else {
                            b""
                        },
                    ),
                    b'x' => (
                        radix(value, 16, false, &mut digits),
                        if self.alternate && value != 0 {
                            b"0x"
                        } else {
                            b""
                        },
                    ),
                    b'X' => (
                        radix(value, 16, true, &mut digits),
                        if self.alternate && value != 0 {
                            b"0X"
                        } else {
                            b""
                        },
                    ),
                    _ => (radix(value, 16, false, &mut digits), b"0x"),
                };
                self.integer(b"", prefix, text, out);
            }
            b'c' => {
                let byte = [arguments.integer() as u8];
                self.pad(&byte, out);
            }
            b's' => {
                let pointer = arguments.integer() as usize as *const u8;
                let text: &[u8] = if pointer.is_null() {
                    b"(null)"
                } else {
                    // SAFETY: the caller's promise: a NUL-terminated string, or one with at least
                    // `precision` readable bytes.
                    unsafe { c_string(pointer, self.precision) }
                };
                let text = &text[..self.precision.map_or(text.len(), |p| p.min(text.len()))];
                self.pad(text, out);
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let value = arguments.double();
                let mut buffer = [0u8; 400];
                let (sign, text) = self.float(value, &mut buffer);
                self.number(sign, b"", text, value.is_finite(), out);
            }
            b'n' => {
                arguments.integer();
            }
            b'%' => out.write(b"%"),
            0 => {}
            other => out.write(&[b'%', other]),
        }
    }

    fn signed(&self, raw: u64) -> i64 {
        match self.length {
            Length::Char => raw as i8 as i64,
            Length::Short => raw as i16 as i64,
            Length::Int => raw as i32 as i64,
            Length::Long => raw as i64,
        }
    }

    fn unsigned(&self, raw: u64) -> u64 {
        match self.length {
            Length::Char => raw as u8 as u64,
            Length::Short => raw as u16 as u64,
            Length::Int => raw as u32 as u64,
            Length::Long => raw,
        }
    }

    fn pad<O: Output>(&self, text: &[u8], out: &mut Counting<'_, O>) {
        let fill = self.width.saturating_sub(text.len());
        if !self.left {
            out.repeat(b' ', fill);
        }
        out.write(text);
        if self.left {
            out.repeat(b' ', fill);
        }
    }

    fn integer<O: Output>(
        &self,
        sign: &[u8],
        prefix: &[u8],
        digits: &[u8],
        out: &mut Counting<'_, O>,
    ) {
        let digits = if self.precision == Some(0) && digits == b"0" {
            &b""[..]
        } else {
            digits
        };
        let precision_zeros = self.precision.map_or(0, |p| p.saturating_sub(digits.len()));
        let body = sign.len() + prefix.len() + precision_zeros + digits.len();
        let fill = self.width.saturating_sub(body);
        let zero_fill = self.zero && !self.left && self.precision.is_none();
        if !self.left && !zero_fill {
            out.repeat(b' ', fill);
        }
        out.write(sign);
        out.write(prefix);
        if zero_fill {
            out.repeat(b'0', fill);
        }
        out.repeat(b'0', precision_zeros);
        out.write(digits);
        if self.left {
            out.repeat(b' ', fill);
        }
    }

    fn number<O: Output>(
        &self,
        sign: &[u8],
        prefix: &[u8],
        digits: &[u8],
        finite: bool,
        out: &mut Counting<'_, O>,
    ) {
        let body = sign.len() + prefix.len() + digits.len();
        let fill = self.width.saturating_sub(body);
        let zero_fill = self.zero && !self.left && finite;
        if !self.left && !zero_fill {
            out.repeat(b' ', fill);
        }
        out.write(sign);
        out.write(prefix);
        if zero_fill {
            out.repeat(b'0', fill);
        }
        out.write(digits);
        if self.left {
            out.repeat(b' ', fill);
        }
    }

    fn float<'b>(&self, value: f64, buffer: &'b mut [u8; 400]) -> (&'static [u8], &'b [u8]) {
        let sign: &'static [u8] = if value.is_sign_negative() && !value.is_nan() {
            b"-"
        } else if self.plus {
            b"+"
        } else if self.space {
            b" "
        } else {
            b""
        };
        let upper = self.conversion.is_ascii_uppercase();
        let magnitude = value.abs();
        let mut writer = Buffer {
            bytes: buffer,
            length: 0,
        };
        if !magnitude.is_finite() {
            let text: &[u8] = match (magnitude.is_nan(), upper) {
                (true, false) => b"nan",
                (true, true) => b"NAN",
                (false, false) => b"inf",
                (false, true) => b"INF",
            };
            writer.push(text);
            let length = writer.length;
            return (sign, &buffer[..length]);
        }
        let precision = self.precision.unwrap_or(6).min(MAX_FLOAT_PRECISION);
        match self.conversion.to_ascii_lowercase() {
            b'f' => fixed(magnitude, precision, self.alternate, &mut writer),
            b'e' => exponential(magnitude, precision, self.alternate, upper, &mut writer),
            _ => {
                let precision = precision.max(1);
                let exponent = if magnitude == 0.0 {
                    0
                } else {
                    decimal_exponent(round_to_significant(magnitude, precision))
                };
                let start = writer.length;
                if exponent < -4 || exponent >= precision as i32 {
                    exponential(magnitude, precision - 1, self.alternate, upper, &mut writer);
                } else {
                    let decimals = (precision as i32 - 1 - exponent) as usize;
                    fixed(magnitude, decimals, self.alternate, &mut writer);
                }
                if !self.alternate {
                    writer.trim_fraction_zeros(start);
                }
            }
        }
        let length = writer.length;
        (sign, &buffer[..length])
    }
}

const MAX_FLOAT_PRECISION: usize = 40;

struct Buffer<'b> {
    bytes: &'b mut [u8; 400],
    length: usize,
}

impl Buffer<'_> {
    fn push(&mut self, text: &[u8]) {
        let room = self.bytes.len() - self.length;
        let taken = text.len().min(room);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text[..taken]);
        self.length += taken;
    }

    fn trim_fraction_zeros(&mut self, start: usize) {
        let text = &self.bytes[start..self.length];
        let Some(point) = text.iter().position(|&b| b == b'.') else {
            return;
        };
        let fraction_end = text[point..]
            .iter()
            .position(|&b| b == b'e' || b == b'E')
            .map_or(text.len(), |offset| point + offset);
        let mut keep = fraction_end;
        while keep > point + 1 && text[keep - 1] == b'0' {
            keep -= 1;
        }
        if keep == point + 1 {
            keep = point;
        }
        let removed = fraction_end - keep;
        if removed == 0 {
            return;
        }
        self.bytes
            .copy_within(start + fraction_end..self.length, start + keep);
        self.length -= removed;
    }
}

fn powers_of_ten(count: i32) -> f64 {
    let mut value = 1.0;
    for _ in 0..count.unsigned_abs() {
        value *= 10.0;
    }
    if count < 0 { 1.0 / value } else { value }
}

fn decimal_exponent(magnitude: f64) -> i32 {
    let mut exponent = 0;
    let mut scaled = magnitude;
    while scaled >= 10.0 && exponent < 400 {
        scaled /= 10.0;
        exponent += 1;
    }
    while scaled < 1.0 && scaled > 0.0 && exponent > -400 {
        scaled *= 10.0;
        exponent -= 1;
    }
    exponent
}

fn round_to_significant(magnitude: f64, digits: usize) -> f64 {
    let exponent = decimal_exponent(magnitude);
    let scale = powers_of_ten(digits as i32 - 1 - exponent);
    round_half_even(magnitude * scale) / scale
}

fn round_half_even(value: f64) -> f64 {
    let floor = floor(value);
    let rest = value - floor;
    if rest > 0.5 || (rest == 0.5 && floor < 9.007_199_254_740_992e15 && floor as u64 % 2 == 1) {
        floor + 1.0
    } else {
        floor
    }
}

fn floor(value: f64) -> f64 {
    if value >= 9.007_199_254_740_992e15 {
        return value;
    }
    let truncated = value as u64 as f64;
    if truncated > value {
        truncated - 1.0
    } else {
        truncated
    }
}

fn digits_of(mut value: f64, writer: &mut Buffer<'_>) {
    let mut reversed = [0u8; 330];
    let mut count = 0;
    loop {
        let quotient = floor(value / 10.0);
        let digit = (value - quotient * 10.0).clamp(0.0, 9.0) as u8;
        reversed[count] = b'0' + digit;
        count += 1;
        value = quotient;
        if value < 1.0 || count == reversed.len() {
            break;
        }
    }
    reversed[..count].reverse();
    writer.push(&reversed[..count]);
}

fn fixed(magnitude: f64, precision: usize, alternate: bool, writer: &mut Buffer<'_>) {
    let scale = powers_of_ten(precision as i32);
    let scaled = magnitude * scale;
    if scaled < 1.8e19 {
        let rounded = round_half_even(scaled) as u64;
        let unit = 10u64.pow(precision.min(19) as u32);
        let mut digits = [0u8; 24];
        if precision <= 19 {
            writer.push(decimal(rounded / unit, &mut digits));
            if precision > 0 || alternate {
                writer.push(b".");
            }
            if precision > 0 {
                let fraction = decimal(rounded % unit, &mut digits);
                for _ in fraction.len()..precision {
                    writer.push(b"0");
                }
                writer.push(fraction);
            }
            return;
        }
    }
    let whole = floor(magnitude);
    digits_of(whole, writer);
    if precision > 0 || alternate {
        writer.push(b".");
    }
    let mut fraction = magnitude - whole;
    for _ in 0..precision {
        fraction *= 10.0;
        let digit = floor(fraction).clamp(0.0, 9.0);
        writer.push(&[b'0' + digit as u8]);
        fraction -= digit;
    }
}

fn exponential(
    magnitude: f64,
    precision: usize,
    alternate: bool,
    upper: bool,
    writer: &mut Buffer<'_>,
) {
    let (mut mantissa, mut exponent) = if magnitude == 0.0 {
        (0.0, 0)
    } else {
        let exponent = decimal_exponent(magnitude);
        (magnitude / powers_of_ten(exponent), exponent)
    };
    let scale = powers_of_ten(precision as i32);
    if round_half_even(mantissa * scale) >= 10.0 * scale {
        mantissa /= 10.0;
        exponent += 1;
    }
    fixed(mantissa, precision, alternate, writer);
    writer.push(if upper { b"E" } else { b"e" });
    writer.push(if exponent < 0 { b"-" } else { b"+" });
    let mut digits = [0u8; 24];
    let text = decimal(u64::from(exponent.unsigned_abs()), &mut digits);
    if text.len() < 2 {
        writer.push(b"0");
    }
    writer.push(text);
}

fn number(format: &[u8], mut at: usize) -> (usize, usize) {
    let mut value: usize = 0;
    while let Some(&digit) = format.get(at).filter(|b| b.is_ascii_digit()) {
        value = (value * 10 + usize::from(digit - b'0')).min(MAX_WIDTH);
        at += 1;
    }
    (value, at)
}

/// `value` in decimal, in the end of `digits`.
pub fn decimal(value: u64, digits: &mut [u8; 24]) -> &[u8] {
    radix(value, 10, false, digits)
}

fn radix(mut value: u64, base: u64, upper: bool, digits: &mut [u8; 24]) -> &[u8] {
    let alphabet: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut first = digits.len();
    loop {
        first -= 1;
        digits[first] = alphabet[(value % base) as usize];
        value /= base;
        if value == 0 {
            return &digits[first..];
        }
    }
}

unsafe fn c_string<'a>(pointer: *const u8, limit: Option<usize>) -> &'a [u8] {
    let mut length = 0;
    while limit.is_none_or(|limit| length < limit) {
        // SAFETY: the caller's promise: every byte up to the NUL (or the limit) is readable.
        if unsafe { pointer.wrapping_add(length).read() } == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: the `length` bytes just read.
    unsafe { core::slice::from_raw_parts(pointer, length) }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Value {
        Int(u64),
        Float(f64),
    }

    struct List<'a>(&'a [Value], usize);

    impl Arguments for List<'_> {
        fn integer(&mut self) -> u64 {
            self.1 += 1;
            match self.0.get(self.1 - 1) {
                Some(Value::Int(value)) => *value,
                _ => 0,
            }
        }

        fn double(&mut self) -> f64 {
            self.1 += 1;
            match self.0.get(self.1 - 1) {
                Some(Value::Float(value)) => *value,
                _ => 0.0,
            }
        }
    }

    impl Output for Vec<u8> {
        fn write(&mut self, bytes: &[u8]) {
            self.extend_from_slice(bytes);
        }
    }

    fn formatted(text: &str, values: &[Value]) -> String {
        let mut out = Vec::new();
        // SAFETY: the tests pass `%s` arguments that are C string literals.
        let count = unsafe { format(text.as_bytes(), &mut List(values, 0), &mut out) };
        assert_eq!(count, out.len());
        String::from_utf8(out).unwrap()
    }

    use Value::{Float, Int};

    #[test]
    fn integers_follow_their_length_and_flags() {
        assert_eq!(
            formatted(
                "%d %i %u %ld %lld %zu %x %X %o %c %%",
                &[
                    Int(-42i64 as u64),
                    Int(7),
                    Int(3_000_000_000),
                    Int(-1_234_567_890_123i64 as u64),
                    Int(9_000_000_000_000_000_000),
                    Int(12),
                    Int(0xbeef),
                    Int(0xbeef),
                    Int(8),
                    Int(b'Z'.into()),
                ]
            ),
            "-42 7 3000000000 -1234567890123 9000000000000000000 12 beef BEEF 10 Z %"
        );
        assert_eq!(formatted("%d", &[Int(0xffff_ffff)]), "-1");
        assert_eq!(formatted("%hhd %hu", &[Int(0x1ff), Int(0x1_0001)]), "-1 1");
    }

    #[test]
    fn widths_precisions_and_flags_pad_as_c_does() {
        let abc = c"abcdef".as_ptr() as u64;
        assert_eq!(
            formatted(
                "[%5d] [%-5d] [%05d] [%+d] [%.3s] [%8s] [%-8s] [%#x] [%*d] [%.3d] [% d] [%-*d]",
                &[
                    Int(42),
                    Int(42),
                    Int(42),
                    Int(42),
                    Int(abc),
                    Int(c"right".as_ptr() as u64),
                    Int(c"left".as_ptr() as u64),
                    Int(255),
                    Int(4),
                    Int(7),
                    Int(5),
                    Int(3),
                    Int(-3i64 as u64),
                    Int(9),
                ]
            ),
            "[   42] [42   ] [00042] [+42] [abc] [   right] [left    ] [0xff] [   7] [005] [ 3] [9  ]"
        );
        assert_eq!(
            formatted("%s|%p|%.0d", &[Int(0), Int(0x1000), Int(0)]),
            "(null)|0x1000|"
        );
        assert_eq!(
            formatted("%-05d|%08.3d", &[Int(1), Int(2)]),
            "1    |     002"
        );
    }

    #[test]
    fn floating_point_matches_the_c_library() {
        assert_eq!(
            formatted(
                "%.2f %f %.0f %e %g",
                &[
                    Float(3.25),
                    Float(-0.5),
                    Float(2.5),
                    Float(12345.678),
                    Float(0.0001)
                ]
            ),
            "3.25 -0.500000 2 1.234568e+04 0.0001"
        );
        assert_eq!(
            formatted(
                "%g %g %g %G %.3g %#g %8.2f|%-8.1f|%08.2f",
                &[
                    Float(100000.0),
                    Float(1000000.0),
                    Float(0.00001),
                    Float(1e-10),
                    Float(1.23456),
                    Float(1.0),
                    Float(1.23456),
                    Float(2.0),
                    Float(-1.5)
                ]
            ),
            "100000 1e+06 1e-05 1E-10 1.23 1.00000     1.23|2.0     |-0001.50"
        );
        assert_eq!(
            formatted(
                "%f %F %e %.1e %f",
                &[
                    Float(f64::INFINITY),
                    Float(f64::NEG_INFINITY),
                    Float(0.0),
                    Float(9.96),
                    Float(1e20)
                ]
            ),
            "inf -INF 0.000000e+00 1.0e+01 100000000000000000000.000000"
        );
    }

    #[test]
    fn malformed_formats_end_cleanly() {
        assert_eq!(formatted("100%", &[]), "100");
        assert_eq!(formatted("%y %n|", &[Int(1)]), "%y |");
        assert_eq!(formatted("%99999999999d", &[Int(1)]).len(), MAX_WIDTH);
        assert_eq!(
            formatted("%.*s", &[Int(-1i64 as u64), Int(c"ab".as_ptr() as u64)]),
            "ab"
        );
    }
}
