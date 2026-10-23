//! AArch64 VMSAv8-64 stage 1 translation with the 4 KiB granule and 48-bit addresses (Arm ARM
//! D8.3): four levels, `TTBR0_EL1` translating the low half and `TTBR1_EL1` the high half.
//! Leaves are 4 KiB pages (level 3 in Arm's numbering), 2 MiB and 1 GiB blocks.
//!
//! Memory types are indexes into `MAIR_EL1`; the AArch64 module loads [`MAIR`].

use super::{Attributes, Entry, Format, MemoryType};

/// The 4 KiB-granule, 48-bit format. See the module documentation.
#[derive(Debug)]
pub struct Aarch64;

/// `MAIR_EL1` for the attribute indexes below: index 0 Device-nGnRnE (0x00), index 1 Normal
/// memory, inner and outer write-back, read- and write-allocate (0xff).
pub const MAIR: u64 = (MAIR_NORMAL << (8 * NORMAL_INDEX)) | (MAIR_DEVICE << (8 * DEVICE_INDEX));
const MAIR_NORMAL: u64 = 0xff;
const MAIR_DEVICE: u64 = 0x00;
const DEVICE_INDEX: u64 = 0;
const NORMAL_INDEX: u64 = 1;

const VALID: u64 = 1 << 0;
/// Bit 1: at the last level a page, above it a table (set) or a block (clear).
const PAGE_OR_TABLE: u64 = 1 << 1;
const ATTRIBUTE_INDEX_SHIFT: u32 = 2;
const ATTRIBUTE_INDEX: u64 = 0b111 << ATTRIBUTE_INDEX_SHIFT;
/// `AP[1]`: EL0 may access the page too (a user mapping); clear for the kernel's.
const USER: u64 = 1 << 6;
/// `AP[2]`: read-only, at EL0 and EL1 alike.
const READ_ONLY: u64 = 1 << 7;
/// Inner shareable, as every CPU of a coherent system shares RAM.
const INNER_SHAREABLE: u64 = 0b11 << 8;
/// The access flag, set up front so the first access does not fault.
const ACCESS_FLAG: u64 = 1 << 10;
/// Not global: the TLB tags the translation with the address space's ASID, so switching
/// address spaces drops user translations alone (`TLBI ASIDE1`) and keeps the kernel's.
const NOT_GLOBAL: u64 = 1 << 11;
const PRIVILEGED_NO_EXECUTE: u64 = 1 << 53;
const USER_NO_EXECUTE: u64 = 1 << 54;
/// Output address bits 47:12.
const ADDRESS: u64 = 0x0000_ffff_ffff_f000;
/// In an invalid descriptor, whose other bits the MMU ignores: [`Format::kept`]'s mark (a bit
/// left to software in valid ones too).
const KEPT: u64 = 1 << 55;

impl Format for Aarch64 {
    const LEVELS: usize = 4;
    const ROOTS: usize = 2;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = true;
    /// `TTBR0_EL1`'s whole 48-bit half.
    const USER_END: u64 = 1 << 48;

    fn root(virt: u64) -> Option<usize> {
        match virt >> 48 {
            0 => Some(0),
            0xffff => Some(1),
            _ => None,
        }
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        self::table(table)
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        phys & ADDRESS | KEPT
    }

    fn decode(entry: u64, level: usize) -> Entry {
        if entry & VALID == 0 {
            return if entry & KEPT != 0 {
                Entry::Kept(entry & ADDRESS)
            } else {
                Entry::Empty
            };
        }
        let is_page_or_table = entry & PAGE_OR_TABLE != 0;
        if level > 0 && is_page_or_table {
            return Entry::Table(entry & ADDRESS);
        }
        if level == 0 && !is_page_or_table {
            // Reserved encoding at the last level: translates nothing.
            return Entry::Empty;
        }
        let memory_type = if (entry & ATTRIBUTE_INDEX) >> ATTRIBUTE_INDEX_SHIFT == DEVICE_INDEX {
            MemoryType::Device
        } else {
            MemoryType::Normal
        };
        let user = entry & USER != 0;
        let no_execute = if user {
            USER_NO_EXECUTE
        } else {
            PRIVILEGED_NO_EXECUTE
        };
        Entry::Leaf(
            entry & ADDRESS,
            Attributes {
                writable: entry & READ_ONLY == 0,
                executable: entry & no_execute == 0,
                memory_type,
                user,
            },
        )
    }
}

