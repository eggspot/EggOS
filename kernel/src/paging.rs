//! Page tables: the kernel's own address space (D29).
//!
//! [`PageTables`] builds a translation tree for any architecture's [`Format`]: it walks from
//! the root down, allocating tables from the frame allocator, and maps each range with the
//! largest leaves the alignment allows (2 MiB and 1 GiB where the format has them), so a large
//! direct map costs a handful of tables and few TLB entries. [`kernel_page_tables`] lays out
//! the kernel's address space:
//! - the kernel image at its virtual address, in the top 2 GiB (D33), each part with only the
//!   access it needs: code read and execute, constants read only, data and the boot stack read
//!   and write but never execute, and the page below the boot stack left unmapped, so an
//!   overflow faults instead of silently overwriting `.bss`;
//! - the direct map: all physical memory at [`Platform::direct_map_offset`], read and write and
//!   never executable, except that the kernel image's own frames keep their access there (code
//!   and constants read-only) and its stack guard page is unmapped, so no alias undoes W^X;
//! - the device registers the HAL uses before the driver model exists (M5), as device memory in
//!   the direct map's window: physical address + [`Platform::direct_map_offset`] (D33).
//!
//! - the device window ([`device_window`]): one root-level entry's worth of addresses below the
//!   kernel image's, whose table is made with the tables, where drivers map device registers
//!   once the tables are active ([`PageTables::map_device_window`], D110).
//!
//! Nothing is mapped in the low half, which belongs to user space: each user address space has
//! a low half of its own and shares this kernel half ([`PageTables::new_sharing_kernel_half`],
//! [`crate::address_space`], D45).
//!
//! The kernel's tables are only built or changed while they are not active, but for the device
//! window, where entries are only ever added below a root entry made in advance: every user
//! address space's copy of the kernel's root entries stays right, and no CPU has a translation
//! to drop.

use crate::memory::{self, FrameAllocator, Memory};
use core::marker::PhantomData;
use core::sync::atomic::{Ordering, fence};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{self, Attributes, Entry, Format, KernelImage};
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// Most roots any [`Format`] has.
pub const MAX_ROOTS: usize = 2;

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
    /// Leaving a device's pages out of the direct map would split a range of a memory map
    /// that has no room left, losing memory the direct map must cover.
    TooManyRanges,
    /// Part of the range to unmap is not mapped.
    NotMapped,
    /// The range to unmap covers only part of a large leaf (a 2 MiB or 1 GiB page).
    PartOfLargePage,
    /// A user address space maps only user mappings, and only in the low half.
    NotUser,
    /// The range is not inside the device window, or the tables are not the kernel's.
    OutsideDeviceWindow,
}

impl MapError {
    pub const fn description(self) -> &'static str {
        match self {
            MapError::Unaligned => "an address or length is not page-aligned",
            MapError::InvalidAddress => "a virtual address is outside the address space",
            MapError::AlreadyMapped => "a page is mapped twice",
            MapError::OutOfMemory => "no free frame for a page table",
            MapError::Active => "the page tables are active and cannot be changed yet",
            MapError::TooManyRanges => "too many memory ranges to leave a device's pages out",
            MapError::NotMapped => "a page to unmap is not mapped",
            MapError::PartOfLargePage => "the range to unmap splits a large page",
            MapError::NotUser => "a user address space maps only user pages in the low half",
            MapError::OutsideDeviceWindow => "the range is outside the kernel's device window",
        }
    }
}

