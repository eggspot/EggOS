//! x86_64's Linux ABI (D218), its own `asm/unistd_64.h` table and structs: see [`super`].

/// The calls x86_64 keeps beyond the generic table (Linux's `__ARCH_WANT_*`): the ones older
/// than the `*at` family (`open`, `stat`, `fork`, `poll`, `select`, ...), `renameat` and
/// `arch_prctl`.
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = true;
}

/// Linux's system-call numbers on x86_64.
pub mod number {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const OPEN: u64 = 2;
    pub const CLOSE: u64 = 3;
    pub const STAT: u64 = 4;
    pub const FSTAT: u64 = 5;
    pub const LSTAT: u64 = 6;
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
    pub const PREAD64: u64 = 17;
    pub const READV: u64 = 19;
    pub const WRITEV: u64 = 20;
    pub const SCHED_YIELD: u64 = 24;
    pub const MREMAP: u64 = 25;
    pub const MSYNC: u64 = 26;
    pub const MADVISE: u64 = 28;
    pub const DUP: u64 = 32;
    pub const DUP2: u64 = 33;
    pub const PAUSE: u64 = 34;
    pub const NANOSLEEP: u64 = 35;
    pub const GETPID: u64 = 39;
    pub const CLONE: u64 = 56;
    pub const FORK: u64 = 57;
    pub const VFORK: u64 = 58;
    pub const EXECVE: u64 = 59;
    pub const EXIT: u64 = 60;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const UNAME: u64 = 63;
    pub const FCNTL: u64 = 72;
    pub const GETTIMEOFDAY: u64 = 96;
    pub const GETRLIMIT: u64 = 97;
    pub const GETUID: u64 = 102;
    pub const GETGID: u64 = 104;
    pub const SETUID: u64 = 105;
    pub const SETGID: u64 = 106;
    pub const GETEUID: u64 = 107;
    pub const GETEGID: u64 = 108;
    pub const SETPGID: u64 = 109;
    pub const GETPPID: u64 = 110;
    pub const GETPGRP: u64 = 111;
    pub const SETSID: u64 = 112;
    pub const SETREUID: u64 = 113;
    pub const SETREGID: u64 = 114;
    pub const GETGROUPS: u64 = 115;
    pub const SETGROUPS: u64 = 116;
    pub const SETRESUID: u64 = 117;
    pub const GETRESUID: u64 = 118;
    pub const SETRESGID: u64 = 119;
    pub const GETRESGID: u64 = 120;
    pub const GETPGID: u64 = 121;
    pub const SETFSUID: u64 = 122;
    pub const SETFSGID: u64 = 123;
    pub const GETSID: u64 = 124;
    pub const RT_SIGPENDING: u64 = 127;
    pub const RT_SIGTIMEDWAIT: u64 = 128;
    pub const RT_SIGQUEUEINFO: u64 = 129;
    pub const RT_SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const PRCTL: u64 = 157;
    pub const ARCH_PRCTL: u64 = 158;
    pub const SETRLIMIT: u64 = 160;
    pub const SETTIMEOFDAY: u64 = 164;
    pub const REBOOT: u64 = 169;
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
    pub const WAITID: u64 = 247;
    pub const NEWFSTATAT: u64 = 262;
    pub const PPOLL: u64 = 271;
    pub const SET_ROBUST_LIST: u64 = 273;
    pub const GET_ROBUST_LIST: u64 = 274;
    pub const RT_TGSIGQUEUEINFO: u64 = 297;
    pub const PRLIMIT64: u64 = 302;
    pub const GETCWD: u64 = 79;
    pub const CHDIR: u64 = 80;
    pub const FCHDIR: u64 = 81;
    pub const GETDENTS64: u64 = 217;
    pub const OPENAT: u64 = 257;
    pub const GETCPU: u64 = 309;
    pub const SCHED_GETPARAM: u64 = 143;
    pub const SCHED_GETSCHEDULER: u64 = 145;
    pub const SCHED_GETAFFINITY: u64 = 204;
    pub const SCHED_SETAFFINITY: u64 = 203;
    pub const GETRUSAGE: u64 = 98;
    pub const TIMES: u64 = 100;
    pub const DUP3: u64 = 292;
    pub const PIPE: u64 = 22;
    pub const PIPE2: u64 = 293;
    pub const PWRITE64: u64 = 18;
    pub const FTRUNCATE: u64 = 77;
    pub const RENAME: u64 = 82;
    pub const MKDIR: u64 = 83;
    pub const RMDIR: u64 = 84;
    pub const UNLINK: u64 = 87;
    pub const MKDIRAT: u64 = 258;
    pub const UNLINKAT: u64 = 263;
    pub const RENAMEAT: u64 = 264;
    pub const UTIMENSAT: u64 = 280;
    pub const FSYNC: u64 = 74;
    pub const FDATASYNC: u64 = 75;
    pub const SYNC: u64 = 162;
    pub const SYNCFS: u64 = 306;
    pub const RENAMEAT2: u64 = 316;
    pub const GETRANDOM: u64 = 318;
    pub const EXECVEAT: u64 = 322;
    pub const STATX: u64 = 332;
    pub const CLOSE_RANGE: u64 = 436;
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SENDMSG: u64 = 46;
    pub const RECVMSG: u64 = 47;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const GETSOCKNAME: u64 = 51;
    pub const GETPEERNAME: u64 = 52;
    pub const SOCKETPAIR: u64 = 53;
    pub const SETSOCKOPT: u64 = 54;
    pub const GETSOCKOPT: u64 = 55;
    pub const ACCEPT4: u64 = 288;
    pub const FALLOCATE: u64 = 285;
    pub const MEMFD_CREATE: u64 = 319;
    pub const ACCESS: u64 = 21;
    pub const READLINK: u64 = 89;
    pub const SETHOSTNAME: u64 = 170;
    pub const SETDOMAINNAME: u64 = 171;
    pub const READLINKAT: u64 = 267;
    pub const FACCESSAT: u64 = 269;
    pub const FACCESSAT2: u64 = 439;
    pub const SELECT: u64 = 23;
    pub const PSELECT6: u64 = 270;
    pub const EPOLL_CREATE: u64 = 213;
    pub const EPOLL_WAIT: u64 = 232;
    pub const EPOLL_CTL: u64 = 233;
    pub const EPOLL_PWAIT: u64 = 281;
    pub const EPOLL_CREATE1: u64 = 291;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const SIGNALFD: u64 = 282;
    pub const TIMERFD_CREATE: u64 = 283;
    pub const EVENTFD: u64 = 284;
    pub const TIMERFD_SETTIME: u64 = 286;
    pub const TIMERFD_GETTIME: u64 = 287;
    pub const SIGNALFD4: u64 = 289;
    pub const EVENTFD2: u64 = 290;
    pub const INOTIFY_INIT: u64 = 253;
    pub const INOTIFY_ADD_WATCH: u64 = 254;
    pub const INOTIFY_RM_WATCH: u64 = 255;
    pub const INOTIFY_INIT1: u64 = 294;
    pub const LINK: u64 = 86;
    pub const SYMLINK: u64 = 88;
    pub const CHMOD: u64 = 90;
    pub const FCHMOD: u64 = 91;
    pub const LINKAT: u64 = 265;
    pub const SYMLINKAT: u64 = 266;
    pub const FCHMODAT: u64 = 268;
    pub const OPENAT2: u64 = 437;
    pub const FCHMODAT2: u64 = 452;
    pub const FLOCK: u64 = 73;
    pub const STATFS: u64 = 137;
    pub const FSTATFS: u64 = 138;
    pub const SYSINFO: u64 = 99;
    pub const FADVISE64: u64 = 221;
    /// The highest number above.
    pub const UMASK: u64 = 95;
    pub const CHOWN: u64 = 92;
    pub const FCHOWN: u64 = 93;
    pub const LCHOWN: u64 = 94;
    pub const FCHOWNAT: u64 = 260;
    pub const PIDFD_OPEN: u64 = 434;
    pub const PIDFD_SEND_SIGNAL: u64 = 424;
    pub const PIDFD_GETFD: u64 = 438;
    pub const SENDFILE: u64 = 40;
    pub const SPLICE: u64 = 275;
    pub const TEE: u64 = 276;
    pub const VMSPLICE: u64 = 278;
    pub const COPY_FILE_RANGE: u64 = 326;
    pub const SETXATTR: u64 = 188;
    pub const LSETXATTR: u64 = 189;
    pub const FSETXATTR: u64 = 190;
    pub const GETXATTR: u64 = 191;
    pub const LGETXATTR: u64 = 192;
    pub const FGETXATTR: u64 = 193;
    pub const LISTXATTR: u64 = 194;
    pub const LLISTXATTR: u64 = 195;
    pub const FLISTXATTR: u64 = 196;
    pub const REMOVEXATTR: u64 = 197;
    pub const LREMOVEXATTR: u64 = 198;
    pub const FREMOVEXATTR: u64 = 199;
    pub const SENDMMSG: u64 = 307;
    pub const RECVMMSG: u64 = 299;
    pub const CAPGET: u64 = 125;
    pub const CAPSET: u64 = 126;
    pub const PTRACE: u64 = 101;
    pub const FANOTIFY_INIT: u64 = 300;
    pub const FANOTIFY_MARK: u64 = 301;
    pub const LAST: u64 = FCHMODAT2;
}

