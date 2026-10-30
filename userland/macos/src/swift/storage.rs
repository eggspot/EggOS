//! The storage of Swift's `Array` and `Dictionary` (D180), whose layouts the programs' inlined
//! standard library code reads and writes directly: an array buffer is a
//! `_ContiguousArrayStorage<T>` object with its count and capacity (shifted left by one, the low
//! bit "bridged verbatim") after the header and the elements from offset 32; a native dictionary
//! is a `_DictionaryStorage<K, V>` with its count, capacity, scale, age, seed and key and value
//! pointers, then the bitmap of occupied buckets (the bits past the last bucket set), keys and
//! values. Programs allocate arrays themselves; the runtime allocates dictionaries, destroys
//! both, provides the empty singletons and the array functions the unspecialized code calls.

use super::entry;
use super::generic;
use super::hash_table::{capacity, first_bitmap_word, scale_for};
use super::heap_object::{self, HeapObject};
use super::metadata::{
    self, ClassMetadata, EMPTY_ARRAY_STORAGE_DESCRIPTOR, EMPTY_DICTIONARY_DESCRIPTOR, Metadata,
    ValueWitnessTable, copy_bytes, set_word, witnesses, word,
};
use super::refcount;
use crate::libsystem::heap;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const COUNT: usize = 16;
const CAPACITY: usize = 24;
const ARRAY_HEADER: usize = 32;
const SCALE: usize = 32;
const AGE: usize = 36;
const SEED: usize = 40;
const KEYS: usize = 48;
const VALUES: usize = 56;
const BITMAP: usize = 64;
const STORAGE_POINTER_MASK: usize = !0xf000_0000_0000_0007;
const BRIDGED_STORAGE: usize = 1 << 62;

/// An immortal storage object: its header and up to nine words of body.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct StaticStorage {
    header: HeapObject,
    body: [AtomicUsize; 9],
}

/// `_swiftEmptyArrayStorage`: no elements, capacity 0, bridged verbatim.
pub static EMPTY_ARRAY: StaticStorage = StaticStorage {
    header: HeapObject {
        metadata: AtomicUsize::new(0),
        refcount: AtomicU64::new(refcount::IMMORTAL),
    },
    body: [
        AtomicUsize::new(0),
        AtomicUsize::new(1),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    ],
};

/// `_swiftEmptyDictionarySingleton`: one unoccupied bucket (the bits past it set), keys and
/// values at 1.
pub static EMPTY_DICTIONARY: StaticStorage = StaticStorage {
    header: HeapObject {
        metadata: AtomicUsize::new(0),
        refcount: AtomicU64::new(refcount::IMMORTAL),
    },
    body: [
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(1),
        AtomicUsize::new(1),
        AtomicUsize::new(!1),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    ],
};

static EMPTY_ARRAY_CLASS: ClassMetadata = ClassMetadata::new(ARRAY_HEADER as u32, 0);
static EMPTY_DICTIONARY_CLASS: ClassMetadata = ClassMetadata::new(BITMAP as u32 + 8, 0);

unsafe extern "C" {
    fn eggos_swift_never_destroyed();
}

core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_swift_never_destroyed",
    ".hidden eggos_swift_never_destroyed",
    "eggos_swift_never_destroyed:",
    "ret",
);

/// Wires the empty singletons to their classes; once, before the program runs.
pub fn set_up() {
    let never = entry::address(eggos_swift_never_destroyed);
    EMPTY_ARRAY_CLASS.fill(never, &EMPTY_ARRAY_STORAGE_DESCRIPTOR);
    EMPTY_DICTIONARY_CLASS.fill(never, &EMPTY_DICTIONARY_DESCRIPTOR);
    EMPTY_ARRAY
        .header
        .metadata
        .store(EMPTY_ARRAY_CLASS.address(), Ordering::Relaxed);
    EMPTY_DICTIONARY
        .header
        .metadata
        .store(EMPTY_DICTIONARY_CLASS.address(), Ordering::Relaxed);
}