/// A translation tree in format `F`. Its tables are frames from a [`FrameAllocator`], reached
/// at physical address + the physical memory offset; the kernel's are never freed, a user
/// address space's low half is ([`Self::free_user_half`]).
#[derive(Debug)]
pub struct PageTables<F: Format> {
    roots: [u64; MAX_ROOTS],
    physical_memory_offset: usize,
    /// Set once the tables are in use (see [`Self::mark_active`]).
    active: bool,
    /// Set for a user address space's tables ([`Self::new_sharing_kernel_half`]): they map and
    /// unmap only user mappings in the low half, and never touch the kernel half they share.
    user_half_only: bool,
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
            user_half_only: false,
            format: PhantomData,
        })
    }

    /// Tables for a user address space (M4): a low half of their own, empty, and the kernel
    /// half of `kernel`, shared. On AArch64 that is `kernel`'s high-half root itself; with one
    /// root, the new root's kernel-half entries are copies of `kernel`'s root's, which point to
    /// the same tables. Sound only because the kernel's root entries never change once active
    /// (see [`Self::map`]; [`Self::map_device_window`] only fills tables below a root entry made
    /// in advance): a root entry added later would be missing here. The new tables
    /// map and unmap only user mappings ([`Attributes::user`]) in the low half
    /// ([`MapError::NotUser`] otherwise), so the kernel half stays the kernel's. Free the low
    /// half with [`Self::free_user_half`]. `kernel` must be kernel tables marked active
    /// ([`Self::mark_active`]), so frozen, or a user address space's tables made from them,
    /// whose kernel half is theirs; anything else stops the kernel.
    pub fn new_sharing_kernel_half(
        kernel: &Self,
        frames: &mut FrameAllocator,
    ) -> Result<Self, MapError> {
        assert!(
            kernel.active || kernel.user_half_only,
            "an address space shares only the kernel's active tables"
        );
        let user_root = F::root(0).expect("address 0 is valid on every format");
        let mut roots = kernel.roots;
        roots[user_root] = new_table(frames)?;
        let tables = Self {
            roots,
            physical_memory_offset: kernel.physical_memory_offset,
            active: false,
            user_half_only: true,
            format: PhantomData,
        };
        let top = F::LEVELS - 1;
        for index in kernel_half_root_entries::<F>() {
            let virt = index as u64 * paging::leaf_size(top);
            let entry = kernel.entry(kernel.roots[user_root], virt, top).read();
            tables.entry(roots[user_root], virt, top).write(entry);
        }
        Ok(tables)
    }

    /// Frees the low half's tables, the user root among them, which
    /// [`Self::new_sharing_kernel_half`] and [`Self::map`] allocated from `frames`; the kernel
    /// half's are the kernel's and stay. The frames the leaves map are not the tables': each
    /// leaf is handed to `leaf` as (`frames`, its physical address, its level), in the same walk,
    /// for an owner that frees them.
    ///
    /// # Safety
    /// The tables must come from [`Self::new_sharing_kernel_half`] with `frames` as the
    /// allocator, must not be active on any CPU, and must not be used again afterwards (not even
    /// to free them again).
    pub unsafe fn free_user_half(
        &mut self,
        frames: &mut FrameAllocator,
        mut leaf: impl FnMut(&mut FrameAllocator, u64, usize),
    ) {
        assert!(
            self.user_half_only,
            "only a user address space's tables are freed"
        );
        let user_root = F::root(0).expect("address 0 is valid on every format");
        let root = self.roots[user_root];
        let top = F::LEVELS - 1;
        let user_entries = kernel_half_root_entries::<F>().start;
        self.for_each_used_entry(root, 0..user_entries, |entry| {
            match F::decode(entry, top) {
                Entry::Table(next) => self.free_table_tree(next, top - 1, frames, &mut leaf),
                Entry::Leaf(phys, _) => leaf(frames, phys, top),
                // Only at level 0.
                Entry::Kept(_) | Entry::Empty => {}
            }
        });
        // SAFETY: the root came from `frames` in `new_sharing_kernel_half`, and nothing walks
        // it any more (the caller guarantees it is not active, and that `self` is not used
        // again).
        unsafe { frames.free(root) };
    }

    /// Frees the table at physical `table`, at `level`, and every table below it, handing
    /// each leaf to `leaf` (see [`Self::free_user_half`]).
    fn free_table_tree(
        &self,
        table: u64,
        level: usize,
        frames: &mut FrameAllocator,
        leaf: &mut impl FnMut(&mut FrameAllocator, u64, usize),
    ) {
        self.for_each_used_entry(table, 0..paging::ENTRIES, |entry| {
            match F::decode(entry, level) {
                Entry::Table(next) if level > 0 => {
                    self.free_table_tree(next, level - 1, frames, leaf)
                }
                Entry::Leaf(phys, _) => leaf(frames, phys, level),
                Entry::Kept(phys) => leaf(frames, phys, 0),
                _ => {}
            }
        });
        // SAFETY: a table of the low half, allocated from `frames` by `map_leaf`, reached once
        // (a table hangs from one entry), and walked by nothing any more (see `free_user_half`).
        unsafe { frames.free(table) };
    }

    /// Records that the tables are in use: [`Self::map`] refuses to change them from then on.
    pub fn mark_active(&mut self) {
        self.active = true;
    }

    /// The physical addresses of the root tables, one per [`Format::ROOTS`].
    pub fn roots(&self) -> &[u64] {
        &self.roots[..F::ROOTS]
    }

    /// Host tests only: the tables with the one root `root`, active (so frozen), to look up
    /// what they map, as a test platform playing a program's part reads its memory.
    #[cfg(test)]
    pub(crate) fn view_for_test(root: u64, physical_memory_offset: usize) -> Self {
        assert_eq!(F::ROOTS, 1);
        let mut roots = [0; MAX_ROOTS];
        roots[0] = root;
        Self {
            roots,
            physical_memory_offset,
            active: true,
            user_half_only: true,
            format: PhantomData,
        }
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
        if self.user_half_only && !attributes.user {
            return Err(MapError::NotUser);
        }
        self.check_user_half(virt, length)?;
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

    /// Unmaps `virt..virt + length`, which must be mapped, 4 KiB-aligned and cover any large
    /// leaf in it whole. Tables left empty stay until the tables are freed. The tables must not
    /// be active, as for [`Self::map`]: the TLB may still hold what is unmapped. On an error,
    /// part of the range may have been unmapped.
    pub fn unmap(&mut self, mut virt: u64, length: u64) -> Result<(), MapError> {
        if self.active {
            return Err(MapError::Active);
        }
        self.check_user_half(virt, length)?;
        if !(virt | length).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        let end = virt.checked_add(length).ok_or(MapError::InvalidAddress)?;
        while virt < end {
            let (entry, level) = self.leaf_entry(virt)?;
            let size = paging::leaf_size(level);
            if !virt.is_multiple_of(size) || end - virt < size {
                return Err(MapError::PartOfLargePage);
            }
            entry.write(0);
            virt += size;
        }
        Ok(())
    }

    /// For a user address space's tables (D53): makes of every 4 KiB page in `virt..virt +
    /// length` that is mapped, or kept ([`Entry::Kept`]), what `change` says, given its frame
    /// and its attributes (`None` for a kept page); each frame unmapped is handed to
    /// `unmapped` once its entry no longer refers to it. Parts of the range that map nothing are
    /// skipped a table at a time, so a large range costs a walk per table, not per page. A
    /// large leaf in the range is [`MapError::PartOfLargePage`] (a process maps 4 KiB pages
    /// only). The tables may be in use: the caller drops what CPUs cached of the old entries
    /// ([`crate::address_space::AddressSpace`] does). On an error, part of the range may have
    /// been changed.
    #[inline(always)]
    pub fn change_user_pages(
        &mut self,
        virt: u64,
        length: u64,
        mut change: impl FnMut(u64, Option<Attributes>) -> PageChange,
        mut unmapped: impl FnMut(u64),
    ) -> Result<(), MapError> {
        if !self.user_half_only {
            return Err(MapError::NotUser);
        }
        self.check_user_half(virt, length)?;
        if !(virt | length).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        self.for_each_user_page(virt, length, |_, entry, phys, attributes| {
            match change(phys, attributes) {
                PageChange::Unmap => {
                    entry.write(0);
                    unmapped(phys);
                }
                PageChange::Map(attributes) if attributes.user => {
                    entry.write(F::leaf(phys, 0, attributes))
                }
                PageChange::Map(_) => return Err(MapError::NotUser),
                PageChange::Inaccessible => entry.write(F::kept(phys)),
            }
            Ok(())
        })
    }

    /// For a user address space's tables: moves every 4 KiB page of `from..from + length` that
    /// is mapped, or kept ([`Entry::Kept`]), to the same offset from `to`, entry and all, so
    /// the frame and its contents go with it; the entries at `from` are left empty. Nothing
    /// may be mapped or kept in `to..to + length` ([`MapError::AlreadyMapped`] otherwise, with
    /// nothing moved), and the ranges must not overlap. Tables the destination lacks come
    /// from `frames`; with none left, the pages moved so far go back and the error is
    /// [`MapError::OutOfMemory`] (tables allocated stay until the tables are freed). The tables
    /// may be in use: the caller drops what CPUs cached of the entries at `from`.
    pub fn move_user_pages(
        &mut self,
        from: u64,
        to: u64,
        length: u64,
        frames: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if !self.user_half_only {
            return Err(MapError::NotUser);
        }
        self.check_user_half(from, length)?;
        self.check_user_half(to, length)?;
        if !(from | to | length).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        if from < to + length && to < from + length {
            return Err(MapError::InvalidAddress);
        }
        self.for_each_user_page(to, length, |_, _, _, _| Err(MapError::AlreadyMapped))?;
        fence(Ordering::Release);
        let mut moved_end = from;
        let result = self.move_entries(from, to, length, frames, &mut moved_end);
        if result.is_err() {
            self.move_entries(to, from, moved_end - from, frames, &mut 0)
                .expect("moving back finds every table of the source still there");
        }
        result
    }

    /// Moves the entries of [`Self::move_user_pages`], recording in `moved_end` the end of
    /// what has moved.
    fn move_entries(
        &self,
        from: u64,
        to: u64,
        length: u64,
        frames: &mut FrameAllocator,
        moved_end: &mut u64,
    ) -> Result<(), MapError> {
        self.for_each_user_page(from, length, |virt, entry, _, _| {
            let destination = self.leaf_table_entry(virt - from + to, 0, true, frames)?;
            destination.write(entry.read());
            entry.write(0);
            *moved_end = virt + FRAME_SIZE;
            Ok(())
        })
    }

    /// For a user address space's tables: hands every 4 KiB page of user space that is mapped,
    /// or kept ([`Entry::Kept`]), to `page` in address order, as its address, its frame and its
    /// attributes (`None` if kept), skipping a table at a time what maps nothing. An error from
    /// `page` stops the walk.
    pub fn for_each_mapped_user_page(
        &self,
        mut page: impl FnMut(u64, u64, Option<Attributes>) -> Result<(), MapError>,
    ) -> Result<(), MapError> {
        if !self.user_half_only {
            return Err(MapError::NotUser);
        }
        self.for_each_user_page(0, F::USER_END, |virt, _, frame, attributes| {
            page(virt, frame, attributes)
        })
    }

    /// For a user address space's tables: maps the 4 KiB page at user address `virt` to
    /// `frame` with `attributes`, a user mapping, or keeps `frame` there with no access if
    /// `attributes` is `None` ([`Entry::Kept`]). Tables it lacks come from `frames`. Nothing may
    /// be mapped or kept there yet ([`MapError::AlreadyMapped`]).
    pub fn map_user_page(
        &mut self,
        virt: u64,
        frame: u64,
        attributes: Option<Attributes>,
        frames: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if !self.user_half_only || attributes.is_some_and(|attributes| !attributes.user) {
            return Err(MapError::NotUser);
        }
        self.check_user_half(virt, FRAME_SIZE)?;
        if !(virt | frame).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        fence(Ordering::Release);
        let entry = self.leaf_table_entry(virt, 0, true, frames)?;
        if entry.read() != 0 {
            return Err(MapError::AlreadyMapped);
        }
        entry.write(match attributes {
            Some(attributes) => F::leaf(frame, 0, attributes),
            None => F::kept(frame),
        });
        Ok(())
    }

    /// Hands every 4 KiB page of `virt..virt + length` in a user address space's tables that
    /// is mapped, or kept ([`Entry::Kept`]), to `page` in address order: its address, its entry,
    /// its frame and its attributes (`None` if kept). Parts of the range that map nothing are
    /// skipped a table at a time, so a large range costs a walk per table, not per page. A
    /// large leaf is [`MapError::PartOfLargePage`]; an error from `page` stops the walk.
    #[inline(always)]
    fn for_each_user_page(
        &self,
        mut virt: u64,
        length: u64,
        mut page: impl FnMut(u64, TableEntry, u64, Option<Attributes>) -> Result<(), MapError>,
    ) -> Result<(), MapError> {
        let end = virt + length;
        'pages: while virt < end {
            let mut table = self.roots[F::root(virt).ok_or(MapError::InvalidAddress)?];
            for level in (0..F::LEVELS).rev() {
                let entry = self.entry(table, virt, level);
                let (phys, attributes) = match F::decode(entry.read(), level) {
                    Entry::Table(next) if level > 0 => {
                        table = next;
                        continue;
                    }
                    Entry::Leaf(phys, attributes) if level == 0 => (phys, Some(attributes)),
                    Entry::Kept(phys) => (phys, None),
                    Entry::Leaf(..) => return Err(MapError::PartOfLargePage),
                    Entry::Empty | Entry::Table(_) => {
                        let size = paging::leaf_size(level);
                        virt = (virt & !(size - 1)).saturating_add(size);
                        continue 'pages;
                    }
                };
                page(virt, entry, phys, attributes)?;
                break;
            }
            virt += FRAME_SIZE;
        }
        Ok(())
    }

    /// For a user address space's tables, whether `virt..virt + length` lies in the low half.
    fn check_user_half(&self, virt: u64, length: u64) -> Result<(), MapError> {
        match virt.checked_add(length) {
            _ if !self.user_half_only => Ok(()),
            Some(end) if end <= F::USER_END => Ok(()),
            _ => Err(MapError::NotUser),
        }
    }

    /// The entry of the leaf mapping `virt`, and its level.
    fn leaf_entry(&self, virt: u64) -> Result<(TableEntry, usize), MapError> {
        let mut table = self.roots[F::root(virt).ok_or(MapError::InvalidAddress)?];
        for level in (0..F::LEVELS).rev() {
            let entry = self.entry(table, virt, level);
            match F::decode(entry.read(), level) {
                Entry::Empty | Entry::Kept(_) => break,
                Entry::Table(next) => table = next,
                Entry::Leaf(..) => return Ok((entry, level)),
            }
        }
        Err(MapError::NotMapped)
    }

    /// Maps the device registers at physical `phys..phys + length` at `virt..virt + length` in
    /// `window`, the kernel's device window ([`device_window`]), as device memory, with the
    /// largest leaves that fit below the root level, while the tables may be active (D110).
    /// Only entries that were empty are written, below the window's root entry, which
    /// [`kernel_page_tables`] made, so the root entries user address spaces copied stay right.
    /// Everything must be 4 KiB-aligned and nothing in the range mapped yet. The caller makes the
    /// new entries visible to this CPU's walks
    /// ([`eggos_hal::Platform::make_kernel_mappings_visible`]); other CPUs must not have reached
    /// the range before. On an error, part of the range may have been mapped.
    #[inline(never)]
    pub fn map_device_window(
        &mut self,
        window: &core::ops::Range<u64>,
        mut virt: u64,
        mut phys: u64,
        length: u64,
        frames: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if self.user_half_only {
            return Err(MapError::OutsideDeviceWindow);
        }
        if !(virt | phys | length).is_multiple_of(FRAME_SIZE) {
            return Err(MapError::Unaligned);
        }
        let end = virt.checked_add(length).ok_or(MapError::InvalidAddress)?;
        if virt < window.start || end > window.end {
            return Err(MapError::OutsideDeviceWindow);
        }
        phys.checked_add(length).ok_or(MapError::InvalidAddress)?;
        let largest = F::MAX_LEAF_LEVEL.min(F::LEVELS - 2);
        while virt != end {
            let remaining = end - virt;
            let level = (0..=largest)
                .rev()
                .find(|&level| {
                    let size = paging::leaf_size(level);
                    (virt | phys).is_multiple_of(size) && remaining >= size
                })
                .unwrap_or(0);
            fence(Ordering::Release);
            let entry = self.leaf_table_entry(virt, level, false, frames)?;
            if entry.read() != 0 {
                return Err(MapError::AlreadyMapped);
            }
            entry.write(F::leaf(phys, level, Attributes::DEVICE));
            virt += paging::leaf_size(level);
            phys += paging::leaf_size(level);
        }
        Ok(())
    }

    /// The physical address `virt` translates to and the mapping's attributes, or `None`.
    pub fn translate(&self, virt: u64) -> Option<(u64, Attributes)> {
        let mut table = self.roots[F::root(virt)?];
        for level in (0..F::LEVELS).rev() {
            match F::decode(self.entry(table, virt, level).read(), level) {
                Entry::Empty | Entry::Kept(_) => return None,
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
        // A user address space's tables gain mappings while other CPUs may walk them (D52):
        // whatever was written to the frame before (a page's contents, a new table's zeros) must
        // be seen by any CPU that sees the entry. A walker on another CPU is an observer as any
        // other, so a release fence orders it (`dmb ish` on AArch64, `fence rw,w` on RISC-V).
        fence(Ordering::Release);
        let entry = self.leaf_table_entry(virt, leaf_level, attributes.user, frames)?;
        if entry.read() != 0 {
            return Err(MapError::AlreadyMapped);
        }
        entry.write(F::leaf(phys, leaf_level, attributes));
        Ok(())
    }

    /// The entry at `leaf_level` for `virt`, with the tables above it allocated from `frames`
    /// where missing (user tables if `user`), each zeroed before it is linked in. A leaf or
    /// kept page above `leaf_level` is [`MapError::AlreadyMapped`].
    #[inline(always)]
    fn leaf_table_entry(
        &self,
        virt: u64,
        leaf_level: usize,
        user: bool,
        frames: &mut FrameAllocator,
    ) -> Result<TableEntry, MapError> {
        let mut table = self.roots[F::root(virt).ok_or(MapError::InvalidAddress)?];
        for level in (leaf_level + 1..F::LEVELS).rev() {
            let entry = self.entry(table, virt, level);
            table = match F::decode(entry.read(), level) {
                Entry::Table(next) => next,
                Entry::Leaf(..) | Entry::Kept(_) => return Err(MapError::AlreadyMapped),
                Entry::Empty => {
                    let next = new_table(frames)?;
                    fence(Ordering::Release);
                    entry.write(F::table(next, level, user));
                    next
                }
            };
        }
        Ok(self.entry(table, virt, leaf_level))
    }

    /// The entry for `virt` in the table at physical `table`, which is at `level`.
    fn entry(&self, table: u64, virt: u64, level: usize) -> TableEntry {
        self.entry_at(table, paging::index(virt, level))
    }

    /// Hands every entry of `indices` in the table at physical `table` that is not 0 to
    /// `used`, in order. Tables being freed are mostly empty, so the scan is what freeing
    /// costs: four entries a step, a step of zeros skipped with one test (D53).
    #[inline(always)]
    fn for_each_used_entry(
        &self,
        table: u64,
        indices: core::ops::Range<usize>,
        mut used: impl FnMut(u64),
    ) {
        let mut index = indices.start;
        while index + 4 <= indices.end {
            let entries = [
                self.entry_at(table, index).read(),
                self.entry_at(table, index + 1).read(),
                self.entry_at(table, index + 2).read(),
                self.entry_at(table, index + 3).read(),
            ];
            if entries[0] | entries[1] | entries[2] | entries[3] != 0 {
                for entry in entries {
                    if entry != 0 {
                        used(entry);
                    }
                }
            }
            index += 4;
        }
        for index in index..indices.end {
            let entry = self.entry_at(table, index).read();
            if entry != 0 {
                used(entry);
            }
        }
    }

    /// Entry `index` (below [`paging::ENTRIES`]) of the table at physical `table`.
    fn entry_at(&self, table: u64, index: usize) -> TableEntry {
        let table = (table as usize).wrapping_add(self.physical_memory_offset) as *mut u64;
        TableEntry(table.wrapping_add(index))
    }
}

/// What [`PageTables::change_user_pages`] makes of a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageChange {
    /// Maps nothing there any more: the frame goes back to the caller.
    Unmap,
    /// Maps the same frame with these attributes, a user mapping.
    Map(Attributes),
    /// Keeps the frame in an entry that translates nothing ([`Entry::Kept`]): the program may
    /// not touch the page, and its contents stay (`PROT_NONE`).
    Inaccessible,
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

/// The root-level entries that map the kernel half in the root that also maps user space:
/// those whose range starts at or above [`Format::USER_END`] (none on AArch64, whose user root
/// maps the low half alone).
fn kernel_half_root_entries<F: Format>() -> core::ops::Range<usize> {
    let per_entry = paging::leaf_size(F::LEVELS - 1);
    let first = F::USER_END.div_ceil(per_entry).min(paging::ENTRIES as u64) as usize;
    first..paging::ENTRIES
}

/// The kernel's device window for platform `P` (D110): the addresses one root-level entry maps,
/// the entry just below the one that maps the kernel image (512 GiB on AArch64 and x86_64, 1 GiB
/// on RISC-V Sv39). The direct map must end below it.
pub fn device_window<P: Platform>() -> core::ops::Range<u64> {
    let image = P::kernel_image();
    let size = paging::leaf_size(<P::PageTableFormat as Format>::LEVELS - 1);
    let image_entry = image.virtual_address(image.start) & !(size - 1);
    let start = image_entry.wrapping_sub(size);
    start..image_entry
}

/// A zeroed table frame from `frames`.
fn new_table(frames: &mut FrameAllocator) -> Result<u64, MapError> {
    let table = frames.allocate().ok_or(MapError::OutOfMemory)?;
    let start = (table as usize).wrapping_add(frames.physical_memory_offset()) as *mut u8;
    // SAFETY: the frame was just allocated, so this code owns it; the allocator's offset
    // reaches it. Linking it in orders these stores before the entry (`map`).
    unsafe { eggos_hal::memory::zero(start, FRAME_SIZE as usize) };
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
        let virt = image.virtual_address(range.start);
        tables.map(virt, range.start, range.len(), attributes, frames)?;
    }
    let offset = P::direct_map_offset() as u64;
    let window = device_window::<P>();
    if !direct_map_fits_below(physical_memory, offset, window.start) {
        return Err(MapError::InvalidAddress);
    }
    tables.leaf_table_entry(
        window.start,
        <P::PageTableFormat as Format>::LEVELS - 2,
        false,
        frames,
    )?;
    let normal_memory = without_early_devices::<P>(physical_memory)?;
    map_direct_map(&mut tables, &normal_memory, offset, &image, frames)?;
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
            let virt = page.wrapping_add(offset);
            if result.is_err() || tables.translate(virt) == Some((page, device_leaf)) {
                continue;
            }
            result = tables.map(virt, page, FRAME_SIZE, Attributes::DEVICE, frames);
        }
    });
    result.map(|()| tables)
}

