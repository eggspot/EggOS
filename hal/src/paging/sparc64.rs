//! SPARC64 page tables (D326). The UltraSPARC's MMU loads its TLBs from software: the miss
//! handlers (`arch/sparc64/exception.rs`) walk four levels of 512 entries over the 44 bits the
//! CPU implements, so the format is the kernel's to choose:
//!
//! - a directory entry is the next table's physical address, which the handlers load from with
//!   a physical ASI; an empty entry is 0, where the walk stops and the miss becomes a fault;
//! - a leaf is a 4 KiB page in a translation table entry's own layout (`V`, the physical page,
//!   `CP` and `CV` for cacheable memory, `E` for side effects, `P` for the kernel's, `W`, `G`),
//!   plus three software bits below bit 12: the page is a program's, executable, and
//!   [`Format::kept`]'s mark.
//!
//! The MMU's smallest page is 8 KiB, so the handlers load an 8 KiB entry for an aligned pair of
//! 4 KiB leaves that are both present, physically contiguous and alike, and turn any other miss
//! into a fault. The low half (below `2^43`) goes through root 0, a program's; the high half
//! (from `2^64 - 2^43`) through root 1, the kernel's, which only the device window needs: the
//! direct map is loaded by the handlers in 4 MiB entries computed from the address, and the
//! image is mapped by locked entries.

use super::{Attributes, Entry, Format, MemoryType};

#[derive(Debug)]
pub struct Sparc64;

pub const VALID: u64 = 1 << 63;
pub const GLOBAL: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const PRIVILEGED: u64 = 1 << 2;
pub const SIDE_EFFECT: u64 = 1 << 3;
pub const CACHEABLE_VIRTUAL: u64 = 1 << 4;
pub const CACHEABLE_PHYSICAL: u64 = 1 << 5;
pub const KEPT: u64 = 1 << 7;
pub const EXECUTABLE: u64 = 1 << 8;
pub const USER: u64 = 1 << 9;
/// The software bits, which a translation table entry's `Soft` field would take with bit 12.
pub const SOFTWARE: u64 = 0x1f << 7;
pub const PHYSICAL: u64 = ((1 << 41) - 1) & !0xfff;
/// A 4 MiB translation table entry's size field.
pub const SIZE_4M: u64 = 3 << 61;
/// The first address of the high half on a CPU with 44 virtual address bits.
pub const HIGH_HALF: u64 = 0xffff_f800_0000_0000;

impl Format for Sparc64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 2;
    const MAX_LEAF_LEVEL: usize = 0;
    const ENCODES_MEMORY_TYPE: bool = true;
    const USER_END: u64 = 1 << 43;
    const PAIRED_PAGES: bool = true;

    fn root(virt: u64) -> Option<usize> {
        match virt {
            _ if virt < 1 << 43 => Some(0),
            _ if virt >= HIGH_HALF => Some(1),
            _ => None,
        }
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        table & PHYSICAL
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        phys & PHYSICAL | KEPT
    }

    fn decode(entry: u64, level: usize) -> Entry {
        if level > 0 {
            return match entry {
                0 => Entry::Empty,
                table => Entry::Table(table & PHYSICAL),
            };
        }
        let phys = entry & PHYSICAL;
        if entry & VALID == 0 {
            return match entry & KEPT != 0 {
                true => Entry::Kept(phys),
                false => Entry::Empty,
            };
        }
        Entry::Leaf(
            phys,
            Attributes {
                writable: entry & WRITABLE != 0,
                executable: entry & EXECUTABLE != 0,
                memory_type: match entry & (SIDE_EFFECT | CACHEABLE_PHYSICAL) {
                    SIDE_EFFECT => MemoryType::Device,
                    0 => MemoryType::WriteCombining,
                    _ => MemoryType::Normal,
                },
                user: entry & USER != 0,
            },
        )
    }
}

pub const fn leaf(phys: u64, _level: usize, attributes: Attributes) -> u64 {
    let mut entry = VALID
        | phys & PHYSICAL
        | match attributes.memory_type {
            MemoryType::Normal => CACHEABLE_PHYSICAL | CACHEABLE_VIRTUAL,
            MemoryType::Device => SIDE_EFFECT,
            MemoryType::WriteCombining => 0,
        };
    if attributes.writable {
        entry |= WRITABLE;
    }
    if attributes.executable {
        entry |= EXECUTABLE;
    }
    entry |= match attributes.user {
        true => USER,
        false => PRIVILEGED | GLOBAL,
    };
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn leaves_round_trip() {
        round_trips::<Sparc64>();
    }

    #[test]
    fn the_halves_are_the_44_bit_ones() {
        assert_eq!(Sparc64::root(0), Some(0));
        assert_eq!(Sparc64::root((1 << 43) - 1), Some(0));
        assert_eq!(Sparc64::root(1 << 43), None);
        assert_eq!(Sparc64::root(HIGH_HALF), Some(1));
    }
}
