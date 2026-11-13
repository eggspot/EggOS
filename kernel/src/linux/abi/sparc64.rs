//! SPARC64's Linux ABI (D218, D327): its own table (`arch/sparc/kernel/syscalls/syscall.tbl`, the
//! 64-bit entries) and, like MIPS's, its own values for the signals, `sa_flags`, `struct stat`,
//! `struct termios` and its `ioctl` numbers, the `open`, `mmap` and `fcntl` constants, the
//! socket options and `poll`'s write bits. The kernel works in the generic values:
//! [`arguments_in`] rewrites a call's arguments before it is dispatched and [`result_out`] the
//! results that carry flags; the HAL gives an error SPARC's number, positive with the carry set.
//! A system call is `ta 0x6d` with the number in `%g1` and the arguments in `%o0` to `%o5`.

use super::translate::{FlagPairs, flags_to_architecture, flags_to_generic, value_to_generic};

/// The calls SPARC64 keeps beyond the generic table: the ones older than the `*at` family and
/// `renameat`; no `arch_prctl`, since a program sets its thread pointer, `%g7`, itself.
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on SPARC64. `time` is 32-bit only: past [`LAST`](number::LAST),
/// it answers `ENOSYS`.
pub mod number {
    pub const EXIT: u64 = 1;
    pub const FORK: u64 = 2;
    pub const READ: u64 = 3;
    pub const WRITE: u64 = 4;
    pub const OPEN: u64 = 5;
    pub const CLOSE: u64 = 6;
    pub const WAIT4: u64 = 7;
    pub const LINK: u64 = 9;
    pub const UNLINK: u64 = 10;
    pub const CHDIR: u64 = 12;
    pub const CHMOD: u64 = 15;
    pub const BRK: u64 = 17;
    pub const LSEEK: u64 = 19;
    pub const GETPID: u64 = 20;
    pub const SETUID: u64 = 23;
    pub const GETUID: u64 = 24;
    pub const SIGALTSTACK: u64 = 28;
    pub const PAUSE: u64 = 29;
    pub const ACCESS: u64 = 33;
    pub const SYNC: u64 = 36;
    pub const KILL: u64 = 37;
    pub const STAT: u64 = 38;
    pub const LSTAT: u64 = 40;
    pub const DUP: u64 = 41;
    pub const PIPE: u64 = 42;
    pub const TIMES: u64 = 43;
    pub const SETGID: u64 = 46;
    pub const GETGID: u64 = 47;
    pub const GETEUID: u64 = 49;
    pub const GETEGID: u64 = 50;
    pub const IOCTL: u64 = 54;
    pub const REBOOT: u64 = 55;
    pub const SYMLINK: u64 = 57;
    pub const READLINK: u64 = 58;
    pub const EXECVE: u64 = 59;
    pub const FSTAT: u64 = 62;
    pub const MSYNC: u64 = 65;
    pub const VFORK: u64 = 66;
    pub const PREAD64: u64 = 67;
    pub const PWRITE64: u64 = 68;
    pub const MMAP: u64 = 71;
    pub const MUNMAP: u64 = 73;
    pub const MPROTECT: u64 = 74;
    pub const MADVISE: u64 = 75;
    pub const GETGROUPS: u64 = 79;
    pub const SETGROUPS: u64 = 80;
    pub const GETPGRP: u64 = 81;
    pub const SETHOSTNAME: u64 = 88;
    pub const DUP2: u64 = 90;
    pub const FCNTL: u64 = 92;
    pub const SELECT: u64 = 93;
    pub const FSYNC: u64 = 95;
    pub const SOCKET: u64 = 97;
    pub const CONNECT: u64 = 98;
    pub const ACCEPT: u64 = 99;
    pub const RT_SIGRETURN: u64 = 101;
    pub const RT_SIGACTION: u64 = 102;
    pub const RT_SIGPROCMASK: u64 = 103;
    pub const RT_SIGPENDING: u64 = 104;
    pub const RT_SIGTIMEDWAIT: u64 = 105;
    pub const RT_SIGQUEUEINFO: u64 = 106;
    pub const RT_SIGSUSPEND: u64 = 107;
    pub const SETRESUID: u64 = 108;
    pub const GETRESUID: u64 = 109;
    pub const SETRESGID: u64 = 110;
    pub const GETRESGID: u64 = 111;
    pub const RECVMSG: u64 = 113;
    pub const SENDMSG: u64 = 114;
    pub const GETTIMEOFDAY: u64 = 116;
    pub const GETRUSAGE: u64 = 117;
    pub const GETSOCKOPT: u64 = 118;
    pub const GETCWD: u64 = 119;
    pub const READV: u64 = 120;
    pub const WRITEV: u64 = 121;
    pub const SETTIMEOFDAY: u64 = 122;
    pub const FCHMOD: u64 = 124;
    pub const RECVFROM: u64 = 125;
    pub const SETREUID: u64 = 126;
    pub const SETREGID: u64 = 127;
    pub const RENAME: u64 = 128;
    pub const FTRUNCATE: u64 = 130;
    pub const FLOCK: u64 = 131;
    pub const SENDTO: u64 = 133;
    pub const SHUTDOWN: u64 = 134;
    pub const SOCKETPAIR: u64 = 135;
    pub const MKDIR: u64 = 136;
    pub const RMDIR: u64 = 137;
    pub const GETPEERNAME: u64 = 141;
    pub const FUTEX: u64 = 142;
    pub const GETTID: u64 = 143;
    pub const GETRLIMIT: u64 = 144;
    pub const SETRLIMIT: u64 = 145;
    pub const PRCTL: u64 = 147;
    pub const GETSOCKNAME: u64 = 150;
    pub const INOTIFY_INIT: u64 = 151;
    pub const INOTIFY_ADD_WATCH: u64 = 152;
    pub const POLL: u64 = 153;
    pub const GETDENTS64: u64 = 154;
    pub const INOTIFY_RM_WATCH: u64 = 156;
    pub const STATFS: u64 = 157;
    pub const FSTATFS: u64 = 158;
    pub const SETDOMAINNAME: u64 = 163;
    pub const SET_TID_ADDRESS: u64 = 166;
    pub const SETSID: u64 = 175;
    pub const FCHDIR: u64 = 176;
    pub const SETPGID: u64 = 185;
    pub const TKILL: u64 = 187;
    pub const EXIT_GROUP: u64 = 188;
    pub const UNAME: u64 = 189;
    pub const EPOLL_CREATE: u64 = 193;
    pub const EPOLL_CTL: u64 = 194;
    pub const EPOLL_WAIT: u64 = 195;
    pub const GETPPID: u64 = 197;
    pub const FADVISE64: u64 = 209;
    pub const TGKILL: u64 = 211;
    pub const SYSINFO: u64 = 214;
    pub const CLONE: u64 = 217;
    pub const GETPGID: u64 = 224;
    pub const SETFSUID: u64 = 228;
    pub const SETFSGID: u64 = 229;
    pub const SCHED_GETPARAM: u64 = 242;
    pub const SCHED_GETSCHEDULER: u64 = 244;
    pub const SCHED_YIELD: u64 = 245;
    pub const NANOSLEEP: u64 = 249;
    pub const MREMAP: u64 = 250;
    pub const GETSID: u64 = 252;
    pub const FDATASYNC: u64 = 253;
    pub const CLOCK_SETTIME: u64 = 256;
    pub const CLOCK_GETTIME: u64 = 257;
    pub const CLOCK_GETRES: u64 = 258;
    pub const CLOCK_NANOSLEEP: u64 = 259;
    pub const SCHED_GETAFFINITY: u64 = 260;
    pub const SCHED_SETAFFINITY: u64 = 261;
    pub const WAITID: u64 = 279;
    pub const OPENAT: u64 = 284;
    pub const MKDIRAT: u64 = 285;
    pub const NEWFSTATAT: u64 = 289;
    pub const UNLINKAT: u64 = 290;
    pub const RENAMEAT: u64 = 291;
    pub const LINKAT: u64 = 292;
    pub const SYMLINKAT: u64 = 293;
    pub const READLINKAT: u64 = 294;
    pub const FCHMODAT: u64 = 295;
    pub const FACCESSAT: u64 = 296;
    pub const PSELECT6: u64 = 297;
    pub const PPOLL: u64 = 298;
    pub const SET_ROBUST_LIST: u64 = 300;
    pub const GET_ROBUST_LIST: u64 = 301;
    pub const GETCPU: u64 = 308;
    pub const EPOLL_PWAIT: u64 = 309;
    pub const UTIMENSAT: u64 = 310;
    pub const SIGNALFD: u64 = 311;
    pub const TIMERFD_CREATE: u64 = 312;
    pub const EVENTFD: u64 = 313;
    pub const FALLOCATE: u64 = 314;
    pub const TIMERFD_SETTIME: u64 = 315;
    pub const TIMERFD_GETTIME: u64 = 316;
    pub const SIGNALFD4: u64 = 317;
    pub const EVENTFD2: u64 = 318;
    pub const EPOLL_CREATE1: u64 = 319;
    pub const DUP3: u64 = 320;
    pub const PIPE2: u64 = 321;
    pub const INOTIFY_INIT1: u64 = 322;
    pub const ACCEPT4: u64 = 323;
    pub const RT_TGSIGQUEUEINFO: u64 = 326;
    pub const PRLIMIT64: u64 = 331;
    pub const SYNCFS: u64 = 335;
    pub const RENAMEAT2: u64 = 345;
    pub const GETRANDOM: u64 = 347;
    pub const MEMFD_CREATE: u64 = 348;
    pub const EXECVEAT: u64 = 350;
    pub const BIND: u64 = 353;
    pub const LISTEN: u64 = 354;
    pub const SETSOCKOPT: u64 = 355;
    pub const STATX: u64 = 360;
    pub const CLOSE_RANGE: u64 = 436;
    pub const OPENAT2: u64 = 437;
    pub const FACCESSAT2: u64 = 439;
    pub const EPOLL_PWAIT2: u64 = 441;
    pub const FCHMODAT2: u64 = 452;
    pub const TIME: u64 = NOT_ON_SPARC64;
    /// The highest number above that SPARC64 has.
    pub const UMASK: u64 = 60;
    pub const CHOWN: u64 = 13;
    pub const FCHOWN: u64 = 123;
    pub const LCHOWN: u64 = 16;
    pub const FCHOWNAT: u64 = 291;
    pub const PIDFD_OPEN: u64 = 434;
    pub const PIDFD_SEND_SIGNAL: u64 = 424;
    pub const PIDFD_GETFD: u64 = 438;
    pub const SENDFILE: u64 = 39;
    pub const SPLICE: u64 = 232;
    pub const TEE: u64 = 280;
    pub const VMSPLICE: u64 = 25;
    pub const COPY_FILE_RANGE: u64 = 357;
    pub const SETXATTR: u64 = 169;
    pub const LSETXATTR: u64 = 170;
    pub const FSETXATTR: u64 = 171;
    pub const GETXATTR: u64 = 172;
    pub const LGETXATTR: u64 = 173;
    pub const FGETXATTR: u64 = 177;
    pub const LISTXATTR: u64 = 178;
    pub const LLISTXATTR: u64 = 179;
    pub const FLISTXATTR: u64 = 180;
    pub const REMOVEXATTR: u64 = 181;
    pub const LREMOVEXATTR: u64 = 182;
    pub const FREMOVEXATTR: u64 = 186;
    pub const SENDMMSG: u64 = 336;
    pub const RECVMMSG: u64 = 328;
    pub const CAPGET: u64 = 21;
    pub const CAPSET: u64 = 22;
    pub const PTRACE: u64 = 26;
    pub const FANOTIFY_INIT: u64 = 329;
    pub const FANOTIFY_MARK: u64 = 330;
    pub const LAST: u64 = FCHMODAT2;
    const NOT_ON_SPARC64: u64 = 0x1_0000;
}

