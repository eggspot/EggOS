//! Run-time type information as MSVC lays it out for 64-bit programs (D230): a polymorphic
//! object's virtual function table is preceded by the address of its complete object locator,
//! which names the subobject's offset in the complete object, the complete type's descriptor
//! and its class hierarchy descriptor; that lists every base class of the complete type in
//! depth-first order (the type itself first), each with the number of bases below it, how to
//! find it in the complete object (`Displacement`) and whether it is publicly visible or
//! ambiguous. Every address in them is relative to the image base the locator's own address
//! gives. `dynamic_cast` follows the C++ rules over that list: a downcast to the one object of
//! the target type that holds the source subobject, else a cross cast to the complete object's
//! one public base of the target type. Type descriptors are the same type when they are the same
//! descriptor or carry the same decorated name, as with types from different modules. Pure logic
//! over a reader of the program's objects, with host tests.

use crate::cxx_walk::{Displacement, ObjectMemory};
use alloc::vec::Vec;
use core::cmp::Ordering;

const IMAGE_RELATIVE: i32 = 1;
const TYPE_NAME_OFFSET: u64 = 16;
const MAX_NAME: u64 = 4096;
const MAX_BASES: u32 = 1024;

/// A base class descriptor's attributes.
pub mod attributes {
    /// Not a public base of the complete object.
    pub const NOT_VISIBLE: u32 = 0x01;
    /// The complete object holds more than one such base.
    pub const AMBIGUOUS: u32 = 0x02;
}

/// A polymorphic subobject's complete object locator: the subobject's offset in the complete
/// object, the constructor displacement's place (0 for none), the complete type's descriptor,
/// its class hierarchy descriptor, and the image base its addresses are relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locator {
    pub offset: u32,
    pub constructor_offset: u32,
    pub type_descriptor: u64,
    pub hierarchy: u64,
    pub image_base: u64,
}

/// The complete object locator of the polymorphic subobject whose virtual function table
/// pointer is at `object`, if its table has a 64-bit (image-relative) locator.
pub fn locator(memory: &impl ObjectMemory, object: u64) -> Option<Locator> {
    let table = memory.u64_at(object);
    if table == 0 {
        return None;
    }
    let address = memory.u64_at(table.wrapping_sub(8));
    if address == 0 || memory.i32_at(address) != IMAGE_RELATIVE {
        return None;
    }
    let word = |offset: u64| memory.i32_at(address.wrapping_add(offset)) as u32;
    let image_base = address.wrapping_sub(u64::from(word(20)));
    Some(Locator {
        offset: word(4),
        constructor_offset: word(8),
        type_descriptor: image_base.wrapping_add(u64::from(word(12))),
        hierarchy: image_base.wrapping_add(u64::from(word(16))),
        image_base,
    })
}

/// The complete object that holds the subobject at `object`: its offset back, less the
/// displacement a constructor of a class with virtual bases stores before it.
pub fn complete_object(memory: &impl ObjectMemory, object: u64, locator: &Locator) -> u64 {
    let mut complete = object.wrapping_sub(u64::from(locator.offset));
    if locator.constructor_offset != 0 {
        let at = object.wrapping_sub(u64::from(locator.constructor_offset));
        complete = complete.wrapping_sub(memory.i32_at(at) as i64 as u64);
    }
    complete
}

/// One base class of a complete type: its type descriptor, how many bases follow it that are
/// its own, where it is in the complete object, and its attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseClass {
    pub type_descriptor: u64,
    pub contained: u32,
    pub displacement: Displacement,
    pub attributes: u32,
}

