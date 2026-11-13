//! RISC-V Sv39 (privileged specification, 12.4): three levels translating 39-bit addresses,
//! sign-extended to 64 bits. Leaves are 4 KiB pages, 2 MiB megapages and 1 GiB gigapages.
//!
//! Sv39 is what every RVA profile requires, so it runs on every RISC-V application processor;
//! its upper half (256 GiB) holds the direct map (254 GiB) and the kernel image (D29, D33).
//! Without the Svpbmt extension a page cannot say whether it is device memory: the platform's
//! physical memory attributes decide, so [`super::MemoryType`] is not encoded. With it (the
//! device tree lists `svpbmt`, D552), a leaf's PBMT field, bits 62:61, says: 0 the platform's
//! attributes (normal memory), 1 non-cacheable and idempotent (write-combining), 2 I/O (device).

use super::{Attributes, Entry, Format, MemoryType};
use core::sync::atomic::{AtomicBool, Ordering};

/// The Sv39 format. See the module documentation.
#[derive(Debug)]
pub struct Riscv64Sv39;

/// `satp.MODE` for Sv39.
pub const SATP_MODE_SV39: u64 = 8 << 60;

const VALID: u64 = 1 << 0;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
/// U-mode may access the page; S-mode then only with `sstatus.SUM` set, and never executes it.
const USER: u64 = 1 << 4;
/// Accessed and dirty, set up front so the hardware never needs to set them (or fault).
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
const PPN_SHIFT: u32 = 10;
/// The physical page number, bits 53:10.
const PPN: u64 = ((1 << 44) - 1) << PPN_SHIFT;
/// In an entry without [`VALID`], whose other bits the hart ignores: [`Format::kept`]'s mark
/// (one of the bits left to software, RSW, in valid ones).
const KEPT: u64 = 1 << 8;
/// Svpbmt's page-based memory type field, bits 62:61.
const PBMT_SHIFT: u32 = 61;
const PBMT: u64 = 3 << PBMT_SHIFT;
const PBMT_NONCACHEABLE: u64 = 1 << PBMT_SHIFT;
const PBMT_IO: u64 = 2 << PBMT_SHIFT;

/// Whether the harts have Svpbmt; the HAL sets it from the device tree before it builds tables
/// ([`set_svpbmt`]).
static SVPBMT: AtomicBool = AtomicBool::new(false);

/// Records whether every hart has Svpbmt (the device tree's `svpbmt`), before any table is built
/// with this format; entries then carry their memory type.
pub fn set_svpbmt(present: bool) {
    SVPBMT.store(present, Ordering::Relaxed);
}

impl Format for Riscv64Sv39 {
    const LEVELS: usize = 3;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = false;
    /// The lower 256 GiB.
    const USER_END: u64 = 1 << 38;

    fn root(virt: u64) -> Option<usize> {
        (((virt as i64) << 25 >> 25) as u64 == virt).then_some(0)
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        ((table >> 12) << PPN_SHIFT) & PPN | VALID
    }

    fn encodes_memory_type() -> bool {
        SVPBMT.load(Ordering::Relaxed)
    }

    fn leaf(phys: u64, _level: usize, attributes: Attributes) -> u64 {
        let entry = leaf(phys, attributes);
        if !Self::encodes_memory_type() {
            return entry;
        }
        match attributes.memory_type {
            MemoryType::Normal => entry,
            MemoryType::WriteCombining => entry | PBMT_NONCACHEABLE,
            MemoryType::Device => entry | PBMT_IO,
        }
    }

    fn kept(phys: u64) -> u64 {
        ((phys >> 12) << PPN_SHIFT) & PPN | KEPT
    }

    fn decode(entry: u64, _level: usize) -> Entry {
        if entry & VALID == 0 {
            return if entry & KEPT != 0 {
                Entry::Kept(((entry & PPN) >> PPN_SHIFT) << 12)
            } else {
                Entry::Empty
            };
        }
        let phys = ((entry & PPN) >> PPN_SHIFT) << 12;
        if entry & (READ | WRITE | EXECUTE) == 0 {
            return Entry::Table(phys);
        }
        let memory_type = match entry & PBMT {
            PBMT_NONCACHEABLE => MemoryType::WriteCombining,
            PBMT_IO => MemoryType::Device,
            _ => MemoryType::Normal,
        };
        Entry::Leaf(
            phys,
            Attributes {
                writable: entry & WRITE != 0,
                executable: entry & EXECUTE != 0,
                memory_type,
                user: entry & USER != 0,
            },
        )
    }
}

/// [`Format::leaf`] (the same at every level), usable in constants: the boot code's gigapage
/// entries are built from it.
pub const fn leaf(phys: u64, attributes: Attributes) -> u64 {
    let mut entry = ((phys >> 12) << PPN_SHIFT) & PPN | VALID | READ | ACCESSED | DIRTY;
    if attributes.writable {
        entry |= WRITE;
    }
    if attributes.executable {
        entry |= EXECUTE;
    }
    if attributes.user {
        entry |= USER;
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn encodings_match_the_specification() {
        assert_eq!(Riscv64Sv39::table(0x8020_1000, 2, false), 0x2008_0401);
        assert_eq!(Riscv64Sv39::table(0x8020_1000, 2, true), 0x2008_0401);
        // Data page: V | R | W | A | D.
        assert_eq!(
            Riscv64Sv39::leaf(0x8020_0000, 0, Attributes::READ_WRITE),
            0x2008_00c7
        );
        // User data page: V | R | W | U | A | D.
        assert_eq!(
            Riscv64Sv39::leaf(0x8020_0000, 0, Attributes::USER_READ_WRITE),
            0x2008_00d7
        );
        // Code megapage: V | R | X | A | D.
        assert_eq!(
            Riscv64Sv39::leaf(0x8020_0000, 1, Attributes::TEXT),
            0x2008_00cb
        );
    }

    #[test]
    fn entries_round_trip() {
        round_trips::<Riscv64Sv39>();
    }

    #[test]
    fn canonical_addresses() {
        assert_eq!(Riscv64Sv39::root(0x0000_003f_ffff_ffff), Some(0));
        assert_eq!(Riscv64Sv39::root(0xffff_ffc0_0000_0000), Some(0));
        assert_eq!(Riscv64Sv39::root(0x0000_0040_0000_0000), None);
    }
}
