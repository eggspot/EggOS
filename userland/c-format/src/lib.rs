//! `printf`'s formatting, shared by the macOS layer's libSystem and the Windows layer's C
//! runtime (D146): flags, width, precision and length modifiers and the conversions
//! `d i u o x X c s p f F e E g G %`, reading each argument from an [`Arguments`] source and
//! writing UTF-8 to an [`Output`]. Where the two C libraries differ, a [`Dialect`] says whose
//! rules apply: Darwin's adds Foundation's `%@` (an object's description, which the [`Output`]
//! writes) and `%C` (a character: a UTF-16 unit or a `wint_t`); Windows' has a 32-bit `l`,
//! `I64`, `I32` and `I`, wide (UTF-16) strings and characters through `%S`, `%C`, `%ls`, `%ws`
//! and `%lc` (narrow ones through `%hs` and `%hc`), and `%p` as 16 upper-case hex digits. `%n`
//! writes nothing: it is skipped with its argument, so a format string cannot write to memory.

#![cfg_attr(not(test), no_std)]

/// Where the variadic arguments come from.
pub trait Arguments {
    /// The next integer or pointer argument, as its 64 bits.
    fn integer(&mut self) -> u64;
    /// The next floating-point argument.
    fn double(&mut self) -> f64;
    /// The next `long double` argument (`%Lf`), as the nearest double; a `long double` is a
    /// double on arm64.
    fn long_double(&mut self) -> f64 {
        self.double()
    }
}

/// Where the formatted bytes go.
pub trait Output {
    fn write(&mut self, bytes: &[u8]);
    /// Writes the description of the object `%@` names and returns how many bytes it wrote;
    /// C's formatting writes nothing for it.
    fn object(&mut self, _object: u64) -> usize {
        0
    }
}

/// Whose C library's rules the formatting follows where they differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Darwin's libSystem, with Foundation's `%@`.
    Darwin,
    /// Microsoft's C runtime. `wide_strings` is true for the wide functions (`swprintf` and its
    /// family) with Microsoft's own specifiers, the default: there `%s` and `%c` take wide
    /// strings and characters and `%S` and `%C` narrow ones; otherwise it is the other way
    /// round.
    Windows { wide_strings: bool },
}

/// Formats `format` with `arguments` into `output` by Darwin's rules and returns how many bytes
/// it wrote.
///
/// # Safety
/// Each `%s` argument must be null or point to a NUL-terminated string (or, with a precision,
/// to at least that many readable bytes).
pub unsafe fn format(
    format: &[u8],
    arguments: &mut impl Arguments,
    output: &mut impl Output,
) -> usize {
    // SAFETY: the caller's promise.
    unsafe { format_in(Dialect::Darwin, format, arguments, output) }
}

/// Formats `format` with `arguments` into `output` by `dialect`'s rules and returns how many
/// bytes it wrote.
///
/// # Safety
/// Each string argument must be null or point to a NUL-terminated string of its width (bytes,
/// or UTF-16 units for a wide one), or, with a precision, to at least that many readable
/// elements.
pub unsafe fn format_in(
    dialect: Dialect,
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
        let (spec, next) = Spec::parse(dialect, format, at + 1, arguments);
        at = next;
        // SAFETY: the caller's promise about string arguments.
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
    dialect: Dialect,
    wide: Option<bool>,
    left: bool,
    plus: bool,
    space: bool,
    alternate: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
    length: Length,
    long_double: bool,
    conversion: u8,
}

const MAX_WIDTH: usize = 4096;

