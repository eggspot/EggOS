//! Type metadata as Swift's ABI lays it out (D180): every type's metadata has its value witness
//! table (size, stride, alignment, extra inhabitants and the functions that copy, move and
//! destroy a value) in the word before it, then its kind (or, for a class, its Objective-C
//! `isa`). The runtime's own: the builtin value witness tables programs link (`$sBi64_WV` and
//! kin), the standard library's struct metadata (`Int`, `String`, ...), the existential `Any`,
//! the class metadata of its storage classes, and the descriptors that name them.

use super::enum_tag;
use super::heap_object;
use super::mangling::Standard;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// A type's metadata, by its address.
pub type Metadata = usize;

/// The word at `address`.
pub fn word(address: usize) -> usize {
    // SAFETY: the runtime only reads words of metadata, descriptors, objects and values it was
    // handed by the program or made itself, all mapped.
    unsafe { (address as *const usize).read() }
}

/// Writes the word at `address`.
pub fn set_word(address: usize, value: usize) {
    // SAFETY: as in `word`, for memory the runtime owns or a Swift function lets it write.
    unsafe { (address as *mut usize).write(value) }
}

/// The 32-bit word at `address`.
pub fn word32(address: usize) -> u32 {
    // SAFETY: as in `word`.
    unsafe { (address as *const u32).read_unaligned() }
}

/// The target of the relative pointer at `address`.
pub fn relative(address: usize) -> usize {
    address.wrapping_add_signed(word32(address) as i32 as isize)
}

/// Copies `length` bytes, which may overlap.
pub fn copy_bytes(destination: *mut u8, source: *const u8, length: usize) {
    // SAFETY: the callers copy values of a type's size between places a witness or the
    // runtime's own storage gives them.
    unsafe { core::ptr::copy(source, destination, length) };
}

/// Metadata kinds.
pub mod kind {
    pub const CLASS: usize = 0;
    pub const STRUCT: usize = 0x200;
    pub const OPTIONAL: usize = 0x202;
    pub const TUPLE: usize = 0x301;
    pub const EXISTENTIAL: usize = 0x303;
    pub const LAST_ENUMERATED: usize = 0x7ff;
}

/// The kind of `metadata`; a class's first word is its `isa`, above every kind.
pub fn kind_of(metadata: Metadata) -> usize {
    let first = word(metadata);
    if first > kind::LAST_ENUMERATED {
        kind::CLASS
    } else {
        first
    }
}

/// Value witness table flags.
pub mod value_flags {
    pub const ALIGNMENT_MASK: u32 = 0xff;
    pub const NON_POD: u32 = 0x1_0000;
    pub const NON_INLINE: u32 = 0x2_0000;
    pub const NON_BITWISE_TAKABLE: u32 = 0x10_0000;
    pub const HAS_ENUM_WITNESSES: u32 = 0x20_0000;
}

pub type CopyWitness = unsafe extern "C" fn(*mut u8, *mut u8, Metadata) -> *mut u8;
pub type DestroyWitness = unsafe extern "C" fn(*mut u8, Metadata);
pub type TagWitness = unsafe extern "C" fn(*const u8, u32, Metadata) -> u32;
pub type StoreTagWitness = unsafe extern "C" fn(*mut u8, u32, u32, Metadata);
pub type EnumTagWitness = unsafe extern "C" fn(*const u8, Metadata) -> u32;
pub type ProjectWitness = unsafe extern "C" fn(*mut u8, Metadata);
pub type InjectWitness = unsafe extern "C" fn(*mut u8, u32, Metadata);

/// A value witness table.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ValueWitnessTable {
    pub initialize_buffer_with_copy_of_buffer: CopyWitness,
    pub destroy: DestroyWitness,
    pub initialize_with_copy: CopyWitness,
    pub assign_with_copy: CopyWitness,
    pub initialize_with_take: CopyWitness,
    pub assign_with_take: CopyWitness,
    pub get_enum_tag_single_payload: TagWitness,
    pub store_enum_tag_single_payload: StoreTagWitness,
    pub size: usize,
    pub stride: usize,
    pub flags: u32,
    pub extra_inhabitant_count: u32,
}

/// An enum's value witness table: the common one and the enum witnesses after it.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct EnumValueWitnessTable {
    pub base: ValueWitnessTable,
    pub get_enum_tag: EnumTagWitness,
    pub destructive_project_enum_data: ProjectWitness,
    pub destructive_inject_enum_tag: InjectWitness,
}

impl ValueWitnessTable {
    pub fn alignment_mask(&self) -> usize {
        (self.flags & value_flags::ALIGNMENT_MASK) as usize
    }

    pub fn is_pod(&self) -> bool {
        self.flags & value_flags::NON_POD == 0
    }

    pub fn is_inline(&self) -> bool {
        self.flags & value_flags::NON_INLINE == 0
    }

    pub fn is_bitwise_takable(&self) -> bool {
        self.flags & value_flags::NON_BITWISE_TAKABLE == 0
    }
}

