//! The standard library's protocols a program conforms its own types to (D180): `Equatable`,
//! `Hashable`, `RawRepresentable`, `CaseIterable`, `_HasCustomAnyHashableRepresentation`,
//! `_SwiftNewtypeWrapper` and `_ObjectiveCBridgeable`, whose requirement descriptors a program's
//! resilient witnesses name, with `Hashable`'s default implementations and the dispatch thunks
//! generic code calls; `Sequence`, `Collection` and `StringProtocol`, whose requirements the
//! runtime does not dispatch, so their witness tables hold only their conformance; and
//! `String` and `Int` as `Equatable` and `Hashable`. The slot of each requirement is the
//! runtime's choice, since these protocols are resilient: requirement `i` fills slot `i + 1`.

use super::conformance_layout::requirement::{
    ASSOCIATED_CONFORMANCE, ASSOCIATED_TYPE, BASE_PROTOCOL, GETTER, INIT, INSTANCE, METHOD,
};
use super::entry::{self, address, call_with_context};
use super::hasher::HasherState;
use super::metadata::{self, Metadata, word};
use super::protocols::{
    ConformanceDescriptor, ProtocolDescriptor, WitnessTable, relative_to, requirement,
};
use super::string::{self, bytes_of};
use super::string_object::RawString;

/// `Equatable`: `static ==`.
pub static EQUATABLE: ProtocolDescriptor<1> = ProtocolDescriptor::new([requirement(METHOD)]);
/// `Hashable`: `Equatable`, `hashValue`, `hash(into:)` and `_rawHashValue(seed:)`.
pub static HASHABLE: ProtocolDescriptor<4> = ProtocolDescriptor::new([
    requirement(BASE_PROTOCOL),
    requirement(GETTER | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
]);
/// `RawRepresentable`: `RawValue`, `init?(rawValue:)` and `rawValue`.
pub static RAW_REPRESENTABLE: ProtocolDescriptor<3> = ProtocolDescriptor::new([
    requirement(ASSOCIATED_TYPE),
    requirement(INIT),
    requirement(GETTER | INSTANCE),
]);
/// `CaseIterable`: `AllCases: Collection`, `AllCases` and `static allCases`.
pub static CASE_ITERABLE: ProtocolDescriptor<3> = ProtocolDescriptor::new([
    requirement(ASSOCIATED_CONFORMANCE),
    requirement(ASSOCIATED_TYPE),
    requirement(GETTER),
]);
/// `_HasCustomAnyHashableRepresentation`: `_toCustomAnyHashable()`.
pub static HAS_CUSTOM_ANY_HASHABLE_REPRESENTATION: ProtocolDescriptor<1> =
    ProtocolDescriptor::new([requirement(METHOD | INSTANCE)]);
/// `_SwiftNewtypeWrapper`: `RawRepresentable` and `_HasCustomAnyHashableRepresentation`.
pub static SWIFT_NEWTYPE_WRAPPER: ProtocolDescriptor<2> =
    ProtocolDescriptor::new([requirement(BASE_PROTOCOL), requirement(BASE_PROTOCOL)]);
/// `_ObjectiveCBridgeable`: `_ObjectiveCType`, `_bridgeToObjectiveC()` and the three static
/// bridges back.
pub static OBJECTIVE_C_BRIDGEABLE: ProtocolDescriptor<5> = ProtocolDescriptor::new([
    requirement(ASSOCIATED_TYPE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD),
    requirement(METHOD),
    requirement(METHOD),
]);
pub static SEQUENCE: ProtocolDescriptor<0> = ProtocolDescriptor::new([]);
pub static COLLECTION: ProtocolDescriptor<0> = ProtocolDescriptor::new([]);
pub static STRING_PROTOCOL: ProtocolDescriptor<0> = ProtocolDescriptor::new([]);

const HASH_INTO_SLOT: usize = 3;
const RAW_HASH_VALUE_SLOT: usize = 4;

const fn conformance_only_table(conformance: &'static ConformanceDescriptor) -> WitnessTable<0> {
    WitnessTable {
        conformance,
        witnesses: [],
    }
}

type EqualWitness<T> = extern "C" fn(*const T, *const T) -> bool;

static ARRAY_SEQUENCE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
static ARRAY_SEQUENCE: WitnessTable<0> = conformance_only_table(&ARRAY_SEQUENCE_CONFORMANCE);
static ARRAY_COLLECTION_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
static ARRAY_COLLECTION: WitnessTable<0> = conformance_only_table(&ARRAY_COLLECTION_CONFORMANCE);
static STRING_STRING_PROTOCOL_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
static STRING_STRING_PROTOCOL: WitnessTable<0> =
    conformance_only_table(&STRING_STRING_PROTOCOL_CONFORMANCE);

static STRING_EQUATABLE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
/// `String: Equatable`.
pub static STRING_EQUATABLE: EquatableTable<RawString> = EquatableTable {
    conformance: &STRING_EQUATABLE_CONFORMANCE,
    equal: string_equal,
};
static STRING_HASHABLE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
/// `String: Hashable`.
pub static STRING_HASHABLE: HashableTable<RawString> = HashableTable {
    conformance: &STRING_HASHABLE_CONFORMANCE,
    equatable: &STRING_EQUATABLE,
    witnesses: [
        entry::eggos_swift_string_hash_value,
        entry::eggos_swift_string_hash_into,
        entry::eggos_swift_string_raw_hash,
    ],
};
static INT_EQUATABLE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
/// `Int: Equatable`.
pub static INT_EQUATABLE: EquatableTable<i64> = EquatableTable {
    conformance: &INT_EQUATABLE_CONFORMANCE,
    equal: int_equal,
};
static INT_HASHABLE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
/// `Int: Hashable`.
pub static INT_HASHABLE: HashableTable<i64> = HashableTable {
    conformance: &INT_HASHABLE_CONFORMANCE,
    equatable: &INT_EQUATABLE,
    witnesses: [
        entry::eggos_swift_int_hash_value,
        entry::eggos_swift_int_hash_into,
        entry::eggos_swift_int_raw_hash,
    ],
};

/// An `Equatable` witness table of values of `T`: the conformance, then `==`, which takes the
/// two values by address.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct EquatableTable<T: 'static> {
    pub conformance: &'static ConformanceDescriptor,
    pub equal: EqualWitness<T>,
}

