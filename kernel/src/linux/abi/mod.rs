//! The Linux ABI of each architecture (D218): one file per architecture, `<arch>.rs`, selected
//! here and nowhere else, holds every fact the Linux layer needs that differs between them:
//! system-call numbers, struct layouts (`stat`, `sigaction`, `epoll_event`), open and `mmap`
//! flags, signal-frame layouts and code, fault encodings, the `uname` machine and the ELF
//! machine. Everything else in `kernel/src` and `drivers/src` is architecture-independent
//! (`cargo xtask lint` refuses `target_arch` outside this folder).
//!
//! Each file also has a `wants` module, Linux's `__ARCH_WANT_*`: the calls the architecture
//! keeps beyond the generic table. The kernel's build script turns each `true` into
//! `cfg(linux_abi_<want>)`, which gates the code of those calls. Adding an architecture's Linux
//! ABI is one new file here.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(target_arch = "loongarch64")]
mod loongarch64;
#[cfg(target_arch = "loongarch64")]
pub use loongarch64::*;

#[cfg(target_arch = "mips64")]
mod mips64;
#[cfg(target_arch = "mips64")]
pub use mips64::*;

#[cfg(target_arch = "powerpc64")]
mod powerpc64;
#[cfg(target_arch = "powerpc64")]
pub use powerpc64::*;

#[cfg(target_arch = "s390x")]
mod s390x;
#[cfg(target_arch = "s390x")]
pub use s390x::*;

#[cfg(target_arch = "sparc64")]
mod sparc64;
#[cfg(target_arch = "sparc64")]
pub use sparc64::*;

#[cfg(any(target_arch = "mips64", target_arch = "sparc64"))]
mod translate;

#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

#[cfg(target_arch = "riscv32")]
mod riscv32;
#[cfg(target_arch = "riscv32")]
pub use riscv32::*;

#[cfg(target_arch = "arm")]
mod arm;
#[cfg(target_arch = "arm")]
pub use arm::*;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
mod riscv_misaligned;

/// Does a misaligned load or store a program's instruction trapped on, where the architecture
/// leaves that to the kernel (RISC-V, D560): whether it did. Never elsewhere.
pub fn emulate_misaligned(
    registers: &mut eggos_hal::user::UserRegisters,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
    write: &mut dyn FnMut(u64, &[u8]) -> bool,
) -> bool {
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    {
        riscv_misaligned::emulate(registers, read, write)
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "riscv32")))]
    {
        let _ = (registers, read, write);
        false
    }
}

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "loongarch64",
    target_arch = "mips64",
    target_arch = "powerpc64",
    target_arch = "riscv64",
    target_arch = "riscv32",
    target_arch = "arm",
    target_arch = "s390x",
    target_arch = "sparc64",
    target_arch = "x86_64"
)))]
compile_error!("EggOS has no Linux ABI for this architecture: add kernel/src/linux/abi/<arch>.rs");

/// Bytes of a C `long` and of a pointer in the architecture's user programs: 4 on the 32-bit
/// ones (D485), 8 elsewhere. The structures the generic code reads (`iovec` first) are laid out
/// in these words.
#[cfg(any(target_arch = "riscv32", target_arch = "arm"))]
pub const WORD_BYTES: usize = 4;
/// Bytes of a C `long` and of a pointer in the architecture's user programs: 4 on the 32-bit
/// ones (D485), 8 elsewhere.
#[cfg(not(any(target_arch = "riscv32", target_arch = "arm")))]
pub const WORD_BYTES: usize = 8;

/// The `long` or pointer at the start of `bytes` ([`WORD_BYTES`] of them), widened.
pub fn word(bytes: &[u8]) -> u64 {
    match WORD_BYTES {
        4 => u64::from(u32::from_ne_bytes(
            bytes[..4].try_into().unwrap_or_default(),
        )),
        _ => u64::from_ne_bytes(bytes[..8].try_into().unwrap_or_default()),
    }
}

/// Writes `value` as a `long` or pointer at the start of `bytes`, cut to [`WORD_BYTES`].
pub fn put_word(bytes: &mut [u8], value: u64) {
    match WORD_BYTES {
        4 => bytes[..4].copy_from_slice(&(value as u32).to_ne_bytes()),
        _ => bytes[..8].copy_from_slice(&value.to_ne_bytes()),
    }
}

