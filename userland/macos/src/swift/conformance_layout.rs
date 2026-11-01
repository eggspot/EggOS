//! The layout of a protocol conformance descriptor and a protocol descriptor's requirements, as
//! Swift's ABI fixes them (D180): a conformance names its protocol (a relative pointer, indirect
//! through a bound pointer when its low bit is set), the conforming type, a witness table
//! pattern and flags; after them come, as the flags say, the retroactive context, conditional
//! requirements and pack shapes, the resilient witnesses (each a requirement descriptor and its
//! implementation, relative) and the generic witness table's sizes, instantiator and private
//! data. Pure logic over a [`Memory`] with host tests on bytes `swiftc` emitted.

/// Reads the words the descriptors are made of.
pub trait Memory {
    /// The 32-bit word at `address`.
    fn word32(&self, address: usize) -> u32;
    /// The pointer-sized word at `address`.
    fn word(&self, address: usize) -> usize;
}

/// The target of the relative pointer at `address`, low bit included.
pub fn relative(memory: &impl Memory, address: usize) -> usize {
    address.wrapping_add_signed(memory.word32(address) as i32 as isize)
}

/// The target of the relative pointer at `address`, or `None` when it is zero.
pub fn optional_relative(memory: &impl Memory, address: usize) -> Option<usize> {
    (memory.word32(address) != 0).then(|| relative(memory, address))
}

/// The target of the indirectable relative pointer at `address`: when its low bit is set, the
/// target (without the bit) holds the pointer.
pub fn indirectable(memory: &impl Memory, address: usize) -> usize {
    let target = relative(memory, address);
    if target & 1 != 0 {
        memory.word(target & !1)
    } else {
        target
    }
}

mod flags {
    pub const TYPE_REFERENCE_SHIFT: u32 = 3;
    pub const TYPE_REFERENCE_MASK: u32 = 7;
    pub const RETROACTIVE: u32 = 1 << 6;
    pub const CONDITIONAL_SHIFT: u32 = 8;
    pub const COUNT_MASK: u32 = 0xff;
    pub const RESILIENT_WITNESSES: u32 = 1 << 16;
    pub const GENERIC_WITNESS_TABLE: u32 = 1 << 17;
    pub const CONDITIONAL_PACK_SHIFT: u32 = 24;
}

const DIRECT_DESCRIPTOR: u32 = 0;
const INDIRECT_DESCRIPTOR: u32 = 1;

const HEADER: usize = 16;
const CONDITIONAL_REQUIREMENT: usize = 12;
const PACK_SHAPE_HEADER: usize = 4;
const PACK_SHAPE: usize = 8;
const RESILIENT_WITNESS: usize = 8;

/// A generic witness table's description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericTable {
    /// Words copied from the pattern, the conformance's own word included.
    pub pattern_words: usize,
    /// Words of private data before the table.
    pub private_words: usize,
    /// The function that finishes an instantiated table.
    pub instantiator: Option<usize>,
}

/// A parsed conformance descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conformance {
    pub address: usize,
    flags: u32,
    /// Where the protocol's indirectable relative pointer is.
    pub protocol_field: usize,
    type_field: usize,
    pub pattern: Option<usize>,
    witnesses_at: usize,
    pub witness_count: usize,
    pub generic: Option<GenericTable>,
}

impl Conformance {
    /// The conforming type's context descriptor, unless the conformance names an Objective-C
    /// class.
    pub fn type_descriptor(&self, memory: &impl Memory) -> Option<usize> {
        match (self.flags >> flags::TYPE_REFERENCE_SHIFT) & flags::TYPE_REFERENCE_MASK {
            DIRECT_DESCRIPTOR => Some(relative(memory, self.type_field)),
            INDIRECT_DESCRIPTOR => Some(memory.word(relative(memory, self.type_field))),
            _ => None,
        }
    }

    /// Where resilient witness `index`'s requirement and implementation pointers are.
    pub fn witness_fields(&self, index: usize) -> (usize, usize) {
        let at = self.witnesses_at + index * RESILIENT_WITNESS;
        (at, at + 4)
    }
}

