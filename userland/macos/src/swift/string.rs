//! `String` and `Hasher` in the runtime (D180): reading a string's UTF-8 bytes in each of its
//! forms (`string_object.rs`), appending in place to a uniquely referenced native storage with
//! room or into a new one twice as large, `_StringGuts.grow`, hashing a string's bytes and
//! comparing two strings by their UTF-8 bytes (Unicode scalar order; equal strings in the same
//! normalization form compare equal), and the hasher's entry points over `hasher.rs` keyed by a
//! random execution seed (zero when `SWIFT_DETERMINISTIC_HASHING` is set, as on macOS).

use super::hasher::HasherState;
use super::heap_object::{self, header, swift_alloc_object};
use super::metadata::{ClassMetadata, STRING_STORAGE_DESCRIPTOR, copy_bytes, set_word, word};
use super::refcount;
use super::string_object::{
    self, Form, NATIVE_BIAS, RawString, SMALL_CAPACITY, capacity_of, capacity_word, flags, form,
    is_ascii, small, small_bytes,
};
use crate::libsystem::heap;
use core::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

const CAPACITY_FIELD: usize = 16;
const COUNT_FIELD: usize = 24;

static STRING_STORAGE: ClassMetadata = ClassMetadata::new(NATIVE_BIAS as u32, 0);

/// Wires `__StringStorage`'s class; once, before the program runs.
pub fn set_up() {
    STRING_STORAGE.fill(
        super::entry::address(super::entry::eggos_swift_string_storage_destroy),
        &STRING_STORAGE_DESCRIPTOR,
    );
}

pub(super) extern "C" fn destroy_storage(object: usize) {
    heap::release(object as *mut u8);
}

/// The UTF-8 bytes of `raw`, the string kept alive by its owner; a small string's are copied
/// into `scratch`.
pub fn bytes_of(raw: RawString, scratch: &mut [u8; 16]) -> &[u8] {
    match form(raw) {
        Form::Small { length } => {
            *scratch = small_bytes(raw);
            &scratch[..length]
        }
        Form::Contiguous {
            address, length, ..
        } => {
            // SAFETY: a large string's bytes stay mapped while its owner holds it: a literal
            // in the program's text or a storage object it retains.
            unsafe { core::slice::from_raw_parts(address as usize as *const u8, length) }
        }
        Form::Other => super::unimplemented(&[b"Strings bridged from NSString"]),
    }
}

fn new_storage(capacity: usize) -> usize {
    let class = STRING_STORAGE.address();
    let object = swift_alloc_object(class, NATIVE_BIAS as usize + capacity + 1, 7) as usize;
    let real = heap::size_of(object as *mut u8) - NATIVE_BIAS as usize;
    set_word(object + CAPACITY_FIELD, capacity_word(real) as usize);
    object
}

fn storage_capacity(storage: usize) -> usize {
    capacity_of(word(storage + CAPACITY_FIELD) as u64)
}

fn finish_native(storage: usize, length: usize, ascii: bool) -> RawString {
    let raw = string_object::native(storage as u64, length, ascii);
    set_word(storage + COUNT_FIELD, raw.count_and_flags as usize);
    let end = storage + NATIVE_BIAS as usize + length;
    // SAFETY: the storage has room for its capacity and the NUL after it.
    unsafe { (end as *mut u8).write(0) };
    raw
}

/// A new string of `bytes`: small if they fit, else in a new native storage.
pub fn make(bytes: &[u8]) -> RawString {
    if let Some(raw) = small(bytes) {
        return raw;
    }
    let storage = new_storage(bytes.len());
    copy_bytes(
        (storage + NATIVE_BIAS as usize) as *mut u8,
        bytes.as_ptr(),
        bytes.len(),
    );
    finish_native(storage, bytes.len(), is_ascii(bytes))
}

fn is_ascii_string(raw: RawString, bytes: &[u8]) -> bool {
    match form(raw) {
        Form::Small { .. } => is_ascii(bytes),
        _ => raw.count_and_flags & flags::ASCII != 0,
    }
}

fn unique_native(raw: RawString) -> Option<usize> {
    let Form::Contiguous { native: true, .. } = form(raw) else {
        return None;
    };
    let storage = (raw.object & string_object::ADDRESS_MASK) as usize;
    let bits = header(storage as *mut u8).refcount.load(Ordering::Acquire);
    refcount::is_unique(bits).then_some(storage)
}

