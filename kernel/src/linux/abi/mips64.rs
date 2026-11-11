//! MIPS64's Linux ABI, n64 (D218, D315): its own table (`asm/unistd_n64.h`, numbers from 5000)
//! and, unlike every other port's, its own values for the signals, `struct sigaction` (flags
//! first, no restorer, 128 signals in 16 bytes), `stack_t` (size before flags), `struct stat`,
//! the terminal `ioctl` numbers, and the flags of `open`, `mmap`, `fcntl` and sockets. The
//! kernel works in the generic values: [`arguments_in`] rewrites a call's arguments into them
//! before it is dispatched and [`result_out`] the few results that carry flags back; the HAL
//! gives an error its MIPS number (`a3` set, the number positive in `v0`).

/// The calls n64 keeps beyond the generic table: the ones older than the `*at` family,
/// `renameat`, and `set_thread_area`, which sets the thread pointer (`UserLocal`).
pub mod wants {
    pub const LEGACY_CALLS: bool = true;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
    pub const SET_THREAD_AREA: bool = true;
}

/// Linux's system-call numbers on MIPS64 n64. `time` and `vfork` are not in it: their numbers
/// here are past [`number::LAST`], which answers `ENOSYS` before any is looked up.
pub mod number {
    pub const READ: u64 = 5000;
    pub const WRITE: u64 = 5001;
    pub const OPEN: u64 = 5002;
    pub const CLOSE: u64 = 5003;
    pub const STAT: u64 = 5004;
    pub const FSTAT: u64 = 5005;
    pub const LSTAT: u64 = 5006;
    pub const POLL: u64 = 5007;
    pub const LSEEK: u64 = 5008;
    pub const MMAP: u64 = 5009;
    pub const MPROTECT: u64 = 5010;
    pub const MUNMAP: u64 = 5011;
    pub const BRK: u64 = 5012;
    pub const RT_SIGACTION: u64 = 5013;
    pub const RT_SIGPROCMASK: u64 = 5014;
    pub const IOCTL: u64 = 5015;
    pub const PREAD64: u64 = 5016;
    pub const PWRITE64: u64 = 5017;
    pub const READV: u64 = 5018;
    pub const WRITEV: u64 = 5019;
    pub const ACCESS: u64 = 5020;
    pub const PIPE: u64 = 5021;
    pub const SELECT: u64 = 5022;
    pub const SCHED_YIELD: u64 = 5023;
    pub const MREMAP: u64 = 5024;
    pub const MSYNC: u64 = 5025;
    pub const MADVISE: u64 = 5027;
    pub const DUP: u64 = 5031;
    pub const DUP2: u64 = 5032;
    pub const PAUSE: u64 = 5033;
    pub const NANOSLEEP: u64 = 5034;
    pub const GETPID: u64 = 5038;
    pub const SOCKET: u64 = 5040;
    pub const CONNECT: u64 = 5041;
    pub const ACCEPT: u64 = 5042;
    pub const SENDTO: u64 = 5043;
    pub const RECVFROM: u64 = 5044;
    pub const SENDMSG: u64 = 5045;
    pub const RECVMSG: u64 = 5046;
    pub const SHUTDOWN: u64 = 5047;
    pub const BIND: u64 = 5048;
    pub const LISTEN: u64 = 5049;
    pub const GETSOCKNAME: u64 = 5050;
    pub const GETPEERNAME: u64 = 5051;
    pub const SOCKETPAIR: u64 = 5052;
    pub const SETSOCKOPT: u64 = 5053;
    pub const GETSOCKOPT: u64 = 5054;
    pub const CLONE: u64 = 5055;
    pub const FORK: u64 = 5056;
    pub const EXECVE: u64 = 5057;
    pub const EXIT: u64 = 5058;
    pub const WAIT4: u64 = 5059;
    pub const KILL: u64 = 5060;
    pub const UNAME: u64 = 5061;
    pub const FCNTL: u64 = 5070;
    pub const FLOCK: u64 = 5071;
    pub const FSYNC: u64 = 5072;
    pub const FDATASYNC: u64 = 5073;
    pub const FTRUNCATE: u64 = 5075;
    pub const GETCWD: u64 = 5077;
    pub const CHDIR: u64 = 5078;
    pub const FCHDIR: u64 = 5079;
    pub const RENAME: u64 = 5080;
    pub const MKDIR: u64 = 5081;
    pub const RMDIR: u64 = 5082;
    pub const LINK: u64 = 5084;
    pub const UNLINK: u64 = 5085;
    pub const SYMLINK: u64 = 5086;
    pub const READLINK: u64 = 5087;
    pub const CHMOD: u64 = 5088;
    pub const FCHMOD: u64 = 5089;
    pub const GETTIMEOFDAY: u64 = 5094;
    pub const GETRLIMIT: u64 = 5095;
    pub const GETRUSAGE: u64 = 5096;
    pub const SYSINFO: u64 = 5097;
    pub const TIMES: u64 = 5098;
    pub const GETUID: u64 = 5100;
    pub const GETGID: u64 = 5102;
    pub const SETUID: u64 = 5103;
    pub const SETGID: u64 = 5104;
    pub const GETEUID: u64 = 5105;
    pub const GETEGID: u64 = 5106;
    pub const SETPGID: u64 = 5107;
    pub const GETPPID: u64 = 5108;
    pub const GETPGRP: u64 = 5109;
    pub const SETSID: u64 = 5110;
    pub const SETREUID: u64 = 5111;
    pub const SETREGID: u64 = 5112;
    pub const GETGROUPS: u64 = 5113;
    pub const SETGROUPS: u64 = 5114;
    pub const SETRESUID: u64 = 5115;
    pub const GETRESUID: u64 = 5116;
    pub const SETRESGID: u64 = 5117;
    pub const GETRESGID: u64 = 5118;
    pub const GETPGID: u64 = 5119;
    pub const SETFSUID: u64 = 5120;
    pub const SETFSGID: u64 = 5121;
    pub const GETSID: u64 = 5122;
    pub const RT_SIGPENDING: u64 = 5125;
    pub const RT_SIGTIMEDWAIT: u64 = 5126;
    pub const RT_SIGQUEUEINFO: u64 = 5127;
    pub const RT_SIGSUSPEND: u64 = 5128;
    pub const SIGALTSTACK: u64 = 5129;
    pub const STATFS: u64 = 5134;
    pub const FSTATFS: u64 = 5135;
    pub const SCHED_GETPARAM: u64 = 5140;
    pub const SCHED_GETSCHEDULER: u64 = 5142;
    pub const PRCTL: u64 = 5153;
    pub const SETRLIMIT: u64 = 5155;
    pub const SYNC: u64 = 5157;
    pub const SETTIMEOFDAY: u64 = 5159;
    pub const REBOOT: u64 = 5164;
    pub const SETHOSTNAME: u64 = 5165;
    pub const SETDOMAINNAME: u64 = 5166;
    pub const GETTID: u64 = 5178;
    pub const TKILL: u64 = 5192;
    pub const FUTEX: u64 = 5194;
    pub const SCHED_SETAFFINITY: u64 = 5195;
    pub const SCHED_GETAFFINITY: u64 = 5196;
    pub const EXIT_GROUP: u64 = 5205;
    pub const EPOLL_CREATE: u64 = 5207;
    pub const EPOLL_CTL: u64 = 5208;
    pub const EPOLL_WAIT: u64 = 5209;
    pub const RT_SIGRETURN: u64 = 5211;
    pub const SET_TID_ADDRESS: u64 = 5212;
    pub const FADVISE64: u64 = 5215;
    pub const CLOCK_SETTIME: u64 = 5221;
    pub const CLOCK_GETTIME: u64 = 5222;
    pub const CLOCK_GETRES: u64 = 5223;
    pub const CLOCK_NANOSLEEP: u64 = 5224;
    pub const TGKILL: u64 = 5225;
    pub const WAITID: u64 = 5237;
    pub const SET_THREAD_AREA: u64 = 5242;
    pub const INOTIFY_INIT: u64 = 5243;
    pub const INOTIFY_ADD_WATCH: u64 = 5244;
    pub const INOTIFY_RM_WATCH: u64 = 5245;
    pub const OPENAT: u64 = 5247;
    pub const MKDIRAT: u64 = 5248;
    pub const NEWFSTATAT: u64 = 5252;
    pub const UNLINKAT: u64 = 5253;
    pub const RENAMEAT: u64 = 5254;
    pub const LINKAT: u64 = 5255;
    pub const SYMLINKAT: u64 = 5256;
    pub const READLINKAT: u64 = 5257;
    pub const FCHMODAT: u64 = 5258;
    pub const FACCESSAT: u64 = 5259;
    pub const PSELECT6: u64 = 5260;
    pub const PPOLL: u64 = 5261;
    pub const SET_ROBUST_LIST: u64 = 5268;
    pub const GET_ROBUST_LIST: u64 = 5269;
    pub const GETCPU: u64 = 5271;
    pub const EPOLL_PWAIT: u64 = 5272;
    pub const UTIMENSAT: u64 = 5275;
    pub const SIGNALFD: u64 = 5276;
    pub const EVENTFD: u64 = 5278;
    pub const FALLOCATE: u64 = 5279;
    pub const TIMERFD_CREATE: u64 = 5280;
    pub const TIMERFD_GETTIME: u64 = 5281;
    pub const TIMERFD_SETTIME: u64 = 5282;
    pub const SIGNALFD4: u64 = 5283;
    pub const EVENTFD2: u64 = 5284;
    pub const EPOLL_CREATE1: u64 = 5285;
    pub const DUP3: u64 = 5286;
    pub const PIPE2: u64 = 5287;
    pub const INOTIFY_INIT1: u64 = 5288;
    pub const RT_TGSIGQUEUEINFO: u64 = 5291;
    pub const ACCEPT4: u64 = 5293;
    pub const PRLIMIT64: u64 = 5297;
    pub const SYNCFS: u64 = 5301;
    pub const GETDENTS64: u64 = 5308;
    pub const RENAMEAT2: u64 = 5311;
    pub const GETRANDOM: u64 = 5313;
    pub const MEMFD_CREATE: u64 = 5314;
    pub const EXECVEAT: u64 = 5316;
    pub const STATX: u64 = 5326;
    pub const CLOSE_RANGE: u64 = 5436;
    pub const OPENAT2: u64 = 5437;
    pub const FACCESSAT2: u64 = 5439;
    pub const EPOLL_PWAIT2: u64 = 5441;
    pub const FCHMODAT2: u64 = 5452;
    /// The highest number above.
    pub const LAST: u64 = FCHMODAT2;
    /// Not in n64: past [`LAST`].
    pub const TIME: u64 = LAST + 1;
    pub const VFORK: u64 = LAST + 2;
}

