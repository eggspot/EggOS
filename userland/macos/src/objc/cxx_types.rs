//! C++ run-time type information as libc++abi lays it out on macOS (D380), enough for
//! exceptions: the vtables a program's `type_info` records point at (class, single and
//! multiple inheritance, pointer, fundamental), the fundamental types' and `std::exception`'s
//! own records, and the match a `catch` clause makes against a thrown type: the same type (by
//! address or name), a public base found through the inheritance records with the object's
//! address adjusted to it, or a pointer type's pointee; and `__dynamic_cast` over the same
//! records (D408).

#[cfg(eggos_user)]
use super::exception::{image_byte, image_word};

#[cfg(not(eggos_user))]
fn image_word(_: u64) -> Option<u64> {
    None
}

#[cfg(not(eggos_user))]
fn image_byte(_: u64) -> Option<u8> {
    None
}

const NON_UNIQUE_NAME: u64 = 1 << 63;
const BASE_PUBLIC: i64 = 2;
const BASE_VIRTUAL: i64 = 1;
const BASE_OFFSET_SHIFT: u32 = 8;
const MAX_BASE_DEPTH: usize = 32;
const MAX_BASES: u64 = 64;
const MAX_NAME: u64 = 1024;

/// A `std::type_info`'s first two words.
#[repr(C)]
#[derive(Debug)]
pub struct TypeRecord {
    pub vtable: *const usize,
    pub name: *const u8,
}

/// `__si_class_type_info`: a class with one public, non-virtual base at offset 0.
#[repr(C)]
#[derive(Debug)]
pub struct SingleBaseRecord {
    pub record: TypeRecord,
    pub base: *const TypeRecord,
}

/// `__pointer_type_info` (`__pbase_type_info`'s layout).
#[repr(C)]
#[derive(Debug)]
pub struct PointerRecord {
    pub record: TypeRecord,
    pub flags: u32,
    pub pointee: *const TypeRecord,
}

/// The vtables `type_info` records point two words into, one per kind.
#[repr(C)]
#[derive(Debug)]
pub struct Vtables {
    pub class: [usize; 4],
    pub single_base: [usize; 4],
    pub multiple_bases: [usize; 4],
    pub pointer: [usize; 4],
    pub fundamental: [usize; 4],
}

/// The vtables, which only their addresses matter for.
pub static VTABLES: Vtables = Vtables {
    class: [0; 4],
    single_base: [0; 4],
    multiple_bases: [0; 4],
    pointer: [0; 4],
    fundamental: [0; 4],
};

const fn address_point(table: &'static [usize; 4]) -> *const usize {
    (table as *const [usize; 4]).cast::<usize>().wrapping_add(2)
}

/// The kinds of `type_info` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Class,
    SingleBase,
    MultipleBases,
    Pointer,
    Fundamental,
    Unknown,
}

macro_rules! fundamental {
    ($($name:ident = $text:literal;)*) => {
        $(
            pub static $name: TypeRecord = TypeRecord {
                vtable: address_point(&VTABLES.fundamental),
                name: $text.as_ptr().cast(),
            };
        )*
    };
}

fundamental! {
    TYPE_INT = c"i";
    TYPE_UNSIGNED_INT = c"j";
    TYPE_LONG = c"l";
    TYPE_UNSIGNED_LONG = c"m";
    TYPE_LONG_LONG = c"x";
    TYPE_UNSIGNED_LONG_LONG = c"y";
    TYPE_CHAR = c"c";
    TYPE_SIGNED_CHAR = c"a";
    TYPE_UNSIGNED_CHAR = c"h";
    TYPE_SHORT = c"s";
    TYPE_UNSIGNED_SHORT = c"t";
    TYPE_BOOL = c"b";
    TYPE_FLOAT = c"f";
    TYPE_DOUBLE = c"d";
    TYPE_VOID = c"v";
}

macro_rules! pointer {
    ($($name:ident = $text:literal, $flags:literal, $pointee:ident;)*) => {
        $(
            pub static $name: PointerRecord = PointerRecord {
                record: TypeRecord {
                    vtable: address_point(&VTABLES.pointer),
                    name: $text.as_ptr().cast(),
                },
                flags: $flags,
                pointee: &raw const $pointee,
            };
        )*
    };
}

pointer! {
    TYPE_CONST_CHAR_POINTER = c"PKc", 1, TYPE_CHAR;
    TYPE_CHAR_POINTER = c"Pc", 0, TYPE_CHAR;
    TYPE_VOID_POINTER = c"Pv", 0, TYPE_VOID;
    TYPE_CONST_VOID_POINTER = c"PKv", 1, TYPE_VOID;
}