/// The base classes of the complete type `locator` names, the type itself first.
pub fn base_classes(memory: &impl ObjectMemory, locator: &Locator) -> Vec<BaseClass> {
    let base = locator.image_base;
    let rva = |address: u64| base.wrapping_add(u64::from(memory.i32_at(address) as u32));
    let count = (memory.i32_at(locator.hierarchy.wrapping_add(8)) as u32).min(MAX_BASES);
    let array = rva(locator.hierarchy.wrapping_add(12));
    (0..u64::from(count))
        .map(|index| {
            let descriptor = rva(array.wrapping_add(index * 4));
            let word = |offset: u64| memory.i32_at(descriptor.wrapping_add(offset));
            BaseClass {
                type_descriptor: base.wrapping_add(u64::from(word(0) as u32)),
                contained: word(4) as u32,
                displacement: Displacement {
                    member: word(8),
                    virtual_base_pointer: word(12),
                    virtual_base: word(16),
                },
                attributes: word(20) as u32,
            }
        })
        .collect()
}

/// Whether the type descriptors at `left` and `right` describe the same type: the same
/// descriptor, or the same decorated name.
pub fn same_type(memory: &impl ObjectMemory, left: u64, right: u64) -> bool {
    if left == right {
        return true;
    }
    if left == 0 || right == 0 {
        return false;
    }
    for index in 0..MAX_NAME {
        let at = TYPE_NAME_OFFSET + index;
        let byte = memory.u8_at(left.wrapping_add(at));
        if byte != memory.u8_at(right.wrapping_add(at)) {
            return false;
        }
        if byte == 0 {
            return true;
        }
    }
    false
}

/// `dynamic_cast` of the subobject whose virtual function table pointer is at `object` (the
/// subobject of type `source` starts `vfptr_offset` bytes before it) to the type `target`: the
/// target object's address, or none when the cast fails.
pub fn dynamic_cast(
    memory: &impl ObjectMemory,
    object: u64,
    vfptr_offset: i64,
    source: u64,
    target: u64,
) -> Option<u64> {
    let locator = locator(memory, object)?;
    let complete = complete_object(memory, object, &locator);
    let source_offset = object
        .wrapping_sub(vfptr_offset as u64)
        .wrapping_sub(complete);
    let bases = base_classes(memory, &locator);
    let offsets: Vec<u64> = bases
        .iter()
        .map(|base| {
            base.displacement
                .apply(complete, memory)
                .wrapping_sub(complete)
        })
        .collect();
    let source_index = (0..bases.len()).find(|index| {
        offsets[*index] == source_offset && same_type(memory, bases[*index].type_descriptor, source)
    })?;
    if bases[source_index].attributes & attributes::NOT_VISIBLE != 0 {
        return None;
    }
    let targets: Vec<usize> = (0..bases.len())
        .filter(|index| same_type(memory, bases[*index].type_descriptor, target))
        .collect();
    let mut holding: Option<u64> = None;
    let mut ambiguous = false;
    for index in targets.iter().copied() {
        let last = index.saturating_add(bases[index].contained as usize);
        if !(index..=last).contains(&source_index) {
            continue;
        }
        match holding {
            None => holding = Some(offsets[index]),
            Some(offset) if offset == offsets[index] => {}
            Some(_) => ambiguous = true,
        }
    }
    if let Some(offset) = holding
        && !ambiguous
    {
        return Some(complete.wrapping_add(offset));
    }
    let mut across: Option<u64> = None;
    for index in targets {
        if bases[index].attributes & (attributes::NOT_VISIBLE | attributes::AMBIGUOUS) != 0 {
            return None;
        }
        match across {
            None => across = Some(offsets[index]),
            Some(offset) if offset == offsets[index] => {}
            Some(_) => return None,
        }
    }
    across.map(|offset| complete.wrapping_add(offset))
}

/// The order of two `type_info`s (`type_info::before`, `operator==`) by their decorated names,
/// past the leading `.`, as MSVC's runtime orders them.
pub fn compare_decorated(left: &[u8], right: &[u8]) -> Ordering {
    past_dot(left).cmp(past_dot(right))
}

fn past_dot(name: &[u8]) -> &[u8] {
    name.get(1..).unwrap_or(&[])
}

