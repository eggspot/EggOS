//! Apple's DART (D544), the IOMMU in front of each coprocessor and many devices: per stream, a
//! translation control register and up to four table bases, two levels of 16 KiB tables of
//! 2048 entries each over 16 KiB pages. An address names its table base in bits 37:36, its
//! first-level entry in bits 35:25 and its second-level entry in bits 24:14. Three generations
//! differ in where those registers are, how a page's physical address sits in an entry, and how
//! the TLB is invalidated. A DART the firmware locked keeps its tables, which the driver then
//! adds to.

use crate::io::RegisterIo;

/// The page size every DART maps.
pub const PAGE_BYTES: u64 = 16 * 1024;
const PAGE_SHIFT: u32 = 14;
const ENTRIES: usize = 2048;
const INDEX_MASK: u64 = (ENTRIES - 1) as u64;
const FIRST_LEVEL_SHIFT: u32 = 25;
const TABLE_BASE_SHIFT: u32 = 36;
const TABLE_BASE_MASK: u64 = 0x3;
const VALID: u64 = 1 << 0;
const T8020_DISABLE_SUBPAGES: u64 = 1 << 1;
const SUBPAGE_END: u64 = 0xfff << 52;
const T8020_ADDRESS: u64 = ((1 << 40) - 1) & !((1 << 14) - 1);
const WIDE_ADDRESS: u64 = ((1 << 40) - 1) & !((1 << 10) - 1);
const WIDE_ADDRESS_SHIFT: u32 = 10;
const BUSY_POLLS: u32 = 100_000;

const T8020_CONFIG: usize = 0x60;
const T8020_CONFIG_LOCK: u32 = 1 << 15;
const T8020_STREAM_SELECT: usize = 0x34;
const T8020_STREAM_COMMAND: usize = 0x20;
const T8020_COMMAND_BUSY: u32 = 1 << 2;
const T8020_COMMAND_INVALIDATE: u32 = 1 << 20;
const T8020_ENABLED_STREAMS: usize = 0xfc;
const T8020_TCR: usize = 0x100;
const T8020_TRANSLATE: u32 = 1 << 7;
const T8020_TTBR: usize = 0x200;
const T8020_TTBR_VALID: u32 = 1 << 31;
const T8020_TTBR_ADDRESS: u32 = (1 << 31) - 1;
const T8020_TTBR_SHIFT: u32 = 12;
const T8020_TABLE_BASES: usize = 4;

const T8110_PROTECT: usize = 0x200;
const T8110_PROTECT_TTBR_TCR: u32 = 1 << 0;
const T8110_TLB_COMMAND: usize = 0x80;
const T8110_TLB_BUSY: u32 = 1 << 31;
const T8110_FLUSH_STREAM: u32 = 1 << 8;
const T8110_ENABLE_STREAMS: usize = 0xc00;
const T8110_TCR: usize = 0x1000;
const T8110_TRANSLATE: u32 = 1 << 0;
const T8110_TTBR: usize = 0x1400;
const T8110_TTBR_VALID: u32 = 1 << 0;
const T8110_TTBR_ADDRESS: u32 = ((1 << 30) - 1) & !0x3;
const T8110_TTBR_SHIFT: u32 = 14;
const T8110_TTBR_ADDRESS_SHIFT: u32 = 2;

/// The DART generations, by their device tree `compatible`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `apple,t8103-dart`: M1's, 32 streams, physical addresses up to 40 bits.
    T8020,
    /// `apple,t6000-dart`: M1 Pro and Max's, entries with wider addresses.
    T6000,
    /// `apple,t8110-dart`: M2's and later, 256 streams, one table base each.
    T8110,
}

