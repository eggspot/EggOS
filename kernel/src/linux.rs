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
//! So far: `write` and `writev` to the standard descriptors (the console, D61), `exit`
//! and `exit_group`, the memory calls `brk`, `mmap` (anonymous), `munmap` and `mprotect`
//! ([`memory`], D53), `getpid`, `getppid`, `gettid`, `uname`, `set_tid_address` and
//! `set_robust_list` ([`identity`], D55), `getrandom` ([`random`], D56), and `sched_yield`
//! and, on x86_64, `arch_prctl` for the thread pointer ([`thread_pointer`], D57), and the
//! resource limits' `getrlimit`, `setrlimit` and `prlimit64` ([`limits`], D59), and the signal
//! state's `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` ([`signals`], D60), and, on the
//! console's descriptors 0 to 2, `poll` and `ppoll` ([`poll`]) and the terminal's `ioctl`s
//! ([`terminal`], D61). Every other number returns `ENOSYS`, as Linux does for a call it
//! lacks, so a program that probes for a call carries on. The next calls are the rest of what
//! a static program's startup needs (signal delivery, ...), then the file system's (M7).

pub mod exec;
pub mod identity;
pub mod limits;
pub mod memory;
pub mod poll;
pub mod random;
pub mod signals;
pub mod terminal;
pub mod thread_pointer;

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
        pub const POLL: u64 = 7;
        pub const MMAP: u64 = 9;
        pub const MPROTECT: u64 = 10;
        pub const MUNMAP: u64 = 11;
        pub const BRK: u64 = 12;
        pub const RT_SIGACTION: u64 = 13;
        pub const RT_SIGPROCMASK: u64 = 14;
        pub const IOCTL: u64 = 16;
        pub const WRITEV: u64 = 20;
        pub const SCHED_YIELD: u64 = 24;
        pub const GETPID: u64 = 39;
        pub const EXIT: u64 = 60;
        pub const UNAME: u64 = 63;
        pub const GETRLIMIT: u64 = 97;
        pub const GETPPID: u64 = 110;
        pub const SIGALTSTACK: u64 = 131;
        pub const ARCH_PRCTL: u64 = 158;
        pub const SETRLIMIT: u64 = 160;
        pub const GETTID: u64 = 186;
        pub const SET_TID_ADDRESS: u64 = 218;
        pub const EXIT_GROUP: u64 = 231;
        pub const PPOLL: u64 = 271;
        pub const SET_ROBUST_LIST: u64 = 273;
        pub const PRLIMIT64: u64 = 302;
        pub const GETRANDOM: u64 = 318;
        /// The highest number above.
        pub const LAST: u64 = GETRANDOM;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
        pub const IOCTL: u64 = 29;
        pub const PPOLL: u64 = 73;
        pub const WRITE: u64 = 64;
        pub const WRITEV: u64 = 66;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
        pub const SET_TID_ADDRESS: u64 = 96;
        pub const SET_ROBUST_LIST: u64 = 99;
        pub const SCHED_YIELD: u64 = 124;
        pub const SIGALTSTACK: u64 = 132;
        pub const RT_SIGACTION: u64 = 134;
        pub const RT_SIGPROCMASK: u64 = 135;
        pub const UNAME: u64 = 160;
        pub const GETRLIMIT: u64 = 163;
        pub const SETRLIMIT: u64 = 164;
        pub const GETPID: u64 = 172;
        pub const GETPPID: u64 = 173;
        pub const GETTID: u64 = 178;
        pub const BRK: u64 = 214;
        pub const MUNMAP: u64 = 215;
        pub const MMAP: u64 = 222;
        pub const MPROTECT: u64 = 226;
        pub const PRLIMIT64: u64 = 261;
        pub const GETRANDOM: u64 = 278;
        /// The highest number above.
        pub const LAST: u64 = GETRANDOM;
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
            SCHED_YIELD,
            #[cfg(target_arch = "x86_64")]
            ARCH_PRCTL,
            GETRANDOM,
            GETRLIMIT,
            SETRLIMIT,
            PRLIMIT64,
            RT_SIGACTION,
            RT_SIGPROCMASK,
            SIGALTSTACK,
            #[cfg(target_arch = "x86_64")]
            POLL,
            PPOLL,
            IOCTL,
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
    /// Not permitted: here, a mapping below the lowest address allowed (`execve`, `mmap`), a
    /// hard resource limit raised (`setrlimit`, `prlimit64`), or the alternate signal stack
    /// changed while the thread runs on it (`sigaltstack`).
    pub const EPERM: Self = Self(1);
    /// No such file: here, a program's interpreter (`execve`).
    pub const ENOENT: Self = Self(2);
    /// No such process: here, one the caller cannot reach (`prlimit64`).
    pub const ESRCH: Self = Self(3);
    /// The arguments and environment are too long (`execve`).
    pub const E2BIG: Self = Self(7);
    /// Not a program the kernel can run (`execve`).
    pub const ENOEXEC: Self = Self(8);
    /// Bad file descriptor.
    pub const EBADF: Self = Self(9);
    /// Try again: here, the random number generator is not seeded yet (`getrandom` with
    /// `GRND_NONBLOCK`).
    pub const EAGAIN: Self = Self(11);
    /// Out of memory.
    pub const ENOMEM: Self = Self(12);
    /// Permission denied: here, memory both writable and executable (`mmap`, `mprotect`).
    pub const EACCES: Self = Self(13);
    /// Bad address: a buffer that is not the program's memory.
    pub const EFAULT: Self = Self(14);
    /// Exists already: here, a mapping in the way of `MAP_FIXED_NOREPLACE`.
    pub const EEXIST: Self = Self(17);
    /// No such device: here, a CPU feature the call needs is missing.
    pub const ENODEV: Self = Self(19);
    /// An invalid argument: here, a segment outside user space (`execve`), an unaligned
    /// address or an empty range (the memory calls), a bad signal number, set size or flag
    /// (the signal calls), or a bad timeout or too many descriptors (`ppoll`, `poll`).
    pub const EINVAL: Self = Self(22);
    /// Not a terminal: here, an `ioctl` request the console does not answer.
    pub const ENOTTY: Self = Self(25);
    /// No such system call.
    pub const ENOSYS: Self = Self(38);
    /// Not supported: here, an unknown flag with `MAP_SHARED_VALIDATE` (`mmap`).
    pub const EOPNOTSUPP: Self = Self(95);

    /// The value the program sees in its result register: `-errno`.
    pub fn result(self) -> u64 {
        (-i64::from(self.0)) as u64
    }
}

