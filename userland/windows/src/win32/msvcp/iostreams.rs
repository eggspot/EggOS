//! msvcp140's iostreams for `char` (D405): `std::cin`, `cout`, `cerr` and `clog` as objects
//! laid out as MSVC's 64-bit headers lay them out (a virtual base `basic_ios` after the
//! stream's base pointer and its `vtordisp`), with `basic_streambuf`s of the layer's own that
//! read and write the C runtime's standard streams, so `std::cout` and `printf` keep their
//! order while `sync_with_stdio` holds; the out-of-line members of `ios_base`, `basic_ios`,
//! `basic_streambuf`, `basic_ostream` and `basic_istream` the headers import, formatting and
//! reading numbers as `num_put` and `num_get` do in the classic locale
//! (`crate::stream_format`); the manipulators `endl`, `ends` and `flush`; the classic locale
//! with `ctype<char>` its one facet, which inline `use_facet` reaches; and `_Fiopen`, which
//! `basic_filebuf` opens files with. A program's own stream buffers (`basic_stringbuf`,
//! `basic_filebuf`) are reached through their tables of virtual functions, so they work
//! under every member here.

use super::super::crt::{self, File};
use super::super::{call_program, heap};
use crate::library::Export;
use crate::stream_format::{self, Integer, Read, flags};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr::null_mut;

const GOOD: i32 = 0;
const END: i32 = 1;
const FAILED: i32 = 2;
const BROKEN: i32 = 4;
const EOF: i32 = -1;
const STANDARD_FORMAT: i32 = flags::SKIPWS | flags::DEC;
const STANDARD_PRECISION: i64 = 6;
const OSTREAM_BASE_OFFSET: i32 = 16;
const ISTREAM_BASE_OFFSET: i32 = 24;
const SLOT_LOCK: usize = 1;
const SLOT_UNLOCK: usize = 2;
const SLOT_OVERFLOW: usize = 3;
const SLOT_UNDERFLOW: usize = 6;
const SLOT_UFLOW: usize = 7;
const SLOT_XSGETN: usize = 8;
const SLOT_XSPUTN: usize = 9;
const SLOT_SYNC: usize = 13;
const LONGEST_NUMBER: usize = 128;
const CATEGORY_CTYPE: usize = 2;
const ALL_CATEGORIES: i32 = 0x3f;
const OPEN_IN: i32 = 0x1;
const OPEN_OUT: i32 = 0x2;
const OPEN_APPEND: i32 = 0x8;
const OPEN_TRUNCATE: i32 = 0x10;
const OPEN_BINARY: i32 = 0x20;

const CLASS_UPPER: i16 = 0x1;
const CLASS_LOWER: i16 = 0x2;
const CLASS_DIGIT: i16 = 0x4;
const CLASS_SPACE: i16 = 0x8;
const CLASS_PUNCT: i16 = 0x10;
const CLASS_CONTROL: i16 = 0x20;
const CLASS_BLANK: i16 = 0x40;
const CLASS_HEX: i16 = 0x80;
const CLASS_ALPHA: i16 = 0x100;

#[repr(transparent)]
struct Shared<T>(UnsafeCell<T>);

// SAFETY: the program uses the standard streams under its own rules, as on Windows; the layer
// reaches them only through the program's calls.
unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }
}

macro_rules! shared_pointer {
    ($static:ident) => {
        UnsafeCell::raw_get(&raw const $static.0)
    };
}

#[repr(transparent)]
struct Table<const N: usize>([*const (); N]);

// SAFETY: the table holds the layer's functions' addresses, fixed for the program's life.
unsafe impl<const N: usize> Sync for Table<N> {}

#[repr(transparent)]
struct Offsets([i32; 2]);

#[repr(C)]
struct IosBase {
    table: *const *const (),
    standard: usize,
    state: i32,
    exceptions: i32,
    format: i32,
    reserved: i32,
    precision: i64,
    width: i64,
    words: usize,
    callbacks: usize,
    locale: *mut Locale,
}

#[repr(C)]
struct BasicIos {
    base: IosBase,
    buffer: *mut Streambuf,
    tie: *mut u8,
    fill: u8,
    reserved: [u8; 7],
}

#[repr(C)]
struct Ostream {
    bases: *const Offsets,
    reserved: u32,
    vtordisp: u32,
    ios: BasicIos,
}

#[repr(C)]
struct Istream {
    bases: *const Offsets,
    count: i64,
    reserved: u32,
    vtordisp: u32,
    ios: BasicIos,
}

#[repr(C)]
struct Streambuf {
    table: *const *const (),
    get_first: *mut u8,
    put_first: *mut u8,
    get_first_at: *mut *mut u8,
    put_first_at: *mut *mut u8,
    get_next: *mut u8,
    put_next: *mut u8,
    get_next_at: *mut *mut u8,
    put_next_at: *mut *mut u8,
    get_count: i32,
    put_count: i32,
    get_count_at: *mut i32,
    put_count_at: *mut i32,
    locale: *mut Locale,
}

#[repr(C)]
struct StandardBuffer {
    base: Streambuf,
    stream: usize,
}

#[repr(C)]
struct Locale {
    implementation: *mut Locimp,
}

#[repr(C)]
struct Locimp {
    table: *const *const (),
    references: u32,
    reserved: u32,
    facets: *const *const CtypeChar,
    facet_count: usize,
    categories: i32,
    external_parent: bool,
    reserved_tail: [u8; 3],
    name: *const u8,
    name_nul: usize,
}

#[repr(C)]
struct CtypeChar {
    table: *const *const (),
    references: u32,
    reserved: u32,
    page: u32,
    reserved_page: u32,
    classes: *const i16,
    delete: i32,
    reserved_delete: u32,
    locale_name: usize,
}

#[repr(C)]
struct LocaleId {
    id: usize,
}

#[repr(C)]
struct Position {
    offset: i64,
    file_position: i64,
    state: u64,
}

#[repr(transparent)]
struct Facets([*const CtypeChar; 2]);

// SAFETY: the facet pointers are fixed for the program's life.
unsafe impl Sync for Facets {}

const fn class_of(byte: u8) -> i16 {
    let mut class = 0;
    if byte.is_ascii_uppercase() {
        class |= CLASS_UPPER | CLASS_ALPHA;
    }
    if byte.is_ascii_lowercase() {
        class |= CLASS_LOWER | CLASS_ALPHA;
    }
    if byte.is_ascii_digit() {
        class |= CLASS_DIGIT;
    }
    if byte.is_ascii_hexdigit() {
        class |= CLASS_HEX;
    }
    if matches!(byte, b' ' | 0x09..=0x0d) {
        class |= CLASS_SPACE;
    }
    if matches!(byte, b' ' | b'\t') {
        class |= CLASS_BLANK;
    }
    if byte.is_ascii_punctuation() {
        class |= CLASS_PUNCT;
    }
    if byte.is_ascii_control() {
        class |= CLASS_CONTROL;
    }
    class
}

const fn classes() -> [i16; 256] {
    let mut table = [0i16; 256];
    let mut byte = 0;
    while byte < 256 {
        table[byte] = class_of(byte as u8);
        byte += 1;
    }
    table
}

static CLASSES: [i16; 256] = classes();
static NAME_C: [u8; 2] = *b"C\0";
static NULL_CELL: Shared<*mut u8> = Shared::new(null_mut());
static ZERO_COUNT: Shared<i32> = Shared::new(0);
static OSTREAM_BASES: Offsets = Offsets([0, OSTREAM_BASE_OFFSET]);
static ISTREAM_BASES: Offsets = Offsets([0, ISTREAM_BASE_OFFSET]);
static ID_COUNT: Shared<i32> = Shared::new(1);
static CTYPE_ID: Shared<LocaleId> = Shared::new(LocaleId { id: 1 });
static SYNC_WITH_STDIO: Shared<bool> = Shared::new(true);
static INIT_COUNT: Shared<i32> = Shared::new(1);
static NEXT_WORD: Shared<i32> = Shared::new(0);

static CTYPE: Shared<CtypeChar> = Shared::new(CtypeChar {
    table: (&raw const CTYPE_TABLE.0).cast(),
    references: 1,
    reserved: 0,
    page: 0,
    reserved_page: 0,
    classes: (&raw const CLASSES).cast(),
    delete: 0,
    reserved_delete: 0,
    locale_name: 0,
});

static FACETS: Facets = Facets([core::ptr::null(), shared_pointer!(CTYPE).cast_const()]);

static LOCIMP: Shared<Locimp> = Shared::new(Locimp {
    table: (&raw const FACET_TABLE.0).cast(),
    references: 1,
    reserved: 0,
    facets: (&raw const FACETS.0).cast(),
    facet_count: 2,
    categories: ALL_CATEGORIES,
    external_parent: false,
    reserved_tail: [0; 3],
    name: (&raw const NAME_C).cast(),
    name_nul: 0,
});