fn read_target(target: *mut RawString) -> RawString {
    // SAFETY: Swift hands the runtime an inout `String`: initialized and exclusively its own
    // for the call.
    unsafe { target.read() }
}

fn write_target(target: *mut RawString, value: RawString) {
    // SAFETY: as in `read_target`.
    unsafe { target.write(value) };
}

/// Appends `addition` to the string at `target`, which owns its storage.
pub fn append_bytes(target: *mut RawString, addition: &[u8], addition_ascii: bool) {
    if addition.is_empty() {
        return;
    }
    let current = read_target(target);
    let mut scratch = [0u8; 16];
    let existing = bytes_of(current, &mut scratch);
    let length = existing.len() + addition.len();
    let ascii = is_ascii_string(current, existing) && addition_ascii;
    if let Form::Small { .. } = form(current)
        && length <= SMALL_CAPACITY
    {
        let mut joined = [0u8; SMALL_CAPACITY];
        joined[..existing.len()].copy_from_slice(existing);
        joined[existing.len()..length].copy_from_slice(addition);
        write_target(target, make(&joined[..length]));
        return;
    }
    if let Some(storage) = unique_native(current)
        && storage_capacity(storage) >= length
    {
        let start = storage + NATIVE_BIAS as usize;
        copy_bytes(
            (start + existing.len()) as *mut u8,
            addition.as_ptr(),
            addition.len(),
        );
        write_target(target, finish_native(storage, length, ascii));
        return;
    }
    let old_capacity = unique_native(current).map_or(0, storage_capacity);
    let storage = new_storage(length.max(2 * old_capacity));
    let start = storage + NATIVE_BIAS as usize;
    copy_bytes(start as *mut u8, existing.as_ptr(), existing.len());
    copy_bytes(
        (start + existing.len()) as *mut u8,
        addition.as_ptr(),
        addition.len(),
    );
    write_target(target, finish_native(storage, length, ascii));
    heap_object::swift_bridge_object_release(current.object as usize);
}

/// `String.append(_:)`, and `TextOutputStream.write(_:)` of a string or an interpolation.
pub(super) extern "C" fn string_append(count_and_flags: u64, object: u64, target: *mut RawString) {
    let other = RawString {
        count_and_flags,
        object,
    };
    let current = read_target(target);
    if string_object::length(current) == 0 && unique_native(current).is_none() {
        heap_object::swift_bridge_object_retain(object as usize);
        write_target(target, other);
        heap_object::swift_bridge_object_release(current.object as usize);
        return;
    }
    let mut scratch = [0u8; 16];
    let addition = bytes_of(other, &mut scratch);
    let ascii = is_ascii_string(other, addition);
    append_bytes(target, addition, ascii);
}

/// `TextOutputStream._writeASCII(_:)`.
pub(super) extern "C" fn write_ascii(start: *const u8, count: usize, target: *mut RawString) {
    if start.is_null() || count == 0 {
        return;
    }
    // SAFETY: the caller's buffer of `count` ASCII bytes.
    let bytes = unsafe { core::slice::from_raw_parts(start, count) };
    append_bytes(target, bytes, true);
}

/// `_StringGuts.grow(_:)`: makes the string at `target` native, unique and able to hold
/// `capacity` bytes.
pub(super) extern "C" fn guts_grow(capacity: isize, target: *mut RawString) {
    let capacity = capacity.max(0) as usize;
    let current = read_target(target);
    let old_capacity = unique_native(current).map_or(0, storage_capacity);
    if old_capacity >= capacity && unique_native(current).is_some() {
        return;
    }
    let mut scratch = [0u8; 16];
    let existing = bytes_of(current, &mut scratch);
    let ascii = is_ascii_string(current, existing);
    let storage = new_storage(capacity.max(2 * old_capacity).max(existing.len()));
    copy_bytes(
        (storage + NATIVE_BIAS as usize) as *mut u8,
        existing.as_ptr(),
        existing.len(),
    );
    let length = existing.len();
    write_target(target, finish_native(storage, length, ascii));
    heap_object::swift_bridge_object_release(current.object as usize);
}

extern "C" fn string_hash(hasher: *mut HasherState, count_and_flags: u64, object: u64) {
    let raw = RawString {
        count_and_flags,
        object,
    };
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(raw, &mut scratch);
    // SAFETY: Swift hands over its inout `Hasher`, exclusively for the call.
    let state = unsafe { &mut *hasher };
    state.combine_bytes(bytes);
    state.combine_bytes(&[0xff]);
}

const COMPARE_EQUAL: usize = 0;

