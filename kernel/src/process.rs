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
use crate::linux::credentials::Credentials;
use crate::linux::limits::ResourceLimits;
use crate::linux::signals::Signals;
use crate::memory::{self, FramesToFree};
use crate::paging::{MapError, PageChange, PageTables};
use crate::regions::{Access, Region, RegionError, Regions, is_page_range};
use crate::sync::SpinLock;
use crate::thread::{self, FpStart, JoinHandle, SpawnError};
use alloc::sync::Arc;
use core::num::NonZeroU64;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType, PageFault};
use eggos_hal::user::{TestProgram, UserContext, UserExit};
use eggos_hal::{ExceptionKind, Platform};

/// [`Process::first_thread_status`] before the first thread has ended: no status, which is 8 bits.
const NO_STATUS: u32 = u32::MAX;

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

/// How much of a range of user addresses the regions hold ([`Process::coverage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Nothing,
    Part,
    Whole,
}

/// Why [`Process::populate_range`] did not make every page of its range ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulateError {
    /// A region in the range does not allow the access.
    NotAllowed,
    /// There was no frame for a page.
    OutOfMemory,
    /// Part of the range is in no region; the rest is ready.
    NotCovered,
}

/// Where [`Process::remap_region`] may put a region it resizes or moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remap {
    /// Where it is: it grows only into free addresses right after it, up to `top`.
    InPlace { top: u64 },
    /// Where it is if it can grow there up to `top`, otherwise as high as it fits within
    /// `floor..top`.
    InPlaceOrAnywhere { floor: u64, top: u64 },
    /// At `hint`, if it is given and the range there is free and within `floor..top`;
    /// otherwise as high as it fits within `floor..top`. Never where it is.
    Anywhere {
        hint: Option<u64>,
        floor: u64,
        top: u64,
    },
    /// At this address, page-aligned, replacing whatever held part of the new range before,
    /// pages and all.
    Replace(u64),
}

/// The program break (Linux's `brk`): where the heap a program grows with `brk` begins, and
/// where it ends now. The heap is a region of its own from `start`, page-aligned, to `end`
/// rounded up to a page, joined with the data segment below it as any touching region with
/// the same attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProgramBreak {
    start: NonZeroU64,
    end: u64,
}

/// A process's credentials and resource limits, under one lock: `setrlimit` checks the one
/// and changes the other. Root's credentials and the default limits to start with.
#[derive(Debug, Default)]
pub struct CredentialsAndLimits {
    pub credentials: Credentials,
    pub limits: ResourceLimits,
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
    /// Set once [`Self::exit`] has run: its threads run no more of its program.
    ended: AtomicBool,
    /// Its threads that run its program and have not exited ([`Self::add_thread`]).
    threads: AtomicU32,
    /// The status its first thread ended with on its own, [`NO_STATUS`] until it has
    /// ([`Self::record_first_thread_status`]).
    first_thread_status: AtomicU32,
    /// Its id, unique while the kernel runs ([`Self::id`]).
    id: ProcessId,
    /// Its credentials and resource limits ([`Self::credentials_and_limits`]).
    credentials_and_limits: SpinLock<CredentialsAndLimits>,
    /// Its signal state, which its threads share ([`Self::signals`]).
    signals: SpinLock<Signals>,
}

/// A process's id: from 1 up, never reused, and at most `i32::MAX`, as a Linux `pid_t` is
/// positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(u32);

impl ProcessId {
    /// The value Linux programs see.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The next unused id, or `None` once every id up to `i32::MAX` has been given out
    /// (reusing ended processes' ids waits for a process table, with `wait4`).
    fn allocate() -> Option<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
            (id <= i32::MAX as u32).then_some(id + 1)
        })
        .ok()
        .map(Self)
    }
}

