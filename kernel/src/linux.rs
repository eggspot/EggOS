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
//! So far: `write` and `writev` to standard output and standard error (the console), `exit`
//! and `exit_group`, the memory calls `brk`, `mmap` (anonymous), `munmap` and `mprotect`
//! ([`memory`], D53), and `getpid`, `getppid`, `gettid`, `uname`, `set_tid_address` and
//! `set_robust_list` ([`identity`], D55). Every other number returns `ENOSYS`, as Linux does
//! for a call it lacks, so a program that probes for a call carries on. The next calls are the
//! rest of what a static program's startup needs (`arch_prctl` and the thread pointer,
//! signals, `getrandom`, ...), then the file system's (M7).

pub mod exec;
pub mod identity;
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
        pub const WRITEV: u64 = 20;
        pub const GETPID: u64 = 39;
        pub const EXIT: u64 = 60;
        pub const UNAME: u64 = 63;
        pub const GETPPID: u64 = 110;
        pub const GETTID: u64 = 186;
        pub const SET_TID_ADDRESS: u64 = 218;
        pub const EXIT_GROUP: u64 = 231;
        pub const SET_ROBUST_LIST: u64 = 273;
        /// The highest number above.
        pub const LAST: u64 = SET_ROBUST_LIST;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
        pub const WRITE: u64 = 64;
        pub const WRITEV: u64 = 66;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
        pub const SET_TID_ADDRESS: u64 = 96;
        pub const SET_ROBUST_LIST: u64 = 99;
        pub const UNAME: u64 = 160;
        pub const GETPID: u64 = 172;
        pub const GETPPID: u64 = 173;
        pub const GETTID: u64 = 178;
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
        let all = [
            WRITE,
            WRITEV,
            EXIT,
            EXIT_GROUP,
            BRK,
            MMAP,
            MUNMAP,
            MPROTECT,
            GETPID,
            GETPPID,
            GETTID,
            UNAME,
            SET_TID_ADDRESS,
            SET_ROBUST_LIST,
        ];
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

/// The most one `write` or `writev` transfers, as on Linux (`write(2)`: 0x7ffff000 bytes,
/// `INT_MAX` rounded down to a page); a larger count writes this much and returns it.
const MAX_RW_COUNT: u64 = 0x7fff_f000;

/// The most buffers one `writev` takes: Linux's limit (`IOV_MAX` in C).
const MAX_IOVECS: u64 = 1024;

/// How many of `writev`'s buffers it reads onto the kernel stack: C libraries pass two
/// (their buffer and the new data). More go through the heap.
const STACK_IOVECS: usize = 8;

/// The size of a `struct iovec`: a base address and a length.
const IOVEC_SIZE: usize = 16;

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
/// round trip when the memory calls' dispatch made it too large for the inliner). Called with
/// interrupts masked, no lock held and no address space active (outside
/// `AddressSpace::with_active` and `Process::with_user_memory`), so a call may block or pass a
/// preemption point.
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

/// Runs the system call the program made and sets its result. Always inlined, as [`handle`]:
/// as a call of its own, the loop's state spills around it (+8 to +10% on the system-call
/// round trip when D55's calls tipped the inliner).
#[inline(always)]
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
            _ => out_of_line_call::<P>(process, number, first, second, third),
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

