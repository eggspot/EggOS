//! 64-bit little-endian PowerPC's Linux ABI (D218, D304): its own table (`asm/unistd.h`),
//! structs and terminal `ioctl` numbers: see [`super`]. A system call's number is in `r0` and
//! its arguments in `r3` to `r8`; it returns in `r3`, with `CR0.SO` set and the error number
//! positive when it failed (the HAL's registers turn the kernel's negative results into that).

/// The calls PowerPC keeps beyond the generic table: the ones older than the `*at` family and
/// `renameat`; no `arch_prctl`, since a program sets its thread pointer, `r13`, itself.
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on 64-bit PowerPC.
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
    pub const TIME: u64 = 13;
    pub const CHMOD: u64 = 15;
    pub const LSEEK: u64 = 19;
    pub const GETPID: u64 = 20;
    pub const SETUID: u64 = 23;
    pub const GETUID: u64 = 24;
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
    pub const SETGID: u64 = 46;
    pub const GETGID: u64 = 47;
    pub const GETEUID: u64 = 49;
    pub const GETEGID: u64 = 50;
    pub const IOCTL: u64 = 54;
    pub const FCNTL: u64 = 55;
    pub const SETPGID: u64 = 57;
    pub const DUP2: u64 = 63;
    pub const GETPPID: u64 = 64;
    pub const GETPGRP: u64 = 65;
    pub const SETSID: u64 = 66;
    pub const SETREUID: u64 = 70;
    pub const SETREGID: u64 = 71;
    pub const SETHOSTNAME: u64 = 74;
    pub const SETRLIMIT: u64 = 75;
    pub const GETRUSAGE: u64 = 77;
    pub const GETTIMEOFDAY: u64 = 78;
    pub const SETTIMEOFDAY: u64 = 79;
    pub const GETGROUPS: u64 = 80;
    pub const SETGROUPS: u64 = 81;
    pub const SYMLINK: u64 = 83;
    pub const READLINK: u64 = 85;
    pub const REBOOT: u64 = 88;
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
    pub const SETFSUID: u64 = 138;
    pub const SETFSGID: u64 = 139;
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
    pub const SETRESUID: u64 = 164;
    pub const GETRESUID: u64 = 165;
    pub const POLL: u64 = 167;
    pub const SETRESGID: u64 = 169;
    pub const GETRESGID: u64 = 170;
    pub const PRCTL: u64 = 171;
    pub const RT_SIGRETURN: u64 = 172;
    pub const RT_SIGACTION: u64 = 173;
    pub const RT_SIGPROCMASK: u64 = 174;
    pub const RT_SIGPENDING: u64 = 175;
    pub const RT_SIGTIMEDWAIT: u64 = 176;
    pub const RT_SIGQUEUEINFO: u64 = 177;
    pub const RT_SIGSUSPEND: u64 = 178;
    pub const PREAD64: u64 = 179;
    pub const PWRITE64: u64 = 180;
    pub const GETCWD: u64 = 182;
    pub const SIGALTSTACK: u64 = 185;
    pub const VFORK: u64 = 189;
    /// `ugetrlimit`, the `getrlimit` with Linux's own `struct rlimit` (76 is the old one).
    pub const GETRLIMIT: u64 = 190;
    pub const GETDENTS64: u64 = 202;
    pub const MADVISE: u64 = 205;
    pub const GETTID: u64 = 207;
    pub const TKILL: u64 = 208;
    pub const FUTEX: u64 = 221;
    pub const SCHED_SETAFFINITY: u64 = 222;
    pub const SCHED_GETAFFINITY: u64 = 223;
    pub const SET_TID_ADDRESS: u64 = 232;
    pub const FADVISE64: u64 = 233;
    pub const EXIT_GROUP: u64 = 234;
    pub const EPOLL_CREATE: u64 = 236;
    pub const EPOLL_CTL: u64 = 237;
    pub const EPOLL_WAIT: u64 = 238;
    pub const CLOCK_SETTIME: u64 = 245;
    pub const CLOCK_GETTIME: u64 = 246;
    pub const CLOCK_GETRES: u64 = 247;
    pub const CLOCK_NANOSLEEP: u64 = 248;
    pub const TGKILL: u64 = 250;
    pub const WAITID: u64 = 272;
    pub const INOTIFY_INIT: u64 = 275;
    pub const INOTIFY_ADD_WATCH: u64 = 276;
    pub const INOTIFY_RM_WATCH: u64 = 277;
    pub const PSELECT6: u64 = 280;
    pub const PPOLL: u64 = 281;
    pub const OPENAT: u64 = 286;
    pub const MKDIRAT: u64 = 287;
    pub const NEWFSTATAT: u64 = 291;
    pub const UNLINKAT: u64 = 292;
    pub const RENAMEAT: u64 = 293;
    pub const LINKAT: u64 = 294;
    pub const SYMLINKAT: u64 = 295;
    pub const READLINKAT: u64 = 296;
    pub const FCHMODAT: u64 = 297;
    pub const FACCESSAT: u64 = 298;
    pub const GET_ROBUST_LIST: u64 = 299;
    pub const SET_ROBUST_LIST: u64 = 300;
    pub const GETCPU: u64 = 302;
    pub const EPOLL_PWAIT: u64 = 303;
    pub const UTIMENSAT: u64 = 304;
    pub const SIGNALFD: u64 = 305;
    pub const TIMERFD_CREATE: u64 = 306;
    pub const EVENTFD: u64 = 307;
    pub const FALLOCATE: u64 = 309;
    pub const TIMERFD_SETTIME: u64 = 311;
    pub const TIMERFD_GETTIME: u64 = 312;
    pub const SIGNALFD4: u64 = 313;
    pub const EVENTFD2: u64 = 314;
    pub const EPOLL_CREATE1: u64 = 315;
    pub const DUP3: u64 = 316;
    pub const PIPE2: u64 = 317;
    pub const INOTIFY_INIT1: u64 = 318;
    pub const RT_TGSIGQUEUEINFO: u64 = 322;
    pub const PRLIMIT64: u64 = 325;
    pub const SOCKET: u64 = 326;
    pub const BIND: u64 = 327;
    pub const CONNECT: u64 = 328;
    pub const LISTEN: u64 = 329;
    pub const ACCEPT: u64 = 330;
    pub const GETSOCKNAME: u64 = 331;
    pub const GETPEERNAME: u64 = 332;
    pub const SOCKETPAIR: u64 = 333;
    pub const SENDTO: u64 = 335;
    pub const RECVFROM: u64 = 337;
    pub const SHUTDOWN: u64 = 338;
    pub const SETSOCKOPT: u64 = 339;
    pub const GETSOCKOPT: u64 = 340;
    pub const SENDMSG: u64 = 341;
    pub const RECVMSG: u64 = 342;
    pub const ACCEPT4: u64 = 344;
    pub const SYNCFS: u64 = 348;
    pub const RENAMEAT2: u64 = 357;
    pub const GETRANDOM: u64 = 359;
    pub const MEMFD_CREATE: u64 = 360;
    pub const EXECVEAT: u64 = 362;
    pub const STATX: u64 = 383;
    pub const CLOSE_RANGE: u64 = 436;
    pub const OPENAT2: u64 = 437;
    pub const FACCESSAT2: u64 = 439;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const FCHMODAT2: u64 = 452;
    /// The highest number above.
    pub const LAST: u64 = FCHMODAT2;
}