/// `_ContiguousArrayStorage<element>`.
pub fn contiguous_array_storage(element: Metadata) -> Option<Metadata> {
    generic::storage_class(
        &metadata::CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR,
        ARRAY_HEADER as u32,
        entry::eggos_swift_array_storage_destroy,
        &[element],
    )
}

/// `_DictionaryStorage<key, value>`.
pub fn dictionary_storage(key: Metadata, value: Metadata) -> Option<Metadata> {
    generic::storage_class(
        &metadata::DICTIONARY_STORAGE_DESCRIPTOR,
        BITMAP as u32,
        entry::eggos_swift_dictionary_storage_destroy,
        &[key, value],
    )
}

fn align_up(value: usize, alignment_mask: usize) -> usize {
    (value + alignment_mask) & !alignment_mask
}

/// Where the elements of the array storage `storage` start.
pub fn elements(storage: usize, table: &ValueWitnessTable) -> usize {
    align_up(storage + ARRAY_HEADER, table.alignment_mask())
}

/// The storage object of the array value `array`, if it is native.
pub fn array_storage(array: usize) -> usize {
    if array & BRIDGED_STORAGE != 0 {
        super::unimplemented(&[b"an Array backed by an NSArray"]);
    }
    array & STORAGE_POINTER_MASK
}

/// The count of the array storage `storage`.
pub fn count(storage: usize) -> usize {
    word(storage + COUNT)
}

pub(super) extern "C" fn destroy_array(object: usize) {
    let class = word(object);
    let element = heap_object::class_argument(class, 0);
    let table = witnesses(element);
    if !table.is_pod() {
        let start = elements(object, table);
        for index in 0..count(object) {
            metadata::destroy_value((start + index * table.stride) as *mut u8, element);
        }
    }
    crate::libsystem::heap::release(object as *mut u8);
}

fn bucket_count(storage: usize) -> usize {
    1usize << (word(storage + SCALE) & 0x3f)
}

fn is_occupied(storage: usize, bucket: usize) -> bool {
    word(storage + BITMAP + 8 * (bucket / 64)) & (1usize << (bucket % 64)) != 0
}

pub(super) extern "C" fn destroy_dictionary(object: usize) {
    let class = word(object);
    let (key, value) = (
        heap_object::class_argument(class, 0),
        heap_object::class_argument(class, 1),
    );
    let (key_table, value_table) = (witnesses(key), witnesses(value));
    if !(key_table.is_pod() && value_table.is_pod()) {
        let (keys, values) = (word(object + KEYS), word(object + VALUES));
        for bucket in 0..bucket_count(object) {
            if is_occupied(object, bucket) {
                metadata::destroy_value((keys + bucket * key_table.stride) as *mut u8, key);
                metadata::destroy_value((values + bucket * value_table.stride) as *mut u8, value);
            }
        }
    }
    crate::libsystem::heap::release(object as *mut u8);
}

fn allocate_dictionary(class: Metadata, scale: u8, age: Option<u32>, seed: Option<usize>) -> usize {
    let (key, value) = (
        heap_object::class_argument(class, 0),
        heap_object::class_argument(class, 1),
    );
    let (key_table, value_table) = (witnesses(key), witnesses(value));
    let buckets = 1usize << scale;
    let bitmap_words = buckets.div_ceil(64);
    let keys_at = align_up(BITMAP + 8 * bitmap_words, key_table.alignment_mask());
    let values_at = align_up(
        keys_at + buckets * key_table.stride,
        value_table.alignment_mask(),
    );
    let size = values_at + buckets * value_table.stride;
    let alignment_mask = 7usize
        .max(key_table.alignment_mask())
        .max(value_table.alignment_mask());
    let object = heap_object::swift_alloc_object(class, size, alignment_mask) as usize;
    set_word(object + COUNT, 0);
    set_word(object + CAPACITY, capacity(buckets));
    let age = age.unwrap_or((object >> 4) as u32);
    set_word(object + SCALE, usize::from(scale) | (age as usize) << 32);
    set_word(object + SEED, seed.unwrap_or(object));
    set_word(object + KEYS, object + keys_at);
    set_word(object + VALUES, object + values_at);
    for index in 0..bitmap_words {
        set_word(object + BITMAP + 8 * index, 0);
    }
    set_word(object + BITMAP, first_bitmap_word(buckets));
    object
}