static CLASSIC: Shared<Locale> = Shared::new(Locale {
    implementation: shared_pointer!(LOCIMP),
});

macro_rules! standard_buffer {
    ($stream:expr) => {
        Shared::new(StandardBuffer {
            base: Streambuf {
                table: (&raw const STANDARD_TABLE.0).cast(),
                get_first: null_mut(),
                put_first: null_mut(),
                get_first_at: shared_pointer!(NULL_CELL),
                put_first_at: shared_pointer!(NULL_CELL),
                get_next: null_mut(),
                put_next: null_mut(),
                get_next_at: shared_pointer!(NULL_CELL),
                put_next_at: shared_pointer!(NULL_CELL),
                get_count: 0,
                put_count: 0,
                get_count_at: shared_pointer!(ZERO_COUNT),
                put_count_at: shared_pointer!(ZERO_COUNT),
                locale: shared_pointer!(CLASSIC),
            },
            stream: $stream,
        })
    };
}

static IN_BUFFER: Shared<StandardBuffer> = standard_buffer!(0);
static OUT_BUFFER: Shared<StandardBuffer> = standard_buffer!(1);
static ERROR_BUFFER: Shared<StandardBuffer> = standard_buffer!(2);

macro_rules! ios {
    ($table:ident, $format:expr, $buffer:ident, $tie:expr) => {
        BasicIos {
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
                locale: shared_pointer!(CLASSIC),
            },
            buffer: shared_pointer!($buffer).cast(),
            tie: $tie,
            fill: b' ',
            reserved: [0; 7],
        }
    };
}

static COUT: Shared<Ostream> = Shared::new(Ostream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: ios!(OSTREAM_IOS_TABLE, STANDARD_FORMAT, OUT_BUFFER, null_mut()),
});

static CERR: Shared<Ostream> = Shared::new(Ostream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: ios!(
        OSTREAM_IOS_TABLE,
        STANDARD_FORMAT | flags::UNITBUF,
        ERROR_BUFFER,
        shared_pointer!(COUT).cast()
    ),
});

static CLOG: Shared<Ostream> = Shared::new(Ostream {
    bases: &raw const OSTREAM_BASES,
    reserved: 0,
    vtordisp: 0,
    ios: ios!(OSTREAM_IOS_TABLE, STANDARD_FORMAT, ERROR_BUFFER, null_mut()),
});