/// MIPS's terminal and descriptor `ioctl` numbers (`asm/ioctls.h`): its `_IOC` puts the
/// direction in the top three bits (none 1, read 2, write 4), and its `termios` requests have
/// numbers of their own.
pub mod tty {
    pub const TCGETS: u32 = 0x540d;
    pub const TCSETS: u32 = 0x540e;
    pub const TCSETSW: u32 = 0x540f;
    pub const TCSETSF: u32 = 0x5410;
    pub const TCFLSH: u32 = 0x5407;
    pub const TIOCGWINSZ: u32 = 0x4008_7468;
    pub const TIOCSWINSZ: u32 = 0x8008_7467;
    pub const FIONREAD: u32 = 0x467f;
    pub const TCGETS2: u32 = 0x4030_542a;
    pub const TCSETS2: u32 = 0x8030_542b;
    pub const TCSETSW2: u32 = 0x8030_542c;
    pub const TCSETSF2: u32 = 0x8030_542d;
    pub const FIONBIO: u32 = 0x667e;
    pub const FIONCLEX: u32 = 0x6602;
    pub const FIOCLEX: u32 = 0x6601;
    pub const FIOASYNC: u32 = 0x667d;
    pub const TIOCSCTTY: u32 = 0x5480;
    pub const TIOCGPGRP: u32 = 0x4004_7477;
    pub const TIOCSPGRP: u32 = 0x8004_7476;
    pub const TIOCGSID: u32 = 0x7416;
    pub const TIOCGPTN: u32 = 0x4004_5430;
    pub const TIOCSPTLCK: u32 = 0x8004_5431;
    pub const TIOCGPTLCK: u32 = 0x4004_5439;
    pub const TIOCPKT: u32 = 0x5470;
    pub const TIOCGPKT: u32 = 0x4004_5438;
    pub const TIOCGPTPEER: u32 = 0x2000_5441;
    pub const TIOCSTI: u32 = 0x5472;
}

