//! User address spaces (M4, D45): the low half of the address space, user space, with page
//! tables of its own, and the kernel half shared with the kernel's tables.
//!
//! An [`AddressSpace`] maps user pages ([`Attributes::user`]) at addresses below the format's
//! [`Format::USER_END`]; everything above is the kernel's, the same in every address space: on
//! AArch64 the kernel's `TTBR1_EL1` root itself, elsewhere copies of the kernel's root-level
//! entries, pointing to the same tables. That holds because the kernel's tables never change
//! once active ([`PageTables::map`] refuses), so every kernel-half table exists from boot on.
//!
//! [`AddressSpace::with_active`] runs code with the address space active on its CPU and
//! switches back to the kernel's empty low half afterwards; the switch drops the CPU's cached
//! user translations both ways. Changing the mappings takes `&mut self`, so it never happens
//! while the address space is active anywhere, and nothing cached of it outlives the change:
//! no TLB maintenance per page is needed yet. Threads that carry an address space, user mode
//! and switching address spaces on a context switch come next (roadmap M4).
//!
//! The frames user pages map belong to whoever mapped them; the address space owns only its
//! low half's tables, which it frees when dropped.

use crate::cpu;
use crate::memory;
use crate::paging::{MAX_ROOTS, MapError, PageTables};
use core::marker::PhantomData;
use core::ptr;
use eggos_hal::Platform;
use eggos_hal::paging::Attributes;

/// A user address space. See the module documentation.
#[derive(Debug)]
pub struct AddressSpace<P: Platform> {
    tables: PageTables<P::PageTableFormat>,
    /// The kernel's tables' roots, to switch back to.
    kernel_roots: [u64; MAX_ROOTS],
    platform: PhantomData<fn() -> P>,
}

impl<P: Platform> AddressSpace<P> {
    /// An empty address space sharing the kernel half of `kernel`; its tables come from the
    /// kernel's frame allocator ([`memory::FRAMES`]).
    ///
    /// # Safety
    /// `kernel` must be the kernel's own page tables, active on every CPU that runs
    /// [`Self::with_active`] (the tables [`crate::paging::enable`] activated): `with_active`
    /// loads roots whose kernel half is copied from them, and switches back to them.
    pub unsafe fn new(kernel: &PageTables<P::PageTableFormat>) -> Result<Self, MapError> {
        let mut frames = memory::FRAMES.lock();
        let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
        let tables = PageTables::new_sharing_kernel_half(kernel, frames)?;
        let mut kernel_roots = [0; MAX_ROOTS];
        kernel_roots[..kernel.roots().len()].copy_from_slice(kernel.roots());
        Ok(Self {
            tables,
            kernel_roots,
            platform: PhantomData,
        })
    }

    /// Maps user `virt..virt + length` to physical `phys..phys + length` with `attributes`,
    /// which must be a user mapping, in the low half ([`MapError::NotUser`] otherwise). As
    /// [`PageTables::map`] otherwise.
    pub fn map(
        &mut self,
        virt: u64,
        phys: u64,
        length: u64,
        attributes: Attributes,
    ) -> Result<(), MapError> {
        let mut frames = memory::FRAMES.lock();
        let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
        self.tables.map(virt, phys, length, attributes, frames)
    }

    /// Unmaps user `virt..virt + length`, in the low half ([`MapError::NotUser`] otherwise).
    /// As [`PageTables::unmap`] otherwise.
    pub fn unmap(&mut self, virt: u64, length: u64) -> Result<(), MapError> {
        self.tables.unmap(virt, length)
    }

    /// The physical address `virt` translates to in this address space and the mapping's
    /// attributes, or `None`. Kernel addresses translate as in the kernel's tables.
    pub fn translate(&self, virt: u64) -> Option<(u64, Attributes)> {
        self.tables.translate(virt)
    }

    /// Runs `active` with this address space active on this CPU, then switches back to the
    /// kernel's tables, whose low half is empty. Call it with interrupts masked, as the kernel
    /// runs, and not nested. `active` must not block, yield or reach a preemption point: its
    /// thread could go on on another CPU, leaving this one with tables that may be freed, or
    /// another thread could make its own address space active here meanwhile. Either stops the
    /// kernel rather than go on (D46): each CPU records the address space active on it, and a
    /// second one made active before the first is switched back is refused. The kernel reaches
    /// the user pages only through [`Platform::with_user_access`].
    pub fn with_active<R>(&self, active: impl FnOnce() -> R) -> R {
        let here = cpu::current::<P>();
        let root = self.tables.roots()[0];
        assert!(
            here.active_address_space() == cpu::NO_ADDRESS_SPACE,
            "an address space was made active over another one on this CPU"
        );
        here.set_active_address_space(root);
        // SAFETY: interrupts are masked (the caller's duty, as the kernel runs). The tables
        // are in `P`'s format, built by `PageTables`, and their kernel half is the kernel's
        // active one: `new`'s caller guaranteed `kernel_roots` are the active kernel tables,
        // which `new_sharing_kernel_half` shared (it refuses tables not marked active, which
        // never change); `&self` keeps them allocated and unchanged until the switch back
        // below. Nothing points into the kernel's low half, which is empty.
        unsafe { P::switch_address_space(self.tables.roots()) };
        let result = active();
        assert!(
            ptr::eq(cpu::current::<P>(), here),
            "a thread left its CPU while an address space was active there"
        );
        assert!(
            here.active_address_space() == root,
            "another address space was made active on this CPU meanwhile"
        );
        here.set_active_address_space(cpu::NO_ADDRESS_SPACE);
        // SAFETY: as above, back to the kernel's own tables (`new`'s contract), which stay
        // allocated for as long as the kernel runs; `active` has returned, so nothing uses this
        // address space's low half.
        unsafe { P::switch_address_space(&self.kernel_roots[..self.tables.roots().len()]) };
        result
    }
}