/// A `type_info`'s `hash_code`: FNV-1a over its decorated name past the leading `.`.
pub fn hash_decorated(name: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    past_dot(name).iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x1_4000_0000;
    const MULTIPLE: u32 = 1;
    const VIRTUAL: u32 = 2;
    const HAS_HIERARCHY: u32 = 0x40;
    const VIRTUAL_BASE_OF_COMPLETE: u32 = 0x10;
    const TYPE_A: u32 = 0x100;
    const TYPE_B: u32 = 0x140;
    const TYPE_C: u32 = 0x180;
    const TYPE_B_ELSEWHERE: u32 = 0x1c0;
    const TYPE_X: u32 = 0x200;
    const TYPE_E: u32 = 0x240;
    const OBJECT_C: u64 = BASE + 0x800;
    const OBJECT_E: u64 = BASE + 0x900;

    struct Bytes(Vec<u8>);

    impl Bytes {
        fn word(&mut self, at: u32, values: &[u32]) {
            for (index, value) in values.iter().enumerate() {
                let at = at as usize + index * 4;
                self.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }

        fn address(&mut self, at: u32, value: u64) {
            self.0[at as usize..at as usize + 8].copy_from_slice(&value.to_le_bytes());
        }

        fn type_descriptor(&mut self, at: u32, name: &str) {
            let start = at as usize + 16;
            self.0[start..start + name.len()].copy_from_slice(name.as_bytes());
        }

        fn read<const N: usize>(&self, address: u64) -> [u8; N] {
            let at = address.wrapping_sub(BASE) as usize;
            at.checked_add(N)
                .and_then(|end| self.0.get(at..end))
                .map_or([0; N], |bytes| bytes.try_into().unwrap())
        }
    }

    impl ObjectMemory for Bytes {
        fn u64_at(&self, address: u64) -> u64 {
            u64::from_le_bytes(self.read(address))
        }

        fn i32_at(&self, address: u64) -> i32 {
            i32::from_le_bytes(self.read(address))
        }

        fn u8_at(&self, address: u64) -> u8 {
            self.read::<1>(address)[0]
        }
    }

    fn program(b_attributes: u32) -> Bytes {
        let mut bytes = Bytes(vec![0; 0x1000]);
        bytes.type_descriptor(TYPE_A, ".?AUA@@");
        bytes.type_descriptor(TYPE_B, ".?AUB@@");
        bytes.type_descriptor(TYPE_C, ".?AUC@@");
        bytes.type_descriptor(TYPE_B_ELSEWHERE, ".?AUB@@");
        bytes.type_descriptor(TYPE_X, ".?AUX@@");
        bytes.type_descriptor(TYPE_E, ".?AUE@@");
        bytes.word(0x300, &[TYPE_C, 2, 0, u32::MAX, 0, HAS_HIERARCHY, 0x380]);
        bytes.word(0x320, &[TYPE_A, 0, 0, u32::MAX, 0, 0, 0]);
        bytes.word(0x340, &[TYPE_B, 0, 8, u32::MAX, 0, b_attributes, 0]);
        bytes.word(0x360, &[0x300, 0x320, 0x340]);
        bytes.word(0x380, &[0, MULTIPLE, 3, 0x360]);
        bytes.word(0x400, &[1, 0, 0, TYPE_C, 0x380, 0x400]);
        bytes.word(0x420, &[1, 8, 0, TYPE_C, 0x380, 0x420]);
        bytes.address(0x440, BASE + 0x400);
        bytes.address(0x450, BASE + 0x420);
        bytes.address(0x800, BASE + 0x448);
        bytes.address(0x808, BASE + 0x458);
        bytes.word(0x500, &[TYPE_E, 1, 0, u32::MAX, 0, HAS_HIERARCHY, 0x560]);
        bytes.word(0x520, &[TYPE_A, 0, 0, 0, 4, VIRTUAL_BASE_OF_COMPLETE, 0]);
        bytes.word(0x540, &[0x500, 0x520]);
        bytes.word(0x560, &[0, VIRTUAL, 2, 0x540]);
        bytes.word(0x580, &[1, 8, 0, TYPE_E, 0x560, 0x580]);
        bytes.address(0x5a0, BASE + 0x580);
        bytes.word(0x5c0, &[0, 8]);
        bytes.address(0x900, BASE + 0x5c0);
        bytes.address(0x908, BASE + 0x5a8);
        bytes
    }

    fn descriptor(rva: u32) -> u64 {
        BASE + u64::from(rva)
    }

    #[test]
    fn a_subobject_finds_its_complete_object_and_type() {
        let memory = program(0);
        let second = locator(&memory, OBJECT_C + 8).unwrap();
        assert_eq!(second.image_base, BASE);
        assert_eq!(second.offset, 8);
        assert_eq!(second.type_descriptor, descriptor(TYPE_C));
        assert_eq!(complete_object(&memory, OBJECT_C + 8, &second), OBJECT_C);
        assert_eq!(locator(&memory, BASE + 0xf00), None);
        let bases = base_classes(&memory, &second);
        assert_eq!(bases.len(), 3);
        assert_eq!(bases[2].displacement.member, 8);
    }

    #[test]
    fn downcasts_reach_the_complete_object_from_either_base() {
        let memory = program(0);
        let (a, b, c) = (descriptor(TYPE_A), descriptor(TYPE_B), descriptor(TYPE_C));
        assert_eq!(dynamic_cast(&memory, OBJECT_C, 0, a, c), Some(OBJECT_C));
        assert_eq!(dynamic_cast(&memory, OBJECT_C + 8, 0, b, c), Some(OBJECT_C));
        assert_eq!(dynamic_cast(&memory, OBJECT_C, 0, a, a), Some(OBJECT_C));
    }

    #[test]
    fn cross_casts_move_between_public_bases_and_match_names_from_other_descriptors() {
        let memory = program(0);
        let (a, b) = (descriptor(TYPE_A), descriptor(TYPE_B));
        assert_eq!(dynamic_cast(&memory, OBJECT_C, 0, a, b), Some(OBJECT_C + 8));
        assert_eq!(dynamic_cast(&memory, OBJECT_C + 8, 0, b, a), Some(OBJECT_C));
        let b_elsewhere = descriptor(TYPE_B_ELSEWHERE);
        assert_eq!(
            dynamic_cast(&memory, OBJECT_C, 0, a, b_elsewhere),
            Some(OBJECT_C + 8)
        );
        assert_eq!(
            dynamic_cast(&memory, OBJECT_C, 0, a, descriptor(TYPE_X)),
            None
        );
    }

    #[test]
    fn a_private_base_is_neither_reached_nor_left() {
        let memory = program(attributes::NOT_VISIBLE);
        let (a, b, c) = (descriptor(TYPE_A), descriptor(TYPE_B), descriptor(TYPE_C));
        assert_eq!(dynamic_cast(&memory, OBJECT_C, 0, a, b), None);
        assert_eq!(dynamic_cast(&memory, OBJECT_C + 8, 0, b, c), None);
        assert_eq!(dynamic_cast(&memory, OBJECT_C, 0, a, c), Some(OBJECT_C));
    }

    #[test]
    fn a_virtual_base_is_placed_through_the_virtual_base_table() {
        let memory = program(0);
        let (a, e) = (descriptor(TYPE_A), descriptor(TYPE_E));
        let located = locator(&memory, OBJECT_E + 8).unwrap();
        assert_eq!(complete_object(&memory, OBJECT_E + 8, &located), OBJECT_E);
        assert_eq!(dynamic_cast(&memory, OBJECT_E + 8, 0, a, e), Some(OBJECT_E));
        assert_eq!(dynamic_cast(&memory, OBJECT_E + 8, 0, e, a), None);
    }

    #[test]
    fn type_infos_order_and_hash_by_their_names_past_the_dot() {
        assert_eq!(compare_decorated(b".?AUA@@", b"x?AUA@@"), Ordering::Equal);
        assert_eq!(compare_decorated(b".?AUA@@", b".?AUB@@"), Ordering::Less);
        assert_eq!(hash_decorated(b"."), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash_decorated(b".H"), hash_decorated(b"_H"));
        assert_ne!(hash_decorated(b".H"), hash_decorated(b".I"));
    }
}