/// SPARC's terminal and descriptor `ioctl` numbers (`asm/ioctls.h`): its `_IOC` is PowerPC's
/// (the direction in the top three bits: none 1, read 2, write 4), and its `struct termios` is
/// 36 bytes.
pub mod tty {
    pub const TCGETS: u32 = 0x4024_5408;
    pub const TCSETS: u32 = 0x8024_5409;
    pub const TCSETSW: u32 = 0x8024_540a;
    pub const TCSETSF: u32 = 0x8024_540b;
    pub const TCFLSH: u32 = 0x2000_5407;
    pub const TIOCGWINSZ: u32 = 0x4008_7468;
    pub const TIOCSWINSZ: u32 = 0x8008_7467;
    pub const FIONREAD: u32 = 0x4004_667f;
    pub const TCGETS2: u32 = 0x402c_540c;
    pub const TCSETS2: u32 = 0x802c_540d;
    pub const TCSETSW2: u32 = 0x802c_540e;
    pub const TCSETSF2: u32 = 0x802c_540f;
    pub const FIONBIO: u32 = 0x8004_667e;
    pub const FIONCLEX: u32 = 0x2000_6602;
    pub const FIOCLEX: u32 = 0x2000_6601;
    pub const FIOASYNC: u32 = 0x8004_667d;
    pub const TIOCSCTTY: u32 = 0x2000_7484;
    pub const TIOCGPGRP: u32 = 0x4004_7483;
    pub const TIOCSPGRP: u32 = 0x8004_7482;
    pub const TIOCGSID: u32 = 0x4004_7485;
    pub const TIOCGPTN: u32 = 0x4004_7486;
    pub const TIOCSPTLCK: u32 = 0x8004_7487;
    pub const TIOCGPTLCK: u32 = 0x4004_5439;
    pub const TIOCPKT: u32 = 0x8004_7470;
    pub const TIOCGPKT: u32 = 0x4004_5438;
    pub const TIOCGPTPEER: u32 = 0x2000_7489;
    pub const TIOCSTI: u32 = 0x8001_7472;
}