/// Linux's signal numbers, as far as the kernel sends or checks them (the generic ones, which
/// AArch64, RISC-V and x86_64 share).
pub mod signal {
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGBUS: u32 = 7;
    pub const SIGKILL: u32 = 9;
    pub const SIGSEGV: u32 = 11;
    pub const SIGSTOP: u32 = 19;
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
            _ => out_of_line_call::<P>(process, number, context),
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

/// Every call up to [`number::LAST`] but `write`, `exit` and the memory calls, with the
/// program's registers: `ENOSYS` for a number with no call. One out-of-line function for all
/// of them, as [`memory_call`]: each call [`syscall`]'s own match grows by adds to the thread
/// loop that it is inlined into (the loop's registers spill, and every system call pays).
#[inline(never)]
fn out_of_line_call<P: Platform>(
    process: &Process<P>,
    number: u64,
    context: &mut UserContext,
) -> Result<u64, Errno> {
    let [first, second, third, fourth, fifth, _] = context.syscall_arguments();
    match number {
        number::WRITEV => writev::<P>(process, first, second, third),
        number::GETPID => Ok(identity::getpid(process)),
        number::GETPPID => Ok(identity::getppid()),
        number::GETTID => Ok(identity::gettid(process)),
        number::UNAME => identity::uname(process, first),
        number::SET_TID_ADDRESS => Ok(identity::set_tid_address(process, first)),
        number::SET_ROBUST_LIST => identity::set_robust_list(first, second),
        number::GETRANDOM => random::getrandom::<P>(process, first, second, third),
        number::GETRLIMIT => limits::getrlimit(process, first, second),
        number::SETRLIMIT => limits::setrlimit(process, first, second),
        number::PRLIMIT64 => limits::prlimit64(process, first, second, third, fourth),
        number::RT_SIGACTION => signals::rt_sigaction(process, first, second, third, fourth),
        number::RT_SIGPROCMASK => signals::rt_sigprocmask(process, first, second, third, fourth),
        number::SIGALTSTACK => signals::sigaltstack(process, first, second, context.stack()),
        #[cfg(target_arch = "x86_64")]
        number::POLL => poll::poll::<P>(process, first, second, third),
        number::PPOLL => poll::ppoll::<P>(process, first, second, third, fourth, fifth),
        number::IOCTL => terminal::ioctl::<P>(process, first, second, third),
        number::SCHED_YIELD => {
            thread::yield_now::<P>();
            Ok(0)
        }
        #[cfg(target_arch = "x86_64")]
        number::ARCH_PRCTL => thread_pointer::arch_prctl::<P>(process, context, first, second),
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

/// `write(fd, buffer, count)`: the standard descriptors go to the console. Returns
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
/// `struct iovec` (base, length) in the program's memory at `buffers`. As Linux, all before
/// anything is written: `EINVAL` for more than [`MAX_IOVECS`] buffers; `EFAULT` if the array
/// does not end within user space; then, entry by entry, `EFAULT` for one that is not the
/// program's memory and `EINVAL` for a length that is
/// negative as a `ssize_t` (see [`unreadable_iovecs_error`]); then `EFAULT` if a buffer does
/// not end within user space; then at most [`MAX_RW_COUNT`] bytes in all, stopping at the
/// first byte that is not the program's memory. The array is copied once, so the program
/// changing it meanwhile changes nothing. `count` is an `unsigned int`, as `fd` is: the
/// register's upper half is ignored.
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
    let count = u64::from(count as u32);
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
    if process.read_user_memory(buffers, array).is_err() {
        return Err(unreadable_iovecs_error(process, buffers, count));
    }
    let (entries, _) = array.as_chunks::<IOVEC_SIZE>();
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

/// A `struct iovec`'s base and length.
fn iovec(entry: &[u8; IOVEC_SIZE]) -> (u64, u64) {
    let (base, length) = entry.split_at(8);
    (
        u64::from_ne_bytes(base.try_into().unwrap()),
        u64::from_ne_bytes(length.try_into().unwrap()),
    )
}

/// [`writev`]'s error when its array of `count` `struct iovec`s at `buffers` is not all the
/// program's memory. As Linux reads the entries one at a time: `EFAULT` if the array does not
/// end within user space, before any is read; otherwise `EINVAL` if an entry read before the
/// first unreadable one has a negative length, `EFAULT` if none has (D62). Cold: a program
/// passes a readable array.
#[cold]
#[inline(never)]
fn unreadable_iovecs_error<P: Platform>(process: &Process<P>, buffers: u64, count: u64) -> Errno {
    if !ends_in_user_space::<P>(buffers, count * IOVEC_SIZE as u64) {
        return Errno::EFAULT;
    }
    for index in 0..count {
        let mut entry = [0; IOVEC_SIZE];
        // Within user space, checked above: no overflow.
        let address = buffers + index * IOVEC_SIZE as u64;
        if process.read_user_memory(address, &mut entry).is_err() {
            break;
        }
        if iovec(&entry).1 > i64::MAX as u64 {
            return Errno::EINVAL;
        }
    }
    Errno::EFAULT
}

/// Whether `fd` is one of the standard descriptors, 0 to 2, the only ones so far: the
/// console, open for reading and writing, as Linux starts `init` with `/dev/console` on all
/// three (D61). `fd` is an `unsigned int`: Linux ignores the register's upper half.
fn is_console(fd: u64) -> bool {
    fd as u32 <= 2
}

/// As Linux's `access_ok`: whether `count` bytes from `buffer` end within user space. For
/// none, whether `buffer` is at most the end of user space: a kernel address is `EFAULT`
/// even with nothing to copy, as on Linux (D58). A range that does not is `EFAULT` before
/// anything is copied, even if its start is the program's memory.
fn ends_in_user_space<P: Platform>(buffer: u64, count: u64) -> bool {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    buffer.checked_add(count).is_some_and(|end| end <= user_end)
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
        // Standard input too: the console is open for reading and writing (D61).
        for fd in [0, 1, 2] {
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
        // Nothing to write: any address up to the end of user space, unmapped or not; past
        // it, `EFAULT`, as on Linux.
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(write(1, 0, 0), 0);
        assert_eq!(write(1, user_end, 0), 0);
        for buffer in [user_end + 1, 0xffff_ffff_8000_0000, u64::MAX] {
            assert_eq!(write(1, buffer, 0), Errno::EFAULT.result(), "{buffer:#x}");
        }
        // A range that does not end within user space: nothing is written.
        assert_eq!(write(1, DATA + 4095, user_end), Errno::EFAULT.result());
        // Only the standard descriptors so far, and only the descriptor's low 32 bits count,
        // as on Linux.
        assert_eq!(write(3, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(1 << 32 | 3, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(u64::MAX, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(1 << 32 | 1, DATA + 4095, 0), 0);
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
    fn getrandom_fills_the_program_s_writable_memory() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        crate::random::add_seed(eggos_hal::random::SeedSource::CpuInstruction, &[7; 32]);
        assert!(crate::random::reseed::<P>());
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let getrandom = |buffer, length, flags| {
            call(
                &process,
                number::GETRANDOM,
                [buffer, length, flags, 0, 0, 0],
            )
        };
        let writable = DATA + 4096;
        let read = |offset, length| {
            let mut bytes = vec![0u8; length];
            process
                .read_user_memory(writable + offset, &mut bytes)
                .unwrap();
            bytes
        };
        assert_eq!(getrandom(writable, 16, 0), 16);
        let first = read(0, 16);
        assert_eq!(
            getrandom(writable, 16, u64::from(random::flags::INSECURE)),
            16
        );
        assert_ne!(read(0, 16), first);
        // A long call, with the flags' upper half ignored: every byte is written, and the
        // bytes after are not.
        assert_eq!(getrandom(writable, 1000, 1 << 32), 1000);
        assert!(read(0, 1000).chunks(16).all(|chunk| chunk != [0; 16]));
        assert_eq!(read(1000, 16), [0; 16]);
        // No bytes: 0 within user space, mapped or not; `EFAULT` past it.
        assert_eq!(getrandom(0, 0, 0), 0);
        assert_eq!(getrandom(u64::MAX, 0, 0), Errno::EFAULT.result());
        // Up to the end of the writable page, then unmapped memory: the bytes before.
        assert_eq!(getrandom(writable + 4096 - 100, 200, 0), 100);
        // Read-only memory, or a range past user space: EFAULT.
        assert_eq!(getrandom(DATA, 16, 0), Errno::EFAULT.result());
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(getrandom(user_end - 8, 16, 0), Errno::EFAULT.result());
        assert_eq!(getrandom(writable, 16, 8), Errno::EINVAL.result());
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
        assert_eq!(writev(&[(0, 2)], 3), Errno::EBADF.result());
        // A length negative as a `ssize_t`, even after a good buffer, or one past user space.
        assert_eq!(writev(&[(0, 2), (0, 1 << 63)], 1), Errno::EINVAL.result());
        assert_eq!(writev(&[(0, 2), (0, user_end)], 1), Errno::EFAULT.result());
        // An empty buffer at a kernel address fails the whole call too, as on Linux.
        let kernel_address = 0xffff_ffff_8000_0000u64.wrapping_sub(DATA);
        assert_eq!(
            writev(&[(0, 2), (kernel_address, 0)], 1),
            Errno::EFAULT.result()
        );
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
        // An array that runs off its page: its entries are read in order, so a negative
        // length before the unreadable one is `EINVAL`, as on Linux (D62); none, `EFAULT`.
        let last_entry = DATA + 2 * 4096 - IOVEC_SIZE as u64;
        for (length, error) in [(1 << 63, Errno::EINVAL), (2, Errno::EFAULT)] {
            let mut buffers = alloc::vec![(0, 2); 4096 / IOVEC_SIZE];
            *buffers.last_mut().unwrap() = (0, length);
            let full_page = process_with_iovecs(&kernel, b"ab", &buffers);
            assert_eq!(
                call(&full_page, number::WRITEV, [1, last_entry, 2, 0, 0, 0]),
                error.result()
            );
            // Only the unreadable entry: `EFAULT`.
            assert_eq!(
                call(&full_page, number::WRITEV, [1, last_entry + 16, 1, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
        // An array that does not end within user space: `EFAULT` before any entry is read,
        // even a readable one with a negative length in user space's last page.
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut at_the_end = unsafe { Process::<P>::new(&kernel) }.unwrap();
        let mut entry = [0u8; IOVEC_SIZE];
        entry[8..].copy_from_slice(&(1u64 << 63).to_ne_bytes());
        let last_page = user_end - 4096;
        let mut page = alloc::vec![0u8; 4096];
        page[4096 - IOVEC_SIZE..].copy_from_slice(&entry);
        at_the_end
            .map_new_page(last_page, Attributes::USER_READ_ONLY, &page)
            .unwrap();
        let at_the_end = Arc::new(at_the_end);
        let last_entry = user_end - IOVEC_SIZE as u64;
        assert_eq!(
            call(&at_the_end, number::WRITEV, [1, last_entry, 1, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(&at_the_end, number::WRITEV, [1, last_entry, 2, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert!(console_output().is_empty());
        // The count's upper half is ignored: none, then one buffer.
        for (count, written) in [(1 << 32, 0), (1 << 32 | 1, 2)] {
            assert_eq!(
                call(&process, number::WRITEV, [1, DATA + 4096, count, 0, 0, 0]),
                written
            );
        }
        assert_eq!(console_output(), b"ab");
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
    fn resource_limits_are_read_and_lowered_through_user_memory() {
        use limits::{Limit, resource};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let writable = DATA + 4096;
        let read = |address| {
            let mut words = [0u8; 16];
            process.read_user_memory(address, &mut words).unwrap();
            let (soft, hard) = words.split_at(8);
            Limit {
                soft: u64::from_ne_bytes(soft.try_into().unwrap()),
                hard: u64::from_ne_bytes(hard.try_into().unwrap()),
            }
        };
        let store = |address, limit: Limit| {
            let mut words = [0u8; 16];
            words[..8].copy_from_slice(&limit.soft.to_ne_bytes());
            words[8..].copy_from_slice(&limit.hard.to_ne_bytes());
            process.write_user_memory(address, &words).unwrap();
        };
        let [stack, nofile] = [resource::STACK, resource::NOFILE].map(u64::from);
        let pid = u64::from(process.id().get());
        assert_eq!(
            call(&process, number::GETRLIMIT, [stack, writable, 0, 0, 0, 0]),
            0
        );
        assert_eq!(read(writable), limits::DEFAULTS[resource::STACK as usize]);
        // Lower the open-file limit with `setrlimit`, reading the old one with `prlimit64`.
        let lowered = Limit {
            soft: 256,
            hard: 512,
        };
        store(writable + 32, lowered);
        assert_eq!(
            call(
                &process,
                number::SETRLIMIT,
                [nofile, writable + 32, 0, 0, 0, 0]
            ),
            0
        );
        // pid 0 or the caller's own, with only the low 32 bits of it and of the resource.
        for (pid, resource) in [
            (0, nofile),
            (pid, 1 << 32 | nofile),
            (1 << 32 | pid, nofile),
        ] {
            assert_eq!(
                call(
                    &process,
                    number::PRLIMIT64,
                    [pid, resource, 0, writable + 64, 0, 0]
                ),
                0
            );
            assert_eq!(read(writable + 64), lowered);
        }
        // Set and read the old limits in one call: what it had.
        store(
            writable + 32,
            Limit {
                soft: 128,
                hard: 512,
            },
        );
        assert_eq!(
            call(
                &process,
                number::PRLIMIT64,
                [0, nofile, writable + 32, writable + 96, 0, 0]
            ),
            0
        );
        assert_eq!(read(writable + 96), lowered);
        // The hard limit cannot rise again; the soft limit may not pass it.
        store(
            writable + 32,
            Limit {
                soft: 128,
                hard: 513,
            },
        );
        assert_eq!(
            call(
                &process,
                number::SETRLIMIT,
                [nofile, writable + 32, 0, 0, 0, 0]
            ),
            Errno::EPERM.result()
        );
        store(
            writable + 32,
            Limit {
                soft: 513,
                hard: 512,
            },
        );
        assert_eq!(
            call(
                &process,
                number::SETRLIMIT,
                [nofile, writable + 32, 0, 0, 0, 0]
            ),
            Errno::EINVAL.result()
        );
        // Another process, an unknown resource, memory that is not the program's (or not
        // writable): nothing changes.
        store(
            writable + 32,
            Limit {
                soft: 64,
                hard: 256,
            },
        );
        assert_eq!(
            call(
                &process,
                number::PRLIMIT64,
                [pid + 1, nofile, writable + 32, writable, 0, 0]
            ),
            Errno::ESRCH.result()
        );
        assert_eq!(read(writable), limits::DEFAULTS[resource::STACK as usize]);
        for resource in [resource::COUNT as u64, u64::from(u32::MAX)] {
            assert_eq!(
                call(
                    &process,
                    number::GETRLIMIT,
                    [resource, writable, 0, 0, 0, 0]
                ),
                Errno::EINVAL.result()
            );
        }
        for address in [0, DATA, writable + 4096 - 8] {
            assert_eq!(
                call(&process, number::GETRLIMIT, [nofile, address, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
        assert_eq!(
            call(
                &process,
                number::SETRLIMIT,
                [nofile, writable + 8192, 0, 0, 0, 0]
            ),
            Errno::EFAULT.result()
        );
        assert_eq!(
            process.resource_limits().lock().get(resource::NOFILE),
            Some(Limit {
                soft: 128,
                hard: 512
            })
        );
        // An old limit that cannot be copied out is `EFAULT`, with the new one set, as Linux.
        assert_eq!(
            call(
                &process,
                number::PRLIMIT64,
                [0, nofile, writable + 32, DATA, 0, 0]
            ),
            Errno::EFAULT.result()
        );
        assert_eq!(
            process.resource_limits().lock().get(resource::NOFILE),
            Some(Limit {
                soft: 64,
                hard: 256
            })
        );
        // `setrlimit` and `getrlimit` take no null pointer; `prlimit64` takes two, and only
        // checks.
        for number in [number::GETRLIMIT, number::SETRLIMIT] {
            assert_eq!(
                call(&process, number, [nofile, 0, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
        assert_eq!(
            call(&process, number::PRLIMIT64, [0, nofile, 0, 0, 0, 0]),
            0
        );
        // Another process starts from the defaults.
        let other = process_with_iovecs(&kernel, b"ab", &[]);
        assert_eq!(
            other.resource_limits().lock().get(resource::NOFILE),
            Some(limits::DEFAULTS[resource::NOFILE as usize])
        );
    }

    #[test]
    fn signal_state_is_kept_through_user_memory() {
        use signals::{Action, IGNORE, flag};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let writable = DATA + 4096;
        let read_only = DATA;
        let unmapped = DATA + 2 * 4096;
        let words = |address, count: usize| {
            let mut bytes = alloc::vec![0u8; count * 8];
            process.read_user_memory(address, &mut bytes).unwrap();
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|&word| u64::from_ne_bytes(word))
                .collect::<alloc::vec::Vec<_>>()
        };
        let store = |address, words: &[u64]| {
            let bytes: alloc::vec::Vec<u8> =
                words.iter().flat_map(|word| word.to_ne_bytes()).collect();
            process.write_user_memory(address, &bytes).unwrap();
        };
        let action = |address| {
            let mut bytes = [0u8; 32];
            process.read_user_memory(address, &mut bytes).unwrap();
            let word = |i: usize| u64::from_ne_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
            if cfg!(target_arch = "riscv64") {
                Action {
                    handler: word(0),
                    flags: word(1),
                    restorer: 0,
                    mask: word(2),
                }
            } else {
                Action {
                    handler: word(0),
                    flags: word(1),
                    restorer: word(2),
                    mask: word(3),
                }
            }
        };
        let (sigpipe, sigsegv) = (13, 11);
        let [sigkill, sigstop] = [signal::SIGKILL, signal::SIGSTOP].map(u64::from);
        let einval = Errno::EINVAL.result();
        let efault = Errno::EFAULT.result();

        // `rt_sigaction`: `SIGPIPE` ignored (the upper half of the number ignored), read back.
        store(writable, &[IGNORE, 0, 0, 0]);
        let sigaction = |arguments: [u64; 4]| {
            let [signal, new, old, size] = arguments;
            call(
                &process,
                number::RT_SIGACTION,
                [signal, new, old, size, 0, 0],
            )
        };
        assert_eq!(sigaction([1 << 32 | sigpipe, writable, 0, 8]), 0);
        assert_eq!(sigaction([sigpipe, 0, writable + 64, 8]), 0);
        assert_eq!(
            action(writable + 64),
            Action {
                handler: IGNORE,
                ..Action::default()
            }
        );
        // A handler for `SIGSEGV`: unknown flags cleared, `SIGKILL` and `SIGSTOP` out of its
        // mask, and the old action reported in the same call.
        store(
            writable,
            &if cfg!(target_arch = "riscv64") {
                [
                    0x40_1000,
                    flag::SIGINFO | flag::ONSTACK | flag::UNSUPPORTED,
                    u64::MAX,
                    0,
                ]
            } else {
                [
                    0x40_1000,
                    flag::SIGINFO | flag::ONSTACK | flag::UNSUPPORTED,
                    0,
                    u64::MAX,
                ]
            },
        );
        assert_eq!(sigaction([sigsegv, writable, writable + 64, 8]), 0);
        assert_eq!(action(writable + 64), Action::default());
        assert_eq!(sigaction([sigsegv, 0, writable + 64, 8]), 0);
        let installed = action(writable + 64);
        assert_eq!(installed.handler, 0x40_1000);
        assert_eq!(installed.flags, flag::SIGINFO | flag::ONSTACK);
        assert_eq!(installed.mask, !(1 << (sigkill - 1) | 1 << (sigstop - 1)));
        // Errors: the set size, numbers outside 1..=64, a new action for `SIGKILL` or
        // `SIGSTOP` (theirs can be read), and buffers that are not the program's.
        assert_eq!(sigaction([sigpipe, 0, writable + 64, 4]), einval);
        assert_eq!(sigaction([sigpipe, 0, writable + 64, 1 << 32 | 8]), einval);
        for signal in [0, 65] {
            assert_eq!(sigaction([signal, 0, writable + 64, 8]), einval);
        }
        for signal in [sigkill, sigstop] {
            assert_eq!(sigaction([signal, writable, 0, 8]), einval);
            assert_eq!(sigaction([signal, 0, writable + 64, 8]), 0);
            assert_eq!(action(writable + 64), Action::default());
        }
        assert_eq!(sigaction([sigpipe, unmapped, 0, 8]), efault);
        // A bad new action wins over a bad number, as on Linux.
        assert_eq!(sigaction([0, unmapped, 0, 8]), efault);
        // The action is read from read-only memory, but the old one cannot be written there:
        // `EFAULT`, with the new action kept.
        store(writable, &[0; 4]);
        assert_eq!(sigaction([sigpipe, read_only, read_only, 8]), efault);
        assert_eq!(sigaction([sigpipe, 0, writable + 64, 8]), 0);
        assert_eq!(
            action(writable + 64).handler,
            u64::from_ne_bytes(*b"ab\0\0\0\0\0\0")
        );

        // `rt_sigprocmask`: block, unblock and set, `SIGKILL` and `SIGSTOP` never blocked.
        let sigprocmask = |how, set, old, size| {
            call(
                &process,
                number::RT_SIGPROCMASK,
                [how, set, old, size, 0, 0],
            )
        };
        let mask = |signals: &[u64]| signals.iter().fold(0u64, |set, s| set | 1 << (s - 1));
        store(writable, &[mask(&[sigpipe, sigkill, sigstop])]);
        assert_eq!(sigprocmask(0, writable, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [0]);
        store(writable, &[mask(&[sigsegv])]);
        assert_eq!(sigprocmask(1 << 32, writable, 0, 8), 0);
        assert_eq!(sigprocmask(7, 0, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [mask(&[sigpipe, sigsegv])]);
        store(writable, &[mask(&[sigpipe])]);
        assert_eq!(sigprocmask(1, writable, 0, 8), 0);
        assert_eq!(sigprocmask(0, 0, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [mask(&[sigsegv])]);
        store(writable, &[u64::MAX]);
        assert_eq!(sigprocmask(2, writable, 0, 8), 0);
        assert_eq!(sigprocmask(0, 0, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [!mask(&[sigkill, sigstop])]);
        // Errors: the set size, an unknown `how` with a set, a set that is not the
        // program's, and an old set that cannot be written (the change kept).
        assert_eq!(sigprocmask(0, writable, 0, 16), einval);
        assert_eq!(sigprocmask(3, writable, 0, 8), einval);
        assert_eq!(sigprocmask(3, unmapped, 0, 8), efault);
        store(writable, &[0]);
        assert_eq!(sigprocmask(2, writable, read_only, 8), efault);
        assert_eq!(sigprocmask(0, 0, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [0]);

        // `sigaltstack`: none at first, then one, reported as in use when the stack pointer
        // is on it, where it cannot change.
        let sigaltstack = |new, old, stack| {
            let mut context = UserContext::new(0x40_0000, stack);
            context.simulate_syscall(number::SIGALTSTACK, [new, old, 0, 0, 0, 0], 0x40_0004);
            assert_eq!(
                handle(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            context.syscall_result()
        };
        let flags = |words: &[u64]| words[1] as u32;
        assert_eq!(sigaltstack(0, writable + 64, 0x7fff_0000), 0);
        assert_eq!(words(writable + 64, 3), [0, 2, 0]);
        let (base, size) = (0x7000_0000, 0x4000);
        store(writable, &[base, 0, signals::MIN_STACK_SIZE - 1]);
        assert_eq!(sigaltstack(writable, 0, 0), Errno::ENOMEM.result());
        store(writable, &[base, 0, signals::MIN_STACK_SIZE]);
        assert_eq!(sigaltstack(writable, 0, 0), 0);
        store(writable, &[0, 2, 0]);
        assert_eq!(sigaltstack(writable, 0, 0), 0);
        for bad in [4, 3, 1 << 30] {
            store(writable, &[base, bad, size]);
            assert_eq!(sigaltstack(writable, 0, 0), einval);
        }
        store(writable, &[base, 1, size]);
        assert_eq!(sigaltstack(writable, writable + 64, 0x7fff_0000), 0);
        assert_eq!(flags(&words(writable + 64, 3)), 2);
        assert_eq!(sigaltstack(0, writable + 64, base + 0x100), 0);
        assert_eq!(words(writable + 64, 3), [base, 1, size]);
        assert_eq!(sigaltstack(0, writable + 64, base + size), 0);
        assert_eq!(words(writable + 64, 3), [base, 1, size]);
        assert_eq!(sigaltstack(0, writable + 64, base), 0);
        assert_eq!(words(writable + 64, 3), [base, 0, size]);
        store(writable, &[0, 2, 0]);
        assert_eq!(
            sigaltstack(writable, 0, base + 0x100),
            Errno::EPERM.result()
        );
        assert_eq!(sigaltstack(unmapped, 0, 0), efault);
        // Disabled, with `SS_AUTODISARM` kept and reported; no base or size remains.
        store(writable, &[base, 2 | 1 << 31, size]);
        assert_eq!(sigaltstack(writable, read_only, 0), efault);
        assert_eq!(sigaltstack(0, writable + 64, 0), 0);
        assert_eq!(words(writable + 64, 3), [0, 2 | 1 << 31, 0]);

        // Another process starts with the defaults.
        let other = process_with_iovecs(&kernel, b"ab", &[]);
        assert_eq!(other.signals().lock().action(13), Some(Action::default()));
        assert_eq!(other.signals().lock().thread(), Default::default());
    }

    /// `struct pollfd`s for `entries` (descriptor, requested events), as the program passes
    /// them, with garbage where the kernel writes the returned events.
    fn pollfds(entries: &[(i32, u16)]) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec::Vec::new();
        for &(fd, requested) in entries {
            bytes.extend_from_slice(&fd.to_ne_bytes());
            bytes.extend_from_slice(&requested.to_ne_bytes());
            bytes.extend_from_slice(&0xaaaa_u16.to_ne_bytes());
        }
        bytes
    }

    #[test]
    fn ppoll_reports_the_console_s_descriptors_ready_to_write() {
        use poll::event::{IN, NVAL, OUT, WRNORM};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // The read-only page: a `struct pollfd`, and at 1024 a timeout of one second.
        let mut contents = pollfds(&[(1, OUT)]);
        contents.resize(1024, 0);
        contents.extend_from_slice(&1i64.to_ne_bytes());
        let process = process_with_iovecs(&kernel, &contents, &[]);
        let read_only = DATA;
        let writable = DATA + 4096;
        let timeout = writable + 2048;
        let mask = writable + 2064;
        let returned = |count: usize| {
            let mut bytes = alloc::vec![0u8; count * 8];
            process.read_user_memory(writable, &mut bytes).unwrap();
            bytes
                .chunks(8)
                .map(|entry| u16::from_ne_bytes(entry[6..].try_into().unwrap()))
                .collect::<alloc::vec::Vec<_>>()
        };
        let set_timeout = |seconds: i64, nanoseconds: i64| {
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&seconds.to_ne_bytes());
            bytes[8..].copy_from_slice(&nanoseconds.to_ne_bytes());
            process.write_user_memory(timeout, &bytes).unwrap();
        };
        let ppoll = |fds, count, timeout, mask, set_size| {
            call(
                &process,
                number::PPOLL,
                [fds, count, timeout, mask, set_size, 0],
            )
        };
        set_timeout(0, 0);
        process.write_user_memory(mask, &[0xff; 8]).unwrap();
        // Rust's std at startup: descriptors 0 to 2, no events asked for, no wait.
        let entries = pollfds(&[(0, 0), (1, 0), (2, 0)]);
        process.write_user_memory(writable, &entries).unwrap();
        assert_eq!(ppoll(writable, 3, timeout, 0, 0), 0);
        assert_eq!(returned(3), [0, 0, 0]);
        // Room to write on the console, never data to read; `POLLNVAL` for a closed
        // descriptor, whatever was asked; a negative one skipped. The count's upper half is
        // ignored, and so is the size of a signal set that is not there.
        let entries = pollfds(&[
            (0, IN),
            (1, OUT),
            (2, IN | OUT | WRNORM),
            (3, IN),
            (-1, OUT),
        ]);
        process.write_user_memory(writable, &entries).unwrap();
        assert_eq!(ppoll(writable, 1 << 32 | 5, timeout, 0, 4), 3);
        assert_eq!(returned(5), [0, OUT, OUT | WRNORM, NVAL, 0]);
        // A signal mask is read, then changes nothing (no signal is delivered yet).
        assert_eq!(ppoll(writable, 5, timeout, mask, 8), 3);
        // More than fit on the kernel stack.
        let many = alloc::vec![(1, OUT); 20];
        process
            .write_user_memory(writable, &pollfds(&many))
            .unwrap();
        assert_eq!(ppoll(writable, 20, timeout, 0, 0), 20);
        assert_eq!(returned(20), [OUT; 20]);
        // No descriptors and no wait, with any address up to the end of user space.
        assert_eq!(ppoll(0, 0, timeout, 0, 0), 0);
        #[cfg(target_arch = "x86_64")]
        {
            let entries = pollfds(&[(2, OUT), (7, 0)]);
            process.write_user_memory(writable, &entries).unwrap();
            assert_eq!(call(&process, number::POLL, [writable, 2, 0, 0, 0, 0]), 2);
            assert_eq!(returned(2), [OUT, NVAL]);
            // The timeout is an `int`: the upper half is ignored, so nothing ready means no
            // wait here, not 2^32 milliseconds.
            process
                .write_user_memory(writable, &pollfds(&[(0, IN)]))
                .unwrap();
            assert_eq!(
                call(&process, number::POLL, [writable, 1, 1 << 32, 0, 0, 0]),
                0
            );
            assert_eq!(returned(1), [0]);
        }

        // The errors, in Linux's order: the timeout's address, then its value, then the
        // signal set's size and address, then the count, then the array.
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        let unmapped = DATA + 3 * 4096;
        let efault = Errno::EFAULT.result();
        let einval = Errno::EINVAL.result();
        assert_eq!(ppoll(writable, 1, unmapped, 4, 4), efault);
        for (seconds, nanoseconds) in [(-1, 0), (0, -1), (0, 1_000_000_000), (i64::MIN, 0)] {
            set_timeout(seconds, nanoseconds);
            assert_eq!(
                ppoll(unmapped, 1, timeout, 4, 4),
                einval,
                "{seconds} {nanoseconds}"
            );
        }
        set_timeout(0, 0);
        assert_eq!(ppoll(unmapped, 1, timeout, mask, 4), einval);
        assert_eq!(ppoll(unmapped, 1, timeout, unmapped, 8), efault);
        assert_eq!(ppoll(unmapped, 1025, timeout, 0, 0), einval);
        // Unmapped, past the writable page, past user space, read-only: `EFAULT`.
        assert_eq!(ppoll(unmapped, 1, timeout, 0, 0), efault);
        assert_eq!(ppoll(writable, 1024, timeout, 0, 0), efault);
        assert_eq!(ppoll(user_end + 1, 0, timeout, 0, 0), efault);
        assert_eq!(ppoll(user_end - 4, 1, timeout, 0, 0), efault);
        assert_eq!(ppoll(read_only, 1, timeout, 0, 0), efault);
        // Ready at once: the time not slept is written back, a little less than all of it
        // (the test clock moves a microsecond each read), as Linux does (D62).
        process
            .write_user_memory(writable, &pollfds(&[(1, OUT)]))
            .unwrap();
        let read_timeout = || {
            let mut bytes = [0u8; 16];
            process.read_user_memory(timeout, &mut bytes).unwrap();
            let word = |at: usize| i64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap());
            (word(0), word(8))
        };
        set_timeout(5, 0);
        assert_eq!(ppoll(writable, 1, timeout, 0, 0), 1);
        let (seconds, nanoseconds) = read_timeout();
        assert_eq!(seconds, 4);
        assert!(nanoseconds > 999_000_000, "{nanoseconds}");
        // Written back after an error too, once the timeout was read.
        set_timeout(5, 0);
        assert_eq!(ppoll(unmapped, 1, timeout, 0, 0), efault);
        assert_eq!(read_timeout().0, 4);
        // A wait that ends with its timeout leaves none of it.
        process
            .write_user_memory(writable, &pollfds(&[(0, IN)]))
            .unwrap();
        set_timeout(0, 1000);
        assert_eq!(ppoll(writable, 1, timeout, 0, 0), 0);
        assert_eq!(read_timeout(), (0, 0));
        // One too long to count in nanoseconds is left as it was.
        set_timeout(i64::MAX, 999_999_999);
        process
            .write_user_memory(writable, &pollfds(&[(1, OUT)]))
            .unwrap();
        assert_eq!(ppoll(writable, 1, timeout, 0, 0), 1);
        assert_eq!(read_timeout(), (i64::MAX, 999_999_999));
        set_timeout(0, 0);
        // A timeout the program cannot write is no error.
        let read_only_timeout = read_only + 1024;
        assert_eq!(ppoll(writable, 1, read_only_timeout, 0, 0), 1);
        let mut left = [0u8; 16];
        process
            .read_user_memory(read_only_timeout, &mut left)
            .unwrap();
        assert_eq!(left[..8], 1i64.to_ne_bytes());

        // The count is checked against the open-file limit's soft value, as `prlimit64` set it.
        let lowered = writable + 2048 + 64;
        let mut limit = [0u8; 16];
        limit[..8].copy_from_slice(&2u64.to_ne_bytes());
        limit[8..].copy_from_slice(&2u64.to_ne_bytes());
        process.write_user_memory(lowered, &limit).unwrap();
        let nofile = u64::from(limits::resource::NOFILE);
        assert_eq!(
            call(&process, number::PRLIMIT64, [0, nofile, lowered, 0, 0, 0]),
            0
        );
        let entries = pollfds(&[(1, OUT), (-1, OUT), (2, OUT)]);
        process.write_user_memory(writable, &entries).unwrap();
        assert_eq!(ppoll(writable, 2, timeout, 0, 0), 1);
        assert_eq!(ppoll(writable, 3, timeout, 0, 0), einval);
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn ioctl_answers_as_a_serial_console() {
        use terminal::{TERMIOS, request};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"\x18\x00\x50\x00\x00\x00\x00\x00", &[]);
        let writable = DATA + 4096;
        let ioctl = |fd, request: u32, argument| {
            call(
                &process,
                number::IOCTL,
                [fd, u64::from(request), argument, 0, 0, 0],
            )
        };
        let read = |length: usize| {
            let mut bytes = alloc::vec![0u8; length];
            process.read_user_memory(writable, &mut bytes).unwrap();
            bytes
        };
        // The settings, on each standard descriptor; the descriptor's upper half ignored.
        for fd in [0, 1, 2, 1 << 32 | 1] {
            process.write_user_memory(writable, &[0xff; 64]).unwrap();
            assert_eq!(ioctl(fd, request::TCGETS, writable), 0);
            assert_eq!(read(36), TERMIOS);
            assert_eq!(read(37)[36], 0xff);
        }
        // With the speeds: 115200 baud each way.
        assert_eq!(ioctl(1, request::TCGETS2, writable), 0);
        let settings = read(44);
        assert_eq!(settings[..36], TERMIOS);
        assert_eq!(settings[36..40], 115_200u32.to_ne_bytes());
        assert_eq!(settings[40..], 115_200u32.to_ne_bytes());
        // Nothing waits to be read.
        assert_eq!(ioctl(0, request::FIONREAD, writable), 0);
        assert_eq!(read(4), [0; 4]);
        // The window size: zero, as on a serial line, until a program sets it (24 by 80, from
        // the read-only page).
        assert_eq!(ioctl(1, request::TIOCGWINSZ, writable), 0);
        assert_eq!(read(8), [0; 8]);
        assert_eq!(ioctl(2, request::TIOCSWINSZ, DATA), 0);
        assert_eq!(ioctl(0, request::TIOCGWINSZ, writable), 0);
        assert_eq!(read(8), b"\x18\x00\x50\x00\x00\x00\x00\x00");
        process.write_user_memory(writable, &[0; 8]).unwrap();
        assert_eq!(ioctl(2, request::TIOCSWINSZ, writable), 0);

        // `EBADF` before the request is looked at; then `ENOTTY` for a request the console
        // does not answer (setting the settings, job control, the descriptor's flags), and the
        // request's upper half ignored; then `EFAULT`.
        assert_eq!(ioctl(3, request::TCGETS, writable), Errno::EBADF.result());
        assert_eq!(ioctl(3, 0x5402, writable), Errno::EBADF.result());
        for unknown in [
            0,
            0x5402,
            0x540f,
            0x5421,
            0x5451,
            request::TCGETS | 0x1_0000,
        ] {
            assert_eq!(
                ioctl(1, unknown, writable),
                Errno::ENOTTY.result(),
                "{unknown:#x}"
            );
        }
        assert_eq!(
            call(
                &process,
                number::IOCTL,
                [1, 1 << 32 | u64::from(request::TCGETS), writable, 0, 0, 0]
            ),
            0
        );
        for request in [
            request::TCGETS,
            request::TCGETS2,
            request::TIOCGWINSZ,
            request::FIONREAD,
        ] {
            assert_eq!(
                ioctl(1, request, DATA),
                Errno::EFAULT.result(),
                "{request:#x}"
            );
            assert_eq!(ioctl(1, request, 0), Errno::EFAULT.result(), "{request:#x}");
        }
        assert_eq!(ioctl(1, request::TIOCSWINSZ, 0), Errno::EFAULT.result());
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn arch_prctl_sets_and_reads_the_thread_pointer() {
        use thread_pointer::{arch_prctl, code};
        let [set_fs, get_fs, get_gs, get_cpuid, set_cpuid] = [
            code::ARCH_SET_FS,
            code::ARCH_GET_FS,
            code::ARCH_GET_GS,
            code::ARCH_GET_CPUID,
            code::ARCH_SET_CPUID,
        ]
        .map(u64::from);
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let mut context = UserContext::new(0x40_0000, 0);
        assert_eq!(context.thread_pointer(), 0);
        let mut prctl = |code: u64, address| arch_prctl::<P>(&process, &mut context, code, address);
        // Any user address, mapped or not; nothing at or above the end of user space.
        let end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(prctl(set_fs, end - 1), Ok(0));
        assert_eq!(prctl(set_fs, end), Err(Errno::EPERM));
        assert_eq!(prctl(set_fs, u64::MAX), Err(Errno::EPERM));
        // Only the code's low 32 bits count, as for Linux's `int`.
        assert_eq!(prctl(1 << 32 | set_fs, 0x7000_1000).map(|_| ()), Ok(()));
        // Read back into the writable page; not into the read-only one or past the end.
        assert_eq!(prctl(get_fs, DATA + 4096 + 8), Ok(0));
        assert_eq!(prctl(get_gs, DATA + 4096 + 16), Ok(0));
        for address in [DATA, DATA + 8192 - 4, 0] {
            assert_eq!(prctl(get_fs, address), Err(Errno::EFAULT));
            assert_eq!(prctl(get_gs, address), Err(Errno::EFAULT));
        }
        assert_eq!(prctl(get_cpuid, 0), Ok(1));
        assert_eq!(prctl(set_cpuid, 0), Err(Errno::ENODEV));
        // The GS base waits for the SWAPGS fence (D49); unknown codes are refused too.
        for unknown in [code::ARCH_SET_GS.into(), 0x3001, 0x5001, 0x1023, 0] {
            assert_eq!(prctl(unknown, 0x7000_2000), Err(Errno::EINVAL));
        }
        assert_eq!(context.thread_pointer(), 0x7000_1000);
        let mut words = [0u8; 16];
        process
            .read_user_memory(DATA + 4096 + 8, &mut words)
            .unwrap();
        assert_eq!(&words[..8], &0x7000_1000u64.to_ne_bytes());
        assert_eq!(&words[8..], &[0; 8]);
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