/// [`Format::table`], usable in constants and in the boot code.
pub const fn table(table: u64) -> u64 {
    (table & ADDRESS) | VALID | PAGE_OR_TABLE
}

/// [`Format::leaf`], usable in constants: the boot code's block entries are built from it. The
/// kernel never executes a user page (`PXN`), and user code never a kernel page (`UXN`).
pub const fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
    let mut entry = (phys & ADDRESS) | VALID | ACCESS_FLAG;
    if attributes.user {
        entry |= USER | NOT_GLOBAL | PRIVILEGED_NO_EXECUTE;
        if !attributes.executable {
            entry |= USER_NO_EXECUTE;
        }
    } else {
        entry |= USER_NO_EXECUTE;
        if !attributes.executable {
            entry |= PRIVILEGED_NO_EXECUTE;
        }
    }
    if level == 0 {
        entry |= PAGE_OR_TABLE;
    }
    entry |= match attributes.memory_type {
        MemoryType::Normal => (NORMAL_INDEX << ATTRIBUTE_INDEX_SHIFT) | INNER_SHAREABLE,
        MemoryType::Device => DEVICE_INDEX << ATTRIBUTE_INDEX_SHIFT,
    };
    if !attributes.writable {
        entry |= READ_ONLY;
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn encodings_match_the_architecture() {
        assert_eq!(MAIR, 0xff00);
        assert_eq!(Aarch64::table(0x4000_1000, 3, false), 0x4000_1003);
        assert_eq!(Aarch64::table(0x4000_1000, 3, true), 0x4000_1003);
        // 4 KiB data page: page, AttrIndx 1, inner shareable, AF, PXN, UXN.
        assert_eq!(
            Aarch64::leaf(0x4020_0000, 0, Attributes::READ_WRITE),
            0x0060_0000_4020_0707
        );
        // 2 MiB code block: block, AttrIndx 1, AP[2] read-only, inner shareable, AF, UXN.
        assert_eq!(
            Aarch64::leaf(0x4020_0000, 1, Attributes::TEXT),
            0x0040_0000_4020_0785
        );
        // User data page: page, AttrIndx 1, AP[1] EL0 access, inner shareable, AF, nG, PXN,
        // UXN.
        assert_eq!(
            Aarch64::leaf(0x4020_0000, 0, Attributes::USER_READ_WRITE),
            0x0060_0000_4020_0f47
        );
        // User code page: as data, read-only (AP[2]) and without UXN.
        assert_eq!(
            Aarch64::leaf(0x4020_0000, 0, Attributes::USER_TEXT),
            0x0020_0000_4020_0fc7
        );
        // Device page: AttrIndx 0, AF, PXN, UXN.
        assert_eq!(
            Aarch64::leaf(0x0900_0000, 0, Attributes::DEVICE),
            0x0060_0000_0900_0403
        );
    }

    #[test]
    fn entries_round_trip() {
        round_trips::<Aarch64>();
    }

    #[test]
    fn each_half_has_its_own_root() {
        assert_eq!(Aarch64::root(0x0000_ffff_ffff_ffff), Some(0));
        assert_eq!(Aarch64::root(0xffff_0000_0000_0000), Some(1));
        assert_eq!(Aarch64::root(0x0001_0000_0000_0000), None);
    }

    #[test]
    fn a_block_descriptor_at_the_last_level_is_invalid() {
        assert_eq!(Aarch64::decode(0x4020_0401, 0), Entry::Empty);
    }
}
