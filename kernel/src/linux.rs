//! The Linux system-call ABI (D1, M6): what a process's thread does with each trip back from
//! user mode when its program is a Linux program.
//!
//! [`spawn`] starts a thread of a [`Process`] whose exits go to [`handle`]: a system call is
//! dispatched by its Linux number on the architecture ([`number`]) and returns its result, or
//! `-errno`, in the result register; an interrupt resumes the program; a page fault the
//! process resolves (a page of one of its regions touched for the first time, D52) resumes it
//! too; any other exception ends the process with the signal Linux would send for it.
//!
//! A program is started from its ELF image by [`exec::load`] (D51), which maps it and lays
//! out its stack as Linux does.
//!
//! So far: `write` to standard output and standard error (the console), `exit` and
//! `exit_group`, and the memory calls `brk`, `mmap` (anonymous), `munmap` and `mprotect`
//! ([`memory`], D53). Every other number returns `ENOSYS`, as Linux does for a call it lacks,
//! so a program that probes for a call carries on. The next calls are the rest of what a
//! static hello-world needs (`uname`, `writev`, `set_tid_address`, ...), then the file
//! system's (M7).

pub mod exec;
pub mod memory;

use crate::process::{self, Process, ProcessExit, UnresolvedFault};
use crate::thread::{self, JoinHandle, SpawnError};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Format;
use eggos_hal::user::{UserContext, UserExit};
use eggos_hal::{ExceptionKind, Platform};

/// Linux's system-call numbers. x86_64 has its own table; AArch64 and RISC-V share the generic
/// one (`asm-generic/unistd.h`).
pub mod number {
    #[cfg(target_arch = "x86_64")]
    mod table {
        pub const WRITE: u64 = 1;
        pub const MMAP: u64 = 9;
        pub const MPROTECT: u64 = 10;
        pub const MUNMAP: u64 = 11;
        pub const BRK: u64 = 12;
        pub const EXIT: u64 = 60;
        pub const EXIT_GROUP: u64 = 231;
        /// The highest number above.
        pub const LAST: u64 = EXIT_GROUP;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
        pub const WRITE: u64 = 64;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
        pub const BRK: u64 = 214;
        pub const MUNMAP: u64 = 215;
        pub const MMAP: u64 = 222;
        pub const MPROTECT: u64 = 226;
        /// The highest number above.
        pub const LAST: u64 = MPROTECT;
    }
    pub use table::*;

    // The dispatch answers `ENOSYS` above `LAST` without looking further: no call may be
    // above it.
    const _: () = {
        let all = [WRITE, EXIT, EXIT_GROUP, BRK, MMAP, MUNMAP, MPROTECT];
        let mut i = 0;
        let mut highest = 0;
        while i < all.len() {
            if all[i] > highest {
                highest = all[i];
            }
            i += 1;
        }
        assert!(
            highest == LAST,
            "number::LAST is not the highest call number"
        );
    };
}

/// A Linux error number, returned to the program as `-errno`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub u16);

impl Errno {
    /// Not permitted: here, a mapping below the lowest address allowed (`execve`, `mmap`).
    pub const EPERM: Self = Self(1);
    /// No such file: here, a program's interpreter (`execve`).
    pub const ENOENT: Self = Self(2);
    /// The arguments and environment are too long (`execve`).
    pub const E2BIG: Self = Self(7);
    /// Not a program the kernel can run (`execve`).
    pub const ENOEXEC: Self = Self(8);
    /// Bad file descriptor.
    pub const EBADF: Self = Self(9);
    /// Out of memory.
    pub const ENOMEM: Self = Self(12);
    /// Permission denied: here, memory both writable and executable (`mmap`, `mprotect`).
    pub const EACCES: Self = Self(13);
    /// Bad address: a buffer that is not the program's memory.
    pub const EFAULT: Self = Self(14);
    /// Exists already: here, a mapping in the way of `MAP_FIXED_NOREPLACE`.
    pub const EEXIST: Self = Self(17);
    /// An invalid argument: here, a segment outside user space (`execve`), an unaligned
    /// address or an empty range (the memory calls).
    pub const EINVAL: Self = Self(22);
    /// No such system call.
    pub const ENOSYS: Self = Self(38);
    /// Not supported: here, an unknown flag with `MAP_SHARED_VALIDATE` (`mmap`).
    pub const EOPNOTSUPP: Self = Self(95);

