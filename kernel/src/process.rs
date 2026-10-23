//! Processes and their threads in user mode (M4, D47, D48).
//!
//! A [`Process`] owns a user address space, the frames its pages map and its [`Regions`]: what
//! its program may use. It is built with `&mut` ([`Process::map_new_page`],
//! [`Process::add_region`]), then shared as an `Arc` by the threads that run its program
//! ([`spawn`]). From then on the first touch of a page in a region maps a zeroed frame there
//! ([`Process::resolve_page_fault`], demand paging, D52), and the memory calls (D53) add,
//! remove and change regions with their pages: [`Process::map_region`] (`mmap`),
//! [`Process::unmap_range`] (`munmap`), [`Process::protect_range`] (`mprotect`) and
//! [`Process::move_program_break`] (`brk`). Each such thread runs in a loop
//! (D47's "run until trap"): it loads the process's address space on its CPU if that is not
//! already the one loaded there ([`AddressSpace::load_lazily`], D48), runs the program in user
//! mode with [`Platform::run_user`] until a system call, an interrupt or an exception takes the
//! CPU back, and hands the exit to the thread's handler, which says whether to go on. Before
//! each run it passes a preemption point ([`thread::preemption_point_before_user_mode`]), so a
//! thread whose slice is over, after an interrupt or a system call, lets the next one run:
//! preemption on return to user mode.
//!
//! The handler stands in for the system calls until the Linux ABI (M6) and for signals: it gets
//! the program's registers ([`UserContext`]) and may block, sleep or yield, since the loop loads
//! the address space again before the program runs. The kernel reaches the process's pages
//! with [`Process::with_user_memory`], which loads it first, so a handler that blocked and
//! resumed after another process ran on its CPU still reaches its own.
//!
//! A Linux program is mapped from its ELF image by [`crate::linux::exec::load`] (D51); the
//! HAL's test program ([`Process::with_test_program`]) serves the boot self-tests and
//! benchmarks. Reading images from files comes with the file system (M7).

use crate::address_space::{self, AddressSpace};
use crate::cpu;
use crate::memory;
use crate::paging::{MapError, PageChange, PageTables};
use crate::regions::{Access, Region, RegionError, Regions};
use crate::sync::SpinLock;
use crate::thread::{self, JoinHandle, SpawnError};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType, PageFault};
use eggos_hal::user::{TestProgram, UserContext, UserExit};

/// Where [`Process::with_test_program`] maps the HAL's test program: its first page.
pub const TEST_PROGRAM_ADDRESS: u64 = 0x40_0000;

/// How a process ended ([`Process::exit`]), as its parent will learn it (`wait4`, M6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessExit {
    /// It ended itself with this status (Linux keeps its low 8 bits).
    Exited(u32),
    /// The kernel ended it with this signal, say after an exception it did not handle.
    Killed { signal: u32 },
}

/// The user address `address` and `length` bytes after it are not all user memory the access
/// may reach: unmapped, kernel, device or (for a write) read-only pages, or past
/// [`Format::USER_END`]. A Linux system call reports it as `EFAULT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUserAddress;

/// Why a page fault in a process's program was not resolved ([`Process::resolve_page_fault`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedFault {
    /// The address is in no region, or its region does not allow the access: the program's
    /// own error (Linux sends `SIGSEGV`).
    NotAllowed,
    /// There was no frame for the page (Linux's out-of-memory killer sends `SIGKILL`).
    OutOfMemory,
}

/// Where [`Process::map_region`] puts a new region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// At this address, page-aligned, replacing whatever held part of the range before, pages
    /// and all (Linux's `MAP_FIXED`).
    Replace(u64),
    /// At this address, page-aligned, if no region holds any of the range
    /// ([`RegionError::Overlaps`] otherwise; Linux's `MAP_FIXED_NOREPLACE`).
    Exactly(u64),
    /// At `hint`, if it is given and the range there is free and within `floor..top`;
    /// otherwise as high as a free range fits within `floor..top` ([`RegionError::NoRoom`] if
    /// none does), as Linux places a mapping top-down.
    Anywhere {
        hint: Option<u64>,
        floor: u64,
        top: u64,
    },
}

/// The program break (Linux's `brk`): where the heap a program grows with `brk` begins, and
/// where it ends now. The heap is a region of its own from `start`, page-aligned, to `end`
/// rounded up to a page, joined with the data segment below it as any touching region with
/// the same attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProgramBreak {
    start: u64,
    end: u64,
}

/// A user address space, the frames its pages map and its regions. See the module
/// documentation.
pub struct Process<P: Platform> {
    /// Owns the frames its user pages map ([`AddressSpace::new_owning_frames`]), so they are
    /// freed with it.
    space: AddressSpace<P>,
    /// What the program may use. Held while pages are mapped, unmapped or changed in the
    /// address space after it is shared (demand paging, the memory calls), and while the kernel
    /// copies user memory, so a copy never meets a page being mapped or unmapped. Taken before
    /// the address space's own lock.
    regions: SpinLock<Regions>,
    /// The program break, once the program has one ([`Self::set_program_break`]). Taken
    /// before the regions' lock.
    program_break: SpinLock<Option<ProgramBreak>>,
    /// Where the kernel reaches the process's frames: the frame allocator's offset, which on
    /// the machine is the direct map's (a boot self-test checks it).
    frame_offset: usize,
    /// How the process ended, once it has: the first [`Self::exit`] wins.
    exit: SpinLock<Option<ProcessExit>>,
}

