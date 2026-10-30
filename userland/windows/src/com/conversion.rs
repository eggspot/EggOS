//! `VariantChangeType`: a value converted to another kind. Numbers convert among themselves
//! with range checks (`DISP_E_OVERFLOW`) and round half to even into integers and currency;
//! `VT_BOOL` is -1 or 0 as a number and any non-zero number is true; text reads decimal numbers
//! (spaces around, a sign, `,` between thousands, a point, an exponent, `&H` hex and `&O`
//! octal), `True`/`False` and dates (`date`), and numbers are written with up to 15 significant
//! digits (7 for `VT_R4`) in the English (United States) form, as `VarBstrFromR8` writes them;
//! `VT_BOOL` is written `-1`/`0`, or `True`/`False` with `VARIANT_ALPHABOOL` or
//! `VARIANT_LOCALBOOL`. Objects convert only to `IDispatch` or `IUnknown` (through
//! `QueryInterface`), arrays only to their own kind; nothing reaches an object's default value.

use super::variant::{self, Variant, kind};
use super::{AsciiText, E_INVALIDARG, E_OUTOFMEMORY, HResult, bstr, date, guid};

/// `DISP_E_TYPEMISMATCH`: the value has no form of the kind asked for.
pub const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
/// `DISP_E_OVERFLOW`: the value does not fit the kind asked for.
pub const DISP_E_OVERFLOW: HResult = 0x8002_000a_u32 as i32;

const VARIANT_ALPHABOOL: u16 = 0x2;
const VARIANT_LOCALBOOL: u16 = 0x10;
const CURRENCY_SCALE: i64 = 10_000;
const DOUBLE_FORMAT: &[u8] = b"%.15G";
const SINGLE_FORMAT: &[u8] = b"%.7G";
const EXACTLY_INTEGRAL: f64 = 4_503_599_627_370_496.0;
const LARGEST_NUMBER_TEXT: usize = 80;

/// A numeric value: an exact integer, a double, or currency in ten-thousandths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Integer(i128),
    Real(f64),
    Currency(i64),
}

fn decimal_value(value: &Variant) -> Number {
    let scale = (value.reserved[0] & 0xff) as i32;
    let negative = value.reserved[0] >> 8 != 0;
    let high = u128::from(value.reserved[1]) | (u128::from(value.reserved[2]) << 16);
    let magnitude = ((high << 64) | u128::from(value.data[0])) as i128;
    let signed = if negative { -magnitude } else { magnitude };
    match scale {
        0 => Number::Integer(signed),
        _ => {
            let mut real = signed as f64;
            for _ in 0..scale.min(28) {
                real /= 10.0;
            }
            Number::Real(real)
        }
    }
}

/// The number a numeric value holds (the integer kinds, `VT_R4`, `VT_R8`, `VT_CY`,
/// `VT_DECIMAL`); `None` for any other kind.
pub fn numeric_value(value: &Variant) -> Option<Number> {
    let word = value.data[0];
    Some(match value.kind {
        kind::I1 => Number::Integer(i128::from(word as u8 as i8)),
        kind::UI1 => Number::Integer(i128::from(word as u8)),
        kind::I2 => Number::Integer(i128::from(word as u16 as i16)),
        kind::UI2 => Number::Integer(i128::from(word as u16)),
        kind::I4 | kind::INT => Number::Integer(i128::from(word as u32 as i32)),
        kind::UI4 | kind::UINT => Number::Integer(i128::from(word as u32)),
        kind::I8 => Number::Integer(i128::from(word as i64)),
        kind::UI8 => Number::Integer(i128::from(word)),
        kind::R4 => Number::Real(f64::from(f32::from_bits(word as u32))),
        kind::R8 => Number::Real(f64::from_bits(word)),
        kind::CY => Number::Currency(word as i64),
        kind::DECIMAL => decimal_value(value),
        _ => return None,
    })
}