/// The value witness table of `metadata`.
pub fn witnesses(metadata: Metadata) -> &'static ValueWitnessTable {
    let table = word(metadata.wrapping_sub(8)) as *const ValueWitnessTable;
    // SAFETY: every type metadata keeps a pointer to its permanent table in the word before it.
    unsafe { &*table }
}

/// Where a boxed value lies in its heap box, after the object header.
pub fn project_box(boxed: usize, alignment_mask: usize) -> usize {
    (boxed + 16 + alignment_mask) & !alignment_mask
}

/// Copies a value of `metadata` from `source` into uninitialized `destination`.
pub fn copy_value(destination: *mut u8, source: *mut u8, metadata: Metadata) {
    let table = witnesses(metadata);
    if table.is_pod() {
        copy_bytes(destination, source, table.size);
    } else {
        // SAFETY: the witness's contract, which the callers meet: both point at the type's
        // storage, the source initialized.
        unsafe { (table.initialize_with_copy)(destination, source, metadata) };
    }
}

/// Destroys the value of `metadata` at `value`.
pub fn destroy_value(value: *mut u8, metadata: Metadata) {
    let table = witnesses(metadata);
    if !table.is_pod() {
        // SAFETY: as in `copy_value`, for an initialized value.
        unsafe { (table.destroy)(value, metadata) };
    }
}

/// Moves a value of `metadata` from `source` into uninitialized `destination`.
pub fn take_value(destination: *mut u8, source: *mut u8, metadata: Metadata) {
    let table = witnesses(metadata);
    if table.is_bitwise_takable() {
        copy_bytes(destination, source, table.size);
    } else {
        // SAFETY: as in `copy_value`.
        unsafe { (table.initialize_with_take)(destination, source, metadata) };
    }
}

fn tag_bytes_for(size: usize, empty_cases: u32, extra_inhabitants: u32) -> usize {
    if empty_cases > extra_inhabitants {
        enum_tag::tag_bytes(size, empty_cases - extra_inhabitants, 1)
    } else {
        0
    }
}

/// `getEnumTagSinglePayload` for a payload of `size` bytes with `extra_inhabitants`, read by
/// `inhabitant`.
pub fn single_payload_tag(
    value: *const u8,
    empty_cases: u32,
    size: usize,
    extra_inhabitants: u32,
    inhabitant: impl FnOnce(&[u8]) -> u32,
) -> u32 {
    let length = size + tag_bytes_for(size, empty_cases, extra_inhabitants);
    // SAFETY: the witness's contract: the enum's storage spans the payload and its tag bytes.
    let bytes = unsafe { core::slice::from_raw_parts(value, length) };
    enum_tag::get_tag(bytes, size, empty_cases, extra_inhabitants, inhabitant)
}

/// `storeEnumTagSinglePayload`, as [`single_payload_tag`] reads.
pub fn store_single_payload_tag(
    value: *mut u8,
    case: u32,
    empty_cases: u32,
    size: usize,
    extra_inhabitants: u32,
    store: impl FnOnce(&mut [u8], u32),
) {
    let length = size + tag_bytes_for(size, empty_cases, extra_inhabitants);
    // SAFETY: as in `single_payload_tag`, writable.
    let bytes = unsafe { core::slice::from_raw_parts_mut(value, length) };
    enum_tag::store_tag(bytes, size, case, empty_cases, extra_inhabitants, store);
}

/// The low pointer bits Objective-C reserves for tagged pointers.
pub const RESERVED_LOW_BITS: u32 = if cfg!(target_arch = "x86_64") { 1 } else { 0 };

fn word_of(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    let end = (at + 8).min(bytes.len());
    if at < end {
        word[..end - at].copy_from_slice(&bytes[at..end]);
    }
    u64::from_le_bytes(word)
}

fn set_word_of(bytes: &mut [u8], at: usize, value: u64) {
    let end = (at + 8).min(bytes.len());
    if at < end {
        bytes[at..end].copy_from_slice(&value.to_le_bytes()[..end - at]);
    }
}

unsafe extern "C" fn buffer_copy(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    let table = witnesses(metadata);
    if !table.is_inline() {
        let boxed = word(source as usize);
        heap_object::swift_retain(boxed as *mut u8);
        set_word(destination as usize, boxed);
        return project_box(boxed, table.alignment_mask()) as *mut u8;
    }
    copy_value(destination, source, metadata);
    destination
}

unsafe extern "C" fn plain_copy(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    copy_bytes(destination, source, witnesses(metadata).size);
    destination
}

unsafe extern "C" fn plain_destroy(_: *mut u8, _: Metadata) {}

unsafe extern "C" fn plain_tag(value: *const u8, empty_cases: u32, metadata: Metadata) -> u32 {
    single_payload_tag(value, empty_cases, witnesses(metadata).size, 0, |_| 0)
}

unsafe extern "C" fn plain_store_tag(
    value: *mut u8,
    case: u32,
    empty_cases: u32,
    metadata: Metadata,
) {
    let size = witnesses(metadata).size;
    store_single_payload_tag(value, case, empty_cases, size, 0, |_, _| {});
}

