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

#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
)))]
compile_error!("EggOS has no Linux ABI for this architecture: add kernel/src/linux/abi/<arch>.rs");

/// What the architectures on Linux's generic ABI (`asm-generic`) share: AArch64, RISC-V and
/// the later ones (LoongArch).
pub mod generic {
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
        pub const LAST: u64 = FCHMODAT2;
    }

    /// What the generic architectures' signal frames share: `struct rt_sigframe { siginfo_t
    /// info; ucontext_t uc; }`, the `ucontext_t` holding `uc_sigmask`, 120 unused bytes (room
    /// for a larger `sigset_t`), then `uc_mcontext`, 16-byte aligned. The assertions hold
    /// Linux's offsets from the frame, written out: what programs read.
    pub mod signal_frame {
        use crate::linux::signal_frame::INFO_SIZE;

        pub const UCONTEXT: usize = INFO_SIZE;
        pub const SIGMASK: usize = UCONTEXT + 40;
        pub const MCONTEXT: usize = UCONTEXT + 176;
        const _: () = assert!(UCONTEXT == 128 && SIGMASK == 168 && MCONTEXT == 304);
    }
}

/// The layouts of `struct stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatLayout {
    /// x86_64's: a 64-bit `st_nlink` before `st_mode`, a 64-bit `st_blksize`.
    X86_64,
    /// The generic one: `st_mode` then a 32-bit `st_nlink`, a 32-bit `st_blksize`.
    Generic,
}

/// Three instructions as little-endian code bytes.
pub const fn code_bytes(words: [u32; 3]) -> [u8; 12] {
    let mut bytes = [0; 12];
    let mut index = 0;
    while index < 12 {
        bytes[index] = words[index / 4].to_le_bytes()[index % 4];
        index += 1;
    }
    bytes
}