static CIN: Shared<Istream> = Shared::new(Istream {
    bases: &raw const ISTREAM_BASES,
    count: 0,
    reserved: 0,
    vtordisp: 0,
    ios: ios!(
        ISTREAM_IOS_TABLE,
        STANDARD_FORMAT,
        IN_BUFFER,
        shared_pointer!(COUT).cast()
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

static IOS_TABLE: Table<1> = Table([export_entry!(ios_destroy)]);
static OSTREAM_IOS_TABLE: Table<1> = Table([export_entry!(ios_destroy)]);
static ISTREAM_IOS_TABLE: Table<1> = Table([export_entry!(ios_destroy)]);

static FACET_TABLE: Table<3> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
]);

static CTYPE_TABLE: Table<11> = Table([
    export_entry!(facet_destroy),
    export_entry!(facet_add_reference),
    export_entry!(facet_release),
    export_entry!(ctype_lower),
    export_entry!(ctype_lower_range),
    export_entry!(ctype_upper),
    export_entry!(ctype_upper_range),
    export_entry!(ctype_widen_range),
    export_entry!(ctype_widen),
    export_entry!(ctype_narrow_range),
    export_entry!(ctype_narrow),
]);

fn standard_file(buffer: *mut StandardBuffer) -> *mut File {
    // SAFETY: one of the layer's standard buffers.
    crt::standard_file(unsafe { (*buffer).stream })
}

unsafe extern "efiapi" fn standard_destroy(this: *mut u8, _flags: u32) -> *mut u8 {
    this
}

extern "efiapi" fn nothing_on_buffer(_this: *mut u8) {}

extern "efiapi" fn nothing_on_locale(_this: *mut u8, _locale: *const Locale) {}

extern "efiapi" fn zero_available(_this: *mut u8) -> i64 {
    0
}

extern "efiapi" fn keep_buffer(this: *mut u8, _buffer: *mut u8, _count: i64) -> *mut u8 {
    this
}

unsafe extern "efiapi" fn no_seek_offset(
    _this: *mut u8,
    result: *mut Position,
    _offset: i64,
    _way: i32,
    _which: i32,
) -> *mut Position {
    // SAFETY: the caller's room for the returned `fpos`.
    unsafe {
        result.write(Position {
            offset: -1,
            file_position: 0,
            state: 0,
        })
    };
    result
}

unsafe extern "efiapi" fn no_seek_position(
    this: *mut u8,
    result: *mut Position,
    _position: *const Position,
    which: i32,
) -> *mut Position {
    // SAFETY: as above.
    unsafe { no_seek_offset(this, result, 0, 0, which) }
}

extern "efiapi" fn standard_overflow(this: *mut StandardBuffer, character: i32) -> i32 {
    if character == EOF {
        return 0;
    }
    match crt::stream_write(standard_file(this), &[character as u8]) {
        1 => character,
        _ => EOF,
    }
}

extern "efiapi" fn standard_pbackfail(this: *mut StandardBuffer, character: i32) -> i32 {
    if character == EOF || !crt::stream_unget(standard_file(this), character as u8) {
        return EOF;
    }
    character
}

extern "efiapi" fn standard_underflow(this: *mut StandardBuffer) -> i32 {
    crt::stream_read(standard_file(this), false).map_or(EOF, i32::from)
}

extern "efiapi" fn standard_uflow(this: *mut StandardBuffer) -> i32 {
    crt::stream_read(standard_file(this), true).map_or(EOF, i32::from)
}

unsafe extern "efiapi" fn standard_xsgetn(
    this: *mut StandardBuffer,
    out: *mut u8,
    count: i64,
) -> i64 {
    let mut read = 0;
    while read < count {
        let Some(byte) = crt::stream_read(standard_file(this), true) else {
            break;
        };
        // SAFETY: the caller's room for `count` bytes.
        unsafe { out.add(read as usize).write(byte) };
        read += 1;
    }
    read
}

unsafe extern "efiapi" fn standard_xsputn(
    this: *mut StandardBuffer,
    bytes: *const u8,
    count: i64,
) -> i64 {
    if count <= 0 {
        return 0;
    }
    // SAFETY: the caller's `count` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(bytes, count as usize) };
    crt::stream_write(standard_file(this), bytes) as i64
}

extern "efiapi" fn standard_sync(this: *mut StandardBuffer) -> i32 {
    match crt::stream_flush(standard_file(this)) {
        true => 0,
        false => -1,
    }
}

unsafe fn buffer_call(buffer: *mut Streambuf, slot: usize, arguments: [usize; 3]) -> usize {
    // SAFETY: the caller's duty: a live stream buffer, whose first word is its table.
    let method = unsafe { (*buffer).table.add(slot).read() } as usize;
    // SAFETY: as above: the buffer's own virtual function, called as MSVC calls it.
    unsafe {
        call_program(
            method,
            [buffer as usize, arguments[0], arguments[1], arguments[2]],
        )
    }
}

unsafe fn available(next_at: *mut *mut u8, count_at: *mut i32) -> i32 {
    // SAFETY: the caller's duty: a buffer's own cells.
    unsafe {
        match (*next_at).is_null() {
            true => 0,
            false => *count_at,
        }
    }
}

unsafe fn put_char(buffer: *mut Streambuf, byte: u8) -> i32 {
    // SAFETY: the caller's duty: a live buffer.
    unsafe {
        let held = &*buffer;
        if available(held.put_next_at, held.put_count_at) > 0 {
            let next = *held.put_next_at;
            next.write(byte);
            *held.put_next_at = next.add(1);
            *held.put_count_at -= 1;
            return i32::from(byte);
        }
        buffer_call(buffer, SLOT_OVERFLOW, [usize::from(byte), 0, 0]) as u32 as i32
    }
}

unsafe fn put_bytes(buffer: *mut Streambuf, bytes: &[u8]) -> i64 {
    // SAFETY: the caller's duty.
    unsafe {
        buffer_call(
            buffer,
            SLOT_XSPUTN,
            [bytes.as_ptr() as usize, bytes.len(), 0],
        ) as i64
    }
}

unsafe fn get_char(buffer: *mut Streambuf, take: bool) -> i32 {
    // SAFETY: the caller's duty: a live buffer.
    unsafe {
        let held = &*buffer;
        if available(held.get_next_at, held.get_count_at) > 0 {
            let next = *held.get_next_at;
            let byte = next.read();
            if take {
                *held.get_next_at = next.add(1);
                *held.get_count_at -= 1;
            }
            return i32::from(byte);
        }
        let slot = if take { SLOT_UFLOW } else { SLOT_UNDERFLOW };
        buffer_call(buffer, slot, [0; 3]) as u32 as i32
    }
}

unsafe fn sync_buffer(buffer: *mut Streambuf) -> i32 {
    // SAFETY: the caller's duty.
    unsafe { buffer_call(buffer, SLOT_SYNC, [0; 3]) as u32 as i32 }
}

unsafe fn ios_of(stream: *mut u8) -> *mut BasicIos {
    // SAFETY: the caller's duty: a stream with a virtual `basic_ios`, its first word the
    // table of its bases' offsets.
    unsafe {
        let offsets = stream.cast::<*const Offsets>().read();
        stream.add((*offsets).0[1] as usize).cast()
    }
}

unsafe fn set_state(ios: *mut BasicIos, bits: i32) {
    // SAFETY: the caller's duty: a live `basic_ios`.
    unsafe {
        let mut state = (*ios).base.state | bits;
        if (*ios).buffer.is_null() {
            state |= BROKEN;
        }
        (*ios).base.state = state;
    }
}

unsafe fn flush_stream(stream: *mut u8) {
    // SAFETY: the caller's duty: a live output stream.
    unsafe {
        let ios = ios_of(stream);
        let buffer = (*ios).buffer;
        if !buffer.is_null() && sync_buffer(buffer) == -1 {
            set_state(ios, BROKEN);
        }
    }
}

unsafe fn output_ready(stream: *mut u8) -> bool {
    // SAFETY: the caller's duty: a live output stream.
    unsafe {
        let ios = ios_of(stream);
        if (*ios).base.state != GOOD {
            return false;
        }
        let tie = (*ios).tie;
        if !tie.is_null() && tie != stream {
            flush_stream(tie);
        }
        let buffer = (*ios).buffer;
        if buffer.is_null() {
            set_state(ios, BROKEN);
            return false;
        }
        buffer_call(buffer, SLOT_LOCK, [0; 3]);
        (*ios).base.state == GOOD
    }
}

unsafe fn output_done(stream: *mut u8) {
    // SAFETY: the caller's duty: a live output stream that passed `output_ready`.
    unsafe {
        let ios = ios_of(stream);
        if !(*ios).buffer.is_null() {
            buffer_call((*ios).buffer, SLOT_UNLOCK, [0; 3]);
        }
        if (*ios).base.format & flags::UNITBUF != 0 {
            flush_stream(stream);
        }
    }
}

unsafe fn write_field(stream: *mut u8, body: &[u8], prefix: usize) -> *mut u8 {
    // SAFETY: the caller's duty: a live output stream.
    unsafe {
        if !output_ready(stream) {
            return stream;
        }
        let ios = ios_of(stream);
        let padded = stream_format::pad(
            body,
            prefix,
            (*ios).base.format,
            (*ios).base.width,
            (*ios).fill,
        );
        (*ios).base.width = 0;
        if put_bytes((*ios).buffer, &padded) != padded.len() as i64 {
            set_state(ios, BROKEN);
        }
        output_done(stream);
    }
    stream
}

unsafe fn write_integer(stream: *mut u8, value: Integer) -> *mut u8 {
    // SAFETY: the caller's duty.
    let format = unsafe { (*ios_of(stream)).base.format };
    let (body, prefix) = stream_format::integer(value, format);
    // SAFETY: as above.
    unsafe { write_field(stream, &body, prefix) }
}

struct FloatArguments {
    precision: i64,
    value: f64,
}

impl eggos_c_format::Arguments for FloatArguments {
    fn integer(&mut self) -> u64 {
        self.precision as u64
    }

    fn double(&mut self) -> f64 {
        self.value
    }
}

struct Collected(Vec<u8>);

impl eggos_c_format::Output for Collected {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

unsafe fn write_float(stream: *mut u8, value: f64, long_double: bool) -> *mut u8 {
    // SAFETY: the caller's duty.
    let (format, precision) = unsafe {
        let ios = ios_of(stream);
        ((*ios).base.format, (*ios).base.precision)
    };
    let spec = stream_format::float_specification(format, long_double);
    let mut arguments = FloatArguments { precision, value };
    let mut out = Collected(Vec::new());
    let dialect = eggos_c_format::Dialect::Windows {
        wide_strings: false,
    };
    let spec = &spec[..spec.len() - 1];
    // SAFETY: the specification takes at most the precision and the value, which the
    // arguments give.
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
            // SAFETY: the member's `this`, a live output stream.
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
    // SAFETY: the member's `this`, a live output stream.
    let format = unsafe { (*ios_of(this)).base.format };
    if format & flags::BOOLALPHA == 0 {
        // SAFETY: as above.
        return unsafe { put_int(this, i32::from(value != 0)) };
    }
    let words: &[u8] = if value != 0 { b"true" } else { b"false" };
    // SAFETY: as above.
    unsafe { write_field(this, words, 0) }
}

unsafe extern "efiapi" fn put_double(this: *mut u8, value: f64) -> *mut u8 {
    // SAFETY: the member's `this`, a live output stream.
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
    // SAFETY: the member's `this`, a live output stream.
    unsafe { write_field(this, &text, 0) }
}

unsafe extern "efiapi" fn put_buffer(this: *mut u8, source: *mut Streambuf) -> *mut u8 {
    // SAFETY: the member's `this`, a live output stream; `source` null or a live buffer.
    unsafe {
        let ios = ios_of(this);
        if source.is_null() {
            set_state(ios, BROKEN);
            return this;
        }
        if !output_ready(this) {
            return this;
        }
        let mut copied = 0;
        loop {
            let byte = get_char(source, true);
            if byte == EOF {
                break;
            }
            if put_char((*ios).buffer, byte as u8) == EOF {
                set_state(ios, BROKEN);
                break;
            }
            copied += 1;
        }
        (*ios).base.width = 0;
        if copied == 0 {
            set_state(ios, FAILED);
        }
        output_done(this);
    }
    this
}

unsafe extern "efiapi" fn put_with_stream(this: *mut u8, manipulator: usize) -> *mut u8 {
    // SAFETY: the program's manipulator, taking and giving back its stream.
    unsafe { call_program(manipulator, [this as usize, 0, 0, 0]) as *mut u8 }
}

unsafe extern "efiapi" fn put_with_ios(this: *mut u8, manipulator: usize) -> *mut u8 {
    // SAFETY: as above, with the stream's `basic_ios`.
    unsafe { call_program(manipulator, [ios_of(this) as usize, 0, 0, 0]) };
    this
}

unsafe extern "efiapi" fn put(this: *mut u8, byte: u8) -> *mut u8 {
    // SAFETY: the member's `this`, a live output stream.
    unsafe {
        if output_ready(this) {
            let ios = ios_of(this);
            if put_char((*ios).buffer, byte) == EOF {
                set_state(ios, BROKEN);
            }
            output_done(this);
        }
    }
    this
}

unsafe extern "efiapi" fn write(this: *mut u8, bytes: *const u8, count: i64) -> *mut u8 {
    // SAFETY: the member's `this`, a live output stream, and the caller's `count` bytes.
    unsafe {
        if count > 0 && output_ready(this) {
            let ios = ios_of(this);
            let bytes = core::slice::from_raw_parts(bytes, count as usize);
            if put_bytes((*ios).buffer, bytes) != count {
                set_state(ios, BROKEN);
            }
            output_done(this);
        }
    }
    this
}

unsafe extern "efiapi" fn flush(this: *mut u8) -> *mut u8 {
    // SAFETY: the member's `this`, a live output stream.
    unsafe { flush_stream(this) };
    this
}

unsafe extern "efiapi" fn output_suffix(this: *mut u8) {
    // SAFETY: the member's `this`, a live output stream.
    unsafe {
        let ios = ios_of(this);
        if (*ios).base.format & flags::UNITBUF != 0 && (*ios).base.state == GOOD {
            flush_stream(this);
        }
    }
}

unsafe extern "efiapi" fn output_prefix(this: *mut u8) -> u8 {
    // SAFETY: the member's `this`, a live output stream.
    unsafe {
        let ios = ios_of(this);
        let tie = (*ios).tie;
        if (*ios).base.state == GOOD && !tie.is_null() && tie != this {
            flush_stream(tie);
        }
        u8::from((*ios).base.state == GOOD)
    }
}

unsafe extern "efiapi" fn end_line(stream: *mut u8) -> *mut u8 {
    // SAFETY: the manipulator's stream, a live output stream.
    unsafe {
        put(stream, b'\n');
        flush_stream(stream);
    }
    stream
}

unsafe extern "efiapi" fn end_string(stream: *mut u8) -> *mut u8 {
    // SAFETY: as above.
    unsafe { put(stream, 0) }
}

unsafe extern "efiapi" fn flush_manipulator(stream: *mut u8) -> *mut u8 {
    // SAFETY: as above.
    unsafe { flush(stream) }
}

unsafe fn input_ready(stream: *mut u8, no_skip: bool) -> bool {
    // SAFETY: the caller's duty: a live input stream.
    unsafe {
        let ios = ios_of(stream);
        if (*ios).base.state != GOOD {
            set_state(ios, FAILED);
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
            let next = get_char(buffer, false);
            if next == EOF {
                set_state(ios, END | FAILED);
                return false;
            }
            if CLASSES[next as u8 as usize] & CLASS_SPACE == 0 {
                return true;
            }
            get_char(buffer, true);
        }
    }
}

unsafe extern "efiapi" fn input_prefix(this: *mut u8, no_skip: u8) -> u8 {
    // SAFETY: the member's `this`, a live input stream.
    u8::from(unsafe { input_ready(this, no_skip != 0) })
}

extern "efiapi" fn input_suffix(_this: *mut u8) {}

fn accepts(byte: u8, taken: &[u8], base: u32, float: bool) -> bool {
    let first = taken.is_empty();
    let after_sign = taken.len() == 1 && matches!(taken[0], b'+' | b'-');
    if matches!(byte, b'+' | b'-') {
        return first || (float && matches!(taken.last(), Some(b'e' | b'E')));
    }
    if float {
        return byte.is_ascii_digit()
            || (byte == b'.' && !taken.contains(&b'.'))
            || (matches!(byte, b'e' | b'E') && !taken.iter().any(|b| matches!(b, b'e' | b'E')));
    }
    let leading_zero = taken.last() == Some(&b'0') && (taken.len() == 1 || after_sign);
    if matches!(byte, b'x' | b'X') && leading_zero && (base == 16 || base == 0) {
        return true;
    }
    match base {
        8 => (b'0'..=b'7').contains(&byte),
        16 => byte.is_ascii_hexdigit(),
        0 => byte.is_ascii_hexdigit(),
        _ => byte.is_ascii_digit(),
    }
}

unsafe fn take_number(stream: *mut u8, float: bool) -> Option<Vec<u8>> {
    // SAFETY: the caller's duty: a live input stream.
    unsafe {
        if !input_ready(stream, false) {
            return None;
        }
        let ios = ios_of(stream);
        let base = match (*ios).base.format & flags::BASEFIELD {
            flags::OCT => 8,
            flags::HEX => 16,
            flags::DEC => 10,
            _ => 0,
        };
        let buffer = (*ios).buffer;
        let mut taken = Vec::new();
        while taken.len() < LONGEST_NUMBER {
            let next = get_char(buffer, false);
            if next == EOF {
                set_state(ios, END);
                break;
            }
            if !accepts(next as u8, &taken, base, float) {
                break;
            }
            taken.push(next as u8);
            get_char(buffer, true);
        }
        Some(taken)
    }
}

macro_rules! input_integer {
    ($name:ident, $type:ty, $signed:expr, $bits:expr) => {
        unsafe extern "efiapi" fn $name(this: *mut u8, out: *mut $type) -> *mut u8 {
            // SAFETY: the member's `this`, a live input stream, and the caller's variable.
            unsafe {
                let Some(taken) = take_number(this, false) else {
                    return this;
                };
                let ios = ios_of(this);
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
        let ios = ios_of(this);
        match value {
            0 | 1 => out.write(value as u8),
            _ => {
                out.write(1);
                set_state(ios, FAILED);
            }
        }
    }
    this
}

unsafe fn read_float(this: *mut u8) -> Option<f64> {
    // SAFETY: the caller's duty: a live input stream.
    unsafe {
        let taken = take_number(this, true)?;
        let (value, used) = eggos_c_format::number::parse_float(&taken);
        if used == 0 || used != taken.len() {
            set_state(ios_of(this), FAILED);
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

fn input_of(this: *mut u8) -> *mut Istream {
    this.cast()
}

unsafe extern "efiapi" fn get_one(this: *mut u8) -> i32 {
    // SAFETY: the member's `this`, a live input stream.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return EOF;
        }
        let ios = ios_of(this);
        let byte = get_char((*ios).buffer, true);
        match byte {
            EOF => set_state(ios, END | FAILED),
            _ => (*input_of(this)).count = 1,
        }
        byte
    }
}

unsafe extern "efiapi" fn peek(this: *mut u8) -> i32 {
    // SAFETY: as above.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return EOF;
        }
        let ios = ios_of(this);
        let byte = get_char((*ios).buffer, false);
        if byte == EOF {
            set_state(ios, END);
        }
        byte
    }
}

unsafe fn read_until(this: *mut u8, out: *mut u8, capacity: i64, delimiter: Option<u8>) -> i64 {
    // SAFETY: the caller's duty: a live input stream and room for `capacity` bytes.
    unsafe {
        (*input_of(this)).count = 0;
        let ios = ios_of(this);
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
            let next = get_char((*ios).buffer, false);
            if next == EOF {
                state |= END;
                break;
            }
            if delimiter == Some(next as u8) {
                get_char((*ios).buffer, true);
                (*input_of(this)).count += 1;
                break;
            }
            if count >= room {
                if delimiter.is_some() {
                    state |= FAILED;
                }
                break;
            }
            out.add(count as usize).write(next as u8);
            get_char((*ios).buffer, true);
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
            set_state(ios, state);
        }
        count
    }
}

unsafe extern "efiapi" fn get_line(this: *mut u8, out: *mut u8, capacity: i64) -> *mut u8 {
    // SAFETY: the member's `this` and the caller's room.
    unsafe { read_until(this, out, capacity, Some(b'\n')) };
    this
}

unsafe extern "efiapi" fn get_line_until(
    this: *mut u8,
    out: *mut u8,
    capacity: i64,
    delimiter: u8,
) -> *mut u8 {
    // SAFETY: as above.
    unsafe { read_until(this, out, capacity, Some(delimiter)) };
    this
}

unsafe extern "efiapi" fn read(this: *mut u8, out: *mut u8, count: i64) -> *mut u8 {
    // SAFETY: as above.
    unsafe { read_until(this, out, count, None) };
    this
}

unsafe extern "efiapi" fn ignore(this: *mut u8, count: i64, delimiter: i32) -> *mut u8 {
    // SAFETY: the member's `this`, a live input stream.
    unsafe {
        (*input_of(this)).count = 0;
        if !input_ready(this, true) {
            return this;
        }
        let ios = ios_of(this);
        let mut skipped = 0i64;
        while count == i64::MAX || skipped < count {
            let byte = get_char((*ios).buffer, true);
            if byte == EOF {
                set_state(ios, END);
                break;
            }
            skipped += 1;
            if byte == delimiter {
                break;
            }
        }
        (*input_of(this)).count = skipped;
    }
    this
}

unsafe extern "efiapi" fn gcount(this: *mut u8) -> i64 {
    // SAFETY: the member's `this`, a live input stream.
    unsafe { (*input_of(this)).count }
}

unsafe extern "efiapi" fn put_back(this: *mut u8, byte: u8) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        (*input_of(this)).count = 0;
        let ios = ios_of(this);
        (*ios).base.state &= !END;
        if !input_ready(this, true) {
            return this;
        }
        let back = buffer_call((*ios).buffer, 4, [usize::from(byte), 0, 0]) as u32 as i32;
        if back == EOF {
            set_state(ios, BROKEN);
        }
    }
    this
}