impl Kind {
    /// The generation of a DART node whose `compatible` strings `compatible` lists.
    pub fn of_compatible<'a>(compatible: impl IntoIterator<Item = &'a [u8]>) -> Option<Self> {
        compatible.into_iter().find_map(|name| match name {
            b"apple,t8103-dart" | b"apple,t8020-dart" => Some(Self::T8020),
            b"apple,t6000-dart" => Some(Self::T6000),
            b"apple,t8110-dart" => Some(Self::T8110),
            _ => None,
        })
    }

    fn streams(self) -> u16 {
        match self {
            Self::T8020 | Self::T6000 => 32,
            Self::T8110 => 256,
        }
    }

    fn table_bases(self) -> usize {
        match self {
            Self::T8020 | Self::T6000 => T8020_TABLE_BASES,
            Self::T8110 => 1,
        }
    }

    fn entry_address(self, physical: u64) -> u64 {
        match self {
            Self::T8020 => physical & T8020_ADDRESS,
            Self::T6000 | Self::T8110 => {
                (physical >> PAGE_SHIFT << WIDE_ADDRESS_SHIFT) & WIDE_ADDRESS
            }
        }
    }

    fn physical_of(self, entry: u64) -> u64 {
        match self {
            Self::T8020 => entry & T8020_ADDRESS,
            Self::T6000 | Self::T8110 => (entry & WIDE_ADDRESS) >> WIDE_ADDRESS_SHIFT << PAGE_SHIFT,
        }
    }

    fn page_flags(self) -> u64 {
        match self {
            Self::T8020 => SUBPAGE_END | T8020_DISABLE_SUBPAGES | VALID,
            Self::T6000 | Self::T8110 => SUBPAGE_END | VALID,
        }
    }
}

/// Memory for DART tables: zeroed 16 KiB tables at 16 KiB-aligned physical addresses, and
/// the tables the firmware left in place.
///
/// # Safety
/// [`Self::new_table`] must give memory that only the DART and this driver use from then on, the
/// CPU address of [`PAGE_BYTES`] zeroed bytes at the physical address given with it, both aligned
/// to [`PAGE_BYTES`], mapped and valid for volatile reads and writes for as long as the DART lives.
/// [`Self::table_at`] must give the CPU address of the [`PAGE_BYTES`] at `physical`, under the
/// same terms, or `None`.
pub unsafe trait DartMemory {
    /// A new table: its CPU address and its physical address.
    fn new_table(&mut self) -> Option<(usize, u64)>;
    /// The CPU address of the table at `physical`.
    fn table_at(&mut self, physical: u64) -> Option<usize>;
}

/// Why a DART operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DartError {
    /// The stream number is past the DART's.
    NoSuchStream,
    /// No memory for a table.
    NoMemory,
    /// An address, physical or the device's, or a length is not a whole number of pages.
    Unaligned,
    /// A page is already mapped there.
    AlreadyMapped,
    /// The address is past the DART's table bases.
    OutOfRange,
    /// The DART did not finish invalidating its TLB.
    Timeout,
}

/// One stream of a DART, through its registers in `regs`.
#[derive(Debug)]
pub struct Dart<R: RegisterIo, M: DartMemory> {
    regs: R,
    kind: Kind,
    stream: u8,
    memory: M,
    first_level: [Option<usize>; T8020_TABLE_BASES],
    locked: bool,
}

impl<R: RegisterIo, M: DartMemory> Dart<R, M> {
    /// Takes stream `stream` of the DART: enables it, keeps the tables a locked DART (or one the
    /// caller wants kept, `keep_tables`) already has, gives every other table base a new empty
    /// table, and turns translation on unless the firmware locked the DART.
    pub fn new(
        regs: R,
        kind: Kind,
        stream: u8,
        memory: M,
        keep_tables: bool,
    ) -> Result<Self, DartError> {
        if u16::from(stream) >= kind.streams() {
            return Err(DartError::NoSuchStream);
        }
        let mut dart = Self {
            regs,
            kind,
            stream,
            memory,
            first_level: [None; T8020_TABLE_BASES],
            locked: false,
        };
        let bit = 1u32 << (stream % 32);
        match kind {
            Kind::T8020 | Kind::T6000 => {
                dart.locked = dart.regs.read32(T8020_CONFIG) & T8020_CONFIG_LOCK != 0;
                let enabled = dart.regs.read32(T8020_ENABLED_STREAMS);
                dart.regs.write32(T8020_ENABLED_STREAMS, enabled | bit);
            }
            Kind::T8110 => {
                dart.locked = dart.regs.read32(T8110_PROTECT) & T8110_PROTECT_TTBR_TCR != 0;
                let enable = T8110_ENABLE_STREAMS + 4 * usize::from(stream / 32);
                dart.regs.write32(enable, bit);
            }
        }
        for base in 0..kind.table_bases() {
            if dart.locked || keep_tables {
                let held = dart.regs.read32(dart.ttbr_offset(base));
                if let Some(physical) = dart.ttbr_table(held) {
                    dart.first_level[base] = dart.memory.table_at(physical);
                }
            }
            if dart.first_level[base].is_none() && !dart.locked {
                let (table, physical) = dart.memory.new_table().ok_or(DartError::NoMemory)?;
                let value = dart.ttbr_value(physical);
                dart.regs.write32(dart.ttbr_offset(base), value);
                dart.first_level[base] = Some(table);
            }
        }
        if !dart.locked && !keep_tables {
            let enable = match kind {
                Kind::T8020 | Kind::T6000 => T8020_TRANSLATE,
                Kind::T8110 => T8110_TRANSLATE,
            };
            dart.regs.write32(dart.tcr_offset(), enable);
        }
        dart.invalidate()?;
        Ok(dart)
    }

