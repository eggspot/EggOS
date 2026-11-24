//! Metadata a program's own types get at run time (D740): `swift_getGenericMetadata` makes a
//! generic type's metadata once per set of key arguments through its descriptor's pattern
//! (`swift_allocateGenericClassMetadata` and `swift_allocateGenericValueMetadata`, then the
//! pattern's completion, which lays out fields through `swift_initClassMetadata2` or
//! `swift_initStructMetadata`); `swift_getSingletonMetadata` finishes a type whose metadata the
//! program emitted incomplete, relocating a class with a resilient superclass
//! (`swift_relocateClassMetadata`) into bounds that follow the superclass's. A finished class
//! gets its superclass from its descriptor's mangled name, inherits its ancestors' generic
//! arguments, vtable entries and field offsets, fills its own vtable and overrides, and is
//! entered in the Objective-C runtime under its Swift name, with any categories that waited
//! on its resilient class stub.

use super::cast::is_swift_class;
use super::descriptor_layout::{self, Flags, Layout};
use super::metadata::{
    Metadata, NATIVE_OBJECT_WITNESSES, UNKNOWN_OBJECT_WITNESSES, ValueWitnessTable, kind,
    relative, set_word, value_flags, word, word32,
};
use super::type_lookup::{Response, get_type_by_mangled_name};
use super::{swift_object, type_name};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::ClassObject;
use crate::objc::buffer::Buffer;

const MOST_ARGUMENTS: usize = 16;
const MOST_COMPLETION_ROUNDS: usize = 8;
const MOST_FIELDS: usize = 256;
const SWIFT_STABLE_BIT: usize = 2;
const USES_SWIFT_REFCOUNTING: u32 = 2;
const HAS_STATIC_VTABLE: usize = 0x100;
const EXTRA_DATA_PATTERN: u32 = 1;
const IMMEDIATE_MEMBERS_PATTERN: u32 = 1 << 31;
const VALUE_KIND_SHIFT: u32 = 21;
const VALUE_KIND_MASK: u32 = 0x7ff;
const ROOT_NEGATIVE_WORDS: usize = 2;
const ROOT_POSITIVE_WORDS: usize = 10;
const LARGEST_INLINE: usize = 24;
const LARGEST_INLINE_ALIGNMENT_MASK: usize = 7;

const CLASS_SUPERCLASS: usize = 8;
const CLASS_CACHE: usize = 16;
const CLASS_DATA: usize = 32;
const CLASS_FLAGS: usize = 40;
const CLASS_SIZE: usize = 56;
const CLASS_ADDRESS_POINT: usize = 60;
const CLASS_DESCRIPTION: usize = 64;
const CLASS_IVAR_DESTROYER: usize = 72;
const READ_ONLY_NAME: usize = 24;

const DESCRIPTOR_SUPERCLASS: usize = 20;
const DESCRIPTOR_NEGATIVE: usize = 24;
const DESCRIPTOR_POSITIVE: usize = 28;
const DESCRIPTOR_IMMEDIATE: usize = 32;
const DESCRIPTOR_FIELD_COUNT: usize = 36;
const DESCRIPTOR_FIELD_OFFSETS: usize = 40;

const VALUE_GENERIC_ARGUMENTS: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Instance {
    descriptor: usize,
    arguments: [usize; MOST_ARGUMENTS],
    count: usize,
    metadata: Metadata,
}

static INSTANCES: Exclusive<Buffer<Instance>> = Exclusive::new(Buffer::new());

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Dependency {
    metadata: Metadata,
    state: usize,
}

type Instantiation = extern "C" fn(usize, *const usize, usize) -> Metadata;
type Completion = extern "C" fn(Metadata, *mut [usize; 4], usize) -> Dependency;
type Relocation = extern "C" fn(usize, usize) -> Metadata;
type Accessor = extern "C" fn(usize) -> Response;

fn write32(address: usize, value: u32) {
    // SAFETY: a field of metadata the runtime is making or finishing, which it owns.
    unsafe { (address as *mut u32).write_unaligned(value) };
}

fn nullable(address: usize) -> usize {
    match word32(address) {
        0 => 0,
        _ => relative(address),
    }
}

