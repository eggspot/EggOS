//! s390x page tables, z/Architecture's dynamic address translation (D320): a region-second
//! table at the root (53-bit addresses), region-third and segment tables of 2048 entries
//! (16 KiB) and page tables of 256 entries (2 KiB, each in a 4 KiB frame of its own). An entry's
//! bits are numbered from the most significant in IBM's manuals; here as values.
//!
//! - A region entry is the next table's origin, its table type and length (all 2048 entries)
//!   and the invalid bit.
//! - A segment entry is a page table's origin, or with the format control a 1 MiB leaf.
//! - A page entry is the frame, the invalid bit, the protection bit (read-only) and the
//!   instruction-execution-protection bit, with software bits in its low byte, which the
//!   hardware ignores: the page is a program's, it is kept (`PROT_NONE`), and it is writable
//!   (the protection bit says it too, but a kept page's must be remembered).
//!
//! DAT has no privilege bit: the HAL keeps programs out of the kernel half by running them in
//! the primary space through a root that holds the user half's entries alone (`arch/s390x`).
//! Every access is cached and coherent: entries record no memory type.

use super::{Attributes, Entry, Format, MemoryType};

/// The s390x format. See the module documentation.
#[derive(Debug)]
pub struct S390x;

const INVALID_REGION: u64 = 1 << 5;
const TABLE_TYPE_SHIFT: u32 = 2;
const TYPE_REGION_SECOND: u64 = 2;
const TYPE_REGION_THIRD: u64 = 1;
const TYPE_SEGMENT: u64 = 0;
const TABLE_LENGTH_FULL: u64 = 3;
const LARGE: u64 = 1 << 10;
const SEGMENT_PROTECT: u64 = 1 << 9;
const SEGMENT_NO_EXECUTE: u64 = 1 << 8;
const REGION_ORIGIN: u64 = !0xfff;
const PAGE_TABLE_ORIGIN: u64 = !0x7ff;
const LARGE_FRAME: u64 = !0xf_ffff;

const PAGE_INVALID: u64 = 1 << 10;
const PAGE_PROTECT: u64 = 1 << 9;
const PAGE_NO_EXECUTE: u64 = 1 << 8;
const PAGE_FRAME: u64 = !0xfff;
const SOFT_USER: u64 = 1 << 0;
const SOFT_KEPT: u64 = 1 << 1;
const SOFT_WRITABLE: u64 = 1 << 2;

/// The `ASCE` designation type of a region-second table and its length, for the HAL.
pub const ASCE_REGION_SECOND: u64 = TYPE_REGION_SECOND << TABLE_TYPE_SHIFT | TABLE_LENGTH_FULL;

impl Format for S390x {
    const LEVELS: usize = 4;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 1;
    const ENCODES_MEMORY_TYPE: bool = false;
    const USER_END: u64 = 1 << 52;

    fn root(virt: u64) -> Option<usize> {
        (virt >> 53 == 0).then_some(0)
    }

    fn index_shift(level: usize) -> u32 {
        match level {
            0 => 12,
            1 => 20,
            2 => 31,
            _ => 42,
        }
    }

    fn entries(level: usize) -> usize {
        match level {
            0 => 256,
            _ => 2048,
        }
    }

    fn table_order(level: usize) -> u32 {
        match level {
            0 => 0,
            _ => 2,
        }
    }

    fn empty(level: usize) -> u64 {
        match level {
            0 => PAGE_INVALID,
            1 => INVALID_REGION,
            2 => INVALID_REGION | TYPE_REGION_THIRD << TABLE_TYPE_SHIFT,
            _ => INVALID_REGION | TYPE_REGION_SECOND << TABLE_TYPE_SHIFT,
        }
    }

