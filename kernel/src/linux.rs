//! The Linux system-call ABI (D1, M6): what a process's thread does with each trip back from
//! user mode when its program is a Linux program.
//!
//! [`spawn`] starts a thread of a [`Process`] whose exits go to [`handle`]: a system call is
//! dispatched by its Linux number on the architecture ([`number`]) and returns its result, or
//! `-errno`, in the result register; an interrupt resumes the program; a page fault the
//! process resolves (a page of one of its regions touched for the first time, D52) resumes it
//! too; any other exception sends the program the signal Linux would send for it, which its
//! handler may catch ([`fault`], D65).
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
//! ([`terminal`], D61), and signals sent to the caller with `kill`, `tkill` and `tgkill`,
//! `rt_sigpending`, and delivery by their default actions ([`signals`], D63), and signal
//! handlers on Linux's frames with `rt_sigreturn` ([`signal_frame`], [`trampoline`], D64), and
//! faults delivered to them with Linux's `siginfo_t` ([`fault`], D65), and real-time signals
//! queued with `rt_sigqueueinfo` and `rt_tgsigqueueinfo` and taken with `rt_sigtimedwait`
//! (D68), and waits for a handler with `rt_sigsuspend` and, on x86_64, `pause` (D70), and
//! `ppoll`'s signal mask during the call (D71), and the clocks and sleeping
//! (`clock_gettime`, `gettimeofday`, `nanosleep`, `clock_nanosleep`, ..., [`time`], D73), and
//! the console's status with `fstat`, `newfstatat` and `statx` ([`stat`]) and `lseek`
//! refused on it ([`terminal`], D74), and the process's user and group ids with `getuid`,
//! `setuid`, `setresuid`, `setgroups` and their kin ([`credentials`], D75).
//! Every other number returns `ENOSYS`, as Linux does for a call it lacks, so a program that
//! probes for a call carries on. The next calls are the rest of what a static program's
//! startup needs, then the file system's (M7).

pub mod credentials;
pub mod exec;
pub mod fault;
pub mod futex;
pub mod identity;
pub mod limits;
pub mod memory;
pub mod poll;
pub mod random;
pub mod signal_frame;
pub mod signals;
pub mod stat;
pub mod terminal;
pub mod thread;
pub mod thread_pointer;
pub mod time;
pub mod trampoline;

use crate::process::{self, Process, ProcessExit, UnresolvedFault};
use crate::thread::{JoinHandle, SpawnError};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Format;
use eggos_hal::user::{UserContext, UserExit};
use eggos_hal::{ExceptionKind, Platform};
use thread::LinuxThread;

/// Linux's system-call numbers. x86_64 has its own table; AArch64 and RISC-V share the generic
/// one (`asm-generic/unistd.h`).
pub mod number {
    #[cfg(target_arch = "x86_64")]
    mod table {
        pub const WRITE: u64 = 1;
        pub const FSTAT: u64 = 5;
        pub const POLL: u64 = 7;
        pub const LSEEK: u64 = 8;
        pub const MMAP: u64 = 9;
        pub const MPROTECT: u64 = 10;
        pub const MUNMAP: u64 = 11;
        pub const BRK: u64 = 12;
        pub const RT_SIGACTION: u64 = 13;
        pub const RT_SIGPROCMASK: u64 = 14;
        pub const RT_SIGRETURN: u64 = 15;
        pub const IOCTL: u64 = 16;
        pub const WRITEV: u64 = 20;
        pub const SCHED_YIELD: u64 = 24;
        pub const MREMAP: u64 = 25;
        pub const MADVISE: u64 = 28;
        pub const PAUSE: u64 = 34;
        pub const NANOSLEEP: u64 = 35;
        pub const GETPID: u64 = 39;
        pub const CLONE: u64 = 56;
        pub const EXIT: u64 = 60;
        pub const KILL: u64 = 62;
        pub const UNAME: u64 = 63;
        pub const GETTIMEOFDAY: u64 = 96;
        pub const GETRLIMIT: u64 = 97;
        pub const GETUID: u64 = 102;
        pub const GETGID: u64 = 104;
        pub const SETUID: u64 = 105;
        pub const SETGID: u64 = 106;
        pub const GETEUID: u64 = 107;
        pub const GETEGID: u64 = 108;
        pub const GETPPID: u64 = 110;
        pub const SETREUID: u64 = 113;
        pub const SETREGID: u64 = 114;
        pub const GETGROUPS: u64 = 115;
        pub const SETGROUPS: u64 = 116;
        pub const SETRESUID: u64 = 117;
        pub const GETRESUID: u64 = 118;
        pub const SETRESGID: u64 = 119;
        pub const GETRESGID: u64 = 120;
        pub const SETFSUID: u64 = 122;
        pub const SETFSGID: u64 = 123;
        pub const RT_SIGPENDING: u64 = 127;
        pub const RT_SIGTIMEDWAIT: u64 = 128;
        pub const RT_SIGQUEUEINFO: u64 = 129;
        pub const RT_SIGSUSPEND: u64 = 130;
        pub const SIGALTSTACK: u64 = 131;
        pub const ARCH_PRCTL: u64 = 158;
        pub const SETRLIMIT: u64 = 160;
        pub const SETTIMEOFDAY: u64 = 164;
        pub const GETTID: u64 = 186;
        pub const TKILL: u64 = 200;
        pub const TIME: u64 = 201;
        pub const FUTEX: u64 = 202;
        pub const SET_TID_ADDRESS: u64 = 218;
        pub const CLOCK_SETTIME: u64 = 227;
        pub const CLOCK_GETTIME: u64 = 228;
        pub const CLOCK_GETRES: u64 = 229;
        pub const CLOCK_NANOSLEEP: u64 = 230;
        pub const EXIT_GROUP: u64 = 231;
        pub const TGKILL: u64 = 234;
        pub const NEWFSTATAT: u64 = 262;
        pub const PPOLL: u64 = 271;
        pub const SET_ROBUST_LIST: u64 = 273;
        pub const RT_TGSIGQUEUEINFO: u64 = 297;
        pub const PRLIMIT64: u64 = 302;
        pub const GETRANDOM: u64 = 318;
        pub const STATX: u64 = 332;
        /// The highest number above.
        pub const LAST: u64 = STATX;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
        pub const IOCTL: u64 = 29;
        pub const LSEEK: u64 = 62;
        pub const WRITE: u64 = 64;
        pub const WRITEV: u64 = 66;
        pub const PPOLL: u64 = 73;
        pub const NEWFSTATAT: u64 = 79;
        pub const FSTAT: u64 = 80;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
        pub const SET_TID_ADDRESS: u64 = 96;
        pub const FUTEX: u64 = 98;
        pub const SET_ROBUST_LIST: u64 = 99;
        pub const NANOSLEEP: u64 = 101;
        pub const CLOCK_SETTIME: u64 = 112;
        pub const CLOCK_GETTIME: u64 = 113;
        pub const CLOCK_GETRES: u64 = 114;
        pub const CLOCK_NANOSLEEP: u64 = 115;
        pub const SCHED_YIELD: u64 = 124;
        pub const KILL: u64 = 129;
        pub const TKILL: u64 = 130;
        pub const TGKILL: u64 = 131;
        pub const SIGALTSTACK: u64 = 132;
        pub const RT_SIGSUSPEND: u64 = 133;
        pub const RT_SIGACTION: u64 = 134;
        pub const RT_SIGPROCMASK: u64 = 135;
        pub const RT_SIGPENDING: u64 = 136;
        pub const RT_SIGTIMEDWAIT: u64 = 137;
        pub const RT_SIGQUEUEINFO: u64 = 138;
        pub const RT_SIGRETURN: u64 = 139;
        pub const SETREGID: u64 = 143;
        pub const SETGID: u64 = 144;
        pub const SETREUID: u64 = 145;
        pub const SETUID: u64 = 146;
        pub const SETRESUID: u64 = 147;
        pub const GETRESUID: u64 = 148;
        pub const SETRESGID: u64 = 149;
        pub const GETRESGID: u64 = 150;
        pub const SETFSUID: u64 = 151;
        pub const SETFSGID: u64 = 152;
        pub const GETGROUPS: u64 = 158;
        pub const SETGROUPS: u64 = 159;
        pub const UNAME: u64 = 160;
        pub const GETRLIMIT: u64 = 163;
        pub const SETRLIMIT: u64 = 164;
        pub const GETTIMEOFDAY: u64 = 169;
        pub const SETTIMEOFDAY: u64 = 170;
        pub const GETPID: u64 = 172;
        pub const GETPPID: u64 = 173;
        pub const GETUID: u64 = 174;
        pub const GETEUID: u64 = 175;
        pub const GETGID: u64 = 176;
        pub const GETEGID: u64 = 177;
        pub const GETTID: u64 = 178;
        pub const BRK: u64 = 214;
        pub const CLONE: u64 = 220;
        pub const MUNMAP: u64 = 215;
        pub const MREMAP: u64 = 216;
        pub const MMAP: u64 = 222;
        pub const MPROTECT: u64 = 226;
        pub const MADVISE: u64 = 233;
        pub const RT_TGSIGQUEUEINFO: u64 = 240;
        pub const PRLIMIT64: u64 = 261;
        pub const GETRANDOM: u64 = 278;
        pub const STATX: u64 = 291;
        /// The highest number above.
        pub const LAST: u64 = STATX;
    }
    pub use table::*;

