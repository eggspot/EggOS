//! 32-bit Arm's Linux ABI, EABI (M9, D492): its own table (`arch/arm/tools/syscall.tbl`, the
//! number in `r7`), with the 32-bit-uid, 64-bit-offset and 64-bit-time calls in the places of
//! the generic names (`setuid32` for `SETUID`, `_llseek` for `LSEEK`, `mmap2` for `MMAP`,
//! `clock_gettime64` for `CLOCK_GETTIME`, ...), the generic signals and flags but its own
//! `O_DIRECTORY` and `O_NOFOLLOW`, `struct sigaction` with a restorer, and its signal frame
//! (`struct sigcontext` of 21 words, then the mask, then the VFP record in `uc_regspace`). The
//! calls that only exist with 32-bit time or 32-bit `struct stat` (`nanosleep`, `gettimeofday`,
//! `getrlimit`, `stat64`, `fstatat64`, `_newselect`, `time`) are past [`number::LAST`], so
//! they answer `ENOSYS`: the C libraries of 64-bit time use `clock_nanosleep_time64`,
//! `clock_gettime64`, `prlimit64`, `statx` and `pselect6_time64` instead. `set_tls`, in Arm's
//! private range at `0xf0005`, sets the thread pointer (`TPIDRURO`).

/// The calls 32-bit Arm keeps beyond the generic table: the ones older than the `*at` family
/// and `renameat`.
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
    pub const SET_THREAD_AREA: bool = false;
}