pub(super) extern "C" fn dictionary_allocate(capacity: isize, class: Metadata) -> usize {
    allocate_dictionary(class, scale_for(capacity.max(0) as usize), None, None)
}

pub(super) extern "C" fn dictionary_resize(
    _original: usize,
    capacity: isize,
    _move: usize,
    class: Metadata,
) -> usize {
    allocate_dictionary(class, scale_for(capacity.max(0) as usize), None, None)
}

pub(super) extern "C" fn dictionary_copy(original: usize, class: Metadata) -> usize {
    let scale = (word(original + SCALE) & 0xff) as u8;
    let age = metadata::word32(original + AGE);
    allocate_dictionary(class, scale, Some(age), Some(word(original + SEED)))
}

extern "C" fn allocate_uninitialized_buffer(minimum_capacity: isize, element: Metadata) -> usize {
    if minimum_capacity <= 0 {
        return &raw const EMPTY_ARRAY as usize;
    }
    let class = contiguous_array_storage(element)
        .unwrap_or_else(|| super::fatal_error(&[b"out of memory for Swift metadata"]));
    let table = witnesses(element);
    let stride = table.stride.max(1);
    let header = align_up(ARRAY_HEADER, table.alignment_mask());
    let size = header + stride * minimum_capacity as usize;
    let object = heap_object::swift_alloc_object(class, size, table.alignment_mask().max(7));
    let room = (heap::size_of(object) - header) / stride;
    let verbatim = usize::from(metadata::is_class_type(element));
    set_word(object as usize + COUNT, 0);
    set_word(object as usize + CAPACITY, (room << 1) | verbatim);
    object as usize
}

extern "C" fn array_end_index(array: usize, _element: Metadata) -> isize {
    count(array_storage(array)) as isize
}

pub(super) extern "C" fn array_subscript(
    index: isize,
    array: usize,
    element: Metadata,
    out: *mut u8,
) {
    let storage = array_storage(array);
    if index < 0 || index as usize >= count(storage) {
        super::fatal_error(&[b"Index out of range"]);
    }
    let table = witnesses(element);
    let at = elements(storage, table) + index as usize * table.stride;
    metadata::copy_value(out, at as *mut u8, element);
}

fn each(count: usize, element: Metadata, reverse: bool, mut work: impl FnMut(usize)) {
    let stride = witnesses(element).stride;
    for step in 0..count {
        let index = if reverse { count - 1 - step } else { step };
        work(index * stride);
    }
}

extern "C" fn array_init_with_copy(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    let table = witnesses(element);
    if table.is_pod() {
        copy_bytes(destination, source, count * table.stride);
        return;
    }
    each(count, element, false, |offset| {
        metadata::copy_value(
            destination.wrapping_add(offset),
            source.wrapping_add(offset),
            element,
        )
    });
}

fn init_with_take(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
    reverse: bool,
) {
    let table = witnesses(element);
    if table.is_bitwise_takable() {
        copy_bytes(destination, source, count * table.stride);
        return;
    }
    each(count, element, reverse, |offset| {
        metadata::take_value(
            destination.wrapping_add(offset),
            source.wrapping_add(offset),
            element,
        )
    });
}

extern "C" fn array_init_with_take_front_to_back(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    init_with_take(destination, source, count, element, false);
}