/// The signal numbers every architecture but MIPS shares (`asm-generic/signal.h`, x86's and
/// PowerPC's too); MIPS (D315) and SPARC64 (D327) have their own.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
pub mod signal {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGABRT: u32 = 6;
    pub const SIGBUS: u32 = 7;
    pub const SIGFPE: u32 = 8;
    pub const SIGKILL: u32 = 9;
    pub const SIGUSR1: u32 = 10;
    pub const SIGSEGV: u32 = 11;
    pub const SIGUSR2: u32 = 12;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGSTKFLT: u32 = 16;
    pub const SIGCHLD: u32 = 17;
    pub const SIGCONT: u32 = 18;
    pub const SIGSTOP: u32 = 19;
    pub const SIGTSTP: u32 = 20;
    pub const SIGTTIN: u32 = 21;
    pub const SIGTTOU: u32 = 22;
    pub const SIGURG: u32 = 23;
    pub const SIGXCPU: u32 = 24;
    pub const SIGXFSZ: u32 = 25;
    pub const SIGVTALRM: u32 = 26;
    pub const SIGPROF: u32 = 27;
    pub const SIGWINCH: u32 = 28;
    pub const SIGIO: u32 = 29;
    pub const SIGPWR: u32 = 30;
    pub const SIGSYS: u32 = 31;
}

/// The page size programs are told (`AT_PAGESZ`): the kernel's 4 KiB everywhere but on
/// SPARC64, whose MMU maps 8 KiB pairs (D328).
#[cfg(not(target_arch = "sparc64"))]
pub const REPORTED_PAGE_SIZE: u64 = eggos_hal::memory::FRAME_SIZE;

/// The bytes between a new program's stack pointer and its `argc`: none everywhere but on
/// SPARC64, whose register window's save area is there (D418).
#[cfg(not(target_arch = "sparc64"))]
pub const START_STACK_GAP: u64 = 0;

/// `clone` takes the flags first and the stack second everywhere but on s390x.
#[cfg(not(target_arch = "s390x"))]
pub const CLONE_STACK_FIRST: bool = false;
/// `mmap` takes its arguments in registers everywhere but on s390x.
#[cfg(not(target_arch = "s390x"))]
pub const MMAP_ARGUMENTS_IN_MEMORY: bool = false;

/// Whether a 64-bit value split over two registers starts at an even one (32-bit Arm's EABI,
/// D491); no other port splits values.
#[cfg(not(target_arch = "arm"))]
pub const ALIGNED_REGISTER_PAIRS: bool = false;
/// Whether `fadvise64_64` takes the advice second, before the offset and length (32-bit Arm's
/// `arm_fadvise64_64`, D491), rather than last.
#[cfg(not(target_arch = "arm"))]
pub const FADVISE_ADVICE_SECOND: bool = false;
/// The alignment of `struct statfs` (or `statfs64`): 8 everywhere but on 32-bit Arm, whose
/// `statfs64` is packed to 4 (D491).
#[cfg(not(target_arch = "arm"))]
pub const STATFS_ALIGN: usize = 8;
/// The number of a private call past the table that sets the thread pointer: 32-bit Arm's
/// `set_tls` (D492), none elsewhere.
#[cfg(not(target_arch = "arm"))]
pub const SET_TLS_NUMBER: Option<u64> = None;

/// `struct sigaction`'s handler comes first everywhere but on MIPS.
#[cfg(not(target_arch = "mips64"))]
pub const SIGACTION_FLAGS_FIRST: bool = false;
/// `stack_t` is `ss_sp`, `ss_flags`, `ss_size` everywhere but on MIPS.
#[cfg(not(target_arch = "mips64"))]
pub const STACK_T_SIZE_BEFORE_FLAGS: bool = false;
/// `sigset_t` as the calls take it: 64 signals in 8 bytes everywhere but on MIPS.
#[cfg(not(target_arch = "mips64"))]
pub const SIGSET_SIZE: u64 = 8;
/// Whether some calls' arguments are rewritten before dispatch: only on MIPS.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
pub const REWRITES_ARGUMENTS: bool = false;

/// Rewrites a call's arguments into the generic values: nothing to do here.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn arguments_in(_number: u64, _arguments: &mut [u64; 6]) -> bool {
    false
}

/// A call's result in the architecture's values: the generic ones here.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn result_out(_number: u64, _arguments: [u64; 6], result: u64) -> u64 {
    result
}

/// `sa_flags` as the kernel takes them: as given here.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn sigaction_flags_in(flags: u64) -> u64 {
    flags
}

/// `sa_flags` as a program reads them: as kept here.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn sigaction_flags_out(flags: u64) -> u64 {
    flags
}