impl Spec {
    fn parse(
        dialect: Dialect,
        format: &[u8],
        mut at: usize,
        arguments: &mut impl Arguments,
    ) -> (Self, usize) {
        let mut spec = Self {
            dialect,
            wide: None,
            left: false,
            plus: false,
            space: false,
            alternate: false,
            zero: false,
            width: 0,
            precision: None,
            length: Length::Int,
            long_double: false,
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
        at = match dialect {
            Dialect::Darwin => spec.darwin_modifiers(format, at),
            Dialect::Windows { .. } => spec.windows_modifiers(format, at),
        };
        spec.conversion = format.get(at).copied().unwrap_or(0);
        (spec, (at + 1).min(format.len()))
    }

    fn darwin_modifiers(&mut self, format: &[u8], mut at: usize) -> usize {
        while let Some(&modifier) = format.get(at) {
            self.long_double |= modifier == b'L';
            self.length = match (modifier, self.length) {
                (b'h', Length::Short) => Length::Char,
                (b'h', _) => Length::Short,
                (b'l' | b'q' | b'j' | b'z' | b't' | b'L', _) => Length::Long,
                _ => break,
            };
            at += 1;
        }
        at
    }

    fn windows_modifiers(&mut self, format: &[u8], mut at: usize) -> usize {
        let mut ells = 0;
        while let Some(&modifier) = format.get(at) {
            at += 1;
            match modifier {
                b'h' => {
                    self.wide = Some(false);
                    self.length = match self.length {
                        Length::Short => Length::Char,
                        _ => Length::Short,
                    };
                }
                b'l' => {
                    ells += 1;
                    self.wide = Some(true);
                    self.length = if ells > 1 { Length::Long } else { Length::Int };
                }
                b'w' => self.wide = Some(true),
                b'j' | b'z' | b't' => self.length = Length::Long,
                b'L' => self.long_double = true,
                b'I' => match format.get(at..at + 2) {
                    Some(b"64") => {
                        self.length = Length::Long;
                        at += 2;
                    }
                    Some(b"32") => {
                        self.length = Length::Int;
                        at += 2;
                    }
                    _ => self.length = Length::Long,
                },
                _ => return at - 1,
            }
        }
        at
    }

    fn wide_by_default(&self) -> bool {
        match self.dialect {
            Dialect::Windows { wide_strings } => {
                wide_strings == self.conversion.is_ascii_lowercase()
            }
            Dialect::Darwin => false,
        }
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
            b'p' if matches!(self.dialect, Dialect::Windows { .. }) => {
                let mut digits = [0u8; 24];
                let text = radix(arguments.integer(), 16, true, &mut digits);
                let mut padded = [b'0'; POINTER_DIGITS];
                padded[POINTER_DIGITS - text.len()..].copy_from_slice(text);
                self.pad(&padded, out);
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
            b'c' | b'C' | b's' | b'S'
                if matches!(self.dialect, Dialect::Windows { .. })
                    && self.wide.unwrap_or_else(|| self.wide_by_default()) =>
            {
                let value = arguments.integer();
                match self.conversion.to_ascii_lowercase() {
                    b'c' => {
                        let unit = [value as u16];
                        // SAFETY: one unit, read from this array.
                        unsafe { self.wide_text(unit.as_ptr(), Some(1), out) };
                    }
                    _ => {
                        let text = value as usize as *const u16;
                        // SAFETY: the caller's promise: a NUL-terminated UTF-16 string, or one
                        // with at least `precision` readable units.
                        unsafe { self.wide_text(text, self.precision, out) };
                    }
                }
            }
            b'c' | b'C' if matches!(self.dialect, Dialect::Windows { .. }) => {
                let byte = [arguments.integer() as u8];
                self.pad(&byte, out);
            }
            b'S' if matches!(self.dialect, Dialect::Windows { .. }) => {
                // SAFETY: the caller's promise about narrow strings.
                unsafe { self.narrow_text(arguments, out) };
            }
            b'c' => {
                let byte = [arguments.integer() as u8];
                self.pad(&byte, out);
            }
            b's' => {
                // SAFETY: the caller's promise about narrow strings.
                unsafe { self.narrow_text(arguments, out) };
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let value = if self.long_double {
                    arguments.long_double()
                } else {
                    arguments.double()
                };
                let mut buffer = [0u8; 400];
                let (sign, text) = self.float(value, &mut buffer);
                self.number(sign, b"", text, value.is_finite(), out);
            }
            b'n' => {
                arguments.integer();
            }
            b'@' => {
                let object = arguments.integer();
                out.count += out.output.object(object);
            }
            b'C' => {
                let value = arguments.integer() as u32;
                let code = if (0xd800..0xe000).contains(&value) || value > 0x10_ffff {
                    0xfffd
                } else {
                    value
                };
                let mut buffer = [0u8; 4];
                self.pad(utf8(code, &mut buffer), out);
            }
            b'%' => out.write(b"%"),
            0 => {}
            other => out.write(&[b'%', other]),
        }
    }

    unsafe fn narrow_text<O: Output>(
        &self,
        arguments: &mut impl Arguments,
        out: &mut Counting<'_, O>,
    ) {
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

    unsafe fn wide_text<O: Output>(
        &self,
        pointer: *const u16,
        limit: Option<usize>,
        out: &mut Counting<'_, O>,
    ) {
        let units: &[u16] = if pointer.is_null() {
            &WIDE_NULL
        } else {
            // SAFETY: the caller's promise: a NUL-terminated string, or one with at least
            // `limit` readable units.
            unsafe { wide_c_string(pointer, limit) }
        };
        let units = &units[..limit.map_or(units.len(), |limit| limit.min(units.len()))];
        let fill = self.width.saturating_sub(units.len());
        if !self.left {
            out.repeat(b' ', fill);
        }
        let mut bytes = [0u8; 64];
        let mut length = 0;
        for character in char::decode_utf16(units.iter().copied()) {
            if length + 4 > bytes.len() {
                out.write(&bytes[..length]);
                length = 0;
            }
            let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
            length += character.encode_utf8(&mut bytes[length..]).len();
        }
        out.write(&bytes[..length]);
        if self.left {
            out.repeat(b' ', fill);
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
const POINTER_DIGITS: usize = 16;
const WIDE_NULL: [u16; 6] = [
    b'(' as u16,
    b'n' as u16,
    b'u' as u16,
    b'l' as u16,
    b'l' as u16,
    b')' as u16,
];

fn utf8(code: u32, buffer: &mut [u8; 4]) -> &[u8] {
    if code < 0x80 {
        buffer[0] = code as u8;
        &buffer[..1]
    } else if code < 0x800 {
        buffer[0] = 0xc0 | (code >> 6) as u8;
        buffer[1] = 0x80 | (code & 0x3f) as u8;
        &buffer[..2]
    } else if code < 0x10000 {
        buffer[0] = 0xe0 | (code >> 12) as u8;
        buffer[1] = 0x80 | ((code >> 6) & 0x3f) as u8;
        buffer[2] = 0x80 | (code & 0x3f) as u8;
        &buffer[..3]
    } else {
        buffer[0] = 0xf0 | (code >> 18) as u8;
        buffer[1] = 0x80 | ((code >> 12) & 0x3f) as u8;
        buffer[2] = 0x80 | ((code >> 6) & 0x3f) as u8;
        buffer[3] = 0x80 | (code & 0x3f) as u8;
        &buffer[..4]
    }
}

/// The double nearest to the x87 80-bit extended value with the 64-bit `significand` (its
/// integer bit explicit) and the 16-bit `sign_exponent` (sign, then a bias of 16383).
#[cfg(any(test, target_arch = "x86_64"))]
pub fn extended_to_double(significand: u64, sign_exponent: u16) -> f64 {
    let negative = sign_exponent & 0x8000 != 0;
    let exponent = i32::from(sign_exponent & 0x7fff);
    let magnitude = if exponent == 0x7fff {
        if significand << 1 == 0 {
            f64::INFINITY
        } else {
            f64::NAN
        }
    } else {
        let mut value = significand as f64;
        let mut scale = exponent - 16383 - 63;
        while scale > 0 && value.is_finite() {
            let step = scale.min(1000);
            value *= f64::from_bits(((1023 + step) as u64) << 52);
            scale -= step;
        }
        while scale < 0 && value != 0.0 {
            let step = scale.max(-1000);
            value *= f64::from_bits(((1023 + step) as u64) << 52);
            scale -= step;
        }
        value
    };
    if negative { -magnitude } else { magnitude }
}

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

unsafe fn wide_c_string<'a>(pointer: *const u16, limit: Option<usize>) -> &'a [u16] {
    let mut length = 0;
    while limit.is_none_or(|limit| length < limit) {
        // SAFETY: the caller's promise: every unit up to the NUL (or the limit) is readable.
        if unsafe { pointer.wrapping_add(length).read() } == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: the `length` units just read.
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
    fn x87_extended_values_become_the_nearest_double() {
        assert_eq!(extended_to_double(1 << 63, 16383), 1.0);
        assert_eq!(
            extended_to_double(0xc000_0000_0000_0000, 16384 | 0x8000),
            -3.0
        );
        assert_eq!(extended_to_double(0xa000_0000_0000_0000, 16383 - 1), 0.625);
        assert_eq!(extended_to_double(0, 0), 0.0);
        assert!(extended_to_double(0, 0x8000).is_sign_negative());
        assert_eq!(extended_to_double(1 << 63, 0x7fff), f64::INFINITY);
        assert!(extended_to_double(3 << 62, 0x7fff).is_nan());
        assert_eq!(extended_to_double(1 << 63, 0x7ffe), f64::INFINITY);
        assert_eq!(extended_to_double(1 << 63, 1), 0.0);
        let mut out = Vec::new();
        struct LongDouble;
        impl Arguments for LongDouble {
            fn integer(&mut self) -> u64 {
                0
            }
            fn double(&mut self) -> f64 {
                1.0
            }
            fn long_double(&mut self) -> f64 {
                2.5
            }
        }
        // SAFETY: the format has no `%s`.
        unsafe { format(b"%.1Lf %.1f", &mut LongDouble, &mut out) };
        assert_eq!(out, b"2.5 1.0");
    }

    #[test]
    fn objects_and_utf16_units_are_written_by_the_output() {
        struct Describing(Vec<u8>);
        impl Output for Describing {
            fn write(&mut self, bytes: &[u8]) {
                self.0.extend_from_slice(bytes);
            }
            fn object(&mut self, object: u64) -> usize {
                let text = format!("<object {object}>");
                self.0.extend_from_slice(text.as_bytes());
                text.len()
            }
        }
        struct Words(Vec<u64>);
        impl Arguments for Words {
            fn integer(&mut self) -> u64 {
                self.0.remove(0)
            }
            fn double(&mut self) -> f64 {
                0.0
            }
        }
        let mut out = Describing(Vec::new());
        // SAFETY: no `%s` conversion reads a pointer.
        let count = unsafe {
            format(
                b"%@ %d %C|%@%C",
                &mut Words(vec![7, 42, 0xe9, 9, 0x1f95a]),
                &mut out,
            )
        };
        assert_eq!(out.0, "<object 7> 42 \u{e9}|<object 9>\u{1f95a}".as_bytes());
        assert_eq!(count, out.0.len());
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

    fn windows(text: &str, wide_strings: bool, values: &[Value]) -> String {
        let mut out = Vec::new();
        let dialect = Dialect::Windows { wide_strings };
        // SAFETY: the tests pass string arguments that are NUL-terminated literals.
        let count = unsafe { format_in(dialect, text.as_bytes(), &mut List(values, 0), &mut out) };
        assert_eq!(count, out.len());
        String::from_utf8(out).unwrap()
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    #[test]
    fn windows_lengths_are_its_own() {
        assert_eq!(
            windows(
                "%ld %lu %lld %I64d %I32d %Id %zu %hd",
                false,
                &[
                    Int(0xffff_ffff),
                    Int(0x1_0000_0002),
                    Int(-5i64 as u64),
                    Int(-6i64 as u64),
                    Int(0x1_0000_0007),
                    Int(1 << 40),
                    Int(9),
                    Int(0x1_ffff),
                ]
            ),
            "-1 2 -5 -6 7 1099511627776 9 -1"
        );
        assert_eq!(
            windows("%p|%p", false, &[Int(0xabc), Int(0)]),
            "0000000000000ABC|0000000000000000"
        );
    }

    #[test]
    fn windows_strings_take_their_width_from_the_dialect_and_the_modifiers() {
        let narrow = c"caf\u{e9}".as_ptr() as u64;
        let unicode = wide("caf\u{e9} \u{1f95a}");
        let unicode = unicode.as_ptr() as u64;
        assert_eq!(
            windows(
                "%s|%S|%ls|%ws|%hs|%c|%C|%lc",
                false,
                &[
                    Int(narrow),
                    Int(unicode),
                    Int(unicode),
                    Int(unicode),
                    Int(narrow),
                    Int(u64::from(b'x')),
                    Int(0xe9),
                    Int(0x263a),
                ]
            ),
            "caf\u{e9}|caf\u{e9} \u{1f95a}|caf\u{e9} \u{1f95a}|caf\u{e9} \u{1f95a}|caf\u{e9}|x|\u{e9}|\u{263a}"
        );
        assert_eq!(
            windows(
                "%s|%S|%hs|%c|%C",
                true,
                &[
                    Int(unicode),
                    Int(narrow),
                    Int(narrow),
                    Int(0x263a),
                    Int(u64::from(b'y')),
                ]
            ),
            "caf\u{e9} \u{1f95a}|caf\u{e9}|caf\u{e9}|\u{263a}|y"
        );
    }

    #[test]
    fn windows_wide_strings_pad_and_cut_by_units() {
        let text = wide("abcdef");
        let text = text.as_ptr() as u64;
        assert_eq!(
            windows(
                "[%8ls] [%-8ls] [%.3ls] [%ls]",
                false,
                &[Int(text), Int(text), Int(text), Int(0)]
            ),
            "[  abcdef] [abcdef  ] [abc] [(null)]"
        );
    }
}