unsafe extern "efiapi" fn ios_clear(this: *mut BasicIos, state: i32, _reraise: u8) {
    // SAFETY: the member's `this`, a live `ios_base` (the first part of a `basic_ios`).
    unsafe {
        let mut state = state & (END | FAILED | BROKEN);
        if (*this).buffer.is_null() && (*this).base.standard != 0 {
            state |= BROKEN;
        }
        (*this).base.state = state;
    }
}

unsafe extern "efiapi" fn ios_set_state(this: *mut BasicIos, bits: i32, reraise: u8) {
    // SAFETY: as above.
    unsafe { ios_clear(this, (*this).base.state | bits, reraise) }
}

unsafe extern "efiapi" fn ios_destroy(this: *mut u8, _flags: u32) -> *mut u8 {
    this
}

unsafe extern "efiapi" fn ios_init(this: *mut BasicIos, buffer: *mut Streambuf, standard: u8) {
    // SAFETY: the member's `this`, a `basic_ios` being made.
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
        base.locale = shared_pointer!(CLASSIC);
        (*this).buffer = buffer;
        (*this).tie = null_mut();
        (*this).fill = b' ';
    }
}

unsafe extern "efiapi" fn ios_make(this: *mut BasicIos) -> *mut BasicIos {
    // SAFETY: the constructor's `this`, room for a `basic_ios`.
    unsafe {
        (*this).base.table = (&raw const IOS_TABLE.0).cast();
        ios_init(this, null_mut(), 0);
    }
    this
}