fn indirectable(address: usize) -> usize {
    let value = word32(address) as i32;
    if value == 0 {
        return 0;
    }
    let target = address.wrapping_add_signed((value & !1) as isize);
    match value & 1 {
        0 => target,
        _ => word(target),
    }
}

fn allocate_zeroed(bytes: usize) -> Option<usize> {
    let block = heap::allocate(bytes.max(8), true) as usize;
    (block != 0).then_some(block)
}

fn out_of_memory() -> ! {
    super::fatal_error(&[b"out of memory for Swift metadata"])
}

fn layout_of(descriptor: usize) -> Layout {
    descriptor_layout::layout(|offset| word32(descriptor + offset)).unwrap_or_default()
}

fn key_argument_count(descriptor: usize, layout: &Layout) -> usize {
    layout
        .generic_header
        .map_or(0, |header| (word32(descriptor + header + 12) & 0xffff) as usize)
}

fn copy_words(destination: usize, source: usize, count: usize) {
    for index in 0..count {
        set_word(destination + 8 * index, word(source + 8 * index));
    }
}

fn copy_partial_pattern(section: usize, partial: usize) {
    let source = relative(partial);
    let offset = (word32(partial + 4) & 0xffff) as usize;
    let size = (word32(partial + 4) >> 16) as usize;
    copy_words(section + 8 * offset, source, size);
}

fn partial_pattern_end(partial: usize) -> usize {
    let words = word32(partial + 4);
    ((words & 0xffff) + (words >> 16)) as usize
}

fn find(descriptor: usize, arguments: &[usize]) -> Option<Metadata> {
    INSTANCES.with(|instances| {
        instances
            .as_slice()
            .iter()
            .find(|instance| {
                instance.descriptor == descriptor
                    && instance.arguments[..instance.count] == *arguments
            })
            .map(|instance| instance.metadata)
    })
}

fn remember(descriptor: usize, arguments: &[usize], metadata: Metadata) -> Metadata {
    if let Some(found) = find(descriptor, arguments) {
        return found;
    }
    let mut instance = Instance {
        descriptor,
        arguments: [0; MOST_ARGUMENTS],
        count: arguments.len(),
        metadata,
    };
    instance.arguments[..arguments.len()].copy_from_slice(arguments);
    if !INSTANCES.with(|instances| instances.push(instance)) {
        out_of_memory();
    }
    metadata
}

fn complete(metadata: Metadata, completion: usize, pattern: usize) {
    if completion == 0 {
        return;
    }
    let completion = crate::callback::native_callable(completion);
    // SAFETY: the program's completion function: metadata, a four-word context, the pattern.
    let completion: Completion = unsafe { core::mem::transmute(completion) };
    let mut context = [0usize; 4];
    for _ in 0..MOST_COMPLETION_ROUNDS {
        if completion(metadata, &mut context, pattern).metadata == 0 {
            break;
        }
    }
}

/// The metadata of the program's generic type `descriptor` bound to `arguments` (its key
/// arguments: generic parameters, then witness tables), made once and kept.
pub fn instantiate(descriptor: usize, arguments: &[usize]) -> Option<Metadata> {
    if arguments.len() > MOST_ARGUMENTS {
        return None;
    }
    if let Some(found) = find(descriptor, arguments) {
        return Some(found);
    }
    let header = descriptor + layout_of(descriptor).generic_header?;
    let pattern = nullable(header + 4);
    let instantiation = nullable(pattern);
    if pattern == 0 || instantiation == 0 {
        return None;
    }
    let instantiation = crate::callback::native_callable(instantiation);
    // SAFETY: the pattern's instantiation function: descriptor, key arguments, pattern.
    let instantiation: Instantiation = unsafe { core::mem::transmute(instantiation) };
    let made = instantiation(descriptor, arguments.as_ptr(), pattern);
    if made == 0 {
        return None;
    }
    let kept = remember(descriptor, arguments, made);
    if kept == made {
        complete(made, nullable(pattern + 4), pattern);
    }
    Some(kept)
}

/// The metadata of generic type `descriptor` bound to `arguments` from a mangled name, when
/// the arguments are all its key arguments (no witness tables are needed).
pub fn instantiate_bound(descriptor: usize, arguments: &[usize]) -> Option<Metadata> {
    let layout = layout_of(descriptor);
    (key_argument_count(descriptor, &layout) == arguments.len())
        .then(|| instantiate(descriptor, arguments))
        .flatten()
}

