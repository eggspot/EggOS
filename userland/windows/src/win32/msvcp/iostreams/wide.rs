//! msvcp140's iostreams for `wchar_t` (D748): `std::wcin`, `wcout`, `wcerr` and `wclog`, whose
//! buffers write the C runtime's standard streams in UTF-8 (the layer's narrow text, D178)
//! and read UTF-8 back as UTF-16 units; the out-of-line members of `basic_ios`,
//! `basic_streambuf`, `basic_ostream` and `basic_istream` for `wchar_t`, which share the
//! `char` streams' rules (numbers formatted and read as in the classic locale, end of file
//! `WEOF`, the fill a `wchar_t`) and their `ios_base`; the manipulators; and `ctype<wchar_t>`,
//! classifying the ASCII range as the classic locale does and other units as letters.

use super::{
    BROKEN, BasicIos, CLASS_ALPHA, CLASS_SPACE, CLASSES, CLASSIC, Collected, END,
    Export, FAILED, FloatArguments, GOOD, ISTREAM_BASES, ISTREAM_IOS_TABLE, IosBase,
    LONGEST_NUMBER, Locale, LocaleId, NULL_CELL, OSTREAM_BASES, OSTREAM_IOS_TABLE, Offsets,
    SLOT_OVERFLOW, SLOT_UFLOW, SLOT_UNDERFLOW, SLOT_XSGETN, SLOT_XSPUTN, STANDARD_FORMAT,
    STANDARD_PRECISION, Shared, Streambuf, Table, ZERO_COUNT, accepts, available, buffer_call,
    crt, facet_add_reference, facet_destroy, facet_release, flush_stream, ios_clear, ios_of,
    keep_buffer, new_locale, no_seek_offset, no_seek_position, nothing_on_buffer,
    nothing_on_locale, output_done, output_ready, self_cells, set_state, stream_format,
    zero_available,
};
use super::super::super::call_program;
use super::super::super::exception::throwing_entry;
use crate::stream_format::{Integer, Read, flags};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr::null_mut;

const WIDE_EOF: u32 = 0xffff;
const SLOT_PBACKFAIL: usize = 4;
const SLOT_SHOWMANYC: usize = 5;
const SLOT_SYNC: usize = 13;
const UNIT_BYTES: usize = 2;
const NO_UNIT: u32 = u32::MAX;
const REPLACEMENT: u16 = 0xfffd;
const CATEGORY_CTYPE: usize = 2;

#[repr(C)]
struct WideIos {
    base: IosBase,
    buffer: *mut Streambuf,
    tie: *mut u8,
    fill: u16,
    reserved: [u8; 6],
}

#[repr(C)]
struct WideOstream {
    bases: *const Offsets,
    reserved: u32,
    vtordisp: u32,
    ios: WideIos,
}

#[repr(C)]
struct WideIstream {
    bases: *const Offsets,
    count: i64,
    reserved: u32,
    vtordisp: u32,
    ios: WideIos,
}

#[repr(C)]
struct WideStandardBuffer {
    base: Streambuf,
    stream: usize,
    back: u32,
    pending: u32,
}

#[repr(C)]
struct WideCtype {
    table: *const *const (),
    references: u32,
    reserved: [u32; 13],
}

macro_rules! wide_pointer {
    ($static:ident) => {
        UnsafeCell::raw_get(&raw const $static.0)
    };
}

static WIDE_CTYPE: Shared<WideCtype> = Shared::new(WideCtype {
    table: (&raw const CTYPE_TABLE.0).cast(),
    references: 1,
    reserved: [0; 13],
});

/// `ctype<wchar_t>`, the classic locale's facet 2.
pub(super) const CTYPE_FACET: *const u8 = wide_pointer!(WIDE_CTYPE).cast_const().cast();

static CTYPE_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 2 });

macro_rules! wide_buffer {
    ($stream:expr) => {
        Shared::new(WideStandardBuffer {
            base: Streambuf {
                table: (&raw const STANDARD_TABLE.0).cast(),
                get_first: null_mut(),
                put_first: null_mut(),
                get_first_at: wide_pointer!(NULL_CELL),
                put_first_at: wide_pointer!(NULL_CELL),
                get_next: null_mut(),
                put_next: null_mut(),
                get_next_at: wide_pointer!(NULL_CELL),
                put_next_at: wide_pointer!(NULL_CELL),
                get_count: 0,
                put_count: 0,
                get_count_at: wide_pointer!(ZERO_COUNT),
                put_count_at: wide_pointer!(ZERO_COUNT),
                locale: wide_pointer!(CLASSIC),
            },
            stream: $stream,
            back: NO_UNIT,
            pending: 0,
        })
    };
}

static IN_BUFFER: Shared<WideStandardBuffer> = wide_buffer!(0);
static OUT_BUFFER: Shared<WideStandardBuffer> = wide_buffer!(1);
static ERROR_BUFFER: Shared<WideStandardBuffer> = wide_buffer!(2);

macro_rules! wide_ios {
    ($table:ident, $format:expr, $buffer:ident, $tie:expr) => {
        WideIos {
            base: IosBase {
                table: (&raw const $table.0).cast(),
                standard: 1,
                state: GOOD,
                exceptions: GOOD,
                format: $format,
                reserved: 0,
                precision: STANDARD_PRECISION,
                width: 0,
                words: 0,
                callbacks: 0,
                locale: wide_pointer!(CLASSIC),
            },
            buffer: wide_pointer!($buffer).cast(),
            tie: $tie,
            fill: 0x20,
            reserved: [0; 6],
        }
    };
}

static WCOUT: Shared<WideOstream> = Shared::new(WideOstream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: wide_ios!(OSTREAM_IOS_TABLE, STANDARD_FORMAT, OUT_BUFFER, null_mut()),
});

static WCERR: Shared<WideOstream> = Shared::new(WideOstream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: wide_ios!(
        OSTREAM_IOS_TABLE,
        STANDARD_FORMAT | flags::UNITBUF,
        ERROR_BUFFER,
        wide_pointer!(WCOUT).cast()
    ),
});

static WCLOG: Shared<WideOstream> = Shared::new(WideOstream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: wide_ios!(OSTREAM_IOS_TABLE, STANDARD_FORMAT, ERROR_BUFFER, null_mut()),
});

static WCIN: Shared<WideIstream> = Shared::new(WideIstream {
    bases: &raw const ISTREAM_BASES,
    count: 0,
    reserved: 0,
    vtordisp: 0,
    ios: wide_ios!(
        super::ISTREAM_IOS_TABLE,
        STANDARD_FORMAT,
        IN_BUFFER,
        wide_pointer!(WCOUT).cast()
    ),
});