impl<P: Platform> Process<P> {
    /// A process with an empty address space sharing the kernel half of `kernel`.
    ///
    /// # Safety
    /// As [`AddressSpace::new`]: `kernel` must be the kernel's own page tables, active on every
    /// CPU that runs the process.
    pub unsafe fn new(kernel: &PageTables<P::PageTableFormat>) -> Result<Self, &'static str> {
        // SAFETY: the caller's guarantees.
        let space = unsafe { AddressSpace::new_owning_frames(kernel) }
            .map_err(|error| error.description())?;
        let frame_offset = memory::FRAMES
            .lock()
            .as_ref()
            .ok_or("there is no frame allocator")?
            .physical_memory_offset();
        Ok(Self {
            space,
            regions: SpinLock::new(Regions::new()),
            program_break: SpinLock::new(None),
            frame_offset,
            exit: SpinLock::new(None),
        })
    }

    /// A process whose address space maps the HAL's test program
    /// ([`Platform::user_test_program`]) at [`TEST_PROGRAM_ADDRESS`], [`Attributes::USER_TEXT`].
    ///
    /// # Safety
    /// As [`Self::new`].
    pub unsafe fn with_test_program(
        kernel: &PageTables<P::PageTableFormat>,
    ) -> Result<Self, &'static str> {
        // SAFETY: the caller's guarantees.
        let mut process = unsafe { Self::new(kernel) }?;
        process.map_new_page(
            TEST_PROGRAM_ADDRESS,
            Attributes::USER_TEXT,
            P::user_test_program().code,
        )?;
        Ok(process)
    }

    /// A context that starts the test program at `entry`, an offset from the HAL's
    /// [`TestProgram`] (say, its `checks`), with no stack.
    pub fn test_program_context(entry: impl FnOnce(&TestProgram) -> usize) -> UserContext {
        UserContext::new(
            TEST_PROGRAM_ADDRESS + entry(&P::user_test_program()) as u64,
            0,
        )
    }

    /// Maps a new page at user address `virt` with `attributes`, a user mapping: a frame of its
    /// own, holding `contents` and zero after them, so no stale kernel data is left where the
    /// program can read. An executable page is made visible to instruction fetch. While the
    /// process is built; its regions are added separately ([`Self::add_region`]).
    pub fn map_new_page(
        &mut self,
        virt: u64,
        attributes: Attributes,
        contents: &[u8],
    ) -> Result<(), &'static str> {
        if contents.len() > FRAME_SIZE as usize {
            return Err("a page's contents are larger than a page");
        }
        self.map_frame(virt, attributes, contents)
            .map(|_| ())
            .map_err(|error| match error {
                MapError::OutOfMemory => "no frame for a user page",
                _ => "not a user page",
            })
    }

    /// Records that user addresses `start..end`, page-aligned, are a region with `attributes`
    /// (see [`Regions`]): pages there not mapped yet are mapped, zeroed, when first touched.
    /// While the process is built.
    pub fn add_region(
        &mut self,
        start: u64,
        end: u64,
        attributes: Attributes,
    ) -> Result<(), RegionError> {
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(RegionError::Invalid);
        }
        self.regions.get_mut().insert(start, end, Some(attributes))
    }

    /// Sets the program break's start and end to `start`, page-aligned: where the heap `brk`
    /// grows begins (Linux: the end of the program's last segment). While the process is
    /// built.
    pub fn set_program_break(&mut self, start: u64) {
        debug_assert!(start.is_multiple_of(FRAME_SIZE));
        *self.program_break.get_mut() = Some(ProgramBreak { start, end: start });
    }

    /// Moves the program break's end to `requested` (Linux's `brk`) and returns where it is
    /// then: `requested`, or where it was if the move is refused, because `requested` is
    /// below the break's start or above `limit`, or the pages it adds are in another region
    /// already. Pages the move gives up are unmapped and their frames freed; pages it adds
    /// are mapped, zeroed, as the program first touches them.
    pub fn move_program_break(&self, requested: u64, limit: u64) -> u64 {
        let mut program_break = self.program_break.lock();
        let Some(ProgramBreak { start, end }) = *program_break else {
            return 0;
        };
        if requested < start || requested > limit.min(<P::PageTableFormat as Format>::USER_END) {
            return end;
        }
        // Cannot wrap: both are below `USER_END`.
        let (old, new) = (
            end.next_multiple_of(FRAME_SIZE),
            requested.next_multiple_of(FRAME_SIZE),
        );
        if new > old {
            let added = self
                .regions
                .lock()
                .insert(old, new, Some(Attributes::USER_READ_WRITE));
            if added.is_err() {
                return end;
            }
        } else if new < old {
            self.unmap_range(new, old)
                .expect("the break's pages are page-aligned and in user space");
        }
        *program_break = Some(ProgramBreak {
            start,
            end: requested,
        });
        requested
    }

    /// Adds a region of `length` bytes, page-aligned, with `attributes` (`None`: the program
    /// may not touch it) where `placement` says (see [`Placement`]), and returns its start.
    /// Its pages are mapped, zeroed, as the program first touches them.
    pub fn map_region(
        &self,
        placement: Placement,
        length: u64,
        attributes: Option<Attributes>,
    ) -> Result<u64, RegionError> {
        let user_end = <P::PageTableFormat as Format>::USER_END;
        if length == 0 || !length.is_multiple_of(FRAME_SIZE) {
            return Err(RegionError::Invalid);
        }
        let within = |start: u64, floor: u64, top: u64| {
            start >= floor && start.checked_add(length).is_some_and(|end| end <= top)
        };
        let mut regions = self.regions.lock();
        let start = match placement {
            Placement::Replace(start) | Placement::Exactly(start) => {
                if !within(start, 0, user_end) {
                    return Err(RegionError::Invalid);
                }
                if let Placement::Replace(_) = placement {
                    self.unmap_locked(&mut regions, start, start + length)?;
                }
                start
            }
            Placement::Anywhere { hint, floor, top } => {
                let top = top.min(user_end);
                match hint {
                    Some(hint)
                        if hint.is_multiple_of(FRAME_SIZE)
                            && within(hint, floor, top)
                            && regions.is_free(hint, hint + length) =>
                    {
                        hint
                    }
                    _ => regions
                        .free_below(top, floor, length)
                        .ok_or(RegionError::NoRoom)?,
                }
            }
        };
        regions.insert(start, start + length, attributes)?;
        Ok(start)
    }

    /// Takes user addresses `start..end`, page-aligned, out of the process's regions, and
    /// unmaps the pages there, freeing their frames (Linux's `munmap`). Parts of the range in
    /// no region are fine.
    pub fn unmap_range(&self, start: u64, end: u64) -> Result<(), RegionError> {
        let mut regions = self.regions.lock();
        self.unmap_locked(&mut regions, start, end)
    }

    /// [`Self::unmap_range`], with the regions' lock held (`regions`).
    fn unmap_locked(&self, regions: &mut Regions, start: u64, end: u64) -> Result<(), RegionError> {
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(RegionError::Invalid);
        }
        regions.remove(start, end, |_| ())?;
        // Every page, not only those in the regions taken out: the HAL's test program has
        // none (roadmap).
        self.space
            .change_pages(
                start,
                end - start,
                |_, _| PageChange::Unmap,
                |frame| {
                    // SAFETY: the address space owns its frames (`Process::new`), and this one's
                    // entry no longer refers to it, so nothing reaches it any more but CPUs'
                    // cached translations, which no program runs with before they are dropped
                    // (`AddressSpace::change_pages`).
                    unsafe { free_owned_frame(frame) }
                },
            )
            .map_err(|_| RegionError::Invalid)
    }

    /// Gives user addresses `start..end`, page-aligned, `attributes` (`None`: the program may
    /// not touch them; Linux's `mprotect`), the regions there and the pages mapped in them. A
    /// page the program may not touch keeps its frame and contents ([`PageChange::Inaccessible`]);
    /// one that becomes executable is made visible to instruction fetch. If part of the range
    /// is in no region, what lies before that part changes and the error is
    /// [`RegionError::NotCovered`], as on Linux.
    pub fn protect_range(
        &self,
        start: u64,
        end: u64,
        attributes: Option<Attributes>,
    ) -> Result<(), RegionError> {
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(RegionError::Invalid);
        }
        let mut regions = self.regions.lock();
        let mut changed_end = start;
        let result = regions.protect(start, end, attributes, |part| changed_end = part.end);
        if changed_end > start {
            let frame_offset = self.frame_offset;
            self.space
                .change_pages(
                    start,
                    changed_end - start,
                    |frame, old| match attributes {
                        None => PageChange::Inaccessible,
                        Some(new) => {
                            if new.executable && !old.is_some_and(|old| old.executable) {
                                // The program may have written it as data.
                                let page = (frame as usize).wrapping_add(frame_offset);
                                P::make_instructions_visible(page, FRAME_SIZE as usize);
                            }
                            PageChange::Map(new)
                        }
                    },
                    |_| unreachable!("nothing is unmapped"),
                )
                .map_err(|_| RegionError::Invalid)?;
        }
        result
    }

    /// Maps a frame of its own at user address `virt` with `attributes`, holding `contents`
    /// (at most a page) and zero after them; an executable page is made visible to
    /// instruction fetch. Allowed while the process is shared: the caller holds its regions'
    /// lock (or `&mut`), so nothing else maps the page meanwhile. Returns the frame; if the
    /// page is mapped already, or kept ([`MapError::AlreadyMapped`]), the frame goes back.
    fn map_frame(
        &self,
        virt: u64,
        attributes: Attributes,
        contents: &[u8],
    ) -> Result<u64, MapError> {
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
            frames.allocate().ok_or(MapError::OutOfMemory)?
        };
        let page = (frame as usize).wrapping_add(self.frame_offset) as *mut u8;
        // SAFETY: the frame was just allocated, so this code owns it, and the allocator reaches
        // it through its offset, the process's `frame_offset`. `contents` fit in the frame (the
        // callers' duty) and are not in it, so the two do not overlap.
        unsafe { core::ptr::copy_nonoverlapping(contents.as_ptr(), page, contents.len()) };
        // SAFETY: as above: the rest of the frame, after the contents.
        unsafe {
            core::ptr::write_bytes(
                page.wrapping_add(contents.len()),
                0,
                FRAME_SIZE as usize - contents.len(),
            )
        };
        if attributes.executable {
            // The whole page: its zeroed rest too, or instruction fetch past the contents could
            // see the frame's old data.
            P::make_instructions_visible(page as usize, FRAME_SIZE as usize);
        }
        match self.space.map(virt, frame, FRAME_SIZE, attributes) {
            Ok(()) => Ok(frame),
            Err(error) => {
                free_frame(frame);
                Err(error)
            }
        }
    }

    /// Resolves `fault`, a page fault the process's program took at user address `address`
    /// ([`Platform::page_fault`] classifies it): if the address is in a region that allows the
    /// access, maps the page if it is not mapped yet (demand paging), and drops this CPU's
    /// cached translation of it, so the program can run the access again. A fault that may be
    /// stale on a page already mapped for the access (a RISC-V hart may cache an invalid
    /// entry) is cured by that drop alone; a permission fault on one is not retried, as the
    /// entry the CPU used raised it. Call it from a thread of the process, on the CPU the
    /// program faulted on.
    pub fn resolve_page_fault(
        &self,
        address: u64,
        fault: PageFault,
    ) -> Result<(), UnresolvedFault> {
        let page = address & !(FRAME_SIZE - 1);
        let regions = self.regions.lock();
        self.populate(&regions, page, fault.access, fault.may_be_stale)?;
        drop(regions);
        P::drop_user_translations(page, FRAME_SIZE);
        Ok(())
    }

    /// Makes user page `page` ready for `access`, with the regions' lock held (`regions`):
    /// maps a zeroed frame there if its region allows the access and it is not mapped yet.
    /// A page already mapped for the access is ready only if `mapped_is_ready` (a fault that
    /// may be stale, or a kernel copy). Returns the frame and the mapping's attributes.
    ///
    /// When `mapped_is_ready`, the page is most likely not mapped (a first touch, or a kernel
    /// copy that found it so), so it is mapped first and looked up only if that finds it
    /// mapped: one walk of the tables, not two (D53). The frame zeroed for a page that turns
    /// out to be mapped, a stale fault's, goes back.
    fn populate(
        &self,
        regions: &Regions,
        page: u64,
        access: Access,
        mapped_is_ready: bool,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        let region = regions.find(page).ok_or(UnresolvedFault::NotAllowed)?;
        let attributes = region
            .attributes
            .filter(|&attributes| access.allowed_by(attributes))
            .ok_or(UnresolvedFault::NotAllowed)?;
        if mapped_is_ready {
            match self.map_frame(page, attributes, &[]) {
                Ok(frame) => return Ok((frame, attributes)),
                Err(MapError::AlreadyMapped) => {}
                Err(MapError::OutOfMemory) => return Err(UnresolvedFault::OutOfMemory),
                Err(_) => return Err(UnresolvedFault::NotAllowed),
            }
        }
        match self.space.translate(page) {
            Some((frame, mapped)) if mapped_is_ready && access.allowed_by(mapped) => {
                Ok((frame, mapped))
            }
            Some(_) => Err(UnresolvedFault::NotAllowed),
            None => match self.map_frame(page, attributes, &[]) {
                Ok(frame) => Ok((frame, attributes)),
                Err(MapError::OutOfMemory) => Err(UnresolvedFault::OutOfMemory),
                Err(_) => Err(UnresolvedFault::NotAllowed),
            },
        }
    }

    /// The region that holds user address `address`, if any.
    pub fn region(&self, address: u64) -> Option<Region> {
        self.regions.lock().find(address).copied()
    }

    /// The process's address space.
    pub fn address_space(&self) -> &AddressSpace<P> {
        &self.space
    }

    /// Runs `active` with the process's address space active on this CPU, as
    /// [`AddressSpace::with_active`]: [`Platform::run_user`] runs the program there, outside a
    /// thread of the process (self-tests, benchmarks).
    pub fn with_active<R>(&self, active: impl FnOnce() -> R) -> R {
        self.address_space().with_active(active)
    }

    /// Loads the process's address space on this CPU unless it is already the one loaded
    /// there: a load and a compare when it is ([`AddressSpace::load_lazily`]). Call it with
    /// interrupts masked, as the kernel runs.
    #[inline]
    pub fn load_here(self: &Arc<Self>) {
        // SAFETY: the process owns its address space, and while it is shared (the `Arc`) its
        // mappings are removed or changed only through `AddressSpace::change_pages` (the
        // memory calls): `&mut` access needs the process unshared.
        unsafe { self.address_space().load_lazily(self) };
    }

    /// Runs `access` with the kernel allowed to read and write the process's pages, at their
    /// user addresses: loads its address space here first ([`Self::load_here`]), then opens
    /// the window ([`Platform::with_user_access`]). `access` must not block or switch threads,
    /// and reaches only pages already mapped: an untouched page of a region faults in the
    /// kernel, which nothing resolves (system calls copy with [`Self::read_user_memory`] and
    /// [`Self::write_user_memory`], which map it).
    pub fn with_user_memory<R>(self: &Arc<Self>, access: impl FnOnce() -> R) -> R {
        self.load_here();
        P::with_user_access(access)
    }

    /// Copies the process's memory at user address `address` into `buffer`, as a system call
    /// reads a buffer the program passed. The address is untrusted: every page must be a user
    /// page of normal memory, or nothing more is copied and the error says so (part of `buffer`
    /// may have been written by then). See [`Self::copy_user_pages`] for how.
    pub fn read_user_memory(
        &self,
        address: u64,
        buffer: &mut [u8],
    ) -> Result<(), InvalidUserAddress> {
        let destination = buffer.as_mut_ptr();
        self.copy_user_pages(address, buffer.len(), false, |kernel, done, length| {
            // SAFETY: `kernel` reaches `length` bytes of one of the process's frames through
            // the direct map (`copy_user_pages`' promise), and `done + length` is within
            // `buffer`, a kernel buffer, which no frame of the process is.
            unsafe {
                core::ptr::copy_nonoverlapping(kernel, destination.wrapping_add(done), length)
            };
        })
    }

    /// Copies `bytes` into the process's memory at user address `address`, as a system call
    /// fills a buffer the program passed. As [`Self::read_user_memory`], and every page must
    /// also be writable by the program.
    pub fn write_user_memory(&self, address: u64, bytes: &[u8]) -> Result<(), InvalidUserAddress> {
        let source = bytes.as_ptr();
        self.copy_user_pages(address, bytes.len(), true, |kernel, done, length| {
            // SAFETY: as in `read_user_memory`, the other way round.
            unsafe { core::ptr::copy_nonoverlapping(source.wrapping_add(done), kernel, length) };
        })
    }

    /// Walks `length` bytes of user memory from `address` a page at a time and hands each
    /// piece to `copy` as (its address in the kernel's direct map, bytes done so far, its
    /// length), after checking that the page is a user page of normal memory (and writable, if
    /// `write`). A page in a region the program has not touched yet is mapped first, zeroed, as
    /// the program's own access would (D52). No user-access window is opened and the address
    /// space need not be loaded on this CPU: the copy goes through the direct map (the
    /// process's `frame_offset`), so it can never fault (D50, decided by research). That is
    /// sound because the process owns every frame its pages map and the regions' lock, held
    /// throughout, keeps any from being unmapped or freed meanwhile. Copies move to user
    /// addresses with fault fixups when another thread of the process can write the same page
    /// meanwhile (`clone`): a plain copy racing it is undefined in Rust.
    fn copy_user_pages(
        &self,
        address: u64,
        length: usize,
        write: bool,
        mut copy: impl FnMut(*mut u8, usize, usize),
    ) -> Result<(), InvalidUserAddress> {
        if length == 0 {
            return Ok(());
        }
        let end = address
            .checked_add(length as u64)
            .ok_or(InvalidUserAddress)?;
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(InvalidUserAddress);
        }
        let access = if write { Access::Write } else { Access::Read };
        let regions = self.regions.lock();
        let mut done = 0;
        while done < length {
            let virt = address + done as u64;
            let page = virt & !(FRAME_SIZE - 1);
            let (frame, attributes) = match self.space.translate(page) {
                Some(mapped) => mapped,
                None => self
                    .populate(&regions, page, access, true)
                    .map_err(|_| InvalidUserAddress)?,
            };
            let phys = frame + virt % FRAME_SIZE;
            if !attributes.user
                || attributes.memory_type != MemoryType::Normal
                || (write && !attributes.writable)
            {
                return Err(InvalidUserAddress);
            }
            let in_page = (FRAME_SIZE - virt % FRAME_SIZE).min((length - done) as u64) as usize;
            copy(
                (phys as usize).wrapping_add(self.frame_offset) as *mut u8,
                done,
                in_page,
            );
            done += in_page;
        }
        Ok(())
    }

    /// Records how the process ended, unless it already has: the first to end it wins (say,
    /// two threads calling `exit_group`).
    pub fn exit(&self, how: ProcessExit) {
        self.exit.lock().get_or_insert(how);
    }

    /// How the process ended, or `None` while it has not.
    pub fn exit_status(&self) -> Option<ProcessExit> {
        *self.exit.lock()
    }
}

