//! RISC-V Sv32 (privileged specification, 12.3; D474): RV32's two levels translating 32-bit
//! addresses into 34-bit physical ones through tables of 1,024 four-byte entries, each level
//! resolving 10 address bits. Leaves are 4 KiB pages and 4 MiB megapages. The entry's
//! permission and status bits are Sv39's; its physical page number is 22 bits. The kernel
//! sits in the top gigabyte, users below.

use super::{Attributes, Entry, Format};

/// The Sv32 format. See the module documentation.
#[derive(Debug)]
pub struct Riscv32Sv32;

/// `satp.MODE` for Sv32 (bit 31 of the 32-bit `satp`).
pub const SATP_MODE_SV32: u32 = 1 << 31;
/// The flags of the boot table's megapages: valid, read, write, execute, accessed and dirty.
pub const BOOT_MEGAPAGE: u32 = 0xcf;

const VALID: u64 = 1 << 0;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
const KEPT: u64 = 1 << 8;
const PPN_SHIFT: u32 = 10;
const PPN: u64 = ((1 << 22) - 1) << PPN_SHIFT;
const ENTRIES: usize = 1024;
const INDEX_BITS: u32 = 10;

fn number(phys: u64) -> u64 {
    ((phys >> 12) << PPN_SHIFT) & PPN
}

fn address(entry: u64) -> u64 {
    ((entry & PPN) >> PPN_SHIFT) << 12
}

impl Format for Riscv32Sv32 {
    const LEVELS: usize = 2;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 1;
    const ENCODES_MEMORY_TYPE: bool = false;
    /// The low 3 GiB.
    const USER_END: u64 = 0xc000_0000;
    const ENTRY_BYTES: usize = 4;

    fn root(virt: u64) -> Option<usize> {
        (virt >> 32 == 0).then_some(0)
    }

    fn table(table: u64, _level: usize, _user: bool) -> u64 {
        number(table) | VALID
    }

    fn leaf(phys: u64, _level: usize, attributes: Attributes) -> u64 {
        let mut entry = number(phys) | VALID | READ | ACCESSED | DIRTY;
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

    fn kept(phys: u64) -> u64 {
        number(phys) | KEPT
    }

    fn index_shift(level: usize) -> u32 {
        12 + INDEX_BITS * level as u32
    }

    fn entries(_level: usize) -> usize {
        ENTRIES
    }

    fn decode(entry: u64, _level: usize) -> Entry {
        if entry & VALID == 0 {
            return match entry & KEPT != 0 {
                true => Entry::Kept(address(entry)),
                false => Entry::Empty,
            };
        }
        if entry & (READ | WRITE | EXECUTE) == 0 {
            return Entry::Table(address(entry));
        }
        Entry::Leaf(
            address(entry),
            Attributes {
                writable: entry & WRITE != 0,
                executable: entry & EXECUTE != 0,
                memory_type: super::MemoryType::Normal,
                user: entry & USER != 0,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_take_ten_address_bits_a_level_and_a_22_bit_page_number() {
        assert_eq!(Riscv32Sv32::index(0xc040_3000, 1), 0x301);
        assert_eq!(Riscv32Sv32::index(0xc040_3000, 0), 3);
        assert_eq!(Riscv32Sv32::leaf_bytes(1), 4 << 20);
        assert_eq!(
            Riscv32Sv32::leaf(0x8020_0000, 0, Attributes::READ_WRITE),
            0x2008_00c7
        );
        assert_eq!(Riscv32Sv32::table(0x3_ffff_f000, 1, false) >> 10, 0x3f_ffff);
    }

    #[test]
    fn entries_round_trip_within_34_bit_physical_addresses() {
        for (phys, level) in [(0x2_3456_7000, 0), (0x3_ff40_0000, 1)] {
            let kinds = [Attributes::READ_WRITE, Attributes::USER_READ_WRITE, Attributes::TEXT];
            for attributes in kinds {
                assert_eq!(
                    Riscv32Sv32::decode(Riscv32Sv32::leaf(phys, level, attributes), level),
                    Entry::Leaf(phys, attributes)
                );
            }
        }
        for phys in [0, 0x1234_5000, 0x3_ffff_f000] {
            assert_eq!(Riscv32Sv32::decode(Riscv32Sv32::kept(phys), 0), Entry::Kept(phys));
        }
        assert_eq!(Riscv32Sv32::decode(0, 0), Entry::Empty);
    }
}