static STANDARD_TABLE: Table<15> = Table([
    export_entry!(standard_destroy),
    export_entry!(nothing_on_buffer),
    export_entry!(nothing_on_buffer),
    export_entry!(standard_overflow),
    export_entry!(standard_pbackfail),
    export_entry!(zero_available),
    export_entry!(standard_underflow),
    export_entry!(standard_uflow),
    export_entry!(standard_xsgetn),
    export_entry!(standard_xsputn),
    export_entry!(no_seek_offset),
    export_entry!(no_seek_position),
    export_entry!(keep_buffer),
    export_entry!(standard_sync),
    export_entry!(nothing_on_locale),
]);

static BASE_TABLE: Table<15> = Table([
    export_entry!(base_destroy),
    export_entry!(nothing_on_buffer),
    export_entry!(nothing_on_buffer),
    export_entry!(base_overflow),
    export_entry!(base_overflow),
    export_entry!(zero_available),
    export_entry!(base_underflow),
    export_entry!(base_uflow),
    export_entry!(base_xsgetn),
    export_entry!(base_xsputn),
    export_entry!(no_seek_offset),
    export_entry!(no_seek_position),
    export_entry!(keep_buffer),
    export_entry!(base_sync),
    export_entry!(nothing_on_locale),
]);

static CTYPE_TABLE: Table<15> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(ctype_is_range),
    export_entry!(ctype_is),
    export_entry!(ctype_scan_is),
    export_entry!(ctype_scan_not),
    export_entry!(ctype_lower_range),
    export_entry!(ctype_lower),
    export_entry!(ctype_upper_range),
    export_entry!(ctype_upper),
    export_entry!(ctype_widen_range),
    export_entry!(ctype_widen),
    export_entry!(ctype_narrow_range),
    export_entry!(ctype_narrow),
]);

fn wide_ios_of(stream: *mut u8) -> *mut WideIos {
    // SAFETY: the callers pass a live wide stream, whose `basic_ios` has the wide layout.
    unsafe { ios_of(stream) }.cast()
}

fn as_ios(ios: *mut WideIos) -> *mut BasicIos {
    ios.cast()
}

fn standard_file(buffer: *mut WideStandardBuffer) -> *mut crt::File {
    // SAFETY: one of the layer's wide standard buffers.
    crt::standard_file(unsafe { (*buffer).stream })
}

fn encode(unit: u32, out: &mut Vec<u8>) {
    let mut bytes = [0u8; 4];
    let character = char::from_u32(unit).unwrap_or('\u{fffd}');
    out.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
}

unsafe extern "efiapi" fn standard_destroy(this: *mut u8, _flags: u32) -> *mut u8 {
    this
}

extern "efiapi" fn standard_overflow(this: *mut WideStandardBuffer, unit: u32) -> u32 {
    let unit = unit & 0xffff;
    if unit == WIDE_EOF {
        return 0;
    }
    let mut bytes = Vec::new();
    // SAFETY: one of the layer's buffers, written by the program's one stream at a time.
    let pending = unsafe { &mut (*this).pending };
    match unit {
        0xd800..=0xdbff => {
            *pending = unit;
            return unit;
        }
        0xdc00..=0xdfff if *pending != 0 => {
            let joined = 0x1_0000 + ((*pending - 0xd800) << 10) + (unit - 0xdc00);
            *pending = 0;
            encode(joined, &mut bytes);
        }
        other => encode(other, &mut bytes),
    }
    match crt::stream_write(standard_file(this), &bytes) == bytes.len() {
        true => unit,
        false => WIDE_EOF,
    }
}

fn read_unit(this: *mut WideStandardBuffer, take: bool) -> u32 {
    // SAFETY: one of the layer's buffers.
    let back = unsafe { &mut (*this).back };
    if *back != NO_UNIT {
        let unit = *back;
        if take {
            *back = NO_UNIT;
        }
        return unit;
    }
    let file = standard_file(this);
    let Some(first) = crt::stream_read(file, true) else {
        return WIDE_EOF;
    };
    let length = match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    };
    let mut bytes = [first, 0, 0, 0];
    for slot in bytes.iter_mut().take(length).skip(1) {
        *slot = crt::stream_read(file, true).unwrap_or(0);
    }
    let unit = core::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(|text| text.chars().next())
        .map_or(u32::from(REPLACEMENT), |character| match u32::from(character) {
            code if code > 0xffff => u32::from(REPLACEMENT),
            code => code,
        });
    if !take {
        *back = unit;
    }
    unit
}

extern "efiapi" fn standard_pbackfail(this: *mut WideStandardBuffer, unit: u32) -> u32 {
    let unit = unit & 0xffff;
    if unit == WIDE_EOF {
        return WIDE_EOF;
    }
    // SAFETY: one of the layer's buffers.
    unsafe { (*this).back = unit };
    unit
}

extern "efiapi" fn standard_underflow(this: *mut WideStandardBuffer) -> u32 {
    read_unit(this, false)
}

extern "efiapi" fn standard_uflow(this: *mut WideStandardBuffer) -> u32 {
    read_unit(this, true)
}

unsafe extern "efiapi" fn standard_xsgetn(
    this: *mut WideStandardBuffer,
    out: *mut u16,
    count: i64,
) -> i64 {
    let mut read = 0;
    while read < count {
        let unit = read_unit(this, true);
        if unit == WIDE_EOF {
            break;
        }
        // SAFETY: the caller's room for `count` units.
        unsafe { out.add(read as usize).write(unit as u16) };
        read += 1;
    }
    read
}

unsafe extern "efiapi" fn standard_xsputn(
    this: *mut WideStandardBuffer,
    units: *const u16,
    count: i64,
) -> i64 {
    let mut written = 0;
    while written < count {
        // SAFETY: the caller's `count` units.
        let unit = unsafe { units.add(written as usize).read() };
        if standard_overflow(this, u32::from(unit)) == WIDE_EOF {
            break;
        }
        written += 1;
    }
    written
}

extern "efiapi" fn standard_sync(this: *mut WideStandardBuffer) -> i32 {
    match crt::stream_flush(standard_file(this)) {
        true => 0,
        false => -1,
    }
}

unsafe fn put_unit(buffer: *mut Streambuf, unit: u16) -> u32 {
    // SAFETY: the caller's duty: a live wide buffer.
    unsafe {
        let held = &*buffer;
        if available(held.put_next_at, held.put_count_at) > 0 {
            let next = *held.put_next_at;
            next.cast::<u16>().write_unaligned(unit);
            *held.put_next_at = next.add(UNIT_BYTES);
            *held.put_count_at -= 1;
            return u32::from(unit);
        }
        buffer_call(buffer, SLOT_OVERFLOW, [usize::from(unit), 0, 0]) as u32 & 0xffff
    }
}