/// SPARC's signal numbers (`asm/signal.h`, SunOS's): `SIGEMT` where the others have
/// `SIGSTKFLT`, which SPARC lacks and the kernel names by that number.
pub mod signal {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGABRT: u32 = 6;
    pub const SIGSTKFLT: u32 = 7;
    pub const SIGFPE: u32 = 8;
    pub const SIGKILL: u32 = 9;
    pub const SIGBUS: u32 = 10;
    pub const SIGSEGV: u32 = 11;
    pub const SIGSYS: u32 = 12;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGURG: u32 = 16;
    pub const SIGSTOP: u32 = 17;
    pub const SIGTSTP: u32 = 18;
    pub const SIGCONT: u32 = 19;
    pub const SIGCHLD: u32 = 20;
    pub const SIGTTIN: u32 = 21;
    pub const SIGTTOU: u32 = 22;
    pub const SIGIO: u32 = 23;
    pub const SIGXCPU: u32 = 24;
    pub const SIGXFSZ: u32 = 25;
    pub const SIGVTALRM: u32 = 26;
    pub const SIGPROF: u32 = 27;
    pub const SIGWINCH: u32 = 28;
    pub const SIGPWR: u32 = 29;
    pub const SIGUSR1: u32 = 30;
    pub const SIGUSR2: u32 = 31;
}

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "sparc64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::SPARCV9;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// `clone(flags, stack, parent_tid, tls, child_tid)`, as on the generic architectures.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// `struct sigaction` has `sa_restorer`; `rt_sigaction`'s fourth argument is a restorer, which
/// [`arguments_in`] drops: handlers return through the kernel's trampoline, which is the same
/// `rt_sigreturn` glibc's restorer makes.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// The arguments of some calls are rewritten into the generic values ([`arguments_in`]).
pub const REWRITES_ARGUMENTS: bool = true;
/// `MINSIGSTKSZ`.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 4096;
/// A new program's `argc` is 128 bytes above its stack pointer, past the save area of its
/// register window, where `_start` reads it (D418).
pub const START_STACK_GAP: u64 = 128;
/// The page size programs are told (`AT_PAGESZ`): the MMU's 8 KiB, so that they keep their
/// mappings in whole pairs (D328).
pub const REPORTED_PAGE_SIZE: u64 = 8192;