/// Linux's system-call numbers on 32-bit Arm, EABI.
pub mod number {
    pub const RESTART_SYSCALL: u64 = 0;
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
    pub const CHMOD: u64 = 15;
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
    pub const SETPGID: u64 = 57;
    pub const DUP2: u64 = 63;
    pub const GETPPID: u64 = 64;
    pub const GETPGRP: u64 = 65;
    pub const SETSID: u64 = 66;
    pub const SETHOSTNAME: u64 = 74;
    pub const GETRUSAGE: u64 = 77;
    pub const SYMLINK: u64 = 83;
    pub const READLINK: u64 = 85;
    pub const REBOOT: u64 = 88;
    pub const MUNMAP: u64 = 91;
    pub const FCHMOD: u64 = 94;
    pub const WAIT4: u64 = 114;
    pub const SYSINFO: u64 = 116;
    pub const FSYNC: u64 = 118;
    pub const CLONE: u64 = 120;
    pub const SETDOMAINNAME: u64 = 121;
    pub const UNAME: u64 = 122;
    pub const MPROTECT: u64 = 125;
    pub const GETPGID: u64 = 132;
    pub const FCHDIR: u64 = 133;
    /// `_llseek`: the offset's high and low halves, then where the result goes (D491).
    pub const LSEEK: u64 = 140;
    pub const FLOCK: u64 = 143;
    pub const MSYNC: u64 = 144;
    pub const READV: u64 = 145;
    pub const WRITEV: u64 = 146;
    pub const GETSID: u64 = 147;
    pub const FDATASYNC: u64 = 148;
    pub const SCHED_GETPARAM: u64 = 155;
    pub const SCHED_GETSCHEDULER: u64 = 157;
    pub const SCHED_YIELD: u64 = 158;
    pub const MREMAP: u64 = 163;
    pub const POLL: u64 = 168;
    pub const PRCTL: u64 = 172;
    pub const RT_SIGRETURN: u64 = 173;
    pub const RT_SIGACTION: u64 = 174;
    pub const RT_SIGPROCMASK: u64 = 175;
    pub const RT_SIGPENDING: u64 = 176;
    pub const RT_SIGQUEUEINFO: u64 = 178;
    pub const RT_SIGSUSPEND: u64 = 179;
    pub const PREAD64: u64 = 180;
    pub const PWRITE64: u64 = 181;
    pub const GETCWD: u64 = 183;
    pub const SIGALTSTACK: u64 = 186;
    pub const VFORK: u64 = 190;
    /// `mmap2`: the offset in 4096-byte units (D491).
    pub const MMAP: u64 = 192;
    /// `ftruncate64`.
    pub const FTRUNCATE: u64 = 194;
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
    pub const GETDENTS64: u64 = 217;
    pub const MADVISE: u64 = 220;
    /// `fcntl64`.
    pub const FCNTL: u64 = 221;
    pub const GETTID: u64 = 224;
    pub const TKILL: u64 = 238;
    pub const SCHED_SETAFFINITY: u64 = 241;
    pub const SCHED_GETAFFINITY: u64 = 242;
    pub const EXIT_GROUP: u64 = 248;
    pub const EPOLL_CREATE: u64 = 250;
    pub const EPOLL_CTL: u64 = 251;
    pub const EPOLL_WAIT: u64 = 252;
    pub const SET_TID_ADDRESS: u64 = 256;
    /// `statfs64` and `fstatfs64`: the structure's size, then where it goes (D491).
    pub const STATFS: u64 = 266;
    pub const FSTATFS: u64 = 267;
    pub const TGKILL: u64 = 268;
    /// `arm_fadvise64_64`: the advice second (D491).
    pub const FADVISE64: u64 = 270;
    pub const WAITID: u64 = 280;
    pub const SOCKET: u64 = 281;
    pub const BIND: u64 = 282;
    pub const CONNECT: u64 = 283;
    pub const LISTEN: u64 = 284;
    pub const ACCEPT: u64 = 285;
    pub const GETSOCKNAME: u64 = 286;
    pub const GETPEERNAME: u64 = 287;
    pub const SOCKETPAIR: u64 = 288;
    pub const SENDTO: u64 = 290;
    pub const RECVFROM: u64 = 292;
    pub const SHUTDOWN: u64 = 293;
    pub const SETSOCKOPT: u64 = 294;
    pub const GETSOCKOPT: u64 = 295;
    pub const SENDMSG: u64 = 296;
    pub const RECVMSG: u64 = 297;
    pub const INOTIFY_INIT: u64 = 316;
    pub const INOTIFY_ADD_WATCH: u64 = 317;
    pub const INOTIFY_RM_WATCH: u64 = 318;
    pub const OPENAT: u64 = 322;
    pub const MKDIRAT: u64 = 323;
    pub const UNLINKAT: u64 = 328;
    pub const RENAMEAT: u64 = 329;
    pub const LINKAT: u64 = 330;
    pub const SYMLINKAT: u64 = 331;
    pub const READLINKAT: u64 = 332;
    pub const FCHMODAT: u64 = 333;
    pub const FACCESSAT: u64 = 334;
    pub const SET_ROBUST_LIST: u64 = 338;
    pub const GET_ROBUST_LIST: u64 = 339;
    pub const GETCPU: u64 = 345;
    pub const EPOLL_PWAIT: u64 = 346;
    pub const SIGNALFD: u64 = 349;
    pub const TIMERFD_CREATE: u64 = 350;
    pub const EVENTFD: u64 = 351;
    pub const FALLOCATE: u64 = 352;
    pub const SIGNALFD4: u64 = 355;
    pub const EVENTFD2: u64 = 356;
    pub const EPOLL_CREATE1: u64 = 357;
    pub const DUP3: u64 = 358;
    pub const PIPE2: u64 = 359;
    pub const INOTIFY_INIT1: u64 = 360;
    pub const RT_TGSIGQUEUEINFO: u64 = 363;
    pub const ACCEPT4: u64 = 366;
    pub const PRLIMIT64: u64 = 369;
    pub const SYNCFS: u64 = 373;
    pub const RENAMEAT2: u64 = 382;
    pub const GETRANDOM: u64 = 384;
    pub const MEMFD_CREATE: u64 = 385;
    pub const EXECVEAT: u64 = 387;
    pub const STATX: u64 = 397;
    pub const CLOCK_GETTIME: u64 = 403;
    pub const CLOCK_SETTIME: u64 = 404;
    pub const CLOCK_GETRES: u64 = 406;
    pub const CLOCK_NANOSLEEP: u64 = 407;
    pub const TIMERFD_GETTIME: u64 = 410;
    pub const TIMERFD_SETTIME: u64 = 411;
    pub const UTIMENSAT: u64 = 412;
    pub const PSELECT6: u64 = 413;
    pub const PPOLL: u64 = 414;
    pub const RT_SIGTIMEDWAIT: u64 = 421;
    pub const FUTEX: u64 = 422;
    pub const CLOSE_RANGE: u64 = 436;
    pub const OPENAT2: u64 = 437;
    pub const FACCESSAT2: u64 = 439;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const FCHMODAT2: u64 = 452;
    /// The highest number above.
    pub const UMASK: u64 = 60;
    pub const CHOWN: u64 = 212;
    pub const FCHOWN: u64 = 207;
    pub const LCHOWN: u64 = 198;
    pub const FCHOWNAT: u64 = 325;
    pub const PIDFD_OPEN: u64 = 434;
    pub const PIDFD_SEND_SIGNAL: u64 = 424;
    pub const PIDFD_GETFD: u64 = 438;
    pub const SENDFILE: u64 = 239;
    pub const SPLICE: u64 = 340;
    pub const TEE: u64 = 342;
    pub const VMSPLICE: u64 = 343;
    pub const COPY_FILE_RANGE: u64 = 391;
    pub const SETXATTR: u64 = 226;
    pub const LSETXATTR: u64 = 227;
    pub const FSETXATTR: u64 = 228;
    pub const GETXATTR: u64 = 229;
    pub const LGETXATTR: u64 = 230;
    pub const FGETXATTR: u64 = 231;
    pub const LISTXATTR: u64 = 232;
    pub const LLISTXATTR: u64 = 233;
    pub const FLISTXATTR: u64 = 234;
    pub const REMOVEXATTR: u64 = 235;
    pub const LREMOVEXATTR: u64 = 236;
    pub const FREMOVEXATTR: u64 = 237;
    pub const SENDMMSG: u64 = 374;
    pub const RECVMMSG: u64 = 365;
    pub const CAPGET: u64 = 184;
    pub const CAPSET: u64 = 185;
    pub const PTRACE: u64 = 26;
    pub const FANOTIFY_INIT: u64 = 367;
    pub const FANOTIFY_MARK: u64 = 368;
    pub const LAST: u64 = FCHMODAT2;
    /// Not on 32-bit Arm with 64-bit time and `statx`: past [`LAST`].
    pub const NEWFSTATAT: u64 = LAST + 1;
    pub const FSTAT: u64 = LAST + 2;
    pub const STAT: u64 = LAST + 3;
    pub const LSTAT: u64 = LAST + 4;
    pub const NANOSLEEP: u64 = LAST + 5;
    pub const GETTIMEOFDAY: u64 = LAST + 6;
    pub const SETTIMEOFDAY: u64 = LAST + 7;
    pub const GETRLIMIT: u64 = LAST + 8;
    pub const SETRLIMIT: u64 = LAST + 9;
    pub const SELECT: u64 = LAST + 10;
    pub const TIME: u64 = LAST + 11;
    /// `set_tls(address)`, Arm's private call: the thread pointer.
    pub const SET_TLS: u64 = 0xf_0005;
}