/// `std::exception`'s record.
pub static TYPE_EXCEPTION: TypeRecord = TypeRecord {
    vtable: address_point(&VTABLES.class),
    name: c"St9exception".as_ptr().cast(),
};

/// `std::bad_alloc`'s record.
pub static TYPE_BAD_ALLOC: SingleBaseRecord = SingleBaseRecord {
    record: TypeRecord {
        vtable: address_point(&VTABLES.single_base),
        name: c"St9bad_alloc".as_ptr().cast(),
    },
    base: &raw const TYPE_EXCEPTION,
};

// SAFETY: the records are never written, and their pointers are to permanent statics.
unsafe impl Sync for TypeRecord {}
// SAFETY: as above.
unsafe impl Sync for SingleBaseRecord {}
// SAFETY: as above.
unsafe impl Sync for PointerRecord {}

fn own(address: u64, length: u64) -> bool {
    let ranges: [(u64, u64); 3] = [
        (&raw const VTABLES as u64, size_of::<Vtables>() as u64),
        (
            &raw const TYPE_EXCEPTION as u64,
            size_of::<TypeRecord>() as u64,
        ),
        (
            &raw const TYPE_BAD_ALLOC as u64,
            size_of::<SingleBaseRecord>() as u64,
        ),
    ];
    let records = [
        &raw const TYPE_INT as u64,
        &raw const TYPE_UNSIGNED_INT as u64,
        &raw const TYPE_LONG as u64,
        &raw const TYPE_UNSIGNED_LONG as u64,
        &raw const TYPE_LONG_LONG as u64,
        &raw const TYPE_UNSIGNED_LONG_LONG as u64,
        &raw const TYPE_CHAR as u64,
        &raw const TYPE_SIGNED_CHAR as u64,
        &raw const TYPE_UNSIGNED_CHAR as u64,
        &raw const TYPE_SHORT as u64,
        &raw const TYPE_UNSIGNED_SHORT as u64,
        &raw const TYPE_BOOL as u64,
        &raw const TYPE_FLOAT as u64,
        &raw const TYPE_DOUBLE as u64,
        &raw const TYPE_VOID as u64,
    ];
    let pointers = [
        &raw const TYPE_CONST_CHAR_POINTER as u64,
        &raw const TYPE_CHAR_POINTER as u64,
        &raw const TYPE_VOID_POINTER as u64,
        &raw const TYPE_CONST_VOID_POINTER as u64,
    ];
    let inside = |(start, size): (u64, u64)| address >= start && address + length <= start + size;
    ranges.into_iter().any(inside)
        || records
            .into_iter()
            .any(|start| inside((start, size_of::<TypeRecord>() as u64)))
        || pointers
            .into_iter()
            .any(|start| inside((start, size_of::<PointerRecord>() as u64)))
}

fn word(address: u64) -> Option<u64> {
    if own(address, 8) {
        // SAFETY: inside one of the layer's permanent records, eight bytes.
        return Some(unsafe { (address as *const u64).read_unaligned() });
    }
    image_word(address)
}

fn byte(address: u64) -> Option<u8> {
    if own(address, 1) {
        // SAFETY: inside one of the layer's permanent records.
        return Some(unsafe { (address as *const u8).read() });
    }
    image_byte(address)
}

fn name_byte(address: u64) -> Option<u8> {
    if address == 0 {
        return None;
    }
    byte(address).or_else(|| {
        let is_own_name = [
            TYPE_INT.name,
            TYPE_UNSIGNED_INT.name,
            TYPE_LONG.name,
            TYPE_UNSIGNED_LONG.name,
            TYPE_LONG_LONG.name,
            TYPE_UNSIGNED_LONG_LONG.name,
            TYPE_CHAR.name,
            TYPE_SIGNED_CHAR.name,
            TYPE_UNSIGNED_CHAR.name,
            TYPE_SHORT.name,
            TYPE_UNSIGNED_SHORT.name,
            TYPE_BOOL.name,
            TYPE_FLOAT.name,
            TYPE_DOUBLE.name,
            TYPE_VOID.name,
            TYPE_EXCEPTION.name,
            TYPE_BAD_ALLOC.record.name,
            TYPE_CONST_CHAR_POINTER.record.name,
            TYPE_CHAR_POINTER.record.name,
            TYPE_VOID_POINTER.record.name,
            TYPE_CONST_VOID_POINTER.record.name,
        ]
        .into_iter()
        .any(|name| {
            let start = name as u64;
            address >= start && address < start + 16
        });
        // SAFETY: inside one of the layer's own NUL-terminated names, at most 16 bytes.
        is_own_name.then(|| unsafe { (address as *const u8).read() })
    })
}