/// A `Hashable` witness table: the conformance, the `Equatable` table (a base protocol's table
/// sits in its slot, which generic code reads directly), then three witnesses.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct HashableTable<T: 'static> {
    pub conformance: &'static ConformanceDescriptor,
    pub equatable: &'static EquatableTable<T>,
    pub witnesses: [unsafe extern "C" fn(); 3],
}

/// Fills the descriptors' relative pointers and `Hashable`'s default implementations; once,
/// before the program runs.
pub fn set_up() {
    EQUATABLE.fill(b"Equatable\0");
    HASHABLE.fill(b"Hashable\0");
    RAW_REPRESENTABLE.fill(b"RawRepresentable\0");
    CASE_ITERABLE.fill(b"CaseIterable\0");
    HAS_CUSTOM_ANY_HASHABLE_REPRESENTATION.fill(b"_HasCustomAnyHashableRepresentation\0");
    SWIFT_NEWTYPE_WRAPPER.fill(b"_SwiftNewtypeWrapper\0");
    OBJECTIVE_C_BRIDGEABLE.fill(b"_ObjectiveCBridgeable\0");
    SEQUENCE.fill(b"Sequence\0");
    COLLECTION.fill(b"Collection\0");
    STRING_PROTOCOL.fill(b"StringProtocol\0");
    relative_to(
        &HASHABLE.requirements[1].default_implementation,
        address(entry::eggos_swift_default_hash_value) as usize,
    );
    relative_to(
        &HASHABLE.requirements[3].default_implementation,
        address(entry::eggos_swift_default_raw_hash_value) as usize,
    );
    let conformances = [
        (
            &ARRAY_SEQUENCE_CONFORMANCE,
            &raw const SEQUENCE as usize,
            &metadata::ARRAY_DESCRIPTOR,
            &raw const ARRAY_SEQUENCE as usize,
        ),
        (
            &ARRAY_COLLECTION_CONFORMANCE,
            &raw const COLLECTION as usize,
            &metadata::ARRAY_DESCRIPTOR,
            &raw const ARRAY_COLLECTION as usize,
        ),
        (
            &STRING_STRING_PROTOCOL_CONFORMANCE,
            &raw const STRING_PROTOCOL as usize,
            &metadata::STRING_DESCRIPTOR,
            &raw const STRING_STRING_PROTOCOL as usize,
        ),
        (
            &STRING_EQUATABLE_CONFORMANCE,
            &raw const EQUATABLE as usize,
            &metadata::STRING_DESCRIPTOR,
            &raw const STRING_EQUATABLE as usize,
        ),
        (
            &STRING_HASHABLE_CONFORMANCE,
            &raw const HASHABLE as usize,
            &metadata::STRING_DESCRIPTOR,
            &raw const STRING_HASHABLE as usize,
        ),
        (
            &INT_EQUATABLE_CONFORMANCE,
            &raw const EQUATABLE as usize,
            &metadata::INT_DESCRIPTOR,
            &raw const INT_EQUATABLE as usize,
        ),
        (
            &INT_HASHABLE_CONFORMANCE,
            &raw const HASHABLE as usize,
            &metadata::INT_DESCRIPTOR,
            &raw const INT_HASHABLE as usize,
        ),
    ];
    for (conformance, protocol, conforming, table) in conformances {
        conformance.fill(protocol, conforming, table);
    }
}

