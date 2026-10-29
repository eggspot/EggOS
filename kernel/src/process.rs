//! Processes and their threads in user mode (M4, D47, D48).
//!
//! A [`Process`] owns its memory: a user address space, the frames its pages map and its
//! [`Regions`], what its program may use, behind an `Arc` that a child started with `CLONE_VM`
//! shares, as Linux's tasks share an `mm`; a child started without it gets the pages copy on
//! write ([`Process::new_child`], D104, D169): both map each frame read-only, and the first
//! write on either side copies the page ([`crate::shared_frames`]). The memory's locks are
//! taken in this order: the program break's, the regions', the address space's, the frame
//! allocator's. It is built with `&mut` ([`Process::map_new_page`],
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

use crate::address_space::{self, AddressSpace, ForkPage};
use crate::cpu;
use crate::linux::credentials::Credentials;
use crate::linux::limits::ResourceLimits;
use crate::linux::process_table;
use crate::linux::signals::Signals;
use crate::memory::{self, FramesToFree};
use crate::paging::{MapError, PageChange, PageTables};
use crate::regions::{
    Access, FilePages, FileRange, FileRanges, ForkMarks, Region, RegionError, Regions, WHOLE_FILE,
    fork_mark, is_page_range,
};
use crate::shared_frames;
use crate::sync::{LeafSpinLock, SpinLock};
use crate::thread::{self, FpStart, JoinHandle, SpawnError};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::num::{NonZeroU32, NonZeroU64};
use core::ops::ControlFlow;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
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
    /// The page maps a file and is past the file's end, or the file could not be read (Linux
    /// sends `SIGBUS`, D167).
    BusError,
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
    /// A page maps a file and is past its end, or the file could not be read.
    BusError,
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

impl CredentialsAndLimits {
    /// A copy, `None` without memory for it.
    pub fn copy(&self) -> Option<Self> {
        Some(Self {
            credentials: self.credentials.copy()?,
            limits: self.limits.copy()?,
        })
    }
}

/// Where `execve` laid out a program (D168): what `/proc/<pid>` reads of it. Empty for a
/// process whose program the kernel built otherwise.
#[derive(Clone, Default)]
pub struct ImageLayout {
    /// The file the program was read from (`/proc/<pid>/exe`), if it was one.
    pub executable: Option<Arc<dyn FilePages>>,
    /// The argument strings on the stack, `start..end`, each with its NUL
    /// (`/proc/<pid>/cmdline`).
    pub arguments: (u64, u64),
    /// The environment strings on the stack, `start..end`.
    pub environment: (u64, u64),
    /// The stack's reserve, `start..end` (`[stack]` in `/proc/<pid>/maps`).
    pub stack: (u64, u64),
    /// Where the stack pointer started, at `argc` (`startstack` in `/proc/<pid>/stat`).
    pub stack_pointer: u64,
}

/// What a [`Mapping`] is, besides its file: `/proc/<pid>/maps`' label for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingLabel {
    /// Anything else.
    Plain,
    /// The heap `brk` grows (`[heap]`).
    Heap,
    /// The main thread's stack (`[stack]`).
    Stack,
}

/// A part of a process's user space with one access, one backing and one label: a line of
/// `/proc/<pid>/maps` (D168, [`Process::mappings`]).
#[derive(Clone)]
pub struct Mapping {
    pub start: u64,
    pub end: u64,
    /// The access, `None` for `PROT_NONE`.
    pub attributes: Option<Attributes>,
    /// Whether its pages are shared with the file or another mapping (`MAP_SHARED`).
    pub shared: bool,
    /// The file its pages hold and the offset of `start` in it.
    pub file: Option<(Arc<dyn FilePages>, u64)>,
    pub label: MappingLabel,
}

/// A user address space, the frames its pages map and its regions. See the module
/// documentation.
pub struct Process<P: Platform> {
    memory: Arc<Memory<P>>,
    root: u64,
    /// How the process ended, once it has: the first [`Self::exit`] wins.
    record: Arc<SpinLock<PidRecord<P>>>,
    /// Set once [`Self::exit`] has run: its threads run no more of its program.
    ended: AtomicBool,
    /// How many times one of its threads sent another a signal ([`Self::record_signal_sent`]),
    /// wrapping.
    signals_sent: AtomicU16,
    /// Whether signals sent to its threads wait for one to take them in
    /// ([`Self::set_sent_signals_waiting`]).
    sent_signals_waiting: AtomicBool,
    /// Its threads that run its program and have not exited ([`Self::add_thread`]).
    threads: AtomicU32,
    /// The status its first thread ended with on its own, [`NO_STATUS`] until it has
    /// ([`Self::record_first_thread_status`]).
    first_thread_status: AtomicU32,
    /// Its id, unique while the kernel runs ([`Self::id`]).
    id: ProcessId,
    vfork_parent: AtomicU32,
    /// Its credentials and resource limits ([`Self::credentials_and_limits`]).
    credentials_and_limits: SpinLock<CredentialsAndLimits>,
    /// Its signal state, which its threads share ([`Self::signals`]).
    signals: SpinLock<Signals>,
}

struct Memory<P: Platform> {
    space: AddressSpace<P>,
    regions: SpinLock<Regions>,
    program_break: SpinLock<Option<ProgramBreak>>,
    frame_offset: usize,
    futex_space: u32,
    shared_anonymous: AtomicBool,
    lent: SpinLock<Vec<LentRange>>,
    has_lent: AtomicBool,
    files: LeafSpinLock<FileRanges>,
    has_files: AtomicBool,
    fork_marks: LeafSpinLock<ForkMarks>,
    has_fork_marks: AtomicBool,
    image: ImageLayout,
}

/// Where the mapping that starts at `at` in a region ending at `end` ends: at the next bound
/// of a file range, a lent range, the heap or the stack, or the region's end.
fn mapping_end(
    at: u64,
    end: u64,
    files: &[FileRange],
    lent: &[(u64, u64)],
    heap: Option<(u64, u64)>,
    stack: (u64, u64),
) -> u64 {
    let bounds = files
        .iter()
        .flat_map(|range| [range.start, range.end])
        .chain(lent.iter().flat_map(|&(start, end)| [start, end]))
        .chain(heap.into_iter().flat_map(|(start, end)| [start, end]))
        .chain([stack.0, stack.1]);
    bounds
        .filter(|&bound| bound > at && bound < end)
        .fold(end, u64::min)
}

/// Whether `next` goes on where `last` stops, as one mapping.
fn continues(last: &Mapping, next: &Mapping) -> bool {
    let same_file = match (&last.file, &next.file) {
        (None, None) => true,
        (Some((last_file, offset)), Some((next_file, next_offset))) => {
            Arc::ptr_eq(last_file, next_file) && offset + (last.end - last.start) == *next_offset
        }
        _ => false,
    };
    last.end == next.start
        && last.attributes == next.attributes
        && last.shared == next.shared
        && last.label == next.label
        && same_file
}

/// Frames a device or a memory file lends to user space ([`Process::map_lent_frames`]): owned
/// by the lender, and kept by every process that maps them for as long as it does.
pub trait LentFrames: Send + Sync {
    /// The physical address of the frame `offset` bytes in, a multiple of the frame size below
    /// [`Self::length`].
    fn frame_at(&self, offset: u64) -> u64;
    /// How many bytes, whole frames, may be mapped.
    fn length(&self) -> u64;
    /// Whether the program may make the mapping writable (`mprotect`): `false` for a private
    /// mapping of a memory file, which shares the file's frames read-only.
    fn writable(&self) -> bool {
        true
    }
    /// Writes what the frames hold back to the file they are the pages of, at `now` (seconds
    /// since the epoch), as `msync` asks: nothing to do for frames no file is behind (D169).
    fn write_back(&self, _now: u64) -> Result<(), crate::vfs::StorageError> {
        Ok(())
    }
}

struct LentRange {
    start: u64,
    end: u64,
    frames: Arc<dyn LentFrames>,
}

enum CopyError {
    OutOfMemory,
    SharedAnonymous,
}

impl<P: Platform> Memory<P> {
    fn new(space: AddressSpace<P>, frame_offset: usize, futex_space: u32) -> Self {
        Self {
            space,
            regions: SpinLock::new(Regions::new()),
            program_break: SpinLock::new(None),
            frame_offset,
            futex_space,
            shared_anonymous: AtomicBool::new(false),
            lent: SpinLock::new(Vec::new()),
            has_lent: AtomicBool::new(false),
            files: LeafSpinLock::new_leaf(FileRanges::new()),
            has_files: AtomicBool::new(false),
            fork_marks: LeafSpinLock::new_leaf(ForkMarks::new()),
            has_fork_marks: AtomicBool::new(false),
            image: ImageLayout::default(),
        }
    }