const MOV_RT_SIGRETURN_G1: u32 = 0x8210_2000 | number::RT_SIGRETURN as u32;
const TA_0X6D: u32 = 0x91d0_206d;
const NOP: u32 = 0x0100_0000;
/// The kernel's `rt_sigreturn` trampoline: `mov 101, %g1`, `ta 0x6d`, then a `nop`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([MOV_RT_SIGRETURN_G1, TA_0X6D, NOP]));

/// Linux's `stat` on SPARC64: a 32-bit `st_dev` and `st_rdev`, a 16-bit `st_nlink`, the times
/// in whole seconds.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 104;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::Sparc64;
}

/// The `open` flags whose values differ between architectures, as the kernel sees them after
/// [`arguments_in`]: the generic ones.
pub mod open {
    /// `O_DIRECTORY`.
    pub const DIRECTORY: u32 = 0o200_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o400_000;
}

/// The `mmap` bits only some architectures have: none once [`arguments_in`] rewrote them.
pub mod mmap {
    pub const PROT_ARCH: u64 = 0;
    pub const MAP_32BIT: u64 = 0;

    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

const OPEN_FLAGS: FlagPairs = &[
    (0x0008, 0o2000),
    (0x0200, 0o100),
    (0x0400, 0o1000),
    (0x0800, 0o200),
    (0x2000, 0o10000),
    (0x4000, 0o4000),
    (0x8000, 0o400),
    (0x1_0000, 0o200_000),
    (0x2_0000, 0o400_000),
    (0x4_0000, 0o100_000),
    (0x10_0000, 0o40_000),
    (0x20_0000, 0o1_000_000),
    (0x40_0000, 0o2_000_000),
    (0x80_2000, 0o4_010_000),
    (0x100_0000, 0o10_000_000),
];

const MMAP_FLAGS: FlagPairs = &[(0x40, 0x4000), (0x100, 0x2000), (0x200, 0x100)];

const NONBLOCK_SPARC: u64 = 0x4000;
const NONBLOCK_GENERIC: u64 = 0o4000;
const CLOEXEC_SPARC: u64 = 0x40_0000;
const CLOEXEC_GENERIC: u64 = 0o2_000_000;

const SOL_SOCKET_SPARC: u64 = 0xffff;
const SOL_SOCKET_GENERIC: u64 = 1;
const SOCKET_OPTIONS: FlagPairs = &[
    (0x0001, 1),
    (0x0004, 2),
    (0x1008, 3),
    (0x1007, 4),
    (0x0010, 5),
    (0x0020, 6),
    (0x1001, 7),
    (0x1002, 8),
    (0x0008, 9),
    (0x0100, 10),
    (0x0080, 13),
    (0x0200, 15),
    (0x0002, 16),
    (0x0040, 17),
    (0x0800, 18),
    (0x1000, 19),
    (0x2000, 20),
    (0x4000, 21),
    (0x8000, 30),
    (0x1028, 38),
    (0x1029, 39),
    (0x0044, 66),
    (0x0045, 67),
];

const FCNTL_COMMANDS: FlagPairs = &[(7, 5), (8, 6), (9, 7), (6, 8), (5, 9)];
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;

fn descriptor_flags(value: u64) -> u64 {
    let mut out = value & !(NONBLOCK_SPARC | CLOEXEC_SPARC);
    if value & NONBLOCK_SPARC != 0 {
        out |= NONBLOCK_GENERIC;
    }
    if value & CLOEXEC_SPARC != 0 {
        out |= CLOEXEC_GENERIC;
    }
    out
}

const SIGPROCMASK_HOW: [(u64, u64); 3] = [(1, 0), (2, 1), (4, 2)];

/// Rewrites call `number`'s `arguments` from SPARC's values into the generic ones the kernel
/// takes; whether it changed any. `rt_sigaction`'s restorer (its fourth argument) gives way to
/// the signal-set size (its fifth), and `rt_sigprocmask`'s `how` is SPARC's (`SIG_BLOCK` 1,
/// `SIG_UNBLOCK` 2, `SIG_SETMASK` 4).
pub fn arguments_in(number: u64, arguments: &mut [u64; 6]) -> bool {
    let before = *arguments;
    match number {
        number::OPEN => arguments[1] = flags_to_generic(arguments[1], OPEN_FLAGS),
        number::OPENAT => arguments[2] = flags_to_generic(arguments[2], OPEN_FLAGS),
        number::MMAP => arguments[3] = flags_to_generic(arguments[3], MMAP_FLAGS),
        number::FCNTL => {
            arguments[1] = value_to_generic(arguments[1], FCNTL_COMMANDS);
            if arguments[1] == F_SETFL {
                arguments[2] = flags_to_generic(arguments[2], OPEN_FLAGS);
            }
        }
        number::PIPE2 | number::INOTIFY_INIT1 | number::EPOLL_CREATE1 => {
            let at = if number == number::PIPE2 { 1 } else { 0 };
            arguments[at] = descriptor_flags(arguments[at]);
        }
        number::DUP3 => arguments[2] = descriptor_flags(arguments[2]),
        number::EVENTFD2 | number::TIMERFD_CREATE => arguments[1] = descriptor_flags(arguments[1]),
        number::SIGNALFD4 => arguments[3] = descriptor_flags(arguments[3]),
        number::SOCKET | number::SOCKETPAIR => arguments[1] = descriptor_flags(arguments[1]),
        number::ACCEPT4 => arguments[3] = descriptor_flags(arguments[3]),
        number::SETSOCKOPT | number::GETSOCKOPT if arguments[1] == SOL_SOCKET_SPARC => {
            arguments[1] = SOL_SOCKET_GENERIC;
            arguments[2] = value_to_generic(arguments[2], SOCKET_OPTIONS);
        }
        number::RT_SIGACTION => arguments[3] = arguments[4],
        number::RT_SIGPROCMASK => {
            arguments[0] = SIGPROCMASK_HOW
                .iter()
                .find(|&&(sparc, _)| sparc == arguments[0] & 0xffff_ffff)
                .map_or(u64::MAX, |&(_, generic)| generic);
        }
        _ => {}
    }
    *arguments != before
}

/// Gives call `number`'s result SPARC's values: `F_GETFL`'s flags.
pub fn result_out(number: u64, arguments: [u64; 6], result: u64) -> u64 {
    match number {
        number::FCNTL if arguments[1] == F_GETFL && (result as i64) >= 0 => {
            flags_to_architecture(result, OPEN_FLAGS)
        }
        _ => result,
    }
}

const SIGACTION_FLAGS: FlagPairs = &[
    (0x1, 0x0800_0000),
    (0x2, 0x1000_0000),
    (0x4, 0x8000_0000),
    (0x8, 0x1),
    (0x20, 0x4000_0000),
    (0x100, 0x2),
    (0x200, 0x4),
];

/// `sa_flags` as the kernel takes them.
pub fn sigaction_flags_in(flags: u64) -> u64 {
    flags_to_generic(flags, SIGACTION_FLAGS)
}

/// `sa_flags` as a program reads them back.
pub fn sigaction_flags_out(flags: u64) -> u64 {
    flags_to_architecture(flags, SIGACTION_FLAGS)
}

const POLL_WRITE_BAND_SPARC: u16 = 0x100;
const POLL_WRITE_NORMAL_GENERIC: u16 = 0x100;
const POLL_WRITE_BAND_GENERIC: u16 = 0x200;
const POLL_OUT: u16 = 0x4;

/// Requested `poll` events in the generic values.
pub fn poll_events_in(events: u16) -> u16 {
    let band = if events & POLL_WRITE_BAND_SPARC != 0 {
        POLL_WRITE_BAND_GENERIC
    } else {
        0
    };
    (events & !POLL_WRITE_BAND_SPARC) | band
}

/// Returned `poll` events in SPARC's values.
pub fn poll_events_out(events: u16) -> u16 {
    let normal = if events & POLL_WRITE_NORMAL_GENERIC != 0 {
        POLL_OUT
    } else {
        0
    };
    let band = if events & POLL_WRITE_BAND_GENERIC != 0 {
        POLL_WRITE_BAND_SPARC
    } else {
        0
    };
    (events & !(POLL_WRITE_NORMAL_GENERIC | POLL_WRITE_BAND_GENERIC)) | normal | band
}

const SO_ERROR: u32 = 4;

/// A `SOL_SOCKET` option's `int` in SPARC's values: `SO_ERROR`'s error number.
pub fn socket_option_out(name: u32, value: u32) -> u32 {
    match name {
        SO_ERROR => eggos_hal::user::sparc_errno(u64::from(value)) as u32,
        _ => value,
    }
}

/// `siginfo_t` has the generic order on SPARC.
#[inline(always)]
pub fn reorder_siginfo(_info: &mut [u8]) {}

/// SPARC's `struct termios`: 17 control characters, `VMIN` and `VTIME` sharing `VEOF`'s and
/// `VEOL`'s places (which one holds depends on `ICANON`), and `FLUSHO` at its own bit.
pub mod termios {
    use super::{FlagPairs, flags_to_architecture, flags_to_generic};
    use crate::console::line_discipline::{Termios, index, local};

