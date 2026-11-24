//! The classic locale's other facets (D748), for programs that ask `use_facet` for them:
//! `numpunct<char>` and `numpunct<wchar_t>` (a point, a comma, no grouping, `true` and
//! `false`), `codecvt<wchar_t, char, mbstate_t>` converting between UTF-16 and the layer's
//! narrow text (UTF-8, D178), and `num_put` for both, writing numbers to an
//! `ostreambuf_iterator` as the streams' `operator<<` does (`crate::stream_format`).

use super::{
    Collected, Export, FloatArguments, IosBase, Locale, LocaleId, Shared, Streambuf, Table,
    facet_add_reference, facet_destroy, facet_release, put_bytes,
};
use crate::stream_format::{self, Integer};
use alloc::vec::Vec;
use core::cell::UnsafeCell;

const CATEGORY_CTYPE: usize = 2;
const CATEGORY_NUMERIC: usize = 4;
const SHORT_BYTES: usize = 15;
const SHORT_UNITS: usize = 7;
const OK: i32 = 0;
const PARTIAL: i32 = 1;
const ERROR: i32 = 2;
const NO_CONVERSION: i32 = 3;
const LONGEST_UTF8: i32 = 4;

#[repr(C)]
struct Facet {
    table: *const *const (),
    references: u32,
    reserved: [u32; 13],
}

#[repr(C)]
struct StdString {
    text: [u8; 16],
    size: usize,
    capacity: usize,
}

#[repr(C)]
struct Iterator {
    failed: u8,
    reserved: [u8; 7],
    buffer: *mut Streambuf,
}

macro_rules! facet {
    ($name:ident, $table:ident) => {
        static $name: Shared<Facet> = Shared::new(Facet {
            table: (&raw const $table.0).cast(),
            references: 1,
            reserved: [0; 13],
        });
    };
}

macro_rules! facet_pointer {
    ($static:ident) => {
        UnsafeCell::raw_get(&raw const $static.0)
            .cast_const()
            .cast::<u8>()
    };
}

facet!(NUMPUNCT, NUMPUNCT_TABLE);
facet!(WIDE_NUMPUNCT, WIDE_NUMPUNCT_TABLE);
facet!(CODECVT, CODECVT_TABLE);
facet!(NUM_PUT, NUM_PUT_TABLE);
facet!(WIDE_NUM_PUT, WIDE_NUM_PUT_TABLE);

/// The classic locale's facets 3 to 7.
pub(super) const NUMPUNCT_FACET: *const u8 = facet_pointer!(NUMPUNCT);
pub(super) const WIDE_NUMPUNCT_FACET: *const u8 = facet_pointer!(WIDE_NUMPUNCT);
pub(super) const CODECVT_FACET: *const u8 = facet_pointer!(CODECVT);
pub(super) const NUM_PUT_FACET: *const u8 = facet_pointer!(NUM_PUT);
pub(super) const WIDE_NUM_PUT_FACET: *const u8 = facet_pointer!(WIDE_NUM_PUT);

static NUMPUNCT_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 3 });
static WIDE_NUMPUNCT_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 4 });
static CODECVT_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 5 });
static NUM_PUT_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 6 });
static WIDE_NUM_PUT_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 7 });

static NUMPUNCT_TABLE: Table<8> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(decimal_point),
    export_entry!(thousands_separator),
    export_entry!(grouping),
    export_entry!(false_name),
    export_entry!(true_name),
]);

static WIDE_NUMPUNCT_TABLE: Table<8> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(wide_decimal_point),
    export_entry!(wide_thousands_separator),
    export_entry!(grouping),
    export_entry!(wide_false_name),
    export_entry!(wide_true_name),
]);

static CODECVT_TABLE: Table<10> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(always_no_conversion),
    export_entry!(longest),
    export_entry!(encoding),
    export_entry!(convert_in),
    export_entry!(convert_out),
    export_entry!(unshift),
    export_entry!(length),
]);