fn new_hasher(seed: isize) -> HasherState {
    let mut state = core::mem::MaybeUninit::<HasherState>::uninit();
    string::hasher_init(state.as_mut_ptr(), seed);
    // SAFETY: `hasher_init` wrote a whole state.
    unsafe { state.assume_init() }
}

/// `Hashable._rawHashValue(seed:)`'s default: a hasher with the seed, fed by the conformance's
/// `hash(into:)`.
pub(super) extern "C" fn default_raw_hash_value(
    seed: isize,
    conforming: Metadata,
    table: usize,
    value: usize,
) -> isize {
    let mut hasher = new_hasher(seed);
    let hash_into = word(table + 8 * HASH_INTO_SLOT);
    let arguments = [&raw mut hasher as usize, conforming, table, 0];
    // SAFETY: the conformance's `hash(into:)` witness: the hasher in the first argument
    // register, the value as its context, then its type and witness table.
    unsafe { call_with_context(hash_into, value, arguments) };
    hasher.finalize() as isize
}

/// `Hashable.hashValue`'s default: `_rawHashValue(seed: 0)` through the conformance.
pub(super) extern "C" fn default_hash_value(
    conforming: Metadata,
    table: usize,
    value: usize,
) -> isize {
    let raw_hash = word(table + 8 * RAW_HASH_VALUE_SLOT);
    // SAFETY: the conformance's `_rawHashValue(seed:)` witness: the seed, the value as its
    // context, then its type and witness table; it returns the hash.
    (unsafe { call_with_context(raw_hash, value, [0, conforming, table, 0]) }) as isize
}

fn read_string(value: *const RawString) -> RawString {
    // SAFETY: a witness's `self` or argument: the address of an initialized `String`.
    unsafe { value.read() }
}

fn read_int(value: *const i64) -> i64 {
    // SAFETY: as in `read_string`, an `Int`.
    unsafe { value.read() }
}

extern "C" fn string_equal(left: *const RawString, right: *const RawString) -> bool {
    let (mut left_scratch, mut right_scratch) = ([0u8; 16], [0u8; 16]);
    let left = bytes_of(read_string(left), &mut left_scratch);
    left == bytes_of(read_string(right), &mut right_scratch)
}

fn hash_string(state: &mut HasherState, value: RawString) {
    let mut scratch = [0u8; 16];
    state.combine_bytes(bytes_of(value, &mut scratch));
    state.combine_bytes(&[0xff]);
}

/// A `String`'s hash with `seed`, as `String.hash(into:)` feeds a hasher.
pub fn string_hash(value: RawString, seed: isize) -> isize {
    let mut hasher = new_hasher(seed);
    hash_string(&mut hasher, value);
    hasher.finalize() as isize
}

pub(super) extern "C" fn string_hash_value(value: *const RawString) -> isize {
    string_hash(read_string(value), 0)
}