/// MIPS's signal numbers (`asm/signal.h`): `SIGEMT` where the others have `SIGSTKFLT`, which
/// MIPS lacks and the kernel names by that number.
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
    pub const SIGUSR1: u32 = 16;
    pub const SIGUSR2: u32 = 17;
    pub const SIGCHLD: u32 = 18;
    pub const SIGPWR: u32 = 19;
    pub const SIGWINCH: u32 = 20;
    pub const SIGURG: u32 = 21;
    pub const SIGIO: u32 = 22;
    pub const SIGSTOP: u32 = 23;
    pub const SIGTSTP: u32 = 24;
    pub const SIGCONT: u32 = 25;
    pub const SIGTTIN: u32 = 26;
    pub const SIGTTOU: u32 = 27;
    pub const SIGVTALRM: u32 = 28;
    pub const SIGPROF: u32 = 29;
    pub const SIGXCPU: u32 = 30;
    pub const SIGXFSZ: u32 = 31;
}

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "mips64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::MIPS;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// `clone(flags, stack, parent_tid, tls, child_tid)`, as on the generic architectures.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// `struct sigaction` has no `sa_restorer`: handlers return through the kernel's trampoline.
pub const SIGACTION_HAS_RESTORER: bool = false;
/// `struct sigaction` starts with `sa_flags` (an `int`, padded), then the handler and the mask.
pub const SIGACTION_FLAGS_FIRST: bool = true;
/// `stack_t` is `ss_sp`, `ss_size`, then `ss_flags`.
pub const STACK_T_SIZE_BEFORE_FLAGS: bool = true;
/// `sigset_t` holds 128 signals: 16 bytes, of which the kernel's 64 are the first 8.
pub const SIGSET_SIZE: u64 = 16;
/// The arguments of some calls are rewritten into the generic values ([`arguments_in`]).
pub const REWRITES_ARGUMENTS: bool = true;
/// `MINSIGSTKSZ`.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 2048;