unsafe fn put_units(buffer: *mut Streambuf, units: &[u16]) -> i64 {
    // SAFETY: the caller's duty.
    unsafe {
        buffer_call(
            buffer,
            SLOT_XSPUTN,
            [units.as_ptr() as usize, units.len(), 0],
        ) as i64
    }
}

unsafe fn get_unit(buffer: *mut Streambuf, take: bool) -> u32 {
    // SAFETY: the caller's duty: a live wide buffer.
    unsafe {
        let held = &*buffer;
        if available(held.get_next_at, held.get_count_at) > 0 {
            let next = *held.get_next_at;
            let unit = next.cast::<u16>().read_unaligned();
            if take {
                *held.get_next_at = next.add(UNIT_BYTES);
                *held.get_count_at -= 1;
            }
            return u32::from(unit);
        }
        let slot = if take { SLOT_UFLOW } else { SLOT_UNDERFLOW };
        buffer_call(buffer, slot, [0; 3]) as u32 & 0xffff
    }
}

fn widened(bytes: &[u8], fill: Option<u16>) -> Vec<u16> {
    bytes
        .iter()
        .map(|&byte| match (byte, fill) {
            (0, Some(fill)) => fill,
            (byte, _) => u16::from(byte),
        })
        .collect()
}

unsafe fn write_field(stream: *mut u8, body: &[u8], prefix: usize) -> *mut u8 {
    // SAFETY: the caller's duty: a live wide output stream.
    unsafe {
        if !output_ready(stream) {
            return stream;
        }
        let ios = wide_ios_of(stream);
        let padded =
            stream_format::pad(body, prefix, (*ios).base.format, (*ios).base.width, 0);
        (*ios).base.width = 0;
        let units = widened(&padded, Some((*ios).fill));
        if put_units((*ios).buffer, &units) != units.len() as i64 {
            set_state(as_ios(ios), BROKEN);
        }
        output_done(stream);
    }
    stream
}

unsafe fn write_integer(stream: *mut u8, value: Integer) -> *mut u8 {
    // SAFETY: the caller's duty.
    let format = unsafe { (*wide_ios_of(stream)).base.format };
    let (body, prefix) = stream_format::integer(value, format);
    // SAFETY: as above.
    unsafe { write_field(stream, &body, prefix) }
}

unsafe fn write_float(stream: *mut u8, value: f64, long_double: bool) -> *mut u8 {
    // SAFETY: the caller's duty.
    let (format, precision) = unsafe {
        let ios = wide_ios_of(stream);
        ((*ios).base.format, (*ios).base.precision)
    };
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
    // SAFETY: as above.
    unsafe { write_field(stream, &out.0, prefix) }
}

macro_rules! output_integer {
    ($name:ident, $type:ty, $signed:expr, $bits:expr) => {
        unsafe extern "efiapi" fn $name(this: *mut u8, value: $type) -> *mut u8 {
            let value = Integer {
                bits: value as u64,
                signed: $signed,
                width: $bits,
            };
            // SAFETY: the member's `this`, a live wide output stream.
            unsafe { write_integer(this, value) }
        }
    };
}

output_integer!(put_short, i16, true, 16);
output_integer!(put_unsigned_short, u16, false, 16);
output_integer!(put_int, i32, true, 32);
output_integer!(put_unsigned_int, u32, false, 32);
output_integer!(put_long_long, i64, true, 64);
output_integer!(put_unsigned_long_long, u64, false, 64);

unsafe extern "efiapi" fn put_bool(this: *mut u8, value: u8) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide output stream.
    let format = unsafe { (*wide_ios_of(this)).base.format };
    if format & flags::BOOLALPHA == 0 {
        // SAFETY: as above.
        return unsafe { put_int(this, i32::from(value != 0)) };
    }
    let words: &[u8] = if value != 0 { b"true" } else { b"false" };
    // SAFETY: as above.
    unsafe { write_field(this, words, 0) }
}

unsafe extern "efiapi" fn put_double(this: *mut u8, value: f64) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide output stream.
    unsafe { write_float(this, value, false) }
}

unsafe extern "efiapi" fn put_long_double(this: *mut u8, value: f64) -> *mut u8 {
    // SAFETY: as above.
    unsafe { write_float(this, value, true) }
}

unsafe extern "efiapi" fn put_float(this: *mut u8, value: f32) -> *mut u8 {
    // SAFETY: as above.
    unsafe { write_float(this, f64::from(value), false) }
}

unsafe extern "efiapi" fn put_pointer(this: *mut u8, value: usize) -> *mut u8 {
    let mut text = Vec::with_capacity(16);
    for shift in (0..16).rev() {
        text.push(b"0123456789ABCDEF"[(value >> (shift * 4)) & 0xf]);
    }
    // SAFETY: the member's `this`, a live wide output stream.
    unsafe { write_field(this, &text, 0) }
}

unsafe extern "efiapi" fn put_buffer(this: *mut u8, source: *mut Streambuf) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide output stream; `source` null or a live buffer.
    unsafe {
        let ios = wide_ios_of(this);
        if source.is_null() {
            set_state(as_ios(ios), BROKEN);
            return this;
        }
        if !output_ready(this) {
            return this;
        }
        let mut copied = 0;
        loop {
            let unit = get_unit(source, true);
            if unit == WIDE_EOF {
                break;
            }
            if put_unit((*ios).buffer, unit as u16) == WIDE_EOF {
                set_state(as_ios(ios), BROKEN);
                break;
            }
            copied += 1;
        }
        (*ios).base.width = 0;
        if copied == 0 {
            set_state(as_ios(ios), FAILED);
        }
        output_done(this);
    }
    this
}

unsafe extern "efiapi" fn put(this: *mut u8, unit: u16) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide output stream.
    unsafe {
        if output_ready(this) {
            let ios = wide_ios_of(this);
            if put_unit((*ios).buffer, unit) == WIDE_EOF {
                set_state(as_ios(ios), BROKEN);
            }
            output_done(this);
        }
    }
    this
}

unsafe extern "efiapi" fn write(this: *mut u8, units: *const u16, count: i64) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide output stream, and the caller's units.
    unsafe {
        if count > 0 && output_ready(this) {
            let ios = wide_ios_of(this);
            let units = core::slice::from_raw_parts(units, count as usize);
            if put_units((*ios).buffer, units) != count {
                set_state(as_ios(ios), BROKEN);
            }
            output_done(this);
        }
    }
    this
}

unsafe extern "efiapi" fn end_line(stream: *mut u8) -> *mut u8 {
    // SAFETY: the manipulator's stream, a live wide output stream.
    unsafe {
        put(stream, 0x0a);
        flush_stream(stream);
    }
    stream
}

unsafe extern "efiapi" fn end_string(stream: *mut u8) -> *mut u8 {
    // SAFETY: as above.
    unsafe { put(stream, 0) }
}

