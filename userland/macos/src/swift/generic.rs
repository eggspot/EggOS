//! Metadata the runtime instantiates for generic standard library types (D180): `Optional<T>`
//! (an enum with its own value witnesses built on `T`'s, extra inhabitants first, then a tag
//! byte), `Array`, `Dictionary` and `Set` (one bridge-object word), the storage classes
//! `_ContiguousArrayStorage<T>` and `_DictionaryStorage<K, V>`, and existentials of a protocol
//! list. Each is made once per set of arguments and kept for the program's life, so the same
//! arguments always give the same metadata, as type identity requires.

use super::entry;
use super::metadata::{
    self, ANY, BRIDGE_OBJECT_WITNESSES, ClassMetadata, EnumValueWitnessTable, Metadata,
    OPAQUE_EXISTENTIAL, TypeDescriptor, ValueWitnessTable, address_of, kind, set_word,
    single_payload_tag, store_single_payload_tag, value_flags, witnesses, word,
};
use crate::libsystem::{Exclusive, heap};
use crate::objc::buffer::Buffer;

#[derive(Debug, Clone, Copy)]
struct Instance {
    key: [usize; 4],
    metadata: Metadata,
}

static INSTANCES: Exclusive<Buffer<Instance>> = Exclusive::new(Buffer::new());

fn instantiate(key: [usize; 4], make: impl FnOnce() -> Option<Metadata>) -> Option<Metadata> {
    INSTANCES.with(|instances| {
        if let Some(found) = instances.as_slice().iter().find(|found| found.key == key) {
            return Some(found.metadata);
        }
        let made = make()?;
        instances
            .push(Instance {
                key,
                metadata: made,
            })
            .then_some(made)
    })
}

fn allocate_words(count: usize) -> Option<usize> {
    let block = heap::allocate(count * 8, true);
    (!block.is_null()).then_some(block as usize)
}

const OPTIONAL_WITNESS_WORDS: usize = core::mem::size_of::<EnumValueWitnessTable>() / 8;
const OPTIONAL_ARGUMENT: usize = 16;

fn wrapped_of(optional: Metadata) -> Metadata {
    word(optional + OPTIONAL_ARGUMENT)
}

struct OptionalLayout {
    payload: Metadata,
    size: usize,
    payload_inhabitants: u32,
}

fn optional_layout(optional: Metadata) -> OptionalLayout {
    let payload = wrapped_of(optional);
    let table = witnesses(payload);
    OptionalLayout {
        payload,
        size: table.size,
        payload_inhabitants: table.extra_inhabitant_count,
    }
}

fn optional_case(value: *const u8, layout: &OptionalLayout) -> u32 {
    let table = witnesses(layout.payload);
    // SAFETY: the payload's own witness, on the optional's storage.
    unsafe { (table.get_enum_tag_single_payload)(value, 1, layout.payload) }
}

fn set_optional_case(value: *mut u8, case: u32, layout: &OptionalLayout) {
    let table = witnesses(layout.payload);
    // SAFETY: as in `optional_case`.
    unsafe { (table.store_enum_tag_single_payload)(value, case, 1, layout.payload) };
}

fn copy_bytes_of(destination: *mut u8, source: *const u8, optional: Metadata) {
    metadata::copy_bytes(destination, source, witnesses(optional).size);
}

unsafe extern "C" fn optional_copy(
    destination: *mut u8,
    source: *mut u8,
    optional: Metadata,
) -> *mut u8 {
    let layout = optional_layout(optional);
    if optional_case(source, &layout) == 0 {
        metadata::copy_value(destination, source, layout.payload);
        set_optional_case(destination, 0, &layout);
    } else {
        copy_bytes_of(destination, source, optional);
    }
    destination
}

unsafe extern "C" fn optional_take(
    destination: *mut u8,
    source: *mut u8,
    optional: Metadata,
) -> *mut u8 {
    let layout = optional_layout(optional);
    if optional_case(source, &layout) == 0 {
        metadata::take_value(destination, source, layout.payload);
        set_optional_case(destination, 0, &layout);
    } else {
        copy_bytes_of(destination, source, optional);
    }
    destination
}

unsafe extern "C" fn optional_destroy(value: *mut u8, optional: Metadata) {
    let layout = optional_layout(optional);
    if optional_case(value, &layout) == 0 {
        metadata::destroy_value(value, layout.payload);
    }
}

unsafe extern "C" fn optional_assign_copy(
    destination: *mut u8,
    source: *mut u8,
    optional: Metadata,
) -> *mut u8 {
    if destination != source {
        // SAFETY: the witnesses' own contracts, passed on.
        unsafe { optional_destroy(destination, optional) };
        // SAFETY: as above.
        unsafe { optional_copy(destination, source, optional) };
    }
    destination
}