const BOOL_EXTRA_INHABITANTS: u32 = 254;

unsafe extern "C" fn bool_tag(value: *const u8, empty_cases: u32, _: Metadata) -> u32 {
    single_payload_tag(
        value,
        empty_cases,
        1,
        BOOL_EXTRA_INHABITANTS,
        |payload| match payload.first().copied().unwrap_or(0) {
            0 | 1 => 0,
            byte => u32::from(byte) - 1,
        },
    )
}

unsafe extern "C" fn bool_store_tag(value: *mut u8, case: u32, empty_cases: u32, _: Metadata) {
    store_single_payload_tag(
        value,
        case,
        empty_cases,
        1,
        BOOL_EXTRA_INHABITANTS,
        |payload, index| payload[0] = (index + 2) as u8,
    );
}

unsafe extern "C" fn raw_pointer_tag(value: *const u8, empty_cases: u32, _: Metadata) -> u32 {
    single_payload_tag(value, empty_cases, 8, 1, |payload| {
        u32::from(word_of(payload, 0) == 0)
    })
}

unsafe extern "C" fn raw_pointer_store_tag(
    value: *mut u8,
    case: u32,
    empty_cases: u32,
    _: Metadata,
) {
    store_single_payload_tag(value, case, empty_cases, 8, 1, |payload, _| {
        set_word_of(payload, 0, 0)
    });
}

/// `getEnumTagSinglePayload` for a value whose word at `at` is a pointer with heap pointer
/// extra inhabitants.
pub fn pointer_word_tag(value: *const u8, empty_cases: u32, size: usize, at: usize) -> u32 {
    single_payload_tag(
        value,
        empty_cases,
        size,
        enum_tag::POINTER_EXTRA_INHABITANTS,
        |payload| enum_tag::pointer_tag(word_of(payload, at), RESERVED_LOW_BITS),
    )
}

/// `storeEnumTagSinglePayload` for the same values as [`pointer_word_tag`].
pub fn store_pointer_word_tag(value: *mut u8, case: u32, empty_cases: u32, size: usize, at: usize) {
    store_single_payload_tag(
        value,
        case,
        empty_cases,
        size,
        enum_tag::POINTER_EXTRA_INHABITANTS,
        |payload, index| {
            payload.fill(0);
            set_word_of(
                payload,
                at,
                enum_tag::pointer_inhabitant(index, RESERVED_LOW_BITS),
            );
        },
    );
}

unsafe extern "C" fn pointer_tag(value: *const u8, empty_cases: u32, _: Metadata) -> u32 {
    pointer_word_tag(value, empty_cases, 8, 0)
}

unsafe extern "C" fn pointer_store_tag(value: *mut u8, case: u32, empty_cases: u32, _: Metadata) {
    store_pointer_word_tag(value, case, empty_cases, 8, 0);
}

/// How a reference-counted word is retained and released.
#[derive(Debug, Clone, Copy)]
enum Reference {
    Native,
    Unknown,
    Bridge,
}

impl Reference {
    fn retain(self, value: usize) {
        match self {
            Self::Native => {
                heap_object::swift_retain(value as *mut u8);
            }
            Self::Unknown => {
                heap_object::unknown_object_retain(value as *mut u8);
            }
            Self::Bridge => {
                heap_object::swift_bridge_object_retain(value);
            }
        }
    }

    fn release(self, value: usize) {
        match self {
            Self::Native => heap_object::swift_release(value as *mut u8),
            Self::Unknown => heap_object::unknown_object_release(value as *mut u8),
            Self::Bridge => heap_object::swift_bridge_object_release(value),
        }
    }
}

macro_rules! reference_witnesses {
    ($copy:ident, $destroy:ident, $assign_copy:ident, $assign_take:ident, $reference:expr) => {
        unsafe extern "C" fn $copy(destination: *mut u8, source: *mut u8, _: Metadata) -> *mut u8 {
            let value = word(source as usize);
            $reference.retain(value);
            set_word(destination as usize, value);
            destination
        }

        unsafe extern "C" fn $destroy(value: *mut u8, _: Metadata) {
            $reference.release(word(value as usize));
        }

        unsafe extern "C" fn $assign_copy(
            destination: *mut u8,
            source: *mut u8,
            _: Metadata,
        ) -> *mut u8 {
            let (old, new) = (word(destination as usize), word(source as usize));
            $reference.retain(new);
            set_word(destination as usize, new);
            $reference.release(old);
            destination
        }

        unsafe extern "C" fn $assign_take(
            destination: *mut u8,
            source: *mut u8,
            _: Metadata,
        ) -> *mut u8 {
            let old = word(destination as usize);
            set_word(destination as usize, word(source as usize));
            $reference.release(old);
            destination
        }
    };
}