/// The terminal and descriptor `ioctl` numbers: the generic ones.
pub use super::generic::tty;

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "x86_64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::X86_64;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = true;
/// Whether `clone`'s fourth argument is `child_tid` and its fifth `tls` (x86_64's order), not
/// the other way round.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = true;
/// The size of a `struct epoll_event`: x86_64 packs its 32-bit events and 64-bit data.
pub const EPOLL_EVENT_SIZE: usize = 12;
/// Whether `struct sigaction` has `sa_restorer`.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 2048;
/// The kernel's `rt_sigreturn` trampoline: none, as handlers need `SA_RESTORER` here.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> = None;

/// Linux's `stat` here.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 144;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::X86_64;
}

/// The `open` flags whose values differ between architectures.
pub mod open {
    /// `O_DIRECTORY`.
    pub const DIRECTORY: u32 = 0o200_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o400_000;
}

/// The `mmap` bits only some architectures have.
pub mod mmap {
    /// The architecture's own `prot` bits: none.
    pub const PROT_ARCH: u64 = 0;
    /// `MAP_32BIT`: somewhere in the second gigabyte, x86_64 only.
    pub const MAP_32BIT: u64 = 0x40;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        flags & MAP_32BIT != 0
    }
}

/// `struct rt_sigframe { char *pretcode; ucontext_t uc; siginfo_t info; }`, the
/// `ucontext_t` holding `uc_mcontext` (`struct sigcontext`) before `uc_sigmask`.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{
        BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    pub const UCONTEXT: usize = 8;
    pub const MCONTEXT: usize = UCONTEXT + 40;
    pub const SIGMASK: usize = UCONTEXT + 296;
    pub const INFO: usize = UCONTEXT + 304;
    /// The whole frame.
    pub const WRITTEN: usize = INFO + INFO_SIZE;
    const _: () = assert!(WRITTEN == 440);
    /// What `rt_sigreturn` reads first: the frame, then the FP/SIMD image its `fpstate` points
    /// at ([`read_extra`]).
    pub const READ: usize = WRITTEN;

    /// The `FXSAVE` image (`struct _fpstate`): 512 bytes, 64-byte aligned, above the frame.
    pub const FP_SIZE: u64 = 512;
    /// The image, its software-reserved bytes from 464 zero: no `XSAVE` state follows.
    pub type FpPart = [u8; FP_SIZE as usize];
    pub const FP_SOFTWARE: usize = 464;

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = true;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = *state.fxsave_image();
        part[FP_SOFTWARE..].fill(0);
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        state.set_fxsave_image(part).map_err(|_| BadFrame)
    }

    /// `uc_flags`: `UC_SIGCONTEXT_SS` and `UC_STRICT_RESTORE_SS`, as Linux sets them for
    /// every 64-bit frame (the saved `ss` is the real one).
    pub const FLAGS: u64 = 0x2 | 0x4;

    /// `struct sigcontext`'s fields, from `uc_mcontext`, in the order of `regs`' below.
    pub const REGISTERS: usize = 18;
    pub const SELECTORS: usize = 144;
    pub const ERR: usize = 152;
    pub const TRAPNO: usize = 160;
    pub const OLDMASK: usize = 168;
    pub const CR2: usize = 176;
    pub const FPSTATE: usize = 184;
    const _: () = assert!(UCONTEXT == 8 && MCONTEXT == 48 && SIGMASK == 304 && INFO == 312);
    const _: () = assert!(MCONTEXT + SELECTORS == 192 && MCONTEXT + FPSTATE == 232);

    /// The flags a handler starts without, as on Linux: trap, direction and resume.
    pub const HANDLER_CLEARS: u64 = 1 << 8 | 1 << 10 | 1 << 16;

    pub fn regs(registers: &UserRegisters) -> [u64; REGISTERS] {
        [
            registers.r8,
            registers.r9,
            registers.r10,
            registers.r11,
            registers.r12,
            registers.r13,
            registers.r14,
            registers.r15,
            registers.rdi,
            registers.rsi,
            registers.rbp,
            registers.rbx,
            registers.rdx,
            registers.rax,
            registers.rcx,
            registers.rsp,
            registers.rip,
            registers.rflags,
        ]
    }

    /// The 128 bytes below the stack pointer that leaf functions may use (the ABI's red
    /// zone).
    pub const RED_ZONE: u64 = 128;

    /// The FP/SIMD image, then the frame, so that `(rsp + 8) % 16 == 0` when the handler
    /// starts, as after a `call`.
    pub const fn fp_image(top: u64) -> u64 {
        top.wrapping_sub(FP_SIZE) & !63
    }

    pub const fn place(top: u64) -> u64 {
        (fp_image(top).wrapping_sub(WRITTEN as u64) & !15).wrapping_sub(8)
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        trap: Option<&Trap>,
        restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        put(&mut frame, 0, restorer);
        if let Some(trap) = trap {
            put(&mut frame, MCONTEXT + ERR, trap.cause);
            put(&mut frame, MCONTEXT + TRAPNO, trap.number);
            put(&mut frame, MCONTEXT + CR2, trap.address);
        }
        put(&mut frame, UCONTEXT, FLAGS);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        for (index, value) in regs(&saved.registers).into_iter().enumerate() {
            put(&mut frame, MCONTEXT + index * 8, value);
        }
        let selectors = u64::from(saved.registers.cs) | u64::from(saved.registers.ss) << 48;
        put(&mut frame, MCONTEXT + SELECTORS, selectors);
        put(&mut frame, MCONTEXT + OLDMASK, saved.blocked);
        put(&mut frame, MCONTEXT + FPSTATE, fp_image(top));
        put(&mut frame, SIGMASK, saved.blocked);
        frame[INFO..].copy_from_slice(info);
        (place(top), frame)
    }

    /// The FP/SIMD image above the frame.
    pub fn write_extra<P: Platform>(
        process: &Process<P>,
        top: u64,
        _frame: u64,
        saved: &Saved,
    ) -> Result<(), BadFrame> {
        process
            .write_user_memory(fp_image(top), &saved.fp)
            .map_err(|_| BadFrame)
    }

    /// The FP/SIMD image `fpstate` points at; the initial state if it is null. One that is
    /// not 16-byte aligned, as `FXRSTOR64` needs, is refused, as on Linux, which loads it from
    /// the program's memory.
    pub fn read_extra<P: Platform>(
        process: &Process<P>,
        frame: &[u8; READ],
        saved: &mut Saved,
    ) -> Result<(), BadFrame> {
        match word(frame, MCONTEXT + FPSTATE) {
            0 => {
                saved.fp = fp_part(&FpState::new());
                Ok(())
            }
            image if image.is_multiple_of(16) => process
                .read_user_memory(image, &mut saved.fp)
                .map_err(|_| BadFrame),
            _ => Err(BadFrame),
        }
    }

    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.rdi = u64::from(handler.signal);
        registers.rsi = frame + INFO as u64;
        registers.rdx = frame + UCONTEXT as u64;
        registers.rax = 0;
        registers.rsp = frame;
        registers.rip = handler.entry;
        registers.rflags &= !HANDLER_CLEARS;
    }

    /// The handler's `ret` popped `pretcode`.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        registers.rsp.checked_sub(8)
    }

    /// The registers, but the segment selectors, which stay the kernel's.
    pub fn parse(frame: &[u8; READ], current: &UserRegisters) -> Option<Saved> {
        let regs: [u64; REGISTERS] =
            core::array::from_fn(|index| word(frame, MCONTEXT + index * 8));
        let [
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            rdi,
            rsi,
            rbp,
            rbx,
            rdx,
            rax,
            rcx,
            rsp,
            rip,
            rflags,
        ] = regs;
        Some(Saved {
            registers: UserRegisters {
                r8,
                r9,
                r10,
                r11,
                r12,
                r13,
                r14,
                r15,
                rdi,
                rsi,
                rbp,
                rbx,
                rdx,
                rax,
                rcx,
                rsp,
                rip,
                rflags,
                ..*current
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: [0; FP_SIZE as usize],
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.rax
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
        pub const FPSTATE: usize = super::MCONTEXT + super::FPSTATE;
        pub const ERR: usize = super::MCONTEXT + super::ERR;
        pub const TRAPNO: usize = super::MCONTEXT + super::TRAPNO;
        pub const CR2: usize = super::MCONTEXT + super::CR2;
    }

    /// The most stack a frame takes below an alternate stack's top, worked out by hand.
    #[cfg(test)]
    pub const LARGEST_FRAME_SPACE_EXPECTED: u64 = 512 + 63 + 448 + 8;

    /// FP/SIMD registers holding values a new thread's do not.
    #[cfg(test)]
    pub fn state_with_values() -> FpState {
        let mut state = FpState::new();
        let mut image = *state.fxsave_image();
        image[24..28].copy_from_slice(&0x3f80u32.to_le_bytes());
        image[160 + 8 * 16] = 0x23;
        image[160 + 15 * 16 + 15] = 0x45;
        state.set_fxsave_image(&image).unwrap();
        state
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::linux::signal_frame::initial_fp_part;

        #[test]
        fn the_initial_part_is_a_new_program_s_registers() {
            let part = initial_fp_part();
            assert_eq!(u16::from_ne_bytes([part[0], part[1]]), 0x037f);
            assert_eq!(u32::from_ne_bytes(part[24..28].try_into().unwrap()), 0x1f80);
            assert!(
                part.iter()
                    .enumerate()
                    .all(|(offset, &byte)| matches!(offset, 0 | 1 | 24 | 25) || byte == 0)
            );
        }

        #[test]
        fn x86_64_s_part_leaves_the_software_bytes_zero_and_refuses_reserved_mxcsr_bits() {
            let mut part = fp_part(&state_with_values());
            assert!(part[464..].iter().all(|&byte| byte == 0));
            part[24..28].copy_from_slice(&(0x1f80u32 | 1 << 16).to_le_bytes());
            let mut state = state_with_values();
            assert_eq!(load_fp_part(&part, &mut state), Err(BadFrame));
            assert_eq!(fp_part(&state), fp_part(&state_with_values()));
        }
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// The page-fault error code's protection bit: set for every address outside user space,
    /// mapped or not, as Linux does.
    pub const PAGE_FAULT_PROTECTION: u64 = 1 << 0;

    /// `trapno`: the vector of an exception of `kind`. The HAL's kinds tell every vector a
    /// program can raise apart but `#MF` and `#XM` from `#DE` (reported as 0: they cannot
    /// happen while programs have no FP/SIMD registers, D49), `#NP` from `#SS` (12), and
    /// `#NM` from `#UD` (6: FP/SIMD use, which D54 turns into a first use, not a fault).
    pub fn vector(kind: ExceptionKind) -> u64 {
        match kind {
            ExceptionKind::Arithmetic => 0,
            ExceptionKind::Debug => 1,
            ExceptionKind::Breakpoint => 3,
            ExceptionKind::IllegalInstruction => 6,
            ExceptionKind::Segment => 12,
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => 14,
            ExceptionKind::Alignment => 17,
            ExceptionKind::Interrupt | ExceptionKind::Other => 13,
        }
    }

    /// The signal, `si_code` and `si_addr` Linux gives `exception`, and its `trapno`, `err` and
    /// `cr2`: a `#PF`, the only fault on memory, by the process's regions, `cr2` its address;
    /// `int3` `SIGTRAP` (`SI_KERNEL`, no address: `rip` is past it already); `#UD` `SIGILL` at
    /// the instruction; `#AC` `SIGBUS` (`BUS_ADRALN`, no address); `#DE` `SIGFPE`
    /// (`FPE_INTDIV`; `#MF` and `#XM` take their `si_code` from the FP status with D54); `int1`
    /// (`icebp`) and a single step a program set up itself with `popf` `SIGTRAP` (`TRAP_BRKPT`,
    /// though Linux says `TRAP_TRACE` for the step; programs have no debug registers yet);
    /// `#NP` and `#SS` `SIGBUS` (`SI_KERNEL`); `#GP` (a privileged instruction, a
    /// non-canonical address) and the rest `SIGSEGV` (`SI_KERNEL`).
    pub fn classify(
        exception: &Exception,
        _stack: u64,
        in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        let mut trap = Trap {
            number: vector(exception.kind),
            cause: exception.code,
            address: 0,
        };
        let (signal, code, address) = match exception.kind {
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => {
                let address = exception.address.unwrap_or(0);
                if !in_user_space(address) {
                    trap.cause |= PAGE_FAULT_PROTECTION;
                }
                trap.address = address;
                segv(address)
            }
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::SI_KERNEL, 0),
            ExceptionKind::IllegalInstruction => (signal::SIGILL, code::ILL_ILLOPN, pc),
            ExceptionKind::Alignment => (signal::SIGBUS, code::BUS_ADRALN, 0),
            ExceptionKind::Arithmetic => (signal::SIGFPE, code::FPE_INTDIV, pc),
            ExceptionKind::Debug => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            ExceptionKind::Segment => (signal::SIGBUS, code::SI_KERNEL, 0),
            ExceptionKind::Interrupt | ExceptionKind::Other => {
                (signal::SIGSEGV, code::SI_KERNEL, 0)
            }
        };
        fault((signal, code, address), trap)
    }
}

/// What the kernel's host tests need to know of this architecture's registers and frames.
#[cfg(test)]
pub mod test_support {
    use super::signal_frame::layout;
    use crate::linux::signal;
    use eggos_hal::ExceptionKind;
    use eggos_hal::user::UserRegisters;

    pub const SYSCALL_INSTRUCTION_SIZE: u64 = 2;
    /// `st_mode`, `st_uid`, `st_gid` and `st_rdev` in `struct stat`.
    pub const STAT_FIELDS: [usize; 4] = [24, 28, 32, 40];

    /// The test platform's exception code: the error code alone.
    pub fn exception_code(_class: u64, status: u64) -> u64 {
        status
    }

    /// Exceptions with no handler: kind, vector, error code and the signal that ends the
    /// process.
    pub const FAULTS_WITHOUT_HANDLERS: &[(ExceptionKind, u64, u64, u32)] = &[
        (ExceptionKind::Arithmetic, 0, 0, signal::SIGFPE),
        (ExceptionKind::Debug, 1, 0, signal::SIGTRAP),
        (ExceptionKind::Breakpoint, 3, 0, signal::SIGTRAP),
        (ExceptionKind::IllegalInstruction, 6, 0, signal::SIGILL),
        (ExceptionKind::Segment, 12, 0, signal::SIGBUS),
        (ExceptionKind::Other, 13, 0, signal::SIGSEGV),
        (ExceptionKind::DataFault, 14, 0, signal::SIGSEGV),
        (ExceptionKind::InstructionFault, 14, 0x10, signal::SIGSEGV),
        (ExceptionKind::Alignment, 17, 0, signal::SIGBUS),
    ];

    /// Where a handler returns to and its stack pointer then: `ret` pops the return address.
    pub fn return_point(registers: &UserRegisters, read_word: impl Fn(u64) -> u64) -> (u64, u64) {
        (read_word(registers.rsp), registers.rsp + 8)
    }

    pub fn set_returned_stack(registers: &mut UserRegisters, stack: u64) {
        registers.rsp = stack;
    }

    /// A callee-saved register.
    pub fn callee_saved(registers: &mut UserRegisters) -> &mut u64 {
        &mut registers.rbx
    }

    /// Whether a handler's frame at `frame` is placed as after a `call`, below the red zone
    /// under `stack`.
    pub fn frame_is_placed(frame: u64, stack: u64) -> bool {
        (frame + 8).is_multiple_of(16) && frame + 128 < stack
    }

    /// The FP/SIMD image's address: where `fpstate` points.
    pub fn fp_part_address(frame: u64, read_word: impl Fn(u64) -> u64) -> u64 {
        read_word(frame + layout::FPSTATE as u64)
    }

    /// Checks a new frame's FP/SIMD image at `fp`: 64-byte aligned, the initial control words.
    pub fn check_initial_fp_part(fp: u64, part: &[u8], _read: impl Fn(u64, &mut [u8])) {
        assert_eq!(fp % 64, 0);
        assert_eq!(u16::from_ne_bytes([part[0], part[1]]), 0x037f);
        assert_eq!(u32::from_ne_bytes(part[24..28].try_into().unwrap()), 0x1f80);
    }

    /// Where the frame points at its FP/SIMD image, if it does.
    pub const FPSTATE_POINTER: Option<usize> = Some(layout::FPSTATE);

    /// A page fault's vector and the status bits the test platform adds.
    pub const PAGE_FAULT_CLASS: u64 = 14;
    pub const PAGE_FAULT_STATUS: u64 = 0;

    /// `err`, `trapno` and `cr2` of a frame, read with `word`.
    pub fn reported_trap(word: impl Fn(usize) -> u64) -> [u64; 3] {
        [word(layout::ERR), word(layout::TRAPNO), word(layout::CR2)]
    }

    /// What [`reported_trap`] reads for a page fault with error code `code` at `address`: the
    /// protection bit set past user space.
    pub fn expected_trap(code: u64, kernel_half: bool, address: u64) -> Option<[u64; 3]> {
        Some([code | u64::from(kernel_half), 14, address])
    }

    /// A handler's `ret` pops the restorer.
    pub fn pop_return_address(registers: &mut UserRegisters) {
        registers.rsp += 8;
    }

    /// The registers the simulated signal-handler program sets: one the handler clobbers and
    /// one it changes in its frame.
    pub fn kept_and_changed(registers: &mut UserRegisters) -> [&mut u64; 2] {
        [&mut registers.rbx, &mut registers.r12]
    }
    pub const CHANGED_IN_UCONTEXT: u64 = 40 + 4 * 8;

    /// Where the simulated handler sets `xmm9` in the frame whose `ucontext_t` is at `ucontext`.
    pub fn fp_changed_in_frame(ucontext: u64, read_word: impl Fn(u64) -> u64) -> u64 {
        read_word(ucontext + 40 + 184) + 160 + 9 * 16
    }

    /// The four registers the simulated fault-handler program sets (`r12` to `r15`).
    pub fn fault_registers(registers: &mut UserRegisters) -> [&mut u64; 4] {
        [
            &mut registers.r12,
            &mut registers.r13,
            &mut registers.r14,
            &mut registers.r15,
        ]
    }
    pub const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] =
        [40 + 4 * 8, 40 + 5 * 8, 40 + 6 * 8, 40 + 7 * 8, 40 + 16 * 8];

    /// Whether the machine context reports a fault at `address`: `cr2` and `trapno` 14.
    pub fn reports_fault(word: impl Fn(u64) -> u64, address: u64) -> bool {
        word(40 + 176) == address && word(40 + 160) == 14
    }

    /// A read's page fault as the test platform encodes it.
    pub const READ_FAULT_CODE: u64 = 0;
}
