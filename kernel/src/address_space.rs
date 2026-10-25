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
//! switch knows nothing of address spaces.
//!
//! The tables sit behind the address space's lock. Adding a mapping ([`AddressSpace::map`])
//! takes `&self` and may happen while the address space is active or loaded on any CPU: a
//! process's thread maps the pages its program touches as it touches them (demand paging,
//! D52). That is sound because an added mapping changes nothing a CPU has cached: the entry
//! was invalid, which AArch64 and x86 never cache, and a RISC-V hart that did faults once, and
//! the fault handler drops the stale entry ([`Platform::drop_user_translations`]); the
//! frame's contents and new tables are ordered before the entry that makes them reachable.
//!
//! Removing or changing mappings of a live address space ([`AddressSpace::change_pages`], the
//! memory calls, D53) leaves translations CPUs may have cached behind: the CPU that changes
//! them drops its own, and every other CPU that keeps the address space loaded must drop
//! them before it next uses them. While a process has one thread, the one changing its
//! mappings, no other CPU uses its address space meanwhile, and the thread reaches another
//! CPU only through the scheduler's locks: those CPUs are only marked (`Cpu::mark_stale`), so
//! they switch to it again, dropping what they cached, before they next use it, with no
//! interrupt. Once a process starts a second thread ([`AddressSpace::spread_over_cpus`]),
//! its threads run on several CPUs at once, and a change shoots the old translations down
//! (D94): it asks every other CPU that keeps the address space loaded to drop its user
//! translations (`Cpu::request_shootdown`), sends it the wake-up interrupt, and waits until
//! each has, serving requests made of its own CPU meanwhile (`cpu::serve_shootdowns`), as
//! every CPU does on the wake-up interrupt and while it waits for a contended lock that is
//! not a leaf lock, so two CPUs that shoot each other down with interrupts masked both go on. Only then may the frames
//! unmapped go to someone else. [`AddressSpace::with_active`] on another CPU still needs the
//! address space unchanged meanwhile. [`AddressSpace::unmap`] takes `&mut self`, for an
//! address space not shared yet.
//!
//! The frames user pages map belong to whoever mapped them, unless the address space was made
//! to own them ([`AddressSpace::new_owning_frames`], as a process's is): then it frees them
//! with its low half's tables when dropped.

use crate::cpu::{self, Cpu};
use crate::memory;
use crate::paging::{MAX_ROOTS, MapError, PageChange, PageTables};
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use core::any::Any;
use core::marker::PhantomData;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering, fence};
use eggos_hal::Platform;
use eggos_hal::paging::{Attributes, Format};

