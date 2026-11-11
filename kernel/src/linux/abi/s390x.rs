//! s390x's Linux ABI (D218, D323): its own table (`arch/s390/kernel/syscalls/syscall.tbl`,
//! the 64-bit entries), big-endian structs, the generic terminal numbers. A system call is
//! `svc 0` with the number in `r1` and the arguments in `r2` to `r7`; it returns in `r2`,
//! negative on an error. `mmap` takes one pointer to its six arguments (Linux's `old_mmap`),
//! and `clone` takes the stack before the flags (`CLONE_BACKWARDS2`).

/// The calls s390x keeps beyond the generic table: the ones older than the `*at` family and
/// `renameat`; no `arch_prctl`, since a program sets its thread pointer, `a0:a1`, itself.
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on s390x. `time` and `accept` are 31-bit or `socketcall`
/// only: they get numbers past [`LAST`](number::LAST), which answer `ENOSYS`.
pub mod number {
    pub const EXIT: u64 = 1;
    pub const FORK: u64 = 2;
    pub const READ: u64 = 3;
    pub const WRITE: u64 = 4;
    pub const OPEN: u64 = 5;
    pub const CLOSE: u64 = 6;
    pub const LINK: u64 = 9;
    pub const UNLINK: u64 = 10;
    pub const EXECVE: u64 = 11;
    pub const CHDIR: u64 = 12;
    pub const TIME: u64 = NOT_ON_S390X;
    pub const CHMOD: u64 = 15;
    pub const LSEEK: u64 = 19;
    pub const GETPID: u64 = 20;
    pub const PAUSE: u64 = 29;
    pub const ACCESS: u64 = 33;
    pub const SYNC: u64 = 36;
    pub const KILL: u64 = 37;
    pub const RENAME: u64 = 38;
    pub const MKDIR: u64 = 39;
    pub const RMDIR: u64 = 40;
    pub const DUP: u64 = 41;
    pub const PIPE: u64 = 42;
    pub const TIMES: u64 = 43;
    pub const BRK: u64 = 45;
    pub const IOCTL: u64 = 54;
    pub const FCNTL: u64 = 55;
    pub const SETPGID: u64 = 57;
    pub const DUP2: u64 = 63;
    pub const GETPPID: u64 = 64;
    pub const GETPGRP: u64 = 65;
    pub const SETSID: u64 = 66;
    pub const SETHOSTNAME: u64 = 74;
    pub const SETRLIMIT: u64 = 75;
    pub const GETRUSAGE: u64 = 77;
    pub const GETTIMEOFDAY: u64 = 78;
    pub const SETTIMEOFDAY: u64 = 79;
    pub const SYMLINK: u64 = 83;
    pub const READLINK: u64 = 85;
    pub const REBOOT: u64 = 88;
    /// `old_mmap`: its one argument points at the six (D323).
    pub const MMAP: u64 = 90;
    pub const MUNMAP: u64 = 91;
    pub const FTRUNCATE: u64 = 93;
    pub const FCHMOD: u64 = 94;
    pub const STATFS: u64 = 99;
    pub const FSTATFS: u64 = 100;
    pub const STAT: u64 = 106;
    pub const LSTAT: u64 = 107;
    pub const FSTAT: u64 = 108;
    pub const WAIT4: u64 = 114;
    pub const SYSINFO: u64 = 116;
    pub const FSYNC: u64 = 118;
    pub const CLONE: u64 = 120;
    pub const SETDOMAINNAME: u64 = 121;
    pub const UNAME: u64 = 122;
    pub const MPROTECT: u64 = 125;
    pub const GETPGID: u64 = 132;
    pub const FCHDIR: u64 = 133;
    pub const SELECT: u64 = 142;
    pub const FLOCK: u64 = 143;
    pub const MSYNC: u64 = 144;
    pub const READV: u64 = 145;
    pub const WRITEV: u64 = 146;
    pub const GETSID: u64 = 147;
    pub const FDATASYNC: u64 = 148;
    pub const SCHED_GETPARAM: u64 = 155;
    pub const SCHED_GETSCHEDULER: u64 = 157;
    pub const SCHED_YIELD: u64 = 158;
    pub const NANOSLEEP: u64 = 162;
    pub const MREMAP: u64 = 163;
    pub const POLL: u64 = 168;
    pub const PRCTL: u64 = 172;
    pub const RT_SIGRETURN: u64 = 173;
    pub const RT_SIGACTION: u64 = 174;
    pub const RT_SIGPROCMASK: u64 = 175;
    pub const RT_SIGPENDING: u64 = 176;
    pub const RT_SIGTIMEDWAIT: u64 = 177;
    pub const RT_SIGQUEUEINFO: u64 = 178;
    pub const RT_SIGSUSPEND: u64 = 179;
    pub const PREAD64: u64 = 180;
    pub const PWRITE64: u64 = 181;
    pub const GETCWD: u64 = 183;
    pub const SIGALTSTACK: u64 = 186;
    pub const VFORK: u64 = 190;
    pub const GETRLIMIT: u64 = 191;
    pub const GETUID: u64 = 199;
    pub const GETGID: u64 = 200;
    pub const GETEUID: u64 = 201;
    pub const GETEGID: u64 = 202;
    pub const SETREUID: u64 = 203;
    pub const SETREGID: u64 = 204;
    pub const GETGROUPS: u64 = 205;
    pub const SETGROUPS: u64 = 206;
    pub const SETRESUID: u64 = 208;
    pub const GETRESUID: u64 = 209;
    pub const SETRESGID: u64 = 210;
    pub const GETRESGID: u64 = 211;
    pub const SETUID: u64 = 213;
    pub const SETGID: u64 = 214;
    pub const SETFSUID: u64 = 215;
    pub const SETFSGID: u64 = 216;
    pub const MADVISE: u64 = 219;
    pub const GETDENTS64: u64 = 220;
    pub const GETTID: u64 = 236;
    pub const TKILL: u64 = 237;
    pub const FUTEX: u64 = 238;
    pub const SCHED_SETAFFINITY: u64 = 239;
    pub const SCHED_GETAFFINITY: u64 = 240;
    pub const TGKILL: u64 = 241;
    pub const EXIT_GROUP: u64 = 248;
    pub const EPOLL_CREATE: u64 = 249;
    pub const EPOLL_CTL: u64 = 250;
    pub const EPOLL_WAIT: u64 = 251;
    pub const SET_TID_ADDRESS: u64 = 252;
    pub const FADVISE64: u64 = 253;
    pub const CLOCK_SETTIME: u64 = 259;
    pub const CLOCK_GETTIME: u64 = 260;
    pub const CLOCK_GETRES: u64 = 261;
    pub const CLOCK_NANOSLEEP: u64 = 262;
    pub const WAITID: u64 = 281;
    pub const INOTIFY_INIT: u64 = 284;
    pub const INOTIFY_ADD_WATCH: u64 = 285;
    pub const INOTIFY_RM_WATCH: u64 = 286;
    pub const OPENAT: u64 = 288;
    pub const MKDIRAT: u64 = 289;
    pub const NEWFSTATAT: u64 = 293;
    pub const UNLINKAT: u64 = 294;
    pub const RENAMEAT: u64 = 295;
    pub const LINKAT: u64 = 296;
    pub const SYMLINKAT: u64 = 297;
    pub const READLINKAT: u64 = 298;
    pub const FCHMODAT: u64 = 299;
    pub const FACCESSAT: u64 = 300;
    pub const PSELECT6: u64 = 301;
    pub const PPOLL: u64 = 302;
    pub const SET_ROBUST_LIST: u64 = 304;
    pub const GET_ROBUST_LIST: u64 = 305;
    pub const GETCPU: u64 = 311;
    pub const EPOLL_PWAIT: u64 = 312;
    pub const FALLOCATE: u64 = 314;
    pub const UTIMENSAT: u64 = 315;
    pub const SIGNALFD: u64 = 316;
    pub const EVENTFD: u64 = 318;
    pub const TIMERFD_CREATE: u64 = 319;
    pub const TIMERFD_SETTIME: u64 = 320;
    pub const TIMERFD_GETTIME: u64 = 321;
    pub const SIGNALFD4: u64 = 322;
    pub const EVENTFD2: u64 = 323;
    pub const INOTIFY_INIT1: u64 = 324;
    pub const PIPE2: u64 = 325;
    pub const DUP3: u64 = 326;
    pub const EPOLL_CREATE1: u64 = 327;
    pub const RT_TGSIGQUEUEINFO: u64 = 330;
    pub const PRLIMIT64: u64 = 334;
    pub const SYNCFS: u64 = 338;
    pub const RENAMEAT2: u64 = 347;
    pub const GETRANDOM: u64 = 349;
    pub const MEMFD_CREATE: u64 = 350;
    pub const EXECVEAT: u64 = 354;
    pub const SOCKET: u64 = 359;
    pub const SOCKETPAIR: u64 = 360;
    pub const BIND: u64 = 361;
    pub const CONNECT: u64 = 362;
    pub const LISTEN: u64 = 363;
    pub const ACCEPT4: u64 = 364;
    pub const GETSOCKOPT: u64 = 365;
    pub const SETSOCKOPT: u64 = 366;
    pub const GETSOCKNAME: u64 = 367;
    pub const GETPEERNAME: u64 = 368;
    pub const SENDTO: u64 = 369;
    pub const SENDMSG: u64 = 370;
    pub const RECVFROM: u64 = 371;
    pub const RECVMSG: u64 = 372;
    pub const SHUTDOWN: u64 = 373;
    pub const STATX: u64 = 379;
    pub const CLOSE_RANGE: u64 = 436;
    pub const OPENAT2: u64 = 437;
    pub const FACCESSAT2: u64 = 439;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const FCHMODAT2: u64 = 452;
    pub const ACCEPT: u64 = NOT_ON_S390X + 1;
    /// The highest number above that s390x has.
    pub const LAST: u64 = FCHMODAT2;
    /// Where the names s390x lacks are numbered: past [`LAST`], so they answer `ENOSYS`.
    const NOT_ON_S390X: u64 = 0x1_0000;
}