    fn copy(&self, futex_space: u32) -> Result<Self, CopyError> {
        if self.shared_anonymous.load(Ordering::Relaxed) {
            return Err(CopyError::SharedAnonymous);
        }
        let mut space = self
            .space
            .new_owning_frames_beside()
            .map_err(|_| CopyError::OutOfMemory)?;
        let program_break = self.program_break.lock();
        let regions = self.regions.lock();
        if self.shared_anonymous.load(Ordering::Relaxed) {
            return Err(CopyError::SharedAnonymous);
        }
        let mut copied_regions = regions.try_copy().ok_or(CopyError::OutOfMemory)?;
        let mut copied_files = self.files.lock().try_copy().ok_or(CopyError::OutOfMemory)?;
        let mut marks = match self.has_fork_marks.load(Ordering::Relaxed) {
            true => self
                .fork_marks
                .lock()
                .try_copy()
                .ok_or(CopyError::OutOfMemory)?,
            false => ForkMarks::new(),
        };
        let not_forked = Self::not_forked(&marks)?;
        for &(start, end) in &not_forked {
            let cut = copied_regions
                .remove(start, end, |_| ())
                .and_then(|()| copied_files.remove(start, end))
                .and_then(|()| marks.remove(start, end));
            cut.map_err(|_| CopyError::OutOfMemory)?;
        }
        let lent = self.lent.lock();
        let copied_lent = Self::lent_outside(&lent, &not_forked)?;
        let frame_offset = self.frame_offset;
        let at = |frame: u64| (frame as usize).wrapping_add(frame_offset) as *mut u8;
        let kind = |virt: u64| {
            let within = |&(start, end): &(u64, u64)| start <= virt && virt < end;
            if not_forked.iter().any(within) || marks.at(virt) & fork_mark::WIPE_ON_FORK != 0 {
                ForkPage::Skip
            } else if lent.iter().any(|range| within(&(range.start, range.end))) {
                ForkPage::Lend
            } else {
                ForkPage::Share
            }
        };
        self.space
            .share_pages_into(&mut space, kind, |frame, new, attributes| {
                // SAFETY: both frames are reached through the frame allocator's offset: `frame`
                // is mapped in this memory, whose regions' lock is held, so it stays the
                // process's, and `new` was just allocated for the copy, so they are distinct and
                // nothing else uses `new`. Another thread of the process may write `frame`
                // meanwhile; the copy goes through raw pointers only, as kernel copies of user
                // memory do (D94).
                unsafe { ptr::copy_nonoverlapping(at(frame), at(new), FRAME_SIZE as usize) };
                if attributes.is_some_and(|attributes| attributes.executable) {
                    P::make_instructions_visible(at(new) as usize, FRAME_SIZE as usize);
                }
            })
            .map_err(|_| CopyError::OutOfMemory)?;
        drop(lent);
        let has_lent = !copied_lent.is_empty();
        let has_fork_marks = !marks.is_empty();
        Ok(Self {
            space,
            regions: SpinLock::new(copied_regions),
            program_break: SpinLock::new(*program_break),
            frame_offset,
            futex_space,
            shared_anonymous: AtomicBool::new(false),
            lent: SpinLock::new(copied_lent),
            has_lent: AtomicBool::new(has_lent),
            has_files: AtomicBool::new(!copied_files.is_empty()),
            files: LeafSpinLock::new_leaf(copied_files),
            fork_marks: LeafSpinLock::new_leaf(marks),
            has_fork_marks: AtomicBool::new(has_fork_marks),
            image: self.image.clone(),
        })
    }

    fn not_forked(marks: &ForkMarks) -> Result<Vec<(u64, u64)>, CopyError> {
        let mut ranges = Vec::new();
        for mark in marks.iter() {
            if mark.marks & fork_mark::DONT_FORK != 0 {
                ranges.try_reserve(1).map_err(|_| CopyError::OutOfMemory)?;
                ranges.push((mark.start, mark.end));
            }
        }
        Ok(ranges)
    }

    fn lent_outside(
        lent: &[LentRange],
        not_forked: &[(u64, u64)],
    ) -> Result<Vec<LentRange>, CopyError> {
        let mut copied: Vec<LentRange> = Vec::new();
        for range in lent {
            let mut from = range.start;
            for &(start, end) in not_forked {
                if start < range.end && range.start < end {
                    Self::keep_lent(&mut copied, from, start, range)?;
                    from = from.max(end);
                }
            }
            Self::keep_lent(&mut copied, from, range.end, range)?;
        }
        Ok(copied)
    }

    fn keep_lent(
        copied: &mut Vec<LentRange>,
        start: u64,
        end: u64,
        range: &LentRange,
    ) -> Result<(), CopyError> {
        if start >= end {
            return Ok(());
        }
        copied.try_reserve(1).map_err(|_| CopyError::OutOfMemory)?;
        copied.push(LentRange {
            start,
            end,
            frames: Arc::clone(&range.frames),
        });
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn unmap_files(&self, start: u64, end: u64) -> Result<(), RegionError> {
        self.files.lock().remove(start, end)
    }

    #[cold]
    #[inline(never)]
    fn unmap_fork_marks(&self, start: u64, end: u64) -> Result<(), RegionError> {
        self.fork_marks.lock().remove(start, end)
    }

    #[cold]
    #[inline(never)]
    fn move_fork_marks(&self, start: u64, length: u64, to: u64, keep_source: bool) {
        let mut marks = self.fork_marks.lock();
        if marks.copy_to(start, length, to).is_ok() && !keep_source {
            let _ = marks.remove(start, start + length);
        }
    }

    fn maps_file_in(&self, start: u64, end: u64) -> bool {
        self.has_files.load(Ordering::Relaxed) && self.files.lock().overlaps(start, end)
    }

    #[cold]
    #[inline(never)]
    fn unmap_lent(&self, start: u64, end: u64) -> Result<(), RegionError> {
        let mut lent = self.lent.lock();
        let mut index = 0;
        while index < lent.len() {
            let range = &lent[index];
            let (from, to) = (range.start.max(start), range.end.min(end));
            if from >= to {
                index += 1;
                continue;
            }
            if lent.try_reserve(1).is_err() {
                return Err(RegionError::OutOfMemory);
            }
            self.space
                .unmap_pages(from, to - from, |_| ())
                .map_err(|_| RegionError::Invalid)?;
            let range = lent.swap_remove(index);
            if range.start < from {
                lent.push(LentRange {
                    start: range.start,
                    end: from,
                    frames: Arc::clone(&range.frames),
                });
            }
            if to < range.end {
                lent.push(LentRange {
                    start: to,
                    end: range.end,
                    frames: range.frames,
                });
            }
        }
        Ok(())
    }

    fn lends_in(&self, start: u64, end: u64) -> bool {
        self.has_lent.load(Ordering::Relaxed)
            && self
                .lent
                .lock()
                .iter()
                .any(|range| range.start < end && start < range.end)
    }

    fn lends_read_only_in(&self, start: u64, end: u64) -> bool {
        self.has_lent.load(Ordering::Relaxed)
            && self
                .lent
                .lock()
                .iter()
                .any(|range| range.start < end && start < range.end && !range.frames.writable())
    }
}

impl<P: Platform> Drop for Memory<P> {
    fn drop(&mut self) {
        for range in self.lent.get_mut().drain(..) {
            self.space
                .forget_user_pages(range.start, range.end - range.start);
        }
    }
}

struct PidRecord<P: Platform> {
    id: ProcessId,
    in_table: bool,
    exit: Option<ProcessExit>,
    handed_over: Option<(usize, Arc<Process<P>>)>,
}

impl<P: Platform> PidRecord<P> {
    fn new(id: ProcessId, in_table: bool) -> Self {
        Self {
            id,
            in_table,
            exit: None,
            handed_over: None,
        }
    }
}

impl<P: Platform> Drop for PidRecord<P> {
    fn drop(&mut self) {
        if !self.in_table || self.exit.is_some() {
            return;
        }
        let removed = process_table::TABLE.lock().remove_unended(self.id.get());
        for notice in [removed.to_parent, removed.to_new_parent]
            .into_iter()
            .flatten()
        {
            crate::linux::wait::notify::<P>(notice);
        }
    }
}

/// What memory a child process starts with ([`Process::new_child`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildMemory {
    /// The parent's own, shared (`CLONE_VM`).
    Shared,
    /// The parent's pages copy on write (`fork`, D169): the same frames, read-only on both
    /// sides until either writes one, the lent ones shared, as `madvise`'s marks say
    /// ([`Process::mark_for_fork`]).
    Copied,
}

/// Why [`Process::new_child`] made no process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewChildError {
    /// Every process id has been given out.
    OutOfIds,
    /// There was no memory for the process or its copy of the parent's.
    OutOfMemory,
    /// The parent's memory has had shared anonymous memory that is not lent frames (`MAP_SHARED`
    /// with `PROT_NONE` or `PROT_EXEC`), which a copy would stop sharing
    /// ([`Process::mark_shared_anonymous`]).
    SharedAnonymousMemory,
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