extern "C" fn string_compare(
    left_count_and_flags: u64,
    left_object: u64,
    right_count_and_flags: u64,
    right_object: u64,
    expecting: usize,
) -> bool {
    let left = RawString {
        count_and_flags: left_count_and_flags,
        object: left_object,
    };
    let right = RawString {
        count_and_flags: right_count_and_flags,
        object: right_object,
    };
    let (mut left_scratch, mut right_scratch) = ([0u8; 16], [0u8; 16]);
    let left = bytes_of(left, &mut left_scratch);
    let right = bytes_of(right, &mut right_scratch);
    if expecting & 0xff == COMPARE_EQUAL {
        left == right
    } else {
        left < right
    }
}

static EXECUTION_SEED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static SEEDED: AtomicIsize = AtomicIsize::new(0);

fn execution_seed() -> [u64; 2] {
    heap_object::once(&SEEDED, || {
        // SAFETY: a NUL-terminated name.
        let deterministic =
            unsafe { crate::libsystem::process::getenv(c"SWIFT_DETERMINISTIC_HASHING".as_ptr()) };
        if !deterministic.is_null() {
            return;
        }
        let mut bytes = [0u8; 16];
        // SAFETY: a local buffer of 16 bytes.
        unsafe { crate::libsystem::process::arc4random_buf(bytes.as_mut_ptr().cast(), 16) };
        for (index, seed) in EXECUTION_SEED.iter().enumerate() {
            let part = bytes[index * 8..index * 8 + 8].try_into().unwrap_or([0; 8]);
            seed.store(u64::from_le_bytes(part), Ordering::Relaxed);
        }
    });
    EXECUTION_SEED
        .each_ref()
        .map(|seed| seed.load(Ordering::Relaxed))
}

pub(super) extern "C" fn hasher_init(out: *mut HasherState, seed: isize) {
    let state = HasherState::new(execution_seed(), seed as u64);
    // SAFETY: Swift's indirect result: room for a `Hasher`.
    unsafe { out.write(state) };
}

fn state<'a>(hasher: *mut HasherState) -> &'a mut HasherState {
    // SAFETY: Swift hands over its `Hasher` by address, exclusively for the call.
    unsafe { &mut *hasher }
}

pub(super) extern "C" fn hasher_finalize(hasher: *mut HasherState) -> isize {
    state(hasher).finalize() as isize
}

pub(super) extern "C" fn hasher_combine(value: u64, hasher: *mut HasherState) {
    state(hasher).combine_u64(value);
}

pub(super) extern "C" fn hasher_combine8(value: u8, hasher: *mut HasherState) {
    state(hasher).combine_bytes(&[value]);
}

pub(super) extern "C" fn hasher_combine16(value: u16, hasher: *mut HasherState) {
    state(hasher).combine_bytes(&value.to_le_bytes());
}

pub(super) extern "C" fn hasher_combine32(value: u32, hasher: *mut HasherState) {
    state(hasher).combine_bytes(&value.to_le_bytes());
}

/// An owned copy of `raw` (its storage retained).
pub fn retained(raw: RawString) -> RawString {
    heap_object::swift_bridge_object_retain(raw.object as usize);
    raw
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    use super::entry::{self, address};
    let address = match name {
        b"_$sSS6appendyySSF" => address(entry::eggos_swift_string_append),
        b"_$ss11_StringGutsV4growyySiF" => address(entry::eggos_swift_guts_grow),
        b"_$sSS4hash4intoys6HasherVz_tF" => string_hash as *const () as u64,
        b"_$ss27_stringCompareWithSmolCheck__9expectingSbs11_StringGutsV_ADs01_G16ComparisonResultOtF" => {
            string_compare as *const () as u64
        }
        b"_$ss6HasherV5_seedABSi_tcfC" => address(entry::eggos_swift_hasher_init),
        b"_$ss6HasherV9_finalizeSiyF" => address(entry::eggos_swift_hasher_finalize),
        b"_$ss6HasherV8_combineyySuF" | b"_$ss6HasherV8_combineyys6UInt64VF" => {
            address(entry::eggos_swift_hasher_combine)
        }
        b"_$ss6HasherV8_combineyys5UInt8VF" => address(entry::eggos_swift_hasher_combine8),
        b"_$ss6HasherV8_combineyys6UInt16VF" => address(entry::eggos_swift_hasher_combine16),
        b"_$ss6HasherV8_combineyys6UInt32VF" => address(entry::eggos_swift_hasher_combine32),
        _ => return None,
    };
    Some(address)
}