    const CONTROL_CHARACTERS_SPARC: usize = 17;
    /// Its size, padded to its 4-byte alignment.
    pub const SIZE: usize = 36;
    const SHARED_EOF_MIN: usize = 4;
    const SHARED_EOL_TIME: usize = 5;
    const INDEXES: [(usize, usize); 13] = [
        (0, 0),
        (1, 1),
        (2, 2),
        (3, 3),
        (6, 16),
        (7, 7),
        (8, 8),
        (9, 9),
        (10, 10),
        (12, 12),
        (13, 13),
        (14, 14),
        (15, 15),
    ];
    const LOCAL_FLAGS: FlagPairs = &[(0x2000, 0x1000)];
    const _: () = assert!(17 + CONTROL_CHARACTERS_SPARC <= SIZE);

    /// `settings` as a SPARC program reads them.
    pub fn to_program(settings: &Termios) -> [u8; SIZE] {
        let mut bytes = [0; SIZE];
        let local_flags = flags_to_architecture(u64::from(settings.local), LOCAL_FLAGS) as u32;
        for (at, word) in [
            settings.input,
            settings.output,
            settings.control,
            local_flags,
        ]
        .into_iter()
        .enumerate()
        {
            bytes[at * 4..][..4].copy_from_slice(&word.to_ne_bytes());
        }
        bytes[16] = settings.line;
        let characters = &settings.control_characters;
        for (sparc, generic) in INDEXES {
            bytes[17 + sparc] = characters[generic];
        }
        let canonical = settings.local & local::ICANON != 0;
        let (first, second) = match canonical {
            true => (characters[index::VEOF], characters[index::VEOL]),
            false => (characters[index::VMIN], characters[index::VTIME]),
        };
        bytes[17 + SHARED_EOF_MIN] = first;
        bytes[17 + SHARED_EOL_TIME] = second;
        bytes
    }