/// A user address space. See the module documentation.
#[derive(Debug)]
pub struct AddressSpace<P: Platform> {
    tables: LeafSpinLock<PageTables<P::PageTableFormat>>,
    /// The tables' roots, which never change: what a CPU switches to, without the lock.
    roots: [u64; MAX_ROOTS],
    /// The kernel's tables' roots, to switch back to.
    kernel_roots: [u64; MAX_ROOTS],
    /// Whether the frames user pages map are the address space's, freed with it.
    owns_frames: bool,
    /// Whether threads using it may run on several CPUs at once ([`Self::spread_over_cpus`]):
    /// changes are then shot down on other CPUs instead of marking them stale. Never cleared.
    spread: AtomicBool,
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
        let mut roots = [0; MAX_ROOTS];
        roots[..tables.roots().len()].copy_from_slice(tables.roots());
        Ok(Self {
            tables: LeafSpinLock::new_leaf(tables),
            roots,
            kernel_roots,
            owns_frames: false,
            spread: AtomicBool::new(false),
            platform: PhantomData,
        })
    }

    /// As [`Self::new`], for an address space that owns the frames its user pages map: every
    /// one must be a 4 KiB frame from the kernel's frame allocator ([`memory::FRAMES`]) that
    /// nothing else frees, mapped once; dropping the address space frees them.
    ///
    /// # Safety
    /// As [`Self::new`].
    pub unsafe fn new_owning_frames(
        kernel: &PageTables<P::PageTableFormat>,
    ) -> Result<Self, MapError> {
        // SAFETY: the caller's guarantees.
        let mut space = unsafe { Self::new(kernel) }?;
        space.owns_frames = true;
        Ok(space)
    }

    /// From now on, threads using this address space may run on several CPUs at once: removed
    /// or changed mappings are shot down on every other CPU that keeps it loaded before
    /// [`Self::change_pages`] returns, not left for them to drop when they next use it (see the
    /// module documentation). Call it before a second thread that uses it can run.
    pub fn spread_over_cpus(&self) {
        self.spread.store(true, Ordering::Relaxed);
    }

    /// The physical addresses of its root tables, one per [`eggos_hal::paging::Format::ROOTS`].
    #[inline]
    fn roots(&self) -> &[u64] {
        &self.roots[..<P::PageTableFormat as Format>::ROOTS]
    }

    /// Maps user `virt..virt + length` to physical `phys..phys + length` with `attributes`,
    /// which must be a user mapping, in the low half ([`MapError::NotUser`] otherwise). As
    /// [`PageTables::map`] otherwise. Allowed while the address space is active or loaded
    /// anywhere (see the module documentation); a CPU that runs its program and may have cached
    /// the pages as unmapped drops that with [`Platform::drop_user_translations`].
    pub fn map(
        &self,
        virt: u64,
        phys: u64,
        length: u64,
        attributes: Attributes,
    ) -> Result<(), MapError> {
        let mut tables = self.tables.lock();
        let mut frames = memory::FRAMES.lock();
        let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
        tables.map(virt, phys, length, attributes, frames)
    }

    /// Unmaps user `virt..virt + length`, in the low half ([`MapError::NotUser`] otherwise).
    /// As [`PageTables::unmap`] otherwise. Not for an address space that owns its frames
    /// ([`Self::new_owning_frames`]): an unmapped frame would leak (its owner unmaps through
    /// [`Self::change_pages`], which hands the frames back, D53).
    pub fn unmap(&mut self, virt: u64, length: u64) -> Result<(), MapError> {
        assert!(
            !self.owns_frames,
            "an address space that owns its frames is unmapped through its owner"
        );
        self.tables.get_mut().unmap(virt, length)
    }

    /// Changes every 4 KiB page of user `virt..virt + length` that is mapped, or kept with no
    /// access ([`eggos_hal::paging::Entry::Kept`]), to what `change` says (see
    /// [`PageTables::change_user_pages`]), handing each frame unmapped to `unmapped`; then
    /// makes sure no CPU goes on using what it cached of the old entries (see the module
    /// documentation). Allowed while the address space is loaded on any CPU, and in use on
    /// others if it was spread over CPUs ([`Self::spread_over_cpus`]); otherwise used on at
    /// most this one. Call it with interrupts masked, holding no leaf lock
    /// ([`crate::sync::LeafSpinLock`]), and do not block until it returns: it may wait for
    /// other CPUs, which serve it while they wait for any other lock it holds.
    #[inline(always)]
    pub fn change_pages(
        &self,
        virt: u64,
        length: u64,
        change: impl FnMut(u64, Option<Attributes>) -> PageChange,
        unmapped: impl FnMut(u64),
    ) -> Result<(), MapError> {
        let result = self
            .tables
            .lock()
            .change_user_pages(virt, length, change, unmapped);
        // Even after an error: part of the range may have changed.
        self.drop_old_translations(virt, length, self.spread.load(Ordering::Relaxed));
        result
    }

    /// Unmaps every 4 KiB page of user `virt..virt + length` that is mapped or kept, handing
    /// each frame to `unmapped` once no CPU can write it through what it cached, so the caller
    /// may reuse it at once: as [`Self::change_pages`] with [`PageChange::Unmap`] for an
    /// address space not spread over CPUs. Spread, the pages are first made inaccessible,
    /// keeping their frames, and the old translations shot down; only then are they unmapped
    /// and the frames handed over, since another CPU could otherwise write a frame through
    /// its old translation after `unmapped` reused it (the frame lists of `munmap` are linked
    /// through the frames themselves). Meanwhile the program faults on them. Used as
    /// [`Self::change_pages`].
    #[inline(always)]
    pub fn unmap_pages(
        &self,
        virt: u64,
        length: u64,
        unmapped: impl FnMut(u64),
    ) -> Result<(), MapError> {
        let spread = self.spread.load(Ordering::Relaxed);
        if spread {
            self.take_out_of_use(virt, length);
        }
        let result =
            self.tables
                .lock()
                .change_user_pages(virt, length, |_, _| PageChange::Unmap, unmapped);
        if !spread {
            self.drop_old_translations(virt, length, false);
        }
        result
    }

    /// [`Self::unmap_pages`]' first step for an address space spread over CPUs: every page
    /// mapped in the range kept, inaccessible, and the old translations shot down. An error
    /// comes back from the second step, whose walk stops at the same page.
    #[cold]
    #[inline(never)]
    fn take_out_of_use(&self, virt: u64, length: u64) {
        let _ = self.tables.lock().change_user_pages(
            virt,
            length,
            |_, _| PageChange::Inaccessible,
            |_| unreachable!("nothing is unmapped"),
        );
        self.drop_old_translations(virt, length, true);
    }

    /// Moves every page of user `from..from + length` that is mapped or kept to the same
    /// offset from `to`, frames and contents with them, as [`PageTables::move_user_pages`]
    /// (nothing there may be mapped yet; on an error nothing has moved), then makes sure no CPU
    /// goes on using what it cached of the entries at `from`, or, after an error, of the
    /// entries at `to` that held pages until they went back. Used as [`Self::change_pages`].
    pub fn move_pages(&self, from: u64, to: u64, length: u64) -> Result<(), MapError> {
        let result = {
            let mut tables = self.tables.lock();
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
            tables.move_user_pages(from, to, length, frames)
        };
        let spread = self.spread.load(Ordering::Relaxed);
        match result {
            Ok(()) => self.drop_old_translations(from, length, spread),
            Err(_) => self.drop_old_translations(to, length, spread),
        }
        result
    }

    /// After mappings in user `virt..virt + length` were removed or changed: drops this
    /// CPU's cached translations of them if it has the address space loaded, and has every
    /// other CPU that keeps it loaded drop them: by a shootdown if it is `spread` over CPUs,
    /// else by marking it stale (`Cpu::mark_stale`). Increases too: a stale entry with less
    /// access raises a permission fault that is never retried (D52).
    #[inline(always)]
    fn drop_old_translations(&self, virt: u64, length: u64, spread: bool) {
        let here = cpu::current::<P>();
        let root = self.root();
        if here.active_address_space() == root {
            P::drop_user_translations(virt, length);
        }
        // The entries before any look at another CPU's record (`mark_stale` explains why a
        // record read before a switch is fine).
        fence(Ordering::SeqCst);
        if spread {
            shoot_down_elsewhere::<P>(cpu::CPUS.all(), here, root);
        } else {
            mark_stale_elsewhere(cpu::CPUS.all(), here, root);
        }
    }

    /// The physical address `virt` translates to in this address space and the mapping's
    /// attributes, or `None`. Kernel addresses translate as in the kernel's tables.
    pub fn translate(&self, virt: u64) -> Option<(u64, Attributes)> {
        self.tables.lock().translate(virt)
    }

    /// Runs `active` with this address space active on this CPU, then switches back to the
    /// kernel's tables, whose low half is empty. Call it with interrupts masked, as the kernel
    /// runs, and not nested. `active` must not block, yield or reach a preemption point: its
    /// thread could go on on another CPU, leaving this one with tables that may be freed, or
    /// another thread could make its own address space active here meanwhile. Either stops the
    /// kernel rather than go on (D46): each CPU records the address space active on it, and a
    /// second one made active before the first is switched back is refused. One a process
    /// loaded lazily ([`Self::load_lazily`]) is unloaded first. The kernel reaches the user
    /// pages only through [`Platform::with_user_access`]. Mappings may change meanwhile only
    /// from this CPU ([`Self::change_pages`]): one changed from another CPU marks this one
    /// stale, which the check on the way back stops at (D53), unless the address space is
    /// spread over CPUs, whose changes are shot down instead (D94).
    pub fn with_active<R>(&self, active: impl FnOnce() -> R) -> R {
        let here = cpu::current::<P>();
        let root = self.roots()[0];
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
        // never change); `&self` keeps them allocated, with mappings removed or changed only
        // through `change_pages`, which drops this CPU's translations of them (added ones are
        // allowed, see the module documentation), until the switch back below. Nothing
        // points into the kernel's low half, which is empty.
        unsafe { P::switch_address_space(self.roots()) };
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
        unsafe { P::switch_address_space(&self.kernel_roots[..self.roots().len()]) };
        result
    }

    /// Makes this address space the one loaded on this CPU, for a thread of the process that
    /// `owner` is (D48): switches to it only if the CPU has another one loaded (or none), and
    /// keeps a reference to `owner`, so the tables stay allocated, until the CPU loads another
    /// one, `unload_lazily_loaded` or `release_unused` put it away, or [`Self::with_active`]
    /// needs the CPU. Call it with interrupts masked, as the kernel runs; it does not block.
    ///
    /// The address space stays loaded after the thread leaves the CPU: kernel threads run with
    /// it loaded, which they never use (the kernel reaches user pages only through
    /// [`Platform::with_user_access`], after loading the address space it means), and the next
    /// thread of the same process runs without a switch.
    ///
    /// # Safety
    /// `owner` must own this address space and, for as long as it lives, remove or change
    /// mappings in it only through [`Self::change_pages`] (nothing reaches it through `&mut`);
    /// mappings may be added ([`Self::map`]).
    #[inline]
    pub unsafe fn load_lazily<O: Any + Send + Sync>(&self, owner: &Arc<O>) {
        let here = cpu::current::<P>();
        if here.active_address_space() != self.roots()[0] {
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
        // Held throughout (only this CPU takes it), so the slot is known empty when it is
        // filled below and nothing is dropped there.
        let mut slot = here.lazily_loaded.lock();
        let previous = slot.take();
        assert!(
            previous.is_some() || here.active_address_space() == cpu::NO_ADDRESS_SPACE,
            "an address space was loaded lazily inside `with_active`"
        );
        let root = self.roots()[0];
        here.set_active_address_space(root);
        // SAFETY: interrupts are masked (the caller's duty). The tables are in `P`'s format,
        // built by `PageTables`, and share the kernel's active half (`new`'s contract). `owner`
        // keeps them allocated, with mappings removed or changed only through `change_pages`,
        // which marks this CPU stale, or shoots its translations down if the address space is
        // spread over CPUs, while it is loaded here (the caller's duty), and is
        // kept below until the CPU switches away from them. The previous address space, if
        // any, is no longer used: `with_active` never runs around this call (checked above),
        // and the kernel reaches user pages only after loading the address space it means.
        unsafe { P::switch_address_space(self.roots()) };
        let mut kernel_roots = self.kernel_roots;
        kernel_roots[self.roots().len()..].fill(0);
        *slot = Some(LazilyLoaded {
            owner,
            kernel_roots,
            root_count: self.roots().len(),
        });
        drop(slot);
        // Freed, if this CPU held its last reference, now that it is not loaded. Other CPUs that
        // keep it are not woken here, which would cost every switch a look at every CPU: one
        // whose last look raced with this switch keeps it until something wakes it (roadmap).
        drop(previous);
    }

    /// Whether this address space is the one active or loaded on this CPU, and not marked
    /// stale (`Cpu::mark_stale`).
    #[inline]
    pub fn is_loaded_here(&self) -> bool {
        cpu::current::<P>().active_address_space() == self.root()
    }

    /// The first root of its tables: what a CPU records while it has the address space loaded.
    pub(crate) fn root(&self) -> u64 {
        self.roots()[0]
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
            usize::from(here.holds(root))
        } else {
            cpus.iter().filter(|cpu| cpu.holds(root)).count()
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
    let root = here.active_address_space() & !cpu::STALE;
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
    if here.holds(root) {
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
        if !ptr::eq(other, here) && other.holds(root) {
            cpu::wake::<P>(other);
        }
    }
}

/// Marks every CPU of `cpus` but `here` that keeps the address space whose first root is
/// `root` loaded stale (`Cpu::mark_stale`).
fn mark_stale_elsewhere(cpus: &[&'static Cpu], here: &'static Cpu, root: u64) {
    for &other in cpus {
        if !ptr::eq(other, here) && other.active_address_space() == root {
            other.mark_stale(root);
        }
    }
}

/// Asks every CPU of `cpus` but `here` that keeps the address space whose first root is `root`
/// loaded to drop its user translations, sends it the wake-up interrupt, and waits until each
/// CPU has served the shootdowns asked of it until then, serving those asked of `here`
/// meanwhile. Out of line: only processes with several threads take it.
#[cold]
#[inline(never)]
fn shoot_down_elsewhere<P: Platform>(cpus: &[&'static Cpu], here: &'static Cpu, root: u64) {
    let mut asked = false;
    for &other in cpus {
        if !ptr::eq(other, here) && other.holds(root) {
            other.request_shootdown();
            cpu::wake::<P>(other);
            asked = true;
        }
    }
    if !asked {
        return;
    }
    for &other in cpus {
        if ptr::eq(other, here) {
            continue;
        }
        let asked = other.shootdowns_requested();
        while other.shootdowns_served() < asked {
            cpu::serve_shootdowns::<P>();
            core::hint::spin_loop();
        }
    }
}

/// Switches `here`, this CPU, back to the kernel's tables from the address space `loaded` keeps,
/// and drops the CPU's reference to it; if only CPUs held it, has the others that keep it
/// loaded look again ([`release_unused`]). The CPU records that it has none loaded only after
/// the reference is dropped, so once every CPU records none, everything the last reference
/// kept is freed (D81).
fn unload<P: Platform>(here: &'static Cpu, loaded: LazilyLoaded) {
    let root = here.active_address_space() & !cpu::STALE;
    let ended = loaded.only_cpus_hold_it(here, root);
    // SAFETY: interrupts are masked (the callers' duty); back to the kernel's own tables, which
    // stay allocated for as long as the kernel runs (`AddressSpace::new`'s contract). The
    // address space switched away from stays allocated until `loaded` is dropped below, and
    // nothing uses it: the kernel reaches user pages only after loading the address space it
    // means.
    unsafe { P::switch_address_space(&loaded.kernel_roots[..loaded.root_count]) };
    drop(loaded);
    fence(Ordering::Release);
    here.set_active_address_space(cpu::NO_ADDRESS_SPACE);
    if ended {
        wake_cpus_holding::<P>(here, root);
    }
}

impl<P: Platform> Drop for AddressSpace<P> {
    fn drop(&mut self) {
        let owns_frames = self.owns_frames;
        let mut frames = memory::FRAMES.lock();
        let frames = frames
            .as_mut()
            .expect("the address space's tables came from the frame allocator");
        let free_owned = |frames: &mut memory::FrameAllocator, frame, level| {
            if owns_frames {
                assert_eq!(level, 0, "an address space owns only 4 KiB frames");
                // SAFETY: an owned frame came from this allocator and is the address space's
                // alone, mapped once (`new_owning_frames`' contract); its tables are going, so
                // nothing maps it any more.
                unsafe { frames.free(frame) };
            }
        };
        // SAFETY: the tables come from `new_sharing_kernel_half` with the kernel's frame
        // allocator, the one locked here. They are not active anywhere: `with_active` borrows
        // `self` while they are, and switches back before it returns, and a CPU that loaded them
        // lazily holds their owner until it has switched away. They are freed once, here, and
        // never used again.
        unsafe { self.tables.get_mut().free_user_half(frames, free_owned) };
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
        assert_ne!(space.roots(), kernel.roots());
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
        let space = new_space(&kernel).unwrap();
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
            let space = new_space(&kernel).unwrap();
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
        assert_eq!(root, space.roots()[0]);
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
            first.roots()[0],
            "the second never became active"
        );
        // As if the first thread had switched back: the next use starts clean.
        let here = cpu::current::<P>();
        assert_eq!(here.active_address_space(), first.roots()[0]);
        here.set_active_address_space(cpu::NO_ADDRESS_SPACE);
        // SAFETY: back to the kernel's own tables, as `with_active` would have.
        unsafe { P::switch_address_space(kernel.roots()) };
        assert_eq!(second.with_active(active_root), second.roots()[0]);
        assert_eq!(active_root(), kernel.roots()[0]);
    }

    #[test]
    fn user_attributes_survive_the_round_trip_through_the_tables() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let space = new_space(&kernel).unwrap();
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

    #[test]
    fn changing_pages_marks_the_other_cpus_that_keep_the_address_space_loaded_stale() {
        let (here, keeping, other, idle) = (
            Cpu::for_test(0),
            Cpu::for_test(1),
            Cpu::for_test(2),
            Cpu::for_test(3),
        );
        let root = 0x8_0000;
        for cpu in [here, keeping] {
            cpu.set_active_address_space(root);
        }
        other.set_active_address_space(0x9_0000);
        mark_stale_elsewhere(&[here, keeping, other, idle], here, root);
        assert_eq!(here.active_address_space(), root, "this CPU drops its own");
        assert_eq!(keeping.active_address_space(), root | cpu::STALE);
        assert_eq!(other.active_address_space(), 0x9_0000);
        assert_eq!(idle.active_address_space(), cpu::NO_ADDRESS_SPACE);
        // Still holding it, for the counts that let it go; marked once.
        assert!(keeping.holds(root));
        assert!(!idle.holds(root) && !other.holds(root));
        mark_stale_elsewhere(&[here, keeping], here, root);
        assert_eq!(keeping.active_address_space(), root | cpu::STALE);
    }

    #[test]
    fn a_shootdown_asks_the_cpus_that_hold_the_address_space_and_waits_until_they_serve_it() {
        let _state = crate::thread::host::lock_kernel_state_for_test();
        let (here, holding, other, idle) = (
            Cpu::for_test(0),
            Cpu::for_test(1),
            Cpu::for_test(2),
            Cpu::for_test(3),
        );
        let root = 0x8_0000;
        here.set_active_address_space(root);
        holding.set_active_address_space(root | cpu::STALE);
        other.set_active_address_space(0x9_0000);
        let served = std::thread::spawn(move || {
            loop {
                let requested = holding.shootdowns_requested();
                if requested > holding.shootdowns_served() {
                    cpu::serve_shootdowns_asked::<P>(holding, requested);
                    return requested;
                }
                std::hint::spin_loop();
            }
        });
        shoot_down_elsewhere::<P>(&[here, holding, other, idle], here, root);
        assert_eq!(holding.shootdowns_served(), 1, "waited until it was served");
        assert_eq!(served.join().unwrap(), 1);
        for cpu in [here, other, idle] {
            assert_eq!(
                (cpu.shootdowns_requested(), cpu.shootdowns_served()),
                (0, 0)
            );
        }
        assert_eq!(
            holding.active_address_space(),
            root | cpu::STALE,
            "left as it was"
        );
        shoot_down_elsewhere::<P>(&[here, other, idle], here, root);
        assert_eq!(
            holding.shootdowns_requested(),
            1,
            "nobody else holds it: no wait"
        );
    }

    #[test]
    fn a_cpu_serves_the_shootdowns_asked_of_it_up_to_when_it_looked() {
        let _state = crate::thread::host::lock_kernel_state_for_test();
        let cpu = Cpu::for_test(0);
        cpu.request_shootdown();
        cpu.request_shootdown();
        assert_eq!(
            (cpu.shootdowns_requested(), cpu.shootdowns_served()),
            (2, 0)
        );
        cpu::serve_shootdowns_asked::<P>(cpu, 1);
        assert_eq!(cpu.shootdowns_served(), 1);
        cpu::serve_shootdowns_asked::<P>(cpu, cpu.shootdowns_requested());
        assert_eq!(cpu.shootdowns_served(), 2);
    }

    #[test]
    fn unmapping_pages_of_a_spread_address_space_hands_every_frame_over() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free_frames();
        {
            // SAFETY: as in `new_space`.
            let space = unsafe { AddressSpace::<P>::new_owning_frames(&kernel) }.unwrap();
            space.spread_over_cpus();
            let frames: Vec<u64> = (0..2)
                .map(|_| FRAMES.lock().as_mut().unwrap().allocate().unwrap())
                .collect();
            for (i, &frame) in frames.iter().enumerate() {
                let virt = USER_VIRT + i as u64 * FRAME_SIZE;
                space
                    .map(virt, frame, FRAME_SIZE, Attributes::USER_READ_WRITE)
                    .unwrap();
            }
            space
                .change_pages(
                    USER_VIRT + FRAME_SIZE,
                    FRAME_SIZE,
                    |_, _| PageChange::Inaccessible,
                    |_| unreachable!(),
                )
                .unwrap();
            let mut handed = Vec::new();
            space
                .unmap_pages(USER_VIRT, 2 * FRAME_SIZE, |frame| handed.push(frame))
                .unwrap();
            assert_eq!(handed, frames, "a mapped page and a kept one");
            assert_eq!(space.translate(USER_VIRT), None);
            assert_eq!(space.translate(USER_VIRT + FRAME_SIZE), None);
            let mut frames_lock = FRAMES.lock();
            for frame in handed {
                // SAFETY: allocated above, unmapped and handed back by `unmap_pages`.
                unsafe { frames_lock.as_mut().unwrap().free(frame) };
            }
        }
        assert_eq!(free_frames(), before);
    }

    #[test]
    fn a_stale_address_space_is_switched_to_again_before_it_is_used() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let owner = Arc::new(new_space(&kernel).unwrap());
        // SAFETY: the test platform's switch only records roots; the owner is the space
        // itself, which changes mappings only through `change_pages`.
        unsafe { owner.load_lazily(&owner) };
        assert!(owner.is_loaded_here());
        crate::tests::set_active_root(0);
        cpu::current::<P>().mark_stale(owner.root());
        assert!(!owner.is_loaded_here());
        // SAFETY: as above.
        unsafe { owner.load_lazily(&owner) };
        assert_eq!(active_root(), owner.root(), "switched again");
        assert!(owner.is_loaded_here());
        unload_lazily_loaded::<P>(cpu::current::<P>());
        assert_eq!(active_root(), kernel.roots()[0]);
    }

    #[test]
    fn changed_pages_take_effect_and_kept_frames_are_freed_with_the_tables() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free_frames();
        {
            // SAFETY: as in `new_space`.
            let space = unsafe { AddressSpace::<P>::new_owning_frames(&kernel) }.unwrap();
            let frames: Vec<u64> = (0..3)
                .map(|_| FRAMES.lock().as_mut().unwrap().allocate().unwrap())
                .collect();
            for (i, &frame) in frames.iter().enumerate() {
                let virt = USER_VIRT + i as u64 * FRAME_SIZE;
                space
                    .map(virt, frame, FRAME_SIZE, Attributes::USER_READ_WRITE)
                    .unwrap();
            }
            // Across a large unmapped stretch too, which is skipped a table at a time.
            let mut seen = Vec::new();
            space
                .change_pages(
                    0,
                    1 << 30,
                    |frame, attributes| {
                        seen.push((frame, attributes));
                        match seen.len() {
                            1 => PageChange::Inaccessible,
                            2 => PageChange::Map(Attributes::USER_READ_ONLY),
                            _ => PageChange::Unmap,
                        }
                    },
                    |frame| {
                        assert_eq!(frame, frames[2]);
                        // SAFETY: allocated above, and no entry maps it any more.
                        unsafe { FRAMES.lock().as_mut().unwrap().free(frame) };
                    },
                )
                .unwrap();
            let rw = Some(Attributes::USER_READ_WRITE);
            assert_eq!(seen, [(frames[0], rw), (frames[1], rw), (frames[2], rw)]);
            assert_eq!(space.translate(USER_VIRT), None);
            assert_eq!(
                space.translate(USER_VIRT + FRAME_SIZE),
                Some((frames[1], Attributes::USER_READ_ONLY))
            );
            assert_eq!(space.translate(USER_VIRT + 2 * FRAME_SIZE), None);
            // A kept page is seen as such, and can be mapped again; nothing maps over it.
            assert_eq!(
                space.map(
                    USER_VIRT,
                    frames[2],
                    FRAME_SIZE,
                    Attributes::USER_READ_WRITE
                ),
                Err(MapError::AlreadyMapped)
            );
            let mut kept = Vec::new();
            space
                .change_pages(
                    USER_VIRT,
                    FRAME_SIZE,
                    |frame, attributes| {
                        kept.push((frame, attributes));
                        PageChange::Inaccessible
                    },
                    |_| unreachable!(),
                )
                .unwrap();
            assert_eq!(kept, [(frames[0], None)]);
            assert_eq!(
                space.change_pages(KERNEL_VIRT, FRAME_SIZE, |_, _| PageChange::Unmap, |_| ()),
                Err(MapError::NotUser)
            );
            assert_eq!(
                space.change_pages(
                    USER_VIRT,
                    FRAME_SIZE,
                    |_, _| PageChange::Map(Attributes::READ_WRITE),
                    |_| ()
                ),
                Err(MapError::NotUser)
            );
        }
        // The kept frame and the read-only one go with the tables.
        assert_eq!(free_frames(), before);
    }
}