/// PowerPC's terminal and descriptor `ioctl` numbers (`asm/ioctls.h`): its `_IOC` puts the
/// direction in the top three bits (none 1, read 2, write 4) and has no `termios2`, whose
/// requests get numbers no program sends.
pub mod tty {
    pub const TCGETS: u32 = 0x402c_7413;
    pub const TCSETS: u32 = 0x802c_7414;
    pub const TCSETSW: u32 = 0x802c_7415;
    pub const TCSETSF: u32 = 0x802c_7416;
    pub const TCFLSH: u32 = 0x2000_741f;
    pub const TIOCGWINSZ: u32 = 0x4008_7468;
    pub const TIOCSWINSZ: u32 = 0x8008_7467;
    pub const FIONREAD: u32 = 0x4004_667f;
    pub const TCGETS2: u32 = 0xffff_fff1;
    pub const TCSETS2: u32 = 0xffff_fff2;
    pub const TCSETSW2: u32 = 0xffff_fff3;
    pub const TCSETSF2: u32 = 0xffff_fff4;
    pub const FIONBIO: u32 = 0x8004_667e;
    pub const FIONCLEX: u32 = 0x2000_6602;
    pub const FIOCLEX: u32 = 0x2000_6601;
    pub const FIOASYNC: u32 = 0x8004_667d;
    pub const TIOCSCTTY: u32 = 0x540e;
    pub const TIOCGPGRP: u32 = 0x4004_7477;
    pub const TIOCSPGRP: u32 = 0x8004_7476;
    pub const TIOCGSID: u32 = 0x5429;
    pub const TIOCGPTN: u32 = 0x4004_5430;
    pub const TIOCSPTLCK: u32 = 0x8004_5431;
    pub const TIOCGPTLCK: u32 = 0x4004_5439;
    pub const TIOCPKT: u32 = 0x5420;
    pub const TIOCGPKT: u32 = 0x4004_5438;
    pub const TIOCGPTPEER: u32 = 0x2000_5441;
    pub const TIOCSTI: u32 = 0x5412;
}

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "ppc64le";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::PPC64;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// `clone`'s fourth argument is `parent_tid`, then `tls`, then `child_tid`, as on the
/// generic architectures.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// `struct sigaction` has `sa_restorer`, but programs return through the kernel's trampoline,
/// as through Linux's vDSO, unless they set `SA_RESTORER`.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 8192;

