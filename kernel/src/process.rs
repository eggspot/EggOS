//! Processes and their threads in user mode (M4, D47, D48).
//!
//! A [`Process`] owns a user address space and the frames its pages map. It is built with
//! `&mut` ([`Process::map_new_page`]), then shared as an `Arc` by the threads that run its
//! program ([`spawn`]): from then on its mappings are frozen. Each such thread runs in a loop
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
//! For now the only program is the HAL's test program ([`Process::with_test_program`]), for the
//! boot self-tests and benchmarks; loading programs from files comes with the file system (M7).

use crate::address_space::{self, AddressSpace};
use crate::cpu;
use crate::memory;
use crate::paging::PageTables;
use crate::sync::SpinLock;
use crate::thread::{self, JoinHandle, SpawnError};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType};
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

/// A user address space and the frames its pages map. See the module documentation.
pub struct Process<P: Platform> {
    /// Always `Some` until dropped: taken first in `drop`, so the tables go before the frames
    /// they map.
    space: Option<AddressSpace<P>>,
    /// The frames the process's pages map, freed with it.
    frames: Vec<u64>,
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
        let space = unsafe { AddressSpace::new(kernel) }.map_err(|error| error.description())?;
        let frame_offset = memory::FRAMES
            .lock()
            .as_ref()
            .ok_or("there is no frame allocator")?
            .physical_memory_offset();
        Ok(Self {
            space: Some(space),
            frames: Vec::new(),
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
    /// program can read. An executable page is made visible to instruction fetch.
    pub fn map_new_page(
        &mut self,
        virt: u64,
        attributes: Attributes,
        contents: &[u8],
    ) -> Result<(), &'static str> {
        if contents.len() > FRAME_SIZE as usize {
            return Err("a page's contents are larger than a page");
        }
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or("there is no frame allocator")?;
            let frame = frames.allocate().ok_or("no frame for a user page")?;
            let page = (frame as usize).wrapping_add(frames.physical_memory_offset()) as *mut u8;
            // SAFETY: the frame was just allocated, so this code owns it, and the allocator
            // reaches it through its offset.
            unsafe { core::ptr::write_bytes(page, 0, FRAME_SIZE as usize) };
            // SAFETY: as above; `contents` fit in the frame (checked above) and are not in it,
            // so the two do not overlap.
            unsafe { core::ptr::copy_nonoverlapping(contents.as_ptr(), page, contents.len()) };
            if attributes.executable {
                // The whole page: its zeroed rest too, or instruction fetch past the contents
                // could see the frame's old data.
                P::make_instructions_visible(page as usize, FRAME_SIZE as usize);
            }
            frame
        };
        match self.space_mut().map(virt, frame, FRAME_SIZE, attributes) {
            Ok(()) => {
                self.frames.push(frame);
                Ok(())
            }
            Err(error) => {
                free_frame(frame);
                Err(error.description())
            }
        }
    }

    /// The process's address space.
    pub fn address_space(&self) -> &AddressSpace<P> {
        self.space
            .as_ref()
            .expect("the address space lives until drop")
    }

    fn space_mut(&mut self) -> &mut AddressSpace<P> {
        self.space
            .as_mut()
            .expect("the address space lives until drop")
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
    /// the window ([`Platform::with_user_access`]). `access` must not block or switch threads.
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
    /// `write`). No user-access window is opened and the address space need not be loaded on
    /// this CPU: the copy goes through the direct map (the process's `frame_offset`), so it
    /// can never fault (D50, decided by research). That is sound while a process's mappings are frozen and it owns every frame
    /// they map (an `&self` here means no `&mut` can remap or free them); once mappings can
    /// change under running threads (`munmap`, demand paging, copy-on-write), copies move to
    /// user addresses with fault fixups, behind this same interface.
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
        let space = self.address_space();
        let mut done = 0;
        while done < length {
            let virt = address + done as u64;
            let (phys, attributes) = space.translate(virt).ok_or(InvalidUserAddress)?;
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
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl<P: Platform> Drop for Process<P> {
    fn drop(&mut self) {
        drop(self.space.take());
        for &frame in &self.frames {
            free_frame(frame);
        }
    }
}

/// Gives a process's frame back to the kernel's frame allocator.
fn free_frame(frame: u64) {
    let mut frames = memory::FRAMES.lock();
    let frames = frames
        .as_mut()
        .expect("the frame came from the frame allocator");
    // SAFETY: the frame came from this allocator in `Process::map_new_page`, and nothing maps
    // it any more: the address space that did is gone (or never mapped it).
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
    use crate::tests::{TestPlatform as P, active_root, set_active_root};
    use crate::thread::host::frames_for_test;
    use core::sync::atomic::{AtomicU64, Ordering};

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
            assert_eq!(
                (phys, attributes),
                (process.frames[0], Attributes::USER_TEXT)
            );
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

        // Read-only, unmapped, kernel, past user space, wrapping: refused.
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
            assert!(process.frames.is_empty());
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