extern "C" fn get_generic_metadata(
    _: usize,
    arguments: *const usize,
    descriptor: usize,
) -> Response {
    let count = key_argument_count(descriptor, &layout_of(descriptor));
    let arguments = match count {
        0 => &[][..],
        // SAFETY: the program's key arguments for the descriptor, `count` words.
        _ => unsafe { core::slice::from_raw_parts(arguments, count) },
    };
    let metadata = instantiate(descriptor, arguments).unwrap_or_else(|| {
        super::unimplemented(&[b"generic metadata with more than 16 key arguments"])
    });
    Response { metadata, state: 0 }
}

struct Bounds {
    negative: usize,
    positive: usize,
    members: isize,
}

fn superclass_bounds(descriptor: usize, layout: &Layout) -> (usize, usize) {
    let Some(at) = layout.resilient_superclass else {
        return (ROOT_NEGATIVE_WORDS, ROOT_POSITIVE_WORDS);
    };
    let target = match Flags(word32(descriptor)).resilient_superclass_kind() {
        0 => nullable(descriptor + at),
        1 => word(nullable(descriptor + at)),
        _ => 0,
    };
    let accessor = match target {
        0 => 0,
        _ => nullable(target + 12),
    };
    if accessor == 0 {
        return (ROOT_NEGATIVE_WORDS, ROOT_POSITIVE_WORDS);
    }
    let accessor = crate::callback::native_callable(accessor);
    // SAFETY: the superclass descriptor's metadata accessor, which takes the request.
    let accessor: Accessor = unsafe { core::mem::transmute(accessor) };
    let superclass = accessor(0).metadata;
    let address_point = word32(superclass + CLASS_ADDRESS_POINT) as usize;
    let size = word32(superclass + CLASS_SIZE) as usize;
    (address_point / 8, size.saturating_sub(address_point) / 8)
}

fn class_bounds(descriptor: usize, layout: &Layout) -> Bounds {
    let flags = Flags(word32(descriptor));
    let immediate = word32(descriptor + DESCRIPTOR_IMMEDIATE) as usize;
    if !flags.has_resilient_superclass() {
        let negative = word32(descriptor + DESCRIPTOR_NEGATIVE) as usize;
        let positive = word32(descriptor + DESCRIPTOR_POSITIVE) as usize;
        let members = match flags.immediate_members_negative() {
            true => -(negative as isize) * 8,
            false => (positive - immediate.min(positive)) as isize * 8,
        };
        return Bounds {
            negative,
            positive,
            members,
        };
    }
    let (mut negative, mut positive) = superclass_bounds(descriptor, layout);
    let members = match flags.immediate_members_negative() {
        true => {
            negative += immediate;
            -(negative as isize) * 8
        }
        false => {
            let start = positive as isize * 8;
            positive += immediate;
            start
        }
    };
    let stored = nullable(descriptor + DESCRIPTOR_NEGATIVE);
    if stored != 0 {
        set_word(stored, members as usize);
        write32(stored + 8, negative as u32);
        write32(stored + 12, positive as u32);
    }
    Bounds {
        negative,
        positive,
        members,
    }
}

fn members_start(descriptor: usize, layout: &Layout) -> isize {
    let flags = Flags(word32(descriptor));
    if flags.has_resilient_superclass() {
        let stored = nullable(descriptor + DESCRIPTOR_NEGATIVE);
        if stored != 0 && word(stored) != 0 {
            return word(stored) as isize;
        }
    }
    class_bounds(descriptor, layout).members
}

fn class_members_offset(descriptor: usize, layout: &Layout, words: usize) -> isize {
    let flags = Flags(word32(descriptor));
    match flags.has_resilient_superclass() {
        true => members_start(descriptor, layout) + words as isize * 8,
        false => words as isize * 8,
    }
}

fn generic_arguments_at(metadata: Metadata, descriptor: usize, layout: &Layout) -> usize {
    match Flags(word32(descriptor)).kind() {
        descriptor_layout::CLASS => {
            metadata.wrapping_add_signed(members_start(descriptor, layout))
        }
        _ => metadata + VALUE_GENERIC_ARGUMENTS,
    }
}