const LI_R0_RT_SIGRETURN: u32 = 0x3800_0000 | number::RT_SIGRETURN as u32;
const SC: u32 = 0x4400_0002;
const NOP: u32 = 0x6000_0000;
/// The kernel's `rt_sigreturn` trampoline: `li r0, 172`, `sc` and a `nop` that fills the
/// three words.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> = Some(&super::code_bytes([LI_R0_RT_SIGRETURN, SC, NOP]));

/// Linux's `stat` here: the same 144 bytes as x86_64's.
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
    pub const DIRECTORY: u32 = 0o40_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o100_000;
}

/// The `mmap` bits only some architectures have.
pub mod mmap {
    /// `PROT_SAO`: strong access ordering, which the kernel takes and ignores.
    pub const PROT_ARCH: u64 = 0x10;
    /// `MAP_32BIT`: none here.
    pub const MAP_32BIT: u64 = 0;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `struct rt_sigframe { struct ucontext uc; unsigned long _unused[2]; unsigned int tramp[7];
/// siginfo_t *pinfo; void *puc; siginfo_t info; char abigap[512]; }`. The 64-bit `ucontext`
/// keeps `uc_sigmask` at 40 and `uc_mcontext` (`struct sigcontext`) at 168: `signal`,
/// `handler`, `oldmask` and `regs`, then `gp_regs` (`gpr0` to `gpr31`, `nip`, `msr`,
/// `orig_gpr3`, `ctr`, `link`, `xer`, `ccr`, `softe`, `trap`, `dar`, `dsisr`, `result`, 48
/// words), `fp_regs` (`fpr0` to `fpr31`, `fpscr`), `v_regs` (a pointer) and the vector area it
/// points at, 16-byte aligned: `vr0` to `vr31`, `vscr`, `vrsave`, then the low doublewords of
/// `vs0` to `vs31`. The handler's stack pointer is 128 below the frame, with the back chain.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{
        BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    pub const UCONTEXT: usize = 0;
    pub const SIGMASK: usize = 40;
    pub const MCONTEXT: usize = 168;
    const SIGNAL: usize = MCONTEXT + 32;
    const HANDLER: usize = MCONTEXT + 40;
    const OLDMASK: usize = MCONTEXT + 48;
    const REGS: usize = MCONTEXT + 56;
    pub const GP_REGS: usize = MCONTEXT + 64;
    pub const FP_REGS: usize = GP_REGS + 48 * 8;
    const V_REGS_POINTER: usize = FP_REGS + 33 * 8;
    pub const V_REGS: usize = (V_REGS_POINTER + 8 + 15) & !15;
    const VSX_LOW: usize = V_REGS + 34 * 16;
    const UCONTEXT_END: usize = MCONTEXT + 1528;
    const TRAMPOLINE: usize = UCONTEXT_END + 16;
    const PINFO: usize = TRAMPOLINE + 32;
    const PUC: usize = PINFO + 8;
    pub const INFO: usize = PUC + 8;
    const ABI_GAP: usize = 512;
    /// The whole frame.
    pub const WRITTEN: usize = INFO + INFO_SIZE + ABI_GAP;
    pub const READ: usize = WRITTEN;
    const _: () = assert!(V_REGS + 34 * 16 + 32 * 8 <= UCONTEXT_END);
    /// `__SIGNAL_FRAMESIZE`: the handler's frame below the signal frame.
    pub const HANDLER_FRAME: u64 = 128;