/// Parses the conformance descriptor at `address`.
pub fn parse(memory: &impl Memory, address: usize) -> Conformance {
    let conformance_flags = memory.word32(address + 12);
    let mut at = address + HEADER;
    if conformance_flags & flags::RETROACTIVE != 0 {
        at += 4;
    }
    let conditional =
        ((conformance_flags >> flags::CONDITIONAL_SHIFT) & flags::COUNT_MASK) as usize;
    at += conditional * CONDITIONAL_REQUIREMENT;
    let packs = ((conformance_flags >> flags::CONDITIONAL_PACK_SHIFT) & flags::COUNT_MASK) as usize;
    if packs > 0 {
        at += PACK_SHAPE_HEADER + packs * PACK_SHAPE;
    }
    let mut witness_count = 0;
    if conformance_flags & flags::RESILIENT_WITNESSES != 0 {
        witness_count = memory.word32(at) as usize;
        at += 4;
    }
    let witnesses_at = at;
    at += witness_count * RESILIENT_WITNESS;
    let generic = (conformance_flags & flags::GENERIC_WITNESS_TABLE != 0).then(|| {
        let sizes = memory.word32(at);
        GenericTable {
            pattern_words: (sizes & 0xffff) as usize,
            private_words: (sizes >> 17) as usize,
            instantiator: optional_relative(memory, at + 4),
        }
    });
    Conformance {
        address,
        flags: conformance_flags,
        protocol_field: address,
        type_field: address + 4,
        pattern: optional_relative(memory, address + 8),
        witnesses_at,
        witness_count,
        generic,
    }
}

/// A protocol requirement's kind.
pub mod requirement {
    pub const KIND_MASK: u32 = 0xf;
    pub const BASE_PROTOCOL: u32 = 0;
    pub const METHOD: u32 = 1;
    pub const INIT: u32 = 2;
    pub const GETTER: u32 = 3;
    pub const ASSOCIATED_TYPE: u32 = 7;
    pub const ASSOCIATED_CONFORMANCE: u32 = 8;
    pub const INSTANCE: u32 = 0x10;
}

/// The size of a protocol requirement descriptor.
pub const REQUIREMENT_SIZE: usize = 8;
const GENERIC_REQUIREMENT: usize = 12;

/// The number of requirements of the protocol descriptor at `protocol`.
pub fn requirement_count(memory: &impl Memory, protocol: usize) -> usize {
    memory.word32(protocol + 16) as usize
}

/// Where the protocol descriptor at `protocol` keeps its first requirement.
pub fn requirements_start(memory: &impl Memory, protocol: usize) -> usize {
    let in_signature = memory.word32(protocol + 12) as usize;
    protocol + 24 + in_signature * GENERIC_REQUIREMENT
}

/// The witness table slot of the requirement descriptor at `requirement`, if it is one of the
/// protocol's: slot 0 is the conformance descriptor, so requirement `i` fills slot `i + 1`.
pub fn slot_of(memory: &impl Memory, protocol: usize, requirement: usize) -> Option<usize> {
    let start = requirements_start(memory, protocol);
    let offset = requirement.checked_sub(start)?;
    let index = offset / REQUIREMENT_SIZE;
    (offset % REQUIREMENT_SIZE == 0 && index < requirement_count(memory, protocol))
        .then_some(index + 1)
}

/// The slot an associated type or conformance descriptor names, from the protocol's requirements
/// base descriptor (one requirement before the first).
pub fn slot_from_base(base: usize, requirement: usize) -> Option<usize> {
    let offset = requirement.checked_sub(base)?;
    (offset % REQUIREMENT_SIZE == 0 && offset > 0).then_some(offset / REQUIREMENT_SIZE)
}

/// What a witness table slot of an associated type or conformance holds before it is resolved:
/// the address of a mangled name with the low bit set.
pub fn unresolved_name(slot_value: usize) -> Option<usize> {
    (slot_value & 1 != 0).then_some(slot_value & !1)
}

/// The marker byte of an associated conformance name that is a reference to its accessor.
pub const ACCESSOR_MARKER: u8 = 0xff;
/// The symbolic reference kind of an associated conformance accessor.
pub const ACCESSOR_REFERENCE: u8 = 0x07;