/// Puts `siginfo_t`'s `si_errno` and `si_code` in the architecture's order: as the kernel
/// keeps them (`si_errno` first) everywhere but on MIPS. Its own inverse.
#[cfg(not(target_arch = "mips64"))]
#[inline(always)]
pub fn reorder_siginfo(_info: &mut [u8]) {}

/// `struct pollfd`'s requested events as the kernel takes them: as given everywhere but on MIPS.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn poll_events_in(events: u16) -> u16 {
    events
}

/// `struct pollfd`'s returned events as the program reads them: as kept everywhere but on MIPS.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn poll_events_out(events: u16) -> u16 {
    events
}

/// The `int` a `SOL_SOCKET` option `name` reads back, in the architecture's values: as kept
/// everywhere but on MIPS.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
#[inline(always)]
pub fn socket_option_out(_name: u32, value: u32) -> u32 {
    value
}

/// `struct termios` as programs pass it to `TCGETS` and `TCSETS*`: the kernel's own layout
/// everywhere but on MIPS.
#[cfg(not(any(target_arch = "mips64", target_arch = "sparc64")))]
pub mod termios {
    use crate::console::line_discipline::Termios;

    /// Its size.
    pub const SIZE: usize = Termios::SIZE;

    /// `settings` as a program reads them.
    pub fn to_program(settings: &Termios) -> [u8; SIZE] {
        settings.to_bytes()
    }

    /// The settings a program wrote.
    pub fn from_program(bytes: &[u8; SIZE]) -> Termios {
        Termios::from_bytes(bytes)
    }
}

/// What the architectures on Linux's generic ABI (`asm-generic`) share: AArch64, RISC-V and
/// the later ones (LoongArch).
pub mod generic {
    /// The terminal and descriptor `ioctl` numbers the generic architectures and x86_64 share
    /// (`asm-generic/ioctls.h`); PowerPC has its own.
    pub mod tty {
        pub const TCGETS: u32 = 0x5401;
        pub const TCSETS: u32 = 0x5402;
        pub const TCSETSW: u32 = 0x5403;
        pub const TCSETSF: u32 = 0x5404;
        pub const TCFLSH: u32 = 0x540b;
        pub const TIOCGWINSZ: u32 = 0x5413;
        pub const TIOCSWINSZ: u32 = 0x5414;
        pub const FIONREAD: u32 = 0x541b;
        pub const TCGETS2: u32 = 0x802c_542a;
        pub const TCSETS2: u32 = 0x402c_542b;
        pub const TCSETSW2: u32 = 0x402c_542c;
        pub const TCSETSF2: u32 = 0x402c_542d;
        pub const FIONBIO: u32 = 0x5421;
        pub const FIONCLEX: u32 = 0x5450;
        pub const FIOCLEX: u32 = 0x5451;
        pub const FIOASYNC: u32 = 0x5452;
        pub const TIOCSCTTY: u32 = 0x540e;
        pub const TIOCGPGRP: u32 = 0x540f;
        pub const TIOCSPGRP: u32 = 0x5410;
        pub const TIOCGSID: u32 = 0x5429;
        pub const TIOCGPTN: u32 = 0x8004_5430;
        pub const TIOCSPTLCK: u32 = 0x4004_5431;
        pub const TIOCGPTLCK: u32 = 0x8004_5439;
        pub const TIOCPKT: u32 = 0x5420;
        pub const TIOCGPKT: u32 = 0x8004_5438;
        pub const TIOCGPTPEER: u32 = 0x5441;
        pub const TIOCSTI: u32 = 0x5412;
    }