/// An id for a thread that is not a process's first (`clone`), from the processes' ids: Linux
/// gives threads and processes ids from one space, the first thread's being its process's.
/// `None` once every id has been given out.
pub fn allocate_thread_id() -> Option<u32> {
    ProcessId::allocate().map(ProcessId::get)
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
        let id = ProcessId::allocate().ok_or("out of process ids")?;
        Ok(Self {
            space,
            regions: SpinLock::new(Regions::new()),
            program_break: SpinLock::new(None),
            frame_offset,
            exit: SpinLock::new(None),
            ended: AtomicBool::new(false),
            threads: AtomicU32::new(0),
            first_thread_status: AtomicU32::new(NO_STATUS),
            id,
            credentials_and_limits: SpinLock::new(CredentialsAndLimits::default()),
            signals: SpinLock::new(Signals::new()),
        })
    }

    /// Its id, given when it was created.
    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// Its credentials (D75), starting as root, and its resource limits (`getrlimit`,
    /// `setrlimit`, `prlimit64`; D59), starting from [`crate::linux::limits::DEFAULTS`]. Never
    /// held while user memory is copied.
    pub fn credentials_and_limits(&self) -> &SpinLock<CredentialsAndLimits> {
        &self.credentials_and_limits
    }

    /// Its credentials and resource limits, while the process is built.
    pub fn credentials_and_limits_mut(&mut self) -> &mut CredentialsAndLimits {
        self.credentials_and_limits.get_mut()
    }

    /// Its signal actions (`rt_sigaction`, D60) and the `siginfo_t`s queued with its threads'
    /// pending signals; each thread's own state joins it under this lock (D91). Never held
    /// while user memory is copied.
    pub fn signals(&self) -> &SpinLock<Signals> {
        &self.signals
    }

    /// A process whose address space maps the HAL's test program
    /// ([`Platform::user_test_program`]) at [`TEST_PROGRAM_ADDRESS`], [`Attributes::USER_TEXT`],
    /// on as many pages as it takes.
    ///
    /// # Safety
    /// As [`Self::new`].
    pub unsafe fn with_test_program(
        kernel: &PageTables<P::PageTableFormat>,
    ) -> Result<Self, &'static str> {
        // SAFETY: the caller's guarantees.
        let mut process = unsafe { Self::new(kernel) }?;
        let mut virt = TEST_PROGRAM_ADDRESS;
        for page in P::user_test_program().code.chunks(FRAME_SIZE as usize) {
            process.map_new_page(virt, Attributes::USER_TEXT, page)?;
            virt += FRAME_SIZE;
        }
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
        let start = NonZeroU64::new(start).expect("the program break starts above address 0");
        *self.program_break.get_mut() = Some(ProgramBreak {
            start,
            end: start.get(),
        });
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
        if requested < start.get()
            || requested > limit.min(<P::PageTableFormat as Format>::USER_END)
        {
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
                free_range(&regions, hint, floor, top.min(user_end), length)?
            }
        };
        regions.insert(start, start + length, attributes)?;
        Ok(start)
    }

    /// Adds a region of one page with `attributes` where `placement` says, as
    /// [`Self::map_region`], and maps it at once with a frame of its own holding `contents`
    /// and zero after them, made visible to instruction fetch if executable: a page the
    /// kernel fills for the program, which need not be writable by it (a signal handler's
    /// return trampoline). Returns its start, or `None` if there was no room or no memory.
    /// While the process runs: only its own thread calls this.
    pub fn map_filled_page(
        &self,
        placement: Placement,
        attributes: Attributes,
        contents: &[u8],
    ) -> Option<u64> {
        if contents.len() > FRAME_SIZE as usize {
            return None;
        }
        let start = self
            .map_region(placement, FRAME_SIZE, Some(attributes))
            .ok()?;
        let regions = self.regions.lock();
        let mapped = self.map_frame(start, attributes, contents);
        drop(regions);
        if mapped.is_err() {
            // The region alone: nothing is mapped in it.
            let _ = self.unmap_range(start, start + FRAME_SIZE);
            return None;
        }
        P::drop_user_translations(start, FRAME_SIZE);
        Some(start)
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
        self.free_pages(start, end)
            .map_err(|_| RegionError::Invalid)
    }

    /// Unmaps every page of user addresses `start..end`, page-aligned, freeing the frames, as
    /// [`Self::unmap_range`] does with the regions there: with the regions' lock held. Every
    /// page, not only those in regions: the HAL's test program has none.
    #[inline(always)]
    fn free_pages(&self, start: u64, end: u64) -> Result<(), MapError> {
        let mut unmapped = FramesToFree::new();
        let frame_offset = self.frame_offset;
        let result = self.space.unmap_pages(start, end - start, |frame| {
            // SAFETY: the address space owns its frames (`Process::new`), so this one came from
            // the frame allocator, whose offset `frame_offset` is; its entry no longer refers to
            // it, and no CPU writes it through what it cached of the entry (`unmap_pages`), so
            // nothing writes it any more.
            unsafe { unmapped.push(frame, frame_offset) }
        });
        if !unmapped.is_empty() {
            // SAFETY: every frame was pushed with `frame_offset` (above).
            let chain = unsafe { unmapped.chain(frame_offset) };
            let mut frames = memory::FRAMES.lock();
            let frames = frames
                .as_mut()
                .expect("the frames came from the frame allocator");
            // SAFETY: the frames came from this allocator, linked through its offset (above),
            // and nothing refers to them any more: their entries are cleared and no CPU uses
            // translations of them.
            unsafe { frames.free_chain(chain) };
        }
        result
    }

    /// Unmaps the pages of user addresses `start..end`, page-aligned, that are in regions,
    /// freeing their frames but keeping the regions: their pages are mapped, zeroed, when next
    /// touched (Linux's `MADV_DONTNEED`). Touching regions are freed as one run. If part of
    /// the range is in no region, the rest is discarded all the same and the error is
    /// [`RegionError::NotCovered`].
    pub fn discard_pages(&self, start: u64, end: u64) -> Result<(), RegionError> {
        if !is_page_range(start, end) {
            return Err(RegionError::Invalid);
        }
        let regions = self.regions.lock();
        let free_run = |(run_start, run_end)| {
            self.free_pages(run_start, run_end)
                .map_err(|_| RegionError::Invalid)
        };
        let mut whole = true;
        let mut run = None;
        for region in regions.overlapping(start, end) {
            let part = (region.start.max(start), region.end.min(end));
            run = match run {
                Some((run_start, run_end)) if run_end == part.0 => Some((run_start, part.1)),
                Some(previous) => {
                    free_run(previous)?;
                    whole = false;
                    Some(part)
                }
                None => Some(part),
            };
        }
        match run {
            Some(last) => {
                free_run(last)?;
                whole &= last.0 == start && last.1 == end;
            }
            None => whole = false,
        }
        if whole {
            Ok(())
        } else {
            Err(RegionError::NotCovered)
        }
    }

    /// How much of user addresses `start..end` regions hold.
    pub fn coverage(&self, start: u64, end: u64) -> Coverage {
        let regions = self.regions.lock();
        let mut covered = start;
        let mut any = false;
        for region in regions.overlapping(start, end) {
            if region.start > covered {
                return Coverage::Part;
            }
            covered = region.end;
            any = true;
        }
        match (any, covered >= end) {
            (false, _) => Coverage::Nothing,
            (true, true) => Coverage::Whole,
            (true, false) => Coverage::Part,
        }
    }

    /// Makes the pages of user addresses `start..end`, page-aligned, ready for `access`, region
    /// by region in address order (Linux's `MADV_POPULATE_READ` and `MADV_POPULATE_WRITE`): a
    /// write maps a zeroed frame at every page not mapped yet; a read only checks, as the
    /// program reads zeros from a page not mapped yet all the same. Stops at a region that does
    /// not allow `access` ([`PopulateError::NotAllowed`]) or when there is no frame
    /// ([`PopulateError::OutOfMemory`]); a part of the range in no region is skipped and
    /// reported at the end ([`PopulateError::NotCovered`]).
    pub fn populate_range(
        &self,
        start: u64,
        end: u64,
        access: Access,
    ) -> Result<(), PopulateError> {
        debug_assert!(is_page_range(start, end));
        let regions = self.regions.lock();
        let mut covered = start;
        let mut result = Ok(());
        for region in regions.overlapping(start, end) {
            if region.start > covered {
                result = Err(PopulateError::NotCovered);
            }
            if !region
                .attributes
                .is_some_and(|attributes| access.allowed_by(attributes))
            {
                return Err(PopulateError::NotAllowed);
            }
            let (part_start, part_end) = (region.start.max(start), region.end.min(end));
            if access == Access::Write {
                for page in (part_start..part_end).step_by(FRAME_SIZE as usize) {
                    if self
                        .space
                        .translate(page)
                        .is_some_and(|(_, mapped)| access.allowed_by(mapped))
                    {
                        continue;
                    }
                    self.populate(&regions, page, access, true)
                        .map_err(|error| match error {
                            UnresolvedFault::NotAllowed => PopulateError::NotAllowed,
                            UnresolvedFault::OutOfMemory => PopulateError::OutOfMemory,
                        })?;
                }
            }
            covered = part_end;
        }
        if covered < end {
            result = Err(PopulateError::NotCovered);
        }
        result
    }

    /// Resizes or moves the part `start..start + old_length` of one region (Linux's `mremap`),
    /// both page-aligned and `old_length` not 0, to `new_length` bytes, at least `old_length`,
    /// where `remap` says, and returns its new start. A moved part takes its pages with it,
    /// frames and contents, and its attributes; the addresses it grew by are mapped, zeroed,
    /// when first touched. With `keep_source` (it must move), the old range stays a region
    /// whose pages are mapped, zeroed, when next touched. [`Remap::Replace`] unmaps the new
    /// range once the part is found. Errors: [`RegionError::NotCovered`] if the part is
    /// not all in one region; [`RegionError::NoRoom`] if it cannot be where `remap` allows;
    /// [`RegionError::OutOfMemory`] if there was no frame for a page table, with nothing moved;
    /// [`RegionError::Invalid`] for a [`Remap::Replace`] range past user space or overlapping
    /// the part.
    pub fn remap_region(
        &self,
        start: u64,
        old_length: u64,
        new_length: u64,
        remap: Remap,
        keep_source: bool,
    ) -> Result<u64, RegionError> {
        let user_end = <P::PageTableFormat as Format>::USER_END;
        let old_end = start
            .checked_add(old_length)
            .filter(|&end| end <= user_end)
            .ok_or(RegionError::NotCovered)?;
        if new_length < old_length
            || !is_page_range(start, old_end)
            || !new_length.is_multiple_of(FRAME_SIZE)
        {
            return Err(RegionError::Invalid);
        }
        if let Remap::Replace(to) = remap
            && to
                .checked_add(new_length)
                .is_none_or(|end| end > user_end || (to < old_end && start < end))
        {
            return Err(RegionError::Invalid);
        }
        let mut regions = self.regions.lock();
        let region = regions
            .find(start)
            .copied()
            .filter(|region| old_end <= region.end)
            .ok_or(RegionError::NotCovered)?;
        let growth_top = match remap {
            Remap::InPlace { top } | Remap::InPlaceOrAnywhere { top, .. } if !keep_source => {
                Some(top.min(user_end))
            }
            _ => None,
        };
        if let Some(top) = growth_top {
            if new_length == old_length {
                return Ok(start);
            }
            let new_end = start.checked_add(new_length).filter(|&end| end <= top);
            if let Some(new_end) = new_end
                && old_end == region.end
                && regions.is_free(old_end, new_end)
            {
                regions.insert(old_end, new_end, region.attributes)?;
                return Ok(start);
            }
        }
        let to = match remap {
            Remap::InPlace { .. } => return Err(RegionError::NoRoom),
            Remap::InPlaceOrAnywhere { floor, top } => {
                free_range(&regions, None, floor, top.min(user_end), new_length)?
            }
            Remap::Anywhere { hint, floor, top } => {
                free_range(&regions, hint, floor, top.min(user_end), new_length)?
            }
            Remap::Replace(to) => {
                self.unmap_locked(&mut regions, to, to + new_length)?;
                to
            }
        };
        regions.insert(to, to + new_length, region.attributes)?;
        if let Err(error) = self.space.move_pages(start, to, old_length) {
            regions.remove(to, to + new_length, |_| ())?;
            return Err(match error {
                MapError::OutOfMemory => RegionError::OutOfMemory,
                _ => RegionError::Invalid,
            });
        }
        if !keep_source {
            regions.remove(start, old_end, |_| ())?;
        }
        Ok(to)
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
            eggos_hal::memory::zero(
                page.wrapping_add(contents.len()),
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
    /// out to be mapped, a stale fault's, goes back; with no frame to try with, the page is
    /// looked up all the same, so a stale fault on a mapped page resumes the program even
    /// when memory is out.
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
                Err(MapError::OutOfMemory) => return self.mapped_or_out_of_memory(page, access),
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

    /// For [`Self::populate`], when there was no frame to map page `page` with: the page's
    /// frame and attributes if it is mapped for `access` already (a stale fault, which then
    /// resumes), else [`UnresolvedFault::OutOfMemory`]. Cold and out of line, so the fault
    /// path stays as it was.
    #[cold]
    #[inline(never)]
    fn mapped_or_out_of_memory(
        &self,
        page: u64,
        access: Access,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        self.space
            .translate(page)
            .filter(|&(_, mapped)| access.allowed_by(mapped))
            .ok_or(UnresolvedFault::OutOfMemory)
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
    /// may have been written by then). See `copy_user_pages` for how.
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

    /// Runs `access` on the process's 32-bit word at user address `address`, which must be
    /// 4-byte aligned, as an atomic other threads of the process may use at the same time
    /// (futex words, D94). As [`Self::read_user_memory`] otherwise, and the page must also be
    /// writable by the program if `write`.
    pub fn with_user_word<R>(
        &self,
        address: u64,
        write: bool,
        access: impl FnOnce(&AtomicU32) -> R,
    ) -> Result<R, InvalidUserAddress> {
        if !address.is_multiple_of(4) || address >= <P::PageTableFormat as Format>::USER_END {
            return Err(InvalidUserAddress);
        }
        let regions = self.regions.lock();
        let kernel = self.in_direct_map(&regions, address, write)?;
        // SAFETY: `kernel` reaches the word, 4-byte aligned, in one of the process's frames
        // through the direct map (`in_direct_map`' promise), mapped with the same memory type
        // as the program's mapping; the regions' lock, held until `access` returns, keeps the
        // frame the process's, and every access to the word, the program's and the kernel's,
        // is atomic or the program's own.
        let word = unsafe { AtomicU32::from_ptr(kernel.cast()) };
        Ok(access(word))
    }

    /// Where the kernel reaches user address `virt` through the direct map, with the regions'
    /// lock held (`regions`): its page must be a user page of normal memory (and writable, if
    /// `write`); a page in a region the program has not touched yet is mapped first, zeroed,
    /// as the program's own access would (D52). See `copy_user_pages`.
    #[inline]
    fn in_direct_map(
        &self,
        regions: &Regions,
        virt: u64,
        write: bool,
    ) -> Result<*mut u8, InvalidUserAddress> {
        let page = virt & !(FRAME_SIZE - 1);
        let (frame, attributes) = match self.space.translate(page) {
            Some(mapped) => mapped,
            None => {
                let access = if write { Access::Write } else { Access::Read };
                self.populate(regions, page, access, true)
                    .map_err(|_| InvalidUserAddress)?
            }
        };
        if !attributes.user
            || attributes.memory_type != MemoryType::Normal
            || (write && !attributes.writable)
        {
            return Err(InvalidUserAddress);
        }
        let phys = frame + virt % FRAME_SIZE;
        Ok((phys as usize).wrapping_add(self.frame_offset) as *mut u8)
    }

    /// Walks `length` bytes of user memory from `address` a page at a time and hands each
    /// piece to `copy` as (its address in the kernel's direct map, bytes done so far, its
    /// length), after checking that the page is a user page of normal memory (and writable, if
    /// `write`). A page in a region the program has not touched yet is mapped first, zeroed, as
    /// the program's own access would (D52). No user-access window is opened and the address
    /// space need not be loaded on this CPU: the copy goes through the direct map (the
    /// process's `frame_offset`), so it can never fault (D50, decided by research). That is
    /// sound because the process owns every frame its pages map and the regions' lock, held
    /// throughout, keeps any from being unmapped or freed meanwhile. Another thread of the
    /// process may write the same bytes on another CPU meanwhile: the bytes copied are then
    /// unspecified, as on Linux, and the kernel reads each once, into its own buffer, and
    /// reaches user memory only through raw pointers (decided by research, D94). Words other
    /// threads use as atomics go through [`Self::with_user_word`].
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
        let regions = self.regions.lock();
        let mut done = 0;
        while done < length {
            let virt = address + done as u64;
            let kernel = self.in_direct_map(&regions, virt, write)?;
            let in_page = (FRAME_SIZE - virt % FRAME_SIZE).min((length - done) as u64) as usize;
            copy(kernel, done, in_page);
            done += in_page;
        }
        Ok(())
    }

    /// Records how the process ended, unless it already has: the first to end it wins (say,
    /// two threads calling `exit_group`). Its other threads stop running its program: each
    /// other CPU is asked to look at its scheduler, which a thread of the process does before
    /// it runs the program again (see [`spawn`]), and a CPU running one in user mode is
    /// interrupted.
    pub fn exit(&self, how: ProcessExit) {
        self.exit.lock().get_or_insert(how);
        self.ended.store(true, Ordering::Relaxed);
        if self.threads() > 1 {
            interrupt_other_cpus::<P>();
        }
    }

    /// Whether the process has ended ([`Self::exit`]): its threads stop instead of running its
    /// program again.
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// For a call that passes preemption points on the process's behalf, which may take the
    /// look an ending process asked this CPU for: whether the process has ended. If it has,
    /// this CPU is asked to look again, so the calling thread stops before it returns to user
    /// mode.
    pub fn has_ended_and_will_stop(&self) -> bool {
        let ended = self.has_ended();
        if ended {
            cpu::current::<P>().request_reschedule();
        }
        ended
    }

    /// Records the status the process's first thread ended with while others go on (Linux's
    /// `exit`): the process ends with it if they all end that way too, as Linux reports its
    /// thread group leader's.
    pub fn record_first_thread_status(&self, status: u8) {
        self.first_thread_status
            .store(u32::from(status), Ordering::Release);
    }

    /// The status recorded by [`Self::record_first_thread_status`], if any.
    pub fn first_thread_status(&self) -> Option<u8> {
        u8::try_from(self.first_thread_status.load(Ordering::Acquire)).ok()
    }

    /// Its threads that run its program and have not exited.
    pub fn threads(&self) -> u32 {
        self.threads.load(Ordering::Acquire)
    }

    /// Counts a thread that will run the process's program, before it starts.
    pub fn add_thread(&self) {
        self.threads.fetch_add(1, Ordering::AcqRel);
    }

    /// Counts a thread out, as it exits or could not start: true if it was the last, which
    /// then ends the process.
    pub fn remove_thread(&self) -> bool {
        self.threads.fetch_sub(1, Ordering::AcqRel) == 1
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

/// Where `length` bytes, page-aligned, go within `floor..top`: at `hint` if it is given,
/// page-aligned and the range there is free; otherwise as high as a free range fits
/// ([`RegionError::NoRoom`] if none does), as Linux places a mapping top-down.
#[inline]
fn free_range(
    regions: &Regions,
    hint: Option<u64>,
    floor: u64,
    top: u64,
    length: u64,
) -> Result<u64, RegionError> {
    match hint {
        Some(hint)
            if hint.is_multiple_of(FRAME_SIZE)
                && hint >= floor
                && hint.checked_add(length).is_some_and(|end| end <= top)
                && regions.is_free(hint, hint + length) =>
        {
            Ok(hint)
        }
        _ => regions
            .free_below(top, floor, length)
            .ok_or(RegionError::NoRoom),
    }
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

/// A user thread's loop (see [`spawn`]). A thread that starts after its process has ended
/// (started by a thread as another ended it) never runs the program.
fn run<P: Platform, H>(process: Arc<Process<P>>, mut context: UserContext, mut handler: H)
where
    H: FnMut(&Arc<Process<P>>, &mut UserContext, UserExit) -> ControlFlow<()>,
{
    let mut ended = process.has_ended();
    while !ended {
        // First, as it may switch threads: nothing may between loading and running.
        if thread::preemption_point_before_user_mode::<P>() && process.has_ended() {
            break;
        }
        process.load_here();
        // SAFETY: interrupts are masked, as the kernel runs. The process's address space is
        // loaded on this CPU, not stale, and shares the kernel's half (`Process::new`'s
        // contract); the `Arc` keeps it allocated, and its mappings change only in the handler
        // below, on this thread (`AddressSpace::change_pages`). Nothing switches this CPU's
        // threads or address space before `run_user` returns.
        let exit = unsafe { P::run_user(&mut context) };
        ended = handler(&process, &mut context, exit).is_break();
    }
    let here = cpu::current::<P>();
    let root = process.address_space().root();
    drop(process);
    // If only CPUs still hold the process (this was its last thread), they let it go: this one
    // now, the others, woken, when they idle.
    address_space::release_ended::<P>(here, root);
}

/// Asks every other online CPU to look at its scheduler and interrupts it: a thread of an ended
/// process that runs there in user mode comes back to the kernel, and one about to return to
/// user mode, or switched in there next, looks first ([`run`]).
#[cold]
#[inline(never)]
fn interrupt_other_cpus<P: Platform>() {
    let here = cpu::current::<P>();
    for &other in cpu::CPUS.all() {
        if !core::ptr::eq(other, here) && other.is_online() {
            other.request_reschedule();
            cpu::wake::<P>(other);
        }
    }
}

/// For a thread's handler ([`spawn`]), on an exception of `kind`: whether it was the
/// program's first use of FP/SIMD, which traps as an illegal instruction until the thread has
/// registers of its own (D54). If so, the thread now has them (`thread::start_fp`) and the
/// handler resumes the program, which runs the instruction again. A second illegal
/// instruction, or one with no memory left for the registers, is the handler's. Kept out of the
/// loop in `run`, where system calls would pay for it.
#[cold]
#[inline(never)]
pub fn first_fp_use<P: Platform>(kind: ExceptionKind) -> bool {
    kind == ExceptionKind::IllegalInstruction && thread::start_fp::<P>() == FpStart::Started
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

    #[test]
    fn a_first_illegal_instruction_gives_fp_registers_and_runs_again() {
        use crate::tests::{FpCall, fp_calls};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        thread::start::<P>().unwrap();
        static EXITS: AtomicU64 = AtomicU64::new(0);
        EXITS.store(0, Ordering::Relaxed);
        // SAFETY: as above.
        let process = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        let context = Process::<P>::test_program_context(|program| program.illegal_instruction);
        let illegal = context.pc();
        fp_calls();
        let thread = spawn::<P, _>(process, context, move |_, _, exit| {
            let UserExit::Exception(exception) = exit else {
                panic!("not an exception: {exit:?}");
            };
            assert_eq!(exception.kind, ExceptionKind::IllegalInstruction);
            assert_eq!(exception.pc, illegal);
            EXITS.fetch_add(1, Ordering::Relaxed);
            if first_fp_use::<P>(exception.kind) {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        })
        .unwrap();
        thread.join::<P>();
        assert_eq!(EXITS.load(Ordering::Relaxed), 2);
        let calls = fp_calls();
        let calls = calls
            .strip_prefix(&[FpCall::SetUser(false)])
            .unwrap_or(&calls);
        let [
            FpCall::SetUser(true),
            FpCall::Restore(loaded),
            FpCall::Save(saved),
        ] = calls
        else {
            panic!("unexpected FP/SIMD calls: {calls:?}");
        };
        assert_eq!(loaded, saved);
    }

    #[test]
    fn the_running_thread_s_fp_registers_are_saved_for_a_change_and_loaded_again() {
        use crate::tests::{FpCall, fp_calls};
        use eggos_hal::fp::FpState;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        thread::start::<P>().unwrap();
        static SEEN: AtomicU64 = AtomicU64::new(0);
        SEEN.store(0, Ordering::Relaxed);
        assert!(thread::with_running_fp::<P, _>(|_| ()).is_none());
        // SAFETY: as above.
        let process = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        let context = Process::<P>::test_program_context(|program| program.illegal_instruction);
        let thread = spawn::<P, _>(process, context, move |_, _, exit| {
            let UserExit::Exception(exception) = exit else {
                panic!("not an exception: {exit:?}");
            };
            assert!(thread::with_running_fp::<P, _>(|_| ()).is_none());
            assert!(first_fp_use::<P>(exception.kind));
            fp_calls();
            let state = thread::with_running_fp::<P, _>(|state: &mut FpState| {
                *state = FpState::new();
                &raw mut *state as usize
            })
            .unwrap();
            SEEN.store(state as u64, Ordering::Relaxed);
            assert_eq!(fp_calls(), [FpCall::Save(state), FpCall::Restore(state)]);
            ControlFlow::Break(())
        })
        .unwrap();
        thread.join::<P>();
        assert_ne!(SEEN.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_stale_fault_on_a_mapped_page_resumes_with_no_frame_left() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        process
            .add_region(DATA, DATA + 2 * FRAME_SIZE, Attributes::USER_READ_WRITE)
            .unwrap();
        let read = fault(ExceptionKind::DataFault, Some(DATA), false);
        resolve(&process, &read).unwrap();
        let mut taken = Vec::new();
        while let Some(frame) = memory::FRAMES.lock().as_mut().unwrap().allocate() {
            taken.push(frame);
        }
        // Mapped already: the stale fault resumes; untouched: out of memory.
        assert_eq!(resolve(&process, &read), Ok(()));
        let untouched = fault(ExceptionKind::DataFault, Some(DATA + FRAME_SIZE), true);
        assert_eq!(
            resolve(&process, &untouched),
            Err(UnresolvedFault::OutOfMemory)
        );
        for frame in taken {
            // SAFETY: allocated above and never used.
            unsafe { memory::FRAMES.lock().as_mut().unwrap().free(frame) };
        }
    }
}