/// The terminal and descriptor `ioctl` numbers: the generic ones.
pub use super::generic::tty;

/// The architecture, as `uname` names it on ARMv7.
pub const MACHINE: &str = "armv7l";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::ARM;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// `clone`'s fourth argument is `tls` and its fifth `child_tid`.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data 8-byte aligned under the EABI.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// `struct sigaction` has `sa_restorer`.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 2048;
/// A 64-bit value split over two registers starts at an even one (D491).
pub const ALIGNED_REGISTER_PAIRS: bool = true;
/// `arm_fadvise64_64` takes the advice second (D491).
pub const FADVISE_ADVICE_SECOND: bool = true;
/// `struct statfs64` is packed to 4 bytes.
pub const STATFS_ALIGN: usize = 4;
/// `set_tls`, the private call past the table that sets the thread pointer.
pub const SET_TLS_NUMBER: Option<u64> = Some(number::SET_TLS);

const NOP: u32 = 0xe320_f000;
const MOV_R7_RT_SIGRETURN: u32 = 0xe3a0_7000 | number::RT_SIGRETURN as u32;
const SVC_0: u32 = 0xef00_0000;
/// The kernel's `rt_sigreturn` trampoline: `nop`, `mov r7, #173` and `svc #0`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([NOP, MOV_R7_RT_SIGRETURN, SVC_0]));

/// Linux's `stat` here: never written, as `stat64` and `fstatat64` are past the table and
/// programs use `statx`.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 128;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::Generic;
}

/// The `open` flags whose values differ between architectures.
pub mod open {
    /// `O_DIRECTORY`.
    pub const DIRECTORY: u32 = 0o40_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o100_000;
}