unsafe fn input_ready(stream: *mut u8, no_skip: bool) -> bool {
    // SAFETY: the caller's duty: a live wide input stream.
    unsafe {
        let ios = wide_ios_of(stream);
        if (*ios).base.state != GOOD {
            set_state(as_ios(ios), FAILED);
            return false;
        }
        let tie = (*ios).tie;
        if !tie.is_null() {
            flush_stream(tie);
        }
        let buffer = (*ios).buffer;
        if no_skip || (*ios).base.format & flags::SKIPWS == 0 {
            return true;
        }
        loop {
            let next = get_unit(buffer, false);
            if next == WIDE_EOF {
                set_state(as_ios(ios), END | FAILED);
                return false;
            }
            if !is_space(next) {
                return true;
            }
            get_unit(buffer, true);
        }
    }
}

fn is_space(unit: u32) -> bool {
    match unit {
        0..=0x7f => CLASSES[unit as usize] & CLASS_SPACE != 0,
        0xa0 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x3000 => true,
        _ => false,
    }
}

unsafe extern "efiapi" fn input_prefix(this: *mut u8, no_skip: u8) -> u8 {
    // SAFETY: the member's `this`, a live wide input stream.
    u8::from(unsafe { input_ready(this, no_skip != 0) })
}

unsafe fn take_number(stream: *mut u8, float: bool) -> Option<Vec<u8>> {
    // SAFETY: the caller's duty: a live wide input stream.
    unsafe {
        if !input_ready(stream, false) {
            return None;
        }
        let ios = wide_ios_of(stream);
        let base = match (*ios).base.format & flags::BASEFIELD {
            flags::OCT => 8,
            flags::HEX => 16,
            flags::DEC => 10,
            _ => 0,
        };
        let buffer = (*ios).buffer;
        let mut taken = Vec::new();
        while taken.len() < LONGEST_NUMBER {
            let next = get_unit(buffer, false);
            if next == WIDE_EOF {
                set_state(as_ios(ios), END);
                break;
            }
            if next > 0x7f || !accepts(next as u8, &taken, base, float) {
                break;
            }
            taken.push(next as u8);
            get_unit(buffer, true);
        }
        Some(taken)
    }
}

macro_rules! input_integer {
    ($name:ident, $type:ty, $signed:expr, $bits:expr) => {
        unsafe extern "efiapi" fn $name(this: *mut u8, out: *mut $type) -> *mut u8 {
            // SAFETY: the member's `this`, a live wide input stream, and the caller's variable.
            unsafe {
                let Some(taken) = take_number(this, false) else {
                    return this;
                };
                let ios = as_ios(wide_ios_of(this));
                let format = (*ios).base.format;
                match stream_format::read_integer(&taken, format, $signed, $bits) {
                    (Read::Value(bits), used) if used == taken.len() => out.write(bits as $type),
                    (Read::Overflow(edge), _) => {
                        out.write(edge as $type);
                        set_state(ios, FAILED);
                    }
                    _ => {
                        out.write(0 as $type);
                        set_state(ios, FAILED);
                    }
                }
            }
            this
        }
    };
}

input_integer!(get_short, i16, true, 16);
input_integer!(get_unsigned_short, u16, false, 16);
input_integer!(get_int, i32, true, 32);
input_integer!(get_unsigned_int, u32, false, 32);
input_integer!(get_long_long, i64, true, 64);
input_integer!(get_unsigned_long_long, u64, false, 64);

unsafe extern "efiapi" fn get_bool(this: *mut u8, out: *mut u8) -> *mut u8 {
    let mut value = 0i32;
    // SAFETY: the member's `this` and the caller's variable.
    unsafe {
        get_int(this, &mut value);
        match value {
            0 | 1 => out.write(value as u8),
            _ => {
                out.write(1);
                set_state(as_ios(wide_ios_of(this)), FAILED);
            }
        }
    }
    this
}

unsafe fn read_float(this: *mut u8) -> Option<f64> {
    // SAFETY: the caller's duty: a live wide input stream.
    unsafe {
        let taken = take_number(this, true)?;
        let (value, used) = eggos_c_format::number::parse_float(&taken);
        if used == 0 || used != taken.len() {
            set_state(as_ios(wide_ios_of(this)), FAILED);
            return Some(0.0);
        }
        Some(value)
    }
}

unsafe extern "efiapi" fn get_double(this: *mut u8, out: *mut f64) -> *mut u8 {
    // SAFETY: the member's `this` and the caller's variable.
    unsafe {
        if let Some(value) = read_float(this) {
            out.write(value);
        }
    }
    this
}

unsafe extern "efiapi" fn get_float(this: *mut u8, out: *mut f32) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        if let Some(value) = read_float(this) {
            out.write(value as f32);
        }
    }
    this
}

fn input_of(this: *mut u8) -> *mut WideIstream {
    this.cast()
}

unsafe extern "efiapi" fn get_one(this: *mut u8) -> u32 {
    // SAFETY: the member's `this`, a live wide input stream.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return WIDE_EOF;
        }
        let ios = wide_ios_of(this);
        let unit = get_unit((*ios).buffer, true);
        match unit {
            WIDE_EOF => set_state(as_ios(ios), END | FAILED),
            _ => (*input_of(this)).count = 1,
        }
        unit
    }
}

unsafe extern "efiapi" fn peek(this: *mut u8) -> u32 {
    // SAFETY: as above.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return WIDE_EOF;
        }
        let ios = wide_ios_of(this);
        let unit = get_unit((*ios).buffer, false);
        if unit == WIDE_EOF {
            set_state(as_ios(ios), END);
        }
        unit
    }
}

unsafe fn read_until(this: *mut u8, out: *mut u16, capacity: i64, delimiter: Option<u16>) -> i64 {
    // SAFETY: the caller's duty: a live wide input stream and room for `capacity` units.
    unsafe {
        (*input_of(this)).count = 0;
        let ios = wide_ios_of(this);
        if !input_ready(this, true) {
            if capacity > 0 && delimiter.is_some() {
                out.write(0);
            }
            return 0;
        }
        let room = match delimiter {
            Some(_) => capacity - 1,
            None => capacity,
        };
        let mut count = 0i64;
        let mut state = GOOD;
        loop {
            let next = get_unit((*ios).buffer, false);
            if next == WIDE_EOF {
                state |= END;
                break;
            }
            if delimiter.map(u32::from) == Some(next) {
                get_unit((*ios).buffer, true);
                (*input_of(this)).count += 1;
                break;
            }
            if count >= room {
                if delimiter.is_some() {
                    state |= FAILED;
                }
                break;
            }
            out.add(count as usize).write(next as u16);
            get_unit((*ios).buffer, true);
            count += 1;
        }
        (*input_of(this)).count += count;
        if delimiter.is_some() && capacity > 0 {
            out.add(count as usize).write(0);
        }
        let short_read = delimiter.is_none() && count < capacity;
        if (*input_of(this)).count == 0 || short_read {
            state |= FAILED;
        }
        if state != GOOD {
            set_state(as_ios(ios), state);
        }
        count
    }
}