fn device_ranges<P: Platform>(each: &mut dyn FnMut(PhysicalRange)) {
    P::early_device_regions(&mut |device| each(device.covering_frames()));
    P::device_apertures(&mut |aperture| each(aperture.covering_frames()));
}

fn without_early_devices<P: Platform>(memory: &MemoryMap) -> Result<MemoryMap, MapError> {
    let mut normal = memory.clone();
    let mut left_out = MemoryMap::new();
    device_ranges::<P>(&mut |device| {
        normal.reserve(device);
        for range in memory.ranges() {
            left_out.add_ram(PhysicalRange {
                start: device.start.max(range.start),
                end: device.end.min(range.end),
            });
        }
    });
    if normal.total() + left_out.total() != memory.total() {
        return Err(MapError::TooManyRanges);
    }
    Ok(normal)
}

fn overlaps_early_devices<P: Platform>(memory: &MemoryMap) -> bool {
    let mut overlaps = false;
    device_ranges::<P>(&mut |device| {
        overlaps |= memory
            .ranges()
            .iter()
            .any(|range| range.start < device.end && device.start < range.end);
    });
    overlaps
}

fn direct_map_fits_below(ram: &MemoryMap, offset: u64, limit: u64) -> bool {
    ram.ranges().iter().all(|range| {
        range
            .end
            .checked_add(offset)
            .is_some_and(|end| end <= limit)
    })
}