const LI_V0_RT_SIGRETURN: u32 = 0x2402_0000 | number::RT_SIGRETURN as u32;
const SYSCALL: u32 = 0x0000_000c;
const NOP: u32 = 0;
/// The kernel's `rt_sigreturn` trampoline: `li $v0, 5211`, `syscall`, then a `nop`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([LI_V0_RT_SIGRETURN, SYSCALL, NOP]));

/// Linux's `stat` on n64: a 32-bit device and rdev, 32-bit times.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 104;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::Mips64;
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

/// The pairs (MIPS, generic) of one kind of flag word; bits in neither pass unchanged.
type FlagPairs = &'static [(u64, u64)];

const OPEN_FLAGS: FlagPairs = &[
    (0x0008, 0o2000),
    (0x0010, 0o10000),
    (0x0080, 0o4000),
    (0x0100, 0o100),
    (0x0200, 0o1000),
    (0x0400, 0o200),
    (0x0800, 0o400),
    (0x1000, 0o20000),
    (0x2000, 0o100000),
    (0x4000, 0o4010000),
    (0x8000, 0o40000),
];

const MMAP_FLAGS: FlagPairs = &[
    (0x0400, 0x4000),
    (0x0800, 0x20),
    (0x1000, 0x100),
    (0x2000, 0x800),
    (0x4000, 0x1000),
    (0x8000, 0x2000),
    (0x1_0000, 0x8000),
    (0x2_0000, 0x1_0000),
    (0x4_0000, 0x2_0000),
    (0x8_0000, 0x4_0000),
];