    /// The value the program sees in its result register: `-errno`.
    pub fn result(self) -> u64 {
        (-i64::from(self.0)) as u64
    }
}

/// Linux's signal numbers, as far as the kernel sends them.
pub mod signal {
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGBUS: u32 = 7;
    pub const SIGKILL: u32 = 9;
    pub const SIGSEGV: u32 = 11;
}

/// The most `write` copies at a time, through a buffer on the kernel stack.
const WRITE_CHUNK: usize = 256;

/// The most one `write` transfers, as on Linux (`write(2)`: 0x7ffff000 bytes, `INT_MAX`
/// rounded down to a page); a larger count writes this much and returns it.
const MAX_RW_COUNT: u64 = 0x7fff_f000;

/// Starts a thread of `process` that runs its Linux program from `context` (see [`handle`]).
/// Until `clone` exists, a process has this one thread.
pub fn spawn<P: Platform>(
    process: Arc<Process<P>>,
    context: UserContext,
) -> Result<JoinHandle, SpawnError> {
    process::spawn::<P, _>(process, context, handle::<P>)
}

/// What a Linux program's thread does with an exit from user mode: see the module
/// documentation. Breaks when the thread is to end. Inlined into the thread's loop: the
/// common paths (a system call that returns, an interrupt) stay short, and everything with a
/// frame of its own (`write`, the memory calls, ending the process) is out of line. Always
/// inlined: as a call of its own, the loop's state spills around it (+11% on the system-call
/// round trip when the memory calls' dispatch made it too large for the inliner).
#[inline(always)]
pub fn handle<P: Platform>(
    process: &Arc<Process<P>>,
    context: &mut UserContext,
    exit: UserExit,
) -> ControlFlow<()> {
    match exit {
        UserExit::Syscall => syscall(process, context),
        UserExit::Interrupt => ControlFlow::Continue(()),
        UserExit::Exception(exception) => {
            user_exception(process, exception.kind, exception.code, exception.address)
        }
    }
}

/// An exception in the program, of `kind` with raw cause `code` and the faulting address, if
/// any. A page fault resumes the program if the process resolves it (demand paging); otherwise
/// the process ends: `SIGKILL` when there was no memory for the page, as Linux's out-of-memory
/// killer would, else the signal Linux sends for the exception ([`kill`]). Out of line and laid
/// out away from the system-call path, which stays as short as it was (the exception's parts
/// passed alone keep the loop's state in registers): a fault costs a frame's zeroing anyway.
#[cold]
#[inline(never)]
fn user_exception<P: Platform>(
    process: &Process<P>,
    kind: ExceptionKind,
    code: u64,
    address: Option<u64>,
) -> ControlFlow<()> {
    let (Some(fault), Some(address)) = (P::page_fault(kind, code), address) else {
        return kill(process, kind);
    };
    match process.resolve_page_fault(address, fault) {
        Ok(()) => ControlFlow::Continue(()),
        Err(UnresolvedFault::NotAllowed) => kill(process, kind),
        Err(UnresolvedFault::OutOfMemory) => {
            process.exit(ProcessExit::Killed {
                signal: signal::SIGKILL,
            });
            ControlFlow::Break(())
        }
    }
}