extern "C" fn array_init_with_take_back_to_front(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    init_with_take(destination, source, count, element, true);
}

fn assign(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
    reverse: bool,
    take: bool,
) {
    let table = witnesses(element);
    if table.is_pod() {
        copy_bytes(destination, source, count * table.stride);
        return;
    }
    let witness = if take {
        table.assign_with_take
    } else {
        table.assign_with_copy
    };
    each(count, element, reverse, |offset| {
        // SAFETY: the witness's contract, which the caller of the array function promised for
        // each element.
        unsafe {
            witness(
                destination.wrapping_add(offset),
                source.wrapping_add(offset),
                element,
            )
        };
    });
}

extern "C" fn array_assign_with_copy_front_to_back(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    assign(destination, source, count, element, false, false);
}

extern "C" fn array_assign_with_copy_back_to_front(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    assign(destination, source, count, element, true, false);
}

extern "C" fn array_assign_with_take(
    destination: *mut u8,
    source: *mut u8,
    count: usize,
    element: Metadata,
) {
    assign(destination, source, count, element, false, true);
}

extern "C" fn array_destroy(start: *mut u8, count: usize, element: Metadata) {
    if witnesses(element).is_pod() {
        return;
    }
    each(count, element, false, |offset| {
        metadata::destroy_value(start.wrapping_add(offset), element)
    });
}

extern "C" fn key_type_violates_hashable(_key_type: Metadata) -> ! {
    super::fatal_error(&[
        b"Duplicate keys of a Dictionary's key type were found: its Hashable implementation is \
          not consistent with ==",
    ])
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"__swiftEmptyArrayStorage" => &raw const EMPTY_ARRAY as usize,
        b"__swiftEmptyDictionarySingleton" => &raw const EMPTY_DICTIONARY as usize,
        b"_$sSa8endIndexSivg" | b"_$sSa5countSivg" => array_end_index as *const () as usize,
        b"_$sSa28_allocateBufferUninitialized15minimumCapacitys06_ArrayB0VyxGSi_tFZ" => {
            allocate_uninitialized_buffer as *const () as usize
        }
        b"_$sSayxSicig" => entry::address(entry::eggos_swift_array_subscript) as usize,
        b"_$ss18_DictionaryStorageC8allocate8capacityAByxq_GSi_tFZ" => {
            entry::address(entry::eggos_swift_dictionary_allocate) as usize
        }
        b"_$ss18_DictionaryStorageC6resize8original8capacity4moveAByxq_Gs05__RawaB0C_SiSbtFZ" => {
            entry::address(entry::eggos_swift_dictionary_resize) as usize
        }
        b"_$ss18_DictionaryStorageC4copy8originalAByxq_Gs05__RawaB0C_tFZ" => {
            entry::address(entry::eggos_swift_dictionary_copy) as usize
        }
        b"_$ss53KEY_TYPE_OF_DICTIONARY_VIOLATES_HASHABLE_REQUIREMENTSys5NeverOypXpF" => {
            key_type_violates_hashable as *const () as usize
        }
        b"_swift_arrayInitWithCopy" => array_init_with_copy as *const () as usize,
        b"_swift_arrayInitWithTakeNoAlias" | b"_swift_arrayInitWithTakeFrontToBack" => {
            array_init_with_take_front_to_back as *const () as usize
        }
        b"_swift_arrayInitWithTakeBackToFront" => {
            array_init_with_take_back_to_front as *const () as usize
        }
        b"_swift_arrayAssignWithCopyNoAlias" | b"_swift_arrayAssignWithCopyFrontToBack" => {
            array_assign_with_copy_front_to_back as *const () as usize
        }
        b"_swift_arrayAssignWithCopyBackToFront" => {
            array_assign_with_copy_back_to_front as *const () as usize
        }
        b"_swift_arrayAssignWithTake" => array_assign_with_take as *const () as usize,
        b"_swift_arrayDestroy" => array_destroy as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