unsafe extern "efiapi" fn get_line(this: *mut u8, out: *mut u16, capacity: i64) -> *mut u8 {
    // SAFETY: the member's `this` and the caller's room.
    unsafe { read_until(this, out, capacity, Some(0x0a)) };
    this
}

unsafe extern "efiapi" fn get_line_until(
    this: *mut u8,
    out: *mut u16,
    capacity: i64,
    delimiter: u16,
) -> *mut u8 {
    // SAFETY: as above.
    unsafe { read_until(this, out, capacity, Some(delimiter)) };
    this
}

unsafe extern "efiapi" fn read(this: *mut u8, out: *mut u16, count: i64) -> *mut u8 {
    // SAFETY: as above.
    unsafe { read_until(this, out, count, None) };
    this
}

unsafe extern "efiapi" fn ignore(this: *mut u8, count: i64, delimiter: u32) -> *mut u8 {
    // SAFETY: the member's `this`, a live wide input stream.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return this;
        }
        let ios = wide_ios_of(this);
        let mut skipped = 0i64;
        while count == i64::MAX || skipped < count {
            let unit = get_unit((*ios).buffer, true);
            if unit == WIDE_EOF {
                set_state(as_ios(ios), END);
                break;
            }
            skipped += 1;
            if unit == delimiter & 0xffff {
                break;
            }
        }
        (*input_of(this)).count = skipped;
    }
    this
}

unsafe extern "efiapi" fn put_back(this: *mut u8, unit: u16) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        (*input_of(this)).count = 0;
        let ios = wide_ios_of(this);
        (*ios).base.state &= !END;
        if !input_ready(this, true) {
            return this;
        }
        let back = buffer_call((*ios).buffer, SLOT_PBACKFAIL, [usize::from(unit), 0, 0]) as u32;
        if back & 0xffff == WIDE_EOF {
            set_state(as_ios(ios), BROKEN);
        }
    }
    this
}

unsafe extern "efiapi" fn ios_init(this: *mut WideIos, buffer: *mut Streambuf, standard: u8) {
    // SAFETY: the member's `this`, a wide `basic_ios` being made.
    unsafe {
        let base = &mut (*this).base;
        base.standard = usize::from(standard);
        base.state = if buffer.is_null() { BROKEN } else { GOOD };
        base.exceptions = GOOD;
        base.format = STANDARD_FORMAT;
        base.precision = STANDARD_PRECISION;
        base.width = 0;
        base.words = 0;
        base.callbacks = 0;
        base.locale = wide_pointer!(CLASSIC);
        (*this).buffer = buffer;
        (*this).tie = null_mut();
        (*this).fill = 0x20;
        (*this).reserved = [0; 6];
    }
}

unsafe extern "efiapi" fn ios_make(this: *mut WideIos) -> *mut WideIos {
    // SAFETY: the constructor's `this`, room for a wide `basic_ios`.
    unsafe {
        (*this).base.table = (&raw const super::IOS_TABLE.0).cast();
        ios_init(this, null_mut(), 0);
    }
    this
}

unsafe extern "efiapi" fn ios_fill(this: *mut WideIos) -> u16 {
    // SAFETY: the member's `this`.
    unsafe { (*this).fill }
}

unsafe extern "efiapi" fn ios_set_fill(this: *mut WideIos, fill: u16) -> u16 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).fill, fill) }
}

extern "efiapi" fn ios_widen(_this: *mut WideIos, byte: u8) -> u16 {
    u16::from(byte)
}

extern "efiapi" fn ios_narrow(_this: *mut WideIos, unit: u16, default: u8) -> u8 {
    u8::try_from(unit).ok().filter(u8::is_ascii).unwrap_or(default)
}

unsafe extern "efiapi" fn ostream_make(
    this: *mut WideOstream,
    buffer: *mut Streambuf,
    standard: u8,
    most_derived: i32,
) -> *mut WideOstream {
    // SAFETY: the constructor's `this`, room for a wide `basic_ostream`.
    unsafe {
        if most_derived != 0 {
            (*this).bases = &raw const OSTREAM_BASES;
            ios_make(&raw mut (*this).ios);
        }
        (*this).vtordisp = 0;
        let ios = wide_ios_of(this.cast());
        (*ios).base.table = (&raw const OSTREAM_IOS_TABLE.0).cast();
        ios_init(ios, buffer, standard);
    }
    this
}

unsafe extern "efiapi" fn istream_make(
    this: *mut WideIstream,
    buffer: *mut Streambuf,
    standard: u8,
    most_derived: i32,
) -> *mut WideIstream {
    // SAFETY: the constructor's `this`, room for a wide `basic_istream`.
    unsafe {
        if most_derived != 0 {
            (*this).bases = &raw const ISTREAM_BASES;
            ios_make(&raw mut (*this).ios);
        }
        (*this).vtordisp = 0;
        (*this).count = 0;
        let ios = wide_ios_of(this.cast());
        (*ios).base.table = (&raw const ISTREAM_IOS_TABLE.0).cast();
        ios_init(ios, buffer, standard);
    }
    this
}

unsafe extern "efiapi" fn buffer_make(this: *mut Streambuf) -> *mut Streambuf {
    // SAFETY: the constructor's `this`, room for a wide `basic_streambuf`.
    unsafe {
        (*this).table = (&raw const BASE_TABLE.0).cast();
        self_cells(this);
        (*this).locale = new_locale();
    }
    this
}

unsafe extern "efiapi" fn buffer_copy(
    this: *mut Streambuf,
    other: *const Streambuf,
) -> *mut Streambuf {
    // SAFETY: the constructor's `this` and the program's buffer it copies.
    unsafe {
        buffer_make(this);
        (*this).get_first = *(*other).get_first_at;
        (*this).get_next = *(*other).get_next_at;
        (*this).get_count = *(*other).get_count_at;
        (*this).put_first = *(*other).put_first_at;
        (*this).put_next = *(*other).put_next_at;
        (*this).put_count = *(*other).put_count_at;
    }
    this
}

unsafe extern "efiapi" fn base_destroy(this: *mut Streambuf, flags: u32) -> *mut Streambuf {
    // SAFETY: the scalar deleting destructor's `this`, a buffer the program made.
    unsafe {
        super::buffer_unmake(this);
        if flags & 1 != 0 {
            super::heap::free(this.cast());
        }
    }
    this
}

extern "efiapi" fn base_overflow(_this: *mut Streambuf, _unit: u32) -> u32 {
    WIDE_EOF
}

extern "efiapi" fn base_underflow(_this: *mut Streambuf) -> u32 {
    WIDE_EOF
}