/// `value` rounded to the nearest integer, halves to the even one.
pub fn round_half_even(value: f64) -> f64 {
    if value.is_nan() || value.abs() >= EXACTLY_INTEGRAL {
        return value;
    }
    let truncated = value as i64;
    let fraction = (value - truncated as f64).abs();
    let away = if value < 0.0 { -1 } else { 1 };
    let rounded = if fraction > 0.5 || (fraction == 0.5 && truncated % 2 != 0) {
        truncated + away
    } else {
        truncated
    };
    rounded as f64
}

fn currency_to_integer(currency: i64) -> i128 {
    let scale = i128::from(CURRENCY_SCALE);
    let value = i128::from(currency);
    let (quotient, remainder) = (value / scale, value % scale);
    let away = if value < 0 { -1 } else { 1 };
    let half = scale / 2;
    match remainder.abs() {
        more if more > half => quotient + away,
        exactly if exactly == half && quotient % 2 != 0 => quotient + away,
        _ => quotient,
    }
}

fn to_integer(number: Number) -> Option<i128> {
    match number {
        Number::Integer(value) => Some(value),
        Number::Real(value) if value.is_finite() && value.abs() < 1.0e30 => {
            Some(round_half_even(value) as i128)
        }
        Number::Real(_) => None,
        Number::Currency(value) => Some(currency_to_integer(value)),
    }
}

fn to_real(number: Number) -> f64 {
    match number {
        Number::Integer(value) => value as f64,
        Number::Real(value) => value,
        Number::Currency(value) => value as f64 / CURRENCY_SCALE as f64,
    }
}

fn is_nonzero(number: Number) -> bool {
    match number {
        Number::Integer(value) => value != 0,
        Number::Real(value) => value != 0.0,
        Number::Currency(value) => value != 0,
    }
}

fn integer_range(target: u16) -> Option<(i128, i128)> {
    Some(match target {
        kind::I1 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        kind::UI1 => (0, i128::from(u8::MAX)),
        kind::I2 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        kind::UI2 => (0, i128::from(u16::MAX)),
        kind::I4 | kind::INT => (i128::from(i32::MIN), i128::from(i32::MAX)),
        kind::UI4 | kind::UINT => (0, i128::from(u32::MAX)),
        kind::I8 => (i128::from(i64::MIN), i128::from(i64::MAX)),
        kind::UI8 => (0, i128::from(u64::MAX)),
        _ => return None,
    })
}

fn store_number(number: Number, target: u16) -> Result<Variant, HResult> {
    match target {
        kind::R8 => Ok(Variant::new(kind::R8, to_real(number).to_bits())),
        kind::R4 => {
            let value = to_real(number);
            if value.is_finite() && value.abs() > f64::from(f32::MAX) {
                return Err(DISP_E_OVERFLOW);
            }
            Ok(Variant::new(kind::R4, u64::from((value as f32).to_bits())))
        }
        kind::DATE => {
            let value = to_real(number);
            date::moment(value).ok_or(DISP_E_OVERFLOW)?;
            Ok(Variant::new(kind::DATE, value.to_bits()))
        }
        kind::CY => {
            let scaled = match number {
                Number::Currency(value) => Some(i128::from(value)),
                Number::Integer(value) => value.checked_mul(i128::from(CURRENCY_SCALE)),
                Number::Real(value) => to_integer(Number::Real(value * CURRENCY_SCALE as f64)),
            };
            let value = scaled.and_then(|value| i64::try_from(value).ok());
            Ok(Variant::new(kind::CY, value.ok_or(DISP_E_OVERFLOW)? as u64))
        }
        kind::BOOL => Ok(Variant::boolean(is_nonzero(number))),
        _ => {
            let (lowest, highest) = integer_range(target).ok_or(DISP_E_TYPEMISMATCH)?;
            let value = to_integer(number).filter(|value| (lowest..=highest).contains(value));
            let value = value.ok_or(DISP_E_OVERFLOW)? as u64;
            let bits = variant::value_size(target).unwrap_or(8) * 8;
            let mask = u64::MAX >> (64 - bits);
            Ok(Variant::new(target, value & mask))
        }
    }
}

fn is_blank(unit: u16) -> bool {
    unit == u16::from(b' ') || (0x09..=0x0d).contains(&unit)
}

