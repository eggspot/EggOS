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
//! user translations both ways. A process's threads load its address space lazily instead
//! (D48): [`AddressSpace::load_lazily`] switches only when the CPU has another one loaded, and
//! the CPU keeps it, and a reference to its owner, until it needs a different one, so kernel
//! threads and a process's own threads running in turn pay no switch, and the scheduler's
//! switch knows nothing of address spaces. Changing the mappings takes `&mut self`, so it
//! never happens while the address space is active or loaded anywhere (an owner shared to be
//! loaded is frozen), and nothing cached of it outlives the change: no TLB maintenance per
//! page is needed yet.
//!
//! The frames user pages map belong to whoever mapped them; the address space owns only its
//! low half's tables, which it frees when dropped.

use crate::cpu::{self, Cpu};
use crate::memory;
use crate::paging::{MAX_ROOTS, MapError, PageTables};
use alloc::sync::Arc;
use core::any::Any;
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
    /// second one made active before the first is switched back is refused. One a process
    /// loaded lazily ([`Self::load_lazily`]) is unloaded first. The kernel reaches the user
    /// pages only through [`Platform::with_user_access`].
    pub fn with_active<R>(&self, active: impl FnOnce() -> R) -> R {
        let here = cpu::current::<P>();
        let root = self.tables.roots()[0];
        // A process's address space this CPU kept loaded is not in use: put it away first.
        unload_lazily_loaded::<P>(here);
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

    /// Makes this address space the one loaded on this CPU, for a thread of the process that
    /// `owner` is (D48): switches to it only if the CPU has another one loaded (or none), and
    /// keeps a reference to `owner`, so the tables stay allocated, until the CPU loads another
    /// one, [`unload_lazily_loaded`] or [`release_unused`] put it away, or [`Self::with_active`]
    /// needs the CPU. Call it with interrupts masked, as the kernel runs; it does not block.
    ///
    /// The address space stays loaded after the thread leaves the CPU: kernel threads run with
    /// it loaded, which they never use (the kernel reaches user pages only through
    /// [`Platform::with_user_access`], after loading the address space it means), and the next
    /// thread of the same process runs without a switch.
    ///
    /// # Safety
    /// `owner` must own this address space and keep it unchanged for as long as it lives
    /// (frozen: nothing reaches it through `&mut`).
    #[inline]
    pub unsafe fn load_lazily<O: Any + Send + Sync>(&self, owner: &Arc<O>) {
        let here = cpu::current::<P>();
        if here.active_address_space() != self.tables.roots()[0] {
            // SAFETY: the caller's guarantees.
            unsafe { self.switch_lazily(here, owner.clone()) };
        }
    }

    /// [`Self::load_lazily`]'s switch, out of line so the check stays a load and a compare.
    ///
    /// # Safety
    /// As [`Self::load_lazily`].
    #[cold]
    #[inline(never)]
    unsafe fn switch_lazily(&self, here: &'static Cpu, owner: Arc<dyn Any + Send + Sync>) {
        let previous = here.lazily_loaded.lock().take();
        assert!(
            previous.is_some() || here.active_address_space() == cpu::NO_ADDRESS_SPACE,
            "an address space was loaded lazily inside `with_active`"
        );
        let root = self.tables.roots()[0];
        here.set_active_address_space(root);
        // SAFETY: interrupts are masked (the caller's duty). The tables are in `P`'s format,
        // built by `PageTables`, and share the kernel's active half (`new`'s contract). `owner`
        // keeps them allocated and unchanged (the caller's duty) and is kept below until the
        // CPU switches away from them. The previous address space, if any, is no longer used:
        // `with_active` never runs around this call (checked above), and the kernel reaches
        // user pages only after loading the address space it means.
        unsafe { P::switch_address_space(self.tables.roots()) };
        let mut kernel_roots = self.kernel_roots;
        kernel_roots[self.tables.roots().len()..].fill(0);
        *here.lazily_loaded.lock() = Some(LazilyLoaded {
            owner,
            kernel_roots,
            root_count: self.tables.roots().len(),
        });
        // Freed, if this CPU held its last reference, now that it is not loaded. Other CPUs that
        // keep it are not woken here, which would cost every switch a look at every CPU: one
        // whose last look raced with this switch keeps it until something wakes it (roadmap).
        drop(previous);
    }

    /// Whether this address space is the one active or loaded on this CPU.
    #[inline]
    pub fn is_loaded_here(&self) -> bool {
        cpu::current::<P>().active_address_space() == self.root()
    }

    /// The first root of its tables: what a CPU records while it has the address space loaded.
    pub(crate) fn root(&self) -> u64 {
        self.tables.roots()[0]
    }
}