impl<P: Platform> core::fmt::Debug for Process<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Process")
            .field("regions", &*self.regions.lock())
            .finish_non_exhaustive()
    }
}

/// Gives a frame the process's address space owned and no longer maps back to the kernel's
/// frame allocator.
///
/// # Safety
/// The frame must be one the address space owned ([`AddressSpace::new_owning_frames`]) and
/// whose entry is gone, so nothing frees it again.
unsafe fn free_owned_frame(frame: u64) {
    let mut frames = memory::FRAMES.lock();
    let frames = frames
        .as_mut()
        .expect("the frame came from the frame allocator");
    // SAFETY: an owned frame came from this allocator (`new_owning_frames`' contract), and the
    // caller guarantees nothing maps or frees it any more.
    unsafe { frames.free(frame) };
}

/// Gives a frame `map_frame` allocated and could not map back to the kernel's frame allocator.
fn free_frame(frame: u64) {
    let mut frames = memory::FRAMES.lock();
    let frames = frames
        .as_mut()
        .expect("the frame came from the frame allocator");
    // SAFETY: the frame came from this allocator in `Process::map_frame`, which could not map
    // it, so nothing maps it and the address space does not own it.
    unsafe { frames.free(frame) };
}

/// Starts a thread of `process` that runs its program from `context` in user mode and hands
/// every exit, with the program's registers, to `handler`, until it returns
/// [`ControlFlow::Break`]: then the thread ends. On a system call the handler sets the result
/// ([`UserContext::set_syscall_result`]); on an interrupt, already handled, it usually goes on;
/// on an exception it decides the program's fate. It runs with interrupts masked, as the
/// kernel does, and may block. Placed as [`thread::spawn`] places threads.
pub fn spawn<P, H>(
    process: Arc<Process<P>>,
    context: UserContext,
    handler: H,
) -> Result<JoinHandle, SpawnError>
where
    P: Platform,
    H: FnMut(&Arc<Process<P>>, &mut UserContext, UserExit) -> ControlFlow<()> + Send + 'static,
{
    thread::spawn_user::<P, _>(move || run::<P, H>(process, context, handler))
}