/// The terminal and descriptor `ioctl` numbers: the generic ones.
pub use super::generic::tty;

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "s390x";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::S390;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// `clone(stack, flags, parent_tid, child_tid, tls)`: the child's id word before the thread
/// pointer, and [`CLONE_STACK_FIRST`].
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = true;
/// `clone` takes the stack first and the flags second.
pub const CLONE_STACK_FIRST: bool = true;
/// `mmap`'s one argument points at its six, in order.
pub const MMAP_ARGUMENTS_IN_MEMORY: bool = true;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// `struct sigaction` has `sa_restorer`; without `SA_RESTORER` handlers return through the
/// kernel's trampoline.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// `MINSIGSTKSZ`.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 2048;

const SVC_RT_SIGRETURN: [u8; 2] = [0x0a, number::RT_SIGRETURN as u8];
const NOP: [u8; 2] = [0x07, 0x07];
/// The kernel's `rt_sigreturn` trampoline: `svc 173`, then `nopr` halfwords.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> = Some(&[
    SVC_RT_SIGRETURN[0],
    SVC_RT_SIGRETURN[1],
    NOP[0],
    NOP[1],
    NOP[0],
    NOP[1],
]);

/// Linux's `stat` on s390x: 144 bytes, the times before `st_blksize` and `st_blocks`.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 144;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::S390x;
}