extern "efiapi" fn ios_unmake(_this: *mut BasicIos) {}

unsafe extern "efiapi" fn ios_buffer(this: *mut BasicIos) -> *mut Streambuf {
    // SAFETY: the member's `this`.
    unsafe { (*this).buffer }
}

unsafe extern "efiapi" fn ios_set_buffer(
    this: *mut BasicIos,
    buffer: *mut Streambuf,
) -> *mut Streambuf {
    // SAFETY: as above.
    unsafe {
        let old = (*this).buffer;
        (*this).buffer = buffer;
        ios_clear(this, GOOD, 0);
        old
    }
}

unsafe extern "efiapi" fn ios_tie(this: *mut BasicIos) -> *mut u8 {
    // SAFETY: as above.
    unsafe { (*this).tie }
}

unsafe extern "efiapi" fn ios_set_tie(this: *mut BasicIos, tie: *mut u8) -> *mut u8 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).tie, tie) }
}

unsafe extern "efiapi" fn ios_fill(this: *mut BasicIos) -> u8 {
    // SAFETY: as above.
    unsafe { (*this).fill }
}

unsafe extern "efiapi" fn ios_set_fill(this: *mut BasicIos, fill: u8) -> u8 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).fill, fill) }
}

extern "efiapi" fn ios_widen(_this: *mut BasicIos, byte: u8) -> u8 {
    byte
}

extern "efiapi" fn ios_narrow(_this: *mut BasicIos, byte: u8, _default: u8) -> u8 {
    byte
}

unsafe extern "efiapi" fn base_width(this: *mut IosBase) -> i64 {
    // SAFETY: the member's `this`, a live `ios_base`.
    unsafe { (*this).width }
}

unsafe extern "efiapi" fn base_set_width(this: *mut IosBase, width: i64) -> i64 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).width, width) }
}

unsafe extern "efiapi" fn base_precision(this: *mut IosBase) -> i64 {
    // SAFETY: as above.
    unsafe { (*this).precision }
}

unsafe extern "efiapi" fn base_set_precision(this: *mut IosBase, precision: i64) -> i64 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).precision, precision) }
}

unsafe extern "efiapi" fn base_flags(this: *mut IosBase) -> i32 {
    // SAFETY: as above.
    unsafe { (*this).format }
}

unsafe extern "efiapi" fn base_set_flags(this: *mut IosBase, format: i32) -> i32 {
    // SAFETY: as above.
    unsafe { core::mem::replace(&mut (*this).format, format) }
}

unsafe extern "efiapi" fn base_set_some(this: *mut IosBase, format: i32) -> i32 {
    // SAFETY: as above.
    unsafe {
        let old = (*this).format;
        (*this).format |= format;
        old
    }
}

unsafe extern "efiapi" fn base_set_masked(this: *mut IosBase, format: i32, mask: i32) -> i32 {
    // SAFETY: as above.
    unsafe {
        let old = (*this).format;
        (*this).format = (old & !mask) | (format & mask);
        old
    }
}

unsafe extern "efiapi" fn base_unset(this: *mut IosBase, format: i32) {
    // SAFETY: as above.
    unsafe { (*this).format &= !format }
}

unsafe extern "efiapi" fn base_state(this: *mut IosBase) -> i32 {
    // SAFETY: as above.
    unsafe { (*this).state }
}

unsafe extern "efiapi" fn base_good(this: *mut IosBase) -> u8 {
    // SAFETY: as above.
    u8::from(unsafe { (*this).state } == GOOD)
}

unsafe extern "efiapi" fn base_eof(this: *mut IosBase) -> u8 {
    // SAFETY: as above.
    u8::from(unsafe { (*this).state } & END != 0)
}

unsafe extern "efiapi" fn base_fail(this: *mut IosBase) -> u8 {
    // SAFETY: as above.
    u8::from(unsafe { (*this).state } & (FAILED | BROKEN) != 0)
}

unsafe extern "efiapi" fn base_bad(this: *mut IosBase) -> u8 {
    // SAFETY: as above.
    u8::from(unsafe { (*this).state } & BROKEN != 0)
}

unsafe extern "efiapi" fn base_get_locale(this: *mut IosBase, result: *mut Locale) -> *mut Locale {
    // SAFETY: the member's `this` and the caller's room for the returned locale.
    unsafe {
        let implementation = match (*this).locale.is_null() {
            true => shared_pointer!(LOCIMP),
            false => (*(*this).locale).implementation,
        };
        result.write(Locale { implementation });
    }
    result
}

unsafe extern "efiapi" fn base_copy_format(
    this: *mut IosBase,
    other: *const IosBase,
) -> *mut IosBase {
    // SAFETY: the member's `this` and the program's other stream.
    unsafe {
        (*this).format = (*other).format;
        (*this).precision = (*other).precision;
        (*this).width = (*other).width;
        (*this).exceptions = (*other).exceptions;
    }
    this
}

extern "efiapi" fn base_unmake(_this: *mut IosBase) {}

extern "efiapi" fn base_new_index() -> i32 {
    // SAFETY: the counter is the layer's; programs take indices from one thread at a time, as
    // MSVC's own `xalloc` under its lock.
    unsafe {
        let counter = UnsafeCell::raw_get((&raw const NEXT_WORD).cast::<UnsafeCell<i32>>());
        let index = *counter;
        *counter += 1;
        index
    }
}

extern "efiapi" fn sync_with_stdio(sync: u8) -> u8 {
    // SAFETY: the flag is the layer's, read and written by the program's calls.
    unsafe {
        let flag = UnsafeCell::raw_get((&raw const SYNC_WITH_STDIO).cast::<UnsafeCell<bool>>());
        u8::from(core::mem::replace(&mut *flag, sync != 0))
    }
}

extern "efiapi" fn init_make(_this: *mut u8) {}

extern "efiapi" fn init_unmake(_this: *mut u8) {
    let _ = crt::stream_flush(crt::standard_file(1));
}

unsafe extern "efiapi" fn ostream_make(
    this: *mut Ostream,
    buffer: *mut Streambuf,
    standard: u8,
    most_derived: i32,
) -> *mut Ostream {
    // SAFETY: the constructor's `this`, room for a `basic_ostream`.
    unsafe {
        if most_derived != 0 {
            (*this).bases = &raw const OSTREAM_BASES;
            ios_make(&raw mut (*this).ios);
        }
        (*this).vtordisp = 0;
        let ios = ios_of(this.cast());
        (*ios).base.table = (&raw const OSTREAM_IOS_TABLE.0).cast();
        ios_init(ios, buffer, standard);
    }
    this
}

unsafe extern "efiapi" fn istream_make(
    this: *mut Istream,
    buffer: *mut Streambuf,
    standard: u8,
    most_derived: i32,
) -> *mut Istream {
    // SAFETY: the constructor's `this`, room for a `basic_istream`.
    unsafe {
        if most_derived != 0 {
            (*this).bases = &raw const ISTREAM_BASES;
            ios_make(&raw mut (*this).ios);
        }
        (*this).vtordisp = 0;
        (*this).count = 0;
        let ios = ios_of(this.cast());
        (*ios).base.table = (&raw const ISTREAM_IOS_TABLE.0).cast();
        ios_init(ios, buffer, standard);
    }
    this
}

extern "efiapi" fn stream_unmake(_this: *mut u8) {}

unsafe fn self_cells(this: *mut Streambuf) {
    // SAFETY: the caller's duty: a buffer being made, whose cells point at its own fields.
    unsafe {
        (*this).get_first_at = &raw mut (*this).get_first;
        (*this).put_first_at = &raw mut (*this).put_first;
        (*this).get_next_at = &raw mut (*this).get_next;
        (*this).put_next_at = &raw mut (*this).put_next;
        (*this).get_count_at = &raw mut (*this).get_count;
        (*this).put_count_at = &raw mut (*this).put_count;
        (*this).get_first = null_mut();
        (*this).put_first = null_mut();
        (*this).get_next = null_mut();
        (*this).put_next = null_mut();
        (*this).get_count = 0;
        (*this).put_count = 0;
    }
}

fn new_locale() -> *mut Locale {
    let locale = heap::allocate(core::mem::size_of::<Locale>()).cast::<Locale>();
    if !locale.is_null() {
        // SAFETY: a fresh block sized for a locale.
        unsafe {
            locale.write(Locale {
                implementation: shared_pointer!(LOCIMP),
            })
        };
    }
    locale
}