/// Every call up to [`number::LAST`] but `write`, `exit` and the memory calls, with its first
/// three arguments: `ENOSYS` for a number with no call. One out-of-line function for all of
/// them, as [`memory_call`]: each call [`syscall`]'s own match grows by adds to the thread
/// loop that it is inlined into (the loop's registers spill, and every system call pays).
#[inline(never)]
fn out_of_line_call<P: Platform>(
    process: &Process<P>,
    number: u64,
    first: u64,
    second: u64,
    third: u64,
) -> Result<u64, Errno> {
    match number {
        number::WRITEV => writev::<P>(process, first, second, third),
        number::GETPID => Ok(identity::getpid(process)),
        number::GETPPID => Ok(identity::getppid()),
        number::GETTID => Ok(identity::gettid(process)),
        number::UNAME => identity::uname(process, first),
        number::SET_TID_ADDRESS => Ok(identity::set_tid_address(process, first)),
        number::SET_ROBUST_LIST => identity::set_robust_list(first, second),
        _ => Err(Errno::ENOSYS),
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
/// that part, or `EFAULT` if there are none, as Linux does. At most [`MAX_RW_COUNT`] bytes.
#[inline(never)]
fn write<P: Platform>(
    process: &Process<P>,
    fd: u64,
    buffer: u64,
    count: u64,
) -> Result<u64, Errno> {
    if !is_console(fd) {
        return Err(Errno::EBADF);
    }
    if !ends_in_user_space::<P>(buffer, count) {
        return Err(Errno::EFAULT);
    }
    // Linux checks the whole range first, then caps the count.
    let count = count.min(MAX_RW_COUNT);
    let written = write_to_console(process, buffer, count);
    if written == 0 && count > 0 {
        Err(Errno::EFAULT)
    } else {
        Ok(written)
    }
}

/// `writev(fd, buffers, count)`: as [`write`], for `count` buffers one after another, each a
/// `struct iovec` (base, length) in the program's memory at `buffers`. As Linux: `EINVAL` for
/// more than [`MAX_IOVECS`] buffers or a length that is negative as a `ssize_t`, `EFAULT` if
/// the array is not the program's memory or a buffer does not end within user space (all
/// checked before anything is written); then at most [`MAX_RW_COUNT`] bytes in all, stopping
/// at the first byte that is not the program's memory. The array is copied once, so the
/// program changing it meanwhile changes nothing.
#[inline(never)]
fn writev<P: Platform>(
    process: &Process<P>,
    fd: u64,
    buffers: u64,
    count: u64,
) -> Result<u64, Errno> {
    if !is_console(fd) {
        return Err(Errno::EBADF);
    }
    if count > MAX_IOVECS {
        return Err(Errno::EINVAL);
    }
    let bytes = count as usize * IOVEC_SIZE;
    let mut on_stack = [0u8; STACK_IOVECS * IOVEC_SIZE];
    let mut on_heap = alloc::vec::Vec::new();
    let array = if bytes <= on_stack.len() {
        &mut on_stack[..bytes]
    } else {
        on_heap
            .try_reserve_exact(bytes)
            .map_err(|_| Errno::ENOMEM)?;
        on_heap.resize(bytes, 0);
        &mut on_heap[..]
    };
    process
        .read_user_memory(buffers, array)
        .map_err(|_| Errno::EFAULT)?;
    let (entries, _) = array.as_chunks::<IOVEC_SIZE>();
    let iovec = |entry: &[u8; IOVEC_SIZE]| {
        let (base, length) = entry.split_at(8);
        (
            u64::from_ne_bytes(base.try_into().unwrap()),
            u64::from_ne_bytes(length.try_into().unwrap()),
        )
    };
    if entries.iter().any(|entry| iovec(entry).1 > i64::MAX as u64) {
        return Err(Errno::EINVAL);
    }
    if !entries.iter().all(|entry| {
        let (base, length) = iovec(entry);
        ends_in_user_space::<P>(base, length)
    }) {
        return Err(Errno::EFAULT);
    }
    let mut written = 0;
    for entry in entries {
        let (base, length) = iovec(entry);
        let length = length.min(MAX_RW_COUNT - written);
        let done = write_to_console(process, base, length);
        written += done;
        if done < length {
            return if written > 0 {
                Ok(written)
            } else {
                Err(Errno::EFAULT)
            };
        }
    }
    Ok(written)
}

/// Whether `fd` is standard output or standard error, the console's for now. `fd` is an
/// `unsigned int`: Linux ignores the register's upper half.
fn is_console(fd: u64) -> bool {
    matches!(fd as u32, 1 | 2)
}

/// As Linux's `access_ok`: whether `count` bytes from `buffer` end within user space (any
/// address does for none). A range that does not is `EFAULT` before anything is copied, even
/// if its start is the program's memory.
fn ends_in_user_space<P: Platform>(buffer: u64, count: u64) -> bool {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    count == 0 || buffer.checked_add(count).is_some_and(|end| end <= user_end)
}

/// Copies `count` bytes of the program's memory from `buffer`, a range that ends within user
/// space, to the console, and returns how many it copied: fewer if it reached a byte that is
/// not the program's memory. Chunks never cross a page, so the count is exact, and a
/// preemption point follows each, so a long write does not keep its CPU from the threads
/// waiting for it.
fn write_to_console<P: Platform>(process: &Process<P>, buffer: u64, count: u64) -> u64 {
    let mut chunk = [0u8; WRITE_CHUNK];
    let mut written = 0;
    while written < count {
        // Cannot wrap: the range ends within user space.
        let address = buffer.wrapping_add(written);
        let length = (count - written)
            .min(WRITE_CHUNK as u64)
            .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
        let bytes = &mut chunk[..length];
        if process.read_user_memory(address, bytes).is_err() {
            break;
        }
        P::console_write(bytes);
        written += length as u64;
        // System calls may pass one ([`handle`]).
        thread::preemption_point::<P>();
    }
    written
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

    /// A process with [`DATA`]'s first page readable, holding `contents` at its start, and the
    /// second writable, holding the `struct iovec`s for `buffers` (offsets into `contents` and
    /// lengths), as `writev` reads them.
    fn process_with_iovecs(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
        contents: &[u8],
        buffers: &[(u64, u64)],
    ) -> Arc<Process<P>> {
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        process
            .map_new_page(DATA, Attributes::USER_READ_ONLY, contents)
            .unwrap();
        let mut array = alloc::vec::Vec::new();
        for &(offset, length) in buffers {
            array.extend_from_slice(&(DATA + offset).to_ne_bytes());
            array.extend_from_slice(&length.to_ne_bytes());
        }
        process
            .map_new_page(DATA + 4096, Attributes::USER_READ_WRITE, &array)
            .unwrap();
        Arc::new(process)
    }

    fn call(process: &Arc<Process<P>>, number: u64, arguments: [u64; 6]) -> u64 {
        let mut context = called(number, arguments);
        assert_eq!(
            handle(process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        context.syscall_result()
    }

    #[test]
    fn writev_writes_each_buffer_in_turn() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let text = b"hello, writev\n";
        // Out of order, an empty one, and more than fit on the stack.
        let mut buffers = alloc::vec![(7, 6), (5, 2), (0, 5), (3, 0), (13, 1)];
        let process = process_with_iovecs(&kernel, text, &buffers);
        console_output();
        let writev = |count| call(&process, number::WRITEV, [1, DATA + 4096, count, 0, 0, 0]);
        assert_eq!(writev(5), 14);
        assert_eq!(console_output(), b"writev, hello\n");
        assert_eq!(writev(0), 0);
        assert!(console_output().is_empty());
        buffers.resize(STACK_IOVECS + 3, (0, 1));
        let process = process_with_iovecs(&kernel, text, &buffers);
        let count = buffers.len() as u64;
        assert_eq!(
            call(&process, number::WRITEV, [2, DATA + 4096, count, 0, 0, 0]),
            14 + 6
        );
        assert_eq!(console_output(), b"writev, hello\nhhhhhh");
    }

    #[test]
    fn writev_checks_every_buffer_before_writing() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        let writev = |buffers: &[(u64, u64)], fd| {
            let process = process_with_iovecs(&kernel, b"ab", buffers);
            let count = buffers.len() as u64;
            call(&process, number::WRITEV, [fd, DATA + 4096, count, 0, 0, 0])
        };
        console_output();
        assert_eq!(writev(&[(0, 2)], 0), Errno::EBADF.result());
        // A length negative as a `ssize_t`, even after a good buffer, or one past user space.
        assert_eq!(writev(&[(0, 2), (0, 1 << 63)], 1), Errno::EINVAL.result());
        assert_eq!(writev(&[(0, 2), (0, user_end)], 1), Errno::EFAULT.result());
        assert!(console_output().is_empty());
        // Past the readable page: what came before, else `EFAULT`.
        assert_eq!(writev(&[(0, 2), (4096 * 2, 2)], 1), 2);
        assert_eq!(writev(&[(4096 * 2, 2), (0, 2)], 1), Errno::EFAULT.result());
        assert_eq!(console_output(), b"ab");
        // Too many buffers, and an array that is not the program's.
        let process = process_with_iovecs(&kernel, b"ab", &[(0, 2)]);
        let too_many = MAX_IOVECS + 1;
        assert_eq!(
            call(
                &process,
                number::WRITEV,
                [1, DATA + 4096, too_many, 0, 0, 0]
            ),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(&process, number::WRITEV, [1, 0, 1, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert!(console_output().is_empty());
    }

    #[test]
    fn identity_calls_name_the_process_and_the_system() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let other = process_with_iovecs(&kernel, b"ab", &[]);
        let pid = call(&process, number::GETPID, [0; 6]);
        assert_eq!(pid, u64::from(process.id().get()));
        assert!(pid > 0);
        assert_ne!(call(&other, number::GETPID, [0; 6]), pid);
        assert_eq!(call(&process, number::GETTID, [0; 6]), pid);
        assert_eq!(call(&process, number::GETPPID, [0; 6]), 0);
        assert_eq!(
            call(&process, number::SET_TID_ADDRESS, [DATA, 0, 0, 0, 0, 0]),
            pid
        );
        assert_eq!(
            call(&process, number::SET_ROBUST_LIST, [DATA, 24, 0, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::SET_ROBUST_LIST, [DATA, 16, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        // `uname` fills the writable page; not the read-only one, nor across into the unmapped
        // one after it.
        assert_eq!(
            call(&process, number::UNAME, [DATA + 4096, 0, 0, 0, 0, 0]),
            0
        );
        let mut utsname = [0u8; 6 * identity::utsname::FIELD];
        process.read_user_memory(DATA + 4096, &mut utsname).unwrap();
        assert_eq!(&utsname[..6], b"Linux\0");
        let machine = 4 * identity::utsname::FIELD;
        assert!(utsname[machine..].starts_with(identity::utsname::MACHINE.as_bytes()));
        for buffer in [DATA, DATA + 8192 - 100] {
            assert_eq!(
                call(&process, number::UNAME, [buffer, 0, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
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
