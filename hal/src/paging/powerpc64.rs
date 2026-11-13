//! POWER9 radix page tables (Power ISA 3.0B, Book III, 6.7.10; D304): a 52-bit tree, the only
//! size POWER9 walks, as five levels: a root of 16 entries (only the first 16 of the
//! 512-entry table the kernel allocates are walked: the process table entry says 4 index bits),
//! then four levels of 512 entries. Quadrant 0 (the low addresses) is a program's tree,
//! reached through the process table entry of the CPU's `PIDR`; quadrant 3 (`0xc000...`) is
//! the kernel's, through process table entry 0. Leaves are 4 KiB pages and, at levels 1 and 2,
//! 2 MiB and 1 GiB pages.
//!
//! The hardware reads every entry big-endian, so each is stored byte-swapped on this
//! little-endian kernel: [`Format::table`], [`Format::leaf`] and [`Format::kept`] return the
//! swapped value and [`Format::decode`] swaps back.

use super::{Attributes, Entry, Format, MemoryType};

/// The POWER9 radix format. See the module documentation.
#[derive(Debug)]
pub struct Powerpc64;

const VALID: u64 = 1 << 63;
const LEAF: u64 = 1 << 62;
const ADDRESS: u64 = ((1 << 56) - 1) & !0xfff;
const NEXT_LEVEL_SIZE: u64 = 9;
const EXECUTE: u64 = 1 << 0;
const WRITE: u64 = 1 << 1;
const READ: u64 = 1 << 2;
const PRIVILEGED: u64 = 1 << 3;
const ATTRIBUTE_SHIFT: u32 = 4;
const ATTRIBUTE_MASK: u64 = 3 << ATTRIBUTE_SHIFT;
const ATTRIBUTE_IO: u64 = 2 << ATTRIBUTE_SHIFT;
const CHANGED: u64 = 1 << 7;
const REFERENCED: u64 = 1 << 8;
const KEPT: u64 = 1 << 58;

/// The process table entry's root size: 4 index bits at the root.
pub const ROOT_INDEX_BITS: u64 = 4;
/// The radix tree size, 52 bits, as the partition and process table entries encode it:
/// 52 - 31 split into bits 62:61 and 7:5.
pub const RADIX_TREE_SIZE: u64 = (0x2 << 61) | (0x5 << 5);

/// A process or partition table entry's first doubleword for the tree at `root`, as the
/// hardware reads it (byte-swapped).
pub const fn table_entry(root: u64) -> u64 {
    (RADIX_TREE_SIZE | (root & ADDRESS) | ROOT_INDEX_BITS).swap_bytes()
}

impl Format for Powerpc64 {
    const LEVELS: usize = 5;
    const ROOTS: usize = 2;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = true;
    const USER_END: u64 = 1 << 47;

    fn root(virt: u64) -> Option<usize> {
        match virt >> 52 {
            0 => Some(0),
            0xc00 => Some(1),
            _ => None,
        }
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        (VALID | (table & ADDRESS) | NEXT_LEVEL_SIZE).swap_bytes()
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        ((phys & ADDRESS) | KEPT).swap_bytes()
    }

    fn decode(entry: u64, _level: usize) -> Entry {
        let entry = entry.swap_bytes();
        if entry & VALID == 0 {
            if entry & KEPT != 0 {
                return Entry::Kept(entry & ADDRESS);
            }
            return Entry::Empty;
        }
        if entry & LEAF == 0 {
            return Entry::Table(entry & ADDRESS);
        }
        Entry::Leaf(
            entry & ADDRESS,
            Attributes {
                writable: entry & WRITE != 0,
                executable: entry & EXECUTE != 0,
                memory_type: match entry & ATTRIBUTE_MASK {
                    ATTRIBUTE_IO => MemoryType::Device,
                    _ => MemoryType::Normal,
                },
                user: entry & PRIVILEGED == 0,
            },
        )
    }
}

/// A leaf at `level` for `phys` with `attributes`, byte-swapped as the hardware reads it.
pub const fn leaf(phys: u64, _level: usize, attributes: Attributes) -> u64 {
    let mut entry = VALID | LEAF | (phys & ADDRESS) | READ | REFERENCED;
    if attributes.writable {
        entry |= WRITE | CHANGED;
    }
    if attributes.executable {
        entry |= EXECUTE;
    }
    if !attributes.user {
        entry |= PRIVILEGED;
    }
    if let MemoryType::Device = attributes.memory_type {
        entry |= ATTRIBUTE_IO;
    }
    entry.swap_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip() {
        crate::paging::tests::round_trips::<Powerpc64>();
    }

    #[test]
    fn the_quadrants_pick_the_roots() {
        assert_eq!(Powerpc64::root(0x7fff_ffff_f000), Some(0));
        assert_eq!(Powerpc64::root(0xc000_0000_2001_0000), Some(1));
        assert_eq!(Powerpc64::root(0xffff_ffff_8000_0000), None);
    }
}