fn trim(text: &[u16]) -> &[u16] {
    let start = text.iter().position(|&unit| !is_blank(unit));
    let end = text.iter().rposition(|&unit| !is_blank(unit));
    match (start, end) {
        (Some(start), Some(end)) => &text[start..=end],
        _ => &[],
    }
}

fn equal_ignoring_ascii_case(text: &[u16], word: &[u8]) -> bool {
    text.len() == word.len()
        && text
            .iter()
            .zip(word)
            .all(|(&unit, &byte)| unit < 0x80 && (unit as u8).eq_ignore_ascii_case(&byte))
}

fn parse_radix(digits: &[u16], radix: u32) -> Option<i128> {
    if digits.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &unit in digits {
        let digit = char::from_u32(u32::from(unit))?.to_digit(radix)?;
        value = value
            .checked_mul(u64::from(radix))?
            .checked_add(u64::from(digit))?;
    }
    Some(i128::from(value))
}

/// The number `text` spells: blanks around it, a sign, then `&H` hex or `&O` octal digits, or
/// decimal digits with `,` between thousands, a point and an exponent; `None` for anything else.
pub fn parse_number(text: &[u16]) -> Option<Number> {
    let text = trim(text);
    let (negative, body) = match text.first() {
        Some(&unit) if unit == u16::from(b'-') => (true, &text[1..]),
        Some(&unit) if unit == u16::from(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if body.len() >= 2 && body[0] == u16::from(b'&') {
        let radix = match body[1] | 0x20 {
            0x68 => 16,
            0x6f => 8,
            _ => return None,
        };
        let value = parse_radix(&body[2..], radix)?;
        return Some(Number::Integer(if negative { -value } else { value }));
    }
    let mut ascii = [0u8; LARGEST_NUMBER_TEXT];
    ascii[0] = if negative { b'-' } else { b'+' };
    let mut length = 1;
    let (mut mantissa_digits, mut exponent_digits) = (0, 0);
    let (mut seen_point, mut seen_exponent) = (false, false);
    for (index, &unit) in body.iter().enumerate() {
        let byte = u8::try_from(unit).ok()?;
        match byte {
            b'0'..=b'9' if seen_exponent => exponent_digits += 1,
            b'0'..=b'9' => mantissa_digits += 1,
            b',' if !seen_point && !seen_exponent && mantissa_digits > 0 => continue,
            b'.' if !seen_point && !seen_exponent => seen_point = true,
            b'e' | b'E' if !seen_exponent && mantissa_digits > 0 => seen_exponent = true,
            b'+' | b'-' if index > 0 && (body[index - 1] | 0x20) == u16::from(b'e') => {}
            _ => return None,
        }
        *ascii.get_mut(length)? = byte;
        length += 1;
    }
    if mantissa_digits == 0 || (seen_exponent && exponent_digits == 0) {
        return None;
    }
    let ascii = &ascii[..length];
    if !seen_point && !seen_exponent {
        let mut value = 0i128;
        for &digit in &ascii[1..] {
            value = value
                .checked_mul(10)?
                .checked_add(i128::from(digit - b'0'))?;
        }
        return Some(Number::Integer(if negative { -value } else { value }));
    }
    let (value, used) = crate::float::parse_float(ascii);
    (used == ascii.len()).then_some(Number::Real(value))
}

#[derive(Debug)]
struct OneDouble(f64);

impl eggos_c_format::Arguments for OneDouble {
    fn integer(&mut self) -> u64 {
        0
    }

    fn double(&mut self) -> f64 {
        self.0
    }
}

fn write_real(text: &mut AsciiText, value: f64, format: &[u8]) {
    let value = if value == 0.0 { 0.0 } else { value };
    let dialect = eggos_c_format::Dialect::Windows {
        wide_strings: false,
    };
    // SAFETY: the format has no string conversion, so no argument is read as a pointer.
    unsafe { eggos_c_format::format_in(dialect, format, &mut OneDouble(value), text) };
}

fn write_integer(text: &mut AsciiText, value: i128) {
    if value < 0 {
        text.push(b"-");
    }
    text.number(value.unsigned_abs(), 1);
}

fn write_currency(text: &mut AsciiText, value: i64) {
    if value < 0 {
        text.push(b"-");
    }
    let magnitude = value.unsigned_abs();
    let scale = CURRENCY_SCALE as u64;
    text.number(u128::from(magnitude / scale), 1);
    let mut fraction = magnitude % scale;
    if fraction == 0 {
        return;
    }
    let mut digits = 4;
    while fraction.is_multiple_of(10) {
        fraction /= 10;
        digits -= 1;
    }
    text.push(b".");
    text.number(u128::from(fraction), digits);
}

/// The text of a number, as `VarBstrFromR8` and its family write it; `None` for a kind that is
/// not a number.
pub fn number_text(value: &Variant) -> Option<AsciiText> {
    let mut text = AsciiText::new();
    if value.kind == kind::R4 {
        let single = f64::from(f32::from_bits(value.data[0] as u32));
        write_real(&mut text, single, SINGLE_FORMAT);
        return Some(text);
    }
    match numeric_value(value)? {
        Number::Integer(integer) => write_integer(&mut text, integer),
        Number::Real(real) => write_real(&mut text, real, DOUBLE_FORMAT),
        Number::Currency(currency) => write_currency(&mut text, currency),
    }
    Some(text)
}

unsafe fn source_number(value: &Variant, target: u16) -> Result<Number, HResult> {
    if let Some(number) = numeric_value(value) {
        return Ok(number);
    }
    match value.kind {
        kind::EMPTY => Ok(Number::Integer(0)),
        kind::BOOL => Ok(Number::Integer(-i128::from(value.data[0] as u16 != 0))),
        kind::DATE => Ok(Number::Real(f64::from_bits(value.data[0]))),
        kind::BSTR => {
            // SAFETY: the caller's duty: the value holds a BSTR.
            let text = unsafe { bstr::units(value.pointer() as *const u16) };
            if target == kind::DATE
                && let Some(days) = date::parse(trim(text))
            {
                return Ok(Number::Real(days));
            }
            parse_number(text).ok_or(DISP_E_TYPEMISMATCH)
        }
        _ => Err(DISP_E_TYPEMISMATCH),
    }
}

unsafe fn boolean_of(value: &Variant) -> Result<bool, HResult> {
    if value.kind == kind::BSTR {
        // SAFETY: the caller's duty: the value holds a BSTR.
        let text = trim(unsafe { bstr::units(value.pointer() as *const u16) });
        if equal_ignoring_ascii_case(text, b"true") {
            return Ok(true);
        }
        if equal_ignoring_ascii_case(text, b"false") {
            return Ok(false);
        }
    }
    // SAFETY: the caller's duty.
    let number = unsafe { source_number(value, kind::BOOL) };
    number.map(is_nonzero)
}

fn text_of(value: &Variant, flags: u16) -> Result<Variant, HResult> {
    let text = match value.kind {
        kind::EMPTY => AsciiText::new(),
        kind::BOOL => {
            let named = flags & (VARIANT_ALPHABOOL | VARIANT_LOCALBOOL) != 0;
            let word: &[u8] = match (value.data[0] as u16 != 0, named) {
                (true, true) => b"True",
                (false, true) => b"False",
                (true, false) => b"-1",
                (false, false) => b"0",
            };
            let mut text = AsciiText::new();
            text.push(word);
            text
        }
        kind::DATE => date::text(f64::from_bits(value.data[0])).ok_or(DISP_E_OVERFLOW)?,
        _ => number_text(value).ok_or(DISP_E_TYPEMISMATCH)?,
    };
    let string = bstr::from_ascii(text.as_bytes());
    if string.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    Ok(Variant::new(kind::BSTR, string as u64))
}

unsafe fn object_of(value: &Variant, target: u16) -> Result<Variant, HResult> {
    let object = match value.kind {
        kind::EMPTY => 0,
        kind::DISPATCH | kind::UNKNOWN => value.pointer(),
        _ => return Err(DISP_E_TYPEMISMATCH),
    };
    if object == 0 {
        return Ok(Variant::new(target, 0));
    }
    let interface = match target {
        kind::DISPATCH => guid::DISPATCH,
        _ => guid::UNKNOWN,
    };
    let mut answer = 0usize;
    // SAFETY: the caller's duty: the value holds a live object.
    let code = unsafe { super::query_interface(object, &interface, &mut answer) };
    match code < 0 || answer == 0 {
        true => Err(DISP_E_TYPEMISMATCH),
        false => Ok(Variant::new(target, answer as u64)),
    }
}

/// `value` (no `VT_BYREF`) converted to `target`, as a new value the caller owns.
///
/// # Safety
/// `value` must be a valid VARIANT.
pub unsafe fn change(value: &Variant, flags: u16, target: u16) -> Result<Variant, HResult> {
    if !variant::is_valid(target) || target & kind::BYREF != 0 {
        return Err(variant::DISP_E_BADVARTYPE);
    }
    if value.kind == target {
        // SAFETY: the caller's duty.
        return unsafe { variant::duplicate(value) };
    }
    if (target | value.kind) & kind::ARRAY != 0 || value.kind == kind::NULL {
        return Err(DISP_E_TYPEMISMATCH);
    }
    match target {
        kind::EMPTY => Ok(Variant::EMPTY),
        kind::NULL if value.kind == kind::EMPTY => Ok(Variant::new(kind::NULL, 0)),
        kind::BSTR => text_of(value, flags),
        // SAFETY: the caller's duty.
        kind::DISPATCH | kind::UNKNOWN => unsafe { object_of(value, target) },
        // SAFETY: the caller's duty.
        kind::BOOL => unsafe { boolean_of(value) }.map(Variant::boolean),
        kind::R4 | kind::R8 | kind::CY | kind::DATE => {
            // SAFETY: the caller's duty.
            store_number(unsafe { source_number(value, target) }?, target)
        }
        _ if integer_range(target).is_some() => {
            // SAFETY: the caller's duty.
            store_number(unsafe { source_number(value, target) }?, target)
        }
        _ => Err(DISP_E_TYPEMISMATCH),
    }
}

unsafe fn change_type(
    destination: *mut Variant,
    source: *const Variant,
    flags: u16,
    target: u16,
) -> HResult {
    if destination.is_null() || source.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let original = unsafe { source.read_unaligned() };
    // SAFETY: as above.
    let value = match unsafe { variant::view(&original) } {
        Ok(value) => value,
        Err(code) => return code,
    };
    // SAFETY: `view` gave a valid value.
    match unsafe { change(&value, flags, target) } {
        // SAFETY: the caller's duty; the converted value is this call's own.
        Ok(changed) => unsafe { variant::replace(destination, changed) },
        Err(code) => code,
    }
}

/// `VariantChangeType`: `*source` converted to `target` in `*destination`, which may be the
/// same VARIANT; the destination is cleared first.
///
/// # Safety
/// Each must be null or point at a VARIANT.
pub unsafe extern "efiapi" fn variant_change_type(
    destination: *mut Variant,
    source: *const Variant,
    flags: u16,
    target: u16,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { change_type(destination, source, flags, target) }
}

/// `VariantChangeTypeEx`: as `VariantChangeType`; every locale reads and writes the English
/// (United States) forms.
///
/// # Safety
/// Each must be null or point at a VARIANT.
pub unsafe extern "efiapi" fn variant_change_type_ex(
    destination: *mut Variant,
    source: *const Variant,
    _locale: u32,
    flags: u16,
    target: u16,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { change_type(destination, source, flags, target) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn text_variant(text: &str) -> Variant {
        Variant::new(kind::BSTR, bstr::from_units(&wide(text)) as u64)
    }

    fn converted(value: Variant, flags: u16, target: u16) -> Result<Variant, HResult> {
        let mut source = value;
        let mut destination = Variant::EMPTY;
        // SAFETY: both are VARIANTs.
        let code = unsafe { variant_change_type(&mut destination, &source, flags, target) };
        // SAFETY: the source owns its value.
        unsafe { variant::clear(&mut source) };
        if code < 0 { Err(code) } else { Ok(destination) }
    }

    fn as_text(value: Variant) -> String {
        let mut value = converted(value, 0, kind::BSTR).unwrap();
        // SAFETY: the value holds a BSTR.
        let text = String::from_utf16(unsafe { bstr::units(value.pointer() as *const u16) });
        // SAFETY: the value owns its string.
        unsafe { variant::clear(&mut value) };
        text.unwrap()
    }

    fn from_text(text: &str, target: u16) -> Result<Variant, HResult> {
        converted(text_variant(text), 0, target)
    }

    #[test]
    fn numbers_are_written_as_decimal_text() {
        assert_eq!(as_text(Variant::long(42)), "42");
        assert_eq!(as_text(Variant::long(-7)), "-7");
        assert_eq!(as_text(Variant::new(kind::R8, 0.1f64.to_bits())), "0.1");
        assert_eq!(as_text(Variant::new(kind::R8, (-2.5f64).to_bits())), "-2.5");
        assert_eq!(as_text(Variant::new(kind::R8, 1e20f64.to_bits())), "1E+20");
        assert_eq!(as_text(Variant::new(kind::R8, 1e-5f64.to_bits())), "1E-05");
        assert_eq!(
            as_text(Variant::new(kind::R8, (1.0f64 / 3.0).to_bits())),
            "0.333333333333333"
        );
        assert_eq!(as_text(Variant::new(kind::R8, (-0.0f64).to_bits())), "0");
        assert_eq!(
            as_text(Variant::new(kind::R4, u64::from(0.1f32.to_bits()))),
            "0.1"
        );
        assert_eq!(as_text(Variant::new(kind::CY, 10_500)), "1.05");
        assert_eq!(as_text(Variant::new(kind::CY, (-20_000i64) as u64)), "-2");
        assert_eq!(
            as_text(Variant::new(kind::UI8, u64::MAX)),
            "18446744073709551615"
        );
        assert_eq!(as_text(Variant::new(kind::I2, 0xffff)), "-1");
        assert_eq!(as_text(Variant::EMPTY), "");
    }

    #[test]
    fn booleans_and_dates_are_written() {
        assert_eq!(as_text(Variant::boolean(true)), "-1");
        assert_eq!(as_text(Variant::boolean(false)), "0");
        let mut named = converted(Variant::boolean(true), VARIANT_ALPHABOOL, kind::BSTR).unwrap();
        // SAFETY: the value holds a BSTR.
        assert_eq!(
            unsafe { bstr::units(named.pointer() as *const u16) },
            &wide("True")[..]
        );
        // SAFETY: the value owns its string.
        unsafe { variant::clear(&mut named) };
        let date = Variant::new(kind::DATE, 36_526.75f64.to_bits());
        assert_eq!(as_text(date), "1/1/2000 6:00:00 PM");
    }

    #[test]
    fn text_is_read_as_numbers() {
        assert_eq!(from_text(" 12.5 ", kind::I4), Ok(Variant::long(12)));
        assert_eq!(from_text("13.5", kind::I4), Ok(Variant::long(14)));
        assert_eq!(from_text("-0.5", kind::I4), Ok(Variant::long(0)));
        assert_eq!(from_text("1,234", kind::I4), Ok(Variant::long(1234)));
        assert_eq!(from_text("&HFF", kind::I4), Ok(Variant::long(255)));
        assert_eq!(from_text("&o17", kind::I4), Ok(Variant::long(15)));
        assert_eq!(from_text("1e3", kind::I2), Ok(Variant::new(kind::I2, 1000)));
        assert_eq!(
            from_text("-1", kind::I2),
            Ok(Variant::new(kind::I2, 0xffff))
        );
        assert_eq!(from_text("300", kind::UI1), Err(DISP_E_OVERFLOW));
        assert_eq!(from_text("-1", kind::UI4), Err(DISP_E_OVERFLOW));
        assert_eq!(from_text("abc", kind::I4), Err(DISP_E_TYPEMISMATCH));
        assert_eq!(from_text("", kind::I4), Err(DISP_E_TYPEMISMATCH));
        assert_eq!(from_text("1e", kind::R8), Err(DISP_E_TYPEMISMATCH));
        assert_eq!(from_text("1.2.3", kind::R8), Err(DISP_E_TYPEMISMATCH));
        assert_eq!(
            from_text("1.23456", kind::CY),
            Ok(Variant::new(kind::CY, 12_346))
        );
        let big = from_text("9223372036854775807", kind::I8);
        assert_eq!(big, Ok(Variant::new(kind::I8, i64::MAX as u64)));
        assert_eq!(from_text("True", kind::BOOL), Ok(Variant::boolean(true)));
        assert_eq!(from_text("false", kind::BOOL), Ok(Variant::boolean(false)));
        assert_eq!(from_text("2", kind::BOOL), Ok(Variant::boolean(true)));
        let date = from_text("1/1/2000", kind::DATE);
        assert_eq!(date, Ok(Variant::new(kind::DATE, 36_526.0f64.to_bits())));
    }

    #[test]
    fn numbers_survive_a_trip_through_text() {
        for value in [0.1, 123.456, -7.25, 1e-5, 6.02214076e23, 1.0 / 3.0, 255.0] {
            let text = as_text(Variant::new(kind::R8, f64::to_bits(value)));
            let back = from_text(&text, kind::R8).unwrap();
            let expected = format!("{value:.14e}");
            let got = format!("{:.14e}", f64::from_bits(back.data[0]));
            assert_eq!(got, expected, "{text}");
        }
    }

    #[test]
    fn numbers_convert_among_kinds_with_range_checks() {
        let real = |value: f64| Variant::new(kind::R8, value.to_bits());
        assert_eq!(
            converted(real(2.5), 0, kind::I2),
            Ok(Variant::new(kind::I2, 2))
        );
        assert_eq!(
            converted(real(3.5), 0, kind::I2),
            Ok(Variant::new(kind::I2, 4))
        );
        assert_eq!(converted(real(-3.5), 0, kind::I4), Ok(Variant::long(-4)));
        assert_eq!(converted(real(1e10), 0, kind::I4), Err(DISP_E_OVERFLOW));
        assert_eq!(converted(real(1e300), 0, kind::R4), Err(DISP_E_OVERFLOW));
        assert_eq!(converted(real(f64::NAN), 0, kind::I4), Err(DISP_E_OVERFLOW));
        assert_eq!(converted(Variant::long(3), 0, kind::R8), Ok(real(3.0)));
        assert_eq!(
            converted(Variant::boolean(true), 0, kind::I4),
            Ok(Variant::long(-1))
        );
        assert_eq!(
            converted(Variant::long(0), 0, kind::BOOL),
            Ok(Variant::boolean(false))
        );
        let currency = Variant::new(kind::CY, 25_000);
        assert_eq!(converted(currency, 0, kind::I4), Ok(Variant::long(2)));
        let two = Variant::new(kind::CY, 20_000);
        assert_eq!(converted(Variant::long(2), 0, kind::CY), Ok(two));
        assert_eq!(converted(Variant::EMPTY, 0, kind::I4), Ok(Variant::long(0)));
        let null = Variant::new(kind::NULL, 0);
        assert_eq!(converted(null, 0, kind::I4), Err(DISP_E_TYPEMISMATCH));
        let bad = converted(Variant::long(1), 0, kind::VARIANT);
        assert_eq!(bad, Err(variant::DISP_E_BADVARTYPE));
    }

    #[test]
    fn rounding_goes_to_even_on_halves() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(-2.5), -2.0);
        assert_eq!(round_half_even(2.6), 3.0);
        assert_eq!(round_half_even(-2.4), -2.0);
        assert_eq!(currency_to_integer(15_000), 2);
        assert_eq!(currency_to_integer(25_000), 2);
        assert_eq!(currency_to_integer(-35_000), -4);
    }

    #[test]
    fn a_value_converts_in_place() {
        let mut value = text_variant("41");
        let place = &raw mut value;
        // SAFETY: the same VARIANT on both sides.
        let code = unsafe { variant_change_type(place, place, 0, kind::I4) };
        assert_eq!((code, value), (0, Variant::long(41)));
    }
}