pub(super) extern "C" fn string_hash_into(hasher: *mut HasherState, value: *const RawString) {
    // SAFETY: Swift hands over its inout `Hasher` for the call.
    hash_string(unsafe { &mut *hasher }, read_string(value));
}

pub(super) extern "C" fn string_raw_hash(seed: isize, value: *const RawString) -> isize {
    string_hash(read_string(value), seed)
}

extern "C" fn int_equal(left: *const i64, right: *const i64) -> bool {
    read_int(left) == read_int(right)
}

fn int_hash(value: i64, seed: isize) -> isize {
    let mut hasher = new_hasher(seed);
    hasher.combine_u64(value as u64);
    hasher.finalize() as isize
}

pub(super) extern "C" fn int_hash_value(value: *const i64) -> isize {
    int_hash(read_int(value), 0)
}

pub(super) extern "C" fn int_hash_into(hasher: *mut HasherState, value: *const i64) {
    // SAFETY: as in `string_hash_into`.
    unsafe { &mut *hasher }.combine_u64(read_int(value) as u64);
}

pub(super) extern "C" fn int_raw_hash(seed: isize, value: *const i64) -> isize {
    int_hash(read_int(value), seed)
}

extern "C" fn newtype_custom_any_hashable() -> ! {
    super::unimplemented(&[b"AnyHashable of an imported Objective-C type's raw value"])
}