    /// The generic system-call table (`asm-generic/unistd.h`), without the calls an
    /// architecture adds through its `wants`.
    pub mod number {
        pub const GETCWD: u64 = 17;
        pub const DUP: u64 = 23;
        pub const DUP3: u64 = 24;
        pub const FCNTL: u64 = 25;
        pub const IOCTL: u64 = 29;
        pub const CHDIR: u64 = 49;
        pub const FCHDIR: u64 = 50;
        pub const OPENAT: u64 = 56;
        pub const CLOSE: u64 = 57;
        pub const PIPE2: u64 = 59;
        pub const MKDIRAT: u64 = 34;
        pub const UNLINKAT: u64 = 35;
        pub const FTRUNCATE: u64 = 46;
        pub const PWRITE64: u64 = 68;
        pub const UTIMENSAT: u64 = 88;
        pub const SYNC: u64 = 81;
        pub const FSYNC: u64 = 82;
        pub const FDATASYNC: u64 = 83;
        pub const SYNCFS: u64 = 267;
        pub const RENAMEAT2: u64 = 276;
        pub const GETDENTS64: u64 = 61;
        pub const LSEEK: u64 = 62;
        pub const READ: u64 = 63;
        pub const WRITE: u64 = 64;
        pub const READV: u64 = 65;
        pub const WRITEV: u64 = 66;
        pub const PREAD64: u64 = 67;
        pub const PPOLL: u64 = 73;
        pub const NEWFSTATAT: u64 = 79;
        pub const FSTAT: u64 = 80;
        pub const EXIT: u64 = 93;
        pub const EXIT_GROUP: u64 = 94;
        pub const WAITID: u64 = 95;
        pub const SET_TID_ADDRESS: u64 = 96;
        pub const FUTEX: u64 = 98;
        pub const SET_ROBUST_LIST: u64 = 99;
        pub const GET_ROBUST_LIST: u64 = 100;
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
        pub const REBOOT: u64 = 142;
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
        pub const SETPGID: u64 = 154;
        pub const GETPGID: u64 = 155;
        pub const GETSID: u64 = 156;
        pub const SETSID: u64 = 157;
        pub const UNAME: u64 = 160;
        pub const GETRLIMIT: u64 = 163;
        pub const SETRLIMIT: u64 = 164;
        pub const PRCTL: u64 = 167;
        pub const GETCPU: u64 = 168;
        pub const SCHED_GETSCHEDULER: u64 = 120;
        pub const SCHED_GETPARAM: u64 = 121;
        pub const SCHED_SETAFFINITY: u64 = 122;
        pub const SCHED_GETAFFINITY: u64 = 123;
        pub const TIMES: u64 = 153;
        pub const GETRUSAGE: u64 = 165;
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
        pub const EXECVE: u64 = 221;
        pub const MUNMAP: u64 = 215;
        pub const MREMAP: u64 = 216;
        pub const MMAP: u64 = 222;
        pub const MPROTECT: u64 = 226;
        pub const MSYNC: u64 = 227;
        pub const MADVISE: u64 = 233;
        pub const RT_TGSIGQUEUEINFO: u64 = 240;
        pub const WAIT4: u64 = 260;
        pub const PRLIMIT64: u64 = 261;
        pub const GETRANDOM: u64 = 278;
        pub const EXECVEAT: u64 = 281;
        pub const STATX: u64 = 291;
        pub const CLOSE_RANGE: u64 = 436;
        pub const SOCKET: u64 = 198;
        pub const SOCKETPAIR: u64 = 199;
        pub const BIND: u64 = 200;
        pub const LISTEN: u64 = 201;
        pub const ACCEPT: u64 = 202;
        pub const CONNECT: u64 = 203;
        pub const GETSOCKNAME: u64 = 204;
        pub const GETPEERNAME: u64 = 205;
        pub const SENDTO: u64 = 206;
        pub const RECVFROM: u64 = 207;
        pub const SETSOCKOPT: u64 = 208;
        pub const GETSOCKOPT: u64 = 209;
        pub const SHUTDOWN: u64 = 210;
        pub const SENDMSG: u64 = 211;
        pub const RECVMSG: u64 = 212;
        pub const ACCEPT4: u64 = 242;
        pub const FALLOCATE: u64 = 47;
        pub const MEMFD_CREATE: u64 = 279;
        pub const FACCESSAT: u64 = 48;
        pub const READLINKAT: u64 = 78;
        pub const SETHOSTNAME: u64 = 161;
        pub const SETDOMAINNAME: u64 = 162;
        pub const FACCESSAT2: u64 = 439;
        pub const EVENTFD2: u64 = 19;
        pub const INOTIFY_INIT1: u64 = 26;
        pub const INOTIFY_ADD_WATCH: u64 = 27;
        pub const INOTIFY_RM_WATCH: u64 = 28;
        pub const EPOLL_CREATE1: u64 = 20;
        pub const EPOLL_CTL: u64 = 21;
        pub const EPOLL_PWAIT: u64 = 22;
        pub const PSELECT6: u64 = 72;
        pub const SIGNALFD4: u64 = 74;
        pub const TIMERFD_CREATE: u64 = 85;
        pub const TIMERFD_SETTIME: u64 = 86;
        pub const TIMERFD_GETTIME: u64 = 87;
        pub const EPOLL_PWAIT2: u64 = 441;
        pub const SYMLINKAT: u64 = 36;
        pub const LINKAT: u64 = 37;
        pub const FCHMOD: u64 = 52;
        pub const FCHMODAT: u64 = 53;
        pub const OPENAT2: u64 = 437;
        pub const FCHMODAT2: u64 = 452;
        pub const FLOCK: u64 = 32;
        pub const STATFS: u64 = 43;
        pub const FSTATFS: u64 = 44;
        pub const SYSINFO: u64 = 179;
        pub const FADVISE64: u64 = 223;
        /// The highest number above.
        pub const UMASK: u64 = 166;
        pub const FCHOWN: u64 = 55;
        pub const FCHOWNAT: u64 = 54;
        pub const PIDFD_OPEN: u64 = 434;
        pub const PIDFD_SEND_SIGNAL: u64 = 424;
        pub const PIDFD_GETFD: u64 = 438;
        pub const SENDFILE: u64 = 71;
        pub const SPLICE: u64 = 76;
        pub const TEE: u64 = 77;
        pub const VMSPLICE: u64 = 75;
        pub const COPY_FILE_RANGE: u64 = 285;
        pub const SETXATTR: u64 = 5;
        pub const LSETXATTR: u64 = 6;
        pub const FSETXATTR: u64 = 7;
        pub const GETXATTR: u64 = 8;
        pub const LGETXATTR: u64 = 9;
        pub const FGETXATTR: u64 = 10;
        pub const LISTXATTR: u64 = 11;
        pub const LLISTXATTR: u64 = 12;
        pub const FLISTXATTR: u64 = 13;
        pub const REMOVEXATTR: u64 = 14;
        pub const LREMOVEXATTR: u64 = 15;
        pub const FREMOVEXATTR: u64 = 16;
        pub const SENDMMSG: u64 = 269;
        pub const RECVMMSG: u64 = 243;
        pub const CAPGET: u64 = 90;
        pub const CAPSET: u64 = 91;
        pub const PTRACE: u64 = 117;
        pub const FANOTIFY_INIT: u64 = 262;
        pub const FANOTIFY_MARK: u64 = 263;
        pub const LAST: u64 = FCHMODAT2;
    }

