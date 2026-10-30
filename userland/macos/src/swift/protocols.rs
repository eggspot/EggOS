//! The standard library's protocols the runtime implements for its own types (D180):
//! `CustomStringConvertible` (`description` of the integers, `Double`, `Bool` and `String`) and
//! `TextOutputStream` (`String` and `DefaultStringInterpolation`, which string interpolation
//! appends to), with their descriptors, conformance descriptors and witness tables, and the
//! dispatch thunks programs call a requirement through. These protocols are resilient, so a
//! program never indexes their witness tables itself: the slot of each requirement is the
//! runtime's, and the thunks know it. Also `Double.write(to:)`.

use super::entry::{self, address};
use super::metadata::{self, Metadata, TypeDescriptor, word};
use super::number_text;
use super::string::{self, append_bytes};
use super::string_object::RawString;
use core::sync::atomic::{AtomicI32, Ordering};

/// A protocol requirement: its kind and its default implementation (none here).
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct Requirement {
    flags: u32,
    default_implementation: i32,
}

const INSTANCE: u32 = 0x10;
const METHOD: u32 = 1;
const GETTER: u32 = 3;
const PROTOCOL_FLAGS: u32 = metadata::context::PROTOCOL | 0x40 | (1 << 16) | (1 << 17);

/// A protocol's context descriptor with its requirements.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct ProtocolDescriptor<const N: usize> {
    flags: u32,
    parent: AtomicI32,
    name: AtomicI32,
    requirements_in_signature: u32,
    requirement_count: u32,
    associated_type_names: i32,
    requirements: [Requirement; N],
}

impl<const N: usize> ProtocolDescriptor<N> {
    const fn new(requirements: [Requirement; N]) -> Self {
        Self {
            flags: PROTOCOL_FLAGS,
            parent: AtomicI32::new(0),
            name: AtomicI32::new(0),
            requirements_in_signature: 0,
            requirement_count: N as u32,
            associated_type_names: 0,
            requirements,
        }
    }

    fn fill(&self, name: &'static [u8]) {
        relative_to(&self.parent, &raw const metadata::SWIFT_MODULE as usize);
        relative_to(&self.name, name.as_ptr() as usize);
    }
}

fn relative_to(field: &AtomicI32, target: usize) {
    let at = field as *const AtomicI32 as usize;
    field.store(target.wrapping_sub(at) as i32, Ordering::Relaxed);
}

/// `CustomStringConvertible`: `description`'s getter.
pub static CUSTOM_STRING_CONVERTIBLE: ProtocolDescriptor<1> =
    ProtocolDescriptor::new([Requirement {
        flags: GETTER | INSTANCE,
        default_implementation: 0,
    }]);

const fn method() -> Requirement {
    Requirement {
        flags: METHOD | INSTANCE,
        default_implementation: 0,
    }
}

/// `TextOutputStream`: `_lock()`, `_unlock()`, `write(_:)` and `_writeASCII(_:)`, slots 1 to 4.
pub static TEXT_OUTPUT_STREAM: ProtocolDescriptor<4> =
    ProtocolDescriptor::new([method(), method(), method(), method()]);

/// A protocol conformance descriptor: the protocol, the conforming type's descriptor and the
/// witness table, relative.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct ConformanceDescriptor {
    protocol: AtomicI32,
    conforming_type: AtomicI32,
    witness_table: AtomicI32,
    flags: u32,
}

impl ConformanceDescriptor {
    const fn new() -> Self {
        Self {
            protocol: AtomicI32::new(0),
            conforming_type: AtomicI32::new(0),
            witness_table: AtomicI32::new(0),
            flags: 0,
        }
    }

    fn fill(&self, protocol: usize, conforming: &'static TypeDescriptor, table: usize) {
        relative_to(&self.protocol, protocol);
        relative_to(&self.conforming_type, conforming.address());
        relative_to(&self.witness_table, table);
    }
}

/// A witness table: its conformance, then one function per requirement.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct WitnessTable<const N: usize> {
    pub conformance: &'static ConformanceDescriptor,
    pub witnesses: [unsafe extern "C" fn(); N],
}

extern "C" fn nothing() {}

macro_rules! descriptions {
    ($($table:ident, $conformance:ident, $descriptor:ident, $entry:ident, $symbol:literal;)*) => {
        $(
            static $conformance: ConformanceDescriptor = ConformanceDescriptor::new();
            pub static $table: WitnessTable<1> = WitnessTable {
                conformance: &$conformance,
                witnesses: [entry::$entry],
            };
        )*

        fn fill_descriptions() {
            $(
                $conformance.fill(
                    &raw const CUSTOM_STRING_CONVERTIBLE as usize,
                    &metadata::$descriptor,
                    &raw const $table as usize,
                );
            )*
        }

        fn resolve_description(name: &[u8]) -> Option<u64> {
            $(
                if name == $symbol {
                    return Some(&raw const $table as u64);
                }
            )*
            None
        }
    };
}