unsafe extern "efiapi" fn buffer_make(this: *mut Streambuf) -> *mut Streambuf {
    // SAFETY: the constructor's `this`, room for a `basic_streambuf`.
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

unsafe extern "efiapi" fn buffer_unmake(this: *mut Streambuf) {
    // SAFETY: the destructor's `this`; its locale is the one `buffer_make` allocated.
    unsafe {
        heap::free((*this).locale.cast());
        (*this).locale = null_mut();
    }
}

unsafe extern "efiapi" fn base_destroy(this: *mut Streambuf, flags: u32) -> *mut Streambuf {
    // SAFETY: the scalar deleting destructor's `this`, a buffer the program made.
    unsafe {
        buffer_unmake(this);
        if flags & 1 != 0 {
            heap::free(this.cast());
        }
    }
    this
}

extern "efiapi" fn base_overflow(_this: *mut Streambuf, _character: i32) -> i32 {
    EOF
}

extern "efiapi" fn base_underflow(_this: *mut Streambuf) -> i32 {
    EOF
}

unsafe extern "efiapi" fn base_uflow(this: *mut Streambuf) -> i32 {
    // SAFETY: the member's `this`, a live buffer whose `underflow` fills its get area.
    unsafe {
        if buffer_call(this, SLOT_UNDERFLOW, [0; 3]) as u32 as i32 == EOF {
            return EOF;
        }
        let held = &*this;
        if available(held.get_next_at, held.get_count_at) <= 0 {
            return EOF;
        }
        get_char(this, true)
    }
}

unsafe extern "efiapi" fn base_xsgetn(this: *mut Streambuf, out: *mut u8, count: i64) -> i64 {
    let mut read = 0;
    while read < count {
        // SAFETY: the member's `this`, a live buffer, and the caller's room.
        let byte = unsafe { get_char(this, true) };
        if byte == EOF {
            break;
        }
        // SAFETY: as above.
        unsafe { out.add(read as usize).write(byte as u8) };
        read += 1;
    }
    read
}

unsafe extern "efiapi" fn base_xsputn(this: *mut Streambuf, bytes: *const u8, count: i64) -> i64 {
    let mut written = 0;
    while written < count {
        // SAFETY: the member's `this`, a live buffer, and the caller's bytes.
        let byte = unsafe { bytes.add(written as usize).read() };
        // SAFETY: as above.
        if unsafe { put_char(this, byte) } == EOF {
            break;
        }
        written += 1;
    }
    written
}

extern "efiapi" fn base_sync(_this: *mut Streambuf) -> i32 {
    0
}

unsafe extern "efiapi" fn buffer_sputc(this: *mut Streambuf, byte: u8) -> i32 {
    // SAFETY: the member's `this`, a live buffer.
    unsafe { put_char(this, byte) }
}

unsafe extern "efiapi" fn buffer_sputn(this: *mut Streambuf, bytes: *const u8, count: i64) -> i64 {
    if count <= 0 {
        return 0;
    }
    // SAFETY: as above, and the caller's `count` bytes.
    unsafe { put_bytes(this, core::slice::from_raw_parts(bytes, count as usize)) }
}

unsafe extern "efiapi" fn buffer_sgetc(this: *mut Streambuf) -> i32 {
    // SAFETY: the member's `this`, a live buffer.
    unsafe { get_char(this, false) }
}

unsafe extern "efiapi" fn buffer_sbumpc(this: *mut Streambuf) -> i32 {
    // SAFETY: as above.
    unsafe { get_char(this, true) }
}

unsafe extern "efiapi" fn buffer_snextc(this: *mut Streambuf) -> i32 {
    // SAFETY: as above.
    unsafe {
        match get_char(this, true) {
            EOF => EOF,
            _ => get_char(this, false),
        }
    }
}

unsafe extern "efiapi" fn buffer_sgetn(this: *mut Streambuf, out: *mut u8, count: i64) -> i64 {
    // SAFETY: as above, and the caller's room.
    unsafe { buffer_call(this, SLOT_XSGETN, [out as usize, count as usize, 0]) as i64 }
}

unsafe extern "efiapi" fn buffer_pubsync(this: *mut Streambuf) -> i32 {
    // SAFETY: as above.
    unsafe { sync_buffer(this) }
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
        *(*this).get_count_at = last
            .offset_from(next)
            .clamp(0, i64::from(i32::MAX) as isize) as i32;
    }
}

unsafe extern "efiapi" fn buffer_set_put(this: *mut Streambuf, first: *mut u8, last: *mut u8) {
    // SAFETY: as above.
    unsafe {
        *(*this).put_first_at = first;
        *(*this).put_next_at = first;
        *(*this).put_count_at = last
            .offset_from(first)
            .clamp(0, i64::from(i32::MAX) as isize) as i32;
    }
}

unsafe extern "efiapi" fn buffer_get_next(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe { *(*this).get_next_at }
}

unsafe extern "efiapi" fn buffer_put_next(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe { *(*this).put_next_at }
}

unsafe extern "efiapi" fn buffer_get_first(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe { *(*this).get_first_at }
}

unsafe extern "efiapi" fn buffer_put_first(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe { *(*this).put_first_at }
}

unsafe extern "efiapi" fn buffer_get_end(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        let next = *(*this).get_next_at;
        match next.is_null() {
            true => next,
            false => next.add((*(*this).get_count_at).max(0) as usize),
        }
    }
}

unsafe extern "efiapi" fn buffer_put_end(this: *mut Streambuf) -> *mut u8 {
    // SAFETY: as above.
    unsafe {
        let next = *(*this).put_next_at;
        match next.is_null() {
            true => next,
            false => next.add((*(*this).put_count_at).max(0) as usize),
        }
    }
}

unsafe extern "efiapi" fn buffer_get_bump(this: *mut Streambuf, count: i32) {
    // SAFETY: as above.
    unsafe {
        *(*this).get_next_at = (*(*this).get_next_at).offset(count as isize);
        *(*this).get_count_at -= count;
    }
}

unsafe extern "efiapi" fn buffer_put_bump(this: *mut Streambuf, count: i32) {
    // SAFETY: as above.
    unsafe {
        *(*this).put_next_at = (*(*this).put_next_at).offset(count as isize);
        *(*this).put_count_at -= count;
    }
}

unsafe extern "efiapi" fn buffer_in_available(this: *mut Streambuf) -> i64 {
    // SAFETY: as above.
    unsafe {
        let held = &*this;
        let ready = available(held.get_next_at, held.get_count_at);
        match ready {
            0 => buffer_call(this, 5, [0; 3]) as i64,
            ready => i64::from(ready),
        }
    }
}

unsafe extern "efiapi" fn buffer_init(this: *mut Streambuf) {
    // SAFETY: the member's `this`, a buffer whose cells point at its own fields.
    unsafe { self_cells(this) }
}

unsafe extern "efiapi" fn buffer_locale(this: *mut Streambuf, result: *mut Locale) -> *mut Locale {
    // SAFETY: the member's `this` and the caller's room for the returned locale.
    unsafe {
        let implementation = match (*this).locale.is_null() {
            true => shared_pointer!(LOCIMP),
            false => (*(*this).locale).implementation,
        };
        result.write(Locale { implementation });
    }
    result
}

extern "efiapi" fn facet_destroy(this: *mut u8, _flags: u32) -> *mut u8 {
    this
}

extern "efiapi" fn facet_add_reference(_this: *mut u8) {}

extern "efiapi" fn facet_release(_this: *mut u8) -> *mut u8 {
    null_mut()
}