    /// The settings a SPARC program wrote; the pair its `ICANON` does not use keeps a serial
    /// console's values.
    pub fn from_program(bytes: &[u8; SIZE]) -> Termios {
        let word = |at: usize| u32::from_ne_bytes(bytes[at * 4..][..4].try_into().unwrap());
        let mut settings = Termios {
            input: word(0),
            output: word(1),
            control: word(2),
            local: flags_to_generic(u64::from(word(3)), LOCAL_FLAGS) as u32,
            line: bytes[16],
            ..Termios::SERIAL_CONSOLE
        };
        for (sparc, generic) in INDEXES {
            settings.control_characters[generic] = bytes[17 + sparc];
        }
        let (first, second) = (bytes[17 + SHARED_EOF_MIN], bytes[17 + SHARED_EOL_TIME]);
        match settings.local & local::ICANON != 0 {
            true => {
                settings.control_characters[index::VEOF] = first;
                settings.control_characters[index::VEOL] = second;
            }
            false => {
                settings.control_characters[index::VMIN] = first;
                settings.control_characters[index::VTIME] = second;
            }
        }
        settings
    }
}

/// `struct rt_signal_frame { struct sparc_stackf ss; siginfo_t info; struct pt_regs regs;
/// __siginfo_fpu_t *fpu_save; stack_t stack; sigset_t mask; __siginfo_rwin_t *rwin_save; }`,
/// then the `__siginfo_fpu_t` it points at (`%f0`-`%f63`, `%fsr`, `%gsr`, `%fprs`). `ss` is the
/// handler's window save area, `regs` holds `%g0`-`%g7` and `%o0`-`%o7`, `TSTATE`, `%pc`,
/// `%npc`, `%y` and a magic word. The interrupted window's locals and ins go to its own stack,
/// where a spill would put them, and come back from there on `rt_sigreturn`.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, word};
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    const STACK_BIAS: u64 = 2047;
    const WINDOW_BYTES: usize = 128;
    pub const INFO: usize = 192;
    const REGS: usize = INFO + INFO_SIZE;
    const U_REGS: usize = REGS;
    const TSTATE: usize = REGS + 128;
    const TPC: usize = REGS + 136;
    const TNPC: usize = REGS + 144;
    const Y: usize = REGS + 152;
    const MAGIC: usize = REGS + 156;
    const FPU_SAVE: usize = REGS + 160;
    const STACK: usize = FPU_SAVE + 8;
    pub const SIGMASK: usize = STACK + STACK_SIZE;
    const RWIN_SAVE: usize = SIGMASK + 8;
    const FPU: usize = (RWIN_SAVE + 8 + 15) & !15;
    const FPU_BYTES: usize = 256 + 24;
    /// The whole frame.
    pub const WRITTEN: usize = (FPU + FPU_BYTES + 15) & !15;
    pub const READ: usize = WRITTEN;
    const PT_REGS_MAGIC: u32 = 0x57ac_6c00;