descriptions! {
    INT_DESCRIPTION, INT_DESCRIPTION_CONFORMANCE, INT_DESCRIPTOR, eggos_swift_int_description,
        b"_$sSis23CustomStringConvertiblesWP";
    INT8_DESCRIPTION, INT8_DESCRIPTION_CONFORMANCE, INT8_DESCRIPTOR, eggos_swift_int8_description,
        b"_$ss4Int8Vs23CustomStringConvertiblesWP";
    INT16_DESCRIPTION, INT16_DESCRIPTION_CONFORMANCE, INT16_DESCRIPTOR,
        eggos_swift_int16_description, b"_$ss5Int16Vs23CustomStringConvertiblesWP";
    INT32_DESCRIPTION, INT32_DESCRIPTION_CONFORMANCE, INT32_DESCRIPTOR,
        eggos_swift_int32_description, b"_$ss5Int32Vs23CustomStringConvertiblesWP";
    INT64_DESCRIPTION, INT64_DESCRIPTION_CONFORMANCE, INT64_DESCRIPTOR,
        eggos_swift_int_description, b"_$ss5Int64Vs23CustomStringConvertiblesWP";
    UINT_DESCRIPTION, UINT_DESCRIPTION_CONFORMANCE, UINT_DESCRIPTOR, eggos_swift_uint_description,
        b"_$sSus23CustomStringConvertiblesWP";
    UINT8_DESCRIPTION, UINT8_DESCRIPTION_CONFORMANCE, UINT8_DESCRIPTOR,
        eggos_swift_uint8_description, b"_$ss5UInt8Vs23CustomStringConvertiblesWP";
    UINT16_DESCRIPTION, UINT16_DESCRIPTION_CONFORMANCE, UINT16_DESCRIPTOR,
        eggos_swift_uint16_description, b"_$ss6UInt16Vs23CustomStringConvertiblesWP";
    UINT32_DESCRIPTION, UINT32_DESCRIPTION_CONFORMANCE, UINT32_DESCRIPTOR,
        eggos_swift_uint32_description, b"_$ss6UInt32Vs23CustomStringConvertiblesWP";
    UINT64_DESCRIPTION, UINT64_DESCRIPTION_CONFORMANCE, UINT64_DESCRIPTOR,
        eggos_swift_uint_description, b"_$ss6UInt64Vs23CustomStringConvertiblesWP";
    DOUBLE_DESCRIPTION, DOUBLE_DESCRIPTION_CONFORMANCE, DOUBLE_DESCRIPTOR,
        eggos_swift_double_description, b"_$sSds23CustomStringConvertiblesWP";
    BOOL_DESCRIPTION, BOOL_DESCRIPTION_CONFORMANCE, BOOL_DESCRIPTOR,
        eggos_swift_bool_description, b"_$sSbs23CustomStringConvertiblesWP";
    STRING_DESCRIPTION, STRING_DESCRIPTION_CONFORMANCE, STRING_DESCRIPTOR,
        eggos_swift_string_description, b"_$sSSs23CustomStringConvertiblesWP";
}

static INTERPOLATION_STREAM_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
static STRING_STREAM_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();

/// `DefaultStringInterpolation: TextOutputStream`: appends to the interpolation's string, its
/// only field.
pub static INTERPOLATION_STREAM: WitnessTable<4> = WitnessTable {
    conformance: &INTERPOLATION_STREAM_CONFORMANCE,
    witnesses: [
        nothing,
        nothing,
        entry::eggos_swift_string_append,
        entry::eggos_swift_write_ascii,
    ],
};

/// `String: TextOutputStream`.
pub static STRING_STREAM: WitnessTable<4> = WitnessTable {
    conformance: &STRING_STREAM_CONFORMANCE,
    witnesses: [
        nothing,
        nothing,
        entry::eggos_swift_string_append,
        entry::eggos_swift_write_ascii,
    ],
};

const WRITE_SLOT: usize = 3;