impl<P: Platform> Drop for AddressSpace<P> {
    fn drop(&mut self) {
        let mut frames = memory::FRAMES.lock();
        let frames = frames
            .as_mut()
            .expect("the address space's tables came from the frame allocator");
        // SAFETY: the tables come from `new_sharing_kernel_half` with the kernel's frame
        // allocator, the one locked here. They are not active anywhere: `with_active` borrows
        // `self` while they are, and switches back before it returns. They are freed once, here,
        // and never used again.
        unsafe { self.tables.free_user_half(frames) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FRAMES;
    use crate::tests::{TestPlatform as P, active_root};
    use crate::thread::host::frames_for_test;
    use eggos_hal::memory::FRAME_SIZE;
    use eggos_hal::paging::{MemoryType, leaf_size};

    type Format = <P as Platform>::PageTableFormat;

    /// A kernel address the test's kernel tables map, and where.
    const KERNEL_VIRT: u64 = 0xffff_ffff_8000_0000;
    const KERNEL_PHYS: u64 = 0x8_0000;
    const USER_VIRT: u64 = 0x40_0000;

    fn free_frames() -> u64 {
        FRAMES.lock().as_ref().unwrap().free_frames()
    }

    /// Kernel tables mapping one page at [`KERNEL_VIRT`], active (frozen) as the kernel's are.
    fn kernel_tables() -> PageTables<Format> {
        let mut frames = FRAMES.lock();
        let frames = frames.as_mut().unwrap();
        let mut tables = PageTables::new(frames).unwrap();
        tables
            .map(
                KERNEL_VIRT,
                KERNEL_PHYS,
                FRAME_SIZE,
                Attributes::READ_WRITE,
                frames,
            )
            .unwrap();
        tables.mark_active();
        tables
    }

    fn new_space(kernel: &PageTables<Format>) -> Result<AddressSpace<P>, MapError> {
        // SAFETY: the test platform's switch only records the root; `kernel` stands in for the
        // kernel's active tables (marked active).
        unsafe { AddressSpace::new(kernel) }
    }

    #[test]
    #[should_panic(expected = "an address space shares only the kernel's active tables")]
    fn tables_not_active_are_never_shared() {
        let _frames = frames_for_test();
        let mut frames = FRAMES.lock();
        let tables = PageTables::<Format>::new(frames.as_mut().unwrap()).unwrap();
        drop(frames);
        let _ = new_space(&tables);
    }

    #[test]
    fn a_new_address_space_shares_the_kernel_half_and_maps_nothing_below() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let space = new_space(&kernel).unwrap();
        assert_ne!(space.tables.roots(), kernel.roots());
        assert_eq!(space.translate(KERNEL_VIRT), kernel.translate(KERNEL_VIRT));
        assert_eq!(space.translate(USER_VIRT), None);
        assert_eq!(space.translate(0), None);
    }

    #[test]
    fn user_pages_map_and_unmap_in_the_low_half_only() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut space = new_space(&kernel).unwrap();
        space
            .map(
                USER_VIRT,
                0x20_0000,
                FRAME_SIZE,
                Attributes::USER_READ_WRITE,
            )
            .unwrap();
        assert_eq!(
            space.translate(USER_VIRT + 0x123),
            Some((0x20_0123, Attributes::USER_READ_WRITE))
        );
        assert_eq!(kernel.translate(USER_VIRT), None, "the kernel's low half");
        assert_eq!(
            space.map(
                USER_VIRT + FRAME_SIZE,
                0,
                FRAME_SIZE,
                Attributes::READ_WRITE
            ),
            Err(MapError::NotUser),
            "kernel attributes"
        );
        let user_end = <Format as eggos_hal::paging::Format>::USER_END;
        for (virt, length) in [
            (user_end - FRAME_SIZE, 2 * FRAME_SIZE),
            (KERNEL_VIRT, FRAME_SIZE),
            (u64::MAX - FRAME_SIZE + 1, FRAME_SIZE),
        ] {
            assert_eq!(
                space.map(virt, 0, length, Attributes::USER_READ_WRITE),
                Err(MapError::NotUser),
                "{virt:#x}"
            );
            assert_eq!(space.unmap(virt, length), Err(MapError::NotUser));
        }
        space.unmap(USER_VIRT, FRAME_SIZE).unwrap();
        assert_eq!(space.translate(USER_VIRT), None);
        assert_eq!(space.unmap(USER_VIRT, FRAME_SIZE), Err(MapError::NotMapped));
        assert_eq!(space.translate(KERNEL_VIRT), kernel.translate(KERNEL_VIRT));
    }