const SOCKET_TYPES: [(u64, u64); 2] = [(2, 1), (1, 2)];
const SOCKET_TYPE_MASK: u64 = 0xf;
const NONBLOCK_MIPS: u64 = 0x80;
const NONBLOCK_GENERIC: u64 = 0o4000;

const SOL_SOCKET_MIPS: u64 = 0xffff;
const SOL_SOCKET_GENERIC: u64 = 1;
const SOCKET_OPTIONS: FlagPairs = &[
    (0x0004, 2),
    (0x0010, 5),
    (0x0200, 15),
    (0x0008, 9),
    (0x0020, 6),
    (0x0080, 13),
    (0x0100, 10),
    (0x1001, 7),
    (0x1002, 8),
    (0x1005, 21),
    (0x1006, 20),
    (0x1007, 4),
    (0x1008, 3),
    (0x1009, 30),
    (0x1028, 38),
    (0x1029, 39),
    (17, 16),
    (18, 17),
];

const FCNTL_COMMANDS: FlagPairs = &[(14, 5), (24, 8), (23, 9)];
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;

fn flags_to_generic(value: u64, pairs: FlagPairs) -> u64 {
    let known = pairs.iter().fold(0, |known, &(mips, _)| known | mips);
    pairs
        .iter()
        .filter(|&&(mips, _)| value & mips == mips)
        .fold(value & !known, |out, &(_, generic)| out | generic)
}

fn flags_to_mips(value: u64, pairs: FlagPairs) -> u64 {
    let known = pairs.iter().fold(0, |known, &(_, generic)| known | generic);
    pairs
        .iter()
        .filter(|&&(_, generic)| value & generic == generic)
        .fold(value & !known, |out, &(mips, _)| out | mips)
}

fn value_to_generic(value: u64, pairs: FlagPairs) -> u64 {
    pairs.iter().find(|&&(mips, _)| mips == value).map_or(value, |&(_, generic)| generic)
}

fn nonblock(value: u64) -> u64 {
    match value & NONBLOCK_MIPS {
        0 => value,
        _ => value & !NONBLOCK_MIPS | NONBLOCK_GENERIC,
    }
}

fn socket_type(value: u64) -> u64 {
    let base = value & SOCKET_TYPE_MASK;
    let mapped = SOCKET_TYPES.iter().find(|&&(mips, _)| mips == base).map_or(base, |&(_, generic)| generic);
    nonblock(value & !SOCKET_TYPE_MASK) | mapped
}