    fn table(table: u64, level: usize, _user: bool) -> u64 {
        let kind = match level {
            3 => TYPE_REGION_SECOND,
            2 => TYPE_REGION_THIRD,
            _ => TYPE_SEGMENT,
        };
        match level {
            1 => table & PAGE_TABLE_ORIGIN,
            _ => table & REGION_ORIGIN | kind << TABLE_TYPE_SHIFT | TABLE_LENGTH_FULL,
        }
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        phys & PAGE_FRAME | PAGE_INVALID | SOFT_KEPT
    }

    fn decode(entry: u64, level: usize) -> Entry {
        match level {
            0 => match (entry & PAGE_INVALID != 0, entry & SOFT_KEPT != 0) {
                (true, true) => Entry::Kept(entry & PAGE_FRAME),
                (true, false) => Entry::Empty,
                (false, _) => Entry::Leaf(
                    entry & PAGE_FRAME,
                    Attributes {
                        writable: entry & PAGE_PROTECT == 0,
                        executable: entry & PAGE_NO_EXECUTE == 0,
                        memory_type: MemoryType::Normal,
                        user: entry & SOFT_USER != 0,
                    },
                ),
            },
            _ if entry == 0 || entry & INVALID_REGION != 0 => Entry::Empty,
            1 if entry & LARGE != 0 => Entry::Leaf(
                entry & LARGE_FRAME,
                Attributes {
                    writable: entry & SEGMENT_PROTECT == 0,
                    executable: entry & SEGMENT_NO_EXECUTE == 0,
                    memory_type: MemoryType::Normal,
                    user: false,
                },
            ),
            1 => Entry::Table(entry & PAGE_TABLE_ORIGIN),
            _ => Entry::Table(entry & REGION_ORIGIN),
        }
    }
}

/// [`Format::leaf`], usable in constants: a page at level 0, a 1 MiB segment at level 1
/// (kernel mappings only: a segment leaf records no owner).
pub const fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
    match level {
        0 => {
            let mut entry = phys & PAGE_FRAME;
            if !attributes.writable {
                entry |= PAGE_PROTECT;
            } else {
                entry |= SOFT_WRITABLE;
            }
            if !attributes.executable {
                entry |= PAGE_NO_EXECUTE;
            }
            if attributes.user {
                entry |= SOFT_USER;
            }
            entry
        }
        _ => {
            let mut entry = phys & LARGE_FRAME | LARGE;
            if !attributes.writable {
                entry |= SEGMENT_PROTECT;
            }
            if !attributes.executable {
                entry |= SEGMENT_NO_EXECUTE;
            }
            entry
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_geometry_is_z_architecture_s() {
        assert_eq!(S390x::index(0x0000_0400_0000_0000, 3), 1);
        assert_eq!(S390x::index(0x8000_0000, 2), 1);
        assert_eq!(S390x::index(0x10_0000, 1), 1);
        assert_eq!(S390x::index(0x1000, 0), 1);
        assert_eq!(S390x::index(0xff000, 0), 255);
        assert_eq!(S390x::leaf_bytes(1), 1 << 20);
        assert_eq!(S390x::table_order(3), 2);
    }

    #[test]
    fn pages_tables_and_kept_pages_decode_as_written() {
        let page = S390x::leaf(0x1234_5000, 0, Attributes::USER_READ_WRITE);
        assert_eq!(
            S390x::decode(page, 0),
            Entry::Leaf(0x1234_5000, Attributes::USER_READ_WRITE)
        );
        assert_eq!(S390x::decode(S390x::kept(0x7000), 0), Entry::Kept(0x7000));
        assert_eq!(
            S390x::decode(S390x::table(0x8000, 1, true), 1),
            Entry::Table(0x8000)
        );
        assert_eq!(
            S390x::decode(S390x::table(0x10_0000, 3, false), 3),
            Entry::Table(0x10_0000)
        );
        assert_eq!(S390x::decode(0, 2), Entry::Empty);
        assert_eq!(S390x::decode(S390x::empty(0), 0), Entry::Empty);
        assert_eq!(S390x::decode(S390x::empty(3), 3), Entry::Empty);
        assert_eq!(S390x::root(1 << 53), None);
    }
}