reference_witnesses!(
    native_copy,
    native_destroy,
    native_assign_copy,
    native_assign_take,
    Reference::Native
);
reference_witnesses!(
    unknown_copy,
    unknown_destroy,
    unknown_assign_copy,
    unknown_assign_take,
    Reference::Unknown
);
reference_witnesses!(
    bridge_copy,
    bridge_destroy,
    bridge_assign_copy,
    bridge_assign_take,
    Reference::Bridge
);

const fn plain_table(
    size: usize,
    alignment: usize,
    extra: (u32, TagWitness, StoreTagWitness),
) -> ValueWitnessTable {
    let inline = size <= 24 && alignment <= 8;
    ValueWitnessTable {
        initialize_buffer_with_copy_of_buffer: buffer_copy,
        destroy: plain_destroy,
        initialize_with_copy: plain_copy,
        assign_with_copy: plain_copy,
        initialize_with_take: plain_copy,
        assign_with_take: plain_copy,
        get_enum_tag_single_payload: extra.1,
        store_enum_tag_single_payload: extra.2,
        size,
        stride: if size == 0 { 1 } else { size },
        flags: (alignment as u32 - 1) | if inline { 0 } else { value_flags::NON_INLINE },
        extra_inhabitant_count: extra.0,
    }
}

const NO_EXTRA: (u32, TagWitness, StoreTagWitness) = (0, plain_tag, plain_store_tag);

const fn reference_table(
    copy: CopyWitness,
    destroy: DestroyWitness,
    assign_copy: CopyWitness,
    assign_take: CopyWitness,
) -> ValueWitnessTable {
    ValueWitnessTable {
        initialize_buffer_with_copy_of_buffer: copy,
        destroy,
        initialize_with_copy: copy,
        assign_with_copy: assign_copy,
        initialize_with_take: plain_copy,
        assign_with_take: assign_take,
        get_enum_tag_single_payload: pointer_tag,
        store_enum_tag_single_payload: pointer_store_tag,
        size: 8,
        stride: 8,
        flags: 7 | value_flags::NON_POD,
        extra_inhabitant_count: enum_tag::POINTER_EXTRA_INHABITANTS,
    }
}

pub static INT8_WITNESSES: ValueWitnessTable = plain_table(1, 1, NO_EXTRA);
pub static INT16_WITNESSES: ValueWitnessTable = plain_table(2, 2, NO_EXTRA);
pub static INT32_WITNESSES: ValueWitnessTable = plain_table(4, 4, NO_EXTRA);
pub static INT64_WITNESSES: ValueWitnessTable = plain_table(8, 8, NO_EXTRA);
pub static INT128_WITNESSES: ValueWitnessTable = plain_table(16, 16, NO_EXTRA);
pub static EMPTY_WITNESSES: ValueWitnessTable = plain_table(0, 1, NO_EXTRA);
pub static BOOL_WITNESSES: ValueWitnessTable =
    plain_table(1, 1, (BOOL_EXTRA_INHABITANTS, bool_tag, bool_store_tag));
pub static RAW_POINTER_WITNESSES: ValueWitnessTable =
    plain_table(8, 8, (1, raw_pointer_tag, raw_pointer_store_tag));
pub static NATIVE_OBJECT_WITNESSES: ValueWitnessTable = reference_table(
    native_copy,
    native_destroy,
    native_assign_copy,
    native_assign_take,
);
pub static UNKNOWN_OBJECT_WITNESSES: ValueWitnessTable = reference_table(
    unknown_copy,
    unknown_destroy,
    unknown_assign_copy,
    unknown_assign_take,
);
pub static BRIDGE_OBJECT_WITNESSES: ValueWitnessTable = reference_table(
    bridge_copy,
    bridge_destroy,
    bridge_assign_copy,
    bridge_assign_take,
);

unsafe extern "C" fn string_copy(destination: *mut u8, source: *mut u8, _: Metadata) -> *mut u8 {
    copy_bytes(destination, source, 16);
    heap_object::swift_bridge_object_retain(word(source as usize + 8));
    destination
}

unsafe extern "C" fn string_destroy(value: *mut u8, _: Metadata) {
    heap_object::swift_bridge_object_release(word(value as usize + 8));
}

unsafe extern "C" fn string_assign_copy(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    let old = word(destination as usize + 8);
    // SAFETY: the witness's own contract, passed on.
    unsafe { string_copy(destination, source, metadata) };
    heap_object::swift_bridge_object_release(old);
    destination
}

unsafe extern "C" fn string_assign_take(
    destination: *mut u8,
    source: *mut u8,
    _: Metadata,
) -> *mut u8 {
    let old = word(destination as usize + 8);
    copy_bytes(destination, source, 16);
    heap_object::swift_bridge_object_release(old);
    destination
}

unsafe extern "C" fn string_tag(value: *const u8, empty_cases: u32, _: Metadata) -> u32 {
    pointer_word_tag(value, empty_cases, 16, 8)
}

unsafe extern "C" fn string_store_tag(value: *mut u8, case: u32, empty_cases: u32, _: Metadata) {
    store_pointer_word_tag(value, case, empty_cases, 16, 8);
}