    #[test]
    fn the_last_page_before_the_non_canonical_hole_is_never_user_space() {
        // A `syscall` at its end would return to a non-canonical address (x86_64).
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut space = new_space(&kernel).unwrap();
        let last = (1 << 47) - FRAME_SIZE;
        assert_eq!(
            space.map(last, 0, FRAME_SIZE, Attributes::USER_TEXT),
            Err(MapError::NotUser)
        );
        space
            .map(last - FRAME_SIZE, 0, FRAME_SIZE, Attributes::USER_TEXT)
            .unwrap();
    }

    #[test]
    fn unmapping_part_of_a_large_page_fails_and_the_whole_page_unmaps() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut space = new_space(&kernel).unwrap();
        let large = leaf_size(1);
        space
            .map(large, 0x4000_0000, large, Attributes::USER_READ_ONLY)
            .unwrap();
        assert_eq!(
            space.unmap(large, FRAME_SIZE),
            Err(MapError::PartOfLargePage)
        );
        assert_eq!(
            space.unmap(large + FRAME_SIZE, large),
            Err(MapError::PartOfLargePage)
        );
        assert!(space.translate(large).is_some());
        space.unmap(large, large).unwrap();
        assert_eq!(space.translate(large), None);
    }

    #[test]
    fn dropping_an_address_space_frees_every_table_it_allocated_and_no_kernel_table() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free_frames();
        {
            let mut space = new_space(&kernel).unwrap();
            // Pages far apart, so each needs tables of its own at every level.
            for (i, virt) in [0, USER_VIRT, 1 << 30, 1 << 39, (1 << 47) - 2 * FRAME_SIZE]
                .into_iter()
                .enumerate()
            {
                space
                    .map(
                        virt,
                        i as u64 * FRAME_SIZE,
                        FRAME_SIZE,
                        Attributes::USER_TEXT,
                    )
                    .unwrap();
            }
            assert!(free_frames() < before - 5);
        }
        assert_eq!(free_frames(), before);
        assert_eq!(
            kernel.translate(KERNEL_VIRT),
            Some((KERNEL_PHYS, Attributes::READ_WRITE)),
            "the kernel's tables are intact"
        );
    }

    #[test]
    fn with_active_switches_to_the_address_space_and_back_to_the_kernel() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let space = new_space(&kernel).unwrap();
        let root = space.with_active(active_root);
        assert_eq!(root, space.tables.roots()[0]);
        assert_eq!(active_root(), kernel.roots()[0]);
    }

    #[test]
    fn an_address_space_is_never_made_active_over_another() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let [first, second] = [new_space(&kernel).unwrap(), new_space(&kernel).unwrap()];
        let nested = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            first.with_active(|| second.with_active(|| ()))
        }));
        assert!(nested.is_err());
        assert_eq!(
            active_root(),
            first.tables.roots()[0],
            "the second never became active"
        );
        // As if the first thread had switched back: the next use starts clean.
        let here = cpu::current::<P>();
        assert_eq!(here.active_address_space(), first.tables.roots()[0]);
        here.set_active_address_space(cpu::NO_ADDRESS_SPACE);
        // SAFETY: back to the kernel's own tables, as `with_active` would have.
        unsafe { P::switch_address_space(kernel.roots()) };
        assert_eq!(second.with_active(active_root), second.tables.roots()[0]);
        assert_eq!(active_root(), kernel.roots()[0]);
    }

    #[test]
    fn user_attributes_survive_the_round_trip_through_the_tables() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut space = new_space(&kernel).unwrap();
        for (i, attributes) in [
            Attributes::USER_TEXT,
            Attributes::USER_READ_ONLY,
            Attributes::USER_READ_WRITE,
            Attributes {
                memory_type: MemoryType::Device,
                ..Attributes::USER_READ_WRITE
            },
        ]
        .into_iter()
        .enumerate()
        {
            let virt = USER_VIRT + i as u64 * FRAME_SIZE;
            space.map(virt, virt, FRAME_SIZE, attributes).unwrap();
            assert_eq!(space.translate(virt), Some((virt, attributes)));
        }
    }
}