/// Byte `index` of the name of the `type_info` record at `record`, its mangled type.
pub fn name_byte_of(record: u64, index: u64) -> Option<u8> {
    let name = word(record + 8)? & !NON_UNIQUE_NAME;
    name_byte(name + index)
}

/// The kind of the `type_info` record at `record`.
pub fn kind(record: u64) -> Kind {
    let Some(vtable) = word(record) else {
        return Kind::Unknown;
    };
    let points = |table: &'static [usize; 4]| vtable == address_point(table) as u64;
    if points(&VTABLES.class) {
        Kind::Class
    } else if points(&VTABLES.single_base) {
        Kind::SingleBase
    } else if points(&VTABLES.multiple_bases) {
        Kind::MultipleBases
    } else if points(&VTABLES.pointer) {
        Kind::Pointer
    } else if points(&VTABLES.fundamental) {
        Kind::Fundamental
    } else {
        Kind::Unknown
    }
}

fn names_equal(first: u64, second: u64) -> bool {
    let (first, second) = (first & !NON_UNIQUE_NAME, second & !NON_UNIQUE_NAME);
    if first == second {
        return first != 0;
    }
    for offset in 0..MAX_NAME {
        match (name_byte(first + offset), name_byte(second + offset)) {
            (Some(a), Some(b)) if a == b => {
                if a == 0 {
                    return true;
                }
            }
            _ => return false,
        }
    }
    false
}

/// Whether the records at `first` and `second` name the same type.
pub fn same_type(first: u64, second: u64) -> bool {
    first == second
        || match (word(first + 8), word(second + 8)) {
            (Some(a), Some(b)) => names_equal(a, b),
            _ => false,
        }
}

fn base_address(object: u64, offset_flags: i64) -> Option<u64> {
    let offset = offset_flags >> BASE_OFFSET_SHIFT;
    if offset_flags & BASE_VIRTUAL == 0 {
        return Some(object.wrapping_add(offset as u64));
    }
    // SAFETY: `object` is a live thrown object of a polymorphic class with a virtual base, so
    // its first word is its vtable pointer, and the base's offset sits `offset` bytes from it.
    let vtable = unsafe { (object as *const u64).read_unaligned() };
    // SAFETY: as above.
    let virtual_offset = unsafe { ((vtable as i64 + offset) as *const i64).read_unaligned() };
    Some(object.wrapping_add(virtual_offset as u64))
}