/// The `mmap` bits only some architectures have.
pub mod mmap {
    /// The architecture's own `prot` bits: none.
    pub const PROT_ARCH: u64 = 0;
    /// `MAP_32BIT`: none here.
    pub const MAP_32BIT: u64 = 0;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `struct rt_sigframe`: the `siginfo_t`, then the `ucontext_t` (`uc_flags`, `uc_link`,
/// `uc_stack`, then `uc_mcontext`, `struct sigcontext`: `trap_no`, `error_code`, `oldmask`,
/// `r0` to `r10`, `fp`, `ip`, `sp`, `lr`, `pc`, `cpsr` and `fault_address`; then the mask, room
/// to 128 bytes of it, and `uc_regspace`, 8-byte aligned, holding the VFP record and a zero end
/// word), then four words of return code, unused: the handler returns through its restorer.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{
        BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    pub const UCONTEXT: usize = INFO_SIZE;
    const MCONTEXT: usize = UCONTEXT + 20;
    const TRAP_NUMBER: usize = MCONTEXT;
    const ERROR_CODE: usize = MCONTEXT + 4;
    const REGISTERS: usize = MCONTEXT + 12;
    const PC: usize = REGISTERS + 15 * 4;
    const CPSR: usize = PC + 4;
    const FAULT_ADDRESS: usize = CPSR + 4;
    pub const SIGMASK: usize = FAULT_ADDRESS + 4;
    const REGISTER_SPACE: usize = (SIGMASK + 128).next_multiple_of(8);
    pub const FP_PART: usize = REGISTER_SPACE;
    const _: () = assert!(SIGMASK == 232 && REGISTER_SPACE == 360);

    /// The whole frame: the `ucontext_t` of 744 bytes, then the return code's 16.
    pub const SIZE: u64 = (REGISTER_SPACE + 512 + 16) as u64;
    /// What the kernel writes and reads: all of it.
    pub const WRITTEN: usize = SIZE as usize;
    pub const READ: usize = WRITTEN;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;

    /// `struct vfp_sigframe`: its magic and size, `d0` to `d31`, `FPSCR`, then `FPEXC`,
    /// `FPINST` and `FPINST2`, 8-byte aligned.
    pub type FpPart = [u8; VFP_SIZE];
    const VFP_SIZE: usize = 288;
    const VFP_MAGIC: u32 = 0x5646_5001;
    const VFP_REGISTERS: usize = 8;
    const FPSCR: usize = VFP_REGISTERS + 32 * 8;
    const FPEXC: usize = 272;
    const FPEXC_ENABLE: u32 = 1 << 30;

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;

    const THUMB: u64 = 1 << 5;
    const IF_THEN: u64 = 0x0600_fc00;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; VFP_SIZE];
        part[..4].copy_from_slice(&VFP_MAGIC.to_ne_bytes());
        part[4..8].copy_from_slice(&(VFP_SIZE as u32).to_ne_bytes());
        for (index, d) in state.d().iter().enumerate() {
            part[VFP_REGISTERS + index * 8..][..8].copy_from_slice(&d.to_ne_bytes());
        }
        part[FPSCR..][..4].copy_from_slice(&state.fpscr().to_ne_bytes());
        part[FPEXC..][..4].copy_from_slice(&FPEXC_ENABLE.to_ne_bytes());
        part
    }

    /// [`BadFrame`] unless the record's magic and size are the VFP's, as Linux checks; the
    /// exception registers are not taken back.
    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        let field = |at: usize| u32::from_ne_bytes(part[at..at + 4].try_into().unwrap());
        if field(0) != VFP_MAGIC || field(4) != VFP_SIZE as u32 {
            return Err(BadFrame);
        }
        for (index, d) in state.d_mut().iter_mut().enumerate() {
            *d = word(part, VFP_REGISTERS + index * 8);
        }
        state.set_fpscr(field(FPSCR));
        Ok(())
    }

    /// 8-byte aligned below `top`.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(SIZE) & !7
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        trap: Option<&Trap>,
        _restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        let put_u32 = |frame: &mut [u8; WRITTEN], at: usize, value: u64| {
            frame[at..at + 4].copy_from_slice(&(value as u32).to_ne_bytes());
        };
        frame[..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        if let Some(trap) = trap {
            put_u32(&mut frame, TRAP_NUMBER, trap.number);
            put_u32(&mut frame, ERROR_CODE, trap.cause);
            put_u32(&mut frame, FAULT_ADDRESS, trap.address);
        }
        put_u32(&mut frame, MCONTEXT + 8, saved.blocked);
        for (index, &value) in saved.registers.r.iter().enumerate() {
            put_u32(&mut frame, REGISTERS + index * 4, value);
        }
        put_u32(&mut frame, PC, saved.registers.pc);
        put_u32(&mut frame, CPSR, saved.registers.cpsr);
        put(&mut frame, SIGMASK, saved.blocked);
        frame[FP_PART..][..VFP_SIZE].copy_from_slice(&saved.fp);
        (place(top), frame)
    }

    /// Nothing outside the frame.
    pub fn write_extra<P: Platform>(
        _: &Process<P>,
        _: u64,
        _: u64,
        _: &Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(
        _: &Process<P>,
        _: &[u8; READ],
        _: &mut Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    /// `r0` the signal, `r1` the `siginfo_t`, `r2` the `ucontext_t`, `lr` the restorer, and
    /// the handler in Thumb state if its address is odd, as Linux starts one.
    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.r[0] = u64::from(handler.signal);
        registers.r[1] = frame;
        registers.r[2] = frame + UCONTEXT as u64;
        registers.r[13] = frame;
        registers.r[14] = handler.restorer;
        let mut cpsr = registers.cpsr & !(IF_THEN | THUMB);
        if handler.entry & 1 != 0 {
            cpsr |= THUMB;
        }
        registers.cpsr = cpsr;
        registers.pc = handler.entry & !1;
    }

    /// At the stack pointer, which must be 8-byte aligned, as Linux checks.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        let stack = registers.r[13];
        stack.is_multiple_of(8).then_some(stack)
    }

    pub fn parse(frame: &[u8; READ], _current: &UserRegisters) -> Option<Saved> {
        let u32_at =
            |at: usize| u64::from(u32::from_ne_bytes(frame[at..at + 4].try_into().unwrap()));
        let mut r = [0; 15];
        for (index, value) in r.iter_mut().enumerate() {
            *value = u32_at(REGISTERS + index * 4);
        }
        Some(Saved {
            registers: UserRegisters {
                pc: u32_at(PC),
                r,
                cpsr: u32_at(CPSR),
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: frame[FP_PART..][..VFP_SIZE].try_into().unwrap(),
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.r[0]
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// The long-descriptor fault status (`DFSR`/`IFSR` bits 5 to 0, LPAE): an alignment
    /// fault and a debug event.
    pub const STATUS_ALIGNMENT: u64 = 0b10_0001;
    pub const STATUS_DEBUG: u64 = 0b10_0010;
    /// `trap_no` as Linux reports it: 14 for an abort, 6 for an undefined instruction.
    pub const TRAP_ABORT: u64 = 14;
    pub const TRAP_UNDEFINED: u64 = 6;

    /// Whether `status` is a synchronous external abort or a parity error, on the access or on
    /// a table walk for it at levels 1 to 3.
    pub fn external_abort(status: u64) -> bool {
        matches!(status, 0b01_0000 | 0b01_0101..=0b01_0111 | 0b01_1000 | 0b01_1101..=0b01_1111)
    }

    /// The signal, `si_code` and `si_addr` Linux gives `exception`: an abort by its fault
    /// status (alignment `SIGBUS`, an external abort `SIGBUS` with `BUS_OBJERR`, a debug event
    /// `SIGTRAP`, the rest by the process's regions); an undefined instruction `SIGILL` at its
    /// PC. The frame reports `trap_no`, and for an abort the status (`error_code`, the
    /// prefetch mark included, as Linux's `FSR_LNX_PF`) and the address.
    pub fn classify(
        exception: &Exception,
        _stack: u64,
        _in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        let address = exception.address.unwrap_or(0);
        let status = exception.code & 0x3f;
        match exception.kind {
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => {
                let reported = match status {
                    STATUS_ALIGNMENT => (signal::SIGBUS, code::BUS_ADRALN, address),
                    STATUS_DEBUG => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
                    status if external_abort(status) => (signal::SIGBUS, code::BUS_OBJERR, address),
                    _ => segv(address),
                };
                let trap = Trap {
                    number: TRAP_ABORT,
                    cause: exception.code,
                    address,
                };
                fault(reported, trap)
            }
            ExceptionKind::Alignment => fault(
                (signal::SIGBUS, code::BUS_ADRALN, address),
                Trap {
                    number: TRAP_ABORT,
                    cause: STATUS_ALIGNMENT,
                    address,
                },
            ),
            ExceptionKind::Breakpoint => {
                let trap = Trap {
                    number: TRAP_UNDEFINED,
                    ..Trap::default()
                };
                fault((signal::SIGTRAP, code::TRAP_BRKPT, pc), trap)
            }
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Arithmetic
            | ExceptionKind::Debug
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => {
                let trap = Trap {
                    number: TRAP_UNDEFINED,
                    ..Trap::default()
                };
                fault((signal::SIGILL, code::ILL_ILLOPC, pc), trap)
            }
        }
    }
}
