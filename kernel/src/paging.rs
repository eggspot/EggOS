//! Page tables: the kernel's own address space (D29).
//!
//! [`PageTables`] builds a translation tree for any architecture's [`Format`]: it walks from
//! the root down, allocating tables from the frame allocator, and maps each range with the
//! largest leaves the alignment allows (2 MiB and 1 GiB where the format has them), so a large
//! direct map costs a handful of tables and few TLB entries. [`kernel_page_tables`] lays out
//! the kernel's address space:
//! - the kernel image at its physical address, each part with only the access it needs: code
//!   read and execute, constants read only, data and the boot stack read and write but never
//!   execute, and the page below the boot stack left unmapped, so an overflow faults instead of
//!   silently overwriting `.bss`;
//! - the direct map: all physical memory at [`Platform::direct_map_offset`];
//! - the device registers the HAL uses before the driver model exists (M5), as device memory at
//!   their physical addresses.
//!
//! Tables are only built before they are activated: changing live tables needs TLB
//! maintenance, which comes with the first user that does it (M4).

use crate::memory::{self, FrameAllocator, Memory};
use core::marker::PhantomData;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{self, Attributes, Entry, Format};
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// Most roots any [`Format`] has.
const MAX_ROOTS: usize = 2;

/// Why a mapping could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// An address or the length is not a multiple of 4 KiB.
    Unaligned,
    /// Part of the range is not a valid virtual address for the format (or wraps around).
    InvalidAddress,
    /// Part of the range is mapped already.
    AlreadyMapped,
    /// No frame was left for a table.
    OutOfMemory,
    /// The tables are active: changing them needs TLB maintenance, which comes with M4.
    Active,
}

impl MapError {
    pub const fn description(self) -> &'static str {
        match self {
            MapError::Unaligned => "an address or length is not page-aligned",
            MapError::InvalidAddress => "a virtual address is outside the address space",
            MapError::AlreadyMapped => "a page is mapped twice",
            MapError::OutOfMemory => "no free frame for a page table",
            MapError::Active => "the page tables are active and cannot be changed yet",
        }
    }
}

/// A translation tree in format `F`. Its tables are frames from a [`FrameAllocator`], reached
/// at physical address + the physical memory offset; they are never freed.
#[derive(Debug)]
pub struct PageTables<F: Format> {
    roots: [u64; MAX_ROOTS],
    physical_memory_offset: usize,
    /// Set once the tables are in use (see [`Self::mark_active`]).
    active: bool,
    format: PhantomData<fn() -> F>,
}

impl<F: Format> PageTables<F> {
    /// Empty tables (nothing mapped), with their root tables taken from `frames`.
    pub fn new(frames: &mut FrameAllocator) -> Result<Self, MapError> {
        const { assert!(F::ROOTS <= MAX_ROOTS && F::MAX_LEAF_LEVEL < F::LEVELS) };
        let mut roots = [0; MAX_ROOTS];
        for root in &mut roots[..F::ROOTS] {
            *root = new_table(frames)?;
        }
        Ok(Self {
            roots,
            physical_memory_offset: frames.physical_memory_offset(),
            active: false,
            format: PhantomData,
        })
    }

    /// Records that the tables are in use: [`Self::map`] refuses to change them from then on.
    pub fn mark_active(&mut self) {
        self.active = true;
    }

    /// The physical addresses of the root tables, one per [`Format::ROOTS`].
    pub fn roots(&self) -> &[u64] {
        &self.roots[..F::ROOTS]
    }

    /// Where the tables are reached from now on (see
    /// [`FrameAllocator::set_physical_memory_offset`]).
    pub fn set_physical_memory_offset(&mut self, offset: usize) {
        self.physical_memory_offset = offset;
    }

    /// Maps virtual `virt..virt + length` to physical `phys..phys + length` with `attributes`,
    /// using the largest leaves that fit. Everything must be 4 KiB-aligned and nothing in the
    /// range mapped yet. On an error, part of the range may have been mapped.
    pub fn map(
        &mut self,
        mut virt: u64,
        mut phys: u64,
        length: u64,
        attributes: Attributes,
        frames: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if self.active {
            return Err(MapError::Active);
        }
        if !(virt | phys | length).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        if length == 0 {
            return Ok(());
        }
        let last = virt
            .checked_add(length - 1)
            .ok_or(MapError::InvalidAddress)?;
        phys.checked_add(length - 1)
            .ok_or(MapError::InvalidAddress)?;
        // Valid addresses form one range per half of the address space, so a range whose ends
        // are valid and in the same half is valid throughout.
        if F::root(virt).is_none() || F::root(last).is_none() || (virt ^ last) >> 63 != 0 {
            return Err(MapError::InvalidAddress);
        }
        let end = virt.wrapping_add(length);
        while virt != end {
            let remaining = end.wrapping_sub(virt);
            let level = (0..=F::MAX_LEAF_LEVEL)
                .rev()
                .find(|&level| {
                    let size = paging::leaf_size(level);
                    (virt | phys).is_multiple_of(size) && remaining >= size
                })
                .unwrap_or(0);
            self.map_leaf(virt, phys, level, attributes, frames)?;
            virt = virt.wrapping_add(paging::leaf_size(level));
            phys = phys.wrapping_add(paging::leaf_size(level));
        }
        Ok(())
    }