/// Fills the descriptors' relative pointers; once, before the program runs.
pub fn set_up() {
    CUSTOM_STRING_CONVERTIBLE.fill(b"CustomStringConvertible\0");
    TEXT_OUTPUT_STREAM.fill(b"TextOutputStream\0");
    fill_descriptions();
    INTERPOLATION_STREAM_CONFORMANCE.fill(
        &raw const TEXT_OUTPUT_STREAM as usize,
        &metadata::DEFAULT_STRING_INTERPOLATION_DESCRIPTOR,
        &raw const INTERPOLATION_STREAM as usize,
    );
    STRING_STREAM_CONFORMANCE.fill(
        &raw const TEXT_OUTPUT_STREAM as usize,
        &metadata::STRING_DESCRIPTOR,
        &raw const STRING_STREAM as usize,
    );
}

fn read<T: Copy>(value: *const T) -> T {
    // SAFETY: a witness's `self`: the address of an initialized value of the conforming type.
    unsafe { value.read_unaligned() }
}

fn text(bytes: &[u8]) -> RawString {
    string::make(bytes)
}

pub(super) extern "C" fn int_description(value: *const i64) -> RawString {
    text(number_text::signed(read(value)).as_bytes())
}

pub(super) extern "C" fn int8_description(value: *const i8) -> RawString {
    text(number_text::signed(i64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn int16_description(value: *const i16) -> RawString {
    text(number_text::signed(i64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn int32_description(value: *const i32) -> RawString {
    text(number_text::signed(i64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn uint_description(value: *const u64) -> RawString {
    text(number_text::unsigned(read(value)).as_bytes())
}

pub(super) extern "C" fn uint8_description(value: *const u8) -> RawString {
    text(number_text::unsigned(u64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn uint16_description(value: *const u16) -> RawString {
    text(number_text::unsigned(u64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn uint32_description(value: *const u32) -> RawString {
    text(number_text::unsigned(u64::from(read(value))).as_bytes())
}

pub(super) extern "C" fn double_description(value: *const f64) -> RawString {
    text(number_text::double(read(value)).as_bytes())
}

pub(super) extern "C" fn bool_description(value: *const u8) -> RawString {
    text(if read(value) & 1 != 0 {
        b"true"
    } else {
        b"false"
    })
}

pub(super) extern "C" fn string_description(value: *const RawString) -> RawString {
    string::retained(read(value))
}

/// Writes `bytes` to the `TextOutputStream` at `target` of type `stream` with the conformance
/// `table`.
pub fn write_to_stream(target: *mut u8, bytes: &[u8], stream: Metadata, table: usize) {
    let ours = [
        &raw const INTERPOLATION_STREAM as usize,
        &raw const STRING_STREAM as usize,
    ];
    if ours.contains(&table) {
        append_bytes(target.cast(), bytes, super::string_object::is_ascii(bytes));
        return;
    }
    let value = string::make(bytes);
    let write = word(table + 8 * WRITE_SLOT);
    let arguments = [
        value.count_and_flags as usize,
        value.object as usize,
        stream,
        table,
    ];
    // SAFETY: the conformance's `write(_:)` witness: the string in the first two argument
    // registers, the stream as its context, then its type and witness table.
    unsafe { entry::call_with_context(write, target as usize, arguments) };
    super::heap_object::swift_bridge_object_release(value.object as usize);
}

extern "C" fn double_write(target: *mut u8, value: f64, stream: Metadata, table: usize) {
    write_to_stream(target, number_text::double(value).as_bytes(), stream, table);
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    if let Some(found) = resolve_description(name) {
        return Some(found);
    }
    let found = match name {
        b"_$ss23CustomStringConvertibleMp" => &raw const CUSTOM_STRING_CONVERTIBLE as u64,
        b"_$ss16TextOutputStreamMp" => &raw const TEXT_OUTPUT_STREAM as u64,
        b"_$ss26DefaultStringInterpolationVs16TextOutputStreamsWP" => {
            &raw const INTERPOLATION_STREAM as u64
        }
        b"_$sSSs16TextOutputStreamsWP" => &raw const STRING_STREAM as u64,
        b"_$ss23CustomStringConvertibleP11descriptionSSvgTj" => {
            address(entry::eggos_swift_description_dispatch)
        }
        b"_$ss16TextOutputStreamP5_lockyyFTj" => address(entry::eggos_swift_lock_dispatch),
        b"_$ss16TextOutputStreamP7_unlockyyFTj" => address(entry::eggos_swift_unlock_dispatch),
        b"_$ss16TextOutputStreamP5writeyySSFTj" => address(entry::eggos_swift_write_dispatch),
        b"_$ss16TextOutputStreamP11_writeASCIIyySRys5UInt8VGFTj" => {
            address(entry::eggos_swift_write_ascii_dispatch)
        }
        b"_$sSd5write2toyxz_ts16TextOutputStreamRzlF" => double_write as *const () as u64,
        _ => return None,
    };
    Some(found)
}
