//! The Linux system-call ABI (D1, M6): what a process's thread does with each trip back from
//! user mode when its program is a Linux program.
//!
//! [`spawn`] starts a thread of a [`Process`] whose exits go to [`handle`]: a system call is
//! dispatched by its Linux number on the architecture ([`number`]) and returns its result, or
//! `-errno`, in the result register; an interrupt resumes the program; an exception the kernel
//! does not handle ends the process with the signal Linux would send for it.
//!
//! A program is started from its ELF image by [`exec::load`] (D51), which maps it and lays
//! out its stack as Linux does.
//!
//! So far: `write` to standard output and standard error (the console), `exit` and
//! `exit_group`. Every other number returns `ENOSYS`, as Linux does for a call it lacks, so a
//! program that probes for a call carries on. The next calls are the rest of what a static
//! hello-world needs (`brk`, `mmap`, `uname`, ...), then the file system's (M7).

pub mod exec;

use crate::process::{self, Process, ProcessExit};
use crate::thread::{JoinHandle, SpawnError};
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
        pub const EXIT: u64 = 60;
        pub const EXIT_GROUP: u64 = 231;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
        pub const WRITE: u64 = 64;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
    }
    pub use table::*;
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
    /// Bad address: a buffer that is not the program's memory.
    pub const EFAULT: Self = Self(14);
    /// An invalid argument: here, a segment outside user space (`execve`).
    pub const EINVAL: Self = Self(22);
    /// No such system call.
    pub const ENOSYS: Self = Self(38);

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
    pub const SIGSEGV: u32 = 11;
}

/// The most `write` copies at a time, through a buffer on the kernel stack.
const WRITE_CHUNK: usize = 256;

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
/// frame of its own (`write`, ending the process) is out of line.
#[inline]
pub fn handle<P: Platform>(
    process: &Arc<Process<P>>,
    context: &mut UserContext,
    exit: UserExit,
) -> ControlFlow<()> {
    match exit {
        UserExit::Syscall => syscall(process, context),
        UserExit::Interrupt => ControlFlow::Continue(()),
        UserExit::Exception(exception) => kill(process, exception.kind),
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
    let result = match context.syscall_number() {
        number::WRITE => write::<P>(process, first, second, third),
        number::EXIT | number::EXIT_GROUP => return exit(process, first),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
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
/// count is exact.
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