unsafe extern "C" fn optional_assign_take(
    destination: *mut u8,
    source: *mut u8,
    optional: Metadata,
) -> *mut u8 {
    // SAFETY: as above.
    unsafe { optional_destroy(destination, optional) };
    // SAFETY: as above.
    unsafe { optional_take(destination, source, optional) }
}

unsafe extern "C" fn optional_buffer_copy(
    destination: *mut u8,
    source: *mut u8,
    optional: Metadata,
) -> *mut u8 {
    let table = witnesses(optional);
    if !table.is_inline() {
        let boxed = word(source as usize);
        super::heap_object::swift_retain(boxed as *mut u8);
        set_word(destination as usize, boxed);
        return metadata::project_box(boxed, table.alignment_mask()) as *mut u8;
    }
    // SAFETY: as above.
    unsafe { optional_copy(destination, source, optional) }
}

fn payload_inhabitant_tag(value: *const u8, layout: &OptionalLayout) -> u32 {
    let table = witnesses(layout.payload);
    // SAFETY: the payload's own witness, asked about all of its extra inhabitants.
    unsafe {
        (table.get_enum_tag_single_payload)(value, layout.payload_inhabitants, layout.payload)
    }
}

unsafe extern "C" fn optional_tag(value: *const u8, empty_cases: u32, optional: Metadata) -> u32 {
    let layout = optional_layout(optional);
    let own = witnesses(optional);
    single_payload_tag(
        value,
        empty_cases,
        own.size,
        own.extra_inhabitant_count,
        |_| match payload_inhabitant_tag(value, &layout) {
            0 | 1 => 0,
            tag => tag - 1,
        },
    )
}

unsafe extern "C" fn optional_store_tag(
    value: *mut u8,
    case: u32,
    empty_cases: u32,
    optional: Metadata,
) {
    let layout = optional_layout(optional);
    let own = witnesses(optional);
    let inhabitants = own.extra_inhabitant_count;
    if case == 0 || case > inhabitants {
        store_single_payload_tag(value, case, empty_cases, own.size, inhabitants, |_, _| {});
        return;
    }
    store_single_payload_tag(value, 0, empty_cases, own.size, inhabitants, |_, _| {});
    let table = witnesses(layout.payload);
    // SAFETY: the payload's own witness: the optional's extra inhabitant `case - 1` is the
    // payload's inhabitant `case`.
    unsafe {
        (table.store_enum_tag_single_payload)(
            value,
            case + 1,
            layout.payload_inhabitants,
            layout.payload,
        )
    };
}

unsafe extern "C" fn optional_enum_tag(value: *const u8, optional: Metadata) -> u32 {
    let layout = optional_layout(optional);
    u32::from(optional_case(value, &layout) != 0)
}

unsafe extern "C" fn optional_project(_: *mut u8, _: Metadata) {}

unsafe extern "C" fn optional_inject(value: *mut u8, case: u32, optional: Metadata) {
    let layout = optional_layout(optional);
    set_optional_case(value, case, &layout);
}

fn make_optional(wrapped: Metadata) -> Option<Metadata> {
    let payload = witnesses(wrapped);
    let (size, extra_inhabitants) = if payload.extra_inhabitant_count > 0 {
        (payload.size, payload.extra_inhabitant_count - 1)
    } else {
        (
            payload.size + super::enum_tag::tag_bytes(payload.size, 1, 1),
            0,
        )
    };
    let alignment_mask = payload.alignment_mask();
    let inline = size <= 24 && alignment_mask < 8;
    let flags = (payload.flags
        & (value_flags::ALIGNMENT_MASK | value_flags::NON_POD | value_flags::NON_BITWISE_TAKABLE))
        | value_flags::HAS_ENUM_WITNESSES
        | if inline { 0 } else { value_flags::NON_INLINE };
    let table = EnumValueWitnessTable {
        base: ValueWitnessTable {
            initialize_buffer_with_copy_of_buffer: optional_buffer_copy,
            destroy: optional_destroy,
            initialize_with_copy: optional_copy,
            assign_with_copy: optional_assign_copy,
            initialize_with_take: optional_take,
            assign_with_take: optional_assign_take,
            get_enum_tag_single_payload: optional_tag,
            store_enum_tag_single_payload: optional_store_tag,
            size,
            stride: ((size + alignment_mask) & !alignment_mask).max(1),
            flags,
            extra_inhabitant_count: extra_inhabitants,
        },
        get_enum_tag: optional_enum_tag,
        destructive_project_enum_data: optional_project,
        destructive_inject_enum_tag: optional_inject,
    };
    let block = allocate_words(OPTIONAL_WITNESS_WORDS + 4)?;
    // SAFETY: a new zeroed block of the table's size and the metadata's four words after it.
    unsafe { (block as *mut EnumValueWitnessTable).write(table) };
    let address = block + OPTIONAL_WITNESS_WORDS * 8 + 8;
    set_word(address - 8, block);
    set_word(address, kind::OPTIONAL);
    set_word(address + 8, metadata::OPTIONAL_DESCRIPTOR.address());
    set_word(address + OPTIONAL_ARGUMENT, wrapped);
    Some(address)
}