/// What keeps an address space a process loaded lazily on a CPU ([`AddressSpace::load_lazily`])
/// alive, and how to switch back to the kernel's tables from it.
pub(crate) struct LazilyLoaded {
    /// The process that owns the address space: holding it keeps the tables allocated.
    owner: Arc<dyn Any + Send + Sync>,
    kernel_roots: [u64; MAX_ROOTS],
    root_count: usize,
}

impl LazilyLoaded {
    /// Whether only CPUs that keep the address space loaded, whose first root is `root`, still
    /// refer to its owner: no thread or anything else does, so none can load it
    /// again, and every CPU may let it go. A reference taken or dropped while this looks can
    /// make the answer wrong either way, and both are harmless: a wrong "yes" makes a CPU drop
    /// its own reference, which never frees memory another holder uses (the space is loaded
    /// again where it is needed), and a wrong "no" leaves it loaded until the next look.
    fn only_cpus_hold_it(&self, here: &'static Cpu, root: u64) -> bool {
        let cpus = cpu::CPUS.all();
        let holding = if cpus.is_empty() {
            usize::from(here.active_address_space() == root)
        } else {
            cpus.iter()
                .filter(|cpu| cpu.active_address_space() == root)
                .count()
        };
        Arc::strong_count(&self.owner) <= holding
    }
}

/// If a process's address space is loaded lazily on `here`, this CPU, switches back to the
/// kernel's tables and drops the CPU's reference to it, which frees it if that was the last.
/// Call it with interrupts masked, as the kernel runs, and outside [`AddressSpace::with_active`].
pub(crate) fn unload_lazily_loaded<P: Platform>(here: &'static Cpu) {
    if here.active_address_space() == cpu::NO_ADDRESS_SPACE {
        return;
    }
    let Some(loaded) = here.lazily_loaded.lock().take() else {
        return;
    };
    unload::<P>(here, loaded);
}

/// When the CPU idles: puts away a process's address space loaded lazily here if only CPUs
/// still refer to it (its threads have ended), so its memory is freed instead of waiting for the
/// next process to run here, and has every other CPU that keeps it loaded look again (D48).
#[inline]
pub(crate) fn release_unused<P: Platform>(here: &'static Cpu) {
    if here.active_address_space() != cpu::NO_ADDRESS_SPACE {
        release_unused_cold::<P>(here);
    }
}

#[cold]
#[inline(never)]
fn release_unused_cold<P: Platform>(here: &'static Cpu) {
    let root = here.active_address_space();
    let loaded = {
        let mut slot = here.lazily_loaded.lock();
        if !slot
            .as_ref()
            .is_some_and(|loaded| loaded.only_cpus_hold_it(here, root))
        {
            return;
        }
        slot.take()
    };
    if let Some(loaded) = loaded {
        unload::<P>(here, loaded);
    }
}

/// For a process whose last thread just ended on `here`, this CPU, after it dropped its
/// reference: lets the address space whose first root is `root` go here if only CPUs still
/// hold it ([`release_unused`]), and has every other CPU that keeps it loaded look again, so
/// an idle one lets it go too (D48).
pub(crate) fn release_ended<P: Platform>(here: &'static Cpu, root: u64) {
    if here.active_address_space() == root {
        release_unused_cold::<P>(here);
    } else {
        wake_cpus_holding::<P>(here, root);
    }
}

/// Sends the wake-up interrupt to every CPU but `here` whose loaded address space's first root
/// is `root`: an idle one looks again ([`release_unused`]), a busy one is not disturbed beyond
/// the interrupt. The root may have been freed and reused meanwhile, which costs a spurious
/// look at most.
fn wake_cpus_holding<P: Platform>(here: &'static Cpu, root: u64) {
    for &other in cpu::CPUS.all() {
        if !ptr::eq(other, here) && other.active_address_space() == root {
            cpu::wake::<P>(other);
        }
    }
}

/// Switches `here`, this CPU, back to the kernel's tables from the address space `loaded` keeps,
/// and drops the CPU's reference to it; if only CPUs held it, has the others that keep it
/// loaded look again ([`release_unused`]).
fn unload<P: Platform>(here: &'static Cpu, loaded: LazilyLoaded) {
    let root = here.active_address_space();
    let ended = loaded.only_cpus_hold_it(here, root);
    // SAFETY: interrupts are masked (the callers' duty); back to the kernel's own tables, which
    // stay allocated for as long as the kernel runs (`AddressSpace::new`'s contract). The
    // address space switched away from stays allocated until `loaded` is dropped below, and
    // nothing uses it: the kernel reaches user pages only after loading the address space it
    // means.
    unsafe { P::switch_address_space(&loaded.kernel_roots[..loaded.root_count]) };
    here.set_active_address_space(cpu::NO_ADDRESS_SPACE);
    drop(loaded);
    if ended {
        wake_cpus_holding::<P>(here, root);
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
        // `self` while they are, and switches back before it returns, and a CPU that loaded them
        // lazily holds their owner until it has switched away. They are freed once, here, and
        // never used again.
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
