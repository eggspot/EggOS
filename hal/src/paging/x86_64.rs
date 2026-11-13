//! x86_64 4-level paging (Intel SDM vol. 3A, 4.5; AMD APM vol. 2, 5.3): PML4, PDPT, page
//! directory, page table. Leaves are 4 KiB pages, 2 MiB pages and, where CPUID's `Page1GB`
//! (leaf 0x8000_0001, EDX bit 26) says the CPU has them, 1 GiB pages (D552).
//!
//! Read-only kernel mappings need `CR0.WP` and no-execute needs `EFER.NXE`; the x86_64 module
//! sets both before loading tables built with this format.

use super::{Attributes, Entry, Format, MemoryType};
use core::sync::atomic::{AtomicU8, Ordering};
use crate::memory::FRAME_SIZE;

/// The 4-level format. See the module documentation.
#[derive(Debug)]
pub struct X86_64;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
/// User/supervisor: user mode may access what the walk maps, if every level along it allows.
const USER: u64 = 1 << 2;
/// `IA32_PAT` as the kernel programs it on every CPU (D245): PAT entries 0-3 (and 4-7) are
/// write-back, write-combining, uncached-minus and uncached, so `PWT` alone selects
/// write-combining and `PWT | PCD` uncached, as Linux programs it.
pub const PAT: u64 = 0x0007_0106_0007_0106;
/// Page-level write-through and cache disable: with [`PAT`], PWT | PCD selects UC.
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
/// In an entry without [`PRESENT`], whose other bits the CPU ignores: [`Format::kept`]'s mark.
const KEPT: u64 = 1 << 9;

/// Global: the translation survives CR3 loads ([`set_global_kernel_pages`]).
const GLOBAL: u64 = 1 << 8;

/// Whether kernel leaves are made global: the HAL turns it on with `CR4.PGE` on CPUs that need
/// no separate user view (D552), and off again when one is set up.
static GLOBAL_KERNEL_PAGES: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Makes kernel leaves built from now on global (or not).
pub fn set_global_kernel_pages(global: bool) {
    GLOBAL_KERNEL_PAGES.store(global, Ordering::Relaxed);
}

const UNKNOWN: u8 = 0;
/// The highest leaf level the CPU takes, found at the first use ([`Format::max_leaf_level`]).
static GIGABYTE_LEAVES: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Whether CPUID reports 1 GiB pages (`Page1GB`, leaf 0x8000_0001 EDX bit 26). On a host that is
/// not x86_64 (the format's host tests), no.
fn has_gigabyte_pages() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::__cpuid;
        let highest = __cpuid(0x8000_0000).eax;
        highest >= 0x8000_0001 && __cpuid(0x8000_0001).edx & 1 << 26 != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

impl Format for X86_64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = true;
    /// The lower 128 TiB but its last page. A `syscall` in that page's last bytes would return
    /// (`sysretq`) to the first non-canonical address, which faults in ring 0 on the user's
    /// stack on Intel CPUs; with the page never mapped, no user instruction runs there.
    const USER_END: u64 = (1 << 47) - FRAME_SIZE;

    fn max_leaf_level() -> usize {
        match GIGABYTE_LEAVES.load(Ordering::Relaxed) {
            UNKNOWN => {
                let level = if has_gigabyte_pages() { 2 } else { 1 };
                GIGABYTE_LEAVES.store(level, Ordering::Relaxed);
                usize::from(level)
            }
            level => usize::from(level),
        }
    }

    fn root(virt: u64) -> Option<usize> {
        (((virt as i64) << 16 >> 16) as u64 == virt).then_some(0)
    }

    fn table(table: u64, _level: usize, user: bool) -> u64 {
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
        match attributes.memory_type {
            MemoryType::Device => entry |= WRITE_THROUGH | CACHE_DISABLE,
            MemoryType::WriteCombining => entry |= WRITE_THROUGH,
            MemoryType::Normal => {}
        }
        if attributes.user {
            entry |= USER;
        } else if GLOBAL_KERNEL_PAGES.load(Ordering::Relaxed) {
            entry |= GLOBAL;
        }
        entry
    }

    fn kept(phys: u64) -> u64 {
        phys & ADDRESS | KEPT
    }

    fn decode(entry: u64, level: usize) -> Entry {
        if entry & PRESENT == 0 {
            return if entry & KEPT != 0 {
                Entry::Kept(entry & ADDRESS)
            } else {
                Entry::Empty
            };
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
        let memory_type = match (entry & WRITE_THROUGH != 0, entry & CACHE_DISABLE != 0) {
            (false, false) => MemoryType::Normal,
            (true, false) => MemoryType::WriteCombining,
            _ => MemoryType::Device,
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
        assert_eq!(X86_64::table(0x1000, 3, true), 0x1007);
        assert_eq!(
            X86_64::leaf(0x20_0000, 0, Attributes::USER_READ_WRITE),
            0x8000_0000_0020_0067
        );
        assert_eq!(
            X86_64::leaf(0x20_0000, 0, Attributes::READ_WRITE),
            0x8000_0000_0020_0063
        );
        assert_eq!(X86_64::leaf(0x20_0000, 1, Attributes::TEXT), 0x20_00e1);
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
        assert!(matches!(X86_64::decode(0x1083, 0), Entry::Leaf(0x1000, _)));
    }
}
