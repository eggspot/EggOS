//! x86_64 4-level paging (Intel SDM vol. 3A, 4.5; AMD APM vol. 2, 5.3): PML4, PDPT, page
//! directory, page table. Leaves are 4 KiB pages and 2 MiB pages; 1 GiB pages need CPUID's
//! `Page1GB` and come when the kernel checks for it.
//!
//! Read-only kernel mappings need `CR0.WP` and no-execute needs `EFER.NXE`; the x86_64 module
//! sets both before loading tables built with this format.

use super::{Attributes, Entry, Format, MemoryType};
use crate::memory::FRAME_SIZE;

/// The 4-level format. See the module documentation.
#[derive(Debug)]
pub struct X86_64;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
/// User/supervisor: user mode may access what the walk maps, if every level along it allows.
const USER: u64 = 1 << 2;
/// Page-level write-through and cache disable: with the default PAT, PWT | PCD selects UC.
const WRITE_THROUGH: u64 = 1 << 3;
const CACHE_DISABLE: u64 = 1 << 4;
/// Accessed and dirty, set up front so the CPU never writes them back.
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
/// In a PDPT or page-directory entry: the entry maps a page instead of pointing to a table.
const PAGE_SIZE: u64 = 1 << 7;
const NO_EXECUTE: u64 = 1 << 63;
/// Physical address bits 51:12. In a large-page entry bit 12 is the PAT bit, which is never set.
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const LARGE_PAT: u64 = 1 << 12;

impl Format for X86_64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 1;
    const ENCODES_MEMORY_TYPE: bool = true;
    /// The lower 128 TiB but its last page. A `syscall` in that page's last bytes would return
    /// (`sysretq`) to the first non-canonical address, which faults in ring 0 on the user's
    /// stack on Intel CPUs; with the page never mapped, no user instruction runs there.
    const USER_END: u64 = (1 << 47) - FRAME_SIZE;

    fn root(virt: u64) -> Option<usize> {
        // Canonical: bits 63:48 copy bit 47.
        (((virt as i64) << 16 >> 16) as u64 == virt).then_some(0)
    }

    fn table(table: u64, _level: usize, user: bool) -> u64 {
        // Permissions are decided at the leaf; the table entry allows everything, user access
        // only on the way to user mappings.
        let entry = (table & ADDRESS) | PRESENT | WRITABLE;
        if user { entry | USER } else { entry }
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        let mut entry = (phys & ADDRESS) | PRESENT | ACCESSED | DIRTY;
        if level > 0 {
            entry |= PAGE_SIZE;
        }
        if attributes.writable {
            entry |= WRITABLE;
        }
        if !attributes.executable {
            entry |= NO_EXECUTE;
        }
        if attributes.memory_type == MemoryType::Device {
            entry |= WRITE_THROUGH | CACHE_DISABLE;
        }
        if attributes.user {
            entry |= USER;
        }
        entry
    }

    fn decode(entry: u64, level: usize) -> Entry {
        if entry & PRESENT == 0 {
            return Entry::Empty;
        }
        let is_leaf = level == 0 || (level <= 2 && entry & PAGE_SIZE != 0);
        if !is_leaf {
            return Entry::Table(entry & ADDRESS);
        }
        let address = if level == 0 {
            entry & ADDRESS
        } else {
            entry & ADDRESS & !LARGE_PAT
        };
        let memory_type = if entry & (WRITE_THROUGH | CACHE_DISABLE) != 0 {
            MemoryType::Device
        } else {
            MemoryType::Normal
        };
        Entry::Leaf(
            address,
            Attributes {
                writable: entry & WRITABLE != 0,
                executable: entry & NO_EXECUTE == 0,
                memory_type,
                user: entry & USER != 0,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn encodings_match_the_manuals() {
        assert_eq!(X86_64::table(0x1000, 3, false), 0x1003);
        // On the way to user mappings: P | RW | U/S.
        assert_eq!(X86_64::table(0x1000, 3, true), 0x1007);
        // 4 KiB user data page: P | RW | U/S | A | D | NX.
        assert_eq!(
            X86_64::leaf(0x20_0000, 0, Attributes::USER_READ_WRITE),
            0x8000_0000_0020_0067
        );
        // 4 KiB data page: P | RW | A | D | NX.
        assert_eq!(
            X86_64::leaf(0x20_0000, 0, Attributes::READ_WRITE),
            0x8000_0000_0020_0063
        );
        // 2 MiB code page: P | A | D | PS.
        assert_eq!(X86_64::leaf(0x20_0000, 1, Attributes::TEXT), 0x20_00e1);
        // Device page: P | RW | PWT | PCD | A | D | NX.
        assert_eq!(
            X86_64::leaf(0xfee0_0000, 0, Attributes::DEVICE),
            0x8000_0000_fee0_007b
        );
    }

    #[test]
    fn entries_round_trip() {
        round_trips::<X86_64>();
    }

    #[test]
    fn canonical_addresses() {
        assert_eq!(X86_64::root(0x0000_7fff_ffff_ffff), Some(0));
        assert_eq!(X86_64::root(0xffff_8000_0000_0000), Some(0));
        assert_eq!(X86_64::root(0x0000_8000_0000_0000), None);
        assert_eq!(X86_64::root(0xfff0_0000_0000_0000), None);
    }

    #[test]
    fn a_page_directory_entry_with_ps_is_a_leaf_and_without_is_a_table() {
        assert_eq!(X86_64::decode(0x1003, 1), Entry::Table(0x1000));
        assert!(matches!(
            X86_64::decode(0x20_0083, 1),
            Entry::Leaf(0x20_0000, _)
        ));
        // At level 0 bit 7 is the PAT bit, not a size bit.
        assert!(matches!(X86_64::decode(0x1083, 0), Entry::Leaf(0x1000, _)));
    }
}
