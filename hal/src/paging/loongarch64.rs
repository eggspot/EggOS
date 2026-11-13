//! LoongArch64 page tables (LoongArch Reference Manual, volume 1, 5.4 and 7.5; D290): four
//! levels of 512 entries translating 48-bit addresses, walked by the kernel's TLB refill handler
//! with `lddir` and `ldpte` as `PWCL` and `PWCH` describe them. The low half goes through
//! `PGDL` (a program's root) and the high half through `PGDH` (the kernel's), as AArch64's two
//! roots do. Leaves are 4 KiB pages and, at levels 1 and 2, 2 MiB and 1 GiB huge pages, which
//! `lddir` passes down and `ldpte` splits into the TLB's page pair.
//!
//! A directory entry is the next table's physical address and nothing else: the walk never
//! checks validity, so an empty entry (0) leads the walk to physical page 0, which the platform
//! keeps zeroed and never hands out, and the walk ends at an invalid leaf there.

use super::{Attributes, Entry, Format, MemoryType};

/// The LoongArch64 format. See the module documentation.
#[derive(Debug)]
pub struct Loongarch64;

const VALID: u64 = 1 << 0;
const DIRTY: u64 = 1 << 1;
const PLV_SHIFT: u32 = 2;
const PLV_MASK: u64 = 3 << PLV_SHIFT;
const PLV_USER: u64 = 3 << PLV_SHIFT;
const MAT_SHIFT: u32 = 4;
const MAT_MASK: u64 = 3 << MAT_SHIFT;
const MAT_SUC: u64 = 0;
const MAT_CC: u64 = 1 << MAT_SHIFT;
const MAT_WUC: u64 = 2 << MAT_SHIFT;
const GLOBAL: u64 = 1 << 6;
const HUGE: u64 = 1 << 6;
const HUGE_GLOBAL: u64 = 1 << 12;
const NO_EXECUTE: u64 = 1 << 62;
const PHYSICAL: u64 = ((1 << 48) - 1) & !0xfff;
const HUGE_PHYSICAL: u64 = PHYSICAL & !HUGE_GLOBAL;
const KEPT: u64 = 1 << 8;

/// `PWCL` for this format: the page table at bit 12, 9 bits wide, directory 1 at bit 21 and
/// directory 2 at bit 30, each 9 bits wide, 64-bit entries.
pub const PWCL: u64 = 12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25);
/// `PWCH` for this format: directory 3 at bit 39, 9 bits wide; no directory 4.
pub const PWCH: u64 = 39 | (9 << 6);

impl Format for Loongarch64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 2;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = true;
    const USER_END: u64 = 1 << 47;

    fn root(virt: u64) -> Option<usize> {
        match virt >> 47 {
            0 => Some(0),
            0x1_ffff => Some(1),
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
            if entry == 0 {
                return Entry::Empty;
            }
            if entry & HUGE == 0 {
                return Entry::Table(entry & PHYSICAL);
            }
            return Entry::Leaf(entry & HUGE_PHYSICAL, attributes_of(entry));
        }
        if entry & VALID == 0 {
            return match entry & KEPT != 0 {
                true => Entry::Kept(entry & PHYSICAL),
                false => Entry::Empty,
            };
        }
        Entry::Leaf(entry & PHYSICAL, attributes_of(entry))
    }
}

fn attributes_of(entry: u64) -> Attributes {
    Attributes {
        writable: entry & DIRTY != 0,
        executable: entry & NO_EXECUTE == 0,
        memory_type: match entry & MAT_MASK {
            MAT_SUC => MemoryType::Device,
            MAT_WUC => MemoryType::WriteCombining,
            _ => MemoryType::Normal,
        },
        user: entry & PLV_MASK == PLV_USER,
    }
}

/// [`Format::leaf`], usable in constants.
pub const fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
    let mut entry = VALID
        | match attributes.memory_type {
            MemoryType::Normal => MAT_CC,
            MemoryType::Device => MAT_SUC,
            MemoryType::WriteCombining => MAT_WUC,
        };
    if attributes.writable {
        entry |= DIRTY;
    }
    if !attributes.executable {
        entry |= NO_EXECUTE;
    }
    if attributes.user {
        entry |= PLV_USER;
    }
    match level {
        0 => {
            entry |= phys & PHYSICAL;
            if !attributes.user {
                entry |= GLOBAL;
            }
        }
        _ => {
            entry |= phys & HUGE_PHYSICAL | HUGE;
            if !attributes.user {
                entry |= HUGE_GLOBAL;
            }
        }
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn entries_round_trip() {
        round_trips::<Loongarch64>();
    }

    #[test]
    fn encodings_match_the_manual() {
        assert_eq!(
            Loongarch64::leaf(0x20_0000, 0, Attributes::READ_WRITE),
            0x4000_0000_0020_0053
        );
        assert_eq!(
            Loongarch64::leaf(0x20_0000, 0, Attributes::USER_TEXT),
            0x20_001d
        );
        assert_eq!(
            Loongarch64::leaf(0x4000_0000, 2, Attributes::TEXT),
            0x4000_1051
        );
        assert_eq!(Loongarch64::table(0x1234_5000, 3, false), 0x1234_5000);
        assert_eq!(Loongarch64::root(0xffff_8000_0000_0000), Some(1));
        assert_eq!(Loongarch64::root(0x0000_7fff_ffff_ffff), Some(0));
        assert_eq!(Loongarch64::root(0x0000_8000_0000_0000), None);
    }
}