fn vtable_at(metadata: Metadata, descriptor: usize, layout: &Layout) -> Option<(usize, usize)> {
    let vtable = descriptor + layout.vtable?;
    let offset = class_members_offset(descriptor, layout, word32(vtable) as usize);
    Some((metadata.wrapping_add_signed(offset), word32(vtable + 4) as usize))
}

fn field_offsets_at(metadata: Metadata, descriptor: usize, layout: &Layout) -> usize {
    let words = word32(descriptor + DESCRIPTOR_FIELD_OFFSETS) as usize;
    metadata.wrapping_add_signed(class_members_offset(descriptor, layout, words))
}

fn class_witnesses(flags: u32) -> usize {
    match flags & USES_SWIFT_REFCOUNTING {
        0 => &raw const UNKNOWN_OBJECT_WITNESSES as usize,
        _ => &raw const NATIVE_OBJECT_WITNESSES as usize,
    }
}

fn empty_cache() -> usize {
    &raw const crate::objc::cache::EMPTY as usize
}

extern "C" fn allocate_generic_class_metadata(
    descriptor: usize,
    arguments: *const usize,
    pattern: usize,
) -> Metadata {
    let layout = layout_of(descriptor);
    let bounds = class_bounds(descriptor, &layout);
    let pattern_flags = word32(pattern + 8);
    let trailing = pattern + 32;
    let extra = (pattern_flags & EXTRA_DATA_PATTERN != 0).then_some(trailing);
    let members = (pattern_flags & IMMEDIATE_MEMBERS_PATTERN != 0)
        .then_some(trailing + 8 * usize::from(extra.is_some()));
    let extra_words = extra.map_or(0, partial_pattern_end);
    let total = (bounds.negative + bounds.positive + extra_words) * 8;
    let block = allocate_zeroed(total).unwrap_or_else(|| out_of_memory());
    let metadata = block + bounds.negative * 8;
    let extra_data = metadata + bounds.positive * 8;
    if let Some(partial) = extra {
        copy_partial_pattern(extra_data, partial);
    }
    if let Some(partial) = members {
        copy_partial_pattern(metadata.wrapping_add_signed(bounds.members), partial);
    }
    let class_flags = word32(pattern + 20);
    set_word(metadata - 16, nullable(pattern + 12));
    set_word(metadata - 8, class_witnesses(class_flags));
    let metaclass = extra_data + 8 * (word32(pattern + 24) >> 16) as usize;
    let metaclass_data = extra_data + 8 * (word32(pattern + 28) & 0xffff) as usize;
    set_word(metaclass + CLASS_DATA, metaclass_data);
    set_word(metadata, metaclass);
    if word32(descriptor + DESCRIPTOR_SUPERCLASS) == 0 {
        set_word(metadata + CLASS_SUPERCLASS, swift_object::class());
    }
    set_word(metadata + CLASS_CACHE, empty_cache());
    let class_data = extra_data + 8 * (word32(pattern + 24) & 0xffff) as usize;
    set_word(metadata + CLASS_DATA, class_data | SWIFT_STABLE_BIT);
    write32(metadata + CLASS_FLAGS, class_flags);
    write32(metadata + CLASS_SIZE, total as u32);
    write32(metadata + CLASS_ADDRESS_POINT, (bounds.negative * 8) as u32);
    set_word(metadata + CLASS_DESCRIPTION, descriptor);
    set_word(metadata + CLASS_IVAR_DESTROYER, nullable(pattern + 16));
    let count = key_argument_count(descriptor, &layout);
    copy_words(generic_arguments_at(metadata, descriptor, &layout), arguments as usize, count);
    metadata
}