static NUM_PUT_TABLE: Table<11> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(put_pointer),
    export_entry!(put_long_double),
    export_entry!(put_double),
    export_entry!(put_unsigned_long_long),
    export_entry!(put_long_long),
    export_entry!(put_unsigned_long),
    export_entry!(put_long),
    export_entry!(put_bool),
]);

static WIDE_NUM_PUT_TABLE: Table<11> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(wide_put_pointer),
    export_entry!(wide_put_long_double),
    export_entry!(wide_put_double),
    export_entry!(wide_put_unsigned_long_long),
    export_entry!(wide_put_long_long),
    export_entry!(wide_put_unsigned_long),
    export_entry!(wide_put_long),
    export_entry!(wide_put_bool),
]);

unsafe fn write_string(result: *mut StdString, text: &[u8]) -> *mut StdString {
    let mut string = StdString {
        text: [0; 16],
        size: text.len().min(SHORT_BYTES),
        capacity: SHORT_BYTES,
    };
    string.text[..string.size].copy_from_slice(&text[..string.size]);
    // SAFETY: the caller's room for the returned `std::string`.
    unsafe { result.write(string) };
    result
}

unsafe fn write_wide_string(result: *mut StdString, text: &[u8]) -> *mut StdString {
    let size = text.len().min(SHORT_UNITS);
    let mut string = StdString {
        text: [0; 16],
        size,
        capacity: SHORT_UNITS,
    };
    for (index, &byte) in text.iter().take(size).enumerate() {
        string.text[index * 2..index * 2 + 2].copy_from_slice(&u16::from(byte).to_le_bytes());
    }
    // SAFETY: the caller's room for the returned `std::wstring`.
    unsafe { result.write(string) };
    result
}

extern "efiapi" fn decimal_point(_this: *mut u8) -> u8 {
    b'.'
}

extern "efiapi" fn thousands_separator(_this: *mut u8) -> u8 {
    b','
}

extern "efiapi" fn wide_decimal_point(_this: *mut u8) -> u16 {
    u16::from(b'.')
}

extern "efiapi" fn wide_thousands_separator(_this: *mut u8) -> u16 {
    u16::from(b',')
}

unsafe extern "efiapi" fn grouping(_this: *mut u8, result: *mut StdString) -> *mut StdString {
    // SAFETY: the caller's room for the result.
    unsafe { write_string(result, b"") }
}

unsafe extern "efiapi" fn false_name(_this: *mut u8, result: *mut StdString) -> *mut StdString {
    // SAFETY: as above.
    unsafe { write_string(result, b"false") }
}

unsafe extern "efiapi" fn true_name(_this: *mut u8, result: *mut StdString) -> *mut StdString {
    // SAFETY: as above.
    unsafe { write_string(result, b"true") }
}

unsafe extern "efiapi" fn wide_false_name(
    _this: *mut u8,
    result: *mut StdString,
) -> *mut StdString {
    // SAFETY: as above.
    unsafe { write_wide_string(result, b"false") }
}

unsafe extern "efiapi" fn wide_true_name(
    _this: *mut u8,
    result: *mut StdString,
) -> *mut StdString {
    // SAFETY: as above.
    unsafe { write_wide_string(result, b"true") }
}

extern "efiapi" fn always_no_conversion(_this: *mut u8) -> u8 {
    0
}

extern "efiapi" fn longest(_this: *mut u8) -> i32 {
    LONGEST_UTF8
}

extern "efiapi" fn encoding(_this: *mut u8) -> i32 {
    0
}

fn sequence_length(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 0,
    }
}