/// The access the direct map gives a part of the kernel image mapped with `attributes`: the
/// same, never executable. Code and constants stay read-only through their direct-map alias,
/// so a stray or malicious write cannot change them there and W^X holds for every mapping of
/// a frame, not just the image's own.
pub const fn direct_map_attributes(attributes: Attributes) -> Attributes {
    Attributes {
        executable: false,
        ..attributes
    }
}

/// Maps every range of `ram` at its physical address + `offset`, read and write, never
/// executable, except the kernel image's frames: each of its parts gets
/// [`direct_map_attributes`] of its access, and the stack guard page stays unmapped.
fn map_direct_map<F: Format>(
    tables: &mut PageTables<F>,
    ram: &MemoryMap,
    offset: u64,
    image: &KernelImage,
    frames: &mut FrameAllocator,
) -> Result<(), MapError> {
    for range in ram.ranges() {
        let below_image = PhysicalRange {
            start: range.start,
            end: range.end.min(image.start),
        };
        let above_image = PhysicalRange {
            start: range.start.max(image.end),
            end: range.end,
        };
        let image_parts = image.mapped_parts().map(|(part, attributes)| {
            let inside = PhysicalRange {
                start: part.start.max(range.start),
                end: part.end.min(range.end),
            };
            (inside, direct_map_attributes(attributes))
        });
        let pieces = [
            (below_image, Attributes::READ_WRITE),
            (above_image, Attributes::READ_WRITE),
        ];
        for (piece, attributes) in pieces.into_iter().chain(image_parts) {
            if piece.is_empty() {
                continue;
            }
            let virt = piece
                .start
                .checked_add(offset)
                .ok_or(MapError::InvalidAddress)?;
            tables.map(virt, piece.start, piece.len(), attributes, frames)?;
        }
    }
    Ok(())
}