unsafe extern "efiapi" fn base_uflow(this: *mut Streambuf) -> u32 {
    // SAFETY: the member's `this`, a live buffer whose `underflow` fills its get area.
    unsafe {
        if buffer_call(this, SLOT_UNDERFLOW, [0; 3]) as u32 & 0xffff == WIDE_EOF {
            return WIDE_EOF;
        }
        let held = &*this;
        if available(held.get_next_at, held.get_count_at) <= 0 {
            return WIDE_EOF;
        }
        get_unit(this, true)
    }
}

unsafe extern "efiapi" fn base_xsgetn(this: *mut Streambuf, out: *mut u16, count: i64) -> i64 {
    let mut read = 0;
    while read < count {
        // SAFETY: the member's `this`, a live buffer, and the caller's room.
        let unit = unsafe { get_unit(this, true) };
        if unit == WIDE_EOF {
            break;
        }
        // SAFETY: as above.
        unsafe { out.add(read as usize).write(unit as u16) };
        read += 1;
    }
    read
}

unsafe extern "efiapi" fn base_xsputn(this: *mut Streambuf, units: *const u16, count: i64) -> i64 {
    let mut written = 0;
    while written < count {
        // SAFETY: the member's `this`, a live buffer, and the caller's units.
        let unit = unsafe { units.add(written as usize).read() };
        // SAFETY: as above.
        if unsafe { put_unit(this, unit) } == WIDE_EOF {
            break;
        }
        written += 1;
    }
    written
}

extern "efiapi" fn base_sync(_this: *mut Streambuf) -> i32 {
    0
}

unsafe extern "efiapi" fn buffer_sputc(this: *mut Streambuf, unit: u16) -> u32 {
    // SAFETY: the member's `this`, a live wide buffer.
    unsafe { put_unit(this, unit) }
}

unsafe extern "efiapi" fn buffer_sputn(this: *mut Streambuf, units: *const u16, count: i64) -> i64 {
    if count <= 0 {
        return 0;
    }
    // SAFETY: as above, and the caller's `count` units.
    unsafe { put_units(this, core::slice::from_raw_parts(units, count as usize)) }
}

unsafe extern "efiapi" fn buffer_sgetc(this: *mut Streambuf) -> u32 {
    // SAFETY: the member's `this`, a live wide buffer.
    unsafe { get_unit(this, false) }
}

unsafe extern "efiapi" fn buffer_sbumpc(this: *mut Streambuf) -> u32 {
    // SAFETY: as above.
    unsafe { get_unit(this, true) }
}

unsafe extern "efiapi" fn buffer_snextc(this: *mut Streambuf) -> u32 {
    // SAFETY: as above.
    unsafe {
        match get_unit(this, true) {
            WIDE_EOF => WIDE_EOF,
            _ => get_unit(this, false),
        }
    }
}

unsafe extern "efiapi" fn buffer_sgetn(this: *mut Streambuf, out: *mut u16, count: i64) -> i64 {
    // SAFETY: as above, and the caller's room.
    unsafe { buffer_call(this, SLOT_XSGETN, [out as usize, count as usize, 0]) as i64 }
}

unsafe extern "efiapi" fn buffer_pubsync(this: *mut Streambuf) -> i32 {
    // SAFETY: as above.
    unsafe { buffer_call(this, SLOT_SYNC, [0; 3]) as u32 as i32 }
}

fn units_between(first: *mut u8, last: *mut u8) -> i32 {
    // SAFETY: the program's two places in one buffer.
    let bytes = unsafe { last.offset_from(first) };
    (bytes / UNIT_BYTES as isize).clamp(0, i32::MAX as isize) as i32
}

unsafe extern "efiapi" fn buffer_set_get(
    this: *mut Streambuf,
    first: *mut u8,
    next: *mut u8,
    last: *mut u8,
) {
    // SAFETY: the member's `this`, a live buffer with its own cells.
    unsafe {
        *(*this).get_first_at = first;
        *(*this).get_next_at = next;
        *(*this).get_count_at = units_between(next, last);
    }
}

unsafe extern "efiapi" fn buffer_set_put(this: *mut Streambuf, first: *mut u8, last: *mut u8) {
    // SAFETY: as above.
    unsafe {
        *(*this).put_first_at = first;
        *(*this).put_next_at = first;
        *(*this).put_count_at = units_between(first, last);
    }
}

unsafe extern "efiapi" fn buffer_get_end(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        let next = *(*this).get_next_at;
        match next.is_null() {
            true => next,
            false => next.add((*(*this).get_count_at).max(0) as usize * UNIT_BYTES),
        }
    }
}

unsafe extern "efiapi" fn buffer_put_end(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        let next = *(*this).put_next_at;
        match next.is_null() {
            true => next,
            false => next.add((*(*this).put_count_at).max(0) as usize * UNIT_BYTES),
        }
    }
}

unsafe extern "efiapi" fn buffer_get_bump(this: *mut Streambuf, count: i32) {
    // SAFETY: as above.
    unsafe {
        *(*this).get_next_at = (*(*this).get_next_at).offset(count as isize * 2);
        *(*this).get_count_at -= count;
    }
}

unsafe extern "efiapi" fn buffer_put_bump(this: *mut Streambuf, count: i32) {
    // SAFETY: as above.
    unsafe {
        *(*this).put_next_at = (*(*this).put_next_at).offset(count as isize * 2);
        *(*this).put_count_at -= count;
    }
}

unsafe extern "efiapi" fn buffer_in_available(this: *mut Streambuf) -> i64 {
    // SAFETY: as above.
    unsafe {
        let held = &*this;
        match available(held.get_next_at, held.get_count_at) {
            0 => buffer_call(this, SLOT_SHOWMANYC, [0; 3]) as i64,
            ready => i64::from(ready),
        }
    }
}

fn classes_of(unit: u32) -> i16 {
    match unit {
        0..=0xff => CLASSES[unit as usize],
        _ if is_space(unit) => CLASS_SPACE,
        _ => CLASS_ALPHA,
    }
}

extern "efiapi" fn ctype_is(_this: *mut u8, mask: i16, unit: u16) -> u8 {
    u8::from(classes_of(u32::from(unit)) & mask != 0)
}

unsafe extern "efiapi" fn ctype_is_range(
    _this: *mut u8,
    first: *const u16,
    last: *const u16,
    out: *mut i16,
) -> *const u16 {
    let mut at = first;
    let mut index = 0;
    while at < last {
        // SAFETY: the program's range and room for one mask per unit.
        unsafe {
            out.add(index).write(classes_of(u32::from(at.read())));
            at = at.add(1);
        }
        index += 1;
    }
    last
}