#[expect(clippy::too_many_arguments, reason = "do_in's own parameters")]
unsafe extern "efiapi" fn convert_in(
    _this: *mut u8,
    _state: *mut u8,
    from: *const u8,
    from_end: *const u8,
    from_next: *mut *const u8,
    to: *mut u16,
    to_end: *mut u16,
    to_next: *mut *mut u16,
) -> i32 {
    let (mut at, mut out) = (from, to);
    let mut result = OK;
    // SAFETY: the program's ranges and its two cursors.
    unsafe {
        while at < from_end {
            let length = sequence_length(at.read());
            if length == 0 {
                result = ERROR;
                break;
            }
            if (from_end.offset_from(at) as usize) < length {
                result = PARTIAL;
                break;
            }
            let bytes = core::slice::from_raw_parts(at, length);
            let Some(character) = core::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.chars().next())
            else {
                result = ERROR;
                break;
            };
            let mut units = [0u16; 2];
            let encoded = character.encode_utf16(&mut units);
            if (to_end.offset_from(out) as usize) < encoded.len() {
                result = PARTIAL;
                break;
            }
            for &unit in encoded.iter() {
                out.write(unit);
                out = out.add(1);
            }
            at = at.add(length);
        }
        from_next.write(at);
        to_next.write(out);
    }
    result
}

#[expect(clippy::too_many_arguments, reason = "do_out's own parameters")]
unsafe extern "efiapi" fn convert_out(
    _this: *mut u8,
    _state: *mut u8,
    from: *const u16,
    from_end: *const u16,
    from_next: *mut *const u16,
    to: *mut u8,
    to_end: *mut u8,
    to_next: *mut *mut u8,
) -> i32 {
    let (mut at, mut out) = (from, to);
    let mut result = OK;
    // SAFETY: the program's ranges and its two cursors.
    unsafe {
        while at < from_end {
            let first = at.read();
            let (code, taken) = match first {
                0xd800..=0xdbff if at.add(1) < from_end => {
                    let second = at.add(1).read();
                    let joined = 0x1_0000
                        + ((u32::from(first) - 0xd800) << 10)
                        + (u32::from(second).wrapping_sub(0xdc00) & 0x3ff);
                    (joined, 2)
                }
                0xd800..=0xdbff => {
                    result = PARTIAL;
                    break;
                }
                other => (u32::from(other), 1),
            };
            let character = char::from_u32(code).unwrap_or('\u{fffd}');
            let mut bytes = [0u8; 4];
            let encoded = character.encode_utf8(&mut bytes).as_bytes();
            if (to_end.offset_from(out) as usize) < encoded.len() {
                result = PARTIAL;
                break;
            }
            core::ptr::copy_nonoverlapping(encoded.as_ptr(), out, encoded.len());
            out = out.add(encoded.len());
            at = at.add(taken);
        }
        from_next.write(at);
        to_next.write(out);
    }
    result
}

unsafe extern "efiapi" fn unshift(
    _this: *mut u8,
    _state: *mut u8,
    to: *mut u8,
    _to_end: *mut u8,
    to_next: *mut *mut u8,
) -> i32 {
    // SAFETY: the program's cursor.
    unsafe { to_next.write(to) };
    NO_CONVERSION
}

unsafe extern "efiapi" fn length(
    _this: *mut u8,
    _state: *mut u8,
    first: *const u8,
    last: *const u8,
    most: usize,
) -> i32 {
    let mut at = first;
    let mut units = 0usize;
    // SAFETY: the program's range.
    unsafe {
        while at < last && units < most {
            let length = sequence_length(at.read()).max(1);
            if (last.offset_from(at) as usize) < length {
                break;
            }
            units += if length == 4 { 2 } else { 1 };
            at = at.add(length);
        }
        at.offset_from(first) as i32
    }
}

unsafe fn put_text(
    result: *mut Iterator,
    destination: *const Iterator,
    units: Result<&[u8], &[u16]>,
) -> *mut Iterator {
    // SAFETY: the caller's iterators.
    unsafe {
        let mut iterator = destination.read();
        if iterator.failed == 0 && !iterator.buffer.is_null() {
            let written = match units {
                Ok(bytes) => put_bytes(iterator.buffer, bytes) == bytes.len() as i64,
                Err(units) => super::buffer_call(
                    iterator.buffer,
                    super::SLOT_XSPUTN,
                    [units.as_ptr() as usize, units.len(), 0],
                ) as i64
                    == units.len() as i64,
            };
            if !written {
                iterator.failed = 1;
            }
        }
        result.write(iterator);
    }
    result
}