/// Builds the kernel's page tables (see [`kernel_page_tables`]) from the free memory in
/// `free_memory` and switches to them. From then on the frame allocator and the tables reach
/// physical memory through the direct map, and the frame allocator is [`memory::FRAMES`], so
/// the kernel heap works. Called once per boot: the heap keeps frames from the allocator this
/// installs, which a second call would replace.
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
    if overlaps_early_devices::<P>(free_memory) {
        return Err("free memory overlaps an early device's registers");
    }
    let mut frames = FrameAllocator::new(free_memory, P::physical_memory_offset());
    let mut page_tables =
        kernel_page_tables::<P>(&mut frames, &direct_map).map_err(MapError::description)?;
    // SAFETY: `kernel_page_tables` built tables in `P`'s format mapping the kernel image at
    // its virtual address with code executable and data and stack writable, and every range of
    // `P::physical_memory` and every early device region (as device memory) at the direct map
    // offset, and nothing in the low half. Their frames are never freed and nothing changes
    // them while they are active. Everything the kernel reaches is in the image or reached
    // through the direct map, as the boot page tables already had it.
    unsafe { P::activate_page_tables(page_tables.roots()) };
    page_tables.mark_active();
    memory::record_total_frames(frames.free_frames());
    frames.set_user_reserve(memory::user_reserve_for(frames.free_frames()));
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
        // (One of the 16 frames holds the buddy allocator's bitmap.)
        assert_eq!(15 - frames.free_frames(), 5);
        for virt in [start, 2 << 20, (4 << 20) + K - 1] {
            assert_eq!(tables.translate(virt).map(|t| t.0), Some(virt));
        }
    }

    #[test]
    fn the_direct_map_keeps_the_kernel_image_read_only_and_never_executable() {
        let mut frames = frames(64);
        let mut tables = PageTables::<X86_64>::new(&mut frames).unwrap();
        let mb = 1 << 20;
        // 8 MiB of RAM in two ranges; the image sits inside the first, away from 2 MiB
        // boundaries.
        let mut ram = MemoryMap::new();
        ram.add_ram(PhysicalRange::from_start_length(0, 6 * mb));
        ram.add_ram(PhysicalRange::from_start_length(8 * mb, 2 * mb));
        let base = mb + 3 * K;
        let image = KernelImage {
            virtual_offset: 0xffff_ffff_8000_0000,
            start: base,
            text_start: base + K,
            rodata_start: base + 4 * K,
            data_start: base + 6 * K,
            stack_guard_start: base + 9 * K,
            stack_start: base + 10 * K,
            end: base + 14 * K,
        };
        let offset = 0xffff_8000_0000_0000;
        map_direct_map(&mut tables, &ram, offset, &image, &mut frames).unwrap();
        let alias = |phys: u64| tables.translate(offset + phys);
        let rw = Some(Attributes::READ_WRITE);
        let ro = Some(Attributes::READ_ONLY);
        for (phys, expected) in [
            (0, rw),
            (base - 1, rw),
            (image.start, ro),
            (image.text_start, ro),
            (image.rodata_start - 1, ro),
            (image.rodata_start, ro),
            (image.data_start, rw),
            (image.stack_guard_start - 1, rw),
            (image.stack_guard_start, None),
            (image.stack_start - 1, None),
            (image.stack_start, rw),
            (image.end - 1, rw),
            (image.end, rw),
            (6 * mb - 1, rw),
            (6 * mb, None),
            (8 * mb, rw),
            (10 * mb - 1, rw),
        ] {
            assert_eq!(
                alias(phys).map(|t| t.0),
                expected.map(|_| phys),
                "{phys:#x}"
            );
            assert_eq!(alias(phys).map(|t| t.1), expected, "{phys:#x}");
        }
        // Away from the image, the largest leaves still map it: no page table under the
        // second range's 2 MiB entry, nor under the first range's last.
        for phys in [4 * mb, 8 * mb] {
            let walk = |level| {
                let mut table = tables.roots()[0];
                for l in (level + 1..4).rev() {
                    match X86_64::decode(tables.entry(table, offset + phys, l).read(), l) {
                        Entry::Table(next) => table = next,
                        other => return other,
                    }
                }
                X86_64::decode(tables.entry(table, offset + phys, level).read(), level)
            };
            assert!(matches!(walk(1), Entry::Leaf(..)), "{phys:#x}");
        }
    }

    #[test]
    fn the_direct_map_must_end_below_the_device_window() {
        let offset = 0xffff_ffc0_0000_0000;
        let window_start = 0xffff_ffff_4000_0000;
        let gb = 1 << 30;
        let ram = |end| {
            let mut map = MemoryMap::new();
            map.add_ram(PhysicalRange { start: 2 * gb, end });
            map
        };
        assert!(direct_map_fits_below(&ram(4 * gb), offset, window_start));
        let limit = 253 * gb;
        assert!(direct_map_fits_below(&ram(limit), offset, window_start));
        assert!(!direct_map_fits_below(
            &ram(limit + K),
            offset,
            window_start
        ));
        assert!(!direct_map_fits_below(&ram(300 * gb), offset, window_start));
    }

    #[test]
    fn the_device_window_is_the_root_entry_below_the_kernel_images() {
        let window = device_window::<crate::tests::TestPlatform>();
        assert_eq!(window, 0xffff_ff00_0000_0000..0xffff_ff80_0000_0000);
    }

    #[test]
    fn device_windows_map_device_memory_into_active_kernel_tables_with_large_leaves() {
        let mut frames = frames(64);
        let mut tables = PageTables::<X86_64>::new(&mut frames).unwrap();
        let window = 0xffff_ff00_0000_0000..0xffff_ff80_0000_0000;
        tables
            .leaf_table_entry(window.start, X86_64::LEVELS - 2, false, &mut frames)
            .unwrap();
        tables.mark_active();
        let root_entry = tables.entry(tables.roots[0], window.start, 3).read();
        let used_before = frames.free_frames();
        tables
            .map_device_window(&window, window.start, 0xb000_0000, 4 << 20, &mut frames)
            .unwrap();
        tables
            .map_device_window(
                &window,
                window.start + (4 << 20),
                0xfebd_1000,
                0x3000,
                &mut frames,
            )
            .unwrap();
        assert_eq!(used_before - frames.free_frames(), 2);
        assert_eq!(
            tables.entry(tables.roots[0], window.start, 3).read(),
            root_entry
        );
        let device = |virt| {
            tables
                .translate(virt)
                .map(|(phys, a)| (phys, a.memory_type))
        };
        assert_eq!(
            device(window.start + 0x30_0010),
            Some((0xb030_0010, MemoryType::Device))
        );
        assert_eq!(
            device(window.start + (4 << 20) + 0x2004),
            Some((0xfebd_3004, MemoryType::Device))
        );
        assert_eq!(device(window.start + (4 << 20) + 0x3000), None);
        let (_, level) = tables.leaf_entry(window.start).unwrap();
        assert_eq!(level, 1);
    }

    #[test]
    fn device_windows_refuse_ranges_outside_the_window_or_mapped_twice() {
        let mut frames = frames(64);
        let mut tables = PageTables::<Riscv64Sv39>::new(&mut frames).unwrap();
        let window = 0xffff_ffff_4000_0000..0xffff_ffff_8000_0000;
        let map = |tables: &mut PageTables<Riscv64Sv39>, virt, frames: &mut FrameAllocator| {
            tables.map_device_window(&window, virt, 0x1000_1000, 0x1000, frames)
        };
        assert_eq!(
            map(&mut tables, window.start - K, &mut frames),
            Err(MapError::OutsideDeviceWindow)
        );
        assert_eq!(
            map(&mut tables, window.end, &mut frames),
            Err(MapError::OutsideDeviceWindow)
        );
        assert_eq!(
            map(&mut tables, window.start + 1, &mut frames),
            Err(MapError::Unaligned)
        );
        assert_eq!(map(&mut tables, window.start, &mut frames), Ok(()));
        assert_eq!(
            map(&mut tables, window.start, &mut frames),
            Err(MapError::AlreadyMapped)
        );
        let gb =
            tables.map_device_window(&window, window.start + (1 << 21), 0, 1 << 30, &mut frames);
        assert_eq!(gb, Err(MapError::OutsideDeviceWindow));
        let mut user = PageTables::<Riscv64Sv39>::new(&mut frames).unwrap();
        user.user_half_only = true;
        assert_eq!(
            map(&mut user, window.start, &mut frames),
            Err(MapError::OutsideDeviceWindow)
        );
    }

    #[test]
    fn the_direct_map_splits_a_gigabyte_leaf_around_an_image_across_two_ranges() {
        let mut frames = frames(64);
        let mut tables = PageTables::<Riscv64Sv39>::new(&mut frames).unwrap();
        let gb = 1 << 30;
        // The image in the second GiB, across two RAM ranges with a one-page hole in its data
        // part; the first and third GiBs hold no image.
        let base = gb + (5 << 20);
        let image = KernelImage {
            virtual_offset: 0xffff_ffff_8000_0000,
            start: base,
            text_start: base,
            rodata_start: base + 2 * K,
            data_start: base + 3 * K,
            stack_guard_start: base + 6 * K,
            stack_start: base + 7 * K,
            end: base + 9 * K,
        };
        let mut ram = MemoryMap::new();
        ram.add_ram(PhysicalRange::from_start_length(0, base + 4 * K));
        ram.add_ram(PhysicalRange {
            start: base + 5 * K,
            end: 3 * gb,
        });
        let offset = 0xffff_ffc0_0000_0000;
        map_direct_map(&mut tables, &ram, offset, &image, &mut frames).unwrap();
        let alias = |phys: u64| tables.translate(offset + phys).map(|t| t.1);
        let rw = Some(Attributes::READ_WRITE);
        let ro = Some(Attributes::READ_ONLY);
        for (phys, expected) in [
            (image.text_start, ro),
            (image.data_start, rw),
            (base + 4 * K, None),
            (base + 5 * K, rw),
            (image.stack_guard_start, None),
            (image.stack_start, rw),
            (image.end, rw),
        ] {
            assert_eq!(alias(phys), expected, "{phys:#x}");
        }
        // The image-free GiBs keep a 1 GiB leaf; the image's GiB is split into tables.
        let root = |phys: u64| {
            Riscv64Sv39::decode(tables.entry(tables.roots()[0], offset + phys, 2).read(), 2)
        };
        assert!(matches!(root(0), Entry::Leaf(..)));
        assert!(matches!(root(2 * gb), Entry::Leaf(..)));
        assert!(matches!(root(gb), Entry::Table(_)));
    }

    /// The test platform's kernel page tables over its physical memory plus `extra`, which
    /// covers the platform's early device page at 0xa_0000.
    fn test_platform_tables(extra: &[PhysicalRange]) -> Result<PageTables<X86_64>, MapError> {
        use crate::tests::TestPlatform;
        let mut memory = TestPlatform::physical_memory();
        for range in extra {
            memory.add_ram(*range);
        }
        kernel_page_tables::<TestPlatform>(&mut frames(64), &memory)
    }

    #[test]
    fn a_device_page_inside_reported_memory_is_mapped_as_a_device_only() {
        use crate::tests::TestPlatform;
        // The device page with free frames on both sides, as a reserved firmware range
        // around the x86 local APIC would be.
        let tables =
            test_platform_tables(&[PhysicalRange::from_start_length(0x9_f000, 3 * K)]).unwrap();
        let offset = TestPlatform::direct_map_offset() as u64;
        let device = match X86_64::decode(X86_64::leaf(0, 0, Attributes::DEVICE), 0) {
            Entry::Leaf(_, attributes) => attributes,
            _ => unreachable!("a leaf"),
        };
        assert_eq!(
            tables.translate(offset + 0xa_0000),
            Some((0xa_0000, device))
        );
        for page in [0x9_f000, 0xa_1000] {
            assert_eq!(
                tables.translate(offset + page),
                Some((page, Attributes::READ_WRITE)),
                "{page:#x}"
            );
        }
    }

    #[test]
    fn leaving_a_device_out_of_a_full_memory_map_fails_instead_of_losing_memory() {
        use eggos_hal::memory::MAX_RANGES;
        // Splitting the range around the device page needs one range more than the map holds.
        let mut extra = vec![PhysicalRange::from_start_length(0x9_f000, 3 * K)];
        let full = MAX_RANGES - crate::tests::TestPlatform::physical_memory().ranges().len();
        extra.extend((1..full as u64).map(|i| PhysicalRange::from_start_length(i << 32, K)));
        assert_eq!(
            test_platform_tables(&extra).err(),
            Some(MapError::TooManyRanges)
        );
    }

    #[test]
    fn free_memory_on_an_early_device_page_is_refused() {
        use crate::tests::TestPlatform;
        let free = |start, length| {
            let mut map = MemoryMap::new();
            map.add_ram(PhysicalRange::from_start_length(start, length));
            map
        };
        // The test platform's device registers are at 0xa_0000..0xa_0100 and 0xa_0800..0xa_0900.
        assert!(overlaps_early_devices::<TestPlatform>(&free(
            0x9_f000,
            2 * K
        )));
        assert!(overlaps_early_devices::<TestPlatform>(&free(0xa_0000, K)));
        assert!(!overlaps_early_devices::<TestPlatform>(&free(0x9_f000, K)));
        assert!(!overlaps_early_devices::<TestPlatform>(&free(0xa_1000, K)));
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
        // 16 frames, less the buddy allocator's bitmap and the root.
        assert_eq!(frames.free_frames(), 14);
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

    /// On every format: the shared tables reach the kernel's mapping, never change the kernel
    /// half, and free exactly the frames they took; the kernel's tables are never freed.
    fn user_tables_leave_the_kernel_half_alone<F: Format>(high: u64) {
        let mut frames = frames(32);
        let mut kernel = PageTables::<F>::new(&mut frames).unwrap();
        kernel
            .map(high, 0x20_0000, K, Attributes::READ_WRITE, &mut frames)
            .unwrap();
        kernel.mark_active();
        let before = frames.free_frames();
        let mut user = PageTables::new_sharing_kernel_half(&kernel, &mut frames).unwrap();
        assert_eq!(user.translate(high), kernel.translate(high));
        for (virt, attributes) in [
            (high + K, Attributes::USER_READ_WRITE),
            (0x1000, Attributes::READ_WRITE),
            (F::USER_END - K, Attributes::READ_WRITE),
        ] {
            assert_eq!(
                user.map(virt, 0, K, attributes, &mut frames),
                Err(MapError::NotUser),
                "{virt:#x}"
            );
        }
        assert_eq!(user.unmap(high, K), Err(MapError::NotUser));
        user.map(
            F::USER_END - K,
            0,
            K,
            Attributes::USER_READ_WRITE,
            &mut frames,
        )
        .unwrap();
        assert_eq!(kernel.translate(F::USER_END - K), None);
        // SAFETY: from `new_sharing_kernel_half` with `frames`, never active, not used again.
        unsafe { user.free_user_half(&mut frames, |_, _, _| {}) };
        assert_eq!(frames.free_frames(), before);
        assert!(kernel.translate(high).is_some());
    }

    #[test]
    fn user_tables_leave_the_kernel_half_alone_on_every_format() {
        user_tables_leave_the_kernel_half_alone::<Aarch64>(0xffff_0000_0000_0000);
        user_tables_leave_the_kernel_half_alone::<Riscv64Sv39>(0xffff_ffc0_0000_0000);
        user_tables_leave_the_kernel_half_alone::<X86_64>(0xffff_8000_0000_0000);
    }

    #[test]
    #[should_panic(expected = "only a user address space's tables are freed")]
    fn the_kernels_tables_are_never_freed() {
        let mut frames = frames(4);
        let mut kernel = PageTables::<X86_64>::new(&mut frames).unwrap();
        // SAFETY: deliberately breaks the contract; the assertion stops it before any free.
        unsafe { kernel.free_user_half(&mut frames, |_, _, _| {}) };
    }

    #[test]
    fn user_pages_move_with_their_frames_and_go_back_when_tables_run_out() {
        let mut frames = frames(32);
        let mut kernel = PageTables::<Riscv64Sv39>::new(&mut frames).unwrap();
        kernel.mark_active();
        let mut user = PageTables::new_sharing_kernel_half(&kernel, &mut frames).unwrap();
        let (from, to) = (0x1000_0000, 0x20_0000_0000);
        let frame = |page: u64| 0x30_0000 + page * K;
        for page in 0..3 {
            let attributes = Attributes::USER_READ_WRITE;
            user.map(from + page * K, frame(page), K, attributes, &mut frames)
                .unwrap();
        }
        user.change_user_pages(
            from + K,
            K,
            |_, _| PageChange::Inaccessible,
            |_| unreachable!(),
        )
        .unwrap();
        let mapped_at = |user: &PageTables<Riscv64Sv39>, start: u64| {
            [0, 2].map(|page| user.translate(start + page * K).map(|(phys, _)| phys))
        };
        let mut taken = Vec::new();
        while frames.free_frames() > 1 {
            taken.push(frames.allocate().unwrap());
        }
        assert_eq!(
            user.move_user_pages(from, to, 4 * K, &mut frames),
            Err(MapError::OutOfMemory)
        );
        assert_eq!(mapped_at(&user, from), [Some(frame(0)), Some(frame(2))]);
        assert_eq!(mapped_at(&user, to), [None, None]);
        for frame in taken {
            // SAFETY: taken from `frames` above and used for nothing.
            unsafe { frames.free(frame) };
        }
        user.map(
            to + 3 * K,
            frame(3),
            K,
            Attributes::USER_READ_ONLY,
            &mut frames,
        )
        .unwrap();
        assert_eq!(
            user.move_user_pages(from, to, 4 * K, &mut frames),
            Err(MapError::AlreadyMapped)
        );
        assert_eq!(
            user.move_user_pages(from, from + K, 4 * K, &mut frames),
            Err(MapError::InvalidAddress)
        );
        assert_eq!(user.move_user_pages(from, to, 3 * K, &mut frames), Ok(()));
        assert_eq!(mapped_at(&user, from), [None, None]);
        assert_eq!(mapped_at(&user, to), [Some(frame(0)), Some(frame(2))]);
        let mut kept = Vec::new();
        user.change_user_pages(
            to,
            3 * K,
            |phys, attributes| {
                kept.push((phys, attributes.is_none()));
                PageChange::Map(Attributes::USER_READ_WRITE)
            },
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(
            kept,
            [(frame(0), false), (frame(1), true), (frame(2), false)]
        );
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