/// `String`'s table: two words, the second a bridge object.
pub static STRING_WITNESSES: ValueWitnessTable = ValueWitnessTable {
    initialize_buffer_with_copy_of_buffer: string_copy,
    destroy: string_destroy,
    initialize_with_copy: string_copy,
    assign_with_copy: string_assign_copy,
    initialize_with_take: plain_copy,
    assign_with_take: string_assign_take,
    get_enum_tag_single_payload: string_tag,
    store_enum_tag_single_payload: string_store_tag,
    size: 16,
    stride: 16,
    flags: 7 | value_flags::NON_POD,
    extra_inhabitant_count: enum_tag::POINTER_EXTRA_INHABITANTS,
};

const EXISTENTIAL_TYPE_OFFSET: usize = 24;

fn stored_type(container: *const u8) -> Metadata {
    word(container as usize + EXISTENTIAL_TYPE_OFFSET)
}

unsafe extern "C" fn existential_copy(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    let size = witnesses(metadata).size;
    let at = EXISTENTIAL_TYPE_OFFSET;
    copy_bytes(
        destination.wrapping_add(at),
        source.wrapping_add(at),
        size - at,
    );
    let stored = stored_type(source);
    let table = witnesses(stored);
    if table.is_inline() {
        copy_value(destination, source, stored);
    } else {
        let boxed = word(source as usize);
        heap_object::swift_retain(boxed as *mut u8);
        set_word(destination as usize, boxed);
    }
    destination
}

unsafe extern "C" fn existential_destroy(value: *mut u8, _: Metadata) {
    let stored = stored_type(value);
    let table = witnesses(stored);
    if table.is_inline() {
        destroy_value(value, stored);
    } else {
        heap_object::swift_release(word(value as usize) as *mut u8);
    }
}

unsafe extern "C" fn existential_assign_copy(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    if destination != source {
        // SAFETY: the witnesses' own contracts, passed on.
        unsafe { existential_destroy(destination, metadata) };
        // SAFETY: as above.
        unsafe { existential_copy(destination, source, metadata) };
    }
    destination
}

unsafe extern "C" fn existential_assign_take(
    destination: *mut u8,
    source: *mut u8,
    metadata: Metadata,
) -> *mut u8 {
    // SAFETY: as above.
    unsafe { existential_destroy(destination, metadata) };
    copy_bytes(destination, source, witnesses(metadata).size);
    destination
}

unsafe extern "C" fn existential_tag(
    value: *const u8,
    empty_cases: u32,
    metadata: Metadata,
) -> u32 {
    pointer_word_tag(
        value,
        empty_cases,
        witnesses(metadata).size,
        EXISTENTIAL_TYPE_OFFSET,
    )
}

unsafe extern "C" fn existential_store_tag(
    value: *mut u8,
    case: u32,
    empty_cases: u32,
    metadata: Metadata,
) {
    let size = witnesses(metadata).size;
    store_pointer_word_tag(value, case, empty_cases, size, EXISTENTIAL_TYPE_OFFSET);
}

/// The table of an opaque existential with `witness_tables` protocol witness tables: a
/// three-word buffer, the stored value's type, then the tables.
pub const fn existential_table(witness_tables: usize) -> ValueWitnessTable {
    let size = EXISTENTIAL_TYPE_OFFSET + 8 + 8 * witness_tables;
    ValueWitnessTable {
        initialize_buffer_with_copy_of_buffer: buffer_copy,
        destroy: existential_destroy,
        initialize_with_copy: existential_copy,
        assign_with_copy: existential_assign_copy,
        initialize_with_take: plain_copy,
        assign_with_take: existential_assign_take,
        get_enum_tag_single_payload: existential_tag,
        store_enum_tag_single_payload: existential_store_tag,
        size,
        stride: size,
        flags: 7 | value_flags::NON_POD | value_flags::NON_INLINE,
        extra_inhabitant_count: enum_tag::POINTER_EXTRA_INHABITANTS,
    }
}

pub static ANY_WITNESSES: ValueWitnessTable = existential_table(0);

/// Existential type flags: not class-bound.
pub const OPAQUE_EXISTENTIAL: u32 = 0x8000_0000;

/// An existential type's metadata, its protocols after it.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct ExistentialMetadata {
    pub value_witnesses: &'static ValueWitnessTable,
    pub kind: usize,
    pub flags: u32,
    pub protocol_count: u32,
}

/// `Any` (`$sypN`).
pub static ANY: ExistentialMetadata = ExistentialMetadata {
    value_witnesses: &ANY_WITNESSES,
    kind: kind::EXISTENTIAL,
    flags: OPAQUE_EXISTENTIAL,
    protocol_count: 0,
};

/// A struct's metadata with no field offsets: the standard library's own types.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct StructMetadata {
    pub value_witnesses: &'static ValueWitnessTable,
    pub kind: usize,
    pub description: &'static TypeDescriptor,
}