extern "C" fn relocate_class_metadata(descriptor: usize, pattern: usize) -> Metadata {
    let layout = layout_of(descriptor);
    let bounds = class_bounds(descriptor, &layout);
    let total = (bounds.negative + bounds.positive) * 8;
    let block = allocate_zeroed(total).unwrap_or_else(|| out_of_memory());
    let metadata = block + bounds.negative * 8;
    let class_flags = word32(pattern + 12);
    set_word(metadata - 16, nullable(pattern + 4));
    set_word(metadata - 8, class_witnesses(class_flags));
    set_word(metadata, nullable(pattern + 20));
    set_word(metadata + CLASS_CACHE, empty_cache());
    set_word(metadata + CLASS_DATA, nullable(pattern + 16) | SWIFT_STABLE_BIT);
    write32(metadata + CLASS_FLAGS, class_flags);
    write32(metadata + CLASS_SIZE, total as u32);
    write32(metadata + CLASS_ADDRESS_POINT, (bounds.negative * 8) as u32);
    set_word(metadata + CLASS_DESCRIPTION, descriptor);
    set_word(metadata + CLASS_IVAR_DESTROYER, nullable(pattern + 8));
    metadata
}

extern "C" fn get_singleton_metadata(_: usize, descriptor: usize) -> Response {
    let layout = layout_of(descriptor);
    let Some(singleton) = layout.singleton.map(|at| descriptor + at) else {
        return Response {
            metadata: 0,
            state: 0,
        };
    };
    let cache = nullable(singleton);
    if cache != 0 && word(cache) != 0 {
        return Response {
            metadata: word(cache),
            state: 0,
        };
    }
    let metadata = match Flags(word32(descriptor)).has_resilient_superclass() {
        true => {
            let pattern = nullable(singleton + 4);
            let relocation = crate::callback::native_callable(nullable(pattern));
            // SAFETY: the resilient pattern's relocation function: descriptor and pattern.
            let relocation: Relocation = unsafe { core::mem::transmute(relocation) };
            relocation(descriptor, pattern)
        }
        false => nullable(singleton + 4),
    };
    if cache != 0 {
        set_word(cache, metadata);
    }
    complete(metadata, nullable(singleton + 8), 0);
    Response { metadata, state: 0 }
}

extern "C" fn check_metadata_state(_: usize, metadata: Metadata) -> Response {
    Response { metadata, state: 0 }
}

extern "C" fn allocate_generic_value_metadata(
    descriptor: usize,
    arguments: *const usize,
    pattern: usize,
    extra_size: usize,
) -> Metadata {
    let block = allocate_zeroed(8 + VALUE_GENERIC_ARGUMENTS + extra_size)
        .unwrap_or_else(|| out_of_memory());
    let metadata = block + 8;
    let pattern_flags = word32(pattern + 8);
    if pattern_flags & EXTRA_DATA_PATTERN != 0 {
        copy_partial_pattern(metadata + VALUE_GENERIC_ARGUMENTS, pattern + 16);
    }
    set_word(block, nullable(pattern + 12));
    set_word(metadata, ((pattern_flags >> VALUE_KIND_SHIFT) & VALUE_KIND_MASK) as usize);
    set_word(metadata + 8, descriptor);
    let count = key_argument_count(descriptor, &layout_of(descriptor));
    copy_words(metadata + VALUE_GENERIC_ARGUMENTS, arguments as usize, count);
    metadata
}

extern "C" fn init_struct_metadata(
    metadata: Metadata,
    _: usize,
    count: usize,
    field_types: *const usize,
    field_offsets: *mut u32,
) {
    let count = count.min(MOST_FIELDS);
    let mut fields = [(0usize, 1usize); MOST_FIELDS];
    let mut flags = 0u32;
    let mut extra_inhabitants = 0u32;
    for (index, field) in fields.iter_mut().enumerate().take(count) {
        // SAFETY: the compiler's array of `count` field type layouts.
        let layout = unsafe { field_types.add(index).read() };
        let field_flags = word32(layout + 16);
        *field = (word(layout), (field_flags & value_flags::ALIGNMENT_MASK) as usize + 1);
        flags |= field_flags & (value_flags::NON_POD | value_flags::NON_BITWISE_TAKABLE);
        extra_inhabitants = extra_inhabitants.max(word32(layout + 20));
    }
    let mut offsets = [0usize; MOST_FIELDS];
    let (size, mask) =
        super::field_layout::lay_out(0, &fields[..count], &mut offsets[..count], 0);
    for (index, &offset) in offsets[..count].iter().enumerate() {
        // SAFETY: the struct's field offset vector, `count` 32-bit words in its metadata.
        unsafe { field_offsets.add(index).write(offset as u32) };
    }
    let inline = size <= LARGEST_INLINE
        && mask <= LARGEST_INLINE_ALIGNMENT_MASK
        && flags & value_flags::NON_BITWISE_TAKABLE == 0;
    if !inline {
        flags |= value_flags::NON_INLINE;
    }
    let pattern = word(metadata - 8) as *const ValueWitnessTable;
    let made = heap::allocate(core::mem::size_of::<ValueWitnessTable>(), false)
        .cast::<ValueWitnessTable>();
    if made.is_null() || pattern.is_null() {
        out_of_memory();
    }
    // SAFETY: the pattern's table, which the program emitted, and a new block of its size.
    unsafe {
        let mut table = pattern.read();
        table.size = size;
        table.stride = size.max(1).next_multiple_of(mask + 1);
        table.flags = flags | mask as u32;
        table.extra_inhabitant_count = extra_inhabitants;
        made.write(table);
    }
    set_word(metadata - 8, made as usize);
}