unsafe fn padded(ios: *mut IosBase, body: &[u8], prefix: usize, fill: u8) -> Vec<u8> {
    // SAFETY: the caller's `ios_base`.
    unsafe {
        let padded = stream_format::pad(body, prefix, (*ios).format, (*ios).width, fill);
        (*ios).width = 0;
        padded
    }
}

unsafe fn put_field(
    result: *mut Iterator,
    destination: *const Iterator,
    ios: *mut IosBase,
    (body, prefix): (&[u8], usize),
    fill: Result<u8, u16>,
) -> *mut Iterator {
    // SAFETY: the caller's iterators and `ios_base`.
    unsafe {
        match fill {
            Ok(fill) => {
                let text = padded(ios, body, prefix, fill);
                put_text(result, destination, Ok(&text))
            }
            Err(fill) => {
                let text = padded(ios, body, prefix, 0);
                let units: Vec<u16> = text
                    .iter()
                    .map(|&byte| if byte == 0 { fill } else { u16::from(byte) })
                    .collect();
                put_text(result, destination, Err(&units))
            }
        }
    }
}

unsafe fn integer_text(ios: *mut IosBase, value: Integer) -> (Vec<u8>, usize) {
    // SAFETY: the caller's `ios_base`.
    stream_format::integer(value, unsafe { (*ios).format })
}

unsafe fn float_text(ios: *mut IosBase, value: f64, long_double: bool) -> (Vec<u8>, usize) {
    // SAFETY: the caller's `ios_base`.
    let (format, precision) = unsafe { ((*ios).format, (*ios).precision) };
    let spec = stream_format::float_specification(format, long_double);
    let mut arguments = FloatArguments { precision, value };
    let mut out = Collected(Vec::new());
    let dialect = eggos_c_format::Dialect::Windows {
        wide_strings: false,
    };
    let spec = &spec[..spec.len() - 1];
    // SAFETY: the specification takes at most the precision and the value.
    unsafe { eggos_c_format::format_in(dialect, spec, &mut arguments, &mut out) };
    let prefix = stream_format::float_prefix(&out.0);
    (out.0, prefix)
}

fn pointer_text(value: usize) -> (Vec<u8>, usize) {
    let text = (0..16)
        .rev()
        .map(|shift| b"0123456789ABCDEF"[(value >> (shift * 4)) & 0xf])
        .collect();
    (text, 0)
}

unsafe fn bool_text(ios: *mut IosBase, value: u8) -> (Vec<u8>, usize) {
    // SAFETY: the caller's `ios_base`.
    let format = unsafe { (*ios).format };
    match format & crate::stream_format::flags::BOOLALPHA {
        0 => {
            let value = Integer {
                bits: u64::from(value != 0),
                signed: true,
                width: 32,
            };
            stream_format::integer(value, format)
        }
        _ => (
            match value {
                0 => b"false".to_vec(),
                _ => b"true".to_vec(),
            },
            0,
        ),
    }
}

macro_rules! num_put {
    ($narrow:ident, $wide:ident, $type:ty, |$ios:ident, $value:ident| $text:expr) => {
        unsafe extern "efiapi" fn $narrow(
            _this: *mut u8,
            result: *mut Iterator,
            destination: *const Iterator,
            $ios: *mut IosBase,
            fill: u8,
            $value: $type,
        ) -> *mut Iterator {
            // SAFETY: `do_put`'s iterators, `ios_base` and value.
            unsafe {
                let (body, prefix) = $text;
                put_field(result, destination, $ios, (&body, prefix), Ok(fill))
            }
        }

        unsafe extern "efiapi" fn $wide(
            _this: *mut u8,
            result: *mut Iterator,
            destination: *const Iterator,
            $ios: *mut IosBase,
            fill: u16,
            $value: $type,
        ) -> *mut Iterator {
            // SAFETY: as above.
            unsafe {
                let (body, prefix) = $text;
                put_field(result, destination, $ios, (&body, prefix), Err(fill))
            }
        }
    };
}