/// The `open` flags whose values differ between architectures: the generic ones.
pub mod open {
    /// `O_DIRECTORY`.
    pub const DIRECTORY: u32 = 0o200_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o400_000;
}

/// The `mmap` bits only some architectures have: none here.
pub mod mmap {
    pub const PROT_ARCH: u64 = 0;
    pub const MAP_32BIT: u64 = 0;

    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `struct rt_sigframe { u8 callee_used_stack[160]; u16 svc_insn; siginfo_t info; struct
/// ucontext uc; }`, the `ucontext` holding `uc_flags`, `uc_link` and `uc_stack`, then
/// `uc_mcontext` (`_sigregs`: the PSW's mask and address, `gprs[16]`, `acrs[16]`, then the
/// floating-point control word, a pad and `fprs[16]`), `uc_sigmask` and the room after it to
/// 128 bytes, then `uc_mcontext_ext` (the vector registers, zero: programs get no vector
/// facility). The handler's stack pointer is the frame, its first word the back chain.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{
        BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    /// `__SIGNAL_FRAMESIZE`: the callee's save area at the frame's start.
    pub const HANDLER_FRAME: u64 = 160;
    const SVC_INSTRUCTION: usize = 160;
    pub const INFO: usize = 168;
    pub const UCONTEXT: usize = INFO + INFO_SIZE;
    pub const MCONTEXT: usize = UCONTEXT + 40;
    const PSW_MASK: usize = MCONTEXT;
    const PSW_ADDRESS: usize = MCONTEXT + 8;
    const GPRS: usize = MCONTEXT + 16;
    const ACRS: usize = GPRS + 16 * 8;
    pub const FP_REGS: usize = ACRS + 16 * 4;
    pub const SIGMASK: usize = MCONTEXT + 344;
    const MCONTEXT_EXTENSION: usize = UCONTEXT + 512;
    const UCONTEXT_END: usize = MCONTEXT_EXTENSION + 512;
    /// The whole frame.
    pub const WRITTEN: usize = UCONTEXT_END;
    pub const READ: usize = WRITTEN;
    const _: () = assert!(FP_REGS + 8 + 16 * 8 == SIGMASK && UCONTEXT == 296);