    const _: () = {
        let all = [
            WRITE,
            WRITEV,
            EXIT,
            EXIT_GROUP,
            CLONE,
            BRK,
            MMAP,
            MUNMAP,
            MPROTECT,
            MREMAP,
            MADVISE,
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
            RT_SIGPENDING,
            RT_SIGRETURN,
            RT_SIGTIMEDWAIT,
            RT_SIGQUEUEINFO,
            RT_TGSIGQUEUEINFO,
            RT_SIGSUSPEND,
            #[cfg(target_arch = "x86_64")]
            PAUSE,
            SIGALTSTACK,
            KILL,
            TKILL,
            TGKILL,
            #[cfg(target_arch = "x86_64")]
            POLL,
            PPOLL,
            IOCTL,
            LSEEK,
            FSTAT,
            NEWFSTATAT,
            STATX,
            GETUID,
            GETEUID,
            GETGID,
            GETEGID,
            GETRESUID,
            GETRESGID,
            GETGROUPS,
            SETUID,
            SETGID,
            SETREUID,
            SETREGID,
            SETRESUID,
            SETRESGID,
            SETFSUID,
            SETFSGID,
            SETGROUPS,
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
    /// hard resource limit raised or an id changed by a process that is not privileged
    /// (`setrlimit`, `prlimit64`, `setuid`, `setgroups`, ...), the alternate signal stack
    /// changed while the thread runs on it (`sigaltstack`), or a `si_code` only the kernel and
    /// `kill` may send queued to another process (`rt_sigqueueinfo`), or the wall clock set
    /// (`clock_settime`, `settimeofday`).
    pub const EPERM: Self = Self(1);
    /// No such file: here, a program's interpreter (`execve`), or any file a path names,
    /// as there are none yet (`newfstatat`, `statx`).
    pub const ENOENT: Self = Self(2);
    /// No such process: here, one the caller cannot reach (`prlimit64`).
    pub const ESRCH: Self = Self(3);
    /// Interrupted: a signal's handler started during the wait (`rt_sigsuspend`, `pause`).
    pub const EINTR: Self = Self(4);
    /// The arguments and environment are too long (`execve`).
    pub const E2BIG: Self = Self(7);
    /// Not a program the kernel can run (`execve`).
    pub const ENOEXEC: Self = Self(8);
    /// Bad file descriptor: here, one that is not open; so far only the standard descriptors
    /// are.
    pub const EBADF: Self = Self(9);
    /// Try again: here, the random number generator is not seeded yet (`getrandom` with
    /// `GRND_NONBLOCK`), a real-time signal finds the signal queue full (`rt_sigqueueinfo`,
    /// `tgkill`), or no signal came before the timeout (`rt_sigtimedwait`).
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
    /// Not a directory: here, a descriptor a relative path starts from (`newfstatat`,
    /// `statx`).
    pub const ENOTDIR: Self = Self(20);
    /// An invalid argument: here, a segment outside user space (`execve`), an unaligned
    /// address or an empty range (the memory calls), a bad signal number, set size or flag
    /// (the signal calls), a bad timeout or too many descriptors (`ppoll`, `poll`), or a
    /// clock the time calls do not answer for or a bad time (the time calls), or a bad flag
    /// or mask (`newfstatat`, `statx`) or `whence` (`lseek`), or an id of -1 (`setuid`,
    /// `setgid`), a list too small or too long (`getgroups`, `setgroups`).
    pub const EINVAL: Self = Self(22);
    /// Not a terminal: here, an `ioctl` request the console does not answer.
    pub const ENOTTY: Self = Self(25);
    /// Not seekable: here, the console (`lseek`).
    pub const ESPIPE: Self = Self(29);
    /// A path longer than `PATH_MAX` (`newfstatat`, `statx`).
    pub const ENAMETOOLONG: Self = Self(36);
    /// No such system call.
    pub const ENOSYS: Self = Self(38);
    /// Timed out: a wait's deadline came first (`futex`).
    pub const ETIMEDOUT: Self = Self(110);
    /// Not supported: here, an unknown flag with `MAP_SHARED_VALIDATE` (`mmap`), or a clock
    /// `clock_nanosleep` cannot sleep on.
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
    pub const SIGFPE: u32 = 8;
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

/// Starts the first thread of `process`, which runs its Linux program from `context` (see
/// [`handle`]); `clone` starts the others.
pub fn spawn<P: Platform>(
    process: Arc<Process<P>>,
    context: UserContext,
) -> Result<JoinHandle, SpawnError> {
    let thread = LinuxThread::first(&process);
    spawn_thread(process, thread, context)
}

/// Starts `thread`, of `process`, running the program from `context`, its state kept for its
/// loop and handed to [`handle`] with each exit from user mode. The state is boxed: the loop
/// keeps one pointer to it, and its own variables stay in registers (in the loop's closure
/// with them, the system calls taking its address kept them in memory: +3 to +5% on the
/// system-call round trip).
pub(crate) fn spawn_thread<P: Platform>(
    process: Arc<Process<P>>,
    thread: LinuxThread,
    context: UserContext,
) -> Result<JoinHandle, SpawnError> {
    let mut thread = Box::new(thread);
    process.add_thread();
    let counted = Arc::clone(&process);
    process::spawn::<P, _>(process, context, move |process, context, exit| {
        handle(process, &mut thread, context, exit)
    })
    .inspect_err(|_| {
        counted.remove_thread();
    })
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
    thread: &mut LinuxThread,
    context: &mut UserContext,
    exit: UserExit,
) -> ControlFlow<()> {
    match exit {
        UserExit::Syscall => syscall(process, thread, context),
        UserExit::Interrupt => ControlFlow::Continue(()),
        UserExit::Exception(exception) => user_exception(
            process,
            thread,
            context,
            exception.kind,
            exception.code,
            exception.address,
        ),
    }
}

/// An exception in the program, of `kind` with raw cause `code` and the faulting address, if
/// any. A page fault resumes the program if the process resolves it (demand paging); with no
/// memory for the page, the process ends with `SIGKILL`, as Linux's out-of-memory killer
/// would; any other fault sends the program the signal Linux sends for it
/// ([`signal_fault`]). Out of line and laid out away from the system-call path, which stays
/// as short as it was (the exception's parts passed alone keep the loop's state in
/// registers). Only `context` stays live across the page's mapping for the fault's sake, in
/// place of the `kind` that ending the process needed before D65: the fault reads the rest
/// back from it ([`UserContext::last_exception`]). Keeping the raw cause and the address too
/// cost up to +1% on `linux_page_fault`.
#[cold]
#[inline(never)]
fn user_exception<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    kind: ExceptionKind,
    code: u64,
    address: Option<u64>,
) -> ControlFlow<()> {
    let (Some(page_fault), Some(address)) = (P::page_fault(kind, code), address) else {
        return signal_fault(process, thread, context);
    };
    match process.resolve_page_fault(address, page_fault) {
        Ok(()) => ControlFlow::Continue(()),
        Err(UnresolvedFault::NotAllowed) => signal_fault(process, thread, context),
        Err(UnresolvedFault::OutOfMemory) => end_process(
            process,
            ProcessExit::Killed {
                signal: signal::SIGKILL,
            },
        ),
    }
}

/// Sends the program, whose registers are `context` after the exception it raised, the
/// signal Linux sends for that exception ([`fault::classify`]), forced: its handler runs if it
/// has one and neither blocks nor ignores the signal, else the process ends with it
/// ([`signals::deliver_fault`]). Breaks when the process ends. The program's first FP/SIMD
/// instruction is no fault: it gives the thread its registers and runs again (D54).
#[cold]
#[inline(never)]
fn signal_fault<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let exception = context.last_exception();
    if process::first_fp_use::<P>(exception.kind) {
        return ControlFlow::Continue(());
    }
    let fault = fault::classify(process, &exception, context.stack());
    signals::deliver_fault(process, thread, context, &fault)
}

/// Runs the system call the program made and sets its result. Always inlined, as [`handle`]:
/// as a call of its own, the loop's state spills around it (+8 to +10% on the system-call
/// round trip when D55's calls tipped the inliner). A number past the table answers `ENOSYS`
/// with its own store: sharing the others' cost AArch64's round trip two branches (+1.6%,
/// D91).
#[inline(always)]
fn syscall<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [first, second, third, ..] = context.syscall_arguments();
    let number = context.syscall_number();
    if number > number::LAST {
        context.set_syscall_result(Errno::ENOSYS.result());
        return ControlFlow::Continue(());
    }
    let result = match number {
        number::WRITE => write::<P>(process, first, second, third),
        number::EXIT | number::EXIT_GROUP => return exit(process, thread, number, first),
        number::BRK | number::MMAP | number::MUNMAP | number::MPROTECT => {
            memory_call::<P>(process, number, context.syscall_arguments())
        }
        _ => return out_of_line_call::<P>(process, thread, number, context),
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
/// program's registers, whose result it sets: `ENOSYS` for a number with no call. One
/// out-of-line function for all of them, as [`memory_call`]: each call [`syscall`]'s own match
/// grows by adds to the thread loop that it is inlined into (the loop's registers spill, and
/// every system call pays). After a call that can make a signal due, the signal is delivered
/// before the program runs again ([`signals::deliver`]), which may end the process: then it
/// breaks. `rt_sigreturn` sets every register itself and returns the restored result
/// register's value, so writing the result back changes nothing. The waits for a signal set
/// their result and deliver it themselves.
#[inline(never)]
fn out_of_line_call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let flow = match number {
        number::RT_SIGSUSPEND => signals::rt_sigsuspend(process, thread, context),
        #[cfg(target_arch = "x86_64")]
        number::PAUSE => signals::pause(process, thread, context),
        number::PPOLL => poll::ppoll::<P>(process, thread, context),
        _ => {
            let result = match number {
                number::CLONE => thread::clone::<P>(process, thread, context),
                _ => out_of_line_result(process, thread, number, context),
            };
            context.set_syscall_result(result.unwrap_or_else(Errno::result));
            match number {
                number::KILL
                | number::TKILL
                | number::TGKILL
                | number::RT_SIGQUEUEINFO
                | number::RT_TGSIGQUEUEINFO
                | number::RT_SIGPROCMASK
                | number::RT_SIGACTION
                | number::RT_SIGRETURN => signals::deliver(process, thread, context),
                _ => ControlFlow::Continue(()),
            }
        }
    };
    if process.has_ended() {
        return ControlFlow::Break(());
    }
    flow
}

/// The result of [`out_of_line_call`]'s call `number`.
#[inline(always)]
fn out_of_line_result<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> Result<u64, Errno> {
    use credentials::Kind::{Group, User};
    let [first, second, third, fourth, fifth, sixth] = context.syscall_arguments();
    match number {
        number::WRITEV => writev::<P>(process, first, second, third),
        number::GETPID => Ok(identity::getpid(process)),
        number::GETPPID => Ok(identity::getppid()),
        number::GETTID => Ok(identity::gettid(thread)),
        number::UNAME => identity::uname(process, first),
        number::SET_TID_ADDRESS => Ok(identity::set_tid_address(thread, first)),
        number::SET_ROBUST_LIST => identity::set_robust_list(first, second),
        number::GETRANDOM => random::getrandom::<P>(process, first, second, third),
        number::GETRLIMIT => limits::getrlimit(process, first, second),
        number::SETRLIMIT => limits::setrlimit(process, first, second),
        number::PRLIMIT64 => limits::prlimit64(process, first, second, third, fourth),
        number::RT_SIGACTION => {
            signals::rt_sigaction(process, thread, first, second, third, fourth)
        }
        number::RT_SIGPROCMASK => {
            signals::rt_sigprocmask(process, thread, first, second, third, fourth)
        }
        number::SIGALTSTACK => {
            signals::sigaltstack(process, thread, first, second, context.stack())
        }
        number::RT_SIGPENDING => signals::rt_sigpending(process, thread, first, second),
        number::RT_SIGRETURN => signals::rt_sigreturn(process, thread, context),
        number::KILL => signals::kill(process, thread, first, second),
        number::TKILL => signals::tkill(process, thread, first, second),
        number::TGKILL => signals::tgkill(process, thread, first, second, third),
        number::RT_SIGQUEUEINFO => signals::rt_sigqueueinfo(process, thread, first, second, third),
        number::RT_TGSIGQUEUEINFO => {
            signals::rt_tgsigqueueinfo(process, thread, first, second, third, fourth)
        }
        number::RT_SIGTIMEDWAIT => {
            signals::rt_sigtimedwait(process, thread, first, second, third, fourth)
        }
        number::CLOCK_GETTIME => time::clock_gettime(process, first, second),
        number::CLOCK_GETRES => time::clock_getres(process, first, second),
        number::GETTIMEOFDAY => time::gettimeofday(process, first, second),
        #[cfg(target_arch = "x86_64")]
        number::TIME => time::time(process, first),
        number::NANOSLEEP => time::nanosleep(process, first),
        number::FUTEX => futex::futex::<P>(process, first, second, third, fourth, fifth, sixth),
        number::CLOCK_NANOSLEEP => time::clock_nanosleep(process, first, second, third),
        number::CLOCK_SETTIME => time::clock_settime(process, first, second),
        number::SETTIMEOFDAY => time::settimeofday(process, first, second),
        #[cfg(target_arch = "x86_64")]
        number::POLL => poll::poll::<P>(process, first, second, third),
        number::IOCTL => terminal::ioctl::<P>(process, first, second, third),
        number::LSEEK => terminal::lseek(first, third),
        number::FSTAT => stat::fstat(process, first, second),
        number::NEWFSTATAT => stat::newfstatat(process, first, second, third, fourth),
        number::STATX => stat::statx(process, first, second, third, fourth, fifth),
        number::GETUID => Ok(credentials::get_real(process, User)),
        number::GETGID => Ok(credentials::get_real(process, Group)),
        number::GETEUID => Ok(credentials::get_effective(process, User)),
        number::GETEGID => Ok(credentials::get_effective(process, Group)),
        number::GETRESUID => {
            credentials::get_real_effective_and_saved(process, User, first, second, third)
        }
        number::GETRESGID => {
            credentials::get_real_effective_and_saved(process, Group, first, second, third)
        }
        number::GETGROUPS => credentials::getgroups(process, first, second),
        number::SETUID => credentials::set(process, User, first),
        number::SETGID => credentials::set(process, Group, first),
        number::SETREUID => credentials::set_real_and_effective(process, User, first, second),
        number::SETREGID => credentials::set_real_and_effective(process, Group, first, second),
        number::SETRESUID => {
            credentials::set_real_effective_and_saved(process, User, first, second, third)
        }
        number::SETRESGID => {
            credentials::set_real_effective_and_saved(process, Group, first, second, third)
        }
        number::SETFSUID => Ok(credentials::set_filesystem(process, User, first)),
        number::SETFSGID => Ok(credentials::set_filesystem(process, Group, first)),
        number::SETGROUPS => credentials::setgroups(process, first, second),
        number::MREMAP => memory::mremap::<P>(process, first, second, third, fourth, fifth),
        number::MADVISE => memory::madvise::<P>(process, first, second, third),
        number::SCHED_YIELD => {
            crate::thread::yield_now::<P>();
            Ok(0)
        }
        #[cfg(target_arch = "x86_64")]
        number::ARCH_PRCTL => thread_pointer::arch_prctl::<P>(process, context, first, second),
        _ => Err(Errno::ENOSYS),
    }
}

/// `exit(status)`: ends the calling thread. Its queued signals go, and if it set a
/// `clear_child_tid` (`set_tid_address`, `CLONE_CHILD_CLEARTID`) and other threads go on, the
/// 32-bit word there becomes 0 and one waiter on it as a shared futex is woken, as on Linux:
/// nothing is written if the word is not the program's writable memory, and no one is woken
/// if it is not aligned. The last thread to end ends the process, with the status the first
/// thread exited with, as Linux reports its thread group leader's, or with `status` if the
/// first thread is the last. Linux keeps a status's low 8 bits. (A C library ends its last
/// thread with `exit_group` instead.)
#[inline]
pub fn exit_thread<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    status: u64,
) -> ControlFlow<()> {
    let status = status as u8;
    if thread.id() == process.id().get() {
        process.record_first_thread_status(status);
    }
    if process.remove_thread() {
        let status = process.first_thread_status().unwrap_or(status);
        return end_process(process, ProcessExit::Exited(u32::from(status)));
    }
    thread
        .signals(&mut process.signals().lock())
        .discard(u64::MAX);
    let address = thread.clear_child_tid();
    if address != 0 {
        let cleared = process.write_user_memory(address, &0u32.to_ne_bytes());
        if cleared.is_ok() && address.is_multiple_of(4) {
            let key = futex::Key::new(process.id().get(), address, true);
            futex::wake::<P>(key, u32::MAX, 1);
        }
    }
    ControlFlow::Break(())
}

/// `exit(status)` ([`exit_thread`]) and `exit_group(status)`, which ends the process, every
/// thread of it, with `status`, of which Linux keeps the low 8 bits. One call out of line for
/// both, as before threads, so the system-call loop's code stays as it was.
#[inline(never)]
fn exit<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    number: u64,
    status: u64,
) -> ControlFlow<()> {
    if number == number::EXIT {
        exit_thread(process, thread, status)
    } else {
        end_process(process, ProcessExit::Exited(status as u32 & 0xff))
    }
}

/// Ends the process as `how` says ([`Process::exit`]); its other threads waiting on a futex,
/// if it has any, stop waiting, so they end too. Breaks: the calling thread ends.
#[cold]
#[inline(never)]
pub fn end_process<P: Platform, C>(process: &Process<P>, how: ProcessExit) -> ControlFlow<(), C> {
    let others = process.threads() > 1;
    process.exit(how);
    if others {
        futex::wake_space::<P>(process.id().get());
    }
    ControlFlow::Break(())
}