    /// What the generic architectures' signal frames share: `struct rt_sigframe { siginfo_t
    /// info; ucontext_t uc; }`, the `ucontext_t` holding `uc_sigmask`, 120 unused bytes (room
    /// for a larger `sigset_t`), then `uc_mcontext`, 16-byte aligned. The assertions hold
    /// Linux's offsets from the frame, written out: what programs read.
    pub mod signal_frame {
        use crate::linux::signal_frame::{INFO_SIZE, STACK_SIZE, ucontext};

        pub const UCONTEXT: usize = INFO_SIZE;
        pub const SIGMASK: usize = UCONTEXT + ucontext::STACK + STACK_SIZE;
        pub const MCONTEXT: usize = UCONTEXT + (SIGMASK - UCONTEXT + 128).next_multiple_of(16);
        const _: () = assert!(
            super::super::WORD_BYTES != 8 || UCONTEXT == 128 && SIGMASK == 168 && MCONTEXT == 304
        );
        const _: () = assert!(
            super::super::WORD_BYTES != 4 || UCONTEXT == 128 && SIGMASK == 148 && MCONTEXT == 288
        );
    }
}

/// The layouts of `struct stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatLayout {
    /// x86_64's: a 64-bit `st_nlink` before `st_mode`, a 64-bit `st_blksize`.
    X86_64,
    /// The generic one: `st_mode` then a 32-bit `st_nlink`, a 32-bit `st_blksize`.
    Generic,
    /// MIPS n64's: a 32-bit `st_dev` and `st_rdev` with padding, 32-bit times (D315).
    Mips64,
    /// s390x's: x86_64's first fields, then the times before `st_blksize` and `st_blocks`
    /// (D323).
    S390x,
    /// SPARC64's: a 32-bit `st_dev` and `st_rdev`, a 16-bit `st_nlink`, whole-second times
    /// (D327).
    Sparc64,
}

/// Three instructions as code bytes, in the CPU's own byte order (D318).
pub const fn code_bytes(words: [u32; 3]) -> [u8; 12] {
    let mut bytes = [0; 12];
    let mut index = 0;
    while index < 12 {
        bytes[index] = words[index / 4].to_ne_bytes()[index % 4];
        index += 1;
    }
    bytes
}