/// Rewrites call `number`'s `arguments` from MIPS's values into the generic ones the kernel
/// takes: `open` flags, `mmap` flags, `fcntl`'s commands and `F_SETFL`'s flags, the
/// `O_NONBLOCK` of the calls that take one, socket types and `SOL_SOCKET` options. Whether it
/// changed any.
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
        number::PIPE2 | number::DUP3 | number::INOTIFY_INIT1 => {
            let at = if number == number::DUP3 { 2 } else { 1 };
            let at = if number == number::INOTIFY_INIT1 { 0 } else { at };
            arguments[at] = nonblock(arguments[at]);
        }
        number::EVENTFD2 | number::TIMERFD_CREATE => arguments[1] = nonblock(arguments[1]),
        number::SIGNALFD4 => arguments[3] = nonblock(arguments[3]),
        number::SOCKET | number::SOCKETPAIR => arguments[1] = socket_type(arguments[1]),
        number::ACCEPT4 => arguments[3] = nonblock(arguments[3]),
        number::SETSOCKOPT | number::GETSOCKOPT if arguments[1] == SOL_SOCKET_MIPS => {
            arguments[1] = SOL_SOCKET_GENERIC;
            arguments[2] = value_to_generic(arguments[2], SOCKET_OPTIONS);
        }
        _ => {}
    }
    *arguments != before
}

/// Gives call `number`'s result, after the kernel's generic values, MIPS's: `F_GETFL`'s flags.
pub fn result_out(number: u64, arguments: [u64; 6], result: u64) -> u64 {
    match number {
        number::FCNTL if arguments[1] == F_GETFL && (result as i64) >= 0 => flags_to_mips(result, OPEN_FLAGS),
        _ => result,
    }
}

/// The `sa_flags` bits whose values differ: `SA_SIGINFO` and `SA_NOCLDWAIT`.
const SIGACTION_FLAGS: FlagPairs = &[(0x8, 0x4), (0x1_0000, 0x2)];

/// `sa_flags` as the kernel takes them.
pub fn sigaction_flags_in(flags: u64) -> u64 {
    flags_to_generic(flags, SIGACTION_FLAGS)
}

/// `sa_flags` as a program reads them back.
pub fn sigaction_flags_out(flags: u64) -> u64 {
    flags_to_mips(flags, SIGACTION_FLAGS)
}

/// MIPS's `siginfo_t` has `si_code` before `si_errno`: swaps the two (D325).
pub fn reorder_siginfo(info: &mut [u8]) {
    if info.len() >= 12 {
        let (errno, code) = info[4..12].split_at_mut(4);
        errno.swap_with_slice(code);
    }
}

/// MIPS's `POLLWRNORM` is `POLLOUT` and its `POLLWRBAND` is 0x100; the generic ones are 0x100
/// and 0x200.
const POLL_WRITE_BAND_MIPS: u16 = 0x100;
const POLL_WRITE_NORMAL_GENERIC: u16 = 0x100;
const POLL_WRITE_BAND_GENERIC: u16 = 0x200;
const POLL_OUT: u16 = 0x4;

/// Requested `poll` events in the generic values.
pub fn poll_events_in(events: u16) -> u16 {
    let band = if events & POLL_WRITE_BAND_MIPS != 0 { POLL_WRITE_BAND_GENERIC } else { 0 };
    (events & !POLL_WRITE_BAND_MIPS) | band
}

/// Returned `poll` events in MIPS's values.
pub fn poll_events_out(events: u16) -> u16 {
    let normal = if events & POLL_WRITE_NORMAL_GENERIC != 0 { POLL_OUT } else { 0 };
    let band = if events & POLL_WRITE_BAND_GENERIC != 0 { POLL_WRITE_BAND_MIPS } else { 0 };
    (events & !(POLL_WRITE_NORMAL_GENERIC | POLL_WRITE_BAND_GENERIC)) | normal | band
}

const SO_TYPE: u32 = 3;
const SO_ERROR: u32 = 4;

/// A `SOL_SOCKET` option's `int` in MIPS's values: `SO_TYPE`'s socket type and `SO_ERROR`'s
/// error number. `name` is the generic option, as [`arguments_in`] made it.
pub fn socket_option_out(name: u32, value: u32) -> u32 {
    match name {
        SO_TYPE => SOCKET_TYPES
            .iter()
            .find(|&&(_, generic)| generic == u64::from(value))
            .map_or(value, |&(mips, _)| mips as u32),
        SO_ERROR => eggos_hal::user::mips_errno(u64::from(value)) as u32,
        _ => value,
    }
}