fn find_base(wanted: u64, record: u64, object: u64, depth: usize) -> Option<u64> {
    if same_type(wanted, record) {
        return Some(object);
    }
    if depth > MAX_BASE_DEPTH {
        return None;
    }
    match kind(record) {
        Kind::SingleBase => find_base(wanted, word(record + 16)?, object, depth + 1),
        Kind::MultipleBases => {
            let counts = word(record + 16)?;
            let bases = (counts >> 32).min(MAX_BASES);
            for index in 0..bases {
                let entry = record + 24 + index * 16;
                let base = word(entry)?;
                let offset_flags = word(entry + 8)? as i64;
                if offset_flags & BASE_PUBLIC == 0 {
                    continue;
                }
                let at = base_address(object, offset_flags)?;
                if let Some(found) = find_base(wanted, base, at, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// What a `catch` of `wanted` makes of an exception of type `thrown` at `object`: the address
/// its handler receives, or `None` when it does not catch it. A `catch (...)` (`wanted` 0)
/// takes everything.
pub fn catches(wanted: u64, thrown: u64, object: u64) -> Option<u64> {
    if wanted == 0 {
        return Some(object);
    }
    if kind(wanted) == Kind::Pointer && kind(thrown) == Kind::Pointer {
        let wanted_pointee = word(wanted + 24)?;
        let thrown_pointee = word(thrown + 24)?;
        let wanted_flags = word(wanted + 16)? as u32;
        let thrown_flags = word(thrown + 16)? as u32;
        let qualifiers_fit = thrown_flags & !wanted_flags & 0x7 == 0;
        // SAFETY: a thrown pointer's object holds the pointer.
        let pointer = unsafe { (object as *const u64).read_unaligned() };
        let void_target = kind(wanted_pointee) == Kind::Fundamental
            && names_equal(word(wanted_pointee + 8)?, TYPE_VOID.name as u64);
        if !qualifiers_fit {
            return None;
        }
        if void_target || same_type(wanted_pointee, thrown_pointee) {
            return Some(pointer);
        }
        return find_base(wanted_pointee, thrown_pointee, pointer, 0);
    }
    find_base(wanted, thrown, object, 0)
}

/// `__dynamic_cast` (D408): the object `sub` points into, seen as `wanted`: the whole object
/// when that is its dynamic type, else the public base of that type inside it (a downcast or
/// a cross cast), else 0. The object's vtable gives its offset to the whole object and its
/// dynamic type, as the Itanium ABI lays them out.
pub fn dynamic_cast(sub: u64, wanted: u64) -> u64 {
    if sub == 0 || wanted == 0 {
        return 0;
    }
    // SAFETY: `sub` points at a live polymorphic object, whose first word is its vtable's
    // address point.
    let vtable = unsafe { (sub as *const u64).read_unaligned() };
    // SAFETY: the offset to the top is the word two before the vtable's address point.
    let to_top = unsafe { ((vtable - 16) as *const i64).read_unaligned() };
    // SAFETY: the type record is the word just before the vtable's address point.
    let dynamic = unsafe { ((vtable - 8) as *const u64).read_unaligned() };
    let whole = sub.wrapping_add(to_top as u64);
    if same_type(wanted, dynamic) {
        return whole;
    }
    find_base(wanted, dynamic, whole, 0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dynamic_cast_finds_the_whole_object_or_a_base_or_nothing() {
        let vtable: [u64; 3] = [0, &raw const TYPE_BAD_ALLOC as u64, 0];
        let object: [u64; 1] = [(&raw const vtable[2]) as u64];
        let at = &raw const object as u64;
        let bad_alloc = &raw const TYPE_BAD_ALLOC as u64;
        let exception = &raw const TYPE_EXCEPTION as u64;
        assert_eq!(dynamic_cast(at, bad_alloc), at);
        assert_eq!(dynamic_cast(at, exception), at);
        assert_eq!(dynamic_cast(at, &raw const TYPE_INT as u64), 0);
        assert_eq!(dynamic_cast(0, exception), 0);
    }

    #[test]
    fn the_fundamental_records_know_their_kind_and_name() {
        assert_eq!(kind(&raw const TYPE_INT as u64), Kind::Fundamental);
        assert_eq!(
            kind(&raw const TYPE_CONST_CHAR_POINTER as u64),
            Kind::Pointer
        );
        assert!(same_type(
            &raw const TYPE_INT as u64,
            &raw const TYPE_INT as u64
        ));
        assert!(!same_type(
            &raw const TYPE_INT as u64,
            &raw const TYPE_LONG as u64
        ));
    }

    #[test]
    fn a_catch_all_takes_anything_and_a_type_takes_itself() {
        let object = 0x1000;
        assert_eq!(catches(0, &raw const TYPE_INT as u64, object), Some(object));
        assert_eq!(
            catches(
                &raw const TYPE_INT as u64,
                &raw const TYPE_INT as u64,
                object
            ),
            Some(object)
        );
        assert_eq!(
            catches(
                &raw const TYPE_LONG as u64,
                &raw const TYPE_INT as u64,
                object
            ),
            None
        );
    }

    #[test]
    fn a_base_class_catches_a_derived_one() {
        let thrown = &raw const TYPE_BAD_ALLOC as u64;
        let wanted = &raw const TYPE_EXCEPTION as u64;
        assert_eq!(catches(wanted, thrown, 0x2000), Some(0x2000));
        assert_eq!(catches(thrown, wanted, 0x2000), None);
    }

    #[test]
    fn a_pointer_catch_receives_the_pointer() {
        let text = c"thrown";
        let slot: u64 = text.as_ptr() as u64;
        let object = &raw const slot as u64;
        let pointer = &raw const TYPE_CONST_CHAR_POINTER as u64;
        assert_eq!(catches(pointer, pointer, object), Some(slot));
        assert_eq!(
            catches(&raw const TYPE_CHAR_POINTER as u64, pointer, object),
            None
        );
        assert_eq!(
            catches(&raw const TYPE_CONST_VOID_POINTER as u64, pointer, object),
            Some(slot)
        );
    }
}