    /// The physical address `virt` translates to and the mapping's attributes, or `None`.
    pub fn translate(&self, virt: u64) -> Option<(u64, Attributes)> {
        let mut table = self.roots[F::root(virt)?];
        for level in (0..F::LEVELS).rev() {
            match F::decode(self.entry(table, virt, level).read(), level) {
                Entry::Empty => return None,
                Entry::Table(next) => table = next,
                Entry::Leaf(phys, attributes) => {
                    return Some((phys + virt % paging::leaf_size(level), attributes));
                }
            }
        }
        None
    }

    fn map_leaf(
        &mut self,
        virt: u64,
        phys: u64,
        leaf_level: usize,
        attributes: Attributes,
        frames: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        let mut table = self.roots[F::root(virt).ok_or(MapError::InvalidAddress)?];
        for level in (leaf_level + 1..F::LEVELS).rev() {
            let entry = self.entry(table, virt, level);
            table = match F::decode(entry.read(), level) {
                Entry::Table(next) => next,
                Entry::Leaf(..) => return Err(MapError::AlreadyMapped),
                Entry::Empty => {
                    let next = new_table(frames)?;
                    entry.write(F::table(next, level));
                    next
                }
            };
        }
        let entry = self.entry(table, virt, leaf_level);
        if F::decode(entry.read(), leaf_level) != Entry::Empty {
            return Err(MapError::AlreadyMapped);
        }
        entry.write(F::leaf(phys, leaf_level, attributes));
        Ok(())
    }

    /// The entry for `virt` in the table at physical `table`, which is at `level`.
    fn entry(&self, table: u64, virt: u64, level: usize) -> TableEntry {
        let table = (table as usize).wrapping_add(self.physical_memory_offset) as *mut u64;
        TableEntry(table.wrapping_add(paging::index(virt, level)))
    }
}

/// One entry of a table this module allocated.
struct TableEntry(*mut u64);

impl TableEntry {
    fn read(&self) -> u64 {
        // SAFETY: the pointer is an aligned entry inside a table frame that `new_table` took
        // from the frame allocator (which owns it from then on) and zeroed, reached through the
        // physical memory offset the tables were given. Volatile, because the page-table
        // walker reads the entries behind the compiler's back.
        unsafe { self.0.read_volatile() }
    }

    fn write(&self, value: u64) {
        // SAFETY: as in `read`; the tables own the frame, so nothing else writes it.
        unsafe { self.0.write_volatile(value) }
    }
}

/// A zeroed table frame from `frames`.
fn new_table(frames: &mut FrameAllocator) -> Result<u64, MapError> {
    let table = frames.allocate().ok_or(MapError::OutOfMemory)?;
    let start = (table as usize).wrapping_add(frames.physical_memory_offset()) as *mut u64;
    for i in 0..paging::ENTRIES {
        // SAFETY: the frame was just allocated, so this code owns it; the allocator's offset
        // reaches it, and entry `i` of 512 is inside it and aligned.
        unsafe { start.wrapping_add(i).write_volatile(0) };
    }
    Ok(table)
}

/// The kernel's page tables for platform `P` (see the module documentation), with the direct
/// map covering `physical_memory` and every table taken from `frames`.
pub fn kernel_page_tables<P: Platform>(
    frames: &mut FrameAllocator,
    physical_memory: &MemoryMap,
) -> Result<PageTables<P::PageTableFormat>, MapError> {
    let mut tables = PageTables::new(frames)?;
    let image = P::kernel_image();
    for (range, attributes) in image.mapped_parts() {
        tables.map(range.start, range.start, range.len(), attributes, frames)?;
    }
    let offset = P::direct_map_offset() as u64;
    for range in physical_memory.ranges() {
        let virt = range
            .start
            .checked_add(offset)
            .ok_or(MapError::InvalidAddress)?;
        tables.map(
            virt,
            range.start,
            range.len(),
            Attributes::READ_WRITE,
            frames,
        )?;
    }
    // Page by page: two register blocks may share a page. A page mapped already is compared
    // with a device leaf as the format decodes it (RISC-V does not record the memory type).
    let device_leaf = match <P::PageTableFormat as Format>::decode(
        <P::PageTableFormat as Format>::leaf(0, 0, Attributes::DEVICE),
        0,
    ) {
        Entry::Leaf(_, attributes) => attributes,
        _ => Attributes::DEVICE,
    };
    let mut result = Ok(());
    P::early_device_regions(&mut |device| {
        let device = device.covering_frames();
        for page in (device.start..device.end).step_by(FRAME_SIZE as usize) {
            if result.is_err() || tables.translate(page) == Some((page, device_leaf)) {
                continue;
            }
            result = tables.map(page, page, FRAME_SIZE, Attributes::DEVICE, frames);
        }
    });
    result.map(|()| tables)
}