fn mangled_length(name: usize) -> usize {
    let mut at = 0;
    loop {
        // SAFETY: a mangled name the program emitted, ending in a NUL byte.
        let byte = unsafe { ((name + at) as *const u8).read() };
        at += match byte {
            0 => return at,
            1..=0x17 => 5,
            0x18..=0x1f => 9,
            _ => 1,
        };
    }
}

fn resolve_superclass(metadata: Metadata, descriptor: usize, layout: &Layout) -> usize {
    let name = nullable(descriptor + DESCRIPTOR_SUPERCLASS);
    if name == 0 {
        return swift_object::class();
    }
    let arguments = match Flags(word32(descriptor)).is_generic() {
        true => generic_arguments_at(metadata, descriptor, layout) as *const usize,
        false => core::ptr::null(),
    };
    let length = mangled_length(name);
    let found = get_type_by_mangled_name(name as *const u8, length, descriptor, arguments);
    match super::metadata::kind_of(found) {
        kind::OBJC_CLASS_WRAPPER => word(found + 8),
        _ => found,
    }
}

fn copy_from_ancestors(metadata: Metadata, superclass: usize, static_vtable: bool) {
    let mut ancestor = superclass;
    while ancestor != 0 && is_swift_class(ancestor) {
        let descriptor = word(ancestor + CLASS_DESCRIPTION);
        if descriptor == 0 {
            break;
        }
        let layout = layout_of(descriptor);
        let count = key_argument_count(descriptor, &layout);
        if count != 0 {
            let offset = members_start(descriptor, &layout);
            let to = metadata.wrapping_add_signed(offset);
            copy_words(to, superclass.wrapping_add_signed(offset), count);
        }
        if !static_vtable && let Some((to, size)) = vtable_at(metadata, descriptor, &layout) {
            let from = to - metadata + superclass;
            copy_words(to, from, size);
        }
        let fields = word32(descriptor + DESCRIPTOR_FIELD_COUNT) as usize;
        if fields != 0 {
            let to = field_offsets_at(metadata, descriptor, &layout);
            copy_words(to, to - metadata + superclass, fields);
        }
        ancestor = word(ancestor + CLASS_SUPERCLASS);
    }
}

fn fill_vtable(metadata: Metadata, descriptor: usize, layout: &Layout) {
    let own = (layout.vtable, vtable_at(metadata, descriptor, layout));
    if let (Some(vtable), Some((slots, size))) = own {
        for index in 0..size {
            let method = descriptor + vtable + 8 + 8 * index;
            let implementation = nullable(method + 4);
            if implementation != 0 {
                set_word(slots + 8 * index, implementation);
            }
        }
    }
    let Some(table) = layout.override_table.map(|at| descriptor + at) else {
        return;
    };
    for index in 0..word32(table) as usize {
        let entry = table + 4 + 12 * index;
        let (base, method, implementation) =
            (indirectable(entry), indirectable(entry + 4), nullable(entry + 8));
        if base == 0 || method == 0 || implementation == 0 {
            continue;
        }
        let base_layout = layout_of(base);
        let (Some(base_vtable), Some((slots, size))) =
            (base_layout.vtable, vtable_at(metadata, base, &base_layout))
        else {
            continue;
        };
        let first = base + base_vtable + 8;
        let slot = method.wrapping_sub(first) / 8;
        if method >= first && slot < size {
            set_word(slots + 8 * slot, implementation);
        }
    }
}