/// `Optional<wrapped>`.
pub fn optional(wrapped: Metadata) -> Option<Metadata> {
    let key = [metadata::OPTIONAL_DESCRIPTOR.address(), wrapped, 0, 0];
    instantiate(key, || make_optional(wrapped))
}

fn make_struct(descriptor: &'static TypeDescriptor, arguments: &[Metadata]) -> Option<Metadata> {
    let block = allocate_words(3 + arguments.len())?;
    let address = block + 8;
    set_word(block, &raw const BRIDGE_OBJECT_WITNESSES as usize);
    set_word(address, kind::STRUCT);
    set_word(address + 8, descriptor.address());
    for (index, &argument) in arguments.iter().enumerate() {
        set_word(address + 16 + 8 * index, argument);
    }
    Some(address)
}

fn key_of(descriptor: &'static TypeDescriptor, arguments: &[Metadata]) -> Option<[usize; 4]> {
    let mut key = [descriptor.address(), 0, 0, 0];
    for (slot, &argument) in key[1..].iter_mut().zip(arguments) {
        *slot = argument;
    }
    (arguments.len() <= 3).then_some(key)
}

/// `Array<element>`, `Dictionary<key, value>` or `Set<element>`, by their descriptor.
pub fn collection(descriptor: &'static TypeDescriptor, arguments: &[Metadata]) -> Option<Metadata> {
    instantiate(key_of(descriptor, arguments)?, || {
        make_struct(descriptor, arguments)
    })
}

/// The instance of the storage class `descriptor` (`_ContiguousArrayStorage` or
/// `_DictionaryStorage`) for `arguments`, destroyed by `destroy`.
pub fn storage_class(
    descriptor: &'static TypeDescriptor,
    instance_size: u32,
    destroy: unsafe extern "C" fn(),
    arguments: &[Metadata],
) -> Option<Metadata> {
    instantiate(key_of(descriptor, arguments)?, || {
        let words = core::mem::size_of::<ClassMetadata>() / 8 + arguments.len();
        let block = allocate_words(words)?;
        let class = ClassMetadata::new(instance_size, arguments.len() as u32);
        // SAFETY: a new zeroed block of the metadata's size and its arguments.
        unsafe { (block as *mut ClassMetadata).write(class) };
        // SAFETY: as above, now initialized; never freed.
        let class = unsafe { &*(block as *const ClassMetadata) };
        class.fill(entry::address(destroy), descriptor);
        let address = class.address();
        for (index, &argument) in arguments.iter().enumerate() {
            set_word(address + metadata::CLASS_ARGUMENTS + 8 * index, argument);
        }
        Some(address)
    })
}

/// The existential of `protocols` (their descriptors), none of them class-bound: `Any` for
/// none.
pub fn existential(protocols: &[usize]) -> Option<Metadata> {
    if protocols.is_empty() {
        return Some(address_of(&ANY));
    }
    let mut key = [kind::EXISTENTIAL, 0, 0, 0];
    for (slot, &protocol) in key[1..].iter_mut().zip(protocols) {
        *slot = protocol;
    }
    if protocols.len() > 3 {
        return None;
    }
    instantiate(key, || {
        let table = metadata::existential_table(protocols.len());
        let table_words = core::mem::size_of::<ValueWitnessTable>() / 8;
        let block = allocate_words(table_words + 3 + protocols.len())?;
        // SAFETY: a new zeroed block of the table's size and the metadata's words.
        unsafe { (block as *mut ValueWitnessTable).write(table) };
        let address = block + table_words * 8 + 8;
        set_word(address - 8, block);
        set_word(address, kind::EXISTENTIAL);
        let flags =
            u64::from(OPAQUE_EXISTENTIAL | protocols.len() as u32) | (protocols.len() as u64) << 32;
        set_word(address + 8, flags as usize);
        for (index, &protocol) in protocols.iter().enumerate() {
            set_word(address + 16 + 8 * index, protocol);
        }
        Some(address)
    })
}