/// A tuple's metadata with no elements: `()` (`$sytN`).
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct EmptyTupleMetadata {
    pub value_witnesses: &'static ValueWitnessTable,
    pub kind: usize,
    pub count: usize,
    pub labels: usize,
}

pub static EMPTY_TUPLE: EmptyTupleMetadata = EmptyTupleMetadata {
    value_witnesses: &EMPTY_WITNESSES,
    kind: kind::TUPLE,
    count: 0,
    labels: 0,
};

/// The address a static metadata's pointers name: past its value witness table word.
pub fn address_of<T>(metadata: &'static T) -> Metadata {
    metadata as *const T as usize + 8
}

const UNIQUE: u32 = 0x40;
/// Context descriptor kinds.
pub mod context {
    pub const MODULE: u32 = 0;
    pub const PROTOCOL: u32 = 3;
    pub const CLASS: u32 = 16;
    pub const STRUCT: u32 = 17;
    pub const ENUM: u32 = 18;
    pub const KIND_MASK: u32 = 0x1f;
    pub const GENERIC: u32 = 0x80;
}

/// A type's context descriptor: its kind, parent, name and metadata accessor as relative
/// pointers (filled at set-up), and room for the fields readers may look at, zeroed.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct TypeDescriptor {
    pub flags: u32,
    parent: AtomicI32,
    name: AtomicI32,
    access_function: AtomicI32,
    rest: [u32; 8],
}

fn store_relative(field: &AtomicI32, target: usize) {
    let at = field as *const AtomicI32 as usize;
    field.store(target.wrapping_sub(at) as i32, Ordering::Relaxed);
}

impl TypeDescriptor {
    pub const fn new(kind: u32, generic: bool) -> Self {
        Self {
            flags: kind | UNIQUE | if generic { context::GENERIC } else { 0 },
            parent: AtomicI32::new(0),
            name: AtomicI32::new(0),
            access_function: AtomicI32::new(0),
            rest: [0; 8],
        }
    }

    /// Names the descriptor `name` (NUL-terminated) in the Swift module.
    pub fn fill(&self, name: &'static [u8]) {
        store_relative(&self.parent, &raw const SWIFT_MODULE as usize);
        store_relative(&self.name, name.as_ptr() as usize);
        self.access_function.store(0, Ordering::Relaxed);
    }

    pub fn address(&'static self) -> usize {
        self as *const Self as usize
    }
}

/// A module's context descriptor.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct ModuleDescriptor {
    pub flags: u32,
    parent: AtomicI32,
    name: AtomicI32,
}

/// The `Swift` module.
pub static SWIFT_MODULE: ModuleDescriptor = ModuleDescriptor {
    flags: context::MODULE | UNIQUE,
    parent: AtomicI32::new(0),
    name: AtomicI32::new(0),
};

macro_rules! standard_structs {
    ($($metadata:ident, $descriptor:ident, $witnesses:ident, $name:literal;)*) => {
        $(
            pub static $descriptor: TypeDescriptor = TypeDescriptor::new(context::STRUCT, false);
            pub static $metadata: StructMetadata = StructMetadata {
                value_witnesses: &$witnesses,
                kind: kind::STRUCT,
                description: &$descriptor,
            };
        )*

        fn fill_standard_structs() {
            $(
                $descriptor.fill(concat!($name, "\0").as_bytes());
            )*
        }

        /// The standard struct whose descriptor is at `descriptor`.
        pub fn standard_struct(descriptor: usize) -> Option<Metadata> {
            $(
                if descriptor == $descriptor.address() {
                    return Some(address_of(&$metadata));
                }
            )*
            None
        }
    };
}

standard_structs! {
    INT, INT_DESCRIPTOR, INT64_WITNESSES, "Int";
    INT8, INT8_DESCRIPTOR, INT8_WITNESSES, "Int8";
    INT16, INT16_DESCRIPTOR, INT16_WITNESSES, "Int16";
    INT32, INT32_DESCRIPTOR, INT32_WITNESSES, "Int32";
    INT64, INT64_DESCRIPTOR, INT64_WITNESSES, "Int64";
    UINT, UINT_DESCRIPTOR, INT64_WITNESSES, "UInt";
    UINT8, UINT8_DESCRIPTOR, INT8_WITNESSES, "UInt8";
    UINT16, UINT16_DESCRIPTOR, INT16_WITNESSES, "UInt16";
    UINT32, UINT32_DESCRIPTOR, INT32_WITNESSES, "UInt32";
    UINT64, UINT64_DESCRIPTOR, INT64_WITNESSES, "UInt64";
    DOUBLE, DOUBLE_DESCRIPTOR, INT64_WITNESSES, "Double";
    FLOAT, FLOAT_DESCRIPTOR, INT32_WITNESSES, "Float";
    BOOL, BOOL_DESCRIPTOR, BOOL_WITNESSES, "Bool";
    STRING, STRING_DESCRIPTOR, STRING_WITNESSES, "String";
    UNSAFE_RAW_POINTER, UNSAFE_RAW_POINTER_DESCRIPTOR, RAW_POINTER_WITNESSES, "UnsafeRawPointer";
    UNSAFE_MUTABLE_RAW_POINTER, UNSAFE_MUTABLE_RAW_POINTER_DESCRIPTOR, RAW_POINTER_WITNESSES,
        "UnsafeMutableRawPointer";
    DEFAULT_STRING_INTERPOLATION, DEFAULT_STRING_INTERPOLATION_DESCRIPTOR, STRING_WITNESSES,
        "DefaultStringInterpolation";
}