    /// Whether the firmware locked the DART's tables and translation settings.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    fn tcr_offset(&self) -> usize {
        let stream = usize::from(self.stream);
        match self.kind {
            Kind::T8020 | Kind::T6000 => T8020_TCR + 4 * stream,
            Kind::T8110 => T8110_TCR + 4 * stream,
        }
    }

    fn ttbr_offset(&self, base: usize) -> usize {
        let stream = usize::from(self.stream);
        match self.kind {
            Kind::T8020 | Kind::T6000 => T8020_TTBR + 4 * T8020_TABLE_BASES * stream + 4 * base,
            Kind::T8110 => T8110_TTBR + 4 * stream,
        }
    }

    fn ttbr_value(&self, physical: u64) -> u32 {
        match self.kind {
            Kind::T8020 | Kind::T6000 => {
                T8020_TTBR_VALID | (physical >> T8020_TTBR_SHIFT) as u32 & T8020_TTBR_ADDRESS
            }
            Kind::T8110 => {
                T8110_TTBR_VALID
                    | ((physical >> T8110_TTBR_SHIFT) as u32) << T8110_TTBR_ADDRESS_SHIFT
                        & T8110_TTBR_ADDRESS
            }
        }
    }

    fn ttbr_table(&self, value: u32) -> Option<u64> {
        match self.kind {
            Kind::T8020 | Kind::T6000 => (value & T8020_TTBR_VALID != 0)
                .then(|| u64::from(value & T8020_TTBR_ADDRESS) << T8020_TTBR_SHIFT),
            Kind::T8110 => (value & T8110_TTBR_VALID != 0).then(|| {
                u64::from((value & T8110_TTBR_ADDRESS) >> T8110_TTBR_ADDRESS_SHIFT)
                    << T8110_TTBR_SHIFT
            }),
        }
    }

    fn entry(table: usize, index: usize) -> *mut u64 {
        (table as *mut u64).wrapping_add(index)
    }

    fn read_entry(table: usize, index: usize) -> u64 {
        // SAFETY: `table` came from `DartMemory`, whose contract makes it `PAGE_BYTES` of
        // mapped, aligned table memory for as long as the DART lives; `index` is below
        // `ENTRIES`, so the entry lies inside it.
        unsafe { Self::entry(table, index).read_volatile() }
    }

    fn write_entry(table: usize, index: usize, value: u64) {
        // SAFETY: as in `read_entry`; only this driver and the DART touch the table.
        unsafe { Self::entry(table, index).write_volatile(value) }
    }

    fn indices(&self, address: u64) -> Result<(usize, usize, usize), DartError> {
        let base = (address >> TABLE_BASE_SHIFT & TABLE_BASE_MASK) as usize;
        if base >= self.kind.table_bases() || address >> (TABLE_BASE_SHIFT + 2) != 0 {
            return Err(DartError::OutOfRange);
        }
        let first = (address >> FIRST_LEVEL_SHIFT & INDEX_MASK) as usize;
        let second = (address >> PAGE_SHIFT & INDEX_MASK) as usize;
        Ok((base, first, second))
    }

    fn second_level(
        &mut self,
        base: usize,
        first: usize,
        create: bool,
    ) -> Result<usize, DartError> {
        let top = self.first_level[base].ok_or(DartError::OutOfRange)?;
        let held = Self::read_entry(top, first);
        if held & VALID != 0 {
            let physical = self.kind.physical_of(held);
            return self.memory.table_at(physical).ok_or(DartError::NoMemory);
        }
        if !create {
            return Err(DartError::OutOfRange);
        }
        let (table, physical) = self.memory.new_table().ok_or(DartError::NoMemory)?;
        Self::write_entry(top, first, self.kind.entry_address(physical) | VALID);
        Ok(table)
    }

    /// Maps `bytes` from the device's address `address` to physical `physical`, page by page,
    /// then invalidates the stream's TLB. Every argument is a whole number of pages.
    pub fn map(&mut self, address: u64, physical: u64, bytes: u64) -> Result<(), DartError> {
        let aligned = [address, physical, bytes]
            .iter()
            .all(|value| value % PAGE_BYTES == 0);
        if !aligned {
            return Err(DartError::Unaligned);
        }
        let mut done = 0;
        while done < bytes {
            if let Err(error) = self.map_page(address + done, physical + done) {
                self.clear(address, done);
                let _ = self.invalidate();
                return Err(error);
            }
            done += PAGE_BYTES;
        }
        self.invalidate()
    }

    fn map_page(&mut self, address: u64, physical: u64) -> Result<(), DartError> {
        let (base, first, second) = self.indices(address)?;
        let table = self.second_level(base, first, true)?;
        if Self::read_entry(table, second) & VALID != 0 {
            return Err(DartError::AlreadyMapped);
        }
        let entry = self.kind.entry_address(physical) | self.kind.page_flags();
        Self::write_entry(table, second, entry);
        Ok(())
    }

    fn clear(&mut self, address: u64, bytes: u64) {
        let mut done = 0;
        while done < bytes {
            if let Ok((base, first, second)) = self.indices(address + done)
                && let Ok(table) = self.second_level(base, first, false)
            {
                Self::write_entry(table, second, 0);
            }
            done += PAGE_BYTES;
        }
    }

    /// Unmaps `bytes` from the device's address `address`, then invalidates the TLB.
    pub fn unmap(&mut self, address: u64, bytes: u64) -> Result<(), DartError> {
        if address % PAGE_BYTES != 0 || bytes % PAGE_BYTES != 0 {
            return Err(DartError::Unaligned);
        }
        self.clear(address, bytes);
        self.invalidate()
    }

    /// The physical address the device's address `address` reaches; `None` when unmapped.
    pub fn translate(&mut self, address: u64) -> Option<u64> {
        let (base, first, second) = self.indices(address).ok()?;
        let table = self.second_level(base, first, false).ok()?;
        let entry = Self::read_entry(table, second);
        (entry & VALID != 0).then(|| self.kind.physical_of(entry) + address % PAGE_BYTES)
    }

    /// The first device address from `start` on where `bytes` (whole pages) are all unmapped,
    /// within the first table base.
    pub fn find_free(&mut self, start: u64, bytes: u64) -> Option<u64> {
        let limit = 1u64 << TABLE_BASE_SHIFT;
        let mut candidate = start.next_multiple_of(PAGE_BYTES);
        while candidate.checked_add(bytes)? <= limit {
            let mut taken = None;
            let mut offset = 0;
            while offset < bytes {
                if self.translate(candidate + offset).is_some() {
                    taken = Some(offset);
                    break;
                }
                offset += PAGE_BYTES;
            }
            match taken {
                None => return Some(candidate),
                Some(offset) => candidate += offset + PAGE_BYTES,
            }
        }
        None
    }

    fn invalidate(&mut self) -> Result<(), DartError> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let (command, busy) = match self.kind {
            Kind::T8020 | Kind::T6000 => {
                self.regs
                    .write32(T8020_STREAM_SELECT, 1 << (self.stream % 32));
                self.regs
                    .write32(T8020_STREAM_COMMAND, T8020_COMMAND_INVALIDATE);
                (T8020_STREAM_COMMAND, T8020_COMMAND_BUSY)
            }
            Kind::T8110 => {
                let flush = T8110_FLUSH_STREAM | u32::from(self.stream);
                self.regs.write32(T8110_TLB_COMMAND, flush);
                (T8110_TLB_COMMAND, T8110_TLB_BUSY)
            }
        };
        for _ in 0..BUSY_POLLS {
            if self.regs.read32(command) & busy == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(DartError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;
    use std::alloc::{Layout, alloc_zeroed};

    const FIRST_TABLE: u64 = 0x4000_0000;

    #[derive(Debug, Default)]
    struct HostTables(Vec<(u64, usize)>);

    // SAFETY: each table is a leaked, zeroed, 16 KiB-aligned host allocation, valid for the
    // whole test; a small stand-in physical address, which every DART generation's entries
    // hold, names it.
    unsafe impl DartMemory for HostTables {
        fn new_table(&mut self) -> Option<(usize, u64)> {
            let layout = Layout::from_size_align(PAGE_BYTES as usize, PAGE_BYTES as usize).ok()?;
            // SAFETY: the layout's size is non-zero.
            let table = unsafe { alloc_zeroed(layout) } as usize;
            let physical = FIRST_TABLE + self.0.len() as u64 * PAGE_BYTES;
            self.0.push((physical, table));
            Some((table, physical))
        }
        fn table_at(&mut self, physical: u64) -> Option<usize> {
            self.0
                .iter()
                .find_map(|&(held, table)| (held == physical).then_some(table))
        }
    }

    #[test]
    fn a_mapped_range_translates_and_an_overlap_is_refused() {
        let tables = HostTables::default();
        let mut dart = Dart::new(FakeRegs::default(), Kind::T8020, 1, tables, false).unwrap();
        dart.map(0x10_0000, 0x8_0000_0000, 2 * PAGE_BYTES).unwrap();
        assert_eq!(dart.translate(0x10_0000 + 5), Some(0x8_0000_0005));
        assert_eq!(dart.translate(0x10_0000 + PAGE_BYTES), Some(0x8_0000_4000));
        assert_eq!(dart.translate(0x10_0000 + 2 * PAGE_BYTES), None);
        assert_eq!(
            dart.map(0x10_4000, 0x9_0000_0000, PAGE_BYTES),
            Err(DartError::AlreadyMapped)
        );
        assert_eq!(
            dart.map(0x10_0001, 0, PAGE_BYTES),
            Err(DartError::Unaligned)
        );
        assert_eq!(dart.find_free(0x10_0000, PAGE_BYTES), Some(0x10_8000));
        dart.unmap(0x10_0000, 2 * PAGE_BYTES).unwrap();
        assert_eq!(dart.translate(0x10_0000), None);
    }

    #[test]
    fn wide_entries_hold_addresses_past_40_bits_shifted() {
        assert_eq!(
            Kind::T6000.entry_address(0xab_cdef_4000),
            0xab_cdef_4000 >> 14 << 10
        );
        assert_eq!(
            Kind::T6000.physical_of(0xab_cdef_4000 >> 14 << 10),
            0xab_cdef_4000
        );
        assert_eq!(Kind::T8020.entry_address(0xab_cdef_4000), 0xab_cdef_4000);
    }

    #[test]
    fn a_new_t8110_stream_gets_its_table_base_and_translation_on() {
        let tables = HostTables::default();
        let dart = Dart::new(FakeRegs::default(), Kind::T8110, 33, tables, false).unwrap();
        let writes = dart.regs.writes();
        assert!(writes.contains(&(0xc04, 1 << 1)));
        let ttbr = 1 | ((FIRST_TABLE >> 14) as u32) << 2 & 0x3fff_fffc;
        assert!(writes.contains(&(0x1400 + 4 * 33, ttbr)));
        assert!(writes.contains(&(0x1000 + 4 * 33, 1)));
        assert!(writes.contains(&(0x80, 1 << 8 | 33)));
    }
}