/// MIPS's `struct termios`: 23 control characters in their own order, and the local flags
/// `IEXTEN`, `TOSTOP` and `FLUSHO` at their own bits (`asm/termbits.h`).
pub mod termios {
    use super::{FlagPairs, flags_to_generic, flags_to_mips};
    use crate::console::line_discipline::{CONTROL_CHARACTERS, Termios};

    const CONTROL_CHARACTERS_MIPS: usize = 23;
    /// Its size.
    pub const SIZE: usize = 17 + CONTROL_CHARACTERS_MIPS;
    /// (MIPS index, generic index) of each control character.
    const INDEXES: [(usize, usize); 17] = [
        (0, 0),
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 6),
        (5, 5),
        (6, 16),
        (7, 7),
        (8, 8),
        (9, 9),
        (10, 10),
        (12, 12),
        (13, 13),
        (14, 14),
        (15, 15),
        (16, 4),
        (17, 11),
    ];
    const LOCAL_FLAGS: FlagPairs = &[(0x100, 0x8000), (0x8000, 0x100), (0x2000, 0x1000)];

    /// `settings` as a MIPS program reads them.
    pub fn to_program(settings: &Termios) -> [u8; SIZE] {
        let mut bytes = [0; SIZE];
        let local = flags_to_mips(u64::from(settings.local), LOCAL_FLAGS) as u32;
        for (index, word) in [settings.input, settings.output, settings.control, local].into_iter().enumerate() {
            bytes[index * 4..][..4].copy_from_slice(&word.to_ne_bytes());
        }
        bytes[16] = settings.line;
        for (mips, generic) in INDEXES {
            if generic < CONTROL_CHARACTERS {
                bytes[17 + mips] = settings.control_characters[generic];
            }
        }
        bytes
    }

    /// The settings a MIPS program wrote.
    pub fn from_program(bytes: &[u8; SIZE]) -> Termios {
        let word = |index: usize| u32::from_ne_bytes(bytes[index * 4..][..4].try_into().unwrap());
        let mut settings = Termios {
            input: word(0),
            output: word(1),
            control: word(2),
            local: flags_to_generic(u64::from(word(3)), LOCAL_FLAGS) as u32,
            line: bytes[16],
            ..Termios::OFF
        };
        for (mips, generic) in INDEXES {
            if generic < CONTROL_CHARACTERS {
                settings.control_characters[generic] = bytes[17 + mips];
            }
        }
        settings
    }
}