extern "efiapi" fn ctype_lower(_this: *mut u8, byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

extern "efiapi" fn ctype_upper(_this: *mut u8, byte: u8) -> u8 {
    byte.to_ascii_uppercase()
}

unsafe fn each_byte(first: *mut u8, last: *const u8, change: impl Fn(u8) -> u8) -> *const u8 {
    let mut at = first;
    while at.cast_const() < last {
        // SAFETY: the caller's range of the program's bytes.
        unsafe {
            at.write(change(at.read()));
            at = at.add(1);
        }
    }
    last
}

unsafe extern "efiapi" fn ctype_lower_range(
    _this: *mut u8,
    first: *mut u8,
    last: *const u8,
) -> *const u8 {
    // SAFETY: the program's range.
    unsafe { each_byte(first, last, |byte| byte.to_ascii_lowercase()) }
}

unsafe extern "efiapi" fn ctype_upper_range(
    _this: *mut u8,
    first: *mut u8,
    last: *const u8,
) -> *const u8 {
    // SAFETY: the program's range.
    unsafe { each_byte(first, last, |byte| byte.to_ascii_uppercase()) }
}

extern "efiapi" fn ctype_widen(_this: *mut u8, byte: u8) -> u8 {
    byte
}

extern "efiapi" fn ctype_narrow(_this: *mut u8, byte: u8, _default: u8) -> u8 {
    byte
}

unsafe extern "efiapi" fn ctype_widen_range(
    _this: *mut u8,
    first: *const u8,
    last: *const u8,
    out: *mut u8,
) -> *const u8 {
    // SAFETY: the program's range and room for it.
    unsafe {
        let count = last.offset_from(first).max(0) as usize;
        core::ptr::copy(first, out, count);
    }
    last
}

unsafe extern "efiapi" fn ctype_narrow_range(
    _this: *mut u8,
    first: *const u8,
    last: *const u8,
    _default: u8,
    out: *mut u8,
) -> *const u8 {
    // SAFETY: as above.
    unsafe { ctype_widen_range(null_mut(), first, last, out) }
}

extern "efiapi" fn locale_global() -> *mut Locimp {
    shared_pointer!(LOCIMP)
}

extern "efiapi" fn locale_init(_global: u8) -> *mut Locimp {
    shared_pointer!(LOCIMP)
}

extern "efiapi" fn locale_classic() -> *const Locale {
    shared_pointer!(CLASSIC)
}

unsafe extern "efiapi" fn ctype_category(
    facet: *mut *const CtypeChar,
    _locale: *const Locale,
) -> usize {
    if !facet.is_null() {
        // SAFETY: the caller's slot for the facet.
        unsafe {
            if (*facet).is_null() {
                facet.write(shared_pointer!(CTYPE).cast_const());
            }
        }
    }
    CATEGORY_CTYPE
}

extern "efiapi" fn ctype_classic_table() -> *const i16 {
    (&raw const CLASSES).cast()
}

extern "efiapi" fn register_facet(_facet: *mut u8) {}

extern "efiapi" fn lockit_make(this: *mut u8, _kind: i32) -> *mut u8 {
    this
}

extern "efiapi" fn lockit_unmake(_this: *mut u8) {}

unsafe extern "efiapi" fn locale_id_value(this: *mut LocaleId) -> usize {
    // SAFETY: the member's `this`, a facet's id the program or the layer keeps.
    unsafe {
        if (*this).id == 0 {
            let counter = UnsafeCell::raw_get((&raw const ID_COUNT).cast::<UnsafeCell<i32>>());
            *counter += 1;
            (*this).id = *counter as usize;
        }
        (*this).id
    }
}

fn open_mode(mode: i32) -> Option<&'static [u8]> {
    let binary = mode & OPEN_BINARY != 0;
    let mode = mode & !OPEN_BINARY;
    let reads = mode & OPEN_IN != 0;
    let appends = mode & OPEN_APPEND != 0;
    let truncates = mode & OPEN_TRUNCATE != 0;
    let writes = mode & OPEN_OUT != 0 || appends;
    let text: &'static [u8] = match (reads, writes, appends, truncates) {
        (false, true, false, _) => b"w",
        (false, true, true, false) => b"a",
        (true, false, false, false) => b"r",
        (true, true, false, false) => b"r+",
        (true, true, false, true) => b"w+",
        (true, true, true, false) => b"a+",
        _ => return None,
    };
    let chosen: &'static [u8] = match (text, binary) {
        (b"w", false) => b"w\0",
        (b"w", true) => b"wb\0",
        (b"a", false) => b"a\0",
        (b"a", true) => b"ab\0",
        (b"r", false) => b"r\0",
        (b"r", true) => b"rb\0",
        (b"r+", false) => b"r+\0",
        (b"r+", true) => b"r+b\0",
        (b"w+", false) => b"w+\0",
        (b"w+", true) => b"w+b\0",
        (_, false) => b"a+\0",
        (_, true) => b"a+b\0",
    };
    Some(chosen)
}

unsafe extern "efiapi" fn open_file(path: *const u8, mode: i32, _protection: i32) -> *mut File {
    let Some(mode) = open_mode(mode & !0x4) else {
        return null_mut();
    };
    if path.is_null() {
        return null_mut();
    }
    // SAFETY: `_Fiopen`'s contract: a NUL-terminated path.
    let path = unsafe { core::ffi::CStr::from_ptr(path.cast()) };
    crt::open_file(path.to_bytes_with_nul(), mode)
}

unsafe extern "efiapi" fn open_wide_file(
    path: *const u16,
    mode: i32,
    protection: i32,
) -> *mut File {
    if path.is_null() {
        return null_mut();
    }
    // SAFETY: `_Fiopen`'s contract: a NUL-terminated path.
    let units = unsafe { super::super::file::wide_string(path) };
    let mut bytes = alloc::vec![0u8; crate::text::utf8_length(units) + 1];
    let length = bytes.len() - 1;
    let _ = crate::text::write_utf8(units, &mut bytes[..length]);
    // SAFETY: the path converted, NUL-terminated.
    unsafe { open_file(bytes.as_ptr(), mode, protection) }
}

macro_rules! data {
    ($name:literal, $static:ident) => {
        Export {
            name: $name,
            address: (&raw const $static).cast(),
        }
    };
}

/// The standard objects and the data the headers import.
pub(super) const DATA_EXPORTS: &[Export] = &[
    data!(
        b"?cout@std@@3V?$basic_ostream@DU?$char_traits@D@std@@@1@A",
        COUT
    ),
    data!(
        b"?cerr@std@@3V?$basic_ostream@DU?$char_traits@D@std@@@1@A",
        CERR
    ),
    data!(
        b"?clog@std@@3V?$basic_ostream@DU?$char_traits@D@std@@@1@A",
        CLOG
    ),
    data!(
        b"?cin@std@@3V?$basic_istream@DU?$char_traits@D@std@@@1@A",
        CIN
    ),
    data!(b"?id@?$ctype@D@std@@2V0locale@2@A", CTYPE_ID),
    data!(b"?_Id_cnt@id@locale@std@@0HA", ID_COUNT),
    data!(b"?_Sync@ios_base@std@@0_NA", SYNC_WITH_STDIO),
    data!(b"?_Init_cnt@Init@ios_base@std@@0HA", INIT_COUNT),
    data!(b"?_Index@ios_base@std@@0HA", NEXT_WORD),
    Export {
        name: b"??_7?$basic_streambuf@DU?$char_traits@D@std@@@std@@6B@",
        address: (&raw const BASE_TABLE.0).cast(),
    },
    Export {
        name: b"??_7ios_base@std@@6B@",
        address: (&raw const IOS_TABLE.0).cast(),
    },
    Export {
        name: b"??_7?$basic_ios@DU?$char_traits@D@std@@@std@@6B@",
        address: (&raw const IOS_TABLE.0).cast(),
    },
];