/// Ends the process after an exception of `kind` in its program, with the signal Linux sends
/// for it: no signal handlers yet, so the default action, which for these ends the process.
#[cold]
#[inline(never)]
fn kill<P: Platform>(process: &Process<P>, kind: ExceptionKind) -> ControlFlow<()> {
    let signal = match kind {
        ExceptionKind::IllegalInstruction => signal::SIGILL,
        ExceptionKind::Breakpoint => signal::SIGTRAP,
        ExceptionKind::Alignment => signal::SIGBUS,
        ExceptionKind::DataFault
        | ExceptionKind::InstructionFault
        // Never from user mode (interrupts come back as `UserExit::Interrupt`); listed so a
        // new kind is a compile error here.
        | ExceptionKind::Interrupt
        | ExceptionKind::Other => signal::SIGSEGV,
    };
    process.exit(ProcessExit::Killed { signal });
    ControlFlow::Break(())
}

/// Runs the system call the program made and sets its result.
#[inline]
fn syscall<P: Platform>(process: &Arc<Process<P>>, context: &mut UserContext) -> ControlFlow<()> {
    let [first, second, third, ..] = context.syscall_arguments();
    let number = context.syscall_number();
    // Past every call there is: one compare, however many calls the match below grows.
    let result = if number > number::LAST {
        Err(Errno::ENOSYS)
    } else {
        match number {
            number::WRITE => write::<P>(process, first, second, third),
            number::EXIT | number::EXIT_GROUP => return exit(process, first),
            number::BRK | number::MMAP | number::MUNMAP | number::MPROTECT => {
                memory_call::<P>(process, number, context.syscall_arguments())
            }
            _ => Err(Errno::ENOSYS),
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

/// Memory call `number` with `arguments` ([`memory`]): out of line, with the arguments it
/// needs read there, so the other calls' path stays as short as it was.
#[inline(never)]
fn memory_call<P: Platform>(
    process: &Process<P>,
    number: u64,
    [first, second, third, fourth, _, sixth]: [u64; 6],
) -> Result<u64, Errno> {
    match number {
        number::BRK => Ok(memory::brk::<P>(process, first)),
        number::MMAP => memory::mmap::<P>(process, first, second, third, fourth, sixth),
        number::MUNMAP => memory::munmap::<P>(process, first, second),
        _ => memory::mprotect::<P>(process, first, second, third),
    }
}

/// `exit(status)` and `exit_group(status)`: with one thread per process, ending the thread
/// ends the process. Linux keeps the status's low 8 bits.
#[inline(never)]
fn exit<P: Platform>(process: &Process<P>, status: u64) -> ControlFlow<()> {
    process.exit(ProcessExit::Exited(status as u32 & 0xff));
    ControlFlow::Break(())
}

/// `write(fd, buffer, count)`: standard output and standard error go to the console. Returns
/// the bytes written; if part of the buffer is not the program's memory, the bytes before
/// that part, or `EFAULT` if there are none, as Linux does. Chunks never cross a page, so the
/// count is exact. At most [`MAX_RW_COUNT`] bytes, with a preemption point after each chunk, so
/// a long write does not keep its CPU from the threads waiting for it.
#[inline(never)]
fn write<P: Platform>(
    process: &Process<P>,
    fd: u64,
    buffer: u64,
    count: u64,
) -> Result<u64, Errno> {
    // `fd` is an `unsigned int`: Linux ignores the register's upper half.
    if !matches!(fd as u32, 1 | 2) {
        return Err(Errno::EBADF);
    }
    // As Linux's `access_ok`: a range that does not end within user space is `EFAULT` before
    // anything is written, even if its start is the program's memory.
    let user_end = <P::PageTableFormat as Format>::USER_END;
    if count > 0 && buffer.checked_add(count).is_none_or(|end| end > user_end) {
        return Err(Errno::EFAULT);
    }
    // Linux checks the whole range first, then caps the count.
    let count = count.min(MAX_RW_COUNT);
    let mut chunk = [0u8; WRITE_CHUNK];
    let mut written = 0;
    while written < count {
        // Cannot wrap: every chunk so far ended within user space.
        let address = buffer.wrapping_add(written);
        let length = (count - written)
            .min(WRITE_CHUNK as u64)
            .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
        let bytes = &mut chunk[..length];
        if process.read_user_memory(address, bytes).is_err() {
            return if written > 0 {
                Ok(written)
            } else {
                Err(Errno::EFAULT)
            };
        }
        P::console_write(bytes);
        written += length as u64;
        // System calls run with interrupts masked and no lock held (`handle`).
        thread::preemption_point::<P>();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory;
    use crate::paging::PageTables;
    use crate::tests::{TestPlatform as P, console_output};
    use crate::thread::host::frames_for_test;
    use eggos_hal::Exception;
    use eggos_hal::paging::Attributes;

    const DATA: u64 = 0x50_0000;

    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = memory::FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    /// A process with two readable data pages from [`DATA`], holding `contents` at the end
    /// of the first and the start of the second.
    fn process_with_data(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
        contents: &[u8],
    ) -> Arc<Process<P>> {
        let (first, second) = contents.split_at(contents.len() / 2);
        let mut page = [0u8; 4096];
        page[4096 - first.len()..].copy_from_slice(first);
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        process
            .map_new_page(DATA, Attributes::USER_READ_ONLY, &page)
            .unwrap();
        process
            .map_new_page(DATA + 4096, Attributes::USER_READ_ONLY, second)
            .unwrap();
        Arc::new(process)
    }

    /// A context that has just made system call `number` with `arguments`.
    fn called(number: u64, arguments: [u64; 6]) -> UserContext {
        let mut context = UserContext::new(0x40_0000, 0);
        context.simulate_syscall(number, arguments, 0x40_0004);
        context
    }

    #[test]
    fn write_copies_across_pages_to_the_console() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let text =
            b"hello from a buffer that spans two pages, longer than one chunk of the copy loop. \
            hello from a buffer that spans two pages, longer than one chunk of the copy loop. \
            hello from a buffer that spans two pages, longer than one chunk of the copy loop. \
            hello from a buffer that spans two pages, longer than one chunk of the copy loop.\n";
        assert!(text.len() > WRITE_CHUNK);
        let process = process_with_data(&kernel, text);
        let start = DATA + 4096 - (text.len() / 2) as u64;
        console_output();
        for fd in [1, 2] {
            let mut context = called(number::WRITE, [fd, start, text.len() as u64, 0, 0, 0]);
            assert_eq!(
                handle(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            assert_eq!(context.syscall_result(), text.len() as u64);
            assert_eq!(console_output(), text);
        }
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn write_reports_bad_buffers_and_descriptors() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_data(&kernel, b"ab");
        let write = |fd, buffer, count| {
            let mut context = called(number::WRITE, [fd, buffer, count, 0, 0, 0]);
            assert_eq!(
                handle(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            context.syscall_result()
        };
        console_output();
        // Unmapped, kernel, past user space, wrapping around.
        for buffer in [0, 0xffff_ffff_8000_0000, u64::MAX - 1] {
            assert_eq!(write(1, buffer, 2), Errno::EFAULT.result(), "{buffer:#x}");
        }
        // Nothing to write: no check, as on Linux.
        assert_eq!(write(1, 0, 0), 0);
        // A range that does not end within user space: nothing is written.
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(write(1, DATA + 4095, user_end), Errno::EFAULT.result());
        // Only standard output and standard error so far.
        assert_eq!(write(0, DATA + 4095, 2), Errno::EBADF.result());
        // Only the descriptor's low 32 bits count, as on Linux.
        assert_eq!(write(1 << 32, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(1 << 32 | 1, DATA + 4095, 0), 0);
        assert_eq!(write(3, DATA + 4095, 2), Errno::EBADF.result());
        assert!(console_output().is_empty());
        // Past the second page: everything up to it is written, then it stops.
        let readable = 1 + 4096;
        assert_eq!(
            write(1, DATA + 4095, readable + 2 * WRITE_CHUNK as u64),
            readable
        );
        assert_eq!(console_output().len() as u64, readable);
    }

    #[test]
    fn unknown_calls_return_enosys_and_exit_ends_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_data(&kernel, b"ab");
        let mut context = called(1000, [0; 6]);
        assert_eq!(
            handle(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_result(), Errno::ENOSYS.result());
        for exit in [number::EXIT, number::EXIT_GROUP] {
            let process = process_with_data(&kernel, b"ab");
            let mut context = called(exit, [0x1_2345, 0, 0, 0, 0, 0]);
            assert_eq!(
                handle(&process, &mut context, UserExit::Syscall),
                ControlFlow::Break(())
            );
            // Linux keeps the status's low 8 bits.
            assert_eq!(process.exit_status(), Some(ProcessExit::Exited(0x45)));
        }
    }

    #[test]
    fn page_faults_in_regions_resume_and_others_end_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let region = 0x60_0000;
        let fault = |process: &Arc<Process<P>>, address, code| {
            let mut context = called(1000, [0; 6]);
            let exception = Exception {
                kind: ExceptionKind::DataFault,
                name: "test page fault",
                code,
                pc: 0x40_0000,
                address: Some(address),
            };
            handle(process, &mut context, UserExit::Exception(exception))
        };
        let free = || memory::FRAMES.lock().as_ref().unwrap().free_frames();
        let with_region = |pages: u64| {
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            process
                .add_region(region, region + pages * 4096, Attributes::USER_READ_ONLY)
                .unwrap();
            Arc::new(process)
        };
        // A read in the region maps the page and resumes; a write there is SIGSEGV.
        let process = with_region(1);
        assert_eq!(fault(&process, region + 8, 0), ControlFlow::Continue(()));
        assert!(process.address_space().translate(region).is_some());
        assert_eq!(process.exit_status(), None);
        assert_eq!(
            fault(&process, region + 8, crate::tests::FAULT_WRITE),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
        // Outside every region: SIGSEGV.
        let process = with_region(1);
        assert_eq!(fault(&process, region + 4096, 0), ControlFlow::Break(()));
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
        // No frame left for the page: SIGKILL, as Linux's out-of-memory killer.
        let pages = free() + 1;
        let process = with_region(pages);
        let mut page = 0;
        while fault(&process, region + page * 4096, 0) == ControlFlow::Continue(()) {
            page += 1;
            assert!(page < pages, "the frames never ran out");
        }
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGKILL
            })
        );
    }

    #[test]
    fn exceptions_kill_the_process_with_linux_signals_and_interrupts_resume() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut context = called(1000, [0; 6]);
        let process = process_with_data(&kernel, b"ab");
        assert_eq!(
            handle(&process, &mut context, UserExit::Interrupt),
            ControlFlow::Continue(())
        );
        for (kind, signal) in [
            (ExceptionKind::IllegalInstruction, signal::SIGILL),
            (ExceptionKind::Breakpoint, signal::SIGTRAP),
            (ExceptionKind::Alignment, signal::SIGBUS),
            (ExceptionKind::DataFault, signal::SIGSEGV),
            (ExceptionKind::InstructionFault, signal::SIGSEGV),
            (ExceptionKind::Other, signal::SIGSEGV),
        ] {
            let process = process_with_data(&kernel, b"ab");
            let exception = Exception {
                kind,
                name: "test exception",
                code: 0,
                pc: 0x40_0000,
                address: None,
            };
            assert_eq!(
                handle(&process, &mut context, UserExit::Exception(exception)),
                ControlFlow::Break(())
            );
            assert_eq!(process.exit_status(), Some(ProcessExit::Killed { signal }));
            // The first way to end wins.
            process.exit(ProcessExit::Exited(0));
            assert_eq!(process.exit_status(), Some(ProcessExit::Killed { signal }));
        }
    }
}