/// The generic standard library types' descriptors: the runtime instantiates their metadata.
pub static OPTIONAL_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::ENUM, true);
pub static ARRAY_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::STRUCT, true);
pub static DICTIONARY_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::STRUCT, true);
pub static SET_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::STRUCT, true);
pub static CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR: TypeDescriptor =
    TypeDescriptor::new(context::CLASS, true);
pub static DICTIONARY_STORAGE_DESCRIPTOR: TypeDescriptor =
    TypeDescriptor::new(context::CLASS, true);
pub static STRING_STORAGE_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::CLASS, false);
pub static EMPTY_ARRAY_STORAGE_DESCRIPTOR: TypeDescriptor =
    TypeDescriptor::new(context::CLASS, false);
pub static EMPTY_DICTIONARY_DESCRIPTOR: TypeDescriptor = TypeDescriptor::new(context::CLASS, false);

/// The standard library type a one-letter substitution names, if it is not generic.
pub fn standard(standard: Standard) -> Option<Metadata> {
    Some(match standard {
        Standard::Bool => address_of(&BOOL),
        Standard::Double => address_of(&DOUBLE),
        Standard::Float => address_of(&FLOAT),
        Standard::Int => address_of(&INT),
        Standard::String => address_of(&STRING),
        Standard::UInt => address_of(&UINT),
        Standard::UnsafeRawPointer => address_of(&UNSAFE_RAW_POINTER),
        Standard::UnsafeMutableRawPointer => address_of(&UNSAFE_MUTABLE_RAW_POINTER),
        _ => return None,
    })
}

/// A type of the Swift module by its mangled name and kind letter.
pub fn swift_type(name: &[u8], kind: u8) -> Option<Metadata> {
    if kind != b'V' {
        return None;
    }
    Some(match name {
        b"Int8" => address_of(&INT8),
        b"Int16" => address_of(&INT16),
        b"Int32" => address_of(&INT32),
        b"Int64" => address_of(&INT64),
        b"UInt8" => address_of(&UINT8),
        b"UInt16" => address_of(&UINT16),
        b"UInt32" => address_of(&UINT32),
        b"UInt64" => address_of(&UINT64),
        b"DefaultStringInterpolation" => address_of(&DEFAULT_STRING_INTERPOLATION),
        _ => return None,
    })
}

/// Swift class metadata with Objective-C interoperation, from its destructor and value witness
/// table (before the address point) to its description and ivar destroyer; generic arguments
/// follow it.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct ClassMetadata {
    pub destroy: AtomicUsize,
    pub value_witnesses: AtomicUsize,
    pub isa: AtomicUsize,
    pub superclass: AtomicUsize,
    pub cache: AtomicUsize,
    pub vtable: AtomicUsize,
    pub data: AtomicUsize,
    pub flags: u32,
    pub instance_address_point: u32,
    pub instance_size: u32,
    pub instance_alignment_mask: u16,
    pub reserved: u16,
    pub class_size: u32,
    pub class_address_point: u32,
    pub description: AtomicUsize,
    pub ivar_destroyer: usize,
}

/// Where a class's first generic argument is, from its address point.
pub const CLASS_ARGUMENTS: usize = core::mem::size_of::<ClassMetadata>() - 16;
/// `data`'s bit that marks a Swift class of the stable ABI.
pub const SWIFT_CLASS: usize = 2;
/// Class flags: instances count references the Swift way.
pub const USES_SWIFT_REFCOUNTING: u32 = 2;

impl ClassMetadata {
    pub const fn new(instance_size: u32, arguments: u32) -> Self {
        Self {
            destroy: AtomicUsize::new(0),
            value_witnesses: AtomicUsize::new(0),
            isa: AtomicUsize::new(0),
            superclass: AtomicUsize::new(0),
            cache: AtomicUsize::new(0),
            vtable: AtomicUsize::new(0),
            data: AtomicUsize::new(0),
            flags: USES_SWIFT_REFCOUNTING,
            instance_address_point: 0,
            instance_size,
            instance_alignment_mask: 7,
            reserved: 0,
            class_size: CLASS_ARGUMENTS as u32 + 8 * arguments,
            class_address_point: 16,
            description: AtomicUsize::new(0),
            ivar_destroyer: 0,
        }
    }