/// Builds the kernel's page tables (see [`kernel_page_tables`]) from the free memory in
/// `free_memory` and switches to them. From then on the frame allocator and the tables reach
/// physical memory through the direct map, and the frame allocator is [`memory::FRAMES`], so
/// the kernel heap works.
pub fn enable<P: Platform>(free_memory: &MemoryMap) -> Result<Memory<P>, &'static str> {
    let direct_map = P::physical_memory();
    let covered = |free: &PhysicalRange| {
        direct_map
            .ranges()
            .iter()
            .any(|r| r.start <= free.start && free.end <= r.end)
    };
    if !free_memory.ranges().iter().all(covered) {
        return Err("free memory outside the physical memory the direct map covers");
    }
    let mut frames = FrameAllocator::new(free_memory, P::physical_memory_offset());
    let mut page_tables =
        kernel_page_tables::<P>(&mut frames, &direct_map).map_err(MapError::description)?;
    // SAFETY: `kernel_page_tables` built tables in `P`'s format mapping the kernel image at
    // its physical address with code executable and data and stack writable, every range of
    // `P::physical_memory` at the direct map offset and every early device region as device
    // memory at its physical address. Their frames are never freed and nothing changes them
    // while they are active. Only the frame allocator and the tables reach memory through the
    // physical memory offset, and both switch to the new one below.
    unsafe { P::activate_page_tables(page_tables.roots()) };
    page_tables.mark_active();
    let offset = P::physical_memory_offset();
    frames.set_physical_memory_offset(offset);
    page_tables.set_physical_memory_offset(offset);
    *memory::FRAMES.lock() = Some(frames);
    Ok(Memory { page_tables })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_hal::paging::aarch64::Aarch64;
    use eggos_hal::paging::riscv64::Riscv64Sv39;
    use eggos_hal::paging::x86_64::X86_64;
    use eggos_hal::paging::{MemoryType, leaf_size};

    const K: u64 = FRAME_SIZE;

    /// Host memory standing in for `frames` frames of RAM at physical 0x10_0000, and an
    /// allocator over them.
    fn frames(count: u64) -> FrameAllocator {
        const BASE: u64 = 0x10_0000;
        let memory = vec![0u8; ((count + 1) * K) as usize].leak();
        let start = (memory.as_mut_ptr() as usize).next_multiple_of(K as usize);
        let mut map = MemoryMap::new();
        map.add_ram(PhysicalRange::from_start_length(BASE, count * K));
        FrameAllocator::new(&map, start.wrapping_sub(BASE as usize))
    }

    fn maps_and_translates<F: Format>(high: u64) {
        let mut frames = frames(64);
        let mut tables = PageTables::<F>::new(&mut frames).unwrap();
        // 4 KiB pages: a 3-page range not aligned to anything larger.
        tables
            .map(0x40_3000, 0x8000_3000, 3 * K, Attributes::TEXT, &mut frames)
            .unwrap();
        // A large range in the high half with 1 GiB-aligned ends: the largest leaves.
        let big = 2 << 30;
        tables
            .map(high, 0, big, Attributes::READ_WRITE, &mut frames)
            .unwrap();
        tables
            .map(0x900_0000, 0x900_0000, K, Attributes::DEVICE, &mut frames)
            .unwrap();
        assert_eq!(
            tables.translate(0x40_4abc),
            Some((0x8000_4abc, Attributes::TEXT))
        );
        assert_eq!(tables.translate(0x40_2000), None);
        assert_eq!(tables.translate(0x40_6000), None);
        assert_eq!(
            tables.translate(high + big - 8),
            Some((big - 8, Attributes::READ_WRITE))
        );
        assert_eq!(tables.translate(high + big), None);
        let device = tables.translate(0x900_0010).unwrap();
        assert_eq!(device.0, 0x900_0010);
        assert!(device.1.writable && !device.1.executable);
        // Tables used: the roots, plus one table per level above the leaves for each region.
        let used = 64 - frames.free_frames();
        let largest = leaf_size(F::MAX_LEAF_LEVEL);
        assert_eq!(big % largest, 0);
        assert!(
            used <= (F::ROOTS + 3 * (F::LEVELS - 1)) as u64,
            "{used} tables"
        );
    }

    #[test]
    fn maps_and_translates_on_every_format() {
        maps_and_translates::<X86_64>(0xffff_8000_0000_0000);
        maps_and_translates::<Aarch64>(0xffff_0000_0000_0000);
        maps_and_translates::<Riscv64Sv39>(0xffff_ffc0_0000_0000);
    }

    #[test]
    fn large_leaves_are_used_where_aligned_and_small_ones_at_the_edges() {
        let mut frames = frames(16);
        let mut tables = PageTables::<X86_64>::new(&mut frames).unwrap();
        // 4 KiB below a 2 MiB boundary, then 2 MiB, then 4 KiB above: x86_64's largest leaf
        // here is 2 MiB, so one page directory and two page tables.
        let start = (2 << 20) - K;
        tables
            .map(
                start,
                start,
                (2 << 20) + 2 * K,
                Attributes::READ_WRITE,
                &mut frames,
            )
            .unwrap();
        // Root, PDPT, page directory, two page tables.
        assert_eq!(16 - frames.free_frames(), 5);
        for virt in [start, 2 << 20, (4 << 20) + K - 1] {
            assert_eq!(tables.translate(virt).map(|t| t.0), Some(virt));
        }
    }

    #[test]
    fn mapping_twice_fails() {
        let mut frames = frames(16);
        let mut tables = PageTables::<Aarch64>::new(&mut frames).unwrap();
        let rw = Attributes::READ_WRITE;
        tables.map(2 << 20, 0, 2 << 20, rw, &mut frames).unwrap();
        // Inside the 2 MiB block, and a block over an existing page table.
        assert_eq!(
            tables.map((2 << 20) + K, 0, K, rw, &mut frames),
            Err(MapError::AlreadyMapped)
        );
        tables.map(8 << 20, 0, K, rw, &mut frames).unwrap();
        assert_eq!(
            tables.map(8 << 20, 0, 2 << 20, rw, &mut frames),
            Err(MapError::AlreadyMapped)
        );
    }

    #[test]
    fn invalid_ranges_are_refused() {
        let mut frames = frames(16);
        let mut tables = PageTables::<X86_64>::new(&mut frames).unwrap();
        let rw = Attributes::READ_WRITE;
        assert_eq!(
            tables.map(K + 1, 0, K, rw, &mut frames),
            Err(MapError::Unaligned)
        );
        assert_eq!(
            tables.map(0, 0, K / 2, rw, &mut frames),
            Err(MapError::Unaligned)
        );
        // Non-canonical, and a range from the low half across the hole.
        assert_eq!(
            tables.map(0x8000_0000_0000, 0, K, rw, &mut frames),
            Err(MapError::InvalidAddress)
        );
        assert_eq!(
            tables.map(0x7fff_ffff_f000, 0, 2 * K, rw, &mut frames),
            Err(MapError::InvalidAddress)
        );
        assert_eq!(
            tables.map(u64::MAX - K + 1, 0, 2 * K, rw, &mut frames),
            Err(MapError::InvalidAddress)
        );
        // Nothing was allocated beyond the root.
        assert_eq!(frames.free_frames(), 15);
        assert_eq!(tables.map(0, 0, 0, rw, &mut frames), Ok(()));
    }

    #[test]
    fn active_tables_are_not_changed() {
        let mut frames = frames(16);
        let mut tables = PageTables::<X86_64>::new(&mut frames).unwrap();
        tables.mark_active();
        assert_eq!(
            tables.map(0, 0, K, Attributes::READ_WRITE, &mut frames),
            Err(MapError::Active)
        );
        assert_eq!(tables.translate(0), None);
    }

    #[test]
    fn running_out_of_frames_for_tables_fails() {
        let mut frames = frames(2);
        let mut tables = PageTables::<Riscv64Sv39>::new(&mut frames).unwrap();
        assert_eq!(
            tables.map(0, 0, K, Attributes::READ_WRITE, &mut frames),
            Err(MapError::OutOfMemory)
        );
    }

    #[test]
    fn the_memory_type_survives_where_the_format_encodes_it() {
        let mut frames = frames(16);
        let mut tables = PageTables::<Aarch64>::new(&mut frames).unwrap();
        tables
            .map(0x900_0000, 0x900_0000, K, Attributes::DEVICE, &mut frames)
            .unwrap();
        assert_eq!(
            tables.translate(0x900_0000).map(|t| t.1.memory_type),
            Some(MemoryType::Device)
        );
    }
}