    const NIP: usize = 32;
    const MSR: usize = 33;
    const ORIG_GPR3: usize = 34;
    const CTR: usize = 35;
    const LINK: usize = 36;
    const XER: usize = 37;
    const CCR: usize = 38;
    const TRAP: usize = 40;
    const DAR: usize = 41;
    const DSISR: usize = 42;

    /// `fpr0` to `fpr31` and `fpscr`, then the vector area: `vr0` to `vr31`, `vscr` and
    /// `vrsave` in 16-byte slots, then the low doublewords of `vs0` to `vs31`.
    pub type FpPart = [u8; 33 * 8 + 34 * 16 + 32 * 8];
    const VECTOR_PART: usize = 33 * 8;
    const VSX_PART: usize = VECTOR_PART + 34 * 16;

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;
    /// The 288 bytes below the stack pointer the ELFv2 ABI keeps for leaf functions.
    pub const RED_ZONE: u64 = 288;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; 33 * 8 + 34 * 16 + 32 * 8];
        for (index, vsr) in state.vsr().iter().enumerate().take(32) {
            let high = (*vsr >> 64) as u64;
            let low = *vsr as u64;
            part[index * 8..][..8].copy_from_slice(&high.to_ne_bytes());
            part[VSX_PART + index * 8..][..8].copy_from_slice(&low.to_ne_bytes());
        }
        part[32 * 8..][..8].copy_from_slice(&state.fpscr().to_ne_bytes());
        for (index, vr) in state.vsr().iter().skip(32).enumerate() {
            part[VECTOR_PART + index * 16..][..16].copy_from_slice(&vr.to_ne_bytes());
        }
        part[VECTOR_PART + 32 * 16..][..4].copy_from_slice(&state.vscr().to_ne_bytes());
        part[VECTOR_PART + 33 * 16..][..4].copy_from_slice(&state.vrsave().to_ne_bytes());
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for index in 0..32 {
            let high = u128::from(word(part, index * 8));
            let low = u128::from(word(part, VSX_PART + index * 8));
            state.vsr_mut()[index] = high << 64 | low;
        }
        for index in 0..32 {
            let bytes: [u8; 16] = part[VECTOR_PART + index * 16..][..16].try_into().unwrap();
            state.vsr_mut()[32 + index] = u128::from_ne_bytes(bytes);
        }
        state.set_fpscr(word(part, 32 * 8));
        state.set_vscr(u32::from_ne_bytes(part[VECTOR_PART + 32 * 16..][..4].try_into().unwrap()));
        state.set_vrsave(u32::from_ne_bytes(part[VECTOR_PART + 33 * 16..][..4].try_into().unwrap()));
        Ok(())
    }

    /// 16-byte aligned below `top`, with the handler's frame below it.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(WRITTEN as u64) & !15
    }

    fn gp_regs(registers: &UserRegisters) -> [u64; 48] {
        let mut regs = [0; 48];
        regs[..32].copy_from_slice(&registers.gpr);
        regs[NIP] = registers.pc;
        regs[MSR] = registers.msr;
        regs[ORIG_GPR3] = registers.gpr[3];
        regs[CTR] = registers.ctr;
        regs[LINK] = registers.lr;
        regs[XER] = registers.xer;
        regs[CCR] = registers.cr;
        regs
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
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, SIGMASK, saved.blocked);
        put(&mut frame, OLDMASK, saved.blocked);
        put(&mut frame, REGS, frame_address + GP_REGS as u64);
        for (index, value) in gp_regs(&saved.registers).into_iter().enumerate() {
            put(&mut frame, GP_REGS + index * 8, value);
        }
        if let Some(trap) = trap {
            put(&mut frame, GP_REGS + TRAP * 8, trap.number);
            put(&mut frame, GP_REGS + DAR * 8, trap.address);
            put(&mut frame, GP_REGS + DSISR * 8, trap.cause);
        }
        frame[FP_REGS..][..33 * 8].copy_from_slice(&saved.fp[..33 * 8]);
        put(&mut frame, V_REGS_POINTER, frame_address + V_REGS as u64);
        frame[V_REGS..][..34 * 16].copy_from_slice(&saved.fp[VECTOR_PART..VSX_PART]);
        frame[VSX_LOW..][..32 * 8].copy_from_slice(&saved.fp[VSX_PART..]);
        let _ = (SIGNAL, HANDLER, restorer);
        let trampoline = super::SIGRETURN_TRAMPOLINE.unwrap_or(&[]);
        frame[TRAMPOLINE..][..trampoline.len()].copy_from_slice(trampoline);
        put(&mut frame, PINFO, frame_address + INFO as u64);
        put(&mut frame, PUC, frame_address + UCONTEXT as u64);
        frame[INFO..][..INFO_SIZE].copy_from_slice(info);
        (frame_address, frame)
    }

    /// The handler's back chain, 128 below the frame: the stack pointer it interrupted.
    pub fn write_extra<P: Platform>(
        process: &Process<P>,
        _top: u64,
        frame: u64,
        saved: &Saved,
    ) -> Result<(), BadFrame> {
        process
            .write_user_memory(frame - HANDLER_FRAME, &saved.registers.gpr[1].to_ne_bytes())
            .map_err(|_| BadFrame)
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(
        _: &Process<P>,
        _: &[u8; READ],
        _: &mut Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    /// `r3` the signal, `r4` the `siginfo_t`, `r5` the `ucontext_t`, `r6` the frame; the
    /// handler's address in `r12` too, as ELFv2 functions find their TOC from it; it returns
    /// to its restorer through the link register.
    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.gpr[3] = u64::from(handler.signal);
        registers.gpr[4] = frame + INFO as u64;
        registers.gpr[5] = frame + UCONTEXT as u64;
        registers.gpr[6] = frame;
        registers.gpr[1] = frame - HANDLER_FRAME;
        registers.gpr[12] = handler.entry;
        registers.lr = handler.restorer;
        registers.pc = handler.entry;
    }

    /// The handler returned with its stack pointer where it started.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.gpr[1] + HANDLER_FRAME)
    }

    pub fn parse(frame: &[u8; READ], current: &UserRegisters) -> Option<Saved> {
        let mut gpr = [0; 32];
        for (index, value) in gpr.iter_mut().enumerate() {
            *value = word(frame, GP_REGS + index * 8);
        }
        let mut fp = [0; 33 * 8 + 34 * 16 + 32 * 8];
        fp[..33 * 8].copy_from_slice(&frame[FP_REGS..][..33 * 8]);
        fp[VECTOR_PART..VSX_PART].copy_from_slice(&frame[V_REGS..][..34 * 16]);
        fp[VSX_PART..].copy_from_slice(&frame[VSX_LOW..][..32 * 8]);
        Some(Saved {
            registers: UserRegisters {
                gpr,
                pc: word(frame, GP_REGS + NIP * 8),
                ctr: word(frame, GP_REGS + CTR * 8),
                lr: word(frame, GP_REGS + LINK * 8),
                xer: word(frame, GP_REGS + XER * 8),
                cr: word(frame, GP_REGS + CCR * 8),
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
        registers.gpr[3]
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// `FPE_FLTINV`: an invalid floating-point operation.
    const FPE_FLTINV: i32 = 7;

    /// The signal, `si_code` and `si_addr` Linux gives `exception`; the frame's `trap` is the
    /// interrupt's vector, `dar` the faulting address and `dsisr` its cause.
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
            ExceptionKind::Arithmetic => (signal::SIGFPE, FPE_FLTINV, pc),
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        let trap = Trap {
            number: exception.code >> 32,
            cause: exception.code & 0xffff_ffff,
            address,
        };
        fault(reported, trap)
    }
}