num_put!(put_bool, wide_put_bool, u8, |ios, value| bool_text(ios, value));
num_put!(put_long, wide_put_long, i32, |ios, value| integer_text(
    ios,
    Integer {
        bits: value as u64,
        signed: true,
        width: 32
    }
));
num_put!(put_unsigned_long, wide_put_unsigned_long, u32, |ios, value| integer_text(
    ios,
    Integer {
        bits: u64::from(value),
        signed: false,
        width: 32
    }
));
num_put!(put_long_long, wide_put_long_long, i64, |ios, value| integer_text(
    ios,
    Integer {
        bits: value as u64,
        signed: true,
        width: 64
    }
));
num_put!(put_unsigned_long_long, wide_put_unsigned_long_long, u64, |ios, value| integer_text(
    ios,
    Integer {
        bits: value,
        signed: false,
        width: 64
    }
));
num_put!(put_double, wide_put_double, f64, |ios, value| float_text(ios, value, false));
num_put!(put_long_double, wide_put_long_double, f64, |ios, value| float_text(ios, value, true));
num_put!(put_pointer, wide_put_pointer, usize, |ios, value| {
    let _ = ios;
    pointer_text(value)
});

macro_rules! category {
    ($name:ident, $facet:ident, $category:expr) => {
        unsafe extern "efiapi" fn $name(facet: *mut *const u8, _locale: *const Locale) -> usize {
            if !facet.is_null() {
                // SAFETY: the caller's slot for the facet.
                unsafe {
                    if (*facet).is_null() {
                        facet.write($facet);
                    }
                }
            }
            $category
        }
    };
}

category!(numpunct_category, NUMPUNCT_FACET, CATEGORY_NUMERIC);
category!(wide_numpunct_category, WIDE_NUMPUNCT_FACET, CATEGORY_NUMERIC);
category!(codecvt_category, CODECVT_FACET, CATEGORY_CTYPE);
category!(num_put_category, NUM_PUT_FACET, CATEGORY_NUMERIC);
category!(wide_num_put_category, WIDE_NUM_PUT_FACET, CATEGORY_NUMERIC);

/// The facets' ids.
pub(in crate::win32) const DATA_EXPORTS: &[Export] = &[
    Export {
        name: b"?id@?$numpunct@D@std@@2V0locale@2@A",
        address: (&raw const NUMPUNCT_ID).cast(),
    },
    Export {
        name: b"?id@?$numpunct@_W@std@@2V0locale@2@A",
        address: (&raw const WIDE_NUMPUNCT_ID).cast(),
    },
    Export {
        name: b"?id@?$codecvt@_WDU_Mbstatet@@@std@@2V0locale@2@A",
        address: (&raw const CODECVT_ID).cast(),
    },
    Export {
        name: b"?id@?$num_put@DV?$ostreambuf_iterator@DU?$char_traits@D@std@@@std@@@std@@2V0locale@2@A",
        address: (&raw const NUM_PUT_ID).cast(),
    },
    Export {
        name: b"?id@?$num_put@_WV?$ostreambuf_iterator@_WU?$char_traits@_W@std@@@std@@@std@@2V0locale@2@A",
        address: (&raw const WIDE_NUM_PUT_ID).cast(),
    },
];

/// The facets' `_Getcat`, which `use_facet` calls.
pub(in crate::win32) const EXPORTS: &[Export] = exports![
    b"?_Getcat@?$numpunct@D@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => numpunct_category,
    b"?_Getcat@?$numpunct@_W@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => wide_numpunct_category,
    b"?_Getcat@?$codecvt@_WDU_Mbstatet@@@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => codecvt_category,
    b"?_Getcat@?$num_put@DV?$ostreambuf_iterator@DU?$char_traits@D@std@@@std@@@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => num_put_category,
    b"?_Getcat@?$num_put@_WV?$ostreambuf_iterator@_WU?$char_traits@_W@std@@@std@@@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => wide_num_put_category,
];