#[cfg(test)]
mod tests {
    use super::*;

    struct Bytes<'a> {
        base: usize,
        bytes: &'a [u8],
        bound: &'a [(usize, usize)],
    }

    impl Memory for Bytes<'_> {
        fn word32(&self, address: usize) -> u32 {
            let at = address - self.base;
            u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
        }

        fn word(&self, address: usize) -> usize {
            self.bound
                .iter()
                .find(|(slot, _)| *slot == address)
                .map(|(_, value)| *value)
                .unwrap()
        }
    }

    fn words(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    const HASHABLE_AT: usize = 0x1_0000_3c38;

    fn hashable() -> Vec<u8> {
        words(&[
            0x0000_4721,
            0x0000_025c,
            0,
            0x0003_0000,
            4,
            0x0000_4715,
            0x0000_012b,
            0x0000_4715,
            0xffff_e488,
            0x0000_4715,
            0xffff_e4c4,
            0x0000_4715,
            0xffff_e4e4,
            0x0001_0000,
            0,
            0x0000_893c,
        ])
    }

    #[test]
    fn reads_an_enums_hashable_conformance_as_swiftc_lays_it_out() {
        let bytes = hashable();
        let bound = [(0x1_0000_8358, 0x7000)];
        let memory = Bytes {
            base: HASHABLE_AT,
            bytes: &bytes,
            bound: &bound,
        };
        let conformance = parse(&memory, HASHABLE_AT);
        assert_eq!(indirectable(&memory, conformance.protocol_field), 0x7000);
        assert_eq!(conformance.type_descriptor(&memory), Some(0x1_0000_3e98));
        assert_eq!(conformance.pattern, None);
        assert_eq!(conformance.witness_count, 4);
        let (requirement, implementation) = conformance.witness_fields(0);
        assert_eq!(relative(&memory, requirement), 0x1_0000_8361);
        assert_eq!(relative(&memory, implementation), 0x1_0000_3d7b);
        assert_eq!(unresolved_name(0x1_0000_3d7b), Some(0x1_0000_3d7a));
        let (_, hash_value) = conformance.witness_fields(1);
        assert_eq!(relative(&memory, hash_value), 0x1_0000_20e0);
        assert_eq!(
            conformance.generic,
            Some(GenericTable {
                pattern_words: 0,
                private_words: 0,
                instantiator: None,
            })
        );
    }

    #[test]
    fn conditional_requirements_and_retroactive_contexts_come_before_the_witnesses() {
        let flags = flags::RESILIENT_WITNESSES | flags::RETROACTIVE | (2 << 8);
        let mut values = vec![0, 0, 8, flags, 0];
        values.extend([0; 6]);
        values.extend([1, 0x40, 0x44]);
        let bytes = words(&values);
        let memory = Bytes {
            base: 0x1000,
            bytes: &bytes,
            bound: &[],
        };
        let conformance = parse(&memory, 0x1000);
        assert_eq!(conformance.pattern, Some(0x1008 + 8));
        assert_eq!(conformance.witness_count, 1);
        assert_eq!(conformance.witness_fields(0), (0x1000 + 48, 0x1000 + 52));
        assert_eq!(conformance.generic, None);
    }

    #[test]
    fn requirement_slots_follow_the_conformance_word() {
        let mut values = vec![3 | 0x40, 0, 0, 1, 3, 0];
        values.extend([0; 3]);
        values.extend([0, 0, 0x13, 0, 1, 0]);
        let bytes = words(&values);
        let memory = Bytes {
            base: 0x2000,
            bytes: &bytes,
            bound: &[],
        };
        let start = requirements_start(&memory, 0x2000);
        assert_eq!(start, 0x2000 + 36);
        assert_eq!(slot_of(&memory, 0x2000, start), Some(1));
        assert_eq!(slot_of(&memory, 0x2000, start + 16), Some(3));
        assert_eq!(slot_of(&memory, 0x2000, start + 24), None);
        assert_eq!(slot_of(&memory, 0x2000, start + 4), None);
        assert_eq!(slot_from_base(start - 8, start + 8), Some(2));
        assert_eq!(slot_from_base(start - 8, start - 8), None);
    }
}