/// The members and functions the headers import.
pub(super) const EXPORTS: &[Export] = exports![
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@_N@Z" => put_bool,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@F@Z" => put_short,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@G@Z" => put_unsigned_short,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@H@Z" => put_int,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@I@Z" => put_unsigned_int,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@J@Z" => put_int,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@K@Z" => put_unsigned_int,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@_J@Z" => put_long_long,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@_K@Z" => put_unsigned_long_long,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@M@Z" => put_float,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@N@Z" => put_double,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@O@Z" => put_long_double,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@PEBX@Z" => put_pointer,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@PEAV?$basic_streambuf@DU?$char_traits@D@std@@@1@@Z" => put_buffer,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@P6AAEAV01@AEAV01@@Z@Z" => put_with_stream,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@P6AAEAV?$basic_ios@DU?$char_traits@D@std@@@1@AEAV21@@Z@Z" => put_with_ios,
    b"??6?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@P6AAEAVios_base@1@AEAV21@@Z@Z" => put_with_ios,
    b"?put@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@D@Z" => put,
    b"?write@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@PEBD_J@Z" => write,
    b"?flush@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@XZ" => flush,
    b"?_Osfx@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAXXZ" => output_suffix,
    b"?osfx@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAXXZ" => output_suffix,
    b"?opfx@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAA_NXZ" => output_prefix,
    b"??0?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAA@PEAV?$basic_streambuf@DU?$char_traits@D@std@@@1@_N@Z" => ostream_make,
    b"??1?$basic_ostream@DU?$char_traits@D@std@@@std@@UEAA@XZ" => stream_unmake,
    b"?endl@std@@YAAEAV?$basic_ostream@DU?$char_traits@D@std@@@1@AEAV21@@Z" => end_line,
    b"?ends@std@@YAAEAV?$basic_ostream@DU?$char_traits@D@std@@@1@AEAV21@@Z" => end_string,
    b"?flush@std@@YAAEAV?$basic_ostream@DU?$char_traits@D@std@@@1@AEAV21@@Z" => flush_manipulator,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEA_N@Z" => get_bool,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAF@Z" => get_short,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAG@Z" => get_unsigned_short,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAH@Z" => get_int,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAI@Z" => get_unsigned_int,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAJ@Z" => get_int,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAK@Z" => get_unsigned_int,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEA_J@Z" => get_long_long,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEA_K@Z" => get_unsigned_long_long,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAM@Z" => get_float,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAN@Z" => get_double,
    b"??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAO@Z" => get_double,
    b"?get@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAHXZ" => get_one,
    b"?peek@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAHXZ" => peek,
    b"?getline@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@PEAD_J@Z" => get_line,
    b"?getline@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@PEAD_JD@Z" => get_line_until,
    b"?read@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@PEAD_J@Z" => read,
    b"?ignore@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@_JH@Z" => ignore,
    b"?gcount@?$basic_istream@DU?$char_traits@D@std@@@std@@QEBA_JXZ" => gcount,
    b"?putback@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@D@Z" => put_back,
    b"?_Ipfx@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAA_N_N@Z" => input_prefix,
    b"?ipfx@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAA_N_N@Z" => input_prefix,
    b"?isfx@?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAXXZ" => input_suffix,
    b"??0?$basic_istream@DU?$char_traits@D@std@@@std@@QEAA@PEAV?$basic_streambuf@DU?$char_traits@D@std@@@1@_N@Z" => istream_make,
    b"??1?$basic_istream@DU?$char_traits@D@std@@@std@@UEAA@XZ" => stream_unmake,
    b"?clear@ios_base@std@@QEAAXH_N@Z" => ios_clear,
    b"?clear@?$basic_ios@DU?$char_traits@D@std@@@std@@QEAAXH_N@Z" => ios_clear,
    b"?setstate@?$basic_ios@DU?$char_traits@D@std@@@std@@QEAAXH_N@Z" => ios_set_state,
    b"?init@?$basic_ios@DU?$char_traits@D@std@@@std@@IEAAXPEAV?$basic_streambuf@DU?$char_traits@D@std@@@2@_N@Z" => ios_init,
    b"??0?$basic_ios@DU?$char_traits@D@std@@@std@@IEAA@XZ" => ios_make,
    b"??1?$basic_ios@DU?$char_traits@D@std@@@std@@UEAA@XZ" => ios_unmake,
    b"?rdbuf@?$basic_ios@DU?$char_traits@D@std@@@std@@QEBAPEAV?$basic_streambuf@DU?$char_traits@D@std@@@2@XZ" => ios_buffer,
    b"?rdbuf@?$basic_ios@DU?$char_traits@D@std@@@std@@QEAAPEAV?$basic_streambuf@DU?$char_traits@D@std@@@2@PEAV32@@Z" => ios_set_buffer,
    b"?tie@?$basic_ios@DU?$char_traits@D@std@@@std@@QEBAPEAV?$basic_ostream@DU?$char_traits@D@std@@@2@XZ" => ios_tie,
    b"?tie@?$basic_ios@DU?$char_traits@D@std@@@std@@QEAAPEAV?$basic_ostream@DU?$char_traits@D@std@@@2@PEAV32@@Z" => ios_set_tie,
    b"?fill@?$basic_ios@DU?$char_traits@D@std@@@std@@QEBADXZ" => ios_fill,
    b"?fill@?$basic_ios@DU?$char_traits@D@std@@@std@@QEAADD@Z" => ios_set_fill,
    b"?widen@?$basic_ios@DU?$char_traits@D@std@@@std@@QEBADD@Z" => ios_widen,
    b"?narrow@?$basic_ios@DU?$char_traits@D@std@@@std@@QEBADDD@Z" => ios_narrow,
    b"?width@ios_base@std@@QEBA_JXZ" => base_width,
    b"?width@ios_base@std@@QEAA_J_J@Z" => base_set_width,
    b"?precision@ios_base@std@@QEBA_JXZ" => base_precision,
    b"?precision@ios_base@std@@QEAA_J_J@Z" => base_set_precision,
    b"?flags@ios_base@std@@QEBAHXZ" => base_flags,
    b"?flags@ios_base@std@@QEAAHH@Z" => base_set_flags,
    b"?setf@ios_base@std@@QEAAHH@Z" => base_set_some,
    b"?setf@ios_base@std@@QEAAHHH@Z" => base_set_masked,
    b"?unsetf@ios_base@std@@QEAAXH@Z" => base_unset,
    b"?rdstate@ios_base@std@@QEBAHXZ" => base_state,
    b"?good@ios_base@std@@QEBA_NXZ" => base_good,
    b"?eof@ios_base@std@@QEBA_NXZ" => base_eof,
    b"?fail@ios_base@std@@QEBA_NXZ" => base_fail,
    b"?bad@ios_base@std@@QEBA_NXZ" => base_bad,
    b"?getloc@ios_base@std@@QEBA?AVlocale@2@XZ" => base_get_locale,
    b"?copyfmt@ios_base@std@@QEAAAEAV12@AEBV12@@Z" => base_copy_format,
    b"??1ios_base@std@@UEAA@XZ" => base_unmake,
    b"?_Ios_base_dtor@ios_base@std@@CAXPEAV12@@Z" => base_unmake,
    b"?xalloc@ios_base@std@@SAHXZ" => base_new_index,
    b"?sync_with_stdio@ios_base@std@@SA_N_N@Z" => sync_with_stdio,
    b"?_Init_ctor@Init@ios_base@std@@CAXPEAV123@@Z" => init_make,
    b"?_Init_dtor@Init@ios_base@std@@CAXPEAV123@@Z" => init_unmake,
    b"??0?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAA@XZ" => buffer_make,
    b"??0?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAA@AEBV01@@Z" => buffer_copy,
    b"??1?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAA@XZ" => buffer_unmake,
    b"?_Lock@?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAAXXZ" => nothing_on_buffer,
    b"?_Unlock@?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAAXXZ" => nothing_on_buffer,
    b"?overflow@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHH@Z" => base_overflow,
    b"?pbackfail@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHH@Z" => base_overflow,
    b"?showmanyc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JXZ" => zero_available,
    b"?underflow@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHXZ" => base_underflow,
    b"?uflow@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHXZ" => base_uflow,
    b"?xsgetn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JPEAD_J@Z" => base_xsgetn,
    b"?xsputn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JPEBD_J@Z" => base_xsputn,
    b"?seekoff@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA?AV?$fpos@U_Mbstatet@@@2@_JHH@Z" => no_seek_offset,
    b"?seekpos@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA?AV?$fpos@U_Mbstatet@@@2@V32@H@Z" => no_seek_position,
    b"?setbuf@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAPEAV12@PEAD_J@Z" => keep_buffer,
    b"?sync@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHXZ" => base_sync,
    b"?imbue@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAXAEBVlocale@2@@Z" => nothing_on_locale,
    b"?sputc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAAHD@Z" => buffer_sputc,
    b"?sputn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAA_JPEBD_J@Z" => buffer_sputn,
    b"?sgetc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAAHXZ" => buffer_sgetc,
    b"?sbumpc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAAHXZ" => buffer_sbumpc,
    b"?snextc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAAHXZ" => buffer_snextc,
    b"?sgetn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAA_JPEAD_J@Z" => buffer_sgetn,
    b"?pubsync@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAAHXZ" => buffer_pubsync,
    b"?in_avail@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEAA_JXZ" => buffer_in_available,
    b"?setg@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXPEAD00@Z" => buffer_set_get,
    b"?setp@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXPEAD0@Z" => buffer_set_put,
    b"?gptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_get_next,
    b"?pptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_put_next,
    b"?eback@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_get_first,
    b"?pbase@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_put_first,
    b"?egptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_get_end,
    b"?epptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ" => buffer_put_end,
    b"?gbump@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXH@Z" => buffer_get_bump,
    b"?pbump@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXH@Z" => buffer_put_bump,
    b"?_Init@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXXZ" => buffer_init,
    b"?getloc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@QEBA?AVlocale@2@XZ" => buffer_locale,
    b"?_Getgloballocale@locale@std@@CAPEAV_Locimp@12@XZ" => locale_global,
    b"?_Init@locale@std@@CAPEAV_Locimp@12@_N@Z" => locale_init,
    b"?classic@locale@std@@SAAEBV12@XZ" => locale_classic,
    b"??Bid@locale@std@@QEAA_KXZ" => locale_id_value,
    b"?_Getcat@?$ctype@D@std@@SA_KPEAPEBVfacet@locale@2@PEBV42@@Z" => ctype_category,
    b"?classic_table@?$ctype@D@std@@KAPEBFXZ" => ctype_classic_table,
    b"?_Facet_Register@std@@YAXPEAV_Facet_base@1@@Z" => register_facet,
    b"??0_Lockit@std@@QEAA@H@Z" => lockit_make,
    b"??1_Lockit@std@@QEAA@XZ" => lockit_unmake,
    b"?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z" => open_file,
    b"?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z" => open_wide_file,
];