    /// Wires the class into `_SwiftObject`'s hierarchy with its destructor and descriptor.
    pub fn fill(&self, destroy: u64, description: &'static TypeDescriptor) {
        self.destroy.store(destroy as usize, Ordering::Relaxed);
        self.value_witnesses.store(
            &raw const NATIVE_OBJECT_WITNESSES as usize,
            Ordering::Relaxed,
        );
        self.isa
            .store(super::swift_object::metaclass(), Ordering::Relaxed);
        self.superclass
            .store(super::swift_object::class(), Ordering::Relaxed);
        self.cache.store(
            &raw const crate::objc::cache::EMPTY as usize,
            Ordering::Relaxed,
        );
        self.data.store(SWIFT_CLASS, Ordering::Relaxed);
        self.description
            .store(description.address(), Ordering::Relaxed);
    }

    /// The metadata's address point.
    pub fn address(&self) -> Metadata {
        &raw const self.isa as usize
    }
}

/// Fills the descriptors' relative pointers; once, before the program runs.
pub fn set_up() {
    store_relative(&SWIFT_MODULE.name, b"Swift\0".as_ptr() as usize);
    fill_standard_structs();
    OPTIONAL_DESCRIPTOR.fill(b"Optional\0");
    ARRAY_DESCRIPTOR.fill(b"Array\0");
    DICTIONARY_DESCRIPTOR.fill(b"Dictionary\0");
    SET_DESCRIPTOR.fill(b"Set\0");
    CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR.fill(b"_ContiguousArrayStorage\0");
    DICTIONARY_STORAGE_DESCRIPTOR.fill(b"_DictionaryStorage\0");
    STRING_STORAGE_DESCRIPTOR.fill(b"__StringStorage\0");
    EMPTY_ARRAY_STORAGE_DESCRIPTOR.fill(b"__EmptyArrayStorage\0");
    EMPTY_DICTIONARY_DESCRIPTOR.fill(b"__EmptyDictionarySingleton\0");
}

/// The address of the metadata or value witness table export `name`, if it is one of these.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_$sBi8_WV" => &raw const INT8_WITNESSES as usize,
        b"_$sBi16_WV" => &raw const INT16_WITNESSES as usize,
        b"_$sBi32_WV" => &raw const INT32_WITNESSES as usize,
        b"_$sBi64_WV" => &raw const INT64_WITNESSES as usize,
        b"_$sBi128_WV" => &raw const INT128_WITNESSES as usize,
        b"_$sBi1_WV" => &raw const BOOL_WITNESSES as usize,
        b"_$sBpWV" => &raw const RAW_POINTER_WITNESSES as usize,
        b"_$sBoWV" => &raw const NATIVE_OBJECT_WITNESSES as usize,
        b"_$sBOWV" => &raw const UNKNOWN_OBJECT_WITNESSES as usize,
        b"_$sBbWV" => &raw const BRIDGE_OBJECT_WITNESSES as usize,
        b"_$sytWV" => &raw const EMPTY_WITNESSES as usize,
        b"_$sypN" => address_of(&ANY),
        b"_$sytN" => address_of(&EMPTY_TUPLE),
        b"_$sSiN" => address_of(&INT),
        b"_$ss4Int8VN" => address_of(&INT8),
        b"_$ss5Int16VN" => address_of(&INT16),
        b"_$ss5Int32VN" => address_of(&INT32),
        b"_$ss5Int64VN" => address_of(&INT64),
        b"_$sSuN" => address_of(&UINT),
        b"_$ss5UInt8VN" => address_of(&UINT8),
        b"_$ss6UInt16VN" => address_of(&UINT16),
        b"_$ss6UInt32VN" => address_of(&UINT32),
        b"_$ss6UInt64VN" => address_of(&UINT64),
        b"_$sSdN" => address_of(&DOUBLE),
        b"_$sSfN" => address_of(&FLOAT),
        b"_$sSbN" => address_of(&BOOL),
        b"_$sSSN" => address_of(&STRING),
        b"_$sSVN" => address_of(&UNSAFE_RAW_POINTER),
        b"_$sSvN" => address_of(&UNSAFE_MUTABLE_RAW_POINTER),
        b"_$ss26DefaultStringInterpolationVN" => address_of(&DEFAULT_STRING_INTERPOLATION),
        b"_$sSiMn" => INT_DESCRIPTOR.address(),
        b"_$ss5UInt8VMn" => UINT8_DESCRIPTOR.address(),
        b"_$sSdMn" => DOUBLE_DESCRIPTOR.address(),
        b"_$sSSMn" => STRING_DESCRIPTOR.address(),
        b"_$sSbMn" => BOOL_DESCRIPTOR.address(),
        b"_$sSqMn" => OPTIONAL_DESCRIPTOR.address(),
        b"_$sSaMn" => ARRAY_DESCRIPTOR.address(),
        b"_$sSDMn" => DICTIONARY_DESCRIPTOR.address(),
        b"_$sShMn" => SET_DESCRIPTOR.address(),
        b"_$ss23_ContiguousArrayStorageCMn" => CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR.address(),
        b"_$ss18_DictionaryStorageCMn" => DICTIONARY_STORAGE_DESCRIPTOR.address(),
        _ => return None,
    };
    Some(address as u64)
}
