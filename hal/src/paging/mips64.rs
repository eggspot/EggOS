//! MIPS64 page tables (D313). MIPS has no hardware walker: the TLB refill handler
//! (`arch/mips64/exception.rs`) walks four levels of 512 entries over 48-bit addresses and loads
//! the even and odd pages of a pair into `EntryLo0` and `EntryLo1`, so the format is the
//! kernel's to choose:
//!
//! - a directory entry is the next table's address in the cached `XKPHYS` window, which the
//!   handler loads from without converting it; an empty entry is 0, where the walk stops and an
//!   invalid pair is written, so the access raises a TLB-invalid exception;
//! - a leaf is a 4 KiB page in `EntryLo`'s own format (the page frame at bit 6, the cache
//!   attribute, `D` for writable, `V`, `G` for the kernel's, `RI` and `XI`), plus two software
//!   bits in the `Fill` bits the hardware drops on write: the page is a program's, and
//!   [`Format::kept`]'s mark.
//!
//! The low half (`XUSEG`, a program's) goes through root 0, which `KScratch1` holds; everything
//! with bit 63 set through root 1, the kernel's, whose entries only the mapped kernel segment
//! (`XKSEG`, the device window) needs: `XKPHYS` and `CKSEG0` translate without the TLB, so the
//! direct map and the image the kernel also enters there are never walked. Only 4 KiB leaves:
//! the handler loads no large pages.

use super::{Attributes, Entry, Format, MemoryType};

/// The MIPS64 format. See the module documentation.
#[derive(Debug)]
pub struct Mips64;

const GLOBAL: u64 = 1 << 0;
const VALID: u64 = 1 << 1;
const DIRTY: u64 = 1 << 2;
const CACHE_SHIFT: u32 = 3;
const CACHE_MASK: u64 = 7 << CACHE_SHIFT;
/// Uncached: device registers.
const UNCACHED: u64 = 2 << CACHE_SHIFT;
/// Cacheable, coherent: RAM.
const CACHED: u64 = 3 << CACHE_SHIFT;
/// Uncached accelerated: write-combining.
const UNCACHED_ACCELERATED: u64 = 7 << CACHE_SHIFT;
const FRAME_SHIFT: u32 = 6;
/// The page frame field: physical bits 47:12 at bits 41:6.
const FRAME_FIELD: u64 = ((1 << 36) - 1) << FRAME_SHIFT;
const USER: u64 = 1 << 60;
const KEPT: u64 = 1 << 59;
const EXECUTE_INHIBIT: u64 = 1 << 62;
/// The physical address bits a table pointer keeps.
const PHYSICAL: u64 = ((1 << 48) - 1) & !0xfff;

/// The cached `XKPHYS` window: physical address `p` at `CACHED_WINDOW | p`.
pub const CACHED_WINDOW: u64 = 0x9800_0000_0000_0000;

impl Format for Mips64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 2;
    const MAX_LEAF_LEVEL: usize = 0;
    const ENCODES_MEMORY_TYPE: bool = true;
    /// 40 bits: every MIPS64 CPU's `XUSEG` is at least that large.
    const USER_END: u64 = 1 << 40;

    fn root(virt: u64) -> Option<usize> {
        match virt >> 48 {
            0 => Some(0),
            _ if virt >> 63 == 1 => Some(1),
            _ => None,
        }
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        table & PHYSICAL | CACHED_WINDOW
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        (phys >> 12) << FRAME_SHIFT & FRAME_FIELD | KEPT
    }

    fn decode(entry: u64, level: usize) -> Entry {
        if level > 0 {
            return match entry {
                0 => Entry::Empty,
                table => Entry::Table(table & PHYSICAL),
            };
        }
        let phys = (entry & FRAME_FIELD) >> FRAME_SHIFT << 12;
        if entry & VALID == 0 {
            return match entry & KEPT != 0 {
                true => Entry::Kept(phys),
                false => Entry::Empty,
            };
        }
        Entry::Leaf(
            phys,
            Attributes {
                writable: entry & DIRTY != 0,
                executable: entry & EXECUTE_INHIBIT == 0,
                memory_type: match entry & CACHE_MASK {
                    UNCACHED => MemoryType::Device,
                    UNCACHED_ACCELERATED => MemoryType::WriteCombining,
                    _ => MemoryType::Normal,
                },
                user: entry & USER != 0,
            },
        )
    }
}

/// [`Format::leaf`], usable in constants. Only level 0 holds leaves here.
pub const fn leaf(phys: u64, _level: usize, attributes: Attributes) -> u64 {
    let mut entry = VALID
        | ((phys >> 12) << FRAME_SHIFT & FRAME_FIELD)
        | match attributes.memory_type {
            MemoryType::Normal => CACHED,
            MemoryType::Device => UNCACHED,
            MemoryType::WriteCombining => UNCACHED_ACCELERATED,
        };
    if attributes.writable {
        entry |= DIRTY;
    }
    if !attributes.executable {
        entry |= EXECUTE_INHIBIT;
    }
    match attributes.user {
        true => entry |= USER,
        false => entry |= GLOBAL,
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn entries_round_trip() {
        round_trips::<Mips64>();
    }

    #[test]
    fn encodings_match_entrylo() {
        assert_eq!(Mips64::leaf(0x20_0000, 0, Attributes::READ_WRITE), 0x4000_0000_0000_801f);
        assert_eq!(Mips64::leaf(0x20_0000, 0, Attributes::USER_TEXT), 0x1000_0000_0000_801a);
        assert_eq!(Mips64::table(0x1234_5000, 3, false), 0x9800_0000_1234_5000);
        assert_eq!(Mips64::root(0xc000_ff00_0000_0000), Some(1));
        assert_eq!(Mips64::root(0x0000_00ff_ffff_ffff), Some(0));
        assert_eq!(Mips64::root(0x4000_0000_0000_0000), None);
    }
}