unsafe fn scan(first: *const u16, last: *const u16, mask: i16, wanted: bool) -> *const u16 {
    let mut at = first;
    while at < last {
        // SAFETY: the program's range.
        let unit = unsafe { at.read() };
        if (classes_of(u32::from(unit)) & mask != 0) == wanted {
            return at;
        }
        // SAFETY: as above.
        at = unsafe { at.add(1) };
    }
    last
}

unsafe extern "efiapi" fn ctype_scan_is(
    _this: *mut u8,
    mask: i16,
    first: *const u16,
    last: *const u16,
) -> *const u16 {
    // SAFETY: the program's range.
    unsafe { scan(first, last, mask, true) }
}

unsafe extern "efiapi" fn ctype_scan_not(
    _this: *mut u8,
    mask: i16,
    first: *const u16,
    last: *const u16,
) -> *const u16 {
    // SAFETY: the program's range.
    unsafe { scan(first, last, mask, false) }
}

fn lower(unit: u16) -> u16 {
    char::from_u32(u32::from(unit))
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .and_then(|character| u16::try_from(u32::from(character)).ok())
        .unwrap_or(unit)
}

fn upper(unit: u16) -> u16 {
    char::from_u32(u32::from(unit))
        .map(|character| character.to_uppercase().next().unwrap_or(character))
        .and_then(|character| u16::try_from(u32::from(character)).ok())
        .unwrap_or(unit)
}

extern "efiapi" fn ctype_lower(_this: *mut u8, unit: u16) -> u16 {
    lower(unit)
}

extern "efiapi" fn ctype_upper(_this: *mut u8, unit: u16) -> u16 {
    upper(unit)
}

unsafe fn each_unit(first: *mut u16, last: *const u16, change: fn(u16) -> u16) -> *const u16 {
    let mut at = first;
    while at.cast_const() < last {
        // SAFETY: the program's range.
        unsafe {
            at.write(change(at.read()));
            at = at.add(1);
        }
    }
    last
}

unsafe extern "efiapi" fn ctype_lower_range(
    _this: *mut u8,
    first: *mut u16,
    last: *const u16,
) -> *const u16 {
    // SAFETY: the program's range.
    unsafe { each_unit(first, last, lower) }
}

unsafe extern "efiapi" fn ctype_upper_range(
    _this: *mut u8,
    first: *mut u16,
    last: *const u16,
) -> *const u16 {
    // SAFETY: the program's range.
    unsafe { each_unit(first, last, upper) }
}

extern "efiapi" fn ctype_widen(_this: *mut u8, byte: u8) -> u16 {
    u16::from(byte)
}

unsafe extern "efiapi" fn ctype_widen_range(
    _this: *mut u8,
    first: *const u8,
    last: *const u8,
    out: *mut u16,
) -> *const u8 {
    let mut at = first;
    let mut index = 0;
    while at < last {
        // SAFETY: the program's range and room for it.
        unsafe {
            out.add(index).write(u16::from(at.read()));
            at = at.add(1);
        }
        index += 1;
    }
    last
}

extern "efiapi" fn ctype_narrow(_this: *mut u8, unit: u16, default: u8) -> u8 {
    u8::try_from(unit).ok().filter(u8::is_ascii).unwrap_or(default)
}

unsafe extern "efiapi" fn ctype_narrow_range(
    _this: *mut u8,
    first: *const u16,
    last: *const u16,
    default: u8,
    out: *mut u8,
) -> *const u16 {
    let mut at = first;
    let mut index = 0;
    while at < last {
        // SAFETY: the program's range and room for it.
        unsafe {
            out.add(index).write(ctype_narrow(null_mut(), at.read(), default));
            at = at.add(1);
        }
        index += 1;
    }
    last
}

unsafe extern "efiapi" fn ctype_category(facet: *mut *const u8, _locale: *const Locale) -> usize {
    if !facet.is_null() {
        // SAFETY: the caller's slot for the facet.
        unsafe {
            if (*facet).is_null() {
                facet.write(CTYPE_FACET);
            }
        }
    }
    CATEGORY_CTYPE
}

unsafe extern "efiapi" fn put_with_stream(this: *mut u8, manipulator: usize) -> *mut u8 {
    // SAFETY: the program's manipulator, taking and giving back its stream.
    unsafe { call_program(manipulator, [this as usize, 0, 0, 0]) as *mut u8 }
}

/// The wide standard objects and the data the headers import.
pub(in crate::win32) const DATA_EXPORTS: &[Export] = &[
    Export {
        name: b"?wcout@std@@3V?$basic_ostream@_WU?$char_traits@_W@std@@@1@A",
        address: (&raw const WCOUT).cast(),
    },
    Export {
        name: b"?wcerr@std@@3V?$basic_ostream@_WU?$char_traits@_W@std@@@1@A",
        address: (&raw const WCERR).cast(),
    },
    Export {
        name: b"?wclog@std@@3V?$basic_ostream@_WU?$char_traits@_W@std@@@1@A",
        address: (&raw const WCLOG).cast(),
    },
    Export {
        name: b"?wcin@std@@3V?$basic_istream@_WU?$char_traits@_W@std@@@1@A",
        address: (&raw const WCIN).cast(),
    },
    Export {
        name: b"?id@?$ctype@_W@std@@2V0locale@2@A",
        address: (&raw const CTYPE_ID).cast(),
    },
    Export {
        name: b"??_7?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@6B@",
        address: (&raw const BASE_TABLE.0).cast(),
    },
    Export {
        name: b"??_7?$basic_ios@_WU?$char_traits@_W@std@@@std@@6B@",
        address: (&raw const super::IOS_TABLE.0).cast(),
    },
];