fn allocate_futex_space() -> Option<u32> {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |space| {
        (space <= i32::MAX as u32).then_some(space + 1)
    })
    .ok()
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
        let futex_space = allocate_futex_space().ok_or("out of futex spaces")?;
        Ok(Self::with_memory(
            Arc::new(Memory::new(space, frame_offset, futex_space)),
            Arc::new(SpinLock::new(PidRecord::new(id, false))),
            id,
            CredentialsAndLimits::default(),
            Signals::new(),
        ))
    }

    fn with_memory(
        memory: Arc<Memory<P>>,
        record: Arc<SpinLock<PidRecord<P>>>,
        id: ProcessId,
        credentials_and_limits: CredentialsAndLimits,
        signals: Signals,
    ) -> Self {
        Self {
            root: memory.space.root(),
            memory,
            record,
            ended: AtomicBool::new(false),
            signals_sent: AtomicU16::new(0),
            sent_signals_waiting: AtomicBool::new(false),
            threads: AtomicU32::new(0),
            first_thread_status: AtomicU32::new(NO_STATUS),
            id,
            vfork_parent: AtomicU32::new(0),
            credentials_and_limits: SpinLock::new(credentials_and_limits),
            signals: SpinLock::new(signals),
        }
    }

    /// A process the kernel starts, as [`Self::new`] makes one (a new id, the default
    /// credentials and limits, no signal actions, no threads), with an empty address space
    /// beside this one's ([`AddressSpace::new_owning_frames_beside`]): what the desktop's
    /// launcher starts an app in from a thread that holds no page tables.
    pub fn new_beside(&self) -> Result<Self, &'static str> {
        let space = self
            .memory
            .space
            .new_owning_frames_beside()
            .map_err(|error| error.description())?;
        let id = ProcessId::allocate().ok_or("out of process ids")?;
        let futex_space = allocate_futex_space().ok_or("out of futex spaces")?;
        Ok(Self::with_memory(
            Arc::new(Memory::new(space, self.memory.frame_offset, futex_space)),
            Arc::new(SpinLock::new(PidRecord::new(id, false))),
            id,
            CredentialsAndLimits::default(),
            Signals::new(),
        ))
    }

    /// A process for the program `execve` starts in this one's place: an empty address space
    /// beside this one's ([`AddressSpace::new_owning_frames_beside`]), the same id, place in
    /// the process table and record of how it ends, a copy of the credentials and resource
    /// limits, no signal actions and no threads yet. This process is untouched, so the caller
    /// can still drop the new one if loading the program fails.
    pub fn new_image(&self) -> Result<Self, &'static str> {
        let space = self
            .memory
            .space
            .new_owning_frames_beside()
            .map_err(|error| error.description())?;
        let credentials_and_limits = self
            .credentials_and_limits
            .lock()
            .copy()
            .ok_or("no memory for the credentials and limits")?;
        let futex_space = allocate_futex_space().ok_or("out of futex spaces")?;
        Ok(Self::with_memory(
            Arc::new(Memory::new(space, self.memory.frame_offset, futex_space)),
            Arc::clone(&self.record),
            self.id,
            credentials_and_limits,
            Signals::new(),
        ))
    }

    /// A child of this process (`fork`, `clone` without `CLONE_THREAD`): a new id, this
    /// process as its parent in the process table, in its process group and session (its end
    /// sending no signal until `clone` sets one), a record of its own, the memory
    /// `memory` says, a copy of the
    /// credentials and resource limits and of the signal actions (nothing queued), and no
    /// threads yet. With [`ChildMemory::Copied`] this process's writable pages become
    /// read-only until written (copy on write, D169); otherwise it is untouched, so the caller
    /// can drop the child if starting it fails, and a copy that runs out of frames releases
    /// every one it took. Pages other threads of this process write during the copy may be
    /// seen by the child as they are at that moment.
    pub fn new_child(&self, memory: ChildMemory) -> Result<Self, NewChildError> {
        let id = ProcessId::allocate().ok_or(NewChildError::OutOfIds)?;
        let memory = match memory {
            ChildMemory::Shared => Arc::clone(&self.memory),
            ChildMemory::Copied => {
                let futex_space = allocate_futex_space().ok_or(NewChildError::OutOfIds)?;
                Arc::new(self.memory.copy(futex_space).map_err(|error| match error {
                    CopyError::OutOfMemory => NewChildError::OutOfMemory,
                    CopyError::SharedAnonymous => NewChildError::SharedAnonymousMemory,
                })?)
            }
        };
        let credentials_and_limits = self
            .credentials_and_limits
            .lock()
            .copy()
            .ok_or(NewChildError::OutOfMemory)?;
        let signals = self
            .signals
            .lock()
            .copy_actions()
            .ok_or(NewChildError::OutOfMemory)?;
        {
            let mut table = process_table::TABLE.lock();
            table
                .add_if_missing(self.id.get())
                .and_then(|()| table.add(id.get(), Some(self.id.get())))
                .map_err(|_| NewChildError::OutOfMemory)?;
        }
        self.record.lock().in_table = true;
        Ok(Self::with_memory(
            memory,
            Arc::new(SpinLock::new(PidRecord::new(id, true))),
            id,
            credentials_and_limits,
            signals,
        ))
    }

    /// Records that the process is in the process table, which it leaves as its record goes
    /// unless it ended as a zombie: a process the kernel starts is added as it starts
    /// (`linux::spawn`), so one that never runs a Linux program costs the table nothing.
    pub fn note_in_table(&self) {
        self.record.lock().in_table = true;
    }

    /// The id of this process's parent in the process table, 0 for the kernel (`getppid`):
    /// the process that started it, or the one its orphans went to once that one ended.
    pub fn parent_id(&self) -> u32 {
        process_table::TABLE.lock().parent(self.id.get())
    }

    /// The futex space of the process's memory: a number of its own, given to each memory as
    /// it is made and never to another, so futex words in it are one word to every process
    /// using it (`CLONE_VM`) and apart from every other memory's.
    #[inline]
    pub fn futex_space(&self) -> u32 {
        self.memory.futex_space
    }

    /// Records that the process's memory has shared anonymous memory that is not lent frames
    /// (`mmap` with `MAP_SHARED` and `PROT_NONE` or `PROT_EXEC`), before it is mapped: from
    /// then on until `execve` gives the process new memory, a child cannot get a copy of it
    /// ([`NewChildError::SharedAnonymousMemory`]).
    pub fn mark_shared_anonymous(&self) {
        self.memory.shared_anonymous.store(true, Ordering::Relaxed);
    }

    /// Whether another process uses this process's memory (`CLONE_VM` without
    /// `CLONE_THREAD`).
    pub fn shares_memory(&self) -> bool {
        Arc::strong_count(&self.memory) > 1
    }

    /// Records that the thread that started this process with `CLONE_VFORK` waits until it
    /// execs or ends, woken through futex space `futex_space` ([`Self::take_vfork_parent`]).
    pub fn set_vfork_parent(&self, futex_space: u32) {
        self.vfork_parent.store(futex_space, Ordering::Release);
    }

    /// The futex space [`Self::set_vfork_parent`] recorded, once: the waiting parent is to be
    /// woken there as the process execs or ends.
    #[inline]
    pub fn take_vfork_parent(&self) -> Option<u32> {
        if self.vfork_parent.load(Ordering::Relaxed) == 0 {
            return None;
        }
        NonZeroU32::new(self.vfork_parent.swap(0, Ordering::AcqRel)).map(NonZeroU32::get)
    }

    fn memory_mut(&mut self) -> &mut Memory<P> {
        Arc::get_mut(&mut self.memory).expect("a process is built before its memory is shared")
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
        let memory = &*self.memory;
        if contents.len() > FRAME_SIZE as usize {
            return Err("a page's contents are larger than a page");
        }
        Self::map_frame(memory, virt, attributes, contents)
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
        self.memory_mut()
            .regions
            .get_mut()
            .insert(start, end, Some(attributes))
    }

    /// Records that `range`'s pages hold its file (a program's segment, D167): a page there
    /// that is not mapped, or that `MADV_DONTNEED` discards, is read from the file when next
    /// touched. While the process is built; the pages are in a region already.
    pub fn add_file_range(&mut self, range: FileRange) -> Result<(), RegionError> {
        let memory = self.memory_mut();
        memory.files.get_mut().insert(range)?;
        *memory.has_files.get_mut() = true;
        Ok(())
    }

    /// Sets the program break's start and end to `start`, page-aligned: where the heap `brk`
    /// grows begins (Linux: the end of the program's last segment). While the process is
    /// built.
    pub fn set_program_break(&mut self, start: u64) {
        debug_assert!(start.is_multiple_of(FRAME_SIZE));
        let start = NonZeroU64::new(start).expect("the program break starts above address 0");
        *self.memory_mut().program_break.get_mut() = Some(ProgramBreak {
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
        let memory = &*self.memory;
        let mut program_break = memory.program_break.lock();
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
            let added = memory
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
        let memory = &*self.memory;
        let mut regions = memory.regions.lock();
        let start = Self::place_locked(memory, &mut regions, placement, length)?;
        regions.insert(start, start + length, attributes)?;
        Ok(start)
    }

    #[inline(always)]
    fn place_locked(
        memory: &Memory<P>,
        regions: &mut Regions,
        placement: Placement,
        length: u64,
    ) -> Result<u64, RegionError> {
        let user_end = <P::PageTableFormat as Format>::USER_END;
        if length == 0 || !length.is_multiple_of(FRAME_SIZE) {
            return Err(RegionError::Invalid);
        }
        let within = |start: u64, floor: u64, top: u64| {
            start >= floor && start.checked_add(length).is_some_and(|end| end <= top)
        };
        Ok(match placement {
            Placement::Replace(start) | Placement::Exactly(start) => {
                if !within(start, 0, user_end) {
                    return Err(RegionError::Invalid);
                }
                if let Placement::Replace(_) = placement {
                    Self::unmap_locked(memory, regions, start, start + length)?;
                }
                start
            }
            Placement::Anywhere { hint, floor, top } => {
                free_range(regions, hint, floor, top.min(user_end), length)?
            }
        })
    }

    /// Adds a region of `length` bytes with `attributes` where `placement` says, as
    /// [`Self::map_region`], and maps it at once to `frames`, from their first: memory a device
    /// or a memory file shares with the program (`MAP_SHARED` of a device, D127, or of a
    /// memfd, D139). The frames stay the lender's: unmapping the range, or the process's end,
    /// never frees them, and the process keeps `frames` until then. Such a range cannot be
    /// moved (`mremap`) or discarded (`madvise`); `fork` gives the child the same frames
    /// (D169).
    pub fn map_lent_frames(
        &self,
        placement: Placement,
        length: u64,
        attributes: Attributes,
        frames: Arc<dyn LentFrames>,
    ) -> Result<u64, RegionError> {
        let memory = &*self.memory;
        if length > frames.length() {
            return Err(RegionError::Invalid);
        }
        let mut regions = memory.regions.lock();
        let start = Self::place_locked(memory, &mut regions, placement, length)?;
        regions.insert(start, start + length, Some(attributes))?;
        let lender = Arc::clone(&frames);
        let mut lent = memory.lent.lock();
        if lent.try_reserve(1).is_err() {
            drop(lent);
            regions.remove(start, start + length, |_| ())?;
            return Err(RegionError::OutOfMemory);
        }
        lent.push(LentRange {
            start,
            end: start + length,
            frames,
        });
        memory.has_lent.store(true, Ordering::Relaxed);
        drop(lent);
        for offset in (0..length).step_by(FRAME_SIZE as usize) {
            if memory
                .space
                .map(
                    start + offset,
                    lender.frame_at(offset),
                    FRAME_SIZE,
                    attributes,
                )
                .is_err()
            {
                let _ = Self::unmap_locked(memory, &mut regions, start, start + length);
                return Err(RegionError::OutOfMemory);
            }
        }
        P::drop_user_translations(start, length);
        Ok(start)
    }

    /// Adds a region of `length` bytes with `attributes` where `placement` says, as
    /// [`Self::map_region`], whose pages hold `file` from `offset`, page-aligned (a file mapping,
    /// D167), and returns its start. Each page is read into a frame of its own when first
    /// touched, and again after `MADV_DONTNEED`; a page wholly past the file's end is a bus
    /// error ([`UnresolvedFault::BusError`]). Unless `writable`, the range may never be made
    /// writable (a shared mapping, whose pages are read as the file was). The pages are the
    /// process's own: `fork` copies them, and their file goes with them.
    pub fn map_file_region(
        &self,
        placement: Placement,
        length: u64,
        attributes: Option<Attributes>,
        file: Arc<dyn FilePages>,
        offset: u64,
        writable: bool,
    ) -> Result<u64, RegionError> {
        let memory = &*self.memory;
        let mut regions = memory.regions.lock();
        let start = Self::place_locked(memory, &mut regions, placement, length)?;
        memory.files.lock().insert(FileRange {
            start,
            end: start + length,
            offset,
            file_bytes_end: WHOLE_FILE,
            writable,
            file,
        })?;
        if let Err(error) = regions.insert(start, start + length, attributes) {
            memory.files.lock().remove(start, start + length)?;
            return Err(error);
        }
        memory.has_files.store(true, Ordering::Relaxed);
        Ok(start)
    }

    /// Whether part of user addresses `start..end` holds a file ([`Self::map_file_region`],
    /// [`Self::add_file_range`]).
    pub fn maps_file_in(&self, start: u64, end: u64) -> bool {
        let memory = &*self.memory;
        let _regions = memory.regions.lock();
        memory.maps_file_in(start, end)
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
        let memory = &*self.memory;
        if contents.len() > FRAME_SIZE as usize {
            return None;
        }
        let start = self
            .map_region(placement, FRAME_SIZE, Some(attributes))
            .ok()?;
        let regions = memory.regions.lock();
        let mapped = Self::map_frame(memory, start, attributes, contents);
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
        let memory = &*self.memory;
        let mut regions = memory.regions.lock();
        Self::unmap_locked(memory, &mut regions, start, end)
    }

    /// [`Self::unmap_range`], with the regions' lock held (`regions`).
    fn unmap_locked(
        memory: &Memory<P>,
        regions: &mut Regions,
        start: u64,
        end: u64,
    ) -> Result<(), RegionError> {
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(RegionError::Invalid);
        }
        if memory.has_files.load(Ordering::Relaxed) {
            memory.unmap_files(start, end)?;
        }
        if memory.has_fork_marks.load(Ordering::Relaxed) {
            memory.unmap_fork_marks(start, end)?;
        }
        regions.remove(start, end, |_| ())?;
        if memory.has_lent.load(Ordering::Relaxed) {
            memory.unmap_lent(start, end)?;
        }
        Self::free_pages(memory, start, end).map_err(|_| RegionError::Invalid)
    }

    /// Unmaps every page of user addresses `start..end`, page-aligned, freeing the frames, as
    /// [`Self::unmap_range`] does with the regions there: with the regions' lock held. Every
    /// page, not only those in regions: the HAL's test program has none.
    #[inline(always)]
    fn free_pages(memory: &Memory<P>, start: u64, end: u64) -> Result<(), MapError> {
        let mut unmapped = FramesToFree::new();
        let frame_offset = memory.frame_offset;
        let result = memory.space.unmap_pages(start, end - start, |frame| {
            if !shared_frames::release(frame) {
                return;
            }
            // SAFETY: the address space owns its frames (`Process::new`), so this one came from
            // the frame allocator, whose offset `frame_offset` is; its entry no longer refers to
            // it, no CPU writes it through what it cached of the entry (`unmap_pages`), and no
            // other address space a `fork` shared it with maps it (`release`), so nothing writes
            // it any more.
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
        let memory = &*self.memory;
        if !is_page_range(start, end) {
            return Err(RegionError::Invalid);
        }
        let regions = memory.regions.lock();
        if memory.lends_in(start, end) {
            return Err(RegionError::Invalid);
        }
        let free_run = |(run_start, run_end)| {
            Self::free_pages(memory, run_start, run_end).map_err(|_| RegionError::Invalid)
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

    /// Gives the parts of user addresses `start..end`, page-aligned, that regions hold the
    /// marks `change` makes of their [`fork_mark`] bits (`madvise`'s `MADV_DONTFORK`,
    /// `MADV_DOFORK`, `MADV_WIPEONFORK` and `MADV_KEEPONFORK`, D169), which the next `fork`
    /// follows ([`Self::new_child`]). With `anonymous_only`, a range that holds a file or lent
    /// frames is [`RegionError::Invalid`] and nothing changes (`MADV_WIPEONFORK` is for
    /// private anonymous memory). A part of the range in no region is
    /// [`RegionError::NotCovered`], after the rest has changed; no memory for the marks is
    /// [`RegionError::OutOfMemory`].
    pub fn mark_for_fork(
        &self,
        start: u64,
        end: u64,
        change: impl Fn(u8) -> u8,
        anonymous_only: bool,
    ) -> Result<(), RegionError> {
        let memory = &*self.memory;
        if !is_page_range(start, end) {
            return Err(RegionError::Invalid);
        }
        let regions = memory.regions.lock();
        if anonymous_only && (memory.lends_in(start, end) || memory.maps_file_in(start, end)) {
            return Err(RegionError::Invalid);
        }
        let mut marks = memory.fork_marks.lock();
        let mut covered = start;
        let mut whole = true;
        let mut updated = Ok(());
        for region in regions.overlapping(start, end) {
            let (from, to) = (region.start.max(start), region.end.min(end));
            whole &= from == covered;
            updated = marks.update(from, to, &change);
            if updated.is_err() {
                break;
            }
            covered = to;
        }
        memory
            .has_fork_marks
            .store(!marks.is_empty(), Ordering::Relaxed);
        updated?;
        match whole && covered == end {
            true => Ok(()),
            false => Err(RegionError::NotCovered),
        }
    }

    /// Writes the frames lent to user addresses `start..end` back to the files they are the
    /// pages of (`msync` of a shared file mapping, D169), at `now` (seconds since the epoch):
    /// [`crate::vfs::StorageError`] if a file could not be written, after trying them all.
    pub fn write_back_lent(
        &self,
        start: u64,
        end: u64,
        now: u64,
    ) -> Result<(), crate::vfs::StorageError> {
        let memory = &*self.memory;
        if !memory.has_lent.load(Ordering::Relaxed) {
            return Ok(());
        }
        let lent = memory.lent.lock();
        let mut result = Ok(());
        for range in lent.iter() {
            if range.start < end && start < range.end && range.frames.write_back(now).is_err() {
                result = Err(crate::vfs::StorageError);
            }
        }
        result
    }

    /// How much of user addresses `start..end` regions hold.
    pub fn coverage(&self, start: u64, end: u64) -> Coverage {
        let memory = &*self.memory;
        let regions = memory.regions.lock();
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
        let memory = &*self.memory;
        debug_assert!(is_page_range(start, end));
        let regions = memory.regions.lock();
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
                        .memory
                        .space
                        .translate(page)
                        .is_some_and(|(_, mapped)| access.allowed_by(mapped))
                    {
                        continue;
                    }
                    Self::populate(memory, &regions, page, access, true).map_err(|error| {
                        match error {
                            UnresolvedFault::NotAllowed => PopulateError::NotAllowed,
                            UnresolvedFault::OutOfMemory => PopulateError::OutOfMemory,
                            UnresolvedFault::BusError => PopulateError::BusError,
                        }
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
        let memory = &*self.memory;
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
        let mut regions = memory.regions.lock();
        if memory.lends_in(start, old_end) {
            return Err(RegionError::Invalid);
        }
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
                if memory.has_files.load(Ordering::Relaxed) {
                    let mut files = memory.files.lock();
                    let grown = files.moved_parts(start, old_length, start, new_length)?;
                    files.finish_move(start, old_end, grown, false)?;
                }
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
                Self::unmap_locked(memory, &mut regions, to, to + new_length)?;
                to
            }
        };
        let moved_files = match memory.has_files.load(Ordering::Relaxed) {
            true => Some(
                memory
                    .files
                    .lock()
                    .moved_parts(start, old_length, to, new_length)?,
            ),
            false => None,
        };
        regions.insert(to, to + new_length, region.attributes)?;
        if let Err(error) = memory.space.move_pages(start, to, old_length) {
            regions.remove(to, to + new_length, |_| ())?;
            return Err(match error {
                MapError::OutOfMemory => RegionError::OutOfMemory,
                _ => RegionError::Invalid,
            });
        }
        if let Some(moved) = moved_files {
            memory
                .files
                .lock()
                .finish_move(start, old_end, moved, keep_source)?;
        }
        if memory.has_fork_marks.load(Ordering::Relaxed) {
            memory.move_fork_marks(start, old_length, to, keep_source);
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
        let memory = &*self.memory;
        if end > <P::PageTableFormat as Format>::USER_END {
            return Err(RegionError::Invalid);
        }
        let mut regions = memory.regions.lock();
        let lent_refused = attributes.is_none_or(|attributes| attributes.executable);
        if lent_refused && memory.lends_in(start, end) {
            return Err(RegionError::Invalid);
        }
        let writes = attributes.is_some_and(|attributes| attributes.writable);
        if writes && memory.lends_read_only_in(start, end) {
            return Err(RegionError::NotWritable);
        }
        if writes
            && memory.has_files.load(Ordering::Relaxed)
            && memory.files.lock().read_only_in(start, end)
        {
            return Err(RegionError::NotWritable);
        }
        let mut changed_end = start;
        let result = regions.protect(start, end, attributes, |part| changed_end = part.end);
        if changed_end > start {
            let frame_offset = memory.frame_offset;
            memory
                .space
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
                            match new.writable && shared_frames::is_shared(frame) {
                                true => PageChange::Map(Attributes {
                                    writable: false,
                                    ..new
                                }),
                                false => PageChange::Map(new),
                            }
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
        memory: &Memory<P>,
        virt: u64,
        attributes: Attributes,
        contents: &[u8],
    ) -> Result<u64, MapError> {
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(MapError::OutOfMemory)?;
            frames.allocate_for_user().ok_or(MapError::OutOfMemory)?
        };
        let page = (frame as usize).wrapping_add(memory.frame_offset) as *mut u8;
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
        match memory.space.map(virt, frame, FRAME_SIZE, attributes) {
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
        let memory = &*self.memory;
        let page = address & !(FRAME_SIZE - 1);
        let regions = memory.regions.lock();
        Self::populate(memory, &regions, page, fault.access, fault.may_be_stale)?;
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
        memory: &Memory<P>,
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
            if memory.has_files.load(Ordering::Relaxed)
                && let Some(read) = Self::populate_from_file(memory, page, attributes)
            {
                return read;
            }
            match Self::map_frame(memory, page, attributes, &[]) {
                Ok(frame) => return Ok((frame, attributes)),
                Err(MapError::AlreadyMapped) => {}
                Err(MapError::OutOfMemory) => {
                    return Self::mapped_or_out_of_memory(memory, page, access);
                }
                Err(_) => return Err(UnresolvedFault::NotAllowed),
            }
        }
        match memory.space.translate(page) {
            Some((frame, mapped)) if mapped_is_ready && access.allowed_by(mapped) => {
                Ok((frame, mapped))
            }
            Some((frame, mapped)) if access == Access::Write && !mapped.writable => {
                Self::copy_on_write(memory, page, frame, attributes)
            }
            Some(_) => Err(UnresolvedFault::NotAllowed),
            None => {
                if memory.has_files.load(Ordering::Relaxed)
                    && let Some(read) = Self::populate_from_file(memory, page, attributes)
                {
                    return read;
                }
                match Self::map_frame(memory, page, attributes, &[]) {
                    Ok(frame) => Ok((frame, attributes)),
                    Err(MapError::OutOfMemory) => Err(UnresolvedFault::OutOfMemory),
                    Err(_) => Err(UnresolvedFault::NotAllowed),
                }
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn copy_on_write(
        memory: &Memory<P>,
        page: u64,
        frame: u64,
        attributes: Attributes,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        if !shared_frames::is_shared(frame) {
            memory
                .space
                .change_pages(
                    page,
                    FRAME_SIZE,
                    |_, _| PageChange::Map(attributes),
                    |_| unreachable!("nothing is unmapped"),
                )
                .map_err(|_| UnresolvedFault::NotAllowed)?;
            return Ok((frame, attributes));
        }
        let new = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(UnresolvedFault::OutOfMemory)?;
            frames
                .allocate_for_user()
                .ok_or(UnresolvedFault::OutOfMemory)?
        };
        let at = |frame: u64| (frame as usize).wrapping_add(memory.frame_offset) as *mut u8;
        // SAFETY: both frames are reached through the frame allocator's offset: `frame` is
        // mapped read-only by this address space and every other that shares it, so nothing
        // writes it, and the regions' lock keeps it mapped here; `new` was just allocated, so
        // they are distinct and nothing else uses `new`.
        unsafe { ptr::copy_nonoverlapping(at(frame), at(new), FRAME_SIZE as usize) };
        if attributes.executable {
            P::make_instructions_visible(at(new) as usize, FRAME_SIZE as usize);
        }
        let unmapped =
            memory
                .space
                .change_pages(page, FRAME_SIZE, |_, _| PageChange::Unmap, |_| ());
        if unmapped.is_err() {
            free_frame(new);
            return Err(UnresolvedFault::NotAllowed);
        }
        if shared_frames::release(frame) {
            free_frame(frame);
        }
        match memory.space.map(page, new, FRAME_SIZE, attributes) {
            Ok(()) => Ok((new, attributes)),
            Err(error) => {
                free_frame(new);
                Err(match error {
                    MapError::OutOfMemory => UnresolvedFault::OutOfMemory,
                    _ => UnresolvedFault::NotAllowed,
                })
            }
        }
    }

    /// For [`Self::populate`], in a process with file ranges: if user page `page` is in one
    /// and not mapped, maps it with `attributes` and a frame of its own holding the file's
    /// bytes for it, zero after them (D167); `None` for a page in no file range, or one mapped
    /// already, which the anonymous path handles. The regions' lock is held, so nothing maps or
    /// unmaps the page meanwhile; the file is read under it (file systems read their storage
    /// by polling, never by sleeping).
    #[cold]
    #[inline(never)]
    fn populate_from_file(
        memory: &Memory<P>,
        page: u64,
        attributes: Attributes,
    ) -> Option<Result<(u64, Attributes), UnresolvedFault>> {
        let (file, offset, readable) = {
            let files = memory.files.lock();
            let range = files.find(page)?;
            (
                Arc::clone(&range.file),
                range.offset_of(page),
                range.file_bytes_in(page),
            )
        };
        if memory.space.translate(page).is_some() {
            return None;
        }
        Some(Self::map_file_page(
            memory,
            page,
            attributes,
            &*file,
            offset,
            readable as usize,
        ))
    }

    /// Maps user page `page` with `attributes` and a new frame holding the first `readable`
    /// bytes of `file` from `offset` and zero after them: [`UnresolvedFault::BusError`] if the
    /// file has no byte at `offset` although `readable` is not 0 (the page is past its end), or
    /// cannot be read.
    fn map_file_page(
        memory: &Memory<P>,
        page: u64,
        attributes: Attributes,
        file: &dyn FilePages,
        offset: u64,
        readable: usize,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(UnresolvedFault::OutOfMemory)?;
            frames
                .allocate_for_user()
                .ok_or(UnresolvedFault::OutOfMemory)?
        };
        let bytes = (frame as usize).wrapping_add(memory.frame_offset) as *mut u8;
        // SAFETY: the frame was just allocated, so this code owns it, and the allocator reaches
        // it through its offset, the process's `frame_offset`; `readable` is at most a page.
        let buffer = unsafe { core::slice::from_raw_parts_mut(bytes, readable) };
        let read = match file.read_at(offset, buffer) {
            Some(0) if readable > 0 => None,
            read => read,
        };
        let Some(read) = read else {
            free_frame(frame);
            return Err(UnresolvedFault::BusError);
        };
        // SAFETY: as above: the rest of the frame, after the file's bytes.
        unsafe { eggos_hal::memory::zero(bytes.wrapping_add(read), FRAME_SIZE as usize - read) };
        if attributes.executable {
            P::make_instructions_visible(bytes as usize, FRAME_SIZE as usize);
        }
        match memory.space.map(page, frame, FRAME_SIZE, attributes) {
            Ok(()) => Ok((frame, attributes)),
            Err(error) => {
                free_frame(frame);
                Err(match error {
                    MapError::OutOfMemory => UnresolvedFault::OutOfMemory,
                    _ => UnresolvedFault::NotAllowed,
                })
            }
        }
    }

    /// For [`Self::populate`], when there was no frame to map page `page` with: the page's
    /// frame and attributes if it is mapped for `access` already (a stale fault, which then
    /// resumes), else [`UnresolvedFault::OutOfMemory`]. Cold and out of line, so the fault
    /// path stays as it was.
    #[cold]
    #[inline(never)]
    fn mapped_or_out_of_memory(
        memory: &Memory<P>,
        page: u64,
        access: Access,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        memory
            .space
            .translate(page)
            .filter(|&(_, mapped)| access.allowed_by(mapped))
            .ok_or(UnresolvedFault::OutOfMemory)
    }

    /// The region that holds user address `address`, if any.
    pub fn region(&self, address: u64) -> Option<Region> {
        let memory = &*self.memory;
        memory.regions.lock().find(address).copied()
    }

    /// Records where `execve` laid out its program (D168). While the process is built.
    pub fn set_image_layout(&mut self, layout: ImageLayout) {
        self.memory_mut().image = layout;
    }

    /// Where `execve` laid out its program ([`ImageLayout`]).
    pub fn image_layout(&self) -> &ImageLayout {
        &self.memory.image
    }

    /// The program break's start and end (`brk`), if it has one.
    pub fn program_break_range(&self) -> Option<(u64, u64)> {
        let program_break = *self.memory.program_break.lock();
        program_break.map(|ProgramBreak { start, end }| (start.get(), end))
    }

    /// Its user space as `/proc/<pid>/maps` lists it (D168): each region cut where its backing
    /// or label changes (a file's range, frames lent to it, the heap, the stack), lowest
    /// first, neighbours with the same access, backing and label joined; `None` without
    /// memory for the list.
    pub fn mappings(&self) -> Option<Vec<Mapping>> {
        let memory = &*self.memory;
        let heap = self
            .program_break_range()
            .filter(|&(start, end)| end > start)
            .map(|(start, end)| (start, end.next_multiple_of(FRAME_SIZE)));
        let stack = memory.image.stack;
        let regions = memory.regions.lock();
        let lent: Vec<(u64, u64)> = {
            let lent = memory.lent.lock();
            let mut copied = Vec::new();
            copied.try_reserve_exact(lent.len()).ok()?;
            copied.extend(lent.iter().map(|range| (range.start, range.end)));
            copied
        };
        let count = memory.files.lock().iter().count();
        let mut files: Vec<FileRange> = Vec::new();
        files.try_reserve_exact(count).ok()?;
        files.extend(memory.files.lock().iter().take(count).cloned());
        let mut mappings: Vec<Mapping> = Vec::new();
        for region in regions.iter() {
            let mut at = region.start;
            while at < region.end {
                let next = mapping_end(at, region.end, &files, &lent, heap, stack);
                let file = files
                    .iter()
                    .find(|range| range.start <= at && at < range.end);
                let lent_here = lent.iter().any(|&(start, end)| start <= at && at < end);
                let label = match heap {
                    Some((start, end)) if start <= at && at < end => MappingLabel::Heap,
                    _ if stack.0 <= at && at < stack.1 => MappingLabel::Stack,
                    _ => MappingLabel::Plain,
                };
                let mapping = Mapping {
                    start: at,
                    end: next,
                    attributes: region.attributes,
                    shared: lent_here || file.is_some_and(|range| !range.writable),
                    file: file.map(|range| (Arc::clone(&range.file), range.offset_of(at))),
                    label,
                };
                match mappings.last_mut() {
                    Some(last) if continues(last, &mapping) => last.end = next,
                    _ => {
                        mappings.try_reserve(1).ok()?;
                        mappings.push(mapping);
                    }
                }
                at = next;
            }
        }
        Some(mappings)
    }

    /// The process's address space.
    pub fn address_space(&self) -> &AddressSpace<P> {
        let memory = &*self.memory;
        &memory.space
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
        let here = cpu::current::<P>();
        if here.active_address_space() != self.root {
            // SAFETY: as `AddressSpace::load_lazily` requires: the process owns its address
            // space, whose first root `root` is, and while it is shared (the `Arc`) its mappings
            // are removed or changed only through `AddressSpace::change_pages` (the memory
            // calls): `&mut` access needs the process unshared.
            unsafe { self.memory.space.switch_lazily(here, Arc::clone(self) as _) };
        }
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
        let memory = &*self.memory;
        let destination = buffer.as_mut_ptr();
        Self::copy_user_pages(
            memory,
            address,
            buffer.len(),
            false,
            |kernel, done, length| {
                // SAFETY: `kernel` reaches `length` bytes of one of the process's frames through
                // the direct map (`copy_user_pages`' promise), and `done + length` is within
                // `buffer`, a kernel buffer, which no frame of the process is.
                unsafe {
                    core::ptr::copy_nonoverlapping(kernel, destination.wrapping_add(done), length)
                };
            },
        )
    }

    /// Copies `bytes` into the process's memory at user address `address`, as a system call
    /// fills a buffer the program passed. As [`Self::read_user_memory`], and every page must
    /// also be writable by the program.
    pub fn write_user_memory(&self, address: u64, bytes: &[u8]) -> Result<(), InvalidUserAddress> {
        let memory = &*self.memory;
        let source = bytes.as_ptr();
        Self::copy_user_pages(
            memory,
            address,
            bytes.len(),
            true,
            |kernel, done, length| {
                // SAFETY: as in `read_user_memory`, the other way round.
                unsafe {
                    core::ptr::copy_nonoverlapping(source.wrapping_add(done), kernel, length)
                };
            },
        )
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
        let memory = &*self.memory;
        if !address.is_multiple_of(4) || address >= <P::PageTableFormat as Format>::USER_END {
            return Err(InvalidUserAddress);
        }
        let regions = memory.regions.lock();
        let kernel = Self::in_direct_map(memory, &regions, address, write)?;
        // SAFETY: `kernel` reaches the word, 4-byte aligned, in one of the process's frames
        // through the direct map (`in_direct_map`' promise), mapped with the same memory type
        // as the program's mapping; the regions' lock, held until `access` returns, keeps the
        // frame the process's. The program's own accesses to the word are its own business;
        // the kernel's other accesses to it hold the regions' lock too, so none races this.
        let word = unsafe { AtomicU32::from_ptr(kernel.cast()) };
        Ok(access(word))
    }

    /// Where the kernel reaches user address `virt` through the direct map, with the regions'
    /// lock held (`regions`): its page must be a user page of normal memory (and writable, if
    /// `write`); a page in a region the program has not touched yet is mapped first, zeroed,
    /// as the program's own access would (D52). See `copy_user_pages`.
    #[inline]
    fn in_direct_map(
        memory: &Memory<P>,
        regions: &Regions,
        virt: u64,
        write: bool,
    ) -> Result<*mut u8, InvalidUserAddress> {
        let page = virt & !(FRAME_SIZE - 1);
        let (frame, attributes) = match memory.space.translate(page) {
            Some(mapped) if !write || mapped.1.writable => mapped,
            _ => {
                let access = if write { Access::Write } else { Access::Read };
                Self::populate(memory, regions, page, access, true)
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
        Ok((phys as usize).wrapping_add(memory.frame_offset) as *mut u8)
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
        memory: &Memory<P>,
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
        let regions = memory.regions.lock();
        let mut done = 0;
        while done < length {
            let virt = address + done as u64;
            let kernel = Self::in_direct_map(memory, &regions, virt, write)?;
            let in_page = (FRAME_SIZE - virt % FRAME_SIZE).min((length - done) as u64) as usize;
            copy(kernel, done, in_page);
            done += in_page;
        }
        Ok(())
    }

    /// Records how the process ended, unless it already has: the first to end it wins (say,
    /// two threads calling `exit_group`), and an `execve` that ended it ([`Self::end_for_exec`])
    /// leaves the record to the program it started. Its other threads stop running its program:
    /// each other CPU is asked to look at its scheduler, which a thread of the process does
    /// before it runs the program again (see [`spawn`]), and a CPU running one in user mode is
    /// interrupted. Returns whether this call ended it.
    pub fn exit(&self, how: ProcessExit) -> bool {
        let ended = {
            let mut record = self.record.lock();
            let ended = !self.ended.load(Ordering::Relaxed);
            if ended {
                record.exit.get_or_insert(how);
                self.ended.store(true, Ordering::Relaxed);
            }
            ended
        };
        if ended && self.threads() > 1 {
            interrupt_other_cpus::<P>();
        }
        ended
    }

    /// Ends the process's program for an `execve` in it, which goes on in a process of its own
    /// ([`Self::new_image`]) with the same record of how it ends, so nothing is recorded:
    /// its threads stop as [`Self::exit`] stops them. Returns `false`, changing nothing, if the
    /// process has ended already (another thread's `exit_group` or a fatal signal came first).
    pub fn end_for_exec(&self) -> bool {
        {
            let _exit = self.record.lock();
            if self.ended.load(Ordering::Relaxed) {
                return false;
            }
            self.ended.store(true, Ordering::Relaxed);
        }
        if self.threads() > 1 {
            interrupt_other_cpus::<P>();
        }
        true
    }

    /// Hands the process over to `next`, the process an `execve` in it started, for the thread
    /// whose registers are `context`: that thread's loop runs `next`'s program once its
    /// handler breaks ([`spawn`]); the process's other threads end.
    pub fn hand_over(&self, next: Arc<Self>, context: &UserContext) {
        self.record.lock().handed_over = Some((ptr::from_ref(context).addr(), next));
    }

    /// Takes the process [`Self::hand_over`] handed this one over to, if it was for the thread
    /// whose registers are `context`.
    pub fn take_handed_over(&self, context: &UserContext) -> Option<Arc<Self>> {
        let mut record = self.record.lock();
        match &record.handed_over {
            Some((thread, _)) if *thread == ptr::from_ref(context).addr() => {
                record.handed_over.take().map(|(_, next)| next)
            }
            _ => None,
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

    /// How many times one of its threads sent another a signal, wrapping: a thread compares it
    /// with the count it last saw to learn whether it should look for signals sent to it.
    pub fn signals_sent(&self) -> u16 {
        self.signals_sent.load(Ordering::Acquire)
    }

    /// Records that a signal was sent to the process from outside that its threads would
    /// discard as they take it in (ignored): one that looks for signals sent to it, or waits for
    /// them, takes it in then, but none is interrupted for it.
    pub fn record_ignored_signal_sent(&self) {
        self.signals_sent.fetch_add(1, Ordering::Release);
    }

    /// Records that a thread sent another a signal, now in the process's signal state, or
    /// that its pending signals may have changed (another thread ignored a signal), and
    /// interrupts every other CPU, as [`Self::exit`] does, so a thread running the program
    /// there comes back to the kernel to see it ([`spawn`]'s handler gets the interrupt).
    pub fn record_signal_sent(&self) {
        self.signals_sent.fetch_add(1, Ordering::Release);
        interrupt_other_cpus::<P>();
    }

    /// Records whether signals sent to its threads wait in its signal state for one to take
    /// them in. Set and cleared under the signals lock. While set, each of its threads that the
    /// scheduler switches back in interrupts its CPU before it runs the program again, so it
    /// comes back to the kernel at once to look (see [`spawn`]).
    pub fn set_sent_signals_waiting(&self, waiting: bool) {
        self.sent_signals_waiting.store(waiting, Ordering::Relaxed);
    }

    /// Whether signals sent to its threads wait to be taken in, as last set by
    /// [`Self::set_sent_signals_waiting`].
    pub fn sent_signals_waiting(&self) -> bool {
        self.sent_signals_waiting.load(Ordering::Relaxed)
    }

    /// For [`run`], after the scheduler looked: `Break` if the process has ended; otherwise
    /// [`Self::interrupt_here_if_sent_signals_wait`].
    #[inline]
    fn after_look(&self) -> ControlFlow<()> {
        if self.has_ended() {
            return ControlFlow::Break(());
        }
        self.interrupt_here_if_sent_signals_wait();
        ControlFlow::Continue(())
    }

    /// For a thread of the process that may have been switched out and back in: if signals
    /// sent to its threads wait to be taken in, interrupts this CPU, so the thread comes back
    /// from user mode at once and its handler can take in what was sent to it. Called after
    /// the scheduler's look before user mode ([`spawn`]'s loop) and after the preemption points a
    /// system call passes on the process's behalf.
    #[inline]
    pub fn interrupt_here_if_sent_signals_wait(&self) {
        if self.sent_signals_waiting.load(Ordering::Relaxed) {
            cpu::interrupt_here::<P>();
        }
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

    /// How the process ended, or `None` while it has not; after an `execve`, how the program
    /// started in its place ended ([`Self::new_image`]).
    pub fn exit_status(&self) -> Option<ProcessExit> {
        self.record.lock().exit
    }
}

impl<P: Platform> core::fmt::Debug for Process<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Process")
            .field("regions", &*self.memory.regions.lock())
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
    // SAFETY: every caller's frame came from this allocator for a user page and nothing maps it
    // any more: one that could not be mapped (`Process::map_frame`, `map_file_page`), or one
    // copy on write unmapped and released as the last address space mapping it.
    unsafe { frames.free(frame) };
}

/// Starts a thread of `process` that runs its program from `context` in user mode and hands
/// every exit, with the program's registers, to `handler`, until it returns
/// [`ControlFlow::Break`]: then the thread ends. On a system call the handler sets the result
/// ([`UserContext::set_syscall_result`]); on an interrupt, already handled, it usually goes on;
/// on an exception it decides the program's fate. It runs with interrupts masked, as the
/// kernel does, and may block. When it breaks after an `execve` handed the process over to
/// another ([`Process::hand_over`]), the thread runs that one's program from then on. Placed
/// as [`thread::spawn`] places threads.
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
    let mut next = Some(process);
    while let Some(process) = next {
        next = run_program(process, &mut context, &mut handler);
    }
}

#[inline(always)]
fn run_program<P: Platform, H>(
    process: Arc<Process<P>>,
    context: &mut UserContext,
    handler: &mut H,
) -> Option<Arc<Process<P>>>
where
    H: FnMut(&Arc<Process<P>>, &mut UserContext, UserExit) -> ControlFlow<()>,
{
    let mut ended = process.has_ended();
    while !ended {
        // First, as it may switch threads: nothing may between loading and running.
        if thread::preemption_point_before_user_mode::<P>() && process.after_look().is_break() {
            break;
        }
        process.load_here();
        // SAFETY: interrupts are masked, as the kernel runs. The process's address space is
        // loaded on this CPU, not stale, and shares the kernel's half (`Process::new`'s
        // contract); the `Arc` keeps it allocated, and its mappings change only through
        // `AddressSpace::change_pages` and its kin, on this thread or on another CPU, whose
        // change marks this CPU stale or, for a process with threads on several CPUs, shoots
        // its translations down before any frame is reused (D94). Nothing switches this CPU's
        // threads or address space before `run_user` returns.
        let exit = unsafe { P::run_user(context) };
        ended = handler(&process, context, exit).is_break();
    }
    leave_program(process, context)
}

#[cold]
#[inline(never)]
fn leave_program<P: Platform>(
    process: Arc<Process<P>>,
    context: &UserContext,
) -> Option<Arc<Process<P>>> {
    let next = process.take_handed_over(context);
    let here = cpu::current::<P>();
    let root = process.address_space().root();
    drop(process);
    address_space::release_ended::<P>(here, root);
    next
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
    use alloc::vec::Vec;
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
    fn a_child_gets_a_copy_of_every_page_region_and_the_break_and_frees_them() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: as above.
            let mut parent = unsafe { Process::<P>::new(&kernel) }.unwrap();
            let kept = DATA + FRAME_SIZE;
            let untouched = DATA + 2 * FRAME_SIZE;
            parent
                .map_new_page(DATA, Attributes::USER_READ_WRITE, b"parent data")
                .unwrap();
            parent
                .map_new_page(kept, Attributes::USER_TEXT, b"code")
                .unwrap();
            parent
                .add_region(DATA, kept, Attributes::USER_READ_WRITE)
                .unwrap();
            parent
                .add_region(kept, untouched, Attributes::USER_TEXT)
                .unwrap();
            parent
                .add_region(
                    untouched,
                    untouched + FRAME_SIZE,
                    Attributes::USER_READ_WRITE,
                )
                .unwrap();
            parent.set_program_break(untouched + FRAME_SIZE);
            let parent = Arc::new(parent);
            parent.protect_range(kept, untouched, None).unwrap();
            let with_parent = free();
            let child = parent.new_child(ChildMemory::Copied).unwrap();
            assert_ne!(child.id(), parent.id());
            assert_eq!(child.parent_id(), parent.id().get());
            assert_ne!(child.futex_space(), parent.futex_space());
            assert!(!parent.shares_memory() && !child.shares_memory());
            let (parent_frame, attributes) = parent.address_space().translate(DATA).unwrap();
            let (child_frame, child_attributes) = child.address_space().translate(DATA).unwrap();
            assert_eq!(parent_frame, child_frame);
            assert!(!attributes.writable && !child_attributes.writable);
            assert!(crate::shared_frames::is_shared(parent_frame));
            assert_eq!(&page_bytes(&child, DATA)[..11], b"parent data");
            let shared = free();
            child.write_user_memory(DATA, b"child").unwrap();
            assert_eq!(free(), shared - 1, "the child's write copied the page");
            let (copied, copied_attributes) = child.address_space().translate(DATA).unwrap();
            assert_ne!(copied, parent_frame);
            assert!(copied_attributes.writable);
            assert!(!crate::shared_frames::is_shared(parent_frame));
            assert_eq!(&page_bytes(&parent, DATA)[..11], b"parent data");
            assert_eq!(&page_bytes(&child, DATA)[..11], b"childt data");
            parent.write_user_memory(DATA, b"P").unwrap();
            assert_eq!(free(), shared - 1, "the parent's write took its frame back");
            let (frame, attributes) = parent.address_space().translate(DATA).unwrap();
            assert_eq!((frame, attributes.writable), (parent_frame, true));
            assert_eq!(&page_bytes(&parent, DATA)[..11], b"Parent data");
            for address in [DATA, kept, untouched] {
                assert_eq!(child.region(address), parent.region(address));
            }
            assert_eq!(child.address_space().translate(kept), None);
            assert_eq!(child.address_space().translate(untouched), None);
            child
                .protect_range(kept, untouched, Some(Attributes::USER_READ_ONLY))
                .unwrap();
            assert_eq!(&page_bytes(&child, kept)[..4], b"code");
            let end = untouched + FRAME_SIZE;
            assert_eq!(child.move_program_break(end + 10, u64::MAX), end + 10);
            assert_eq!(parent.move_program_break(0, u64::MAX), end);
            assert!(free() < with_parent);
            drop(child);
            assert_eq!(free(), with_parent);
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn a_fork_without_frames_for_its_tables_takes_none_and_leaves_the_parent_as_it_was() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let mut parent = unsafe { Process::<P>::new(&kernel) }.unwrap();
        for page in 0..3 {
            parent
                .map_new_page(
                    DATA + page * FRAME_SIZE,
                    Attributes::USER_READ_WRITE,
                    b"kept",
                )
                .unwrap();
        }
        let mut hoard = Vec::new();
        while free() > 1 {
            hoard.push(memory::FRAMES.lock().as_mut().unwrap().allocate().unwrap());
        }
        let before = free();
        assert_eq!(
            parent.new_child(ChildMemory::Copied).err(),
            Some(NewChildError::OutOfMemory)
        );
        assert_eq!(free(), before);
        for page in 0..3 {
            assert_eq!(&page_bytes(&parent, DATA + page * FRAME_SIZE)[..4], b"kept");
        }
        let mut frames = memory::FRAMES.lock();
        for frame in hoard {
            // SAFETY: allocated above from this allocator and never mapped.
            unsafe { frames.as_mut().unwrap().free(frame) };
        }
    }

    #[test]
    fn a_child_sharing_the_memory_shares_its_pages_and_futex_space() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let mut parent = unsafe { Process::<P>::new(&kernel) }.unwrap();
        parent
            .map_new_page(DATA, Attributes::USER_READ_WRITE, b"shared")
            .unwrap();
        let parent = Arc::new(parent);
        let used = free();
        let child = parent.new_child(ChildMemory::Shared).unwrap();
        assert_eq!(free(), used);
        assert_eq!(child.futex_space(), parent.futex_space());
        assert!(parent.shares_memory() && child.shares_memory());
        child.write_user_memory(DATA, b"SHARED").unwrap();
        assert_eq!(&page_bytes(&parent, DATA)[..6], b"SHARED");
        let image = child.new_image().unwrap();
        assert_ne!(image.futex_space(), parent.futex_space());
        assert_eq!(image.parent_id(), parent.id().get());
        let parent_image = parent.new_image().unwrap();
        assert_ne!(parent_image.futex_space(), child.futex_space());
        assert_ne!(parent_image.futex_space(), image.futex_space());
        drop((child, image));
        assert!(!parent.shares_memory());
    }

    #[test]
    fn memory_with_shared_anonymous_memory_is_shared_with_children_and_never_copied() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let parent = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
        parent.mark_shared_anonymous();
        let before = free();
        assert_eq!(
            parent.new_child(ChildMemory::Copied).err(),
            Some(NewChildError::SharedAnonymousMemory)
        );
        assert_eq!(free(), before);
        let sharer = parent.new_child(ChildMemory::Shared).unwrap();
        assert_eq!(
            sharer.new_child(ChildMemory::Copied).err(),
            Some(NewChildError::SharedAnonymousMemory)
        );
        let image = parent.new_image().unwrap();
        assert!(image.new_child(ChildMemory::Copied).is_ok());
    }

    #[test]
    fn a_child_is_in_the_table_under_its_parent_until_it_goes_and_a_vfork_parent_is_taken_once() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let parent = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
        let child = parent.new_child(ChildMemory::Shared).unwrap();
        let (parent_id, child_id) = (parent.id().get(), child.id().get());
        let in_table = |id| process_table::TABLE.lock().group_and_session(id);
        assert_eq!(in_table(parent_id), Some((parent_id, parent_id)));
        assert_eq!(in_table(child_id), Some((parent_id, parent_id)));
        assert_eq!(child.parent_id(), parent_id);
        let image = child.new_image().unwrap();
        assert_eq!(image.parent_id(), parent_id);
        child.set_vfork_parent(parent.futex_space());
        assert_eq!(child.take_vfork_parent(), Some(parent.futex_space()));
        assert_eq!(child.take_vfork_parent(), None);
        drop(child);
        assert!(in_table(child_id).is_some());
        drop(image);
        assert_eq!(in_table(child_id), None);
        drop(parent);
        assert_eq!(in_table(parent_id), None);
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
        let calls = match &calls[..] {
            [FpCall::Restore(_), FpCall::SetUser(false), rest @ ..] => rest,
            [FpCall::SetUser(false), rest @ ..] => rest,
            rest => rest,
        };
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

    struct TestLentFrames {
        physical: u64,
        order: u32,
        drops: Arc<AtomicU64>,
    }

    impl TestLentFrames {
        fn new(pages: u64, drops: &Arc<AtomicU64>) -> Arc<Self> {
            let order = pages.next_power_of_two().trailing_zeros();
            let physical = memory::FRAMES
                .lock()
                .as_mut()
                .unwrap()
                .allocate_contiguous(order)
                .unwrap();
            Arc::new(Self {
                physical,
                order,
                drops: Arc::clone(drops),
            })
        }
    }

    impl LentFrames for TestLentFrames {
        fn frame_at(&self, offset: u64) -> u64 {
            self.physical + offset
        }

        fn length(&self) -> u64 {
            FRAME_SIZE << self.order
        }
    }

    impl Drop for TestLentFrames {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
            let mut frames = memory::FRAMES.lock();
            // SAFETY: allocated by `new` with this order, and this is its last holder.
            unsafe {
                frames
                    .as_mut()
                    .unwrap()
                    .free_contiguous(self.physical, self.order)
            };
        }
    }

    #[test]
    fn lent_frames_are_mapped_split_replaced_and_kept_until_the_process_ends() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        let drops = Arc::new(AtomicU64::new(0));
        {
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            let lent = TestLentFrames::new(4, &drops);
            let physical = lent.physical;
            let length = 4 * FRAME_SIZE;
            assert_eq!(
                process.map_lent_frames(
                    Placement::Exactly(DATA),
                    length,
                    Attributes::USER_READ_WRITE,
                    Arc::clone(&lent) as Arc<dyn LentFrames>,
                ),
                Ok(DATA)
            );
            let at = |virt| {
                process
                    .address_space()
                    .translate(virt)
                    .map(|(phys, _)| phys)
            };
            for page in 0..4 {
                assert_eq!(
                    at(DATA + page * FRAME_SIZE),
                    Some(physical + page * FRAME_SIZE)
                );
            }
            assert_eq!(
                process.protect_range(DATA, DATA + FRAME_SIZE, None),
                Err(RegionError::Invalid)
            );
            assert_eq!(
                process.protect_range(DATA, DATA + FRAME_SIZE, Some(Attributes::USER_TEXT)),
                Err(RegionError::Invalid)
            );
            assert_eq!(
                process.protect_range(DATA, DATA + FRAME_SIZE, Some(Attributes::USER_READ_ONLY)),
                Ok(())
            );
            assert_eq!(
                process.remap_region(
                    DATA,
                    FRAME_SIZE,
                    2 * FRAME_SIZE,
                    Remap::InPlace {
                        top: DATA + 8 * FRAME_SIZE
                    },
                    false
                ),
                Err(RegionError::Invalid)
            );
            assert_eq!(
                process.discard_pages(DATA, DATA + FRAME_SIZE),
                Err(RegionError::Invalid)
            );
            let child = process.new_child(ChildMemory::Copied).unwrap();
            let in_child = child
                .address_space()
                .translate(DATA + 2 * FRAME_SIZE)
                .map(|(phys, _)| phys);
            assert_eq!(in_child, Some(physical + 2 * FRAME_SIZE));
            assert!(!crate::shared_frames::is_shared(physical));
            drop(child);
            assert_eq!(at(DATA + 2 * FRAME_SIZE), Some(physical + 2 * FRAME_SIZE));
            process
                .unmap_range(DATA + FRAME_SIZE, DATA + 3 * FRAME_SIZE)
                .unwrap();
            assert_eq!(at(DATA), Some(physical));
            assert_eq!(at(DATA + FRAME_SIZE), None);
            assert_eq!(at(DATA + 2 * FRAME_SIZE), None);
            assert_eq!(at(DATA + 3 * FRAME_SIZE), Some(physical + 3 * FRAME_SIZE));
            assert_eq!(
                process.discard_pages(DATA + FRAME_SIZE, DATA + 3 * FRAME_SIZE),
                Err(RegionError::NotCovered),
                "the unmapped middle is no longer lent"
            );
            assert_eq!(
                process.map_region(
                    Placement::Replace(DATA + 3 * FRAME_SIZE),
                    FRAME_SIZE,
                    Some(Attributes::USER_READ_WRITE)
                ),
                Ok(DATA + 3 * FRAME_SIZE)
            );
            assert_eq!(
                at(DATA + 3 * FRAME_SIZE),
                None,
                "MAP_FIXED unmapped the lent page"
            );
            assert_eq!(
                process.discard_pages(DATA + 3 * FRAME_SIZE, DATA + 4 * FRAME_SIZE),
                Ok(())
            );
            drop(lent);
            assert_eq!(
                drops.load(Ordering::Relaxed),
                0,
                "the process still maps a page"
            );
            assert_eq!(at(DATA), Some(physical));
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(free(), before, "every frame went back exactly once");
    }

    /// A file of `pages` pages and `tail` bytes, each byte its offset's low byte plus 1.
    fn test_file(pages: u64, tail: u64) -> Arc<crate::regions::BytesFile> {
        let length = pages * FRAME_SIZE + tail;
        Arc::new(crate::regions::BytesFile(
            (0..length).map(|at| (at as u8).wrapping_add(1)).collect(),
        ))
    }

    const READ_FAULT: PageFault = PageFault {
        access: Access::Read,
        may_be_stale: false,
    };

    /// The page at `virt` as a kernel copy reads it, which maps it first if it is not.
    fn read_page(process: &Process<P>, virt: u64) -> Vec<u8> {
        let mut bytes = alloc::vec![0; FRAME_SIZE as usize];
        process.read_user_memory(virt, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn file_pages_are_read_when_touched_and_past_the_end_are_a_bus_error() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            let file = test_file(2, 100);
            let start = process
                .map_file_region(
                    Placement::Exactly(DATA),
                    4 * FRAME_SIZE,
                    Some(Attributes::USER_READ_WRITE),
                    file.clone() as Arc<dyn FilePages>,
                    FRAME_SIZE,
                    true,
                )
                .unwrap();
            assert_eq!(start, DATA);
            assert_eq!(process.address_space().translate(DATA), None);
            let page = read_page(&process, DATA);
            assert_eq!(
                page[..],
                file.0[FRAME_SIZE as usize..2 * FRAME_SIZE as usize]
            );
            let tail = read_page(&process, DATA + FRAME_SIZE);
            assert_eq!(tail[..100], file.0[2 * FRAME_SIZE as usize..]);
            assert!(tail[100..].iter().all(|&byte| byte == 0));
            let past = DATA + 2 * FRAME_SIZE;
            assert_eq!(
                process.resolve_page_fault(past, READ_FAULT),
                Err(UnresolvedFault::BusError)
            );
            assert_eq!(process.address_space().translate(past), None);
            assert_eq!(
                process.read_user_memory(past, &mut [0; 8]),
                Err(InvalidUserAddress)
            );
            assert_eq!(
                process.populate_range(DATA, DATA + 4 * FRAME_SIZE, Access::Write),
                Err(PopulateError::BusError)
            );
            process.write_user_memory(DATA, b"changed").unwrap();
            assert_eq!(&file.0[FRAME_SIZE as usize..][..7], &page[..7]);
            process.discard_pages(DATA, DATA + FRAME_SIZE).unwrap();
            assert_eq!(read_page(&process, DATA), page, "read from the file again");
            process
                .unmap_range(DATA + FRAME_SIZE, DATA + 2 * FRAME_SIZE)
                .unwrap();
            assert!(process.maps_file_in(DATA, DATA + FRAME_SIZE));
            assert!(!process.maps_file_in(DATA + FRAME_SIZE, DATA + 2 * FRAME_SIZE));
            process
                .map_region(
                    Placement::Exactly(DATA + FRAME_SIZE),
                    FRAME_SIZE,
                    Some(Attributes::USER_READ_WRITE),
                )
                .unwrap();
            assert!(
                read_page(&process, DATA + FRAME_SIZE)
                    .iter()
                    .all(|&byte| byte == 0),
                "an anonymous mapping where the file was reads zeros"
            );
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn file_pages_move_with_mremap_and_go_with_fork_and_shared_ones_stay_read_only() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // SAFETY: as above.
        let process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        let file = test_file(8, 0);
        process
            .map_file_region(
                Placement::Exactly(DATA),
                2 * FRAME_SIZE,
                Some(Attributes::USER_READ_ONLY),
                file.clone() as Arc<dyn FilePages>,
                0,
                true,
            )
            .unwrap();
        let first = read_page(&process, DATA);
        let to = DATA + 64 * FRAME_SIZE;
        assert_eq!(
            process.remap_region(
                DATA,
                2 * FRAME_SIZE,
                3 * FRAME_SIZE,
                Remap::Replace(to),
                false
            ),
            Ok(to)
        );
        assert_eq!(read_page(&process, to), first, "the mapped page moved");
        assert_eq!(
            read_page(&process, to + 2 * FRAME_SIZE)[..],
            file.0[2 * FRAME_SIZE as usize..3 * FRAME_SIZE as usize],
            "the grown page is the file's next"
        );
        let child = process.new_child(ChildMemory::Copied).unwrap();
        assert_eq!(
            read_page(&child, to + FRAME_SIZE)[..],
            file.0[FRAME_SIZE as usize..2 * FRAME_SIZE as usize],
            "the child reads an untouched page from the file"
        );
        let shared = process
            .map_file_region(
                Placement::Exactly(DATA),
                FRAME_SIZE,
                Some(Attributes::USER_READ_ONLY),
                file.clone() as Arc<dyn FilePages>,
                0,
                false,
            )
            .unwrap();
        assert_eq!(
            process.protect_range(
                shared,
                shared + FRAME_SIZE,
                Some(Attributes::USER_READ_WRITE)
            ),
            Err(RegionError::NotWritable)
        );
        assert_eq!(
            process.protect_range(to, to + FRAME_SIZE, Some(Attributes::USER_READ_WRITE)),
            Ok(())
        );
    }

    #[test]
    fn fork_marks_leave_ranges_out_of_the_child_or_give_it_zeros() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            // SAFETY: as above.
            let parent = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
            parent
                .map_region(
                    Placement::Exactly(DATA),
                    3 * FRAME_SIZE,
                    Some(Attributes::USER_READ_WRITE),
                )
                .unwrap();
            for page in 0..3 {
                parent
                    .write_user_memory(DATA + page * FRAME_SIZE, b"parent")
                    .unwrap();
            }
            let (kept, not_forked, wiped) = (DATA, DATA + FRAME_SIZE, DATA + 2 * FRAME_SIZE);
            let dont_fork = |marks| marks | fork_mark::DONT_FORK;
            let wipe_on_fork = |marks| marks | fork_mark::WIPE_ON_FORK;
            parent
                .mark_for_fork(not_forked, wiped, dont_fork, false)
                .unwrap();
            parent
                .mark_for_fork(wiped, wiped + FRAME_SIZE, wipe_on_fork, true)
                .unwrap();
            assert_eq!(
                parent.mark_for_fork(DATA, DATA + 8 * FRAME_SIZE, |marks| marks, false),
                Err(RegionError::NotCovered)
            );
            let child = parent.new_child(ChildMemory::Copied).unwrap();
            assert_eq!(&page_bytes(&child, kept)[..6], b"parent");
            assert_eq!(child.region(not_forked), None);
            assert!(child.region(wiped).is_some());
            assert_eq!(child.address_space().translate(wiped), None);
            let mut zeros = [1u8; 6];
            child.read_user_memory(wiped, &mut zeros).unwrap();
            assert_eq!(zeros, [0; 6]);
            assert_eq!(&page_bytes(&parent, wiped)[..6], b"parent");
            let grandchild = child.new_child(ChildMemory::Copied).unwrap();
            assert_eq!(
                grandchild.address_space().translate(wiped),
                None,
                "the child keeps the wipe-on-fork mark"
            );
            drop((grandchild, child));
            parent
                .mark_for_fork(
                    not_forked,
                    wiped,
                    |marks| marks & !fork_mark::DONT_FORK,
                    false,
                )
                .unwrap();
            let child = parent.new_child(ChildMemory::Copied).unwrap();
            assert_eq!(&page_bytes(&child, not_forked)[..6], b"parent");
        }
        assert_eq!(free(), before);
    }
}