/// `struct rt_sigframe { u32 rs_ass[4]; u32 rs_pad[2]; siginfo_t rs_info; struct ucontext
/// rs_uc; }`. The `ucontext` holds `uc_flags`, `uc_link` and `uc_stack`, then `uc_mcontext`,
/// the 64-bit `struct sigcontext` (`sc_regs[32]`, `sc_fpregs[32]`, `sc_mdhi`, `sc_hi1` to
/// `sc_hi3`, `sc_mdlo`, `sc_lo1` to `sc_lo3`, `sc_pc`, then `sc_fpc_csr`, `sc_used_math`,
/// `sc_dsp` and a reserved word), then `uc_sigmask` (16 bytes). The handler gets `a0` the
/// signal, `a1` the `siginfo_t`, `a2` the `ucontext`, `t9` its own address and `ra` the
/// trampoline, with the stack pointer at the frame.
pub mod signal_frame {
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word};
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    pub const INFO: usize = 24;
    pub const UCONTEXT: usize = INFO + INFO_SIZE;
    pub const MCONTEXT: usize = UCONTEXT + 40;
    const REGISTERS: usize = MCONTEXT;
    const FP_REGISTERS: usize = MCONTEXT + 256;
    const HI: usize = MCONTEXT + 512;
    const LO: usize = MCONTEXT + 544;
    const PC: usize = MCONTEXT + 576;
    const FCSR: usize = MCONTEXT + 584;
    const USED_MATH: usize = MCONTEXT + 588;
    pub const SIGMASK: usize = MCONTEXT + 600;
    /// The whole frame, the 16-byte mask included.
    pub const SIZE: u64 = (SIGMASK + 16) as u64;
    pub const WRITTEN: usize = SIZE as usize;
    pub const READ: usize = WRITTEN;
    /// No red zone in the n64 ABI.
    pub const RED_ZONE: u64 = 0;
    /// `$f0` to `$f31`, then `FCSR`.
    pub type FpPart = [u8; 264];

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; 264];
        for (index, f) in state.f().iter().enumerate() {
            part[index * 8..][..8].copy_from_slice(&f.to_ne_bytes());
        }
        part[256..][..4].copy_from_slice(&state.fcsr().to_ne_bytes());
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for (index, f) in state.f_mut().iter_mut().enumerate() {
            *f = word(part, index * 8);
        }
        state.set_fcsr(u32::from_ne_bytes(part[256..][..4].try_into().unwrap()));
        Ok(())
    }

    /// 16-byte aligned below `top`.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(SIZE) & !15
    }

    /// No exception is reported beyond the `siginfo_t`.
    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        _trap: Option<&Trap>,
        _restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        frame[INFO..][..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        for index in 1..32 {
            put(&mut frame, REGISTERS + index * 8, saved.registers.r[index]);
        }
        frame[FP_REGISTERS..][..256].copy_from_slice(&saved.fp[..256]);
        put(&mut frame, HI, saved.registers.hi);
        put(&mut frame, LO, saved.registers.lo);
        put(&mut frame, PC, saved.registers.pc);
        frame[FCSR..][..4].copy_from_slice(&saved.fp[256..260]);
        frame[USED_MATH..][..4].copy_from_slice(&1u32.to_ne_bytes());
        put(&mut frame, SIGMASK, saved.blocked);
        (place(top), frame)
    }

    /// Nothing outside the frame.
    pub fn write_extra<P: Platform>(_: &Process<P>, _: u64, _: u64, _: &Saved) -> Result<(), BadFrame> {
        Ok(())
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(_: &Process<P>, _: &[u8; READ], _: &mut Saved) -> Result<(), BadFrame> {
        Ok(())
    }

    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.r[4] = u64::from(handler.signal);
        registers.r[5] = frame + INFO as u64;
        registers.r[6] = frame + UCONTEXT as u64;
        registers.r[25] = handler.entry;
        registers.r[31] = handler.restorer;
        registers.r[29] = frame;
        registers.pc = handler.entry;
    }

    /// At the stack pointer.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.r[29])
    }

    pub fn parse(frame: &[u8; READ], _current: &UserRegisters) -> Option<Saved> {
        let mut r = [0; 32];
        for (index, value) in r.iter_mut().enumerate().skip(1) {
            *value = word(frame, REGISTERS + index * 8);
        }
        let mut fp = [0; 264];
        fp[..256].copy_from_slice(&frame[FP_REGISTERS..][..256]);
        fp[256..260].copy_from_slice(&frame[FCSR..][..4]);
        Some(Saved {
            registers: UserRegisters {
                pc: word(frame, PC),
                r,
                hi: word(frame, HI),
                lo: word(frame, LO),
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].try_into().unwrap(),
            fp,
        })
    }

    /// `v0`, which `rt_sigreturn` returns as the program had it.
    pub fn result(registers: &UserRegisters) -> u64 {
        registers.r[2]
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// `FPE_FLTINV`: an invalid floating-point operation.
    const FPE_FLTINV: i32 = 7;

    /// The signal, `si_code` and `si_addr` Linux gives `exception`, from `BadVAddr` for the
    /// faults that set it; the frame reports nothing more.
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
        fault(reported, Trap::default())
    }
}
