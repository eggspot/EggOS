//! Processes and their threads in user mode (M4, D47, D48).
//!
//! A [`Process`] owns a user address space, the frames its pages map and its [`Regions`]: what
//! its program may use. It is built with `&mut` ([`Process::map_new_page`],
//! [`Process::add_region`]), then shared as an `Arc` by the threads that run its program
//! ([`spawn`]). From then on pages are only added: the first touch of a page in a region maps a
//! zeroed frame there ([`Process::resolve_page_fault`], demand paging, D52), and nothing is
//! removed or changed until the memory calls do it (M6). Each such thread runs in a loop
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
use crate::paging::PageTables;
use crate::regions::{Access, Region, RegionError, Regions};
use crate::sync::SpinLock;
use crate::thread::{self, JoinHandle, SpawnError};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType};
use eggos_hal::user::{TestProgram, UserContext, UserExit};
use eggos_hal::{ExceptionKind, Platform};

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

/// The access a program's exception of `kind` with raw cause `code`
/// ([`eggos_hal::Exception::code`]) made, if it is a page fault: what
/// [`Process::resolve_page_fault`] needs.
#[inline]
pub fn page_fault_access<P: Platform>(kind: ExceptionKind, code: u64) -> Option<Access> {
    match kind {
        ExceptionKind::DataFault if P::data_fault_writes(code) => Some(Access::Write),
        ExceptionKind::DataFault => Some(Access::Read),
        ExceptionKind::InstructionFault => Some(Access::Execute),
        _ => None,
    }
}

/// A user address space, the frames its pages map and its regions. See the module
/// documentation.
pub struct Process<P: Platform> {
    /// Owns the frames its user pages map ([`AddressSpace::new_owning_frames`]), so they are
    /// freed with it.
    space: AddressSpace<P>,
    /// What the program may use. Held while pages are mapped into the address space after it
    /// is shared, and while the kernel copies user memory, so a copy never meets a page being
    /// mapped (or, once the memory calls exist, unmapped). Taken before the address space's
    /// own lock.
    regions: SpinLock<Regions>,
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
                UnresolvedFault::OutOfMemory => "no frame for a user page",
                UnresolvedFault::NotAllowed => "not a user page",
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
        self.regions.get_mut().insert(start, end, attributes)
    }

    /// Maps a frame of its own at user address `virt` with `attributes`, holding `contents`
    /// (at most a page) and zero after them; an executable page is made visible to
    /// instruction fetch. Allowed while the process is shared: the caller holds its regions'
    /// lock (or `&mut`), so nothing else maps the page meanwhile. Returns the frame.
    fn map_frame(
        &self,
        virt: u64,
        attributes: Attributes,
        contents: &[u8],
    ) -> Result<u64, UnresolvedFault> {
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or(UnresolvedFault::OutOfMemory)?;
            frames.allocate().ok_or(UnresolvedFault::OutOfMemory)?
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
                Err(match error {
                    crate::paging::MapError::OutOfMemory => UnresolvedFault::OutOfMemory,
                    _ => UnresolvedFault::NotAllowed,
                })
            }
        }
    }

    /// Resolves a page fault the process's program took at user address `address`, for
    /// `access` ([`page_fault_access`]): if the address is in a region that allows the access,
    /// maps the page if it is not mapped yet (demand paging), and drops this CPU's cached
    /// translation of it, so the program can run the access again. A fault on a page already
    /// mapped for the access is a stale translation (a RISC-V hart may cache an invalid
    /// entry): dropping it is the whole fix. Call it from a thread of the process, on the CPU
    /// the program faulted on.
    pub fn resolve_page_fault(&self, address: u64, access: Access) -> Result<(), UnresolvedFault> {
        let page = address & !(FRAME_SIZE - 1);
        let regions = self.regions.lock();
        self.populate(&regions, page, access)?;
        drop(regions);
        P::drop_user_translations(page, FRAME_SIZE);
        Ok(())
    }

    /// Makes user page `page` ready for `access`, with the regions' lock held (`regions`):
    /// maps a zeroed frame there if its region allows the access and it is not mapped yet.
    /// Returns the frame and the mapping's attributes.
    fn populate(
        &self,
        regions: &Regions,
        page: u64,
        access: Access,
    ) -> Result<(u64, Attributes), UnresolvedFault> {
        let region = regions.find(page).ok_or(UnresolvedFault::NotAllowed)?;
        if !access.allowed_by(region.attributes) {
            return Err(UnresolvedFault::NotAllowed);
        }
        match self.space.translate(page) {
            Some((frame, attributes)) if access.allowed_by(attributes) => Ok((frame, attributes)),
            Some(_) => Err(UnresolvedFault::NotAllowed),
            None => {
                let frame = self.map_frame(page, region.attributes, &[])?;
                Ok((frame, region.attributes))
            }
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
        // SAFETY: the process owns its address space, and the `Arc` freezes it: `&mut` access
        // needs the process unshared.
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
                    .populate(&regions, page, access)
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
        // loaded on this CPU and shares the kernel's half (`Process::new`'s contract), and the
        // `Arc` keeps it allocated and unchanged. Nothing switches this CPU's threads or
        // address space before `run_user` returns.
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
    use eggos_hal::Exception;

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
        let access = page_fault_access::<P>(exception.kind, exception.code);
        match (access, exception.address) {
            (Some(access), Some(address)) => process.resolve_page_fault(address, access),
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