/// The `wchar_t` members and functions the headers import.
pub(in crate::win32) const EXPORTS: &[Export] = exports![
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@_N@Z" => put_bool [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@F@Z" => put_short [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@G@Z" => put_unsigned_short [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@H@Z" => put_int [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@I@Z" => put_unsigned_int [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@J@Z" => put_int [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@K@Z" => put_unsigned_int [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@_J@Z" => put_long_long [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@_K@Z" => put_unsigned_long_long [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@M@Z" => put_float [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@N@Z" => put_double [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@O@Z" => put_long_double [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@PEBX@Z" => put_pointer [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@PEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@1@@Z" => put_buffer [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@P6AAEAV01@AEAV01@@Z@Z" => put_with_stream [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@P6AAEAV?$basic_ios@_WU?$char_traits@_W@std@@@1@AEAV21@@Z@Z" => super::put_with_ios [throwing],
    b"??6?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@P6AAEAVios_base@1@AEAV21@@Z@Z" => super::put_with_ios [throwing],
    b"?put@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@_W@Z" => put [throwing],
    b"?write@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@PEB_W_J@Z" => write [throwing],
    b"?flush@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@XZ" => super::flush [throwing],
    b"?_Osfx@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAXXZ" => super::output_suffix [throwing],
    b"?osfx@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAAXXZ" => super::output_suffix [throwing],
    b"?opfx@?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAA_NXZ" => super::output_prefix [throwing],
    b"??0?$basic_ostream@_WU?$char_traits@_W@std@@@std@@QEAA@PEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@1@_N@Z" => ostream_make,
    b"??1?$basic_ostream@_WU?$char_traits@_W@std@@@std@@UEAA@XZ" => super::stream_unmake,
    b"?endl@std@@YAAEAV?$basic_ostream@_WU?$char_traits@_W@std@@@1@AEAV21@@Z" => end_line [throwing],
    b"?ends@std@@YAAEAV?$basic_ostream@_WU?$char_traits@_W@std@@@1@AEAV21@@Z" => end_string [throwing],
    b"?flush@std@@YAAEAV?$basic_ostream@_WU?$char_traits@_W@std@@@1@AEAV21@@Z" => super::flush [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEA_N@Z" => get_bool [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAF@Z" => get_short [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAG@Z" => get_unsigned_short [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAH@Z" => get_int [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAI@Z" => get_unsigned_int [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAJ@Z" => get_int [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAK@Z" => get_unsigned_int [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEA_J@Z" => get_long_long [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEA_K@Z" => get_unsigned_long_long [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAM@Z" => get_float [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAN@Z" => get_double [throwing],
    b"??5?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV01@AEAO@Z" => get_double [throwing],
    b"?get@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAGXZ" => get_one [throwing],
    b"?peek@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAGXZ" => peek [throwing],
    b"?getline@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@PEA_W_J@Z" => get_line [throwing],
    b"?getline@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@PEA_W_J_W@Z" => get_line_until [throwing],
    b"?read@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@PEA_W_J@Z" => read [throwing],
    b"?ignore@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@_JG@Z" => ignore [throwing],
    b"?gcount@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEBA_JXZ" => super::gcount,
    b"?putback@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAAEAV12@_W@Z" => put_back [throwing],
    b"?_Ipfx@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAA_N_N@Z" => input_prefix [throwing],
    b"?ipfx@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAA_N_N@Z" => input_prefix [throwing],
    b"?isfx@?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAAXXZ" => super::input_suffix,
    b"??0?$basic_istream@_WU?$char_traits@_W@std@@@std@@QEAA@PEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@1@_N@Z" => istream_make,
    b"??1?$basic_istream@_WU?$char_traits@_W@std@@@std@@UEAA@XZ" => super::stream_unmake,
    b"?clear@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEAAXH_N@Z" => ios_clear [throwing],
    b"?setstate@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEAAXH_N@Z" => super::ios_set_state [throwing],
    b"?init@?$basic_ios@_WU?$char_traits@_W@std@@@std@@IEAAXPEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@2@_N@Z" => ios_init,
    b"??0?$basic_ios@_WU?$char_traits@_W@std@@@std@@IEAA@XZ" => ios_make,
    b"??1?$basic_ios@_WU?$char_traits@_W@std@@@std@@UEAA@XZ" => super::ios_unmake,
    b"?rdbuf@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEBAPEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@2@XZ" => super::ios_buffer,
    b"?rdbuf@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEAAPEAV?$basic_streambuf@_WU?$char_traits@_W@std@@@2@PEAV32@@Z" => super::ios_set_buffer,
    b"?tie@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEBAPEAV?$basic_ostream@_WU?$char_traits@_W@std@@@2@XZ" => super::ios_tie,
    b"?tie@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEAAPEAV?$basic_ostream@_WU?$char_traits@_W@std@@@2@PEAV32@@Z" => super::ios_set_tie,
    b"?fill@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEBA_WXZ" => ios_fill,
    b"?fill@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEAA_W_W@Z" => ios_set_fill,
    b"?widen@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEBA_WD@Z" => ios_widen,
    b"?narrow@?$basic_ios@_WU?$char_traits@_W@std@@@std@@QEBAD_WD@Z" => ios_narrow,
    b"??0?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAA@XZ" => buffer_make,
    b"??0?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAA@AEBV01@@Z" => buffer_copy,
    b"??1?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@UEAA@XZ" => super::buffer_unmake,
    b"?_Lock@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@UEAAXXZ" => nothing_on_buffer,
    b"?_Unlock@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@UEAAXXZ" => nothing_on_buffer,
    b"?overflow@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAGG@Z" => base_overflow,
    b"?pbackfail@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAGG@Z" => base_overflow,
    b"?showmanyc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAA_JXZ" => zero_available,
    b"?underflow@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAGXZ" => base_underflow,
    b"?uflow@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAGXZ" => base_uflow,
    b"?xsgetn@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAA_JPEA_W_J@Z" => base_xsgetn,
    b"?xsputn@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAA_JPEB_W_J@Z" => base_xsputn,
    b"?seekoff@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAA?AV?$fpos@U_Mbstatet@@@2@_JHH@Z" => no_seek_offset,
    b"?seekpos@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAA?AV?$fpos@U_Mbstatet@@@2@V32@H@Z" => no_seek_position,
    b"?setbuf@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAPEAV12@PEA_W_J@Z" => keep_buffer,
    b"?sync@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAHXZ" => base_sync,
    b"?imbue@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@MEAAXAEBVlocale@2@@Z" => nothing_on_locale,
    b"?sputc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAAG_W@Z" => buffer_sputc,
    b"?sputn@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAA_JPEB_W_J@Z" => buffer_sputn,
    b"?sgetc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAAGXZ" => buffer_sgetc,
    b"?sbumpc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAAGXZ" => buffer_sbumpc,
    b"?snextc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAAGXZ" => buffer_snextc,
    b"?sgetn@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAA_JPEA_W_J@Z" => buffer_sgetn,
    b"?pubsync@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAAHXZ" => buffer_pubsync,
    b"?in_avail@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEAA_JXZ" => buffer_in_available,
    b"?setg@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAAXPEA_W00@Z" => buffer_set_get,
    b"?setp@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAAXPEA_W0@Z" => buffer_set_put,
    b"?gptr@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => super::buffer_get_next,
    b"?pptr@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => super::buffer_put_next,
    b"?eback@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => super::buffer_get_first,
    b"?pbase@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => super::buffer_put_first,
    b"?egptr@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => buffer_get_end,
    b"?epptr@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEBAPEA_WXZ" => buffer_put_end,
    b"?gbump@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAAXH@Z" => buffer_get_bump,
    b"?pbump@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAAXH@Z" => buffer_put_bump,
    b"?_Init@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@IEAAXXZ" => super::buffer_init,
    b"?getloc@?$basic_streambuf@_WU?$char_traits@_W@std@@@std@@QEBA?AVlocale@2@XZ" => super::buffer_locale,
    b"?_Getcat@?$ctype@_W@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => ctype_category,
];