fn resolve_requirements(name: &[u8]) -> Option<u64> {
    Some(match name {
        b"_$sSQMp" => &raw const EQUATABLE as u64,
        b"_$sSQ2eeoiySbx_xtFZTq" => EQUATABLE.requirement(0),
        b"_$sSHMp" => &raw const HASHABLE as u64,
        b"_$sSHTL" => HASHABLE.requirements_base(),
        b"_$sSHSQTb" => HASHABLE.requirement(0),
        b"_$sSH9hashValueSivgTq" => HASHABLE.requirement(1),
        b"_$sSH4hash4intoys6HasherVz_tFTq" => HASHABLE.requirement(2),
        b"_$sSH13_rawHashValue4seedS2i_tFTq" => HASHABLE.requirement(3),
        b"_$sSYMp" => &raw const RAW_REPRESENTABLE as u64,
        b"_$sSYTL" => RAW_REPRESENTABLE.requirements_base(),
        b"_$s8RawValueSYTl" => RAW_REPRESENTABLE.requirement(0),
        b"_$sSY8rawValuexSg03RawB0Qz_tcfCTq" => RAW_REPRESENTABLE.requirement(1),
        b"_$sSY8rawValue03RawB0QzvgTq" => RAW_REPRESENTABLE.requirement(2),
        b"_$ss12CaseIterableMp" => &raw const CASE_ITERABLE as u64,
        b"_$ss12CaseIterableTL" => CASE_ITERABLE.requirements_base(),
        b"_$ss12CaseIterableP8AllCasesAB_SlTn" => CASE_ITERABLE.requirement(0),
        b"_$s8AllCasess12CaseIterablePTl" => CASE_ITERABLE.requirement(1),
        b"_$ss12CaseIterableP8allCases03AllD0QzvgZTq" => CASE_ITERABLE.requirement(2),
        b"_$ss35_HasCustomAnyHashableRepresentationMp" => {
            &raw const HAS_CUSTOM_ANY_HASHABLE_REPRESENTATION as u64
        }
        b"_$ss35_HasCustomAnyHashableRepresentationP03_tobcD0s0cD0VSgyFTq" => {
            HAS_CUSTOM_ANY_HASHABLE_REPRESENTATION.requirement(0)
        }
        b"_$ss20_SwiftNewtypeWrapperMp" => &raw const SWIFT_NEWTYPE_WRAPPER as u64,
        b"_$ss20_SwiftNewtypeWrapperPSYTb" => SWIFT_NEWTYPE_WRAPPER.requirement(0),
        b"_$ss20_SwiftNewtypeWrapperPs35_HasCustomAnyHashableRepresentationTb" => {
            SWIFT_NEWTYPE_WRAPPER.requirement(1)
        }
        b"_$ss21_ObjectiveCBridgeableMp" => &raw const OBJECTIVE_C_BRIDGEABLE as u64,
        b"_$ss21_ObjectiveCBridgeableTL" => OBJECTIVE_C_BRIDGEABLE.requirements_base(),
        b"_$s15_ObjectiveCTypes01_A11CBridgeablePTl" => OBJECTIVE_C_BRIDGEABLE.requirement(0),
        b"_$ss21_ObjectiveCBridgeableP09_bridgeToA1C01_A5CTypeQzyFTq" => {
            OBJECTIVE_C_BRIDGEABLE.requirement(1)
        }
        b"_$ss21_ObjectiveCBridgeableP016_forceBridgeFromA1C_6resulty01_A5CTypeQz_xSgztFZTq" => {
            OBJECTIVE_C_BRIDGEABLE.requirement(2)
        }
        b"_$ss21_ObjectiveCBridgeableP024_conditionallyBridgeFromA1C_6resultSb01_A5CTypeQz_xSgztFZTq" => {
            OBJECTIVE_C_BRIDGEABLE.requirement(3)
        }
        b"_$ss21_ObjectiveCBridgeableP026_unconditionallyBridgeFromA1Cyx01_A5CTypeQzSgFZTq" => {
            OBJECTIVE_C_BRIDGEABLE.requirement(4)
        }
        b"_$sSTMp" => &raw const SEQUENCE as u64,
        b"_$sSlMp" => &raw const COLLECTION as u64,
        b"_$sSyMp" => &raw const STRING_PROTOCOL as u64,
        _ => return None,
    })
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    if let Some(found) = resolve_requirements(name) {
        return Some(found);
    }
    let found = match name {
        b"_$sSayxGSTsMc" => &raw const ARRAY_SEQUENCE_CONFORMANCE as u64,
        b"_$sSayxGSlsMc" => &raw const ARRAY_COLLECTION_CONFORMANCE as u64,
        b"_$sSSSysMc" => &raw const STRING_STRING_PROTOCOL_CONFORMANCE as u64,
        b"_$sSSSQsMc" => &raw const STRING_EQUATABLE_CONFORMANCE as u64,
        b"_$sSSSHsMc" => &raw const STRING_HASHABLE_CONFORMANCE as u64,
        b"_$sSiSQsMc" => &raw const INT_EQUATABLE_CONFORMANCE as u64,
        b"_$sSiSHsMc" => &raw const INT_HASHABLE_CONFORMANCE as u64,
        b"_$sSSSQsWP" => &raw const STRING_EQUATABLE as u64,
        b"_$sSSSHsWP" => &raw const STRING_HASHABLE as u64,
        b"_$sSiSQsWP" => &raw const INT_EQUATABLE as u64,
        b"_$sSiSHsWP" => &raw const INT_HASHABLE as u64,
        b"_$sSS9hashValueSivg" => string_hash_value_of as *const () as u64,
        b"_$sSQ2eeoiySbx_xtFZTj" => address(entry::eggos_swift_equal_dispatch),
        b"_$sSH9hashValueSivgTj" => address(entry::eggos_swift_hash_value_dispatch),
        b"_$sSH4hash4intoys6HasherVz_tFTj" => address(entry::eggos_swift_hash_into_dispatch),
        b"_$sSH13_rawHashValue4seedS2i_tFTj" => address(entry::eggos_swift_raw_hash_dispatch),
        b"_$sSY8rawValuexSg03RawB0Qz_tcfCTj" => address(entry::eggos_swift_raw_value_init_dispatch),
        b"_$sSY8rawValue03RawB0QzvgTj" => address(entry::eggos_swift_raw_value_dispatch),
        b"_$ss12CaseIterableP8allCases03AllD0QzvgZTj" => {
            address(entry::eggos_swift_all_cases_dispatch)
        }
        b"_$ss20_SwiftNewtypeWrapperPsSHRzSH8RawValueSYRpzrlE20_toCustomAnyHashables0hI0VSgyF" => {
            newtype_custom_any_hashable as *const () as u64
        }
        _ => return None,
    };
    Some(found)
}

extern "C" fn string_hash_value_of(count_and_flags: u64, object: u64) -> isize {
    string_hash(
        RawString {
            count_and_flags,
            object,
        },
        0,
    )
}