    /// The floating-point control word, a pad, then `f0` to `f15`.
    pub type FpPart = [u8; 8 + 16 * 8];

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;
    /// No red zone: the callee's save area is the caller's.
    pub const RED_ZONE: u64 = 0;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; 8 + 16 * 8];
        part[..4].copy_from_slice(&state.fpc().to_ne_bytes());
        for (index, register) in state.f().iter().enumerate() {
            part[8 + index * 8..][..8].copy_from_slice(&register.to_ne_bytes());
        }
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for (index, register) in state.f_mut().iter_mut().enumerate() {
            *register = word(part, 8 + index * 8);
        }
        state.set_fpc(u32::from_ne_bytes(part[..4].try_into().unwrap()));
        Ok(())
    }

    /// 8-byte aligned below `top`.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(WRITTEN as u64) & !7
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        trap: Option<&Trap>,
        restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let frame_address = place(top);
        let mut frame = [0; WRITTEN];
        let registers = &saved.registers;
        put(&mut frame, 0, registers.gpr[15]);
        let trampoline = super::SIGRETURN_TRAMPOLINE.unwrap_or(&[]);
        frame[SVC_INSTRUCTION..][..2].copy_from_slice(&trampoline[..2]);
        frame[INFO..][..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, PSW_MASK, registers.mask);
        put(&mut frame, PSW_ADDRESS, registers.pc);
        for (index, value) in registers.gpr.iter().enumerate() {
            put(&mut frame, GPRS + index * 8, *value);
        }
        for (index, value) in registers.access.iter().enumerate() {
            frame[ACRS + index * 4..][..4].copy_from_slice(&value.to_ne_bytes());
        }
        frame[FP_REGS..][..8 + 16 * 8].copy_from_slice(&saved.fp);
        put(&mut frame, SIGMASK, saved.blocked);
        let _ = (trap, restorer);
        (frame_address, frame)
    }

    /// Nothing outside the frame: the back chain is its first word.
    pub fn write_extra<P: Platform>(_: &Process<P>, _: u64, _: u64, _: &Saved) -> Result<(), BadFrame> {
        Ok(())
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(_: &Process<P>, _: &[u8; READ], _: &mut Saved) -> Result<(), BadFrame> {
        Ok(())
    }

    /// `r2` the signal, `r3` the `siginfo_t`, `r4` the `ucontext`, `r5` the last breaking-event
    /// address (none), `r14` the restorer and `r15` the frame.
    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.gpr[2] = u64::from(handler.signal);
        registers.gpr[3] = frame + INFO as u64;
        registers.gpr[4] = frame + UCONTEXT as u64;
        registers.gpr[5] = 0;
        registers.gpr[14] = handler.restorer;
        registers.gpr[15] = frame;
        registers.pc = handler.entry;
    }

    /// The handler returned with its stack pointer at the frame.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.gpr[15])
    }

    pub fn parse(frame: &[u8; READ], current: &UserRegisters) -> Option<Saved> {
        let mut gpr = [0; 16];
        for (index, value) in gpr.iter_mut().enumerate() {
            *value = word(frame, GPRS + index * 8);
        }
        let mut access = [0; 16];
        for (index, value) in access.iter_mut().enumerate() {
            *value = u32::from_ne_bytes(frame[ACRS + index * 4..][..4].try_into().unwrap());
        }
        let mut fp = [0; 8 + 16 * 8];
        fp.copy_from_slice(&frame[FP_REGS..][..8 + 16 * 8]);
        Some(Saved {
            registers: UserRegisters {
                gpr,
                access,
                pc: word(frame, PSW_ADDRESS),
                mask: word(frame, PSW_MASK),
                ..*current
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp,
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.gpr[2]
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// `FPE_INTDIV`: an integer division by zero, the fixed-point divide exception.
    const FPE_INTDIV: i32 = 1;

    /// The signal, `si_code` and `si_addr` Linux gives `exception`: the program-interruption
    /// code is the trap number, the translation-exception address the faulting one.
    pub fn classify(
        exception: &Exception,
        _stack: u64,
        _in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        let address = exception.address.unwrap_or(0);
        let reported = match exception.kind {
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => segv(address),
            ExceptionKind::Alignment => (signal::SIGBUS, code::BUS_ADRALN, address),
            ExceptionKind::Breakpoint | ExceptionKind::Debug => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            ExceptionKind::Arithmetic => (signal::SIGFPE, FPE_INTDIV, pc),
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        let trap = Trap {
            number: exception.code & 0xffff,
            cause: exception.code >> 16,
            address,
        };
        fault(reported, trap)
    }
}