    /// `%d0`-`%d62`, `%fsr`, `%gsr`, `%fprs`.
    pub type FpPart = [u8; FPU_BYTES];

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;
    /// No red zone.
    pub const RED_ZONE: u64 = 0;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; FPU_BYTES];
        for (index, double) in state.d().iter().enumerate() {
            part[index * 8..][..8].copy_from_slice(&double.to_ne_bytes());
        }
        part[256..264].copy_from_slice(&state.fsr().to_ne_bytes());
        part[264..272].copy_from_slice(&state.gsr().to_ne_bytes());
        part[272..280].copy_from_slice(&4u64.to_ne_bytes());
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for (index, double) in state.d_mut().iter_mut().enumerate() {
            *double = word(part, index * 8);
        }
        state.set_fsr(word(part, 256));
        state.set_gsr(word(part, 264));
        Ok(())
    }

    /// 16-byte aligned below `top`, the stack's real address.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(WRITTEN as u64) & !15
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
        for (index, value) in registers.r[16..32].iter().enumerate() {
            put(&mut frame, index * 8, *value);
        }
        frame[INFO..][..INFO_SIZE].copy_from_slice(info);
        for (index, value) in registers.r[..16].iter().enumerate() {
            put(&mut frame, U_REGS + index * 8, *value);
        }
        put(&mut frame, TSTATE, registers.tstate);
        put(&mut frame, TPC, registers.pc);
        put(&mut frame, TNPC, registers.npc);
        frame[Y..][..4].copy_from_slice(&(registers.y as u32).to_ne_bytes());
        let trap_type = trap.map_or(0, |trap| trap.number as u32 & 0x1ff);
        frame[MAGIC..][..4].copy_from_slice(&(PT_REGS_MAGIC | trap_type).to_ne_bytes());
        put(&mut frame, FPU_SAVE, frame_address + FPU as u64);
        frame[STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, SIGMASK, saved.blocked);
        frame[FPU..][..FPU_BYTES].copy_from_slice(&saved.fp);
        let _ = restorer;
        (frame_address, frame)
    }

    /// The interrupted window's locals and ins, to its own stack's save area, as a spill puts
    /// them.
    pub fn write_extra<P: Platform>(
        process: &Process<P>,
        _top: u64,
        _frame: u64,
        saved: &Saved,
    ) -> Result<(), BadFrame> {
        let mut window = [0u8; WINDOW_BYTES];
        for (index, value) in saved.registers.r[16..32].iter().enumerate() {
            put(&mut window, index * 8, *value);
        }
        let save_area = saved.registers.r[14].wrapping_add(STACK_BIAS);
        process
            .write_user_memory(save_area, &window)
            .map_err(|_| BadFrame)
    }

    /// The restored window's locals and ins, from its stack's save area.
    pub fn read_extra<P: Platform>(
        process: &Process<P>,
        _frame: &[u8; READ],
        saved: &mut Saved,
    ) -> Result<(), BadFrame> {
        let mut window = [0u8; WINDOW_BYTES];
        let save_area = saved.registers.r[14].wrapping_add(STACK_BIAS);
        process
            .read_user_memory(save_area, &mut window)
            .map_err(|_| BadFrame)?;
        for (index, value) in saved.registers.r[16..32].iter_mut().enumerate() {
            *value = word(&window, index * 8);
        }
        Ok(())
    }

    /// `%o0` the signal, `%o1` and `%o2` the `siginfo_t` (glibc reads its `sigcontext` there),
    /// `%sp` the frame less the bias, `%o7` the restorer less 8 (the handler returns to
    /// `%i7 + 8`), `%pc` the handler.
    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.r[8] = u64::from(handler.signal);
        registers.r[9] = frame + INFO as u64;
        registers.r[10] = frame + INFO as u64;
        registers.r[14] = frame.wrapping_sub(STACK_BIAS);
        registers.r[15] = handler.restorer.wrapping_sub(8);
        registers.pc = handler.entry;
        registers.npc = handler.entry + 4;
    }

    /// The handler returned with its stack pointer at the frame.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.r[14].wrapping_add(STACK_BIAS))
    }

    pub fn parse(frame: &[u8; READ], current: &UserRegisters) -> Option<Saved> {
        let mut r = current.r;
        for (index, value) in r[..16].iter_mut().enumerate() {
            *value = word(frame, U_REGS + index * 8);
        }
        r[0] = 0;
        let mut fp = [0; FPU_BYTES];
        fp.copy_from_slice(&frame[FPU..][..FPU_BYTES]);
        Some(Saved {
            registers: UserRegisters {
                r,
                pc: word(frame, TPC),
                npc: word(frame, TNPC),
                tstate: word(frame, TSTATE),
                y: u64::from(u32::from_ne_bytes(frame[Y..][..4].try_into().unwrap())),
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[STACK..][..STACK_SIZE].try_into().unwrap(),
            fp,
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.r[8]
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    const FPE_INTDIV: i32 = 1;

    /// The signal, `si_code` and `si_addr` Linux gives `exception`; the trap number is the trap
    /// type.
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
            ExceptionKind::Breakpoint | ExceptionKind::Debug => {
                (signal::SIGTRAP, code::TRAP_BRKPT, pc)
            }
            ExceptionKind::Arithmetic => (signal::SIGFPE, FPE_INTDIV, pc),
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        let trap = Trap {
            number: exception.code,
            cause: 0,
            address,
        };
        fault(reported, trap)
    }
}