/// `write(fd, buffer, count)`: the standard descriptors go to the console. Returns
/// the bytes written; if part of the buffer is not the program's memory, the bytes before
/// that part, or `EFAULT` if there are none, as Linux does. At most [`MAX_RW_COUNT`] bytes,
/// after the whole range is checked against user space, as on Linux.
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
/// waiting for it. It stops early once the process has ended (another thread's `exit_group`),
/// and the thread stops before it returns to user mode.
fn write_to_console<P: Platform>(process: &Process<P>, buffer: u64, count: u64) -> u64 {
    let mut chunk = [0u8; WRITE_CHUNK];
    let mut written = 0;
    while written < count {
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
        crate::thread::preemption_point::<P>();
        if process.has_ended_and_will_stop() {
            break;
        }
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

    static FIRST_THREADS: std::sync::Mutex<alloc::collections::BTreeMap<u32, LinuxThread>> =
        std::sync::Mutex::new(alloc::collections::BTreeMap::new());

    fn take_first_thread(process: &Process<P>) -> LinuxThread {
        FIRST_THREADS
            .lock()
            .unwrap()
            .remove(&process.id().get())
            .unwrap_or_else(|| {
                process.add_thread();
                LinuxThread::first(process)
            })
    }

    fn handle_first(
        process: &Arc<Process<P>>,
        context: &mut UserContext,
        exit: UserExit,
    ) -> ControlFlow<()> {
        let mut thread = take_first_thread(process);
        let flow = handle(process, &mut thread, context, exit);
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(process.id().get(), thread);
        flow
    }

    fn first_thread_signals(process: &Process<P>) -> signals::ThreadSignals {
        let thread = take_first_thread(process);
        let state = thread.signal_state();
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(process.id().get(), thread);
        state
    }

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
        for fd in [0, 1, 2] {
            let mut context = called(number::WRITE, [fd, start, text.len() as u64, 0, 0, 0]);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
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
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            context.syscall_result()
        };
        console_output();
        for buffer in [0, 0xffff_ffff_8000_0000, u64::MAX - 1] {
            assert_eq!(write(1, buffer, 2), Errno::EFAULT.result(), "{buffer:#x}");
        }
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(write(1, 0, 0), 0);
        assert_eq!(write(1, user_end, 0), 0);
        for buffer in [user_end + 1, 0xffff_ffff_8000_0000, u64::MAX] {
            assert_eq!(write(1, buffer, 0), Errno::EFAULT.result(), "{buffer:#x}");
        }
        assert_eq!(write(1, DATA + 4095, user_end), Errno::EFAULT.result());
        assert_eq!(write(3, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(1 << 32 | 3, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(u64::MAX, DATA + 4095, 2), Errno::EBADF.result());
        assert_eq!(write(1 << 32 | 1, DATA + 4095, 0), 0);
        assert!(console_output().is_empty());
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
            handle_first(process, &mut context, UserExit::Syscall),
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
        assert_eq!(getrandom(writable, 1000, 1 << 32), 1000);
        assert!(read(0, 1000).chunks(16).all(|chunk| chunk != [0; 16]));
        assert_eq!(read(1000, 16), [0; 16]);
        assert_eq!(getrandom(0, 0, 0), 0);
        assert_eq!(getrandom(u64::MAX, 0, 0), Errno::EFAULT.result());
        assert_eq!(getrandom(writable + 4096 - 100, 200, 0), 100);
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
        assert_eq!(writev(&[(0, 2), (0, 1 << 63)], 1), Errno::EINVAL.result());
        assert_eq!(writev(&[(0, 2), (0, user_end)], 1), Errno::EFAULT.result());
        let kernel_address = 0xffff_ffff_8000_0000u64.wrapping_sub(DATA);
        assert_eq!(
            writev(&[(0, 2), (kernel_address, 0)], 1),
            Errno::EFAULT.result()
        );
        assert!(console_output().is_empty());
        assert_eq!(writev(&[(0, 2), (4096 * 2, 2)], 1), 2);
        assert_eq!(writev(&[(4096 * 2, 2), (0, 2)], 1), Errno::EFAULT.result());
        assert_eq!(console_output(), b"ab");
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
        let last_entry = DATA + 2 * 4096 - IOVEC_SIZE as u64;
        for (length, error) in [(1 << 63, Errno::EINVAL), (2, Errno::EFAULT)] {
            let mut buffers = alloc::vec![(0, 2); 4096 / IOVEC_SIZE];
            *buffers.last_mut().unwrap() = (0, length);
            let full_page = process_with_iovecs(&kernel, b"ab", &buffers);
            assert_eq!(
                call(&full_page, number::WRITEV, [1, last_entry, 2, 0, 0, 0]),
                error.result()
            );
            assert_eq!(
                call(&full_page, number::WRITEV, [1, last_entry + 16, 1, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
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
        assert_eq!(call(&process, number::SETUID, [1000, 0, 0, 0, 0, 0]), 0);
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
            process
                .credentials_and_limits()
                .lock()
                .limits
                .get(resource::NOFILE),
            Some(Limit {
                soft: 128,
                hard: 512
            })
        );
        assert_eq!(
            call(
                &process,
                number::PRLIMIT64,
                [0, nofile, writable + 32, DATA, 0, 0]
            ),
            Errno::EFAULT.result()
        );
        assert_eq!(
            process
                .credentials_and_limits()
                .lock()
                .limits
                .get(resource::NOFILE),
            Some(Limit {
                soft: 64,
                hard: 256
            })
        );
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
        let other = process_with_iovecs(&kernel, b"ab", &[]);
        assert_eq!(
            other
                .credentials_and_limits()
                .lock()
                .limits
                .get(resource::NOFILE),
            Some(limits::DEFAULTS[resource::NOFILE as usize])
        );
    }

    #[test]
    fn credentials_are_read_and_changed_through_user_memory() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let writable = DATA + 4096;
        let read_only = DATA;
        let unmapped = DATA + 2 * 4096;
        let words = |address, count: usize| {
            let mut bytes = alloc::vec![0u8; count * 4];
            process.read_user_memory(address, &mut bytes).unwrap();
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&word| u32::from_ne_bytes(word))
                .collect::<alloc::vec::Vec<_>>()
        };
        let store = |address, words: &[u32]| {
            let bytes: alloc::vec::Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
            process.write_user_memory(address, &bytes).unwrap();
        };
        let unchanged = u64::from(credentials::UNCHANGED);
        let upper_half = 0xffff_ffff_0000_0000;
        let ids = |real, effective, saved| {
            assert_eq!(
                call(
                    &process,
                    number::GETRESUID,
                    [writable, writable + 4, writable + 8, 0, 0, 0]
                ),
                0
            );
            assert_eq!(words(writable, 3), [real, effective, saved]);
        };
        for getter in [
            number::GETUID,
            number::GETEUID,
            number::GETGID,
            number::GETEGID,
        ] {
            assert_eq!(call(&process, getter, [0; 6]), 0);
        }
        ids(0, 0, 0);
        assert_eq!(call(&process, number::GETGROUPS, [0, 0, 0, 0, 0, 0]), 0);

        store(writable + 64, &[300, 100, 200]);
        assert_eq!(
            call(&process, number::SETGROUPS, [3, writable + 64, 0, 0, 0, 0]),
            0
        );
        assert_eq!(call(&process, number::GETGROUPS, [0, 0, 0, 0, 0, 0]), 3);
        assert_eq!(
            call(&process, number::GETGROUPS, [2, writable, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(
                &process,
                number::GETGROUPS,
                [u64::from(u32::MAX), writable, 0, 0, 0, 0]
            ),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(&process, number::GETGROUPS, [3, read_only, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(
            call(
                &process,
                number::GETGROUPS,
                [upper_half | 3, writable, 0, 0, 0, 0]
            ),
            3
        );
        assert_eq!(words(writable, 3), [100, 200, 300]);
        assert_eq!(
            call(&process, number::SETGROUPS, [65_537, writable, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(&process, number::SETGROUPS, [1, unmapped, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        store(writable + 512, &[5, credentials::UNCHANGED, 7]);
        assert_eq!(
            call(&process, number::SETGROUPS, [3, writable + 512, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        store(unmapped - 8, &[42, credentials::UNCHANGED]);
        for (size, list, error) in [
            (3, unmapped - 8, Errno::EINVAL),
            (2, unmapped - 4, Errno::EINVAL),
            (2, unmapped - 2, Errno::EFAULT),
        ] {
            assert_eq!(
                call(&process, number::SETGROUPS, [size, list, 0, 0, 0, 0]),
                error.result(),
                "{size} groups at {list:#x}"
            );
        }
        assert_eq!(
            call(&process, number::GETGROUPS, [3, writable, 0, 0, 0, 0]),
            3
        );
        assert_eq!(words(writable, 3), [100, 200, 300]);
        let many: alloc::vec::Vec<u32> = (0..100).rev().collect();
        store(writable + 1024, &many);
        assert_eq!(
            call(
                &process,
                number::SETGROUPS,
                [100, writable + 1024, 0, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call(
                &process,
                number::GETGROUPS,
                [128, writable + 2048, 0, 0, 0, 0]
            ),
            100
        );
        assert_eq!(
            words(writable + 2048, 100),
            (0..100).collect::<alloc::vec::Vec<u32>>()
        );
        assert_eq!(
            call(&process, number::SETGROUPS, [3, writable + 64, 0, 0, 0, 0]),
            0
        );

        assert_eq!(call(&process, number::SETRESGID, [5, 6, 7, 0, 0, 0]), 0);
        assert_eq!(call(&process, number::GETGID, [0; 6]), 5);
        assert_eq!(call(&process, number::GETEGID, [0; 6]), 6);
        assert_eq!(
            call(&process, number::SETFSGID, [unchanged, 0, 0, 0, 0, 0]),
            6
        );
        assert_eq!(call(&process, number::SETFSGID, [9, 0, 0, 0, 0, 0]), 6);
        assert_eq!(
            call(
                &process,
                number::SETRESGID,
                [unchanged, unchanged, unchanged, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call(&process, number::SETFSGID, [unchanged, 0, 0, 0, 0, 0]),
            9
        );

        assert_eq!(
            call(
                &process,
                number::SETRESUID,
                [1000, upper_half | 1001, 0, 0, 0, 0]
            ),
            0
        );
        ids(1000, 1001, 0);
        assert_eq!(
            call(&process, number::SETFSUID, [4242, 0, 0, 0, 0, 0]),
            1001
        );
        assert_eq!(
            call(&process, number::SETFSUID, [unchanged, 0, 0, 0, 0, 0]),
            1001
        );
        assert_eq!(
            call(&process, number::SETFSUID, [1000, 0, 0, 0, 0, 0]),
            1001
        );
        assert_eq!(
            call(&process, number::SETFSUID, [unchanged, 0, 0, 0, 0, 0]),
            1000
        );
        assert_eq!(
            call(
                &process,
                number::SETRESUID,
                [unchanged, unchanged, unchanged, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call(&process, number::SETFSUID, [unchanged, 0, 0, 0, 0, 0]),
            1000
        );
        assert_eq!(
            call(&process, number::SETGROUPS, [0, 0, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call(&process, number::SETUID, [4242, 0, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call(&process, number::SETUID, [unchanged, 0, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            call(&process, number::SETREUID, [unchanged, 0, 0, 0, 0, 0]),
            0
        );
        ids(1000, 0, 0);
        assert_eq!(
            call(&process, number::SETREUID, [unchanged, 1000, 0, 0, 0, 0]),
            0
        );
        ids(1000, 1000, 0);
        assert_eq!(
            call(&process, number::SETREUID, [1000, unchanged, 0, 0, 0, 0]),
            0
        );
        ids(1000, 1000, 1000);
        assert_eq!(
            call(&process, number::SETREUID, [unchanged, 0, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call(
                &process,
                number::GETRESUID,
                [writable, unmapped, writable + 8, 0, 0, 0]
            ),
            Errno::EFAULT.result()
        );
        assert_eq!(
            call(&process, number::SETREGID, [7, unchanged, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call(&process, number::SETREGID, [6, unchanged, 0, 0, 0, 0]),
            0
        );
        assert_eq!(call(&process, number::GETGID, [0; 6]), 6);
        assert_eq!(
            call(&process, number::SETGID, [7, 0, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call(
                &process,
                number::SETRESGID,
                [unchanged, unchanged, 6, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call(&process, number::SETGID, [8, 0, 0, 0, 0, 0]),
            Errno::EPERM.result()
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
        assert_eq!(sigaction([0, unmapped, 0, 8]), efault);
        store(writable, &[0; 4]);
        assert_eq!(sigaction([sigpipe, read_only, read_only, 8]), efault);
        assert_eq!(sigaction([sigpipe, 0, writable + 64, 8]), 0);
        assert_eq!(
            action(writable + 64).handler,
            u64::from_ne_bytes(*b"ab\0\0\0\0\0\0")
        );

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
        assert_eq!(sigprocmask(0, writable, 0, 16), einval);
        assert_eq!(sigprocmask(3, writable, 0, 8), einval);
        assert_eq!(sigprocmask(3, unmapped, 0, 8), efault);
        store(writable, &[0]);
        assert_eq!(sigprocmask(2, writable, read_only, 8), efault);
        assert_eq!(sigprocmask(0, 0, writable + 8, 8), 0);
        assert_eq!(words(writable + 8, 1), [0]);

        let sigaltstack = |new, old, stack| {
            let mut context = UserContext::new(0x40_0000, stack);
            context.simulate_syscall(number::SIGALTSTACK, [new, old, 0, 0, 0, 0], 0x40_0004);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
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
        store(writable, &[base, 2 | 1 << 31, size]);
        assert_eq!(sigaltstack(writable, read_only, 0), efault);
        assert_eq!(sigaltstack(0, writable + 64, 0), 0);
        assert_eq!(words(writable + 64, 3), [0, 2 | 1 << 31, 0]);

        let other = process_with_iovecs(&kernel, b"ab", &[]);
        assert_eq!(other.signals().lock().action(13), Some(Action::default()));
        assert_eq!(first_thread_signals(&other), Default::default());
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
        let entries = pollfds(&[(0, 0), (1, 0), (2, 0)]);
        process.write_user_memory(writable, &entries).unwrap();
        assert_eq!(ppoll(writable, 3, timeout, 0, 0), 0);
        assert_eq!(returned(3), [0, 0, 0]);
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
        assert_eq!(ppoll(writable, 5, timeout, mask, 8), 3);
        let many = alloc::vec![(1, OUT); 20];
        process
            .write_user_memory(writable, &pollfds(&many))
            .unwrap();
        assert_eq!(ppoll(writable, 20, timeout, 0, 0), 20);
        assert_eq!(returned(20), [OUT; 20]);
        assert_eq!(ppoll(0, 0, timeout, 0, 0), 0);
        #[cfg(target_arch = "x86_64")]
        {
            let entries = pollfds(&[(2, OUT), (7, 0)]);
            process.write_user_memory(writable, &entries).unwrap();
            assert_eq!(call(&process, number::POLL, [writable, 2, 0, 0, 0, 0]), 2);
            assert_eq!(returned(2), [OUT, NVAL]);
            process
                .write_user_memory(writable, &pollfds(&[(0, IN)]))
                .unwrap();
            assert_eq!(
                call(&process, number::POLL, [writable, 1, 1 << 32, 0, 0, 0]),
                0
            );
            assert_eq!(returned(1), [0]);
        }

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
        assert_eq!(ppoll(unmapped, 1, timeout, 0, 0), efault);
        assert_eq!(ppoll(writable, 1024, timeout, 0, 0), efault);
        assert_eq!(ppoll(user_end + 1, 0, timeout, 0, 0), efault);
        assert_eq!(ppoll(user_end - 4, 1, timeout, 0, 0), efault);
        assert_eq!(ppoll(read_only, 1, timeout, 0, 0), efault);
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
        set_timeout(5, 0);
        assert_eq!(ppoll(unmapped, 1, timeout, 0, 0), efault);
        assert_eq!(read_timeout().0, 4);
        process
            .write_user_memory(writable, &pollfds(&[(0, IN)]))
            .unwrap();
        set_timeout(0, 1000);
        assert_eq!(ppoll(writable, 1, timeout, 0, 0), 0);
        assert_eq!(read_timeout(), (0, 0));
        set_timeout(i64::MAX, 999_999_999);
        process
            .write_user_memory(writable, &pollfds(&[(1, OUT)]))
            .unwrap();
        assert_eq!(ppoll(writable, 1, timeout, 0, 0), 1);
        assert_eq!(read_timeout(), (i64::MAX, 999_999_999));
        set_timeout(0, 0);
        let read_only_timeout = read_only + 1024;
        assert_eq!(ppoll(writable, 1, read_only_timeout, 0, 0), 1);
        let mut left = [0u8; 16];
        process
            .read_user_memory(read_only_timeout, &mut left)
            .unwrap();
        assert_eq!(left[..8], 1i64.to_ne_bytes());

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
        for fd in [0, 1, 2, 1 << 32 | 1] {
            process.write_user_memory(writable, &[0xff; 64]).unwrap();
            assert_eq!(ioctl(fd, request::TCGETS, writable), 0);
            assert_eq!(read(36), TERMIOS);
            assert_eq!(read(37)[36], 0xff);
        }
        assert_eq!(ioctl(1, request::TCGETS2, writable), 0);
        let settings = read(44);
        assert_eq!(settings[..36], TERMIOS);
        assert_eq!(settings[36..40], 115_200u32.to_ne_bytes());
        assert_eq!(settings[40..], 115_200u32.to_ne_bytes());
        assert_eq!(ioctl(0, request::FIONREAD, writable), 0);
        assert_eq!(read(4), [0; 4]);
        assert_eq!(ioctl(1, request::TIOCGWINSZ, writable), 0);
        assert_eq!(read(8), [0; 8]);
        assert_eq!(ioctl(2, request::TIOCSWINSZ, DATA), 0);
        assert_eq!(ioctl(0, request::TIOCGWINSZ, writable), 0);
        assert_eq!(read(8), b"\x18\x00\x50\x00\x00\x00\x00\x00");
        process.write_user_memory(writable, &[0; 8]).unwrap();
        assert_eq!(ioctl(2, request::TIOCSWINSZ, writable), 0);

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
        let end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(prctl(set_fs, end - 1), Ok(0));
        assert_eq!(prctl(set_fs, end), Err(Errno::EPERM));
        assert_eq!(prctl(set_fs, u64::MAX), Err(Errno::EPERM));
        assert_eq!(prctl(1 << 32 | set_fs, 0x7000_1000).map(|_| ()), Ok(()));
        assert_eq!(prctl(get_fs, DATA + 4096 + 8), Ok(0));
        assert_eq!(prctl(get_gs, DATA + 4096 + 16), Ok(0));
        for address in [DATA, DATA + 8192 - 4, 0] {
            assert_eq!(prctl(get_fs, address), Err(Errno::EFAULT));
            assert_eq!(prctl(get_gs, address), Err(Errno::EFAULT));
        }
        assert_eq!(prctl(get_cpuid, 0), Ok(1));
        assert_eq!(prctl(set_cpuid, 0), Err(Errno::ENODEV));
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
    fn signals_sent_to_the_caller_are_delivered_before_the_call_returns() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (sigabrt, sigusr1, sigchld) = (6, 10, 17);
        let (esrch, einval) = (Errno::ESRCH.result(), Errno::EINVAL.result());
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let pid = u64::from(process.id().get());
        let writable = DATA + 4096;
        let own_group = (pid > 1).then(|| pid.wrapping_neg());
        for target in [pid, 0, 1 << 32 | pid].into_iter().chain(own_group) {
            assert_eq!(call(&process, number::KILL, [target, 0, 0, 0, 0, 0]), 0);
            assert_eq!(
                call(&process, number::KILL, [target, sigchld, 0, 0, 0, 0]),
                0
            );
            assert_eq!(
                call(&process, number::KILL, [target, 65, 0, 0, 0, 0]),
                einval
            );
        }
        let group_of_none = u64::from(i32::MIN as u32);
        for target in [pid + 1, u64::MAX, group_of_none, (pid + 1).wrapping_neg()] {
            assert_eq!(
                call(&process, number::KILL, [target, 65, 0, 0, 0, 0]),
                esrch
            );
        }
        assert_eq!(call(&process, number::TKILL, [pid, 0, 0, 0, 0, 0]), 0);
        assert_eq!(call(&process, number::TKILL, [0, 65, 0, 0, 0, 0]), einval);
        assert_eq!(
            call(&process, number::TKILL, [pid + 1, 65, 0, 0, 0, 0]),
            esrch
        );
        assert_eq!(call(&process, number::TKILL, [pid, 65, 0, 0, 0, 0]), einval);
        assert_eq!(call(&process, number::TGKILL, [pid, pid, 0, 0, 0, 0]), 0);
        for (tgid, tid, result) in [
            (0, pid, einval),
            (pid, u64::from(u32::MAX), einval),
            (pid + 1, pid, esrch),
            (pid, pid + 1, esrch),
            (pid, pid, einval),
        ] {
            assert_eq!(
                call(&process, number::TGKILL, [tgid, tid, 65, 0, 0, 0]),
                result
            );
        }
        process
            .write_user_memory(writable, &(1u64 << (sigusr1 - 1)).to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(&process, number::RT_SIGPROCMASK, [0, writable, 0, 8, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::TGKILL, [pid, pid, sigusr1, 0, 0, 0]),
            0
        );
        let pending = writable + 64;
        for size in [8, 2, 0] {
            process.write_user_memory(pending, &[0xff; 8]).unwrap();
            assert_eq!(
                call(&process, number::RT_SIGPENDING, [pending, size, 0, 0, 0, 0]),
                0
            );
            let mut bytes = [0u8; 8];
            process.read_user_memory(pending, &mut bytes).unwrap();
            let mut expected = [0xff; 8];
            expected[..size as usize]
                .copy_from_slice(&(1u64 << (sigusr1 - 1)).to_ne_bytes()[..size as usize]);
            assert_eq!(bytes, expected, "{size}");
        }
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        for (set, size, result) in [
            (pending, 9, einval),
            (DATA, 8, Errno::EFAULT.result()),
            (user_end - 4, 8, Errno::EFAULT.result()),
            (user_end, 0, 0),
        ] {
            assert_eq!(
                call(&process, number::RT_SIGPENDING, [set, size, 0, 0, 0, 0]),
                result
            );
        }
        let mut context = called(number::RT_SIGPROCMASK, [1, writable, 0, 8, 0, 0]);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: sigusr1 as u32
            })
        );
        let process = process_with_iovecs(&kernel, b"ab", &[]);
        let pid = u64::from(process.id().get());
        let mut context = called(number::TGKILL, [pid, pid, sigabrt, 0, 0, 0]);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: sigabrt as u32
            })
        );
    }

    #[test]
    fn unknown_calls_return_enosys_and_exit_ends_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_data(&kernel, b"ab");
        let mut context = called(1000, [0; 6]);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_result(), Errno::ENOSYS.result());
        for exit in [number::EXIT, number::EXIT_GROUP] {
            let process = process_with_data(&kernel, b"ab");
            let mut context = called(exit, [0x1_2345, 0, 0, 0, 0, 0]);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Break(())
            );
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
            let exit = context.simulate_exception(exception);
            handle_first(process, &mut context, exit)
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
        let process = with_region(1);
        assert_eq!(fault(&process, region + 4096, 0), ControlFlow::Break(()));
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
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

    /// An exception of `kind` at 0x40_0000, with the faulting address `address`, if any, whose
    /// raw cause is `status` with the architecture's `class` in it as the HAL reports it:
    /// AArch64's `ESR_EL1.EC`, RISC-V's `scause`; x86_64's error code is `status` alone (the
    /// kind tells its vectors apart).
    fn exception(kind: ExceptionKind, class: u64, status: u64, address: Option<u64>) -> Exception {
        let code = match () {
            _ if cfg!(target_arch = "x86_64") => status,
            _ if cfg!(target_arch = "aarch64") => class << 26 | status,
            _ => class | status,
        };
        Exception {
            kind,
            name: "test exception",
            code,
            pc: 0x40_0000,
            address,
        }
    }

    #[test]
    fn faults_without_handlers_end_the_process_with_linux_signals_and_interrupts_resume() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut context = called(1000, [0; 6]);
        let process = process_with_data(&kernel, b"ab");
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Interrupt),
            ControlFlow::Continue(())
        );
        use ExceptionKind::*;
        #[cfg(target_arch = "x86_64")]
        let faults = [
            (Arithmetic, 0, 0, signal::SIGFPE),
            (Debug, 1, 0, signal::SIGTRAP),
            (Breakpoint, 3, 0, signal::SIGTRAP),
            (IllegalInstruction, 6, 0, signal::SIGILL),
            (Segment, 12, 0, signal::SIGBUS),
            (Other, 13, 0, signal::SIGSEGV),
            (DataFault, 14, 0, signal::SIGSEGV),
            (InstructionFault, 14, 0x10, signal::SIGSEGV),
            (Alignment, 17, 0, signal::SIGBUS),
        ];
        #[cfg(target_arch = "aarch64")]
        let faults = [
            (IllegalInstruction, 0x00, 0, signal::SIGILL),
            (Other, 0x0e, 0, signal::SIGILL),
            (InstructionFault, 0x20, 0, signal::SIGSEGV),
            (Alignment, 0x22, 0, signal::SIGBUS),
            (DataFault, 0x24, 0, signal::SIGSEGV),
            (DataFault, 0x24, 0x21, signal::SIGBUS),
            (DataFault, 0x24, 0x10, signal::SIGBUS),
            (Alignment, 0x26, 0, signal::SIGBUS),
            (Breakpoint, 0x3c, 0, signal::SIGTRAP),
        ];
        #[cfg(target_arch = "riscv64")]
        let faults = [
            (Alignment, 0, 0, signal::SIGBUS),
            (InstructionFault, 1, 0, signal::SIGSEGV),
            (IllegalInstruction, 2, 0, signal::SIGILL),
            (Breakpoint, 3, 0, signal::SIGTRAP),
            (DataFault, 13, 0, signal::SIGSEGV),
            (Other, 9, 0, signal::SIGILL),
        ];
        for (kind, class, status, signal) in faults {
            let process = process_with_data(&kernel, b"ab");
            let exception = exception(kind, class, status, None);
            assert_eq!(
                {
                    let exit = context.simulate_exception(exception);
                    handle_first(&process, &mut context, exit)
                },
                ControlFlow::Break(()),
                "{kind:?} {class:#x}"
            );
            assert_eq!(process.exit_status(), Some(ProcessExit::Killed { signal }));
            process.exit(ProcessExit::Exited(0));
            assert_eq!(process.exit_status(), Some(ProcessExit::Killed { signal }));
        }
    }

    /// A process with the stack pages below [`HANDLER_STACK_TOP`] and an alternate stack's
    /// below [`ALTERNATE_STACK_TOP`], writable, a read-only region at [`READ_ONLY`] with no
    /// page mapped yet, and its pid.
    fn process_with_stacks(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
    ) -> (Arc<Process<P>>, u64) {
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        for top in [HANDLER_STACK_TOP, ALTERNATE_STACK_TOP] {
            for page in 1..=3 {
                process
                    .map_new_page(top - page * FRAME_SIZE, Attributes::USER_READ_WRITE, &[])
                    .unwrap();
            }
        }
        process
            .add_region(
                READ_ONLY,
                READ_ONLY + FRAME_SIZE,
                Attributes::USER_READ_ONLY,
            )
            .unwrap();
        let process = Arc::new(process);
        let pid = u64::from(process.id().get());
        (process, pid)
    }

    const HANDLER_STACK_TOP: u64 = 0x70_0000;
    const ALTERNATE_STACK_TOP: u64 = 0x60_0000;
    const READ_ONLY: u64 = 0x50_0000;
    const HANDLER: u64 = 0x40_1000;
    const RESTORER: u64 = 0x40_2000;

    /// Sets `signal`'s action with `rt_sigaction` through [`DATA`]'s page of `process`.
    fn set_handler(process: &Arc<Process<P>>, signal: u64, flags: u64, mask: u64) {
        let words = if signals::HAS_RESTORER {
            [HANDLER, flags, RESTORER, mask]
        } else {
            [HANDLER, flags, mask, 0]
        };
        let action = HANDLER_STACK_TOP - 3 * FRAME_SIZE;
        process
            .write_user_memory(action, &words.map(u64::to_ne_bytes).concat())
            .unwrap();
        assert_eq!(
            call(process, number::RT_SIGACTION, [signal, action, 0, 8, 0, 0]),
            0
        );
    }

    /// Where the handler that `context` starts returns to, and its stack pointer as it
    /// returns there: x86_64's `ret` pops the return address from the stack.
    fn return_point(process: &Process<P>, context: &UserContext) -> (u64, u64) {
        let registers = context.registers();
        #[cfg(target_arch = "x86_64")]
        {
            let mut bytes = [0; 8];
            process.read_user_memory(registers.rsp, &mut bytes).unwrap();
            (u64::from_ne_bytes(bytes), registers.rsp + 8)
        }
        #[cfg(target_arch = "aarch64")]
        {
            let _ = process;
            (registers.x[30], registers.sp)
        }
        #[cfg(target_arch = "riscv64")]
        {
            let _ = process;
            (registers.x[1], registers.x[2])
        }
    }

    /// The handler `context` runs returns: at its return point, as `rt_sigreturn` (what the
    /// restorer and the trampoline do).
    fn handler_returns(process: &Arc<Process<P>>, context: &mut UserContext) -> ControlFlow<()> {
        let (to, stack) = return_point(process, context);
        let mut registers = context.registers();
        #[cfg(target_arch = "x86_64")]
        {
            registers.rsp = stack;
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = stack;
        #[cfg(target_arch = "aarch64")]
        {
            registers.x[19] = 0xdead;
        }
        #[cfg(target_arch = "riscv64")]
        {
            registers.x[9] = 0xdead;
        }
        #[cfg(target_arch = "x86_64")]
        {
            registers.rbx = 0xdead;
        }
        context.set_registers(&registers);
        context.simulate_syscall(number::RT_SIGRETURN, [0; 6], to + 4);
        handle_first(process, context, UserExit::Syscall)
    }

    #[test]
    fn handlers_run_on_linux_s_frames_and_return_with_rt_sigreturn() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (sigusr1, sigusr2) = (10u64, 12u64);
        let bit = |signal: u64| 1u64 << (signal - 1);
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        set_handler(&process, sigusr1, flags, bit(sigusr2));
        let stack = HANDLER_STACK_TOP - 0x30;
        let mut context = UserContext::new(0x40_0000, stack);
        let mut registers = context.registers();
        #[cfg(target_arch = "aarch64")]
        {
            registers.x[19] = 0x1919;
        }
        #[cfg(target_arch = "riscv64")]
        {
            registers.x[9] = 0x1919;
        }
        #[cfg(target_arch = "x86_64")]
        {
            registers.rbx = 0x1919;
        }
        context.set_registers(&registers);
        context.simulate_syscall(number::TGKILL, [pid, pid, sigusr1, 0, 0, 0], 0x40_0004);
        let mut interrupted = context.clone();
        interrupted.set_syscall_result(0);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        let [signal, info, ucontext, ..] = context.syscall_arguments();
        assert_eq!(signal, sigusr1);
        let frame = context.stack();
        if cfg!(target_arch = "x86_64") {
            assert_eq!((frame + 8) % 16, 0);
            assert!(frame + 128 < stack);
        } else {
            assert_eq!(frame % 16, 0);
            assert!(frame < stack);
        }
        let mut bytes = [0; 128];
        process.read_user_memory(info, &mut bytes).unwrap();
        let int =
            |bytes: &[u8], at: usize| i32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(int(&bytes, 0), sigusr1 as i32);
        assert_eq!(int(&bytes, 8), -6, "SI_TKILL");
        assert_eq!(int(&bytes, 16), pid as i32);
        let sigmask =
            ucontext - signal_frame::layout::UCONTEXT as u64 + signal_frame::layout::SIGMASK as u64;
        let mut mask = [0; 8];
        process.read_user_memory(sigmask, &mut mask).unwrap();
        assert_eq!(u64::from_ne_bytes(mask), 0, "the mask before the handler");
        let (to, _) = return_point(&process, &context);
        if signals::HAS_RESTORER {
            assert_eq!(to, RESTORER);
        } else {
            let trampoline = process.signals().lock().trampoline();
            assert_eq!(to, trampoline + 4);
            assert_eq!(
                process.region(trampoline).unwrap().attributes,
                Some(Attributes::USER_TEXT)
            );
        }
        let blocked = || first_thread_signals(&process).blocked;
        assert_eq!(blocked(), bit(sigusr1) | bit(sigusr2));
        process
            .write_user_memory(sigmask, &bit(sigusr2).to_ne_bytes())
            .unwrap();
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.registers(), interrupted.registers());
        assert_eq!(blocked(), bit(sigusr2));
        set_handler(&process, sigusr1, flags, 0);
        set_handler(&process, sigusr2, flags, 0);
        let set = HANDLER_STACK_TOP - 3 * FRAME_SIZE + 64;
        let both = bit(sigusr1) | bit(sigusr2);
        process.write_user_memory(set, &both.to_ne_bytes()).unwrap();
        assert_eq!(
            call(&process, number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0]),
            0
        );
        for signal in [sigusr2, sigusr1] {
            assert_eq!(
                call(&process, number::TGKILL, [pid, pid, signal, 0, 0, 0]),
                0
            );
        }
        let mut context = UserContext::new(0x40_0000, stack);
        context.simulate_syscall(number::RT_SIGPROCMASK, [1, set, 0, 8, 0, 0], 0x40_0004);
        let mut interrupted = context.clone();
        interrupted.set_syscall_result(0);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_arguments()[0], sigusr2);
        assert_eq!(blocked(), both);
        let first_frame = context.stack();
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(context.syscall_arguments()[0], sigusr1);
        assert!(context.stack() > first_frame);
        assert_eq!(blocked(), bit(sigusr1));
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.registers(), interrupted.registers());
        assert_eq!(blocked(), 0);
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn handlers_start_on_the_alternate_stack_and_bad_frames_are_sigsegv() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let sigusr1 = 10u64;
        let restorer = if signals::HAS_RESTORER {
            signals::flag::RESTORER
        } else {
            0
        };
        let size = 2 * FRAME_SIZE;
        let base = ALTERNATE_STACK_TOP - size;
        let stack_t = HANDLER_STACK_TOP - 3 * FRAME_SIZE + 128;
        let words = [base, 1 << 31, size];
        process
            .write_user_memory(stack_t, &words.map(u64::to_ne_bytes).concat())
            .unwrap();
        assert_eq!(
            call(&process, number::SIGALTSTACK, [stack_t, 0, 0, 0, 0, 0]),
            0
        );
        set_handler(&process, sigusr1, signals::flag::ONSTACK | restorer, 0);
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x40);
        context.simulate_syscall(number::KILL, [pid, sigusr1, 0, 0, 0, 0], 0x40_0004);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert!((base..ALTERNATE_STACK_TOP).contains(&context.stack()));
        let [_, info, ..] = context.syscall_arguments();
        let mut code = [0; 4];
        process.read_user_memory(info + 8, &mut code).unwrap();
        assert_eq!(i32::from_ne_bytes(code), 0, "SI_USER");
        let alternate = || first_thread_signals(&process).alternate_stack;
        assert_eq!(alternate().size, 0);
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!((alternate().base, alternate().size), (base, size));
        assert!(alternate().autodisarm);
        let mut context = UserContext::new(0x40_0000, 0x10_0000);
        context.simulate_syscall(number::KILL, [pid, sigusr1, 0, 0, 0, 0], 0x40_0004);
        set_handler(&process, sigusr1, restorer, 0);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
        let (process, _) = process_with_stacks(&kernel);
        let set = HANDLER_STACK_TOP - 3 * FRAME_SIZE;
        process
            .write_user_memory(set, &(1u64 << (signal::SIGSEGV - 1)).to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(&process, number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0]),
            0
        );
        let mut context = UserContext::new(0x40_0000, 0x10_0000);
        context.simulate_syscall(number::RT_SIGRETURN, [0; 6], 0x40_0004);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
        if cfg!(target_arch = "x86_64") {
            let (process, pid) = process_with_stacks(&kernel);
            set_handler(&process, sigusr1, 0, 0);
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x40);
            context.simulate_syscall(number::KILL, [pid, sigusr1, 0, 0, 0, 0], 0x40_0004);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Break(())
            );
            assert_eq!(
                process.exit_status(),
                Some(ProcessExit::Killed {
                    signal: signal::SIGSEGV
                })
            );
        }
    }
    /// Where the FP/SIMD part of the frame whose `ucontext_t` is at `ucontext` is: x86_64's
    /// image where `fpstate` points, the others' in the frame.
    fn fp_part_address(process: &Process<P>, ucontext: u64) -> u64 {
        let frame = ucontext - signal_frame::layout::UCONTEXT as u64;
        #[cfg(target_arch = "x86_64")]
        {
            let mut pointer = [0; 8];
            process
                .read_user_memory(frame + signal_frame::layout::FPSTATE as u64, &mut pointer)
                .unwrap();
            u64::from_ne_bytes(pointer)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = process;
            frame + signal_frame::layout::FP as u64
        }
    }

    #[test]
    fn frames_hold_the_fp_simd_registers_and_rt_sigreturn_loads_them() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        crate::thread::start::<P>().unwrap();
        let sigusr1 = 10u64;
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        let start_handler = |process: &Arc<Process<P>>, pid: u64| {
            set_handler(process, sigusr1, flags, 0);
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x40);
            context.simulate_syscall(number::KILL, [pid, sigusr1, 0, 0, 0, 0], 0x40_0004);
            assert_eq!(
                handle_first(process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            let [_, _, ucontext, ..] = context.syscall_arguments();
            (context, ucontext)
        };
        let (process, pid) = process_with_stacks(&kernel);
        let (mut context, ucontext) = start_handler(&process, pid);
        let fp = fp_part_address(&process, ucontext);
        let mut part = signal_frame::initial_fp_part();
        part.fill(0xa5);
        process.read_user_memory(fp, &mut part).unwrap();
        assert_eq!(part, signal_frame::initial_fp_part());
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(fp % 64, 0);
            assert_eq!(u16::from_ne_bytes([part[0], part[1]]), 0x037f);
            assert_eq!(u32::from_ne_bytes(part[24..28].try_into().unwrap()), 0x1f80);
        }
        #[cfg(target_arch = "aarch64")]
        {
            let mut header = [0; 8];
            process.read_user_memory(fp - 8, &mut header).unwrap();
            assert_eq!(header, [0x01, 0x80, 0x50, 0x46, 0x10, 0x02, 0, 0]);
        }
        part[part.len() - 1] = 1;
        process.write_user_memory(fp, &part).unwrap();
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(process.exit_status(), None);
        #[cfg(target_arch = "x86_64")]
        {
            let fpstate = ucontext - signal_frame::layout::UCONTEXT as u64
                + signal_frame::layout::FPSTATE as u64;
            let (process, pid) = process_with_stacks(&kernel);
            let (mut context, _) = start_handler(&process, pid);
            process.write_user_memory(fpstate, &[0; 8]).unwrap();
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            assert_eq!(process.exit_status(), None);
            let (process, pid) = process_with_stacks(&kernel);
            let (mut context, _) = start_handler(&process, pid);
            let misaligned = fp + 8;
            process
                .write_user_memory(fpstate, &misaligned.to_ne_bytes())
                .unwrap();
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Break(())
            );
            assert_eq!(
                process.exit_status(),
                Some(ProcessExit::Killed {
                    signal: signal::SIGSEGV
                })
            );
        }
    }

    /// The exception class of a page fault (x86_64's `#PF`, AArch64's data abort from EL0,
    /// RISC-V's store page fault) and its code for a `write`, as the test platform encodes it
    /// (with AArch64's class in it).
    fn page_fault_exception(address: u64, write: bool) -> Exception {
        let class = match () {
            _ if cfg!(target_arch = "x86_64") => 14,
            _ if cfg!(target_arch = "aarch64") => 0x24,
            _ => 15,
        };
        let mut status = if write { crate::tests::FAULT_WRITE } else { 0 };
        if cfg!(target_arch = "aarch64") {
            status |= 1 << 25;
        }
        exception(ExceptionKind::DataFault, class, status, Some(address))
    }

    /// Sets `signal`'s action to `SIG_IGN` with `rt_sigaction`.
    fn ignore(process: &Arc<Process<P>>, signal: u64) {
        let action = HANDLER_STACK_TOP - 3 * FRAME_SIZE;
        let words = [signals::IGNORE, 0, 0, 0];
        process
            .write_user_memory(action, &words.map(u64::to_ne_bytes).concat())
            .unwrap();
        assert_eq!(
            call(process, number::RT_SIGACTION, [signal, action, 0, 8, 0, 0]),
            0
        );
    }

    /// The signal's frame's machine context, for the handler `context` starts: x86_64's
    /// `err`, `trapno` and `cr2`; AArch64's `fault_address`, then the record after the FP/SIMD
    /// one (its magic number and size) and the syndrome it holds; RISC-V's nothing.
    fn reported_trap(process: &Process<P>, context: &UserContext) -> [u64; 3] {
        let [_, _, ucontext, ..] = context.syscall_arguments();
        let frame = ucontext - signal_frame::layout::UCONTEXT as u64;
        let word = |offset: usize| {
            let mut bytes = [0; 8];
            process
                .read_user_memory(frame + offset as u64, &mut bytes)
                .unwrap();
            u64::from_ne_bytes(bytes)
        };
        #[cfg(target_arch = "x86_64")]
        {
            use signal_frame::layout::{CR2, ERR, TRAPNO};
            [word(ERR), word(TRAPNO), word(CR2)]
        }
        #[cfg(target_arch = "aarch64")]
        {
            use signal_frame::layout::{ESR, FAULT_ADDRESS};
            [word(FAULT_ADDRESS), word(ESR), word(ESR + 8)]
        }
        #[cfg(target_arch = "riscv64")]
        {
            let _ = word;
            [0; 3]
        }
    }

    #[test]
    fn faults_run_handlers_with_their_address_and_return_to_the_faulting_instruction() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let sigsegv = u64::from(signal::SIGSEGV);
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        set_handler(&process, sigsegv, flags, 0);
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        for (address, write, code) in [
            (0x1000, false, fault::code::SEGV_MAPERR),
            (READ_ONLY + 8, true, fault::code::SEGV_ACCERR),
            (user_end + 0x1000, false, fault::code::SEGV_MAPERR),
        ] {
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            let interrupted = context.clone();
            let exception = page_fault_exception(address, write);
            assert_eq!(
                {
                    let exit = context.simulate_exception(exception);
                    handle_first(&process, &mut context, exit)
                },
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            let [signal, info, ..] = context.syscall_arguments();
            assert_eq!(signal, sigsegv);
            let mut bytes = [0; 24];
            process.read_user_memory(info, &mut bytes).unwrap();
            let int = |at: usize| i32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
            assert_eq!((int(0), int(8)), (sigsegv as i32, code), "{address:#x}");
            assert_eq!(u64::from_ne_bytes(bytes[16..].try_into().unwrap()), address);
            let kernel_half = address >= user_end;
            let reported = reported_trap(&process, &context);
            #[cfg(target_arch = "x86_64")]
            assert_eq!(
                reported,
                [exception.code | u64::from(kernel_half), 14, address],
                "err (protection set past user space), trapno, cr2"
            );
            #[cfg(target_arch = "aarch64")]
            {
                let esr = if kernel_half {
                    fault::sanitized_syndrome(exception.code)
                } else {
                    exception.code
                };
                assert_eq!(reported, [address, 0x10_4553_5201, esr]);
            }
            #[cfg(target_arch = "riscv64")]
            let _ = (reported, kernel_half);
            assert_eq!(first_thread_signals(&process).blocked, 1 << (sigsegv - 1));
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            assert_eq!(context.registers(), interrupted.registers());
        }
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
        context.simulate_syscall(number::TGKILL, [pid, pid, sigsegv, 0, 0, 0], 0x40_0004);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(reported_trap(&process, &context), [0; 3]);
        assert_eq!(process.exit_status(), None);
        let mut context = UserContext::new(0x40_0000, 0x10_0000);
        let exception = page_fault_exception(0x1000, false);
        assert_eq!(
            {
                let exit = context.simulate_exception(exception);
                handle_first(&process, &mut context, exit)
            },
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
    }

    #[test]
    fn a_failed_frame_s_sigsegv_is_delivered_before_a_lower_numbered_pending_signal() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (sighup, sigusr1) = (1u64, 10u64);
        let restorer = if signals::HAS_RESTORER {
            signals::flag::RESTORER
        } else {
            0
        };
        set_handler(&process, sighup, restorer, 0);
        let set = HANDLER_STACK_TOP - 3 * FRAME_SIZE;
        let both = 1u64 << (sighup - 1) | 1u64 << (sigusr1 - 1);
        process.write_user_memory(set, &both.to_ne_bytes()).unwrap();
        let (block, unblock) = (0, 1);
        assert_eq!(
            call(&process, number::RT_SIGPROCMASK, [block, set, 0, 8, 0, 0]),
            0
        );
        for signal in [sighup, sigusr1] {
            assert_eq!(call(&process, number::KILL, [pid, signal, 0, 0, 0, 0]), 0);
        }
        let unmapped_stack = 0x10_0000;
        let mut context = UserContext::new(0x40_0000, unmapped_stack);
        context.simulate_syscall(
            number::RT_SIGPROCMASK,
            [unblock, set, 0, 8, 0, 0],
            0x40_0004,
        );
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGSEGV
            })
        );
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn aarch64_breakpoints_and_undefined_instructions_report_no_fault_address_or_syndrome() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (ec_unknown, ec_brk64) = (0x00, 0x3c);
        for (signal, kind, class) in [
            (
                signal::SIGILL,
                ExceptionKind::IllegalInstruction,
                ec_unknown,
            ),
            (signal::SIGTRAP, ExceptionKind::Breakpoint, ec_brk64),
        ] {
            set_handler(&process, u64::from(signal), signals::flag::SIGINFO, 0);
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            let exit = context.simulate_exception(exception(kind, class, 1 << 25, None));
            assert_eq!(
                handle_first(&process, &mut context, exit),
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            let [delivered, info, ..] = context.syscall_arguments();
            assert_eq!(delivered, u64::from(signal));
            let mut si_addr = [0; 8];
            process.read_user_memory(info + 16, &mut si_addr).unwrap();
            assert_eq!(u64::from_ne_bytes(si_addr), 0x40_0000);
            assert_eq!(reported_trap(&process, &context), [0; 3]);
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
        }
    }

    const SCRATCH: u64 = HANDLER_STACK_TOP - 2 * FRAME_SIZE;
    const UNMAPPED: u64 = 0x1000;

    fn block_signals(process: &Arc<Process<P>>, set: u64) {
        process
            .write_user_memory(SCRATCH + 0x300, &set.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(
                process,
                number::RT_SIGPROCMASK,
                [0, SCRATCH + 0x300, 0, 8, 0, 0]
            ),
            0
        );
    }

    fn write_siginfo(process: &Process<P>, address: u64, code: i32, value: u64) {
        let mut info = [0u8; signal_frame::INFO_SIZE];
        info[..4].copy_from_slice(&99i32.to_ne_bytes());
        info[8..12].copy_from_slice(&code.to_ne_bytes());
        info[24..32].copy_from_slice(&value.to_ne_bytes());
        info[100] = 0xee;
        process.write_user_memory(address, &info).unwrap();
    }

    fn read_siginfo(process: &Process<P>, address: u64) -> (i32, i32, u64, bool) {
        let mut info = [0u8; signal_frame::INFO_SIZE];
        process.read_user_memory(address, &mut info).unwrap();
        let int = |at: usize| i32::from_ne_bytes(info[at..at + 4].try_into().unwrap());
        let value = u64::from_ne_bytes(info[24..32].try_into().unwrap());
        let tail_zero = info[signals::KEPT_INFO_SIZE..]
            .iter()
            .all(|&byte| byte == 0);
        (int(0), int(8), value, tail_zero)
    }

    #[test]
    fn signals_are_queued_with_their_info_and_linux_s_errors_in_order() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (eperm, esrch, einval, efault) = (
            Errno::EPERM.result(),
            Errno::ESRCH.result(),
            Errno::EINVAL.result(),
            Errno::EFAULT.result(),
        );
        let rt = 40u64;
        block_signals(&process, 1 << (rt - 1) | 1 << 9);
        let (user, tkill, queue) = (SCRATCH, SCRATCH + 0x80, SCRATCH + 0x100);
        write_siginfo(&process, user, signals::code::USER, 0);
        write_siginfo(&process, tkill, signals::code::TKILL, 0);
        write_siginfo(&process, queue, signals::code::QUEUE, 0x77);
        let other = pid + 1;
        let sigqueueinfo = |tgid, signal, info| {
            call(
                &process,
                number::RT_SIGQUEUEINFO,
                [tgid, signal, info, 0, 0, 0],
            )
        };
        for (tgid, signal, info, result) in [
            (other, 65, UNMAPPED, efault),
            (other, 65, user, eperm),
            (other, 65, tkill, eperm),
            (other, 65, queue, esrch),
            (0, rt, queue, esrch),
            (1 << 32 | other, rt, queue, esrch),
            (pid, 65, queue, einval),
            (pid, 0, queue, 0),
            (1 << 32 | pid, 10, user, 0),
        ] {
            assert_eq!(sigqueueinfo(tgid, signal, info), result, "{tgid} {signal}");
        }
        let tgsigqueueinfo = |tgid, tid, signal, info| {
            call(
                &process,
                number::RT_TGSIGQUEUEINFO,
                [tgid, tid, signal, info, 0, 0],
            )
        };
        for (tgid, tid, signal, info, result) in [
            (0, other, 65, UNMAPPED, efault),
            (0, pid, 65, queue, einval),
            (pid, u64::from(u32::MAX), 65, queue, einval),
            (pid, other, 65, tkill, eperm),
            (other, pid, 65, user, esrch),
            (pid, other, 65, queue, esrch),
            (pid, pid, 65, queue, einval),
            (pid, pid, 0, queue, 0),
        ] {
            assert_eq!(
                tgsigqueueinfo(tgid, tid, signal, info),
                result,
                "{tgid} {tid} {signal}"
            );
        }
        assert_eq!(sigqueueinfo(pid, rt, queue), 0);
        write_siginfo(&process, queue, signals::code::QUEUE, 0x78);
        assert_eq!(tgsigqueueinfo(pid, pid, rt, queue), 0);
        let set = SCRATCH + 0x200;
        let received = SCRATCH + 0x400;
        let timeout = SCRATCH + 0x280;
        process
            .write_user_memory(set, &u64::MAX.to_ne_bytes())
            .unwrap();
        process.write_user_memory(timeout, &[0; 16]).unwrap();
        let sigtimedwait = |info, timeout| {
            call(
                &process,
                number::RT_SIGTIMEDWAIT,
                [set, info, timeout, 8, 0, 0],
            )
        };
        assert_eq!(sigtimedwait(received, timeout), 10);
        assert_eq!(
            read_siginfo(&process, received),
            (10, signals::code::USER, 0, true)
        );
        assert_eq!(sigtimedwait(received, timeout), rt);
        assert_eq!(
            read_siginfo(&process, received),
            (rt as i32, signals::code::QUEUE, 0x77, true)
        );
        assert_eq!(sigtimedwait(0, timeout), rt);
        assert_eq!(sigtimedwait(received, timeout), Errno::EAGAIN.result());
    }

    #[test]
    fn sigtimedwait_checks_in_linux_s_order_waits_and_loses_a_signal_it_cannot_report() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (einval, efault, eagain) = (
            Errno::EINVAL.result(),
            Errno::EFAULT.result(),
            Errno::EAGAIN.result(),
        );
        let set = SCRATCH;
        let timeout = SCRATCH + 0x10;
        let received = SCRATCH + 0x100;
        process
            .write_user_memory(set, &u64::MAX.to_ne_bytes())
            .unwrap();
        let wait_for = |seconds: i64, nanoseconds: i64| {
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&seconds.to_ne_bytes());
            bytes[8..].copy_from_slice(&nanoseconds.to_ne_bytes());
            process.write_user_memory(timeout, &bytes).unwrap();
        };
        let sigtimedwait = |set, info, timeout, size| {
            call(
                &process,
                number::RT_SIGTIMEDWAIT,
                [set, info, timeout, size, 0, 0],
            )
        };
        wait_for(-1, 0);
        for (set, timeout, size, result) in [
            (UNMAPPED, UNMAPPED, 7, einval),
            (UNMAPPED, UNMAPPED, 8, efault),
            (set, UNMAPPED, 8, efault),
            (set, timeout, 8, einval),
        ] {
            assert_eq!(sigtimedwait(set, received, timeout, size), result);
        }
        wait_for(0, 1_000_000_000);
        assert_eq!(sigtimedwait(set, received, timeout, 8), einval);
        wait_for(0, 0);
        assert_eq!(sigtimedwait(set, received, timeout, 8), eagain);
        wait_for(0, 2_000_000);
        let start = <P as Platform>::clock_ticks();
        assert_eq!(sigtimedwait(set, received, timeout, 8), eagain);
        let waited = <P as Platform>::clock_ticks() - start;
        assert!(
            waited * 1_000_000_000 >= 2_000_000 * <P as Platform>::clock_frequency(),
            "{waited}"
        );
        block_signals(&process, 1 << 9);
        assert_eq!(call(&process, number::TGKILL, [pid, pid, 10, 0, 0, 0]), 0);
        assert_eq!(sigtimedwait(set, READ_ONLY, timeout, 8), efault);
        let pending = SCRATCH + 0x200;
        assert_eq!(
            call(&process, number::RT_SIGPENDING, [pending, 8, 0, 0, 0, 0]),
            0
        );
        let mut bytes = [0xffu8; 8];
        process.read_user_memory(pending, &mut bytes).unwrap();
        assert_eq!(u64::from_ne_bytes(bytes), 0);
    }

    #[test]
    fn a_full_signal_queue_refuses_real_time_signals_but_kill_s() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let rt = 50u64;
        block_signals(&process, 1 << (rt - 1));
        let limit = SCRATCH;
        process.write_user_memory(limit, &[0; 16]).unwrap();
        assert_eq!(
            call(
                &process,
                number::SETRLIMIT,
                [u64::from(limits::resource::SIGPENDING), limit, 0, 0, 0, 0]
            ),
            0
        );
        let info = SCRATCH + 0x100;
        write_siginfo(&process, info, signals::code::QUEUE, 1);
        let eagain = Errno::EAGAIN.result();
        assert_eq!(
            call(&process, number::RT_SIGQUEUEINFO, [pid, rt, info, 0, 0, 0]),
            eagain
        );
        assert_eq!(
            call(&process, number::TGKILL, [pid, pid, rt, 0, 0, 0]),
            eagain
        );
        assert_eq!(first_thread_signals(&process).pending, 0);
        assert_eq!(call(&process, number::KILL, [pid, rt, 0, 0, 0, 0]), 0);
        assert_eq!(first_thread_signals(&process).pending, 1 << (rt - 1));
    }

    #[test]
    fn a_queued_signal_s_handler_gets_its_info() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let rt = 34u64;
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        set_handler(&process, rt, flags, 0);
        let queued = SCRATCH;
        write_siginfo(&process, queued, signals::code::QUEUE, 0x1234_5678_9abc);
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
        context.simulate_syscall(
            number::RT_SIGQUEUEINFO,
            [pid, rt, queued, 0, 0, 0],
            0x40_0004,
        );
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        let [signal, info, ..] = context.syscall_arguments();
        assert_eq!(signal, rt);
        assert_eq!(
            read_siginfo(&process, info),
            (rt as i32, signals::code::QUEUE, 0x1234_5678_9abc, true)
        );
    }

    /// The mask the frame of the handler `context` runs saved.
    fn saved_mask(process: &Process<P>, context: &UserContext) -> u64 {
        let ucontext = context.syscall_arguments()[2];
        let sigmask =
            ucontext - signal_frame::layout::UCONTEXT as u64 + signal_frame::layout::SIGMASK as u64;
        let mut bytes = [0; 8];
        process.read_user_memory(sigmask, &mut bytes).unwrap();
        u64::from_ne_bytes(bytes)
    }

    #[test]
    fn sigsuspend_runs_handlers_under_its_mask_then_restores_the_mask_it_replaced() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let bit = |signal: u64| 1u64 << (signal - 1);
        let (sighup, sigkill, sigusr1, sigusr2, sigterm, sigchld) =
            (1u64, 9u64, 10u64, 12u64, 15u64, 17u64);
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        set_handler(&process, sigusr1, flags, bit(sighup));
        set_handler(&process, sigusr2, flags, 0);
        let before = bit(sigusr1) | bit(sigusr2) | bit(sigterm) | bit(sigchld);
        block_signals(&process, before);
        for signal in [sigchld, sigusr2, sigusr1] {
            assert_eq!(
                call(&process, number::TGKILL, [pid, pid, signal, 0, 0, 0]),
                0
            );
        }
        let blocked = || first_thread_signals(&process).blocked;
        let mask = SCRATCH + 0x100;
        let during = bit(sigterm);
        process
            .write_user_memory(mask, &(during | bit(sigkill)).to_ne_bytes())
            .unwrap();
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
        context.simulate_syscall(number::RT_SIGSUSPEND, [mask, 8, 0, 0, 0, 0], 0x40_0004);
        let mut interrupted = context.clone();
        interrupted.set_syscall_result(Errno::EINTR.result());
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(context.syscall_arguments()[0], sigusr2);
        assert_eq!(first_thread_signals(&process).pending, 0);
        let in_sigusr1 = during | bit(sighup) | bit(sigusr1);
        assert_eq!(saved_mask(&process, &context), in_sigusr1);
        assert_eq!(blocked(), in_sigusr1 | bit(sigusr2));
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_arguments()[0], sigusr1);
        assert_eq!(saved_mask(&process, &context), before);
        assert_eq!(blocked(), in_sigusr1);
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.registers(), interrupted.registers());
        assert_eq!(blocked(), before);
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn sigsuspend_checks_its_set_first_and_ends_the_process_by_a_default_action() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let sigterm = 15u64;
        let before = 1 << (sigterm - 1);
        block_signals(&process, before);
        assert_eq!(
            call(&process, number::TGKILL, [pid, pid, sigterm, 0, 0, 0]),
            0
        );
        let mask = SCRATCH + 0x100;
        process
            .write_user_memory(mask, &0u64.to_ne_bytes())
            .unwrap();
        for (set, size, errno) in [
            (UNMAPPED, 7, Errno::EINVAL),
            (mask, 16, Errno::EINVAL),
            (UNMAPPED, 8, Errno::EFAULT),
        ] {
            assert_eq!(
                call(&process, number::RT_SIGSUSPEND, [set, size, 0, 0, 0, 0]),
                errno.result()
            );
            assert_eq!(first_thread_signals(&process).blocked, before);
        }
        let mut context = called(number::RT_SIGSUSPEND, [mask, 8, 0, 0, 0, 0]);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: sigterm as u32
            })
        );
    }

    #[test]
    fn ppoll_s_mask_lets_a_signal_it_unblocks_interrupt_a_wait_that_finds_nothing_ready() {
        use poll::event::OUT;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let bit = |signal: u64| 1u64 << (signal - 1);
        let (sigusr1, sigchld) = (10u64, 17u64);
        let flags = signals::flag::SIGINFO
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            };
        set_handler(&process, sigusr1, flags, 0);
        let before = bit(sigusr1) | bit(sigchld);
        block_signals(&process, before);
        for signal in [sigchld, sigusr1] {
            assert_eq!(
                call(&process, number::TGKILL, [pid, pid, signal, 0, 0, 0]),
                0
            );
        }
        let (fds, mask, timeout) = (SCRATCH + 0x100, SCRATCH + 0x200, SCRATCH + 0x280);
        process
            .write_user_memory(fds, &pollfds(&[(1, OUT)]))
            .unwrap();
        process
            .write_user_memory(mask, &0u64.to_ne_bytes())
            .unwrap();
        process.write_user_memory(timeout, &[0; 16]).unwrap();
        let thread = || first_thread_signals(&process);
        assert_eq!(
            call(&process, number::PPOLL, [fds, 1, timeout, mask, 8, 0]),
            1
        );
        assert_eq!((thread().blocked, thread().pending), (before, before));
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
        context.simulate_syscall(number::PPOLL, [0, 0, timeout, mask, 8, 0], 0x40_0004);
        let mut interrupted = context.clone();
        interrupted.set_syscall_result(Errno::EINTR.result());
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(context.syscall_arguments()[0], sigusr1);
        assert_eq!(saved_mask(&process, &context), before);
        assert_eq!((thread().blocked, thread().pending), (bit(sigusr1), 0));
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.registers(), interrupted.registers());
        assert_eq!(thread().blocked, before);
        assert_eq!(
            call(
                &process,
                number::PPOLL,
                [fds, u64::MAX, timeout, mask, 8, 0]
            ),
            Errno::EINVAL.result()
        );
        assert_eq!(thread().blocked, before);
    }

    #[test]
    fn ppoll_s_mask_lets_an_ignored_signal_go_and_a_default_action_end_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (sigchld, sigterm) = (17u64, 15u64);
        for (signal, wait_nanoseconds) in [(sigchld, 2_000_000u64), (sigterm, 0)] {
            let (process, pid) = process_with_stacks(&kernel);
            let before = 1 << (signal - 1);
            block_signals(&process, before);
            assert_eq!(
                call(&process, number::TGKILL, [pid, pid, signal, 0, 0, 0]),
                0
            );
            let (mask, timeout) = (SCRATCH + 0x200, SCRATCH + 0x280);
            process
                .write_user_memory(mask, &0u64.to_ne_bytes())
                .unwrap();
            let mut wait = [0u8; 16];
            wait[8..].copy_from_slice(&wait_nanoseconds.to_ne_bytes());
            process.write_user_memory(timeout, &wait).unwrap();
            let mut context = called(number::PPOLL, [0, 0, timeout, mask, 8, 0]);
            let flow = handle_first(&process, &mut context, UserExit::Syscall);
            if signal == sigchld {
                assert_eq!(flow, ControlFlow::Continue(()));
                assert_eq!(context.syscall_result(), 0);
                let mut left = [0xffu8; 16];
                process.read_user_memory(timeout, &mut left).unwrap();
                assert_eq!(left, [0; 16]);
                let thread = first_thread_signals(&process);
                assert_eq!((thread.blocked, thread.pending), (before, 0));
            } else {
                assert_eq!(flow, ControlFlow::Break(()));
                assert_eq!(
                    process.exit_status(),
                    Some(ProcessExit::Killed {
                        signal: signal as u32
                    })
                );
            }
        }
    }

    #[test]
    fn ppoll_interrupted_by_an_ignored_signal_fails_with_eintr_if_its_timeout_is_read_only() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let mut two_milliseconds = [0u8; 16];
        two_milliseconds[8..].copy_from_slice(&2_000_000u64.to_ne_bytes());
        let process = process_with_iovecs(&kernel, &two_milliseconds, &[]);
        let pid = u64::from(process.id().get());
        let (timeout, mask, set) = (DATA, DATA + 4096 + 64, DATA + 4096 + 72);
        let sigchld = 17u64;
        let before = 1u64 << (sigchld - 1);
        process
            .write_user_memory(set, &before.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(&process, number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::TGKILL, [pid, pid, sigchld, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::PPOLL, [0, 0, timeout, mask, 8, 0]),
            Errno::EINTR.result()
        );
        let thread = first_thread_signals(&process);
        assert_eq!((thread.blocked, thread.pending), (before, 0));
        let mut left = [0u8; 16];
        process.read_user_memory(timeout, &mut left).unwrap();
        assert_eq!(left, two_milliseconds);
    }

    fn read_timespec(process: &Process<P>, address: u64) -> (u64, u64) {
        let mut bytes = [0u8; 16];
        process.read_user_memory(address, &mut bytes).unwrap();
        let [seconds, nanoseconds] =
            [&bytes[..8], &bytes[8..]].map(|half| u64::from_ne_bytes(half.try_into().unwrap()));
        (seconds, nanoseconds)
    }

    fn write_timespec(process: &Process<P>, address: u64, seconds: i64, nanoseconds: i64) {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&seconds.to_ne_bytes());
        bytes[8..].copy_from_slice(&nanoseconds.to_ne_bytes());
        process.write_user_memory(address, &bytes).unwrap();
    }

    fn nanoseconds((seconds, nanoseconds): (u64, u64)) -> u64 {
        seconds * 1_000_000_000 + nanoseconds
    }

    #[test]
    fn clocks_read_the_monotonic_time_and_refuse_the_clocks_they_do_not_answer_for() {
        use time::clock;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let tp = SCRATCH + 0x100;
        let mut last = None;
        for id in [
            clock::REALTIME,
            clock::MONOTONIC,
            clock::MONOTONIC_RAW,
            clock::REALTIME_COARSE,
            clock::MONOTONIC_COARSE,
            clock::BOOTTIME,
            clock::TAI,
        ] {
            let id = u64::from(id as u32);
            assert_eq!(
                call(&process, number::CLOCK_GETTIME, [id, tp, 0, 0, 0, 0]),
                0
            );
            let (seconds, nanoseconds) = read_timespec(&process, tp);
            assert!(nanoseconds < 1_000_000_000);
            let now = self::nanoseconds((seconds, nanoseconds));
            assert!(last.is_none_or(|last| now > last), "clock {id}");
            last = Some(now);
            assert_eq!(
                call(&process, number::CLOCK_GETRES, [id, tp, 0, 0, 0, 0]),
                0
            );
            assert_eq!(read_timespec(&process, tp), (0, 1));
            assert_eq!(call(&process, number::CLOCK_GETRES, [id, 0, 0, 0, 0, 0]), 0);
            for (call_number, unwritable) in [
                (number::CLOCK_GETTIME, READ_ONLY),
                (number::CLOCK_GETTIME, UNMAPPED),
                (number::CLOCK_GETRES, READ_ONLY),
            ] {
                assert_eq!(
                    call(&process, call_number, [id, unwritable, 0, 0, 0, 0]),
                    Errno::EFAULT.result()
                );
            }
        }
        for id in [
            clock::PROCESS_CPUTIME_ID,
            clock::THREAD_CPUTIME_ID,
            clock::REALTIME_ALARM,
            clock::BOOTTIME_ALARM,
            10,
            12,
            -2,
            -6,
        ] {
            let id = 0xffff_ffff_0000_0000 | u64::from(id as u32);
            for call_number in [number::CLOCK_GETTIME, number::CLOCK_GETRES] {
                assert_eq!(
                    call(&process, call_number, [id, UNMAPPED, 0, 0, 0, 0]),
                    Errno::EINVAL.result()
                );
            }
        }
    }

    #[test]
    fn gettimeofday_reads_the_wall_clock_in_microseconds_with_utc() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (tv, tz, tp) = (SCRATCH + 0x100, SCRATCH + 0x110, SCRATCH + 0x120);
        process.write_user_memory(tz, &[0xff; 8]).unwrap();
        assert_eq!(
            call(&process, number::CLOCK_GETTIME, [0, tp, 0, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::GETTIMEOFDAY, [tv, tz, 0, 0, 0, 0]),
            0
        );
        let before = nanoseconds(read_timespec(&process, tp));
        let (seconds, microseconds) = read_timespec(&process, tv);
        assert!(microseconds < 1_000_000);
        assert!((seconds * 1_000_000 + microseconds) * 1000 >= before / 1000 * 1000);
        let mut zone = [0xffu8; 8];
        process.read_user_memory(tz, &mut zone).unwrap();
        assert_eq!(zone, [0; 8]);
        assert_eq!(call(&process, number::GETTIMEOFDAY, [0, 0, 0, 0, 0, 0]), 0);
        for (tv, tz) in [(READ_ONLY, 0), (0, READ_ONLY), (tv, UNMAPPED)] {
            assert_eq!(
                call(&process, number::GETTIMEOFDAY, [tv, tz, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
        #[cfg(target_arch = "x86_64")]
        {
            let tloc = SCRATCH + 0x130;
            let seconds = call(&process, number::TIME, [tloc, 0, 0, 0, 0, 0]);
            let mut written = [0u8; 8];
            process.read_user_memory(tloc, &mut written).unwrap();
            assert_eq!(u64::from_ne_bytes(written), seconds);
            assert_eq!(call(&process, number::TIME, [0, 0, 0, 0, 0, 0]), seconds);
            assert_eq!(
                call(&process, number::TIME, [READ_ONLY, 0, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
        }
    }

    #[test]
    fn nanosleep_never_wakes_early_and_checks_its_request_as_linux() {
        use time::{TIMER_ABSTIME, clock};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (req, tp) = (SCRATCH + 0x100, SCRATCH + 0x120);
        let now = || {
            assert_eq!(
                call(&process, number::CLOCK_GETTIME, [1, tp, 0, 0, 0, 0]),
                0
            );
            nanoseconds(read_timespec(&process, tp))
        };
        let monotonic = u64::from(clock::MONOTONIC as u32);
        for (call_number, arguments) in [
            (number::NANOSLEEP, [req, 0, 0, 0, 0, 0]),
            (number::CLOCK_NANOSLEEP, [monotonic, 0, req, 0, 0, 0]),
            (number::CLOCK_NANOSLEEP, [0, 0xfe, req, 0, 0, 0]),
        ] {
            write_timespec(&process, req, 0, 2_000_001);
            let before = now();
            assert_eq!(call(&process, call_number, arguments), 0);
            assert!(now() - before >= 2_000_001);
            for (seconds, nanoseconds) in [(0, 1_000_000_000), (0, -1), (-1, 0)] {
                write_timespec(&process, req, seconds, nanoseconds);
                assert_eq!(
                    call(&process, call_number, arguments),
                    Errno::EINVAL.result()
                );
            }
        }
        for (call_number, arguments) in [
            (number::NANOSLEEP, [UNMAPPED, 0, 0, 0, 0, 0]),
            (number::CLOCK_NANOSLEEP, [monotonic, 0, UNMAPPED, 0, 0, 0]),
        ] {
            assert_eq!(
                call(&process, call_number, arguments),
                Errno::EFAULT.result()
            );
        }
        let deadline = now() + 3_000_000;
        write_timespec(
            &process,
            req,
            (deadline / 1_000_000_000) as i64,
            (deadline % 1_000_000_000) as i64,
        );
        assert_eq!(
            call(
                &process,
                number::CLOCK_NANOSLEEP,
                [monotonic, TIMER_ABSTIME, req, 0, 0, 0]
            ),
            0
        );
        assert!(now() >= deadline);
        write_timespec(&process, req, 0, 1);
        let before = now();
        assert_eq!(
            call(
                &process,
                number::CLOCK_NANOSLEEP,
                [0, TIMER_ABSTIME, req, 0, 0, 0]
            ),
            0
        );
        assert!(now() - before < 1_000_000);
        for (id, errno) in [
            (clock::PROCESS_CPUTIME_ID, Errno::EINVAL),
            (clock::THREAD_CPUTIME_ID, Errno::EOPNOTSUPP),
            (clock::REALTIME_ALARM, Errno::EFAULT),
            (clock::BOOTTIME_ALARM, Errno::EFAULT),
            (clock::MONOTONIC_RAW, Errno::EOPNOTSUPP),
            (clock::MONOTONIC_COARSE, Errno::EOPNOTSUPP),
            (clock::REALTIME_COARSE, Errno::EOPNOTSUPP),
        ] {
            assert_eq!(
                call(
                    &process,
                    number::CLOCK_NANOSLEEP,
                    [u64::from(id as u32), 0, UNMAPPED, 0, 0, 0]
                ),
                errno.result()
            );
        }
        write_timespec(&process, req, 0, -1);
        for alarm in [clock::REALTIME_ALARM, clock::BOOTTIME_ALARM] {
            let alarm = u64::from(alarm as u32);
            assert_eq!(
                call(&process, number::CLOCK_NANOSLEEP, [alarm, 0, req, 0, 0, 0]),
                Errno::EINVAL.result()
            );
        }
        write_timespec(&process, req, 0, 1);
        for alarm in [clock::REALTIME_ALARM, clock::BOOTTIME_ALARM] {
            let alarm = u64::from(alarm as u32);
            assert_eq!(
                call(&process, number::CLOCK_NANOSLEEP, [alarm, 0, req, 0, 0, 0]),
                Errno::EOPNOTSUPP.result()
            );
        }
    }

    #[test]
    fn no_process_may_set_the_wall_clock_and_the_checks_come_first_as_on_linux() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (time, zone) = (SCRATCH + 0x100, SCRATCH + 0x110);
        process.write_user_memory(zone, &[0; 8]).unwrap();
        write_timespec(&process, time, 1, 0);
        let settime =
            |clock: u64, tp: u64| call(&process, number::CLOCK_SETTIME, [clock, tp, 0, 0, 0, 0]);
        assert_eq!(settime(1, UNMAPPED), Errno::EINVAL.result());
        assert_eq!(settime(0, UNMAPPED), Errno::EFAULT.result());
        assert_eq!(settime(0, time), Errno::EPERM.result());
        for seconds in [-1, 8_277_292_036, i64::MAX] {
            write_timespec(&process, time, seconds, 0);
            assert_eq!(settime(0, time), Errno::EINVAL.result());
        }
        write_timespec(&process, time, 8_277_292_035, 999_999_999);
        assert_eq!(settime(0, time), Errno::EPERM.result());
        let settimeofday =
            |tv: u64, tz: u64| call(&process, number::SETTIMEOFDAY, [tv, tz, 0, 0, 0, 0]);
        assert_eq!(settimeofday(UNMAPPED, UNMAPPED), Errno::EFAULT.result());
        write_timespec(&process, time, 1, 1_000_001);
        assert_eq!(settimeofday(time, UNMAPPED), Errno::EINVAL.result());
        write_timespec(&process, time, 1, 1_000_000);
        assert_eq!(settimeofday(time, UNMAPPED), Errno::EFAULT.result());
        assert_eq!(settimeofday(time, zone), Errno::EINVAL.result());
        for seconds in [-1, 8_277_292_036] {
            write_timespec(&process, time, seconds, 0);
            assert_eq!(settimeofday(time, 0), Errno::EINVAL.result());
        }
        write_timespec(&process, time, 8_277_292_035, 0);
        assert_eq!(settimeofday(time, 0), Errno::EPERM.result());
        write_timespec(&process, time, 1, 999_999);
        assert_eq!(settimeofday(time, zone), Errno::EPERM.result());
        assert_eq!(settimeofday(0, 0), Errno::EPERM.result());
    }

    #[test]
    fn a_fault_s_signal_blocked_or_ignored_ends_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let sigsegv = u64::from(signal::SIGSEGV);
        for blocked in [true, false] {
            let (process, _) = process_with_stacks(&kernel);
            if blocked {
                set_handler(&process, sigsegv, signals::flag::SIGINFO, 0);
                let set = HANDLER_STACK_TOP - 3 * FRAME_SIZE + 64;
                process
                    .write_user_memory(set, &(1u64 << (sigsegv - 1)).to_ne_bytes())
                    .unwrap();
                assert_eq!(
                    call(&process, number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0]),
                    0
                );
            } else {
                ignore(&process, sigsegv);
            }
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            let exception = page_fault_exception(0x1000, false);
            assert_eq!(
                {
                    let exit = context.simulate_exception(exception);
                    handle_first(&process, &mut context, exit)
                },
                ControlFlow::Break(())
            );
            assert_eq!(
                process.exit_status(),
                Some(ProcessExit::Killed {
                    signal: signal::SIGSEGV
                })
            );
        }
    }

    const STAT_BUFFER: u64 = SCRATCH + 0x200;
    const STATX_BUFFER: u64 = SCRATCH + 0x400;
    const EMPTY: u64 = SCRATCH + 0x600;
    const RELATIVE: u64 = SCRATCH + 0x610;
    const ABSOLUTE: u64 = SCRATCH + 0x620;

    fn process_for_status(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
    ) -> Arc<Process<P>> {
        let (process, _) = process_with_stacks(kernel);
        for (address, path) in [
            (EMPTY, &b"\0"[..]),
            (RELATIVE, b"tty\0"),
            (ABSOLUTE, b"/dev/console\0"),
        ] {
            process.write_user_memory(address, path).unwrap();
        }
        process
    }

    fn user_bytes<const N: usize>(process: &Process<P>, address: u64) -> [u8; N] {
        let mut bytes = [0; N];
        process.read_user_memory(address, &mut bytes).unwrap();
        bytes
    }

    fn word(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn double_word(bytes: &[u8], offset: usize) -> u64 {
        u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    #[test]
    fn the_standard_descriptors_are_the_console_s_character_device() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let x86_64 = cfg!(target_arch = "x86_64");
        let [mode, owner, group, rdev] = if x86_64 {
            [24, 28, 32, 40]
        } else {
            [16, 24, 28, 32]
        };
        let links = |bytes: &[u8]| {
            if x86_64 {
                double_word(bytes, 16)
            } else {
                u64::from(word(bytes, 20))
            }
        };
        for fd in [0, 1, 2, 0xffff_ffff_0000_0001] {
            process
                .write_user_memory(STAT_BUFFER, &[0xff; stat::STAT_SIZE])
                .unwrap();
            assert_eq!(
                call(&process, number::FSTAT, [fd, STAT_BUFFER, 0, 0, 0, 0]),
                0
            );
            let bytes: [u8; stat::STAT_SIZE] = user_bytes(&process, STAT_BUFFER);
            assert_eq!(double_word(&bytes, 0), 5);
            assert_eq!(double_word(&bytes, 8), stat::CONSOLE_INODE);
            assert_eq!(word(&bytes, mode), 0o020600);
            assert_eq!(links(&bytes), 1);
            assert_eq!((word(&bytes, owner), word(&bytes, group)), (0, 0));
            assert_eq!(double_word(&bytes, rdev), 0x501);
            assert_eq!(double_word(&bytes, 48), 0);
            assert_eq!(word(&bytes, 56), 4096);
            assert!(bytes[64..].iter().all(|&byte| byte == 0));
            for (path, sync) in [
                (EMPTY, 0),
                (0, 0),
                (EMPTY, 0x2000),
                (EMPTY, 0x4000),
                (0, 0x6000),
            ] {
                process
                    .write_user_memory(STAT_BUFFER, &[0xff; stat::STAT_SIZE])
                    .unwrap();
                let flags = stat::at::EMPTY_PATH | stat::at::SYMLINK_NOFOLLOW | sync;
                assert_eq!(
                    call(
                        &process,
                        number::NEWFSTATAT,
                        [fd, path, STAT_BUFFER, flags, 0, 0]
                    ),
                    0
                );
                assert_eq!(
                    user_bytes::<{ stat::STAT_SIZE }>(&process, STAT_BUFFER),
                    bytes
                );
            }
        }
    }

    #[test]
    fn statx_reports_the_console_s_basic_fields_whatever_the_mask() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        for (fd, path, mask) in [(1, EMPTY, 0x7ff), (2, 0, 0), (0, EMPTY, 0x0fff_ffff)] {
            process
                .write_user_memory(STATX_BUFFER, &[0xff; stat::STATX_SIZE])
                .unwrap();
            let flags = stat::at::EMPTY_PATH | 0x2000;
            assert_eq!(
                call(
                    &process,
                    number::STATX,
                    [fd, path, flags, mask, STATX_BUFFER, 0]
                ),
                0
            );
            let bytes: [u8; stat::STATX_SIZE] = user_bytes(&process, STATX_BUFFER);
            assert_eq!(word(&bytes, 0), 0x7ff);
            assert_eq!(word(&bytes, 4), 4096);
            assert_eq!(double_word(&bytes, 8), 0);
            assert_eq!(
                [word(&bytes, 16), word(&bytes, 20), word(&bytes, 24)],
                [1, 0, 0]
            );
            assert_eq!(u16::from_ne_bytes([bytes[28], bytes[29]]), 0o020600);
            assert_eq!(double_word(&bytes, 32), stat::CONSOLE_INODE);
            assert_eq!(
                [
                    double_word(&bytes, 40),
                    double_word(&bytes, 48),
                    double_word(&bytes, 56)
                ],
                [0; 3]
            );
            assert!(bytes[64..128].iter().all(|&byte| byte == 0));
            assert_eq!(
                [128, 132, 136, 140].map(|offset| word(&bytes, offset)),
                [5, 1, 0, 5]
            );
            assert!(bytes[144..].iter().all(|&byte| byte == 0));
        }
    }

    #[test]
    fn the_status_calls_fail_as_linux_s_do_in_linux_s_order() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let fstatat = |fd, path, buffer, flags| {
            call(
                &process,
                number::NEWFSTATAT,
                [fd, path, buffer, flags, 0, 0],
            )
        };
        let statx = |fd, path, flags, mask, buffer| {
            call(&process, number::STATX, [fd, path, flags, mask, buffer, 0])
        };
        let fdcwd = u64::from(stat::at::FDCWD as u32);
        let empty = stat::at::EMPTY_PATH;
        for buffer in [READ_ONLY, UNMAPPED] {
            assert_eq!(
                call(&process, number::FSTAT, [1, buffer, 0, 0, 0, 0]),
                Errno::EFAULT.result()
            );
            assert_eq!(fstatat(1, EMPTY, buffer, empty), Errno::EFAULT.result());
            assert_eq!(
                statx(1, EMPTY, empty, 0x7ff, buffer),
                Errno::EFAULT.result()
            );
        }
        assert_eq!(
            call(&process, number::FSTAT, [3, UNMAPPED, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
        assert_eq!(
            call(&process, number::FSTAT, [u64::MAX, STAT_BUFFER, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
        assert_eq!(statx(0, 0, 0, 0xfff, 0), Errno::EFAULT.result());
        for (fd, path, flags, error) in [
            (3, UNMAPPED, 0x200, Errno::EINVAL),
            (3, UNMAPPED, 0x1_0000, Errno::EINVAL),
            (1, UNMAPPED, empty, Errno::EFAULT),
            (1, 0, 0, Errno::EFAULT),
            (3, EMPTY, 0, Errno::ENOENT),
            (1, EMPTY, 0, Errno::ENOENT),
            (3, EMPTY, empty, Errno::EBADF),
            (3, 0, empty, Errno::EBADF),
            (fdcwd, EMPTY, empty, Errno::ENOENT),
            (fdcwd, RELATIVE, 0, Errno::ENOENT),
            (3, ABSOLUTE, 0, Errno::ENOENT),
            (1, ABSOLUTE, empty, Errno::ENOENT),
            (1, RELATIVE, empty, Errno::ENOTDIR),
            (3, RELATIVE, 0, Errno::EBADF),
        ] {
            assert_eq!(
                fstatat(fd, path, UNMAPPED, flags),
                error.result(),
                "{fd} {path:#x} {flags:#x}"
            );
            assert_eq!(
                fstatat(fd, path, STAT_BUFFER, flags | 1 << 32),
                error.result(),
                "{fd} {path:#x} {flags:#x}"
            );
            assert_eq!(
                statx(fd, path, flags, 0x7ff, UNMAPPED),
                error.result(),
                "{fd} {path:#x} {flags:#x}"
            );
        }
        for (flags, mask) in [
            (0x6000, 0x7ff),
            (0x8000, 0x7ff),
            (0, 0x8000_0000),
            (empty | 0x4000, 0xffff_ffff),
        ] {
            assert_eq!(
                statx(1, EMPTY, flags, mask, STATX_BUFFER),
                Errno::EINVAL.result(),
                "{flags:#x} {mask:#x}"
            );
        }
        assert_eq!(
            statx(1, EMPTY, empty | 0x4000, 0x1_7fff_ffff, STATX_BUFFER),
            0
        );
    }

    #[test]
    fn paths_are_read_to_their_end_and_no_further() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let fdcwd = u64::from(stat::at::FDCWD as u32);
        let lowest = HANDLER_STACK_TOP - 3 * FRAME_SIZE;
        let fstatat = |path| {
            call(
                &process,
                number::NEWFSTATAT,
                [fdcwd, path, STAT_BUFFER, 0, 0, 0],
            )
        };
        process
            .write_user_memory(HANDLER_STACK_TOP - 2, b"x\0")
            .unwrap();
        assert_eq!(fstatat(HANDLER_STACK_TOP - 2), Errno::ENOENT.result());
        process
            .write_user_memory(HANDLER_STACK_TOP - 2, b"xy")
            .unwrap();
        assert_eq!(fstatat(HANDLER_STACK_TOP - 2), Errno::EFAULT.result());
        let longest = alloc::vec![b'a'; 4095];
        process.write_user_memory(lowest + 1, &longest).unwrap();
        process.write_user_memory(lowest + 4096, &[0]).unwrap();
        assert_eq!(fstatat(lowest + 1), Errno::ENOENT.result());
        process.write_user_memory(lowest + 4096, b"a\0").unwrap();
        assert_eq!(fstatat(lowest + 1), Errno::ENAMETOOLONG.result());
        assert_eq!(fstatat(0xffff_ffff_ffff_fff0), Errno::EFAULT.result());
    }

    #[test]
    fn the_console_cannot_seek() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let lseek =
            |fd, offset, whence| call(&process, number::LSEEK, [fd, offset, whence, 0, 0, 0]);
        for fd in [0, 1, 2, 1 << 32 | 2] {
            for whence in [0, 1, 2, 3, 4, 0xffff_ffff_0000_0001] {
                assert_eq!(
                    lseek(fd, 0, whence),
                    Errno::ESPIPE.result(),
                    "{fd:#x} {whence:#x}"
                );
                assert_eq!(
                    lseek(fd, u64::MAX, whence),
                    Errno::ESPIPE.result(),
                    "{fd:#x} {whence:#x}"
                );
            }
            for whence in [5, 0xffff_ffff] {
                assert_eq!(lseek(fd, 0, whence), Errno::EINVAL.result());
            }
        }
        assert_eq!(lseek(3, 0, 9), Errno::EBADF.result());
    }

    fn call_as(
        process: &Arc<Process<P>>,
        thread: &mut LinuxThread,
        number: u64,
        arguments: [u64; 6],
    ) -> u64 {
        let mut context = called(number, arguments);
        assert_eq!(
            handle(process, thread, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        context.syscall_result()
    }

    fn second_thread(
        process: &Process<P>,
        first: &LinuxThread,
        clear_child_tid: u64,
    ) -> LinuxThread {
        process.add_thread();
        first.new_thread(process::allocate_thread_id().unwrap(), clear_child_tid)
    }

    #[test]
    fn clone_refuses_what_linux_refuses_and_new_processes_until_they_exist() {
        use thread::flag::*;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let clone = |flags| call(&process, number::CLONE, [flags, 0, 0, 0, 0, 0]);
        for flags in [
            NEW_THREAD | PIDFD | PARENT_SETTID,
            NEW_THREAD | NEWNS | FS,
            NEW_THREAD | NEWUSER | FS,
            VM | THREAD,
            SIGHAND,
            THREAD | SIGHAND,
            NEW_THREAD | NEWPID,
            VM | SIGHAND | PIDFD | DETACHED,
            NEW_THREAD | PIDFD,
            1 << 40 | SIGHAND | EXIT_SIGNAL,
            NEW_THREAD | NEWNS,
            NEW_THREAD | NEWNET,
            VM | PIDFD,
        ] {
            assert_eq!(clone(flags), Errno::EINVAL.result(), "{flags:#x}");
        }
        const SIGCHLD: u64 = 17;
        for flags in [
            SIGCHLD,
            VM | VFORK | SIGCHLD,
            VM | SIGHAND,
            NEW_THREAD | PARENT,
            NEW_THREAD | VFORK,
        ] {
            assert_eq!(clone(flags), Errno::ENOSYS.result(), "{flags:#x}");
        }
        #[cfg(target_arch = "x86_64")]
        assert_eq!(
            call(
                &process,
                number::CLONE,
                [NEW_THREAD | SETTLS, 0, 0, 0, 1 << 63, 0]
            ),
            Errno::EPERM.result()
        );
        assert!(!process.has_ended());
        assert!(process.remove_thread(), "a refused clone counted a thread");
    }

    #[test]
    fn threads_have_their_own_ids_and_only_the_last_to_exit_ends_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let word = SCRATCH;
        process.write_user_memory(word, &[0xff; 8]).unwrap();
        let mut first = take_first_thread(&process);
        assert_eq!(call_as(&process, &mut first, number::GETTID, [0; 6]), pid);
        let mut second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        assert!(tid > pid);
        assert_eq!(call_as(&process, &mut second, number::GETTID, [0; 6]), tid);
        assert_eq!(call_as(&process, &mut second, number::GETPID, [0; 6]), pid);
        assert_eq!(
            call_as(
                &process,
                &mut second,
                number::SET_TID_ADDRESS,
                [word, 0, 0, 0, 0, 0]
            ),
            tid
        );
        assert_eq!(second.clear_child_tid(), word);

        let mut context = called(number::EXIT, [3, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert!(!process.has_ended());
        assert_eq!(process.exit_status(), None);
        let mut bytes = [0u8; 8];
        process.read_user_memory(word, &mut bytes).unwrap();
        assert_eq!(bytes, [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);

        let mut context = called(number::EXIT, [0x107, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert!(process.has_ended());
        assert_eq!(process.exit_status(), Some(ProcessExit::Exited(7)));
    }

    #[test]
    fn a_process_whose_threads_all_exit_alone_ends_with_its_first_thread_s_status() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let mut context = called(number::EXIT, [5, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(process.exit_status(), None);
        let mut context = called(number::EXIT, [9, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(process.exit_status(), Some(ProcessExit::Exited(5)));
    }

    #[test]
    fn exit_group_ends_the_process_whatever_threads_go_on() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, SCRATCH);
        let mut context = called(number::EXIT_GROUP, [0x2a, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert!(process.has_ended());
        assert_eq!(process.exit_status(), Some(ProcessExit::Exited(0x2a)));
        let mut context = called(number::GETPID, [0; 6]);
        let mut first = first;
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(()),
            "a thread of an ended process went back to its program after a call out of line"
        );
    }

    #[test]
    fn each_thread_blocks_and_queues_its_own_signals() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let rt = 40u64;
        let mut first = take_first_thread(&process);
        let mask = SCRATCH + 0x300;
        process
            .write_user_memory(mask, &(1u64 << (rt - 1)).to_ne_bytes())
            .unwrap();
        let block = [0, mask, 0, 8, 0, 0];
        assert_eq!(
            call_as(&process, &mut first, number::RT_SIGPROCMASK, block),
            0
        );
        let mut second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        assert_eq!(second.signal_state().blocked, 1 << (rt - 1));
        assert_eq!(second.signal_state().pending, 0);

        let (info, waited) = (SCRATCH, SCRATCH + 0x100);
        write_siginfo(&process, info, signals::code::QUEUE, 0x11);
        let queue = |thread: &mut LinuxThread, target| {
            call_as(
                &process,
                thread,
                number::RT_TGSIGQUEUEINFO,
                [pid, target, rt, info, 0, 0],
            )
        };
        assert_eq!(queue(&mut first, pid), 0);
        write_siginfo(&process, info, signals::code::QUEUE, 0x22);
        assert_eq!(queue(&mut second, tid), 0);
        assert_eq!(queue(&mut first, tid), Errno::ESRCH.result());
        assert_eq!(queue(&mut second, pid), Errno::ESRCH.result());

        let set = SCRATCH + 0x200;
        process
            .write_user_memory(set, &(1u64 << (rt - 1)).to_ne_bytes())
            .unwrap();
        let wait = |thread: &mut LinuxThread| {
            let taken = call_as(
                &process,
                thread,
                number::RT_SIGTIMEDWAIT,
                [set, waited, 0, 8, 0, 0],
            );
            (taken, read_siginfo(&process, waited).2)
        };
        assert_eq!(wait(&mut second), (rt, 0x22));
        assert_eq!(wait(&mut first), (rt, 0x11));
        assert_eq!(first.signal_state().pending, 0);
        assert_eq!(second.signal_state().pending, 0);
    }
}