fn set_up_metaclass(metadata: Metadata, superclass: usize, generic: bool) {
    let metaclass = word(metadata);
    if metaclass == 0 || superclass == 0 {
        return;
    }
    let super_metaclass = word(superclass);
    if word(metaclass) == 0 {
        set_word(metaclass, word(super_metaclass));
    }
    if generic || word(metaclass + CLASS_SUPERCLASS) == 0 {
        set_word(metaclass + CLASS_SUPERCLASS, super_metaclass);
    }
    if word(metaclass + CLASS_CACHE) == 0 {
        set_word(metaclass + CLASS_CACHE, empty_cache());
    }
}

/// Finishes the class `metadata` before its fields are laid out: its superclass, what it
/// inherits from its ancestors, its own vtable and overrides, and its metaclass.
pub fn finish_class(metadata: Metadata, layout_flags: usize) {
    let descriptor = word(metadata + CLASS_DESCRIPTION);
    if descriptor == 0 {
        return;
    }
    let layout = layout_of(descriptor);
    if word(metadata + CLASS_SUPERCLASS) == 0 {
        let superclass = resolve_superclass(metadata, descriptor, &layout);
        set_word(metadata + CLASS_SUPERCLASS, superclass);
    }
    let superclass = word(metadata + CLASS_SUPERCLASS);
    let static_vtable = layout_flags & HAS_STATIC_VTABLE != 0;
    copy_from_ancestors(metadata, superclass, static_vtable);
    if !static_vtable {
        fill_vtable(metadata, descriptor, &layout);
    }
    let generic = Flags(word32(descriptor)).is_generic()
        || (is_swift_class(superclass)
            && word(superclass + CLASS_DESCRIPTION) != 0
            && Flags(word32(word(superclass + CLASS_DESCRIPTION))).is_generic());
    set_up_metaclass(metadata, superclass, generic);
}

fn permanent_name(metadata: Metadata) -> *const u8 {
    let name = type_name::name_of(metadata, true);
    let bytes = name.as_bytes();
    let made = heap::allocate(bytes.len() + 1, false);
    if made.is_null() {
        out_of_memory();
    }
    // SAFETY: a new block of the name's length and its NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), made, bytes.len());
        made.add(bytes.len()).write(0);
    }
    made
}

/// Enters the finished class `metadata` in the Objective-C runtime if it is not there yet (a
/// generic instance, a relocated class), naming it after its Swift type when its data has no
/// name, and attaches the categories waiting on its resilient class stub.
pub fn register_class(metadata: Metadata) {
    let descriptor = word(metadata + CLASS_DESCRIPTION);
    let stub = match descriptor {
        0 => 0,
        _ => layout_of(descriptor)
            .class_stub
            .map_or(0, |at| nullable(descriptor + at)),
    };
    let read_only = word(metadata + CLASS_DATA) & !7;
    let mut name = match read_only {
        0 => 0,
        _ => word(read_only + READ_ONLY_NAME),
    };
    if name == 0 && read_only != 0 && descriptor != 0 {
        name = permanent_name(metadata) as usize;
        set_word(read_only + READ_ONLY_NAME, name);
        let meta_read_only = word(word(metadata) + CLASS_DATA) & !7;
        if meta_read_only != 0 && word(meta_read_only + READ_ONLY_NAME) == 0 {
            set_word(meta_read_only + READ_ONLY_NAME, name);
        }
    }
    let class = metadata as *mut ClassObject;
    if !crate::objc::register_swift_class(class, name as *const u8, stub) {
        out_of_memory();
    }
}

/// The address of the runtime function `name`, if this module has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_swift_getGenericMetadata" => get_generic_metadata as *const () as usize,
        b"_swift_allocateGenericClassMetadata" => {
            allocate_generic_class_metadata as *const () as usize
        }
        b"_swift_allocateGenericValueMetadata" => {
            allocate_generic_value_metadata as *const () as usize
        }
        b"_swift_initStructMetadata" => init_struct_metadata as *const () as usize,
        b"_swift_getSingletonMetadata" => get_singleton_metadata as *const () as usize,
        b"_swift_relocateClassMetadata" => relocate_class_metadata as *const () as usize,
        b"_swift_checkMetadataState" => check_metadata_state as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