/// A user thread's loop (see [`spawn`]).
fn run<P: Platform, H>(process: Arc<Process<P>>, mut context: UserContext, mut handler: H)
where
    H: FnMut(&Arc<Process<P>>, &mut UserContext, UserExit) -> ControlFlow<()>,
{
    loop {
        // First, as it may switch threads: nothing may between loading and running.
        thread::preemption_point_before_user_mode::<P>();
        process.load_here();
        // SAFETY: interrupts are masked, as the kernel runs. The process's address space is
        // loaded on this CPU, not stale, and shares the kernel's half (`Process::new`'s
        // contract); the `Arc` keeps it allocated, and its mappings change only in the handler
        // below, on this thread (`AddressSpace::change_pages`). Nothing switches this CPU's
        // threads or address space before `run_user` returns.
        let exit = unsafe { P::run_user(&mut context) };
        if handler(&process, &mut context, exit).is_break() {
            break;
        }
    }
    let here = cpu::current::<P>();
    let root = process.address_space().root();
    drop(process);
    // If only CPUs still hold the process (this was its last thread), they let it go: this one
    // now, the others, woken, when they idle.
    address_space::release_ended::<P>(here, root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{TestPlatform as P, active_root, dropped_translations, set_active_root};
    use crate::thread::host::frames_for_test;
    use core::sync::atomic::{AtomicU64, Ordering};
    use eggos_hal::{Exception, ExceptionKind};

    /// Kernel tables, active (frozen) as the kernel's are.
    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = memory::FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    fn free() -> u64 {
        memory::FRAMES.lock().as_ref().unwrap().free_frames()
    }

    /// The first root of `process`'s tables, as the test platform records it.
    fn root(process: &Process<P>) -> u64 {
        process.address_space().with_active(active_root)
    }

    #[test]
    fn the_program_is_mapped_as_user_code_and_everything_is_freed() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let process = unsafe { Process::<P>::with_test_program(&kernel) }.unwrap();
            let space = process.address_space();
            let (phys, attributes) = space.translate(TEST_PROGRAM_ADDRESS).unwrap();
            assert_eq!(attributes, Attributes::USER_TEXT);
            let code = P::user_test_program().code;
            let offset = memory::FRAMES
                .lock()
                .as_ref()
                .unwrap()
                .physical_memory_offset();
            // SAFETY: the program's frame, through the test allocator's offset.
            let page = unsafe {
                core::slice::from_raw_parts(
                    (phys as usize).wrapping_add(offset) as *const u8,
                    FRAME_SIZE as usize,
                )
            };
            assert_eq!(&page[..code.len()], code);
            assert!(page[code.len()..].iter().all(|&byte| byte == 0));
            let context = Process::<P>::test_program_context(|program| program.spin);
            assert_eq!(
                context.pc(),
                TEST_PROGRAM_ADDRESS + P::user_test_program().spin as u64
            );
            assert!(free() < before);
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn user_memory_is_copied_only_where_the_program_may_reach() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        const DATA: u64 = 0x50_0000;
        process
            .map_new_page(DATA, Attributes::USER_READ_WRITE, b"0123456789")
            .unwrap();
        process
            .map_new_page(DATA + FRAME_SIZE, Attributes::USER_READ_ONLY, b"abc")
            .unwrap();

        // Across the two pages, both ways where the program may write.
        let mut read = [0u8; 6];
        assert_eq!(
            process.write_user_memory(DATA + FRAME_SIZE - 3, b"xyz"),
            Ok(())
        );
        process
            .read_user_memory(DATA + FRAME_SIZE - 3, &mut read)
            .unwrap();
        assert_eq!(&read, b"xyzabc");
        process.read_user_memory(DATA + 2, &mut read).unwrap();
        assert_eq!(&read, b"234567");

        // Read-only, unmapped, kernel, past user space, wrapping: refused. (The kernel half is
        // refused by the user-space bound before the walk; the user and memory-type checks
        // stand behind it.)
        assert_eq!(
            process.write_user_memory(DATA + FRAME_SIZE - 1, b"no"),
            Err(InvalidUserAddress)
        );
        let end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        for address in [DATA + 2 * FRAME_SIZE - 1, 0, end - 1, u64::MAX - 1] {
            assert_eq!(
                process.read_user_memory(address, &mut read),
                Err(InvalidUserAddress),
                "{address:#x}"
            );
        }
        // Nothing to copy needs no check.
        assert_eq!(process.read_user_memory(0, &mut []), Ok(()));
    }

    const DATA: u64 = 0x50_0000;

    /// A page fault of `kind` at `address`, a write if `write`.
    fn fault(kind: ExceptionKind, address: Option<u64>, write: bool) -> Exception {
        Exception {
            kind,
            name: "test page fault",
            code: if write { crate::tests::FAULT_WRITE } else { 0 },
            pc: TEST_PROGRAM_ADDRESS,
            address,
        }
    }

    /// What a thread's handler does with `exception`: resolves it if it is a page fault.
    fn resolve(process: &Process<P>, exception: &Exception) -> Result<(), UnresolvedFault> {
        match (
            P::page_fault(exception.kind, exception.code),
            exception.address,
        ) {
            (Some(fault), Some(address)) => process.resolve_page_fault(address, fault),
            _ => Err(UnresolvedFault::NotAllowed),
        }
    }

    /// The page's bytes, through the test allocator's offset.
    fn page_bytes(process: &Process<P>, virt: u64) -> Vec<u8> {
        let (phys, _) = process.address_space().translate(virt).unwrap();
        let mut bytes = alloc::vec![0; FRAME_SIZE as usize];
        process.read_user_memory(virt, &mut bytes).unwrap();
        assert_eq!(phys % FRAME_SIZE, virt % FRAME_SIZE);
        bytes
    }

    #[test]
    fn a_fault_in_a_region_maps_a_zeroed_page_once_and_drops_the_stale_translation() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: as above.
            let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            process
                .add_region(DATA, DATA + 2 * FRAME_SIZE, Attributes::USER_READ_WRITE)
                .unwrap();
            let process = Arc::new(process);
            dropped_translations();
            let write = fault(ExceptionKind::DataFault, Some(DATA + 0x18), true);
            assert_eq!(resolve(&process, &write), Ok(()));
            assert_eq!(dropped_translations(), [(DATA, FRAME_SIZE)]);
            let (_, attributes) = process.address_space().translate(DATA).unwrap();
            assert_eq!(attributes, Attributes::USER_READ_WRITE);
            assert!(page_bytes(&process, DATA).iter().all(|&byte| byte == 0));
            assert_eq!(process.address_space().translate(DATA + FRAME_SIZE), None);
            // Again, as a hart with a stale invalid entry would: no new frame, the
            // translation dropped again.
            let used = free();
            assert_eq!(resolve(&process, &write), Ok(()));
            assert_eq!(free(), used);
            assert_eq!(dropped_translations(), [(DATA, FRAME_SIZE)]);
            // A read maps the second page.
            let read = fault(
                ExceptionKind::DataFault,
                Some(DATA + 2 * FRAME_SIZE - 1),
                false,
            );
            assert_eq!(resolve(&process, &read), Ok(()));
            assert!(
                process
                    .address_space()
                    .translate(DATA + FRAME_SIZE)
                    .is_some()
            );
        }
        assert_eq!(
            free(),
            before,
            "the faulted-in frames are freed with the process"
        );
    }

    #[test]
    fn faults_the_regions_do_not_allow_are_not_resolved_and_map_nothing() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        let text = DATA + 4 * FRAME_SIZE;
        process
            .add_region(DATA, DATA + FRAME_SIZE, Attributes::USER_READ_ONLY)
            .unwrap();
        process
            .add_region(
                DATA + FRAME_SIZE,
                DATA + 2 * FRAME_SIZE,
                Attributes::USER_READ_WRITE,
            )
            .unwrap();
        process
            .add_region(text, text + FRAME_SIZE, Attributes::USER_TEXT)
            .unwrap();
        let used = free();
        for exception in [
            // Outside every region, below, between and above them.
            fault(ExceptionKind::DataFault, Some(DATA - 1), false),
            fault(ExceptionKind::DataFault, Some(DATA + 3 * FRAME_SIZE), false),
            fault(ExceptionKind::DataFault, Some(text + FRAME_SIZE), true),
            // A write to read-only memory, code fetched from data, a write to code.
            fault(ExceptionKind::DataFault, Some(DATA), true),
            fault(
                ExceptionKind::InstructionFault,
                Some(DATA + FRAME_SIZE),
                false,
            ),
            fault(ExceptionKind::DataFault, Some(text), true),
            // No address, and not a page fault.
            fault(ExceptionKind::DataFault, None, false),
            fault(ExceptionKind::Alignment, Some(DATA + FRAME_SIZE), true),
        ] {
            assert_eq!(
                resolve(&process, &exception),
                Err(UnresolvedFault::NotAllowed),
                "{exception:?}"
            );
        }
        assert_eq!(free(), used, "nothing was mapped");
        // A read of read-only memory and a fetch from code are allowed.
        let fetch = fault(ExceptionKind::InstructionFault, Some(text + 8), false);
        assert_eq!(resolve(&process, &fetch), Ok(()));
        assert_eq!(
            process.address_space().translate(text).unwrap().1,
            Attributes::USER_TEXT
        );
        let read = fault(ExceptionKind::DataFault, Some(DATA + 8), false);
        assert_eq!(resolve(&process, &read), Ok(()));
        // On that mapped, readable page: a read that is not a page fault (say, an alignment
        // fault reported as a data abort), and a permission fault the entry raised, are never
        // retried; retrying would fault again for ever.
        for code in [
            crate::tests::NOT_A_PAGE_FAULT,
            crate::tests::FAULT_PERMISSION,
        ] {
            let mut again = read;
            again.code = code;
            assert_eq!(
                resolve(&process, &again),
                Err(UnresolvedFault::NotAllowed),
                "{code:#x}"
            );
        }
        // Mapped read-only, a write still faults and is not resolved.
        let write = fault(ExceptionKind::DataFault, Some(DATA + 8), true);
        assert_eq!(resolve(&process, &write), Err(UnresolvedFault::NotAllowed));
        // Regions outside user space or overlapping are refused.
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(
            process.add_region(
                user_end - FRAME_SIZE,
                user_end + FRAME_SIZE,
                Attributes::USER_READ_WRITE
            ),
            Err(RegionError::Invalid)
        );
        assert_eq!(
            process.add_region(DATA, DATA + FRAME_SIZE, Attributes::USER_READ_WRITE),
            Err(RegionError::Overlaps)
        );
    }

    #[test]
    fn kernel_copies_map_untouched_pages_their_regions_allow() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        let read_only = DATA + 2 * FRAME_SIZE;
        process
            .add_region(DATA, read_only, Attributes::USER_READ_WRITE)
            .unwrap();
        process
            .add_region(
                read_only,
                read_only + FRAME_SIZE,
                Attributes::USER_READ_ONLY,
            )
            .unwrap();
        // A write across the two writable pages maps both.
        assert_eq!(
            process.write_user_memory(DATA + FRAME_SIZE - 2, b"abcd"),
            Ok(())
        );
        let mut read = [0xff; 6];
        process
            .read_user_memory(DATA + FRAME_SIZE - 3, &mut read)
            .unwrap();
        assert_eq!(&read, b"\0abcd\0");
        // An untouched read-only page reads as zero; it is never written.
        process.read_user_memory(read_only, &mut read).unwrap();
        assert_eq!(read, [0; 6]);
        assert_eq!(
            process.write_user_memory(read_only + FRAME_SIZE - 1, b"x"),
            Err(InvalidUserAddress)
        );
        // Past the regions: refused, and nothing mapped.
        let used = free();
        assert_eq!(
            process.read_user_memory(read_only + FRAME_SIZE, &mut read),
            Err(InvalidUserAddress)
        );
        assert_eq!(
            process.write_user_memory(read_only + 2 * FRAME_SIZE - 1, b"xy"),
            Err(InvalidUserAddress)
        );
        assert_eq!(free(), used);
    }

    #[test]
    fn a_page_larger_than_a_page_is_refused_and_nothing_leaks() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: as above.
            let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            let too_big = [0u8; FRAME_SIZE as usize + 1];
            assert!(
                process
                    .map_new_page(0x50_0000, Attributes::USER_READ_WRITE, &too_big)
                    .is_err()
            );
            // Not a user address: the frame is given back.
            assert!(
                process
                    .map_new_page(!0xfff, Attributes::USER_READ_WRITE, &[1])
                    .is_err()
            );
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn a_process_is_loaded_lazily_and_kept_until_another_needs_the_cpu() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let here = cpu::current::<P>();
        let before = free();
        // SAFETY: as above.
        let first = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        // SAFETY: as above.
        let second = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        let (first_root, second_root) = (root(&first), root(&second));
        assert_ne!(first_root, second_root);

        first.load_here();
        assert_eq!(active_root(), first_root);
        assert!(first.address_space().is_loaded_here());
        // Already loaded: no switch (the record would show one to the same root, so move it
        // away to see).
        set_active_root(0);
        first.load_here();
        assert_eq!(
            active_root(),
            0,
            "a loaded address space was switched to again"
        );
        set_active_root(first_root);

        // The CPU keeps the first alive after its last owner is gone...
        let first_tables = free();
        drop(first);
        assert_eq!(free(), first_tables);
        // ...until another process needs the CPU, which frees it.
        second.load_here();
        assert_eq!(active_root(), second_root);
        assert!(free() > first_tables);

        // `with_active` puts the lazily loaded one away first and leaves the kernel's tables.
        second.with_active(|| assert_eq!(active_root(), second_root));
        assert_eq!(here.active_address_space(), cpu::NO_ADDRESS_SPACE);
        assert!(here.lazily_loaded.lock().is_none());

        // Idle release: only once the CPU holds the last reference.
        second.load_here();
        address_space::release_unused::<P>(here);
        assert!(second.address_space().is_loaded_here());
        drop(second);
        address_space::release_unused::<P>(here);
        assert_eq!(here.active_address_space(), cpu::NO_ADDRESS_SPACE);
        assert_eq!(free(), before);
    }

    #[test]
    fn a_user_thread_hands_every_exit_to_its_handler_and_frees_its_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let here = cpu::current::<P>();
        // The idle thread's stack, taken once, is not the process's.
        thread::start::<P>().unwrap();
        let before = free();
        static SYSCALLS: AtomicU64 = AtomicU64::new(0);
        SYSCALLS.store(0, Ordering::Relaxed);
        // SAFETY: as above.
        let process = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        let context = Process::<P>::test_program_context(|program| program.null_syscalls);
        let thread = spawn::<P, _>(process, context, |process, context, exit| {
            assert_eq!(exit, UserExit::Syscall);
            assert_eq!(context.syscall_number(), TestProgram::SYSCALL_NULL);
            assert!(process.address_space().is_loaded_here());
            context.set_syscall_result(0);
            if SYSCALLS.fetch_add(1, Ordering::Relaxed) + 1 == 3 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
        thread.join::<P>();
        assert_eq!(SYSCALLS.load(Ordering::Relaxed), 3);
        // The thread held the last reference: its CPU let the process go.
        assert_eq!(here.active_address_space(), cpu::NO_ADDRESS_SPACE);
        assert_eq!(free(), before);
    }
}
