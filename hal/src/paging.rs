//! Page-table entry formats: how each architecture encodes a translation.
//!
//! Every supported architecture translates virtual addresses through a radix tree of 4 KiB
//! tables of 512 eight-byte entries, each level resolving 9 bits of the address above the
//! 12-bit page offset. Only the encoding of an entry, the number of levels, which levels may
//! hold a large-page leaf and which addresses are valid differ; a [`Format`] describes exactly
//! that, and the kernel's architecture-independent page-table code builds every architecture's
//! tables with it (D29).
//!
//! The formats are plain bit encodings, compiled on every target so the host unit tests check
//! each architecture's encoding.

pub mod aarch64;
pub mod riscv64;
pub mod x86_64;

use crate::memory::PhysicalRange;

/// Entries in one table: a 4 KiB table of 8-byte entries.
pub const ENTRIES: usize = 512;

/// Bytes a leaf at `level` maps: 4 KiB at level 0, 2 MiB at level 1, 1 GiB at level 2, ...
pub const fn leaf_size(level: usize) -> u64 {
    1 << (12 + 9 * level)
}

/// The index of `virt` in a table at `level` (0 is the last level, whose entries map 4 KiB).
pub const fn index(virt: u64, level: usize) -> usize {
    ((virt >> (12 + 9 * level)) as usize) % ENTRIES
}

/// What a mapping is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// RAM: cached, and accesses may be merged, reordered or speculated.
    Normal,
    /// Device registers: uncached, accesses happen exactly as the program issues them.
    Device,
}

/// The permissions and memory type of a kernel mapping. Kernel mappings are never accessible
/// to user mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    pub writable: bool,
    pub executable: bool,
    pub memory_type: MemoryType,
}

impl Attributes {
    /// Code: read and execute.
    pub const TEXT: Self = Self {
        writable: false,
        executable: true,
        memory_type: MemoryType::Normal,
    };
    /// Constants: read only.
    pub const READ_ONLY: Self = Self {
        writable: false,
        executable: false,
        memory_type: MemoryType::Normal,
    };
    /// Data: read and write, never execute.
    pub const READ_WRITE: Self = Self {
        writable: true,
        executable: false,
        memory_type: MemoryType::Normal,
    };
    /// Device registers: read and write, never execute.
    pub const DEVICE: Self = Self {
        writable: true,
        executable: false,
        memory_type: MemoryType::Device,
    };
}

/// Where the parts of the kernel image are in physical memory, from the linker script, and
/// where it runs: physical address `p` of the image at virtual address `p + virtual_offset`
/// (wrapping), in the top 2 GiB of the address space. Every boundary is 4 KiB-aligned; the
/// parts follow each other in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelImage {
    /// The image's virtual address minus its physical address (wrapping).
    pub virtual_offset: u64,
    /// The first byte of the image: anything before the code (the PVH note on x86_64).
    pub start: u64,
    pub text_start: u64,
    pub rodata_start: u64,
    /// `.data`, then `.bss`.
    pub data_start: u64,
    /// The page below the boot stack, left unmapped.
    pub stack_guard_start: u64,
    pub stack_start: u64,
    /// The end of the boot stack, which ends the image.
    pub end: u64,
}

impl KernelImage {
    /// Every part to map with the access it needs: code read and execute, the rest never
    /// executable, and only data and the stack writable. The stack guard page is not among them.
    pub fn mapped_parts(&self) -> [(PhysicalRange, Attributes); 5] {
        let range = |start, end| PhysicalRange { start, end };
        [
            (range(self.start, self.text_start), Attributes::READ_ONLY),
            (range(self.text_start, self.rodata_start), Attributes::TEXT),
            (
                range(self.rodata_start, self.data_start),
                Attributes::READ_ONLY,
            ),
            (
                range(self.data_start, self.stack_guard_start),
                Attributes::READ_WRITE,
            ),
            (range(self.stack_start, self.end), Attributes::READ_WRITE),
        ]
    }

    /// The virtual address the kernel runs physical address `phys` of its image at.
    pub const fn virtual_address(&self, phys: u64) -> u64 {
        phys.wrapping_add(self.virtual_offset)
    }

    /// The unmapped page below the boot stack.
    pub const fn stack_guard(&self) -> PhysicalRange {
        PhysicalRange {
            start: self.stack_guard_start,
            end: self.stack_start,
        }
    }
}

/// One decoded page-table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Translates nothing.
    Empty,
    /// Points to the next-level table at this physical address.
    Table(u64),
    /// Maps [`leaf_size`] bytes starting at this physical address.
    Leaf(u64, Attributes),
}

/// One architecture's page-table format. See the module documentation.
pub trait Format {
    /// Levels of tables a walk goes through, root included.
    const LEVELS: usize;
    /// How many trees translate the address space: 2 on AArch64, where the low and the high
    /// half have their own root (`TTBR0_EL1` and `TTBR1_EL1`), 1 elsewhere.
    const ROOTS: usize;
    /// The highest level that may hold a leaf (0: 4 KiB pages only, 1: also 2 MiB, 2: also
    /// 1 GiB).
    const MAX_LEAF_LEVEL: usize;
    /// Whether an entry records its [`MemoryType`]; see [`Format::decode`].
    const ENCODES_MEMORY_TYPE: bool;

    /// Which root translates `virt`; `None` if `virt` is not a valid (canonical) address.
    fn root(virt: u64) -> Option<usize>;

    /// An entry at `level` (at least 1) pointing to the next-level table at physical `table`.
    fn table(table: u64, level: usize) -> u64;

    /// A leaf entry at `level` mapping [`leaf_size`]`(level)` bytes at physical `phys`, which is
    /// aligned to that size.
    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64;

    /// Decodes an entry found at `level`. A leaf's attributes are what the hardware will
    /// enforce; where an architecture cannot express a [`MemoryType`] in the entry (RISC-V
    /// without Svpbmt), it decodes as [`MemoryType::Normal`].
    fn decode(entry: u64, level: usize) -> Entry;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Checks that every leaf level and attribute combination round-trips through `F`.
    pub(crate) fn round_trips<F: Format>() {
        let types = [MemoryType::Normal, MemoryType::Device];
        for level in 0..=F::MAX_LEAF_LEVEL {
            let phys = 0x1234 * leaf_size(level);
            for writable in [false, true] {
                for executable in [false, true] {
                    for memory_type in types {
                        let attributes = Attributes {
                            writable,
                            executable,
                            memory_type,
                        };
                        let mut expected = attributes;
                        if !F::ENCODES_MEMORY_TYPE {
                            expected.memory_type = MemoryType::Normal;
                        }
                        assert_eq!(
                            F::decode(F::leaf(phys, level, attributes), level),
                            Entry::Leaf(phys, expected),
                            "level {level}, {attributes:?}"
                        );
                    }
                }
            }
        }
        for level in 1..F::LEVELS {
            assert_eq!(
                F::decode(F::table(0x5678_9000, level), level),
                Entry::Table(0x5678_9000)
            );
        }
        for level in 0..F::LEVELS {
            assert_eq!(F::decode(0, level), Entry::Empty);
        }
    }

    #[test]
    fn index_and_leaf_size() {
        assert_eq!(leaf_size(0), 4096);
        assert_eq!(leaf_size(1), 2 << 20);
        assert_eq!(leaf_size(2), 1 << 30);
        let virt = (3 << 39) | (5 << 30) | (7 << 21) | (9 << 12) | 0xabc;
        assert_eq!([3, 2, 1, 0].map(|level| index(virt, level)), [3, 5, 7, 9]);
        assert_eq!(index(u64::MAX, 3), 511);
    }
}
