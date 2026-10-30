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
//! So far: each process's descriptor table, which starts with the console on descriptors 0 to
//! 2, with `close`, `dup`, `dup2`, `dup3`, `fcntl` and `close_range` ([`descriptors`], D101);
//! `write` and `writev` to the console (D61), and `read` and `readv` from it through its line
//! discipline ([`terminal`], D100), `exit`
//! and `exit_group`, the memory calls `brk`, `mmap` (anonymous), `munmap` and `mprotect`
//! ([`memory`], D53), `getpid`, `getppid`, `gettid`, `uname`, `set_tid_address` and
//! `set_robust_list` ([`identity`], D55), `getrandom` ([`random`], D56), and `sched_yield`
//! and, on x86_64, `arch_prctl` for the thread pointer ([`thread_pointer`], D57), and the
//! resource limits' `getrlimit`, `setrlimit` and `prlimit64` ([`limits`], D59), and the signal
//! state's `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` ([`signals`], D60), and, on the
//! console's descriptors, `poll` and `ppoll` ([`poll`]) and the terminal's `ioctl`s
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
//! `setuid`, `setresuid`, `setgroups` and their kin ([`credentials`], D75), and signals
//! between a process's threads with `tkill`, `tgkill` and `rt_tgsigqueueinfo` ([`signals`],
//! D95), and programs started from files with `execve` and `execveat` ([`exec`], D103), and
//! children waited for with `wait4` and `waitid` ([`wait`], D105) and process groups,
//! sessions and the console's foreground group ([`job_control`], D105), and the machine
//! powered off, halted or reset with `reboot` ([`reboot`], D106), and changes put on the
//! disk with `sync`, `syncfs`, `fsync` and `fdatasync` ([`file_changes`], D131), and `AF_UNIX`
//! stream sockets with `SCM_RIGHTS` ([`socket`], [`unix_socket`], D138), and memory files
//! with `memfd_create`, `fallocate` and their shared mappings ([`memfd`], D139), and, for
//! real programs, `/proc/<pid>` and `/proc/self` ([`proc`]), the memory devices `/dev/null`,
//! `/dev/zero`, `/dev/full`, `/dev/random` and `/dev/urandom` ([`memory_devices`]),
//! `readlink` and `readlinkat` ([`files`]), `access`, `faccessat` and `faccessat2`
//! ([`stat`]), and `sethostname` and `setdomainname` ([`identity`]) (D168), and symbolic and
//! hard links with `symlinkat`, `linkat` and `openat2`'s `RESOLVE_NO_SYMLINKS`, and
//! permissions with `fchmodat`, `fchmodat2` and `fchmod` ([`file_changes`], [`files`], D170),
//! and `sysinfo` ([`sysinfo`]) and `fadvise64` ([`files`]) for GTK apps (D176), and
//! `inotify_init`, `inotify_init1`, `inotify_add_watch` and `inotify_rm_watch` ([`inotify`],
//! D179) for file watchers such as Qt's and GLib's. Every
//! other number returns `ENOSYS`, as Linux does for a call it lacks, so a program that
//! probes for a call carries on. The next calls are the rest of what a static program's
//! startup needs, then the file system's (M7).

pub mod agent_channel;
pub mod credentials;
pub mod descriptors;
pub mod epoll;
pub mod eventfd;
pub mod exec;
pub mod fault;
pub mod file_changes;
pub mod file_locks;
pub mod files;
pub mod fork;
pub mod futex;
pub mod identity;
pub mod inotify;
pub mod job_control;
pub mod limits;
pub mod memfd;
pub mod memory;
pub mod memory_devices;
pub mod pipe;
pub mod poll;
pub mod proc;
pub mod process_ids;
pub mod process_table;
pub mod random;
pub mod reboot;
pub mod signal_frame;
pub mod signalfd;
pub mod signals;
pub mod socket;
pub mod stat;
pub mod statfs;
pub mod sysinfo;
pub mod terminal;
pub mod thread;
pub mod thread_pointer;
pub mod time;
pub mod timerfd;
pub mod trampoline;
pub mod unix_socket;
pub mod wait;
pub mod window;

use crate::process::{self, Process, ProcessExit, UnresolvedFault};
use crate::thread::{JoinHandle, SpawnError};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use descriptors::{Descriptors, FileKind};
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
        pub const LAST: u64 = FCHMODAT2;
    }
    #[cfg(not(target_arch = "x86_64"))]
    mod table {
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
        #[cfg(target_arch = "aarch64")]
        pub const RENAMEAT: u64 = 38;
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
        pub const SCHED_GETAFFINITY: u64 = 123;
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
    pub use table::*;

    const _: () = {
        let all = [
            READ,
            WRITE,
            READV,
            WRITEV,
            CLOSE,
            DUP,
            #[cfg(target_arch = "x86_64")]
            DUP2,
            DUP3,
            FCNTL,
            CLOSE_RANGE,
            #[cfg(target_arch = "x86_64")]
            PIPE,
            PIPE2,
            PWRITE64,
            FTRUNCATE,
            MKDIRAT,
            UNLINKAT,
            #[cfg(not(target_arch = "riscv64"))]
            RENAMEAT,
            RENAMEAT2,
            UTIMENSAT,
            SYNC,
            FSYNC,
            FDATASYNC,
            SYNCFS,
            #[cfg(target_arch = "x86_64")]
            RENAME,
            #[cfg(target_arch = "x86_64")]
            MKDIR,
            #[cfg(target_arch = "x86_64")]
            RMDIR,
            #[cfg(target_arch = "x86_64")]
            UNLINK,
            EXIT,
            EXIT_GROUP,
            CLONE,
            #[cfg(target_arch = "x86_64")]
            FORK,
            #[cfg(target_arch = "x86_64")]
            VFORK,
            WAIT4,
            WAITID,
            SETPGID,
            GETPGID,
            #[cfg(target_arch = "x86_64")]
            GETPGRP,
            SETSID,
            GETSID,
            PRCTL,
            REBOOT,
            BRK,
            MMAP,
            MUNMAP,
            MPROTECT,
            MREMAP,
            MSYNC,
            MADVISE,
            GETPID,
            GETPPID,
            GETTID,
            UNAME,
            SET_TID_ADDRESS,
            SET_ROBUST_LIST,
            GET_ROBUST_LIST,
            SCHED_YIELD,
            GETCPU,
            SCHED_GETSCHEDULER,
            SCHED_GETPARAM,
            SCHED_GETAFFINITY,
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
            #[cfg(target_arch = "x86_64")]
            OPEN,
            #[cfg(target_arch = "x86_64")]
            STAT,
            #[cfg(target_arch = "x86_64")]
            LSTAT,
            OPENAT,
            PREAD64,
            GETDENTS64,
            GETCWD,
            CHDIR,
            FCHDIR,
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
            SOCKET,
            SOCKETPAIR,
            BIND,
            LISTEN,
            ACCEPT,
            ACCEPT4,
            CONNECT,
            GETSOCKNAME,
            GETPEERNAME,
            SENDTO,
            RECVFROM,
            SENDMSG,
            RECVMSG,
            SHUTDOWN,
            SETSOCKOPT,
            GETSOCKOPT,
            FALLOCATE,
            MEMFD_CREATE,
            #[cfg(target_arch = "x86_64")]
            ACCESS,
            #[cfg(target_arch = "x86_64")]
            READLINK,
            READLINKAT,
            FACCESSAT,
            FACCESSAT2,
            SETHOSTNAME,
            SETDOMAINNAME,
            #[cfg(target_arch = "x86_64")]
            SELECT,
            PSELECT6,
            #[cfg(target_arch = "x86_64")]
            EPOLL_CREATE,
            EPOLL_CREATE1,
            EPOLL_CTL,
            #[cfg(target_arch = "x86_64")]
            EPOLL_WAIT,
            EPOLL_PWAIT,
            EPOLL_PWAIT2,
            #[cfg(target_arch = "x86_64")]
            EVENTFD,
            EVENTFD2,
            TIMERFD_CREATE,
            TIMERFD_SETTIME,
            TIMERFD_GETTIME,
            #[cfg(target_arch = "x86_64")]
            SIGNALFD,
            SIGNALFD4,
            #[cfg(target_arch = "x86_64")]
            INOTIFY_INIT,
            INOTIFY_INIT1,
            INOTIFY_ADD_WATCH,
            INOTIFY_RM_WATCH,
            #[cfg(target_arch = "x86_64")]
            LINK,
            #[cfg(target_arch = "x86_64")]
            SYMLINK,
            #[cfg(target_arch = "x86_64")]
            CHMOD,
            FCHMOD,
            LINKAT,
            SYMLINKAT,
            FCHMODAT,
            OPENAT2,
            FCHMODAT2,
            FLOCK,
            STATFS,
            FSTATFS,
            SYSINFO,
            FADVISE64,
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
    /// `kill` may send queued to another process (`rt_sigqueueinfo`), or the wall clock set by
    /// a process that is not privileged (`clock_settime`, `settimeofday`).
    pub const EPERM: Self = Self(1);
    /// No such file: here, a program's interpreter (`execve`), or any file a path names,
    /// as there are none yet (`newfstatat`, `statx`).
    pub const ENOENT: Self = Self(2);
    /// No such process: here, one the caller cannot reach (`prlimit64`).
    pub const ESRCH: Self = Self(3);
    /// Interrupted: a signal's handler started during the wait (`rt_sigsuspend`, `pause`).
    pub const EINTR: Self = Self(4);
    /// An input or output error: the disk under a file system failed (`read`, `execve`).
    pub const EIO: Self = Self(5);
    /// No child the caller may wait for (`wait4`, `waitid`).
    pub const ECHILD: Self = Self(10);
    /// The arguments and environment are too long (`execve`).
    pub const E2BIG: Self = Self(7);
    /// Not a program the kernel can run (`execve`).
    pub const ENOEXEC: Self = Self(8);
    /// A program's interpreter (its dynamic loader) is not one the kernel can run (`execve`,
    /// D167).
    pub const ELIBBAD: Self = Self(80);
    /// Bad file descriptor: one that is not open, or not open for what the call does (D101),
    /// or a target above the open-file limit (`dup2`, `dup3`).
    pub const EBADF: Self = Self(9);
    /// Try again: here, the random number generator is not seeded yet (`getrandom` with
    /// `GRND_NONBLOCK`), a real-time signal finds the signal queue full (`rt_sigqueueinfo`,
    /// `tgkill`), no signal came before the timeout (`rt_sigtimedwait`), or a read with
    /// `O_NONBLOCK` finds no input (`read`, `readv`).
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
    /// No such device or address: an offset at or past the end of a file (`lseek`'s
    /// `SEEK_DATA` and `SEEK_HOLE`), or `/dev/window` without a display.
    pub const ENXIO: Self = Self(6);
    /// Not a directory: a path component, or a descriptor a relative path starts from, that
    /// is not one.
    pub const ENOTDIR: Self = Self(20);
    /// Is a directory: a directory opened for writing or read as a file.
    pub const EISDIR: Self = Self(21);
    /// The result does not fit: a buffer too small for the working directory (`getcwd`).
    pub const ERANGE: Self = Self(34);
    /// A read-only file system: a file opened for writing or created on one.
    pub const EROFS: Self = Self(30);
    /// An invalid argument: here, a bad flag, the same descriptor twice (`dup3`), an unknown
    /// command or a too high lowest descriptor (`fcntl`), or a bad range (`close_range`), a
    /// segment outside user space (`execve`), an unaligned
    /// address or an empty range (the memory calls), a bad signal number, set size or flag
    /// (the signal calls), a bad timeout or too many descriptors (`ppoll`, `poll`), or a
    /// clock the time calls do not answer for or a bad time (the time calls), or a bad flag
    /// or mask (`newfstatat`, `statx`) or `whence` (`lseek`), or an id of -1 (`setuid`,
    /// `setgid`), a list too small or too long (`getgroups`, `setgroups`).
    pub const EINVAL: Self = Self(22);
    /// Too many open files: every descriptor below the open-file limit is taken (`dup`,
    /// `fcntl`'s `F_DUPFD`).
    pub const EMFILE: Self = Self(24);
    /// Too many open files in the system: here, every client slot of the window server is
    /// taken (`/dev/window`, D127).
    pub const ENFILE: Self = Self(23);
    /// Not a terminal: here, an `ioctl` request the console does not answer.
    pub const ENOTTY: Self = Self(25);
    /// Not seekable: here, the console or a pipe (`lseek`, `pread64`).
    pub const ESPIPE: Self = Self(29);
    /// A write to a pipe that no reader has open any more.
    pub const EPIPE: Self = Self(32);
    /// Busy: here, a rename of `.` or `..`, or a mount point removed.
    pub const EBUSY: Self = Self(16);
    /// A rename or a hard link across file systems.
    pub const EXDEV: Self = Self(18);
    /// Too many links: a file that has as many names as it may have (`linkat`, D170).
    pub const EMLINK: Self = Self(31);
    /// A file larger than the largest offset.
    pub const EFBIG: Self = Self(27);
    /// A file system's memory is used up.
    pub const ENOSPC: Self = Self(28);
    /// A directory removed or replaced while it has entries.
    pub const ENOTEMPTY: Self = Self(39);
    /// A path longer than `PATH_MAX`, or a component longer than `NAME_MAX`.
    pub const ENAMETOOLONG: Self = Self(36);
    /// A walk met too many symbolic links, or `O_NOFOLLOW` met one at the end (D168).
    pub const ELOOP: Self = Self(40);
    /// No such system call.
    pub const ENOSYS: Self = Self(38);
    /// Timed out: a wait's deadline came first (`futex`).
    pub const ETIMEDOUT: Self = Self(110);
    /// Not supported: here, an unknown flag with `MAP_SHARED_VALIDATE` (`mmap`), or a clock
    /// `clock_nanosleep` cannot sleep on, or out-of-band data or another option level on a
    /// socket (D138).
    pub const EOPNOTSUPP: Self = Self(95);
    /// Out of the domain: a timeout's microseconds out of range (`setsockopt`).
    pub const EDOM: Self = Self(33);
    /// Not a socket: a socket call on another descriptor.
    pub const ENOTSOCK: Self = Self(88);
    /// No such option (`getsockopt`, `setsockopt`).
    pub const ENOPROTOOPT: Self = Self(92);
    /// A protocol the family does not have (`socket`).
    pub const EPROTONOSUPPORT: Self = Self(93);
    /// A socket type the family does not have (`socket`): datagrams and sequenced packets.
    pub const ESOCKTNOSUPPORT: Self = Self(94);
    /// An address family there is no socket for (`socket`): every one but `AF_UNIX`.
    pub const EAFNOSUPPORT: Self = Self(97);
    /// The address is taken (`bind`).
    pub const EADDRINUSE: Self = Self(98);
    /// The peer closed with bytes it had not read (`recv`, `SO_ERROR`).
    pub const ECONNRESET: Self = Self(104);
    /// No room for the control buffer (`sendmsg`).
    pub const ENOBUFS: Self = Self(105);
    /// The socket is connected already (`connect`, `sendto` with an address).
    pub const EISCONN: Self = Self(106);
    /// The socket is not connected (`send`, `recv`, `getpeername`, `shutdown`).
    pub const ENOTCONN: Self = Self(107);
    /// Too many descriptors in flight, or a socket that would make a cycle (`sendmsg`).
    pub const ETOOMANYREFS: Self = Self(109);
    /// Nothing listens there (`connect`).
    pub const ECONNREFUSED: Self = Self(111);
    /// Another call is connecting the socket already (`connect`).
    pub const EALREADY: Self = Self(114);
    /// A non-blocking connection is being made (`connect`, D158).
    pub const EINPROGRESS: Self = Self(115);
    /// Too many buffers for one message (`sendmsg`, `recvmsg`).
    pub const EMSGSIZE: Self = Self(90);
    /// A mapping that would end past the largest offset (`mmap` of a memfd), or a lock's range
    /// past it (`fcntl`, D171).
    pub const EOVERFLOW: Self = Self(75);
    /// No memory left for one more file lock (`flock`, `fcntl`, D171).
    pub const ENOLCK: Self = Self(37);
    /// A record lock's wait would never end: its holder waits for the caller (`fcntl`'s
    /// `F_SETLKW`, D173).
    pub const EDEADLK: Self = Self(35);
    /// A datagram socket with no destination (`send` unconnected, D157).
    pub const EDESTADDRREQ: Self = Self(89);
    /// An address this machine does not have (`bind`, D157).
    pub const EADDRNOTAVAIL: Self = Self(99);
    /// The network is down: no cable (`sendto`, D157).
    pub const ENETDOWN: Self = Self(100);
    /// No route to the network (`sendto`, D157).
    pub const ENETUNREACH: Self = Self(101);

    /// The value the program sees in its result register: `-errno`.
    pub fn result(self) -> u64 {
        (-i64::from(self.0)) as u64
    }
}

/// Linux's standard signal numbers (the generic ones, which AArch64, RISC-V and x86_64 share);
/// the real-time ones follow, up to 64.
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
    job_control::start_session_leader(&process, &thread);
    spawn_thread(process, thread, context)
}

/// Starts `thread`, of `process`, running the program from `context`, its state kept for its
/// loop and handed to [`handle`] with each exit from user mode. The state is boxed: the loop
/// keeps one pointer to it, and its own variables stay in registers (in the loop's closure
/// with them, the system calls taking its address kept them in memory: +3 to +5% on the
/// system-call round trip). [`SpawnError::OutOfMemory`] when its stack would take the free
/// frames below the reserve programs leave the kernel (D155).
pub(crate) fn spawn_thread<P: Platform>(
    process: Arc<Process<P>>,
    thread: LinuxThread,
    context: UserContext,
) -> Result<JoinHandle, SpawnError> {
    if !crate::heap::may_grow_for_user(crate::thread::STACK_SIZE) {
        return Err(SpawnError::OutOfMemory);
    }
    let mut thread = Box::new(thread);
    process.add_thread();
    let counted = Arc::clone(&process);
    let handler = move |process: &Arc<Process<P>>, context: &mut UserContext, exit| {
        handle(process, &mut thread, context, exit)
    };
    process::spawn::<P, _>(process, context, handler).inspect_err(|_| {
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
        UserExit::Interrupt => signals::deliver_sent_on_interrupt(process, thread, context),
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
        Err(UnresolvedFault::BusError) => {
            let exception = context.last_exception();
            let fault = fault::bus_error(process, &exception, context.stack());
            signals::deliver_fault(process, thread, context, &fault)
        }
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
    if first_fp_use::<P>(thread, exception.kind) {
        return ControlFlow::Continue(());
    }
    let fault = fault::classify(process, &exception, context.stack());
    signals::deliver_fault(process, thread, context, &fault)
}

pub(crate) fn load_inherited_fp<P: Platform>(thread: &mut LinuxThread) {
    if let Some(state) = thread.take_inherited_fp() {
        let _ = crate::thread::start_fp_from::<P>(state);
    }
}

fn first_fp_use<P: Platform>(thread: &mut LinuxThread, kind: ExceptionKind) -> bool {
    if kind == ExceptionKind::IllegalInstruction
        && let Some(state) = thread.take_inherited_fp()
    {
        return crate::thread::start_fp_from::<P>(state) == crate::thread::FpStart::Started;
    }
    process::first_fp_use::<P>(kind)
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
        number::WRITE => match write::<P>(process, thread.files(), first, second, third) {
            Err(NOT_THE_CONSOLE) => return out_of_line_call::<P>(process, thread, number, context),
            result => result,
        },
        number::EXIT | number::EXIT_GROUP => return exit(process, thread, number, first),
        number::BRK | number::MMAP | number::MUNMAP | number::MPROTECT => {
            memory_call::<P>(process, thread.files(), number, context.syscall_arguments())
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
    files: &Descriptors,
    number: u64,
    [first, second, third, fourth, fifth, sixth]: [u64; 6],
) -> Result<u64, Errno> {
    match number {
        number::BRK => Ok(memory::brk::<P>(process, first)),
        number::MMAP => {
            let file = memory::MappedFile { files, fd: fifth };
            memory::mmap_file::<P>(process, first, second, third, fourth, file, sixth)
        }
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
/// breaks; after `sched_yield`, which may switch the thread out, what other threads sent
/// meanwhile is taken in and delivered ([`signals::deliver_sent`]); the others pay nothing
/// for it. `rt_sigreturn` sets every register itself and returns the restored result
/// register's value, so writing the result back changes nothing. The calls that wait (for a
/// signal, for time, on a `futex`, for console input) set their result and deliver
/// themselves, since a signal that interrupts them decides their result; `poll` delivers
/// after it as `ppoll` does.
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
        number::READ => read::<P>(process, thread, context, Buffers::One),
        number::READV => read::<P>(process, thread, context, Buffers::Vectors),
        number::WRITE => write_out_of_line::<P>(process, thread, context, Buffers::One),
        number::WRITEV => write_out_of_line::<P>(process, thread, context, Buffers::Vectors),
        number::PPOLL => match poll::ppoll::<P>(process, thread, context) {
            ControlFlow::Continue(()) => signals::deliver(process, thread, context),
            ended => ended,
        },
        #[cfg(target_arch = "x86_64")]
        number::POLL => {
            let [fds, count, timeout, ..] = context.syscall_arguments();
            let result = poll::poll::<P>(process, thread, fds, count, timeout);
            context.set_syscall_result(result.unwrap_or_else(Errno::result));
            signals::deliver(process, thread, context)
        }
        number::NANOSLEEP => time::nanosleep(process, thread, context),
        number::CLOCK_NANOSLEEP => time::clock_nanosleep(process, thread, context),
        number::FUTEX => futex::futex::<P>(process, thread, context),
        number::EXECVE => exec::execve(process, thread, context),
        number::IOCTL => ioctl_call::<P>(process, thread, context),
        number::WAIT4 => wait::wait4(process, thread, context),
        number::WAITID => wait::waitid(process, thread, context),
        number::EXECVEAT => exec::execveat(process, thread, context),
        number::FLOCK => file_locks::flock(process, thread, context),
        number::FCNTL if file_locks::is_lock_command(context.syscall_arguments()[1]) => {
            file_locks::fcntl(process, thread, context)
        }
        number::SOCKET
        | number::SOCKETPAIR
        | number::BIND
        | number::LISTEN
        | number::ACCEPT
        | number::ACCEPT4
        | number::CONNECT
        | number::GETSOCKNAME
        | number::GETPEERNAME
        | number::SENDTO
        | number::RECVFROM
        | number::SENDMSG
        | number::RECVMSG
        | number::SHUTDOWN
        | number::SETSOCKOPT
        | number::GETSOCKOPT => socket::call(process, thread, number, context),
        number::PSELECT6
        | number::EPOLL_CREATE1
        | number::EPOLL_CTL
        | number::EPOLL_PWAIT
        | number::EPOLL_PWAIT2
        | number::EVENTFD2
        | number::TIMERFD_CREATE
        | number::TIMERFD_SETTIME
        | number::TIMERFD_GETTIME
        | number::SIGNALFD4
        | number::INOTIFY_INIT1
        | number::INOTIFY_ADD_WATCH
        | number::INOTIFY_RM_WATCH => poll::call(process, thread, number, context),
        #[cfg(target_arch = "x86_64")]
        number::SELECT
        | number::EPOLL_CREATE
        | number::EPOLL_WAIT
        | number::EVENTFD
        | number::SIGNALFD
        | number::INOTIFY_INIT => poll::call(process, thread, number, context),
        _ => {
            let result = match number {
                number::CLONE => {
                    load_inherited_fp::<P>(thread);
                    thread::clone::<P>(process, thread, context)
                }
                #[cfg(target_arch = "x86_64")]
                number::FORK | number::VFORK => {
                    load_inherited_fp::<P>(thread);
                    match number {
                        number::FORK => fork::fork::<P>(process, thread, context),
                        _ => fork::vfork::<P>(process, thread, context),
                    }
                }
                _ => out_of_line_result(process, thread, number, context),
            };
            context.set_syscall_result(result.unwrap_or_else(Errno::result));
            if !is_in(&SIGNALS_AFTER, number) {
                ControlFlow::Continue(())
            } else if number == number::SCHED_YIELD {
                signals::deliver_sent(process, thread, context)
            } else {
                signals::deliver(process, thread, context)
            }
        }
    };
    if process.has_ended() {
        return ControlFlow::Break(());
    }
    flow
}

/// A set of call numbers up to [`number::LAST`], a bit each.
type CallSet = [u64; number::LAST as usize / 64 + 1];

const fn call_set(numbers: &[u64]) -> CallSet {
    let mut set = [0; number::LAST as usize / 64 + 1];
    let mut index = 0;
    while index < numbers.len() {
        let number = numbers[index] as usize;
        set[number / 64] |= 1 << (number % 64);
        index += 1;
    }
    set
}

#[inline(always)]
fn is_in(set: &CallSet, number: u64) -> bool {
    set.get(number as usize / 64)
        .is_some_and(|word| word >> (number % 64) & 1 != 0)
}

/// The calls after which [`out_of_line_call`] delivers signals: those that can make one due,
/// and `sched_yield`, which may switch the thread out (after it, only what other threads sent
/// meanwhile is delivered, [`signals::deliver_sent`]). One bit test on the others' path,
/// which a list of comparisons would lengthen with each call.
const SIGNALS_AFTER: CallSet = call_set(&[
    number::KILL,
    number::TKILL,
    number::TGKILL,
    number::RT_SIGQUEUEINFO,
    number::RT_TGSIGQUEUEINFO,
    number::RT_SIGPROCMASK,
    number::RT_SIGACTION,
    number::RT_SIGRETURN,
    number::RT_SIGTIMEDWAIT,
    number::SCHED_YIELD,
]);

/// The result of [`out_of_line_call`]'s call `number`.
#[inline(always)]
fn out_of_line_result<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> Result<u64, Errno> {
    use credentials::Kind::{Group, User};
    let [first, second, third, fourth, fifth, _] = context.syscall_arguments();
    match number {
        number::CLOSE => file_locks::close(process.id().get(), thread.files(), first),
        number::DUP => descriptors::dup(process, thread.files(), first),
        #[cfg(target_arch = "x86_64")]
        number::DUP2 => descriptors::dup2(process, thread.files(), first, second),
        number::DUP3 => descriptors::dup3(process, thread.files(), first, second, third),
        number::FCNTL => descriptors::fcntl(process, thread.files(), first, second, third),
        number::PIPE2 => pipe::pipe2::<P>(process, thread, first, second),
        number::PWRITE64 => file_changes::pwrite64(process, thread, first, second, third, fourth),
        number::FTRUNCATE => file_changes::ftruncate::<P>(thread, first, second),
        number::FALLOCATE => memfd::fallocate(thread, first, second, third, fourth),
        number::MEMFD_CREATE => memfd::memfd_create(process, thread, first, second),
        number::MKDIRAT => file_changes::mkdirat(process, thread, first, second, third),
        number::UNLINKAT => file_changes::unlinkat(process, thread, first, second, third),
        #[cfg(not(target_arch = "riscv64"))]
        number::RENAMEAT => {
            file_changes::renameat2(process, thread, first, second, third, fourth, 0)
        }
        number::RENAMEAT2 => {
            file_changes::renameat2(process, thread, first, second, third, fourth, fifth)
        }
        number::UTIMENSAT => file_changes::utimensat(process, thread, first, second, third, fourth),
        number::SYNC => file_changes::sync(),
        number::FSYNC | number::FDATASYNC => file_changes::fsync(thread, first),
        number::SYNCFS => file_changes::syncfs(thread, first),
        #[cfg(target_arch = "x86_64")]
        number::MKDIR => file_changes::mkdirat(process, thread, CWD, first, second),
        #[cfg(target_arch = "x86_64")]
        number::RMDIR => {
            file_changes::unlinkat(process, thread, CWD, first, file_changes::AT_REMOVEDIR)
        }
        #[cfg(target_arch = "x86_64")]
        number::UNLINK => file_changes::unlinkat(process, thread, CWD, first, 0),
        #[cfg(target_arch = "x86_64")]
        number::RENAME => file_changes::renameat2(process, thread, CWD, first, CWD, second, 0),
        #[cfg(target_arch = "x86_64")]
        number::PIPE => pipe::pipe2::<P>(process, thread, first, 0),
        number::CLOSE_RANGE => {
            let own =
                descriptors::close_range(process.id().get(), thread.files(), first, second, third)?;
            if let Some(own) = own {
                thread.set_files(own);
            }
            Ok(0)
        }
        number::GETPID => Ok(identity::getpid(process)),
        number::GETPPID => Ok(identity::getppid(process)),
        number::SETPGID => job_control::setpgid(process, first, second),
        number::GETPGID => job_control::getpgid(process, first),
        #[cfg(target_arch = "x86_64")]
        number::GETPGRP => job_control::getpgid(process, 0),
        number::SETSID => job_control::setsid(process),
        number::GETSID => job_control::getsid(process, first),
        number::PRCTL => job_control::prctl(process, first, second),
        number::REBOOT => reboot::reboot(process, first, second, third),
        number::GETTID => Ok(identity::gettid(thread)),
        number::UNAME => identity::uname(process, first),
        number::SET_TID_ADDRESS => Ok(identity::set_tid_address(thread, first)),
        number::SET_ROBUST_LIST => identity::set_robust_list(thread, first, second),
        number::GET_ROBUST_LIST => identity::get_robust_list(process, thread, first, second, third),
        number::GETRANDOM => random::getrandom::<P>(process, first, second, third),
        number::GETCPU => identity::getcpu(process, first, second),
        number::SCHED_GETAFFINITY => {
            identity::sched_getaffinity(process, thread, first, second, third)
        }
        number::SCHED_GETSCHEDULER => identity::sched_getscheduler(process, thread, first),
        number::SCHED_GETPARAM => identity::sched_getparam(process, thread, first, second),
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
        number::CLOCK_SETTIME => time::clock_settime(process, first, second),
        number::SETTIMEOFDAY => time::settimeofday(process, first, second),
        number::LSEEK => lseek(thread.files(), first, second, third),
        number::FSTAT => stat::fstat(process, thread.files(), first, second),
        number::STATFS => statfs::statfs(process, thread, first, second),
        number::FSTATFS => statfs::fstatfs(process, thread.files(), first, second),
        number::SYSINFO => sysinfo::sysinfo(process, first),
        number::FADVISE64 => files::fadvise64(thread.files(), first, third, fourth),
        number::NEWFSTATAT => stat::newfstatat(process, thread, first, second, third, fourth),
        number::STATX => stat::statx(process, thread, first, second, third, fourth, fifth),
        number::OPENAT => files::openat(process, thread, first, second, third, fourth),
        #[cfg(target_arch = "x86_64")]
        number::OPEN => files::openat(process, thread, CWD, first, second, third),
        #[cfg(target_arch = "x86_64")]
        number::STAT => stat::newfstatat(process, thread, stat::at::FDCWD as u64, first, second, 0),
        #[cfg(target_arch = "x86_64")]
        number::LSTAT => stat::newfstatat(
            process,
            thread,
            stat::at::FDCWD as u64,
            first,
            second,
            stat::at::SYMLINK_NOFOLLOW,
        ),
        number::PREAD64 => files::pread64(process, thread.files(), first, second, third, fourth),
        number::GETDENTS64 => {
            files::getdents64(process, &*thread.files().file(first)?, second, third)
        }
        number::GETCWD => files::getcwd(process, thread, first, second),
        number::READLINKAT => files::readlinkat(process, thread, first, second, third, fourth),
        #[cfg(target_arch = "x86_64")]
        number::READLINK => files::readlinkat(process, thread, CWD, first, second, third),
        number::FACCESSAT => stat::faccessat2(process, thread, first, second, third, 0),
        number::FACCESSAT2 => stat::faccessat2(process, thread, first, second, third, fourth),
        #[cfg(target_arch = "x86_64")]
        number::ACCESS => stat::faccessat2(process, thread, CWD, first, second, 0),
        number::SYMLINKAT => file_changes::symlinkat(process, thread, first, second, third),
        #[cfg(target_arch = "x86_64")]
        number::SYMLINK => file_changes::symlinkat(process, thread, first, CWD, second),
        number::LINKAT => {
            file_changes::linkat(process, thread, first, second, third, fourth, fifth)
        }
        #[cfg(target_arch = "x86_64")]
        number::LINK => file_changes::linkat(process, thread, CWD, first, CWD, second, 0),
        number::FCHMODAT => file_changes::fchmodat2(process, thread, first, second, third, 0),
        number::FCHMODAT2 => file_changes::fchmodat2(process, thread, first, second, third, fourth),
        #[cfg(target_arch = "x86_64")]
        number::CHMOD => file_changes::fchmodat2(process, thread, CWD, first, second, 0),
        number::FCHMOD => file_changes::fchmod(process, thread, first, second),
        number::OPENAT2 => files::openat2(process, thread, first, second, third, fourth),
        number::SETHOSTNAME => identity::sethostname(process, first, second),
        number::SETDOMAINNAME => identity::setdomainname(process, first, second),
        number::CHDIR => files::chdir(process, thread, first),
        number::FCHDIR => files::fchdir(thread, first),
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
        number::MSYNC => memory::msync::<P>(process, first, second, third),
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
        clear_child_tid_if_memory_shared::<P>(process, thread);
        return end_process(process, ProcessExit::Exited(u32::from(status)));
    }
    {
        let mut signals = process.signals().lock();
        signals.remove_thread(thread.id());
        if !signals.has_sent_to_threads() {
            process.set_sent_signals_waiting(false);
        }
        thread.signals(&mut signals).discard(u64::MAX);
    }
    let robust_list = thread.robust_list();
    if robust_list != 0 {
        futex::release_robust_list(process, thread.id(), robust_list);
    }
    let address = thread.clear_child_tid();
    if address != 0 {
        clear_child_tid::<P>(process, address);
    }
    ControlFlow::Break(())
}

fn clear_child_tid<P: Platform>(process: &Process<P>, address: u64) {
    let cleared = process.write_user_memory(address, &0u32.to_ne_bytes());
    if cleared.is_ok() && address.is_multiple_of(4) {
        let key = futex::Key::new(process.futex_space(), address, true);
        futex::wake::<P>(key, u32::MAX, 1);
    }
}

/// For `thread`, the last of `process` to run its program, as it ends it or `execve` replaces
/// it: clears and wakes its `clear_child_tid` ([`exit_thread`]) if another process shares the
/// memory (`CLONE_VM`) and may wait on the word, as Linux does when it releases the memory.
#[inline]
pub fn clear_child_tid_if_memory_shared<P: Platform>(process: &Process<P>, thread: &LinuxThread) {
    let address = thread.clear_child_tid();
    if address != 0 && process.shares_memory() {
        clear_child_tid::<P>(process, address);
    }
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
        clear_child_tid_if_memory_shared::<P>(process, thread);
        end_process(process, ProcessExit::Exited(status as u32 & 0xff))
    }
}

/// Ends the process as `how` says ([`Process::exit`]); its parent learns of it (D105,
/// [`wait::process_ended`]); its other threads waiting on a futex or for console input, if it
/// has any, stop waiting, so they end too. Breaks: the calling thread ends.
#[cold]
#[inline(never)]
pub fn end_process<P: Platform, C>(process: &Process<P>, how: ProcessExit) -> ControlFlow<(), C> {
    let others = process.threads() > 1;
    if !process.exit(how) {
        return ControlFlow::Break(());
    }
    fork::release_vfork_parent::<P>(process);
    file_locks::process_ended(process.id().get());
    wait::process_ended::<P>(process, how);
    if others {
        futex::wake_space::<P>(process.futex_space());
        crate::console::wake_input_waiters::<P>();
        wait::wake_child_waiters::<P>(process.id().get());
    }
    ControlFlow::Break(())
}

#[inline(never)]
fn write<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    fd: u64,
    buffer: u64,
    count: u64,
) -> Result<u64, Errno> {
    let file = writable_file(files, fd)?;
    if !ends_in_user_space::<P>(buffer, count) {
        return Err(Errno::EFAULT);
    }
    let count = count.min(MAX_RW_COUNT);
    let written = match file.kind() {
        FileKind::Console => write_to_console(process, buffer, count),
        FileKind::Window(_) => return Err(Errno::EINVAL),
        _ => return Err(NOT_THE_CONSOLE),
    };
    if written == 0 && count > 0 {
        Err(Errno::EFAULT)
    } else {
        Ok(written)
    }
}

fn writable_file(files: &Descriptors, fd: u64) -> Result<Arc<descriptors::OpenFile>, Errno> {
    let file = files.file(fd)?;
    match file.is_writable() {
        true => Ok(file),
        false => Err(Errno::EBADF),
    }
}

fn write_iovecs_to_console<P: Platform>(
    process: &Process<P>,
    iovecs: &Iovecs,
) -> Result<u64, Errno> {
    let mut written = 0;
    for entry in iovecs.entries() {
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

const NOT_THE_CONSOLE: Errno = Errno(u16::MAX);

#[cfg(target_arch = "x86_64")]
const CWD: u64 = stat::at::FDCWD as u64;

#[inline(never)]
fn write_out_of_line<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    buffers: Buffers,
) -> ControlFlow<()> {
    let [fd, address, count, ..] = context.syscall_arguments();
    let prepared = writable_file(thread.files(), fd).and_then(|file| {
        let iovecs = match buffers {
            Buffers::One if !ends_in_user_space::<P>(address, count) => Err(Errno::EFAULT),
            Buffers::One => Ok(Iovecs::one(address, count)),
            Buffers::Vectors => Iovecs::read(process, address, count),
        }?;
        Ok((file, iovecs))
    });
    match prepared {
        Ok((file, iovecs)) => match file.kind() {
            FileKind::Pipe(end) => pipe::write(process, thread, context, &file, end, &iovecs),
            FileKind::Regular(node) => {
                let result = file_changes::write_regular(process, &file, *node, &iovecs, None);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::MemoryFile(memory) => {
                let result = memfd::write(process, &file, memory, &iovecs, None);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::MemoryDevice(device, _) => {
                let buffers = iovecs.entries().iter().map(iovec);
                let result = memory_devices::write(process, *device, buffers);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::Console => {
                let result = write_iovecs_to_console(process, &iovecs);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::Window(_) => {
                context.set_syscall_result(Errno::EINVAL.result());
                ControlFlow::Continue(())
            }
            FileKind::AgentChannel(_) => {
                agent_channel::write(process, thread, context, &file, &iovecs)
            }
            FileKind::Socket(socket) => {
                socket::write(process, thread, context, &file, socket, iovecs)
            }
            FileKind::InetSocket(socket) => {
                socket::inet::write(process, thread, context, socket, iovecs)
            }
            FileKind::Event(event) => {
                poll::write_event(process, thread, context, &file, event, &iovecs)
            }
            _ => {
                context.set_syscall_result(Errno::EBADF.result());
                ControlFlow::Continue(())
            }
        },
        Err(error) => {
            context.set_syscall_result(error.result());
            ControlFlow::Continue(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Buffers {
    One,
    Vectors,
}

#[inline(never)]
fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    buffers: Buffers,
) -> ControlFlow<()> {
    let [fd, address, count, ..] = context.syscall_arguments();
    let prepared = thread.files().file(fd).and_then(|file| {
        if !file.is_readable() || matches!(file.kind(), FileKind::Path(_)) {
            return Err(Errno::EBADF);
        }
        let iovecs = match buffers {
            Buffers::One if !ends_in_user_space::<P>(address, count) => Err(Errno::EFAULT),
            Buffers::One => Ok(Iovecs::one(address, count)),
            Buffers::Vectors => Iovecs::read(process, address, count),
        }?;
        Ok((file, iovecs))
    });
    match prepared {
        Ok((file, iovecs)) => match file.kind() {
            FileKind::Console => terminal::read(process, thread, context, &file, &iovecs),
            FileKind::Pipe(end) => pipe::read(process, thread, context, &file, end, &iovecs),
            FileKind::Window(connection) => {
                window::read(process, thread, context, &file, connection, &iovecs)
            }
            FileKind::AgentChannel(_) => {
                agent_channel::read(process, thread, context, &file, &iovecs)
            }
            FileKind::Socket(socket) => {
                socket::read(process, thread, context, &file, socket, iovecs)
            }
            FileKind::InetSocket(socket) => {
                socket::inet::read(process, thread, context, &file, socket, iovecs)
            }
            FileKind::MemoryFile(memory) => {
                let result = memfd::read(process, &file, memory, &iovecs);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::MemoryDevice(device, _) => {
                let nonblocking = file.status_flags() & descriptors::open_flag::NONBLOCK != 0;
                let buffers = iovecs.entries().iter().map(iovec);
                let result = memory_devices::read(process, *device, nonblocking, buffers);
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
            FileKind::Event(event) => {
                poll::read_event(process, thread, context, &file, event, &iovecs)
            }
            kind => {
                let result = match files::Contents::of(kind) {
                    Some(contents) => {
                        let buffers = iovecs.entries().iter().map(iovec);
                        files::read_regular(process, &file, contents, buffers)
                    }
                    None => Err(Errno::EISDIR),
                };
                context.set_syscall_result(result.unwrap_or_else(Errno::result));
                ControlFlow::Continue(())
            }
        },
        Err(error) => {
            context.set_syscall_result(error.result());
            ControlFlow::Continue(())
        }
    }
}

fn ioctl_call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fd, request, argument, ..] = context.syscall_arguments();
    let result = ioctl::<P>(process, thread, fd, request, argument);
    if result == Err(job_control::SIGNALLED) {
        return job_control::restart_after_signal(process, thread, context);
    }
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    fd: u64,
    request: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    if file.is_location_only() {
        return Err(Errno::EBADF);
    }
    if let Some(result) =
        descriptors::descriptor_ioctl(process, thread.files(), &file, fd, request, argument)
    {
        return result;
    }
    match file.kind() {
        FileKind::Console if terminal::changes_the_terminal(request) => {
            match job_control::terminal_access(process, thread, signal::SIGTTOU) {
                job_control::Access::Allowed => terminal::ioctl(process, request, argument),
                job_control::Access::Refused => Err(Errno::EIO),
                job_control::Access::Signalled => Err(job_control::SIGNALLED),
            }
        }
        FileKind::Console => terminal::ioctl(process, request, argument),
        FileKind::Window(connection) => {
            window::ioctl(process, thread, connection, request, argument)
        }
        FileKind::Socket(socket) => {
            socket::ioctl(process, socket, request, argument).unwrap_or(Err(Errno::ENOTTY))
        }
        FileKind::InetSocket(socket) => {
            socket::inet::ioctl(process, socket, request, argument).unwrap_or(Err(Errno::ENOTTY))
        }
        FileKind::Pipe(end) if request as u32 == terminal::request::FIONREAD => {
            let available = i32::try_from(end.available()).unwrap_or(i32::MAX);
            process
                .write_user_memory(argument, &available.to_ne_bytes())
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        FileKind::Event(descriptors::EventFile::Inotify(instance))
            if request as u32 == terminal::request::FIONREAD =>
        {
            let queued = i32::try_from(instance.queued_bytes()).unwrap_or(i32::MAX);
            process
                .write_user_memory(argument, &queued.to_ne_bytes())
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        _ => Err(Errno::ENOTTY),
    }
}

fn lseek(files: &Descriptors, fd: u64, offset: u64, whence: u64) -> Result<u64, Errno> {
    let file = files.file(fd)?;
    match file.kind() {
        FileKind::Console
        | FileKind::Pipe(_)
        | FileKind::Window(_)
        | FileKind::AgentChannel(_)
        | FileKind::Socket(_)
        | FileKind::InetSocket(_)
        | FileKind::Event(_) => terminal::lseek(whence),
        FileKind::MemoryDevice(..) => Ok(0),
        _ => files::lseek_node(&file, offset, whence),
    }
}

struct Iovecs {
    on_stack: [[u8; IOVEC_SIZE]; STACK_IOVECS],
    on_heap: alloc::vec::Vec<[u8; IOVEC_SIZE]>,
    count: usize,
}

impl Iovecs {
    fn one(base: u64, length: u64) -> Self {
        let mut on_stack = [[0; IOVEC_SIZE]; STACK_IOVECS];
        on_stack[0][..8].copy_from_slice(&base.to_ne_bytes());
        on_stack[0][8..].copy_from_slice(&length.to_ne_bytes());
        Self {
            on_stack,
            on_heap: alloc::vec::Vec::new(),
            count: 1,
        }
    }

    fn read<P: Platform>(process: &Process<P>, buffers: u64, count: u64) -> Result<Self, Errno> {
        let count = u64::from(count as u32);
        if count > MAX_IOVECS {
            return Err(Errno::EINVAL);
        }
        let mut iovecs = Self {
            on_stack: [[0; IOVEC_SIZE]; STACK_IOVECS],
            on_heap: alloc::vec::Vec::new(),
            count: count as usize,
        };
        if iovecs.count > STACK_IOVECS {
            iovecs
                .on_heap
                .try_reserve_exact(iovecs.count)
                .map_err(|_| Errno::ENOMEM)?;
            iovecs.on_heap.resize(iovecs.count, [0; IOVEC_SIZE]);
        }
        let array = match iovecs.count > STACK_IOVECS {
            true => iovecs.on_heap.as_flattened_mut(),
            false => iovecs.on_stack[..iovecs.count].as_flattened_mut(),
        };
        if process.read_user_memory(buffers, array).is_err() {
            return Err(unreadable_iovecs_error(process, buffers, count));
        }
        let entries = iovecs.entries();
        if entries.iter().any(|entry| iovec(entry).1 > i64::MAX as u64) {
            return Err(Errno::EINVAL);
        }
        if !entries.iter().all(|entry| {
            let (base, length) = iovec(entry);
            ends_in_user_space::<P>(base, length)
        }) {
            return Err(Errno::EFAULT);
        }
        Ok(iovecs)
    }

    fn entries(&self) -> &[[u8; IOVEC_SIZE]] {
        match self.count > STACK_IOVECS {
            true => &self.on_heap,
            false => &self.on_stack[..self.count],
        }
    }

    fn total(&self) -> u64 {
        self.entries()
            .iter()
            .fold(0, |total: u64, entry| total.saturating_add(iovec(entry).1))
    }
}

/// A `struct iovec`'s base and length.
fn iovec(entry: &[u8; IOVEC_SIZE]) -> (u64, u64) {
    let (base, length) = entry.split_at(8);
    (
        u64::from_ne_bytes(base.try_into().unwrap()),
        u64::from_ne_bytes(length.try_into().unwrap()),
    )
}

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
        crate::console::write::<P>(bytes);
        written += length as u64;
        crate::thread::preemption_point::<P>();
        process.interrupt_here_if_sent_signals_wait();
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
    use eggos_hal::Exception;
    use eggos_hal::paging::Attributes;

    const DATA: u64 = 0x50_0000;

    struct FramesForTest {
        _frames: crate::thread::host::FramesForTest,
    }

    fn frames_for_test() -> FramesForTest {
        FramesForTest {
            _frames: crate::thread::host::frames_for_test(),
        }
    }

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
        use terminal::request;
        let _input = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = frames_for_test();
        crate::console::set_settings::<P>(Termios::SERIAL_CONSOLE, true);
        let termios = Termios::SERIAL_CONSOLE.to_bytes();
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
            assert_eq!(read(36), termios);
            assert_eq!(read(37)[36], 0xff);
        }
        assert_eq!(ioctl(1, request::TCGETS2, writable), 0);
        let settings = read(44);
        assert_eq!(settings[..36], termios);
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
        crate::linux::terminal::set_window_size(49, 158, 8, 16);
        assert_eq!(ioctl(1, request::TIOCGWINSZ, writable), 0);
        assert_eq!(read(8), b"\x31\x00\x9e\x00\xf0\x04\x10\x03");
        crate::linux::terminal::set_window_size(0, 0, 0, 0);

        assert_eq!(ioctl(3, request::TCGETS, writable), Errno::EBADF.result());
        assert_eq!(ioctl(3, 0x5402, writable), Errno::EBADF.result());
        for unknown in [0, 0x5405, 0x540f, 0x5453, request::TCGETS | 0x1_0000] {
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
        for request in [request::TIOCSWINSZ, request::TCSETS, request::TCSETSF2] {
            assert_eq!(ioctl(1, request, 0), Errno::EFAULT.result(), "{request:#x}");
        }
        assert_eq!(crate::console::settings(), Termios::SERIAL_CONSOLE);
        assert_eq!(process.exit_status(), None);
    }

    use crate::console::line_discipline::{Termios, index, local};

    struct ConsoleForTest {
        _frames: FramesForTest,
        _interrupts: std::sync::MutexGuard<'static, ()>,
    }

    impl ConsoleForTest {
        fn take() -> Self {
            let interrupts = crate::interrupt::lock_timer_interrupts_for_test();
            let frames = frames_for_test();
            crate::console::set_settings::<P>(Termios::SERIAL_CONSOLE, true);
            console_output();
            Self {
                _frames: frames,
                _interrupts: interrupts,
            }
        }

        fn types(&self, input: &[u8]) {
            crate::tests::CONSOLE_INPUT.lock().unwrap().extend(input);
            crate::interrupt::handle::<P>(eggos_hal::Interrupt::ConsoleInput);
        }
    }

    impl Drop for ConsoleForTest {
        fn drop(&mut self) {
            crate::console::set_settings::<P>(Termios::SERIAL_CONSOLE, true);
        }
    }

    fn read_bytes(process: &Arc<Process<P>>, address: u64, length: usize) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec![0u8; length];
        process.read_user_memory(address, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn ioctl_changes_flushes_and_counts_the_console_s_input() {
        use terminal::request;
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let ioctl = |request: u32, argument| {
            call(
                &process,
                number::IOCTL,
                [0, u64::from(request), argument, 0, 0, 0],
            )
        };
        let mut raw = Termios::SERIAL_CONSOLE;
        raw.local &= !(local::ICANON | local::ECHO);
        raw.control_characters[index::VMIN] = 0;
        process
            .write_user_memory(writable, &raw.to_bytes())
            .unwrap();
        assert_eq!(ioctl(request::TCSETS, writable), 0);
        assert_eq!(crate::console::settings(), raw);
        assert_eq!(ioctl(request::TCGETS, writable + 64), 0);
        assert_eq!(read_bytes(&process, writable + 64, 36), raw.to_bytes());
        console.types(b"abc");
        assert!(console_output().is_empty());
        assert_eq!(ioctl(request::FIONREAD, writable + 128), 0);
        assert_eq!(read_bytes(&process, writable + 128, 4), 3i32.to_ne_bytes());
        assert_eq!(ioctl(request::TCFLSH, 1), 0);
        assert_eq!(crate::console::pending_input(), 3);
        assert_eq!(ioctl(request::TCFLSH, 3), Errno::EINVAL.result());
        assert_eq!(ioctl(request::TCFLSH, 0), 0);
        assert_eq!(crate::console::pending_input(), 0);
        console.types(b"def");
        let canonical = Termios::SERIAL_CONSOLE.to_bytes();
        process.write_user_memory(writable, &canonical).unwrap();
        assert_eq!(ioctl(request::TCSETSW2, writable), 0);
        assert_eq!(ioctl(request::FIONREAD, writable + 128), 0);
        assert_eq!(read_bytes(&process, writable + 128, 4), 3i32.to_ne_bytes());
        assert_eq!(ioctl(request::TCSETSF, writable), 0);
        assert_eq!(crate::console::pending_input(), 0);
        console.types(b"partial line");
        assert_eq!(ioctl(request::FIONREAD, writable + 128), 0);
        assert_eq!(read_bytes(&process, writable + 128, 4), 0i32.to_ne_bytes());
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn read_returns_typed_lines_one_at_a_time_and_end_of_file_as_zero() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let read = |fd, buffer, count| call(&process, number::READ, [fd, buffer, count, 0, 0, 0]);
        console.types(b"hello\rab\x08c\rwrongx\x7f\x7f\x7f\x7f\x7f\x7fright\r");
        console.types(b"discard me\x15kept\rpartial\x04\x04last\r");
        assert_eq!(read(0, writable, 64), 6);
        assert_eq!(read_bytes(&process, writable, 6), b"hello\n");
        for (fd, line) in [
            (1, &b"ac\n"[..]),
            (2, b"right\n"),
            (0, b"kept\n"),
            (0, b"partial"),
        ] {
            assert_eq!(read(fd, writable, 64), line.len() as u64);
            assert_eq!(read_bytes(&process, writable, line.len()), line);
        }
        assert_eq!(read(0, writable, 64), 0);
        assert_eq!(read(0, writable, 0), 0);
        assert_eq!(read(0, writable, 2), 2);
        assert_eq!(read_bytes(&process, writable, 2), b"la");
        assert_eq!(read(0, writable + 4094, 64), 2);
        assert_eq!(read_bytes(&process, writable + 4094, 2), b"st");
        console.types(b"\r");
        assert_eq!(read(3, writable, 64), Errno::EBADF.result());
        assert_eq!(read(1 << 32 | 3, writable, 64), Errno::EBADF.result());
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(read(0, user_end, 1), Errno::EFAULT.result());
        assert_eq!(read(0, DATA, 64), Errno::EFAULT.result());
        assert!(
            !crate::console::input_ready(),
            "a failed copy loses the line"
        );
        let echo = console_output();
        assert!(echo.starts_with(b"hello\nab\x08 \x08c\nwrongx"), "{echo:?}");
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn a_non_canonical_read_takes_what_vmin_and_vtime_allow() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let read = |count| call(&process, number::READ, [0, writable, count, 0, 0, 0]);
        let mut raw = Termios::SERIAL_CONSOLE;
        raw.local &= !(local::ICANON | local::ECHO);
        raw.control_characters[index::VMIN] = 0;
        crate::console::set_settings::<P>(raw, false);
        assert_eq!(read(8), 0);
        console.types(b"xy\x7fz\r");
        assert_eq!(read(3), 3);
        assert_eq!(read_bytes(&process, writable, 3), b"xy\x7f");
        raw.control_characters[index::VMIN] = 2;
        crate::console::set_settings::<P>(raw, false);
        assert_eq!(read(8), 2);
        assert_eq!(read_bytes(&process, writable, 2), b"z\n");
        raw.control_characters[index::VMIN] = 0;
        raw.control_characters[index::VTIME] = 1;
        crate::console::set_settings::<P>(raw, false);
        assert_eq!(read(8), 0, "VTIME passes with nothing typed");
        assert!(console_output().is_empty());
    }

    #[test]
    fn a_read_with_vmin_and_vtime_returns_vmin_bytes_or_what_came_before_the_gap_timer() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let read = |count| call(&process, number::READ, [0, writable, count, 0, 0, 0]);
        let mut raw = Termios::SERIAL_CONSOLE;
        raw.local &= !(local::ICANON | local::ECHO);
        raw.control_characters[index::VMIN] = 2;
        raw.control_characters[index::VTIME] = 1;
        crate::console::set_settings::<P>(raw, false);
        console.types(b"abc");
        assert_eq!(read(8), 3);
        assert_eq!(read_bytes(&process, writable, 3), b"abc");
        console.types(b"d");
        assert_eq!(read(8), 1, "the gap timer passes after one byte");
        assert_eq!(read_bytes(&process, writable, 1), b"d");
        console.types(b"ef");
        assert_eq!(read(1), 1, "no more than the count");
        assert_eq!(read(8), 1);
    }

    #[test]
    fn a_nonblocking_console_read_fails_with_eagain_until_input_is_there() {
        use descriptors::{command, open_flag};
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let read = |fd, count| call(&process, number::READ, [fd, writable, count, 0, 0, 0]);
        let fcntl = |fd, command: u32, argument: u32| {
            call(
                &process,
                number::FCNTL,
                [fd, u64::from(command), u64::from(argument), 0, 0, 0],
            )
        };
        assert_eq!(fcntl(2, command::SETFL, open_flag::NONBLOCK), 0);
        assert_eq!(
            read(0, 64),
            Errno::EAGAIN.result(),
            "0 to 2 share one description"
        );
        console.types(b"half a line");
        assert_eq!(read(0, 64), Errno::EAGAIN.result());
        console.types(b"\r");
        assert_eq!(read(1, 64), 12);
        assert_eq!(read_bytes(&process, writable, 12), b"half a line\n");
        let mut raw = Termios::SERIAL_CONSOLE;
        raw.local &= !(local::ICANON | local::ECHO);
        raw.control_characters[index::VMIN] = 4;
        crate::console::set_settings::<P>(raw, false);
        assert_eq!(read(0, 64), Errno::EAGAIN.result());
        console.types(b"xy");
        assert_eq!(read(0, 64), 2, "fewer than VMIN bytes, without waiting");
        raw.control_characters[index::VMIN] = 0;
        crate::console::set_settings::<P>(raw, false);
        assert_eq!(read(0, 64), 0, "VMIN and VTIME 0 never wait, so read 0");
        assert_eq!(fcntl(0, command::SETFL, 0), 0);
        assert_eq!(fcntl(1, command::GETFL, 0), u64::from(open_flag::RDWR));
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(
            call(&process, number::IOCTL, [0, 0x5421, user_end, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        process
            .write_user_memory(writable, &1i32.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(&process, number::IOCTL, [1, 0x5421, writable, 0, 0, 0]),
            0
        );
        assert_eq!(read(0, 64), 0);
        assert_eq!(
            fcntl(2, command::GETFL, 0),
            u64::from(open_flag::RDWR | open_flag::NONBLOCK)
        );
        process
            .write_user_memory(writable, &0i32.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call(&process, number::IOCTL, [1, 0x5421, writable, 0, 0, 0]),
            0
        );
        assert_eq!(fcntl(2, command::GETFL, 0), u64::from(open_flag::RDWR));
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn readv_fills_each_buffer_in_turn_with_one_line() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let page = DATA + 4096;
        let iovecs = |buffers: &[(u64, u64)]| {
            let mut array = alloc::vec::Vec::new();
            for &(base, length) in buffers {
                array.extend_from_slice(&base.to_ne_bytes());
                array.extend_from_slice(&length.to_ne_bytes());
            }
            process.write_user_memory(page, &array).unwrap();
        };
        let readv = |fd, count| call(&process, number::READV, [fd, page, count, 0, 0, 0]);
        console.types(b"hello world\rnext\r");
        iovecs(&[(page + 256, 3), (page + 512, 0), (page + 768, 64)]);
        assert_eq!(readv(0, 3), 12);
        assert_eq!(read_bytes(&process, page + 256, 3), b"hel");
        assert_eq!(read_bytes(&process, page + 768, 9), b"lo world\n");
        iovecs(&[(page + 256, 2), (page + 768, 2)]);
        assert_eq!(readv(1, 2), 4);
        assert_eq!(read_bytes(&process, page + 256, 2), b"ne");
        assert_eq!(read_bytes(&process, page + 768, 2), b"xt");
        assert_eq!(readv(1, 2), 1);
        assert_eq!(read_bytes(&process, page + 256, 1), b"\n");
        assert_eq!(readv(0, 0), 0);
        assert_eq!(readv(3, 2), Errno::EBADF.result());
        assert_eq!(readv(0, 1025), Errno::EINVAL.result());
        iovecs(&[(DATA, 8)]);
        console.types(b"lost\r");
        assert_eq!(readv(0, 1), Errno::EFAULT.result());
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn descriptors_are_duplicated_flagged_and_closed_through_the_calls() {
        use descriptors::{FD_CLOEXEC, close_range_flag, command, open_flag};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let writable = DATA + 4096;
        let call = |number, arguments: &[u64]| {
            let mut all = [0; 6];
            all[..arguments.len()].copy_from_slice(arguments);
            call(&process, number, all)
        };
        let fcntl = |fd, command: u32, argument: u64| {
            call(number::FCNTL, &[fd, u64::from(command), argument])
        };
        let cloexec = u64::from(open_flag::CLOEXEC);
        assert_eq!(call(number::CLOSE, &[1]), 0);
        assert_eq!(call(number::CLOSE, &[1]), Errno::EBADF.result());
        assert_eq!(call(number::WRITE, &[1, DATA, 1]), Errno::EBADF.result());
        assert_eq!(call(number::DUP, &[2]), 1);
        assert_eq!(call(number::DUP, &[7]), Errno::EBADF.result());
        assert_eq!(call(number::DUP3, &[0, 5, cloexec]), 5);
        assert_eq!(fcntl(5, command::GETFD, 0), u64::from(FD_CLOEXEC));
        assert_eq!(fcntl(0, command::GETFD, 0), 0);
        assert_eq!(call(number::DUP3, &[0, 0, 0]), Errno::EINVAL.result());
        assert_eq!(call(number::DUP3, &[0, 6, 1]), Errno::EINVAL.result());
        assert_eq!(call(number::DUP3, &[9, 6, 0]), Errno::EBADF.result());
        assert_eq!(call(number::DUP3, &[0, 1024, 0]), Errno::EBADF.result());
        assert_eq!(call(number::DUP3, &[1, 5, 0]), 5);
        assert_eq!(fcntl(5, command::GETFD, 0), 0);
        assert_eq!(call(number::DUP, &[0]), 3);
        assert_eq!(fcntl(0, command::DUPFD, 3), 4);
        assert_eq!(fcntl(0, command::DUPFD_CLOEXEC, 10), 10);
        assert_eq!(fcntl(10, command::GETFD, 0), u64::from(FD_CLOEXEC));
        assert_eq!(fcntl(10, command::SETFD, 0xfe), 0);
        assert_eq!(fcntl(10, command::GETFD, 0), 0);
        assert_eq!(fcntl(0, command::DUPFD, 1024), Errno::EINVAL.result());
        assert_eq!(fcntl(0, command::DUPFD, u64::MAX), Errno::EINVAL.result());
        assert_eq!(
            fcntl(0, command::DUPFD, 1 << 32 | 12),
            12,
            "an int argument"
        );
        assert_eq!(call(number::CLOSE, &[12]), 0);
        assert_eq!(fcntl(10, command::SETFD, 1 << 32), 0);
        assert_eq!(fcntl(10, command::GETFD, 0), 0);
        assert_eq!(fcntl(11, command::DUPFD, 1024), Errno::EBADF.result());
        assert_eq!(fcntl(0, 1031, 0), Errno::EINVAL.result());
        assert_eq!(fcntl(11, 1031, 0), Errno::EBADF.result());
        assert_eq!(fcntl(3, command::GETFL, 0), u64::from(open_flag::RDWR));
        let settable = open_flag::APPEND | open_flag::NONBLOCK;
        assert_eq!(
            fcntl(3, command::SETFL, u64::from(settable | open_flag::WRONLY)),
            0
        );
        assert_eq!(
            fcntl(0, command::GETFL, 0),
            u64::from(open_flag::RDWR | settable)
        );
        assert_eq!(fcntl(4, command::SETFL, 0), 0);
        assert_eq!(call(number::IOCTL, &[4, 0x5451]), 0);
        assert_eq!(fcntl(4, command::GETFD, 0), u64::from(FD_CLOEXEC));
        assert_eq!(call(number::IOCTL, &[4, 0x5450]), 0);
        assert_eq!(fcntl(4, command::GETFD, 0), 0);
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        assert_eq!(call(number::IOCTL, &[4, 0x5452, writable]), 0);
        assert_eq!(
            call(number::IOCTL, &[4, 0x5452, user_end]),
            Errno::EFAULT.result()
        );
        assert_eq!(
            call(number::IOCTL, &[11, 0x5452, writable]),
            Errno::EBADF.result()
        );
        assert_eq!(fcntl(4, command::GETFL, 0), u64::from(open_flag::RDWR));
        let range =
            |first, last, flags: u32| call(number::CLOSE_RANGE, &[first, last, u64::from(flags)]);
        assert_eq!(range(4, 3, 0), Errno::EINVAL.result());
        assert_eq!(range(0, 9, 1), Errno::EINVAL.result());
        assert_eq!(range(4, 10, close_range_flag::CLOEXEC), 0);
        assert_eq!(fcntl(4, command::GETFD, 0), u64::from(FD_CLOEXEC));
        assert_eq!(fcntl(10, command::GETFD, 0), u64::from(FD_CLOEXEC));
        assert_eq!(fcntl(3, command::GETFD, 0), 0);
        assert_eq!(range(4, u64::from(u32::MAX), close_range_flag::UNSHARE), 0);
        assert_eq!(fcntl(4, command::GETFD, 0), Errno::EBADF.result());
        assert_eq!(fcntl(10, command::GETFD, 0), Errno::EBADF.result());
        assert_eq!(call(number::DUP, &[0]), 4);
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(call(number::DUP2, &[4, 4]), 4);
            assert_eq!(call(number::DUP2, &[8, 8]), Errno::EBADF.result());
            assert_eq!(call(number::DUP3, &[0, 8, cloexec]), 8);
            assert_eq!(call(number::DUP2, &[0, 8]), 8);
            assert_eq!(fcntl(8, command::GETFD, 0), 0);
            assert_eq!(call(number::DUP2, &[0, 1024]), Errno::EBADF.result());
        }
        assert_eq!(call(number::CLOSE, &[0]), 0);
        assert_eq!(call(number::READ, &[0, writable, 8]), Errno::EBADF.result());
        assert_eq!(call(number::FSTAT, &[0, writable]), Errno::EBADF.result());
        assert_eq!(call(number::LSEEK, &[0, 0, 0]), Errno::EBADF.result());
        assert_eq!(
            call(number::IOCTL, &[0, 0x5401, writable]),
            Errno::EBADF.result()
        );
        assert_eq!(call(number::FSTAT, &[3, writable]), 0);
        assert_eq!(call(number::LSEEK, &[3, 0, 0]), Errno::ESPIPE.result());
        assert_eq!(call(number::IOCTL, &[3, 0x5401, writable]), 0);
        process
            .write_user_memory(writable, &[3u64.to_ne_bytes(), 4u64.to_ne_bytes()].concat())
            .unwrap();
        assert_eq!(call(number::SETRLIMIT, &[7, writable]), 0);
        assert_eq!(call(number::DUP, &[3]), 0);
        assert_eq!(call(number::DUP, &[3]), Errno::EMFILE.result());
        assert_eq!(fcntl(3, command::DUPFD, 3), Errno::EINVAL.result());
        assert_eq!(call(number::DUP3, &[0, 3, 0]), Errno::EBADF.result());
        assert_eq!(call(number::DUP3, &[3, 1, 0]), 1);
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn a_signal_interrupts_a_console_read_restarted_only_with_sa_restart() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let sigusr1 = 10;
        set_handler(&process, sigusr1, caught_with(0), 0);
        let mut send = first_thread_and_sender(&process, sigusr1);
        let buffer = SCRATCH + 0x100;
        let call_size = if cfg!(target_arch = "x86_64") { 2 } else { 4 };
        for restart in [0, signals::flag::RESTART] {
            set_handler(&process, sigusr1, caught_with(restart), 0);
            send();
            let arguments = [0, buffer, 64, 0, 0, 0];
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            context.simulate_syscall(number::READ, arguments, 0x40_0004);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            if restart != 0 {
                assert_eq!(context.pc(), 0x40_0004 - call_size);
                assert_eq!(context.syscall_number(), number::READ);
                assert_eq!(context.syscall_arguments(), arguments);
            } else {
                assert_eq!(context.pc(), 0x40_0004);
                assert_eq!(context.syscall_result(), Errno::EINTR.result());
            }
        }
        console.types(b"line\r");
        send();
        let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
        context.simulate_syscall(number::READ, [0, buffer, 64, 0, 0, 0], 0x40_0004);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(
            context.pc(),
            HANDLER,
            "the signal is delivered with the line"
        );
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!((context.pc(), context.syscall_result()), (0x40_0004, 5));
        assert_eq!(read_bytes(&process, buffer, 5), b"line\n");
        assert_eq!(process.exit_status(), None);
    }

    fn register_running(process: &Arc<Process<P>>) {
        let thread = take_first_thread(process);
        wait::record_running(process, &thread, |_, _| {});
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(process.id().get(), thread);
    }

    #[test]
    fn an_interrupt_typed_signals_the_console_s_foreground_group_and_its_leader_s_end_frees_it() {
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let id = process.id().get();
        let writable = DATA + 4096;
        register_running(&process);
        process_table::TABLE.lock().take_console(id, true).unwrap();
        assert_eq!(
            call(&process, number::IOCTL, [0, 0x540f, writable, 0, 0, 0]),
            0
        );
        assert_eq!(read_bytes(&process, writable, 4), id.to_ne_bytes());
        assert_eq!(
            call(&process, number::IOCTL, [0, 0x5429, writable, 0, 0, 0]),
            0
        );
        assert_eq!(read_bytes(&process, writable, 4), id.to_ne_bytes());
        console.types(b"first\r");
        assert_eq!(call(&process, number::READ, [0, writable, 64, 0, 0, 0]), 6);
        console.types(b"gone\x03");
        assert_eq!(console_output(), b"first\ngone^C");
        let mut context = called(number::READ, [0, writable, 64, 0, 0, 0]);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed {
                signal: signal::SIGINT
            })
        );
        assert_eq!(
            process_table::TABLE.lock().controlling_terminal(id),
            Err(Errno::ENOTTY)
        );
    }

    #[test]
    fn ppoll_reports_console_input_once_a_line_is_typed() {
        use poll::event::{IN, OUT, RDNORM};
        let console = ConsoleForTest::take();
        let kernel = kernel_tables();
        let process = process_with_iovecs(&kernel, b"", &[]);
        let (fds, timeout) = (DATA + 4096, DATA + 4096 + 64);
        process.write_user_memory(timeout, &[0; 16]).unwrap();
        let entries = pollfds(&[(0, IN | RDNORM), (1, OUT | IN)]);
        let ppoll = || {
            process.write_user_memory(fds, &entries).unwrap();
            let ready = call(&process, number::PPOLL, [fds, 2, timeout, 0, 0, 0]);
            let revents = |entry: u64| {
                u16::from_ne_bytes(
                    read_bytes(&process, fds + 8 * entry + 6, 2)
                        .try_into()
                        .unwrap(),
                )
            };
            (ready, revents(0), revents(1))
        };
        assert_eq!(ppoll(), (1, 0, OUT));
        console.types(b"not yet");
        assert_eq!(ppoll(), (1, 0, OUT));
        console.types(b"\r");
        assert_eq!(ppoll(), (2, IN | RDNORM, OUT | IN));
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
        for unknown in [0x3001, 0x5001, 0x1023, 0] {
            assert_eq!(prctl(unknown, 0x7000_2000), Err(Errno::EINVAL));
        }
        let set_gs = u64::from(code::ARCH_SET_GS);
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(prctl(set_gs, end), Err(Errno::EPERM));
            assert_eq!(prctl(1 << 32 | set_gs, 0x7000_3000), Ok(0));
            assert_eq!(prctl(get_gs, DATA + 4096 + 32), Ok(0));
            let mut word = [0u8; 8];
            process
                .read_user_memory(DATA + 4096 + 32, &mut word)
                .unwrap();
            assert_eq!(word, 0x7000_3000u64.to_ne_bytes());
        }
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(prctl(set_gs, 0x7000_2000), Err(Errno::EINVAL));
        #[cfg(target_arch = "x86_64")]
        assert_eq!(context.gs_base(), 0x7000_3000);
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
        let no_process = u64::from(i32::MAX as u32);
        for target in [no_process, group_of_none, no_process.wrapping_neg()] {
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
    fn only_a_privileged_process_sets_the_wall_clock_and_the_checks_come_first_as_on_linux() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (time, zone, tp) = (SCRATCH + 0x100, SCRATCH + 0x110, SCRATCH + 0x120);
        process.write_user_memory(zone, &[0; 8]).unwrap();
        let settime =
            |clock: u64, tp: u64| call(&process, number::CLOCK_SETTIME, [clock, tp, 0, 0, 0, 0]);
        let settimeofday =
            |tv: u64, tz: u64| call(&process, number::SETTIMEOFDAY, [tv, tz, 0, 0, 0, 0]);
        let wall = || {
            assert_eq!(
                call(&process, number::CLOCK_GETTIME, [0, tp, 0, 0, 0, 0]),
                0
            );
            nanoseconds(read_timespec(&process, tp))
        };
        write_timespec(&process, time, 1, 0);
        assert_eq!(settime(1, time), Errno::EINVAL.result());
        assert_eq!(settime(0, UNMAPPED), Errno::EFAULT.result());
        for seconds in [-1, 8_277_292_036, i64::MAX] {
            write_timespec(&process, time, seconds, 0);
            assert_eq!(settime(0, time), Errno::EINVAL.result());
        }
        write_timespec(&process, time, 1_793_282_709, 5);
        assert_eq!(settime(0, time), 0);
        let set = 1_793_282_709_000_000_005;
        let read = wall();
        assert!(read >= set && read - set < 1_000_000_000, "{read}");
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
        write_timespec(&process, time, 1_893_456_000, 250_000);
        assert_eq!(settimeofday(time, zone), 0);
        let set = 1_893_456_000_250_000_000;
        let read = wall();
        assert!(read >= set && read - set < 1_000_000_000, "{read}");
        assert_eq!(settimeofday(0, 0), 0);
        assert_eq!(call(&process, number::SETUID, [1000, 0, 0, 0, 0, 0]), 0);
        write_timespec(&process, time, 1, 0);
        assert_eq!(settime(0, time), Errno::EPERM.result());
        assert_eq!(settimeofday(time, 0), Errno::EPERM.result());
        assert_eq!(settimeofday(0, zone), Errno::EPERM.result());
        assert_eq!(settimeofday(0, 0), Errno::EPERM.result());
        write_timespec(&process, time, -1, 0);
        assert_eq!(settime(0, time), Errno::EINVAL.result());
        assert!(wall() >= set);
        crate::wall_clock::host::reset();
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
        assert_eq!(
            fstatat(fdcwd, EMPTY, UNMAPPED, empty),
            Errno::EFAULT.result()
        );
        assert_eq!(fstatat(fdcwd, EMPTY, STAT_BUFFER, empty), 0);
        assert_eq!(statx(fdcwd, 0, empty, 0x7ff, STATX_BUFFER), 0);
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
        let longest: alloc::vec::Vec<u8> = b"a/".iter().copied().cycle().take(4095).collect();
        process.write_user_memory(lowest + 1, &longest).unwrap();
        process.write_user_memory(lowest + 4096, &[0]).unwrap();
        assert_eq!(fstatat(lowest + 1), Errno::ENOENT.result());
        process.write_user_memory(lowest + 4096, b"a\0").unwrap();
        assert_eq!(fstatat(lowest + 1), Errno::ENAMETOOLONG.result());
        assert_eq!(fstatat(0xffff_ffff_ffff_fff0), Errno::EFAULT.result());
    }

    const PATH: u64 = SCRATCH + 0x700;
    const READ_BUFFER: u64 = SCRATCH + 0x800;
    const FDCWD: u64 = stat::at::FDCWD as u32 as u64;

    fn at_path(process: &Arc<Process<P>>, path: &[u8]) -> u64 {
        let mut bytes = path.to_vec();
        bytes.push(0);
        process.write_user_memory(PATH, &bytes).unwrap();
        PATH
    }

    fn open_at(process: &Arc<Process<P>>, dirfd: u64, path: &[u8], flags: u32) -> u64 {
        let path = at_path(process, path);
        call(
            process,
            number::OPENAT,
            [dirfd, path, u64::from(flags), 0, 0, 0],
        )
    }

    fn read_into(
        process: &Arc<Process<P>>,
        fd: u64,
        count: u64,
    ) -> Result<alloc::vec::Vec<u8>, u64> {
        let read = call(process, number::READ, [fd, READ_BUFFER, count, 0, 0, 0]);
        match read > MAX_RW_COUNT {
            true => Err(read),
            false => Ok(read_bytes(process, READ_BUFFER, read as usize)),
        }
    }

    #[test]
    fn files_are_opened_read_and_seeked_through_the_calls() {
        use descriptors::open_flag;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let fd = open_at(&process, FDCWD, b"/etc/hostname", open_flag::CLOEXEC);
        assert_eq!(fd, 3);
        assert_eq!(call(&process, number::FCNTL, [fd, 1, 0, 0, 0, 0]), 1);
        assert_eq!(read_into(&process, fd, 4), Ok(b"eggo".to_vec()));
        assert_eq!(read_into(&process, fd, 100), Ok(b"s\n".to_vec()));
        assert_eq!(read_into(&process, fd, 100), Ok(alloc::vec::Vec::new()));
        let lseek = |offset: i64, whence| {
            call(
                &process,
                number::LSEEK,
                [fd, offset as u64, whence, 0, 0, 0],
            )
        };
        assert_eq!(lseek(1, 0), 1);
        assert_eq!(read_into(&process, fd, 3), Ok(b"ggo".to_vec()));
        assert_eq!(lseek(-1, 2), 5);
        assert_eq!(lseek(-10, 1), Errno::EINVAL.result());
        assert_eq!(lseek(0, 1), 5);
        assert_eq!(lseek(100, 0), 100);
        assert_eq!(read_into(&process, fd, 3), Ok(alloc::vec::Vec::new()));
        assert_eq!(lseek(6, 3), Errno::ENXIO.result());
        assert_eq!(lseek(2, 3), 2);
        assert_eq!(lseek(0, 4), 6);
        assert_eq!(lseek(0, 5), Errno::EINVAL.result());
        let pread = |offset: i64| {
            call(
                &process,
                number::PREAD64,
                [fd, READ_BUFFER, 3, offset as u64, 0, 0],
            )
        };
        assert_eq!(pread(2), 3);
        assert_eq!(read_bytes(&process, READ_BUFFER, 3), b"gos");
        assert_eq!(lseek(0, 1), 6);
        assert_eq!(pread(-1), Errno::EINVAL.result());
        assert_eq!(
            call(&process, number::PREAD64, [1, READ_BUFFER, 3, 0, 0, 0]),
            Errno::ESPIPE.result()
        );
        let duplicate = call(&process, number::DUP, [fd, 0, 0, 0, 0, 0]);
        assert_eq!(lseek(1, 0), 1);
        assert_eq!(read_into(&process, duplicate, 2), Ok(b"gg".to_vec()));
        assert_eq!(lseek(0, 1), 3);
        assert_eq!(
            call(&process, number::READ, [fd, UNMAPPED, 3, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(
            call(&process, number::WRITE, [fd, READ_BUFFER, 1, 0, 0, 0]),
            Errno::EBADF.result()
        );
        let status = call(&process, number::FSTAT, [fd, STAT_BUFFER, 0, 0, 0, 0]);
        assert_eq!(status, 0);
        let bytes: [u8; stat::STAT_SIZE] = user_bytes(&process, STAT_BUFFER);
        let mode_offset = if cfg!(target_arch = "x86_64") { 24 } else { 16 };
        assert_eq!(word(&bytes, mode_offset), stat::S_IFREG | 0o644);
        assert_eq!(double_word(&bytes, 48), 6);
        assert_eq!(double_word(&bytes, 88), 1_700_000_000);
        for fd in [fd, duplicate] {
            assert_eq!(call(&process, number::CLOSE, [fd, 0, 0, 0, 0, 0]), 0);
        }
    }

    const ARGUMENT_LIST: u64 = SCRATCH + 0x100;
    const ARGUMENT_STRINGS: u64 = SCRATCH + 0x200;

    fn argument_list(process: &Arc<Process<P>>, strings: &[&[u8]]) -> u64 {
        let mut at = ARGUMENT_STRINGS;
        for (index, string) in strings.iter().enumerate() {
            let mut bytes = string.to_vec();
            bytes.push(0);
            process.write_user_memory(at, &bytes).unwrap();
            process
                .write_user_memory(ARGUMENT_LIST + 8 * index as u64, &at.to_ne_bytes())
                .unwrap();
            at += bytes.len() as u64;
        }
        process
            .write_user_memory(ARGUMENT_LIST + 8 * strings.len() as u64, &[0; 8])
            .unwrap();
        ARGUMENT_LIST
    }

    #[test]
    fn execve_refuses_in_linux_s_order_and_leaves_the_caller_as_it_was() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let execve = |path: &[u8], arguments: u64| {
            let path = at_path(&process, path);
            call(&process, number::EXECVE, [path, arguments, 0, 0, 0, 0])
        };
        for (path, error) in [
            (&b""[..], Errno::ENOENT),
            (b"/missing", Errno::ENOENT),
            (b"/etc", Errno::EACCES),
            (b"/etc/hostname", Errno::EACCES),
            (b"/etc/hostname/program", Errno::ENOTDIR),
            (b"/usr/libexec/script", Errno::ENOENT),
            (b"/usr/libexec/bare", Errno::ENOEXEC),
            (b"/usr/libexec/loop", Errno::ELOOP),
            (b"/usr/libexec/to-text", Errno::EACCES),
            (b"/usr/libexec/text", Errno::ENOEXEC),
        ] {
            assert_eq!(execve(path, 0), error.result(), "{path:?}");
        }
        assert_eq!(
            execve(b"/usr/libexec/program", UNMAPPED),
            Errno::EFAULT.result()
        );
        let arguments = argument_list(&process, &[b"program"]);
        assert_eq!(
            call(
                &process,
                number::EXECVEAT,
                [
                    FDCWD,
                    at_path(&process, b"/usr/libexec/program"),
                    arguments,
                    0,
                    1,
                    0
                ]
            ),
            Errno::EINVAL.result()
        );
        let empty = at_path(&process, b"");
        assert_eq!(
            call(&process, number::EXECVEAT, [FDCWD, empty, 0, 0, 1, 0]),
            Errno::ENOENT.result()
        );
        assert_eq!(
            call(&process, number::EXECVEAT, [FDCWD, UNMAPPED, 0, 0, 1, 0]),
            Errno::EFAULT.result()
        );
        assert!(!process.has_ended());
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn execve_starts_the_program_in_a_new_process_with_the_caller_s_id() {
        use descriptors::open_flag::*;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let closed_on_exec = open_at(&process, FDCWD, b"/etc/hostname", CLOEXEC);
        let kept = open_at(&process, FDCWD, b"/etc/motd", RDONLY);
        let directory = open_at(&process, FDCWD, b"/usr/libexec", DIRECTORY);
        let arguments = argument_list(&process, &[b"program", b"argument"]);
        let path = at_path(&process, b"program");
        let mut thread = take_first_thread(&process);
        let mut context = called(number::EXECVEAT, [directory, path, arguments, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        let other = UserContext::new(0, 0);
        assert!(process.take_handed_over(&other).is_none());
        let current = process.take_handed_over(&context).unwrap();
        assert!(process.take_handed_over(&context).is_none());
        assert_eq!(current.id(), process.id());
        assert_eq!(thread.id(), process.id().get());
        assert!(process.has_ended() && !current.has_ended());
        assert_eq!(current.threads(), 1);
        assert_eq!(
            thread.files().file(closed_on_exec).err(),
            Some(Errno::EBADF)
        );
        assert!(thread.files().file(kept).is_ok());
        let mut argc = [0u8; 8];
        current
            .read_user_memory(context.stack(), &mut argc)
            .unwrap();
        assert_eq!(u64::from_ne_bytes(argc), 2);
        current.exit(ProcessExit::Exited(7));
        assert_eq!(process.exit_status(), Some(ProcessExit::Exited(7)));
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(current.id().get(), thread);
    }

    fn exec_in(
        process: &Arc<Process<P>>,
        thread: &mut LinuxThread,
        number: u64,
        arguments: [u64; 6],
    ) -> (UserContext, Arc<Process<P>>) {
        let mut context = called(number, arguments);
        assert_eq!(
            handle(process, thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(()),
            "{:#x}",
            context.syscall_result()
        );
        let current = process.take_handed_over(&context).unwrap();
        (context, current)
    }

    #[test]
    fn a_script_runs_its_interpreter_with_the_line_s_argument_and_its_own_path() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let path = at_path(&process, b"/usr/libexec/runs");
        let arguments = argument_list(&process, &[b"runs", b"arg"]);
        let mut thread = take_first_thread(&process);
        let (_, current) = exec_in(
            &process,
            &mut thread,
            number::EXECVE,
            [path, arguments, 0, 0, 0, 0],
        );
        let (start, end) = current.image_layout().arguments;
        let mut strings = alloc::vec![0u8; (end - start) as usize];
        current.read_user_memory(start, &mut strings).unwrap();
        assert_eq!(
            strings,
            b"/usr/libexec/program\0-x\0/usr/libexec/runs\0arg\0"
        );
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(current.id().get(), thread);
    }

    #[test]
    fn execve_makes_the_saved_and_file_system_ids_the_effective_ones() {
        use credentials::{Ids, Kind};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let set = |kind, ids| {
            let mut locked = process.credentials_and_limits().lock();
            locked.credentials.set_ids(kind, ids).unwrap();
        };
        let user = Ids {
            real: 1000,
            effective: 1000,
            saved: 0,
            filesystem: 1000,
        };
        set(Kind::User, user);
        set(
            Kind::Group,
            Ids {
                saved: 0,
                ..Ids::all(100)
            },
        );
        let path = at_path(&process, b"/usr/libexec/program");
        let mut thread = take_first_thread(&process);
        let (_, current) = exec_in(&process, &mut thread, number::EXECVE, [path, 0, 0, 0, 0, 0]);
        let locked = current.credentials_and_limits().lock();
        assert_eq!(locked.credentials.ids(Kind::User), Ids::all(1000));
        assert_eq!(locked.credentials.ids(Kind::Group), Ids::all(100));
        drop(locked);
        assert_eq!(
            process
                .credentials_and_limits()
                .lock()
                .credentials
                .ids(Kind::User),
            user
        );
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(current.id().get(), thread);
    }

    #[test]
    fn execve_keeps_the_process_s_place_in_the_table_and_frees_the_old_program() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let id = process.id().get();
        let mut thread = take_first_thread(&process);
        wait::record_running(&process, &thread, |_, _| {});
        process_table::TABLE.lock().set_child_subreaper(id, true);
        let path = at_path(&process, b"/usr/libexec/program");
        let (_, current) = exec_in(&process, &mut thread, number::EXECVE, [path, 0, 0, 0, 0, 0]);
        assert_eq!(Arc::strong_count(&process), 1);
        let table = process_table::TABLE.lock();
        assert_eq!(table.group_and_session(id), Some((id, id)));
        assert!(table.is_child_subreaper(id));
        assert_eq!(
            table.members(id)[0].process.upgrade().map(|image| image
                .downcast::<Process<P>>()
                .is_ok_and(|image| Arc::ptr_eq(&image, &current))),
            Some(true)
        );
    }

    #[test]
    fn execve_from_another_thread_gives_it_the_process_s_id_and_a_table_of_its_own() {
        use descriptors::open_flag::*;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let program = open_at(&process, FDCWD, b"/usr/libexec/program", PATH);
        let first = take_first_thread(&process);
        let mut second = first.new_thread(
            process.id().get() + 1_000_000,
            0,
            first.files().share(),
            first.working_directory().share(),
        );
        process.add_thread();
        assert!(second.files().is_shared());
        const CROSSING: u64 = SCRATCH + FRAME_SIZE - 3;
        process.write_user_memory(CROSSING, b"crossing\0").unwrap();
        process
            .write_user_memory(ARGUMENT_LIST, &CROSSING.to_ne_bytes())
            .unwrap();
        process
            .write_user_memory(ARGUMENT_LIST + 8, &[0; 8])
            .unwrap();
        let empty = at_path(&process, b"");
        let (context, current) = exec_in(
            &process,
            &mut second,
            number::EXECVEAT,
            [program, empty, ARGUMENT_LIST, 0, stat::at::EMPTY_PATH, 0],
        );
        assert_eq!(second.id(), process.id().get());
        assert!(!second.files().is_shared());
        assert!(first.files().file(program).is_ok());
        let mut pointer = [0u8; 8];
        current
            .read_user_memory(context.stack() + 8, &mut pointer)
            .unwrap();
        let mut argument = [0u8; 9];
        current
            .read_user_memory(u64::from_ne_bytes(pointer), &mut argument)
            .unwrap();
        assert_eq!(&argument, b"crossing\0");
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(current.id().get(), second);
    }

    #[test]
    fn execve_loses_to_an_exit_that_came_first_and_no_later_exit_records_anything() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let exited = process_for_status(&kernel);
        assert!(exited.exit(ProcessExit::Exited(3)));
        assert!(!exited.end_for_exec());
        assert_eq!(exited.exit_status(), Some(ProcessExit::Exited(3)));
        let path = at_path(&exited, b"/usr/libexec/program");
        let mut context = called(number::EXECVE, [path, 0, 0, 0, 0, 0]);
        let mut thread = take_first_thread(&exited);
        assert_eq!(
            handle(&exited, &mut thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert!(exited.take_handed_over(&context).is_none());
        let replaced = process_for_status(&kernel);
        assert!(replaced.end_for_exec());
        assert!(!replaced.end_for_exec());
        assert!(!replaced.exit(ProcessExit::Exited(4)));
        assert_eq!(
            end_process::<P, ()>(&replaced, ProcessExit::Killed { signal: 9 }),
            ControlFlow::Break(())
        );
        assert_eq!(replaced.exit_status(), None);
    }

    #[test]
    fn opening_fails_as_on_a_read_only_linux_mount() {
        use descriptors::open_flag::*;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let open = |path: &[u8], flags| open_at(&process, FDCWD, path, flags);
        for (path, flags, error) in [
            (&b""[..], RDONLY, Errno::ENOENT),
            (b"/etc/missing", RDONLY, Errno::ENOENT),
            (b"/missing/x", CREAT, Errno::ENOENT),
            (b"/etc/hostname/", RDONLY, Errno::ENOTDIR),
            (b"/etc/hostname/x", CREAT, Errno::ENOTDIR),
            (b"/etc/hostname", WRONLY, Errno::EROFS),
            (b"/etc/hostname", RDWR, Errno::EROFS),
            (b"/etc/hostname", TRUNC, Errno::EROFS),
            (b"/etc/new", CREAT | WRONLY, Errno::EROFS),
            (b"/etc/new/", CREAT, Errno::EISDIR),
            (b"/etc/hostname", CREAT | EXCL, Errno::EEXIST),
            (b"/etc/hostname", DIRECTORY, Errno::ENOTDIR),
            (b"/etc", WRONLY, Errno::EISDIR),
            (b"/etc", CREAT, Errno::EISDIR),
            (b"/etc", CREAT | DIRECTORY, Errno::EINVAL),
            (b"/etc", TMPFILE, Errno::EINVAL),
            (b"/etc", TMPFILE | RDWR, Errno::EROFS),
            (b"/etc", (TMPFILE & !DIRECTORY) | RDWR, Errno::EINVAL),
            (b"/etc", RDONLY | TRUNC, Errno::EISDIR),
            (b"/missing/.", CREAT, Errno::ENOENT),
            (b"/missing/x/", CREAT, Errno::ENOENT),
            (b"/etc/hostname/", CREAT, Errno::ENOTDIR),
        ] {
            assert_eq!(open(path, flags), error.result(), "{path:?} {flags:#o}");
        }
        let mut long = b"/etc/".to_vec();
        long.extend_from_slice(&[b'n'; 256]);
        assert_eq!(open(&long, RDONLY), Errno::ENAMETOOLONG.result());
        assert_eq!(open(&long, CREAT), Errno::ENAMETOOLONG.result());
        assert_eq!(
            call(&process, number::OPENAT, [FDCWD, UNMAPPED, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(open(b"/etc/hostname", CREAT | NOFOLLOW), 3);
        assert_eq!(open(b"/etc/", DIRECTORY), 4);
        assert_eq!(read_into(&process, 4, 10), Err(Errno::EISDIR.result()));
        let only_path = open(b"/etc/motd", PATH | WRONLY | TRUNC);
        assert_eq!(only_path, 5);
        const FIOCLEX: u64 = 0x5451;
        const F_GETFL: u64 = 3;
        const F_SETFL: u64 = 4;
        const F_GETLK: u64 = 5;
        for (number, arguments, result) in [
            (
                number::IOCTL,
                [only_path, FIOCLEX, 0],
                Errno::EBADF.result(),
            ),
            (
                number::FCNTL,
                [only_path, F_SETFL, 0],
                Errno::EBADF.result(),
            ),
            (
                number::FCNTL,
                [only_path, F_GETLK, 0],
                Errno::EBADF.result(),
            ),
            (number::FCNTL, [only_path, F_GETFL, 0], u64::from(PATH)),
        ] {
            let [first, second, third] = arguments;
            assert_eq!(
                call(&process, number, [first, second, third, 0, 0, 0]),
                result,
                "{number} {second}"
            );
        }
        assert_eq!(
            read_into(&process, only_path, 10),
            Err(Errno::EBADF.result())
        );
        assert_eq!(
            call(&process, number::LSEEK, [only_path, 0, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
        assert_eq!(
            call(
                &process,
                number::FSTAT,
                [only_path, STAT_BUFFER, 0, 0, 0, 0]
            ),
            0
        );
        assert_eq!(open_at(&process, 4, b"motd", RDONLY), 6);
        assert_eq!(
            open_at(&process, only_path, b"x", RDONLY),
            Errno::ENOTDIR.result()
        );
        assert_eq!(
            open_at(&process, 1, b"motd", RDONLY),
            Errno::ENOTDIR.result()
        );
        assert_eq!(
            open_at(&process, 99, b"motd", RDONLY),
            Errno::EBADF.result()
        );
        assert_eq!(open_at(&process, 99, b"/etc/motd", RDONLY), 7);
        let directory_path = open(b"/usr", PATH | DIRECTORY);
        assert_eq!(
            open_at(&process, directory_path, b"share/eggos/about", RDONLY),
            9
        );
    }

    fn dirents(bytes: &[u8]) -> alloc::vec::Vec<(alloc::vec::Vec<u8>, u8, u64)> {
        let mut entries = alloc::vec::Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let length = u16::from_ne_bytes([bytes[at + 16], bytes[at + 17]]) as usize;
            let name = &bytes[at + 19..at + length];
            let name = &name[..name.iter().position(|&byte| byte == 0).unwrap()];
            assert_eq!(length % 8, 0);
            entries.push((name.to_vec(), bytes[at + 18], double_word(bytes, at + 8)));
            at += length;
        }
        entries
    }

    #[test]
    fn directories_are_listed_in_records_resumed_from_the_offset() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let fd = open_at(&process, FDCWD, b"/etc", descriptors::open_flag::DIRECTORY);
        let getdents =
            |fd, buffer, count| call(&process, number::GETDENTS64, [fd, buffer, count, 0, 0, 0]);
        let all = getdents(fd, READ_BUFFER, 1024);
        let entries = dirents(&read_bytes(&process, READ_BUFFER, all as usize));
        let expected: [(&[u8], u8, u64); 4] = [
            (b".", 4, 1),
            (b"..", 4, 2),
            (b"hostname", 8, 3),
            (b"motd", 8, 4),
        ];
        assert_eq!(entries.len(), 4);
        for ((name, kind, next), (expected_name, expected_kind, expected_next)) in
            entries.iter().zip(expected)
        {
            assert_eq!(
                (&name[..], *kind, *next),
                (expected_name, expected_kind, expected_next)
            );
        }
        assert_eq!(getdents(fd, READ_BUFFER, 1024), 0);
        assert_eq!(call(&process, number::LSEEK, [fd, 0, 0, 0, 0, 0]), 0);
        assert_eq!(getdents(fd, READ_BUFFER, 24), 24);
        assert_eq!(getdents(fd, READ_BUFFER, 24), 24);
        assert_eq!(getdents(fd, READ_BUFFER, 31), Errno::EINVAL.result());
        assert_eq!(getdents(fd, READ_BUFFER, 1 << 32 | 32), 32);
        assert_eq!(
            dirents(&read_bytes(&process, READ_BUFFER, 32))[0].0,
            b"hostname"
        );
        assert_eq!(getdents(fd, READ_BUFFER, 1024), 24);
        assert_eq!(getdents(fd, READ_BUFFER, 1024), 0);
        assert_eq!(
            call(&process, number::LSEEK, [fd, 2, 2, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(call(&process, number::LSEEK, [fd, 0, 0, 0, 0, 0]), 0);
        assert_eq!(getdents(fd, UNMAPPED, 64), Errno::EFAULT.result());
        assert_eq!(getdents(fd, u64::MAX - 8, 64), Errno::EFAULT.result());
        let file = open_at(&process, FDCWD, b"/etc/motd", 0);
        assert_eq!(getdents(file, READ_BUFFER, 1024), Errno::ENOTDIR.result());
        assert_eq!(getdents(1, READ_BUFFER, 1024), Errno::ENOTDIR.result());
        assert_eq!(getdents(99, READ_BUFFER, 1024), Errno::EBADF.result());
    }

    #[test]
    fn the_working_directory_is_changed_and_reported() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let getcwd = |size| call(&process, number::GETCWD, [READ_BUFFER, size, 0, 0, 0, 0]);
        let chdir = |path: &[u8]| {
            let path = at_path(&process, path);
            call(&process, number::CHDIR, [path, 0, 0, 0, 0, 0])
        };
        assert_eq!(getcwd(100), 2);
        assert_eq!(read_bytes(&process, READ_BUFFER, 2), b"/\0");
        assert_eq!(getcwd(1), Errno::ERANGE.result());
        assert_eq!(chdir(b"/usr/share/eggos"), 0);
        assert_eq!(getcwd(17), 17);
        assert_eq!(read_bytes(&process, READ_BUFFER, 17), b"/usr/share/eggos\0");
        assert_eq!(getcwd(16), Errno::ERANGE.result());
        assert_eq!(
            call(&process, number::GETCWD, [UNMAPPED, 100, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(open_at(&process, FDCWD, b"about", 0), 3);
        assert_eq!(chdir(b".."), 0);
        assert_eq!(getcwd(100), 11);
        assert_eq!(chdir(b"/etc/hostname"), Errno::ENOTDIR.result());
        assert_eq!(chdir(b"/nowhere"), Errno::ENOENT.result());
        assert_eq!(chdir(b""), Errno::ENOENT.result());
        let etc = open_at(&process, FDCWD, b"/etc", descriptors::open_flag::PATH);
        assert_eq!(call(&process, number::FCHDIR, [etc, 0, 0, 0, 0, 0]), 0);
        assert_eq!(getcwd(100), 5);
        let path = at_path(&process, b"hostname");
        assert_eq!(
            call(
                &process,
                number::NEWFSTATAT,
                [FDCWD, path, STAT_BUFFER, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call(&process, number::FCHDIR, [3, 0, 0, 0, 0, 0]),
            Errno::ENOTDIR.result()
        );
        assert_eq!(
            call(&process, number::FCHDIR, [1, 0, 0, 0, 0, 0]),
            Errno::ENOTDIR.result()
        );
        assert_eq!(
            call(&process, number::FCHDIR, [99, 0, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
        assert_eq!(getcwd(100), 5);
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
        let id = process::allocate_thread_id::<P>().unwrap();
        let second = first.new_thread(
            id,
            clear_child_tid,
            first.files().share(),
            first.working_directory().share(),
        );
        process
            .signals()
            .lock()
            .add_thread(&second, first, process.id().get())
            .unwrap();
        second
    }

    #[test]
    fn clone_refuses_what_linux_refuses_and_what_processes_do_not_share() {
        use thread::flag::*;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let clone = |flags| call(&process, number::CLONE, [flags, 0, 0, 0, 0, 0]);
        const SIGCHLD: u64 = 17;
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
            SIGCHLD | NEWPID,
            65,
            VM | VFORK | EXIT_SIGNAL,
        ] {
            assert_eq!(clone(flags), Errno::EINVAL.result(), "{flags:#x}");
        }
        for flags in [
            VM | SIGHAND | SIGCHLD,
            PARENT | SIGCHLD,
            NEW_THREAD | PARENT,
            NEW_THREAD | VFORK,
        ] {
            assert_eq!(clone(flags), Errno::ENOSYS.result(), "{flags:#x}");
        }
        #[cfg(target_arch = "x86_64")]
        for flags in [NEW_THREAD | SETTLS, SETTLS | SIGCHLD] {
            assert_eq!(
                call(&process, number::CLONE, [flags, 0, 0, 0, 1 << 63, 0]),
                Errno::EPERM.result()
            );
        }
        assert!(!process.has_ended());
        assert!(!has_children(&process));
        assert!(process.remove_thread(), "a refused clone counted a thread");
    }

    #[test]
    fn a_fork_without_frames_for_the_copy_is_enomem_and_takes_none() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        process.write_user_memory(SCRATCH, b"kept").unwrap();
        let free = || memory::FRAMES.lock().as_ref().unwrap().free_frames();
        let mut hoard = Vec::new();
        while free() > 2 {
            hoard.push(memory::FRAMES.lock().as_mut().unwrap().allocate().unwrap());
        }
        let before = free();
        let fork = || call(&process, number::CLONE, [17, 0, 0, 0, 0, 0]);
        assert_eq!(fork(), Errno::ENOMEM.result());
        #[cfg(target_arch = "x86_64")]
        assert_eq!(call(&process, number::FORK, [0; 6]), Errno::ENOMEM.result());
        assert_eq!(free(), before);
        assert!(!has_children(&process));
        let mut kept = [0u8; 4];
        process.read_user_memory(SCRATCH, &mut kept).unwrap();
        assert_eq!(&kept, b"kept");
        let mut frames = memory::FRAMES.lock();
        for frame in hoard {
            // SAFETY: allocated above from this allocator and never mapped.
            unsafe { frames.as_mut().unwrap().free(frame) };
        }
    }

    #[test]
    fn a_fork_of_memory_with_inaccessible_shared_anonymous_memory_is_enosys() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let fork = || call(&process, number::CLONE, [17, 0, 0, 0, 0, 0]);
        let private = crate::linux::memory::map::PRIVATE | crate::linux::memory::map::ANONYMOUS;
        let shared = crate::linux::memory::map::SHARED | crate::linux::memory::map::ANONYMOUS;
        let mmap = |prot, flags| call(&process, number::MMAP, [0, 4096, prot, flags, u64::MAX, 0]);
        assert!(mmap(3, private) < Errno::EINVAL.result());
        assert!(mmap(3, shared) < Errno::EINVAL.result());
        assert!(mmap(0, shared) < Errno::EINVAL.result());
        assert_eq!(fork(), Errno::ENOSYS.result());
        assert!(!has_children(&process));
    }

    #[test]
    fn inherited_fp_registers_become_the_running_ones_before_anything_reads_them() {
        use core::sync::atomic::{AtomicBool, Ordering};
        use eggos_hal::fp::FpState;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        crate::thread::start::<P>().unwrap();
        static CHECKED: AtomicBool = AtomicBool::new(false);
        CHECKED.store(false, Ordering::SeqCst);
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let process = Arc::new(unsafe { Process::<P>::with_test_program(&kernel) }.unwrap());
        let context = Process::<P>::test_program_context(|program| program.illegal_instruction);
        let user = process::spawn::<P, _>(process, context, move |process, _, _| {
            let mut thread = LinuxThread::first(process);
            let inherited = Box::new(FpState::new());
            let address = core::ptr::from_ref::<FpState>(&inherited).addr();
            thread.inherit_fp(Some(inherited));
            assert!(thread::running_fp::<P>().is_none());
            load_inherited_fp::<P>(&mut thread);
            assert!(thread.take_inherited_fp().is_none());
            let running = crate::thread::with_running_fp::<P, _>(|state| {
                core::ptr::from_mut::<FpState>(state).addr()
            });
            assert_eq!(running, Some(address));
            assert!(thread::running_fp::<P>().is_some());
            CHECKED.store(true, Ordering::SeqCst);
            ControlFlow::Break(())
        })
        .unwrap();
        user.join::<P>();
        assert!(CHECKED.load(Ordering::SeqCst));
    }

    #[test]
    fn execve_forgets_the_fp_registers_a_thread_inherited() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let mut thread = LinuxThread::first(&process);
        thread.inherit_fp(Some(Box::new(eggos_hal::fp::FpState::new())));
        thread.start_image(process.id().get());
        assert!(thread.take_inherited_fp().is_none());
    }

    fn child_thread(
        parent: &Arc<Process<P>>,
        memory: process::ChildMemory,
        clear_child_tid: u64,
    ) -> (Arc<Process<P>>, LinuxThread) {
        let first = take_first_thread(parent);
        let child = Arc::new(parent.new_child(memory).unwrap());
        process_table::TABLE
            .lock()
            .set_exit_signal(child.id().get(), 17);
        child.add_thread();
        let thread = first.new_process_thread(
            child.id().get(),
            clear_child_tid,
            first.files().copy().unwrap(),
            first.working_directory().copy(),
            true,
        );
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(parent.id().get(), first);
        (child, thread)
    }

    #[test]
    fn a_child_knows_its_parent_keeps_its_actions_and_its_exit_is_recorded_for_it() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (parent, pid) = process_with_stacks(&kernel);
        let sigusr1 = u64::from(signal::SIGUSR1);
        set_handler(&parent, sigusr1, 0, 0);
        let (child, mut thread) = child_thread(&parent, process::ChildMemory::Copied, 0);
        assert_eq!(call_as(&child, &mut thread, number::GETPPID, [0; 6]), pid);
        assert_eq!(
            call_as(&child, &mut thread, number::GETPID, [0; 6]),
            u64::from(child.id().get())
        );
        assert_eq!(call(&parent, number::GETPPID, [0; 6]), 0);
        assert_eq!(
            child.signals().lock().action(signal::SIGUSR1),
            parent.signals().lock().action(signal::SIGUSR1)
        );
        let wait4 = |pid: i64, options| {
            let arguments = [pid as u64, SCRATCH, options, SCRATCH + 8, 0, 0];
            call(&parent, number::WAIT4, arguments)
        };
        let child_id = u64::from(child.id().get());
        assert_eq!(wait4(-1, 1), 0);
        assert_eq!(wait4(-1, 0x100), Errno::EINVAL.result());
        assert_eq!(wait4(i64::from(i32::MIN), 1), Errno::ESRCH.result());
        let mut context = called(number::EXIT_GROUP, [0x115, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&child, &mut thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        drop((child, thread));
        let waitid = |options| call(&parent, number::WAITID, [0, 0, SCRATCH, options, 0, 0]);
        assert_eq!(waitid(1), Errno::EINVAL.result());
        assert_eq!(waitid(2 | 1), Errno::ECHILD.result());
        assert_eq!(wait4(child_id as i64 + 1, 1), Errno::ECHILD.result());
        assert_eq!(wait4(-1, 0x8000_0000), Errno::ECHILD.result());
        parent.write_user_memory(SCRATCH + 8, &[0xff; 144]).unwrap();
        assert_eq!(wait4(0, 0), child_id);
        assert_eq!(read_bytes(&parent, SCRATCH, 4), 0x1500u32.to_ne_bytes());
        assert_eq!(read_bytes(&parent, SCRATCH + 8, 144), [0; 144]);
        assert_eq!(wait4(-1, 1), Errno::ECHILD.result());
    }

    fn has_children(process: &Process<P>) -> bool {
        use process_table::{ChildKind, Ends, Found, Selector};
        let id = process.id().get();
        process_table::TABLE
            .lock()
            .wait(id, Selector::Any, ChildKind::All, Ends::Report)
            != Found::NoChild
    }

    #[test]
    fn a_blocking_wait_wakes_as_a_child_ends_and_its_parent_takes_sigchld() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        crate::thread::start::<P>().unwrap();
        let (parent, _) = process_with_stacks(&kernel);
        register_running(&parent);
        block_signals(&parent, 1 << (signal::SIGCHLD - 1));
        let (child, mut thread) = child_thread(&parent, process::ChildMemory::Copied, 0);
        static WAITED: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
        let waiter = crate::thread::spawn::<P, _>({
            let parent = Arc::clone(&parent);
            move || {
                let infop = SCRATCH + 0x100;
                let arguments = [0, 0, infop, 4, 0, 0];
                let result = call(&parent, number::WAITID, arguments);
                assert_eq!(read_bytes(&parent, infop, 4), 17u32.to_ne_bytes());
                assert_eq!(read_bytes(&parent, infop + 8, 4), 1u32.to_ne_bytes());
                assert_eq!(read_bytes(&parent, infop + 24, 4), 9u32.to_ne_bytes());
                *WAITED.lock().unwrap() = Some(result);
            }
        })
        .unwrap();
        for _ in 0..4 {
            crate::thread::yield_now::<P>();
        }
        assert_eq!(*WAITED.lock().unwrap(), None);
        let mut context = called(number::EXIT_GROUP, [9, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&child, &mut thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        waiter.join::<P>();
        assert_eq!(*WAITED.lock().unwrap(), Some(0));
        assert_eq!(
            blocked_pending(&parent, &mut take_first_thread(&parent)) & (1 << 16),
            1 << 16
        );
        assert!(!has_children(&parent));
    }

    #[test]
    fn a_wait_for_a_running_child_ends_as_another_thread_ends_the_process() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        crate::thread::start::<P>().unwrap();
        let (parent, _) = process_with_stacks(&kernel);
        register_running(&parent);
        let (_child, _thread) = child_thread(&parent, process::ChildMemory::Copied, 0);
        parent.add_thread();
        static WAIT_ENDED: std::sync::Mutex<Option<ControlFlow<()>>> = std::sync::Mutex::new(None);
        let waiter = crate::thread::spawn::<P, _>({
            let parent = Arc::clone(&parent);
            move || {
                let mut context = called(number::WAIT4, [u64::MAX, 0, 0, 0, 0, 0]);
                let flow = handle_first(&parent, &mut context, UserExit::Syscall);
                *WAIT_ENDED.lock().unwrap() = Some(flow);
            }
        })
        .unwrap();
        for _ in 0..4 {
            crate::thread::yield_now::<P>();
        }
        assert_eq!(*WAIT_ENDED.lock().unwrap(), None);
        let _ = end_process::<P, ()>(&parent, ProcessExit::Exited(0));
        waiter.join::<P>();
        assert_eq!(*WAIT_ENDED.lock().unwrap(), Some(ControlFlow::Break(())));
    }

    #[test]
    fn children_of_a_parent_ignoring_sigchld_are_reaped_as_they_end() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (parent, _) = process_with_stacks(&kernel);
        register_running(&parent);
        ignore(&parent, u64::from(signal::SIGCHLD));
        let (child, mut thread) = child_thread(&parent, process::ChildMemory::Copied, 0);
        assert!(has_children(&parent));
        let mut context = called(number::EXIT_GROUP, [0, 0, 0, 0, 0, 0]);
        let _ = handle(&child, &mut thread, &mut context, UserExit::Syscall);
        assert!(!has_children(&parent));
        let wait4 = [u64::MAX, 0, 0, 0, 0, 0];
        assert_eq!(call(&parent, number::WAIT4, wait4), Errno::ECHILD.result());
    }

    #[test]
    fn an_orphan_goes_to_the_subreaper_and_groups_and_sessions_follow_the_calls() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (reaper, reaper_id) = process_with_stacks(&kernel);
        register_running(&reaper);
        assert_eq!(call(&reaper, number::PRCTL, [36, 1, 0, 0, 0, 0]), 0);
        assert_eq!(call(&reaper, number::PRCTL, [37, SCRATCH, 0, 0, 0, 0]), 0);
        assert_eq!(read_bytes(&reaper, SCRATCH, 4), 1u32.to_ne_bytes());
        assert_eq!(
            call(&reaper, number::PRCTL, [15, 0, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        let (middle, mut middle_thread) = child_thread(&reaper, process::ChildMemory::Copied, 0);
        let middle_id = u64::from(middle.id().get());
        let mut first = middle_thread;
        let orphan = Arc::new(middle.new_child(process::ChildMemory::Copied).unwrap());
        process_table::TABLE
            .lock()
            .set_exit_signal(orphan.id().get(), 17);
        let orphan_id = u64::from(orphan.id().get());
        let mut orphan_thread = first.new_process_thread(
            orphan.id().get(),
            0,
            first.files().copy().unwrap(),
            first.working_directory().copy(),
            true,
        );
        orphan.add_thread();
        let call_in = |process: &Arc<Process<P>>, thread: &mut LinuxThread, number, arguments| {
            call_as(process, thread, number, arguments)
        };
        assert_eq!(
            call_in(&orphan, &mut orphan_thread, number::GETPPID, [0; 6]),
            middle_id
        );
        assert_eq!(
            call_in(
                &middle,
                &mut first,
                number::SETPGID,
                [orphan_id, 0, 0, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call_in(&orphan, &mut orphan_thread, number::GETPGID, [0; 6]),
            orphan_id
        );
        assert_eq!(
            call_in(
                &middle,
                &mut first,
                number::GETPGID,
                [orphan_id, 0, 0, 0, 0, 0]
            ),
            orphan_id
        );
        assert_eq!(
            call_in(&orphan, &mut orphan_thread, number::SETSID, [0; 6]),
            Errno::EPERM.result()
        );
        assert_eq!(
            call_in(&middle, &mut first, number::SETSID, [0; 6]),
            middle_id
        );
        assert_eq!(
            call_in(&middle, &mut first, number::GETSID, [0; 6]),
            middle_id
        );
        assert_eq!(
            call_in(&orphan, &mut orphan_thread, number::GETSID, [0; 6]),
            reaper_id
        );
        assert_eq!(
            call_in(
                &middle,
                &mut first,
                number::SETPGID,
                [orphan_id, middle_id, 0, 0, 0, 0]
            ),
            Errno::EPERM.result()
        );
        assert_eq!(
            call_in(
                &middle,
                &mut first,
                number::IOCTL,
                [0, 0x540f, SCRATCH, 0, 0, 0]
            ),
            Errno::ENOTTY.result()
        );
        let mut context = called(number::EXIT_GROUP, [0, 0, 0, 0, 0, 0]);
        let _ = handle(&middle, &mut first, &mut context, UserExit::Syscall);
        assert_eq!(
            call_in(&orphan, &mut orphan_thread, number::GETPPID, [0; 6]),
            reaper_id
        );
        middle_thread = first;
        drop((middle, middle_thread));
        let mut context = called(number::EXIT_GROUP, [4, 0, 0, 0, 0, 0]);
        let _ = handle(&orphan, &mut orphan_thread, &mut context, UserExit::Syscall);
        let wait4 = |pid: u64| call(&reaper, number::WAIT4, [pid, SCRATCH, 0, 0, 0, 0]);
        assert_eq!(wait4(orphan_id), orphan_id);
        assert_eq!(read_bytes(&reaper, SCRATCH, 4), 0x400u32.to_ne_bytes());
        assert_eq!(wait4(middle_id), middle_id);
        assert_eq!(wait4(u64::MAX), Errno::ECHILD.result());
    }

    #[test]
    fn a_child_sharing_the_memory_clears_its_tid_word_there_as_it_ends() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (parent, _) = process_with_stacks(&kernel);
        parent.write_user_memory(SCRATCH, &[0xff; 8]).unwrap();
        let (child, mut thread) = child_thread(&parent, process::ChildMemory::Shared, SCRATCH);
        assert_eq!(child.futex_space(), parent.futex_space());
        assert!(parent.shares_memory());
        let mut context = called(number::EXIT_GROUP, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&child, &mut thread, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        let mut word = [0u8; 8];
        parent.read_user_memory(SCRATCH, &mut word).unwrap();
        assert_eq!(word, [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        drop((child, thread));
        assert!(!parent.shares_memory());
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

    fn write_u64(process: &Process<P>, address: u64, value: u64) {
        process
            .write_user_memory(address, &value.to_ne_bytes())
            .unwrap();
    }

    fn write_u32(process: &Process<P>, address: u64, value: u32) {
        process
            .write_user_memory(address, &value.to_ne_bytes())
            .unwrap();
    }

    fn read_u32(process: &Process<P>, address: u64) -> u32 {
        let mut bytes = [0; 4];
        process.read_user_memory(address, &mut bytes).unwrap();
        u32::from_ne_bytes(bytes)
    }

    #[test]
    fn get_robust_list_reports_the_caller_s_head_and_its_size() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let (head, size) = (SCRATCH, SCRATCH + 8);
        let get = |thread: &mut LinuxThread, tid: u64, head: u64| {
            call_as(
                &process,
                thread,
                number::GET_ROBUST_LIST,
                [tid, head, size, 0, 0, 0],
            )
        };
        assert_eq!(get(&mut second, 0, head), 0);
        let mut bytes = [0u8; 16];
        process.read_user_memory(head, &mut bytes).unwrap();
        assert_eq!(bytes[..8], 0u64.to_ne_bytes());
        assert_eq!(bytes[8..], 24u64.to_ne_bytes());
        let list = SCRATCH + 0x100;
        let set = [list, 24, 0, 0, 0, 0];
        assert_eq!(
            call_as(&process, &mut second, number::SET_ROBUST_LIST, set),
            0
        );
        assert_eq!(second.robust_list(), list);
        assert_eq!(first.robust_list(), 0);
        let tid = u64::from(second.id());
        assert_eq!(get(&mut second, tid, head), 0);
        process.read_user_memory(head, &mut bytes[..8]).unwrap();
        assert_eq!(bytes[..8], list.to_ne_bytes());
        assert_eq!(get(&mut second, tid | 1 << 32, head), 0);
        assert_eq!(get(&mut first, tid, head), Errno::ESRCH.result());
        assert_eq!(get(&mut second, 0, 0), Errno::EFAULT.result());
        let refused = [list, 16, 0, 0, 0, 0];
        assert_eq!(
            call_as(&process, &mut second, number::SET_ROBUST_LIST, refused),
            Errno::EINVAL.result()
        );
        assert_eq!(second.robust_list(), list);
    }

    #[test]
    fn getcpu_writes_the_cpu_and_node_it_is_given_room_for() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let (cpu_word, node_word) = (SCRATCH, SCRATCH + 4);
        process.write_user_memory(cpu_word, &[0xff; 8]).unwrap();
        let getcpu = |cpu, node| call(&process, number::GETCPU, [cpu, node, 0, 0, 0, 0]);
        assert_eq!(getcpu(cpu_word, node_word), 0);
        let mut bytes = [0u8; 8];
        process.read_user_memory(cpu_word, &mut bytes).unwrap();
        let index = crate::cpu::current::<P>().index() as u32;
        assert_eq!(bytes[..4], index.to_ne_bytes());
        assert_eq!(bytes[4..], 0u32.to_ne_bytes());
        assert_eq!(getcpu(0, 0), 0);
        assert_eq!(getcpu(0, node_word), 0);
        assert_eq!(getcpu(8, 0), Errno::EFAULT.result());
        assert_eq!(getcpu(cpu_word, 8), Errno::EFAULT.result());
    }

    #[test]
    fn the_scheduling_policy_is_sched_other_with_priority_0_for_the_caller_only() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let pid = u64::from(process.id().get());
        let other = u64::from(i32::MAX as u32);
        let negative = u64::MAX;
        for asked in [0, pid] {
            assert_eq!(
                call(&process, number::SCHED_GETSCHEDULER, [asked, 0, 0, 0, 0, 0]),
                0
            );
            process.write_user_memory(SCRATCH, &[0xff; 8]).unwrap();
            let param = [asked, SCRATCH, 0, 0, 0, 0];
            assert_eq!(call(&process, number::SCHED_GETPARAM, param), 0);
            assert_eq!(
                read_bytes(&process, SCRATCH, 8),
                [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]
            );
        }
        let scheduler = |pid| call(&process, number::SCHED_GETSCHEDULER, [pid, 0, 0, 0, 0, 0]);
        let param = |pid, at| call(&process, number::SCHED_GETPARAM, [pid, at, 0, 0, 0, 0]);
        assert_eq!(scheduler(negative), Errno::EINVAL.result());
        assert_eq!(scheduler(other), Errno::ESRCH.result());
        assert_eq!(param(0, 0), Errno::EINVAL.result());
        assert_eq!(param(negative, SCRATCH), Errno::EINVAL.result());
        assert_eq!(param(other, SCRATCH), Errno::ESRCH.result());
        assert_eq!(param(0, u64::MAX - 1), Errno::EFAULT.result());
    }

    #[test]
    fn sched_getaffinity_writes_the_online_cpus_and_returns_the_mask_s_size() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let cpus = crate::cpu::CPUS.all();
        let words = cpus.len().max(1).div_ceil(64);
        let size = (words * 8) as u64;
        let expected: u64 = if cpus.is_empty() {
            1
        } else {
            cpus.iter()
                .take(64)
                .enumerate()
                .filter(|(_, cpu)| cpu.is_online())
                .fold(0, |bits, (index, _)| bits | 1 << index)
        };
        process.write_user_memory(SCRATCH, &[0xff; 24]).unwrap();
        let affinity = |pid, size, mask| {
            call(
                &process,
                number::SCHED_GETAFFINITY,
                [pid, size, mask, 0, 0, 0],
            )
        };
        let pid = u64::from(process.id().get());
        for asked in [0, pid] {
            assert_eq!(affinity(asked, size + 8, SCRATCH), size);
            let mut word = [0u8; 8];
            process.read_user_memory(SCRATCH, &mut word).unwrap();
            assert_eq!(u64::from_ne_bytes(word), expected);
            assert_ne!(expected & 1, 0);
        }
        let mut beyond = [0u8; 8];
        process
            .read_user_memory(SCRATCH + size, &mut beyond)
            .unwrap();
        assert_eq!(beyond, [0xff; 8]);
        assert_eq!(affinity(0, size - 8, SCRATCH), Errno::EINVAL.result());
        assert_eq!(affinity(0, size + 4, SCRATCH), Errno::EINVAL.result());
        assert_eq!(
            affinity(u64::from(u32::MAX), size, SCRATCH),
            Errno::ESRCH.result()
        );
        assert_eq!(affinity(0, size, 8), Errno::EFAULT.result());
    }

    #[test]
    fn a_thread_s_exit_releases_the_robust_locks_it_holds() {
        use futex::robust::{OWNER_DIED, PRIORITY_INHERITANCE, WAITERS};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, SCRATCH + 0xff0);
        let tid = second.id();
        let head = SCRATCH;
        let entry = |n: u64| SCRATCH + 0x100 * n;
        let lock = |n: u64| entry(n) - 0x10;
        write_u64(&process, head, entry(1));
        write_u64(&process, head + 8, (-0x10i64) as u64);
        write_u64(&process, head + 16, entry(5));
        write_u64(&process, entry(1), entry(2));
        write_u64(&process, entry(2), entry(3) | PRIORITY_INHERITANCE);
        write_u64(&process, entry(3), entry(4));
        write_u64(&process, entry(4), head);
        write_u32(&process, lock(1), tid | WAITERS);
        write_u32(&process, lock(2), tid);
        write_u32(&process, lock(3), tid | WAITERS);
        write_u32(&process, lock(4), (tid + 1) | WAITERS);
        write_u32(&process, lock(5), tid);
        second.set_robust_list(head);

        let mut context = called(number::EXIT, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert!(!process.has_ended());
        assert_eq!(read_u32(&process, lock(1)), WAITERS | OWNER_DIED);
        assert_eq!(read_u32(&process, lock(2)), OWNER_DIED);
        assert_eq!(read_u32(&process, lock(3)), WAITERS | OWNER_DIED);
        assert_eq!(read_u32(&process, lock(4)), (tid + 1) | WAITERS);
        assert_eq!(read_u32(&process, lock(5)), OWNER_DIED);
        assert_eq!(read_u32(&process, SCRATCH + 0xff0), 0);
    }

    #[test]
    fn a_robust_list_that_loops_or_faults_ends_the_walk() {
        use futex::robust::OWNER_DIED;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let first = take_first_thread(&process);
        let mut looping = second_thread(&process, &first, 0);
        let tid = looping.id();
        let (head, entry, pending) = (SCRATCH, SCRATCH + 0x100, SCRATCH + 0x200);
        write_u64(&process, head, entry);
        write_u64(&process, head + 8, 8);
        write_u64(&process, head + 16, pending);
        write_u64(&process, entry, entry);
        write_u32(&process, entry + 8, tid);
        write_u32(&process, pending + 8, tid);
        looping.set_robust_list(head);
        let mut context = called(number::EXIT, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut looping, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(read_u32(&process, entry + 8), OWNER_DIED);
        assert_eq!(read_u32(&process, pending + 8), OWNER_DIED);

        let mut faulting = second_thread(&process, &first, 0);
        let tid = faulting.id();
        write_u64(&process, entry, 0x10);
        write_u32(&process, entry + 8, tid);
        write_u32(&process, pending + 8, tid);
        faulting.set_robust_list(head);
        let mut context = called(number::EXIT, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut faulting, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(read_u32(&process, entry + 8), OWNER_DIED);
        assert_eq!(
            read_u32(&process, pending + 8),
            tid,
            "a fault did not end the walk"
        );
    }

    #[test]
    fn a_thread_of_an_ended_process_does_not_sleep() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, SCRATCH);
        let mut context = called(number::EXIT_GROUP, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        let an_hour = SCRATCH + 0x100;
        let mut timespec = [0u8; 16];
        timespec[..8].copy_from_slice(&3600u64.to_ne_bytes());
        process.write_user_memory(an_hour, &timespec).unwrap();
        let mut context = called(number::CLOCK_NANOSLEEP, [1, 0, an_hour, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        let mask = SCRATCH + 0x200;
        process.write_user_memory(mask, &[0; 8]).unwrap();
        let mut context = called(number::RT_SIGSUSPEND, [mask, 8, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
    }

    #[test]
    fn threads_queue_signals_for_each_other_which_each_takes_in_order() {
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
        write_siginfo(&process, info, signals::code::QUEUE, 0x33);
        assert_eq!(queue(&mut first, tid), 0);
        assert_eq!(second.signal_state().pending, 1 << (rt - 1));
        assert_eq!(queue(&mut first, tid + 1000), Errno::ESRCH.result());
        assert_eq!(queue(&mut first, i32::MAX as u64), Errno::ESRCH.result());

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
        assert_eq!(wait(&mut second), (rt, 0x33));
        assert_eq!(wait(&mut first), (rt, 0x11));
        assert_eq!(first.signal_state().pending, 0);
        assert_eq!(second.signal_state().pending, 0);
    }

    #[test]
    fn a_signal_sent_to_another_thread_is_delivered_when_it_comes_back() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        let tgkill = |thread: &mut LinuxThread, signal| {
            call_as(
                &process,
                thread,
                number::TGKILL,
                [pid, tid, signal, 0, 0, 0],
            )
        };
        assert_eq!(tgkill(&mut first, 0), 0);
        assert_eq!(tgkill(&mut first, 65), Errno::EINVAL.result());
        let sent = process.signals_sent();
        let sigusr1 = 10;
        assert_eq!(tgkill(&mut first, sigusr1), 0);
        assert_ne!(process.signals_sent(), sent);
        assert_eq!(second.signal_state().pending, 0);
        assert_eq!(process.exit_status(), None);
        let mut context = called(number::GETPID, [0; 6]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Interrupt),
            ControlFlow::Break(())
        );
        assert_eq!(second.signals_seen(), process.signals_sent());
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed { signal: 10 })
        );
    }

    #[test]
    fn sigkill_sent_to_another_thread_ends_the_process_at_once() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        let mut context = called(number::TKILL, [tid, 9, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed { signal: 9 })
        );
    }

    #[test]
    fn an_exited_first_thread_takes_signals_it_never_delivers_sigkill_included() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let mut context = called(number::EXIT, [3, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut first, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        let sent = process.signals_sent();
        let tgkill = |thread: &mut LinuxThread, signal| {
            call_as(
                &process,
                thread,
                number::TGKILL,
                [pid, pid, signal, 0, 0, 0],
            )
        };
        assert_eq!(tgkill(&mut second, 10), 0);
        assert_eq!(tgkill(&mut second, 65), Errno::EINVAL.result());
        assert_eq!(tgkill(&mut second, 9), 0);
        assert_eq!(process.signals_sent(), sent);
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn what_waits_for_a_thread_stays_bounded_however_often_it_is_sent_signals() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        let mut tgkill = |signal: u64| {
            call_as(
                &process,
                &mut first,
                number::TGKILL,
                [pid, tid, signal, 0, 0, 0],
            )
        };
        let left = || process.signals().lock().left_for(tid as u32);
        let (sigusr1, sigcont, sigtstp, sigttin) = (10, 18, 20, 21);
        for _ in 0..100 {
            assert_eq!(tgkill(sigusr1), 0);
        }
        assert_eq!(left(), 1);
        assert_eq!(tgkill(sigtstp), 0);
        assert_eq!(tgkill(sigcont), 0);
        assert_eq!(left(), 2);
        assert_eq!(tgkill(sigttin), 0);
        assert_eq!(left(), 3);
        for _ in 0..100 {
            assert_eq!(tgkill(sigttin), 0);
        }
        assert_eq!(left(), 3);
        assert!(signals::take_in_sent(&process, &mut second));
        assert_eq!(left(), 0);
        assert_eq!(
            second.signal_state().pending,
            1 << (sigusr1 - 1) | 1 << (sigttin - 1)
        );
    }

    #[test]
    fn ignoring_a_signal_discards_it_from_every_thread_but_blocked_ones_ignored_by_default_stay() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let tid = u64::from(second.id());
        let (sigusr2, sigchld) = (12u64, 17u64);
        let blocked = 1u64 << (sigusr2 - 1) | 1 << (sigchld - 1);
        process
            .write_user_memory(SCRATCH + 0x300, &blocked.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call_as(
                &process,
                &mut second,
                number::RT_SIGPROCMASK,
                [0, SCRATCH + 0x300, 0, 8, 0, 0]
            ),
            0
        );
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::TGKILL,
                [pid, tid, sigusr2, 0, 0, 0]
            ),
            0
        );
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::TGKILL,
                [pid, tid, sigchld, 0, 0, 0]
            ),
            0
        );
        assert!(signals::take_in_sent(&process, &mut second));
        assert_eq!(second.signal_state().pending, blocked);
        let action = SCRATCH + 0x400;
        let words = [signals::IGNORE, 0, 0, 0];
        process
            .write_user_memory(action, &words.map(u64::to_ne_bytes).concat())
            .unwrap();
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::RT_SIGACTION,
                [sigusr2, action, 0, 8, 0, 0]
            ),
            0
        );
        assert!(signals::take_in_sent(&process, &mut second));
        assert_eq!(second.signal_state().pending, 1 << (sigchld - 1));
        assert!(!signals::take_in_sent(&process, &mut second));
        assert_eq!(second.signal_state().pending, 1 << (sigchld - 1));

        let mut context = called(number::EXIT, [0, 0, 0, 0, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::TGKILL,
                [pid, tid, sigusr2, 0, 0, 0]
            ),
            Errno::ESRCH.result()
        );
    }

    fn block_signals_in(process: &Arc<Process<P>>, thread: &mut LinuxThread, set: u64) {
        let address = SCRATCH + 0x300;
        process
            .write_user_memory(address, &set.to_ne_bytes())
            .unwrap();
        assert_eq!(
            call_as(
                process,
                thread,
                number::RT_SIGPROCMASK,
                [0, address, 0, 8, 0, 0]
            ),
            0
        );
    }

    fn blocked_pending(process: &Arc<Process<P>>, thread: &mut LinuxThread) -> u64 {
        let set = SCRATCH + 0x380;
        assert_eq!(
            call_as(process, thread, number::RT_SIGPENDING, [set, 8, 0, 0, 0, 0]),
            0
        );
        let mut bytes = [0; 8];
        process.read_user_memory(set, &mut bytes).unwrap();
        u64::from_ne_bytes(bytes)
    }

    #[test]
    fn a_signal_sent_to_the_process_goes_to_a_thread_that_does_not_block_it() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let mut second = second_thread(&process, &first, 0);
        let sigusr1 = 10u64;
        block_signals_in(&process, &mut first, 1 << (sigusr1 - 1));
        let sent = process.signals_sent();
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::KILL,
                [pid, sigusr1, 0, 0, 0, 0]
            ),
            0
        );
        assert_ne!(process.signals_sent(), sent);
        assert_eq!(first.signal_state().pending, 0);
        assert_eq!(blocked_pending(&process, &mut first), 1 << (sigusr1 - 1));
        assert_eq!(second.signal_state().pending, 0);
        let mut context = called(number::GETPID, [0; 6]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Interrupt),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed { signal: 10 })
        );
    }

    #[test]
    fn a_signal_every_thread_blocks_waits_for_one_that_waits_for_it_or_unblocks_it() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (rt, sigusr2) = (40u64, 12u64);
        let blocked = 1u64 << (rt - 1) | 1 << (sigusr2 - 1);
        let mut first = take_first_thread(&process);
        block_signals_in(&process, &mut first, blocked);
        let mut second = second_thread(&process, &first, 0);
        let (info, waited, set) = (SCRATCH, SCRATCH + 0x100, SCRATCH + 0x200);
        for value in [0x11, 0x22] {
            write_siginfo(&process, info, signals::code::QUEUE, value);
            assert_eq!(
                call_as(
                    &process,
                    &mut first,
                    number::RT_SIGQUEUEINFO,
                    [pid, rt, info, 0, 0, 0]
                ),
                0
            );
        }
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::KILL,
                [pid, sigusr2, 0, 0, 0, 0]
            ),
            0
        );
        assert!(!signals::take_in_sent(&process, &mut second));
        assert_eq!(second.signal_state().pending, 0);
        assert_eq!(blocked_pending(&process, &mut second), blocked);

        process
            .write_user_memory(set, &(1u64 << (rt - 1)).to_ne_bytes())
            .unwrap();
        let no_wait = SCRATCH + 0x280;
        process.write_user_memory(no_wait, &[0; 16]).unwrap();
        let wait = |thread: &mut LinuxThread| {
            let taken = call_as(
                &process,
                thread,
                number::RT_SIGTIMEDWAIT,
                [set, waited, no_wait, 8, 0, 0],
            );
            (taken, read_siginfo(&process, waited).2)
        };
        assert_eq!(wait(&mut second), (rt, 0x11));
        assert_eq!(second.signal_state().waited, 0);
        assert_eq!(wait(&mut first), (rt, 0x22));
        assert_eq!(first.signal_state().pending, 0);
        assert_eq!(second.signal_state().pending, 0);
        assert_eq!(process.signals().lock().left_for(0), 1);

        let unblock = SCRATCH + 0x300;
        process
            .write_user_memory(unblock, &(1u64 << (sigusr2 - 1)).to_ne_bytes())
            .unwrap();
        let mut context = called(number::RT_SIGPROCMASK, [1, unblock, 0, 8, 0, 0]);
        assert_eq!(
            handle(&process, &mut second, &mut context, UserExit::Syscall),
            ControlFlow::Break(())
        );
        assert_eq!(
            process.exit_status(),
            Some(ProcessExit::Killed { signal: 12 })
        );
    }

    #[test]
    fn a_signal_sent_to_the_process_waits_once_and_ignoring_it_discards_it() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let (sigusr1, sigcont, sigtstp) = (10u64, 18u64, 20u64);
        let blocked = 1u64 << (sigusr1 - 1) | 1 << (sigcont - 1) | 1 << (sigtstp - 1);
        let mut first = take_first_thread(&process);
        block_signals_in(&process, &mut first, blocked);
        let mut second = second_thread(&process, &first, 0);
        let mut kill = |signal: u64| {
            call_as(
                &process,
                &mut first,
                number::KILL,
                [pid, signal, 0, 0, 0, 0],
            )
        };
        for _ in 0..100 {
            assert_eq!(kill(sigusr1), 0);
        }
        assert_eq!(kill(sigtstp), 0);
        assert_eq!(kill(sigcont), 0);
        assert_eq!(process.signals().lock().left_for(0), 2);
        assert_eq!(kill(sigtstp), 0);
        assert_eq!(
            blocked_pending(&process, &mut second),
            1 << (sigusr1 - 1) | 1 << (sigtstp - 1)
        );
        let action = SCRATCH + 0x400;
        let words = [signals::IGNORE, 0, 0, 0];
        process
            .write_user_memory(action, &words.map(u64::to_ne_bytes).concat())
            .unwrap();
        assert_eq!(
            call_as(
                &process,
                &mut second,
                number::RT_SIGACTION,
                [sigusr1, action, 0, 8, 0, 0]
            ),
            0
        );
        assert_eq!(blocked_pending(&process, &mut second), 1 << (sigtstp - 1));
        assert_eq!(process.signals().lock().left_for(0), 1);
    }

    /// `process`'s first thread, kept for [`handle_first`], and a second thread that sends it
    /// `signal` with `tgkill` whenever the returned function is called.
    fn first_thread_and_sender(process: &Arc<Process<P>>, signal: u64) -> impl FnMut() + use<'_> {
        let first = take_first_thread(process);
        let mut second = second_thread(process, &first, 0);
        let pid = u64::from(process.id().get());
        FIRST_THREADS
            .lock()
            .unwrap()
            .insert(process.id().get(), first);
        move || {
            assert_eq!(
                call_as(
                    process,
                    &mut second,
                    number::TGKILL,
                    [pid, pid, signal, 0, 0, 0]
                ),
                0
            );
        }
    }

    fn caught_with(restart: u64) -> u64 {
        restart
            | if signals::HAS_RESTORER {
                signals::flag::RESTORER
            } else {
                0
            }
    }

    #[test]
    fn a_signal_another_thread_sends_interrupts_a_sleep_which_writes_the_time_left() {
        use time::{TIMER_ABSTIME, clock};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let sigusr1 = 10;
        set_handler(&process, sigusr1, caught_with(signals::flag::RESTART), 0);
        let mut send = first_thread_and_sender(&process, sigusr1);
        let (req, rem) = (SCRATCH + 0x100, SCRATCH + 0x120);
        let one_second = 1_000_000_000;
        let monotonic = u64::from(clock::MONOTONIC as u32);
        let sleep = |arguments: [u64; 6], number| {
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            context.simulate_syscall(number, arguments, 0x40_0004);
            context
        };
        for (number, arguments) in [
            (number::NANOSLEEP, [req, rem, 0, 0, 0, 0]),
            (number::CLOCK_NANOSLEEP, [monotonic, 0, req, rem, 0, 0]),
        ] {
            write_timespec(&process, req, 1, 0);
            write_timespec(&process, rem, -1, -1);
            send();
            let mut context = sleep(arguments, number);
            let mut interrupted = context.clone();
            interrupted.set_syscall_result(Errno::EINTR.result());
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            let left = nanoseconds(read_timespec(&process, rem));
            assert!(
                left > one_second / 2 && left <= one_second,
                "{left} ns left"
            );
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            assert_eq!(context.registers(), interrupted.registers());
        }
        let far = nanoseconds(read_timespec(&process, rem)) + 1000 * one_second;
        write_timespec(&process, req, (far / one_second) as i64, 0);
        write_timespec(&process, rem, 7, 7);
        send();
        let mut context = sleep(
            [monotonic, TIMER_ABSTIME, req, rem, 0, 0],
            number::CLOCK_NANOSLEEP,
        );
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(read_timespec(&process, rem), (7, 7));
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_result(), Errno::EINTR.result());
        write_timespec(&process, req, 1, 0);
        send();
        let mut context = sleep([req, READ_ONLY, 0, 0, 0, 0], number::NANOSLEEP);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(context.syscall_result(), Errno::EFAULT.result());
        write_timespec(&process, req, 0, 0);
        send();
        let mut context = sleep([req, rem, 0, 0, 0, 0], number::NANOSLEEP);
        assert_eq!(
            handle_first(&process, &mut context, UserExit::Syscall),
            ControlFlow::Continue(())
        );
        assert_eq!(context.pc(), HANDLER);
        assert_eq!(
            handler_returns(&process, &mut context),
            ControlFlow::Continue(())
        );
        assert_eq!(
            context.syscall_result(),
            0,
            "a sleep whose time is up returns 0, a signal due or not"
        );
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn a_signal_interrupts_a_futex_wait_restarted_only_without_a_timeout_and_with_sa_restart() {
        use futex::op;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, _) = process_with_stacks(&kernel);
        let sigusr1 = 10;
        set_handler(&process, sigusr1, caught_with(0), 0);
        let mut send = first_thread_and_sender(&process, sigusr1);
        let (word, timeout) = (SCRATCH + 0x140, SCRATCH + 0x150);
        process
            .write_user_memory(word, &7u32.to_ne_bytes())
            .unwrap();
        write_timespec(&process, timeout, 5, 0);
        let wait = u64::from(op::WAIT | op::PRIVATE_FLAG);
        let call_size = if cfg!(target_arch = "x86_64") { 2 } else { 4 };
        for (restart, timeout, restarted) in [
            (0, 0, false),
            (signals::flag::RESTART, 0, true),
            (signals::flag::RESTART, timeout, false),
        ] {
            set_handler(&process, sigusr1, caught_with(restart), 0);
            send();
            let arguments = [word, wait, 7, timeout, 0, 0];
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            context.simulate_syscall(number::FUTEX, arguments, 0x40_0004);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            if restarted {
                assert_eq!(context.pc(), 0x40_0004 - call_size);
                assert_eq!(context.syscall_number(), number::FUTEX);
                assert_eq!(context.syscall_arguments(), arguments);
            } else {
                assert_eq!(context.pc(), 0x40_0004);
                assert_eq!(context.syscall_result(), Errno::EINTR.result());
            }
        }
        let past = SCRATCH + 0x160;
        write_timespec(&process, past, 0, 0);
        let wait_bitset = u64::from(op::WAIT_BITSET | op::PRIVATE_FLAG);
        for (arguments, expected) in [
            ([word, wait, 8, 0, 0, 0], Errno::EAGAIN),
            ([word, wait_bitset, 7, past, 0, u64::MAX], Errno::ETIMEDOUT),
        ] {
            send();
            let mut context = UserContext::new(0x40_0000, HANDLER_STACK_TOP - 0x30);
            context.simulate_syscall(number::FUTEX, arguments, 0x40_0004);
            assert_eq!(
                handle_first(&process, &mut context, UserExit::Syscall),
                ControlFlow::Continue(())
            );
            assert_eq!(context.pc(), HANDLER);
            assert_eq!(
                handler_returns(&process, &mut context),
                ControlFlow::Continue(())
            );
            assert_eq!(context.syscall_result(), expected.result());
        }
        assert_eq!(process.exit_status(), None);
    }

    #[test]
    fn a_signal_the_only_thread_blocks_stays_its_own() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let (process, pid) = process_with_stacks(&kernel);
        let mut first = take_first_thread(&process);
        let sigusr1 = 10u64;
        block_signals_in(&process, &mut first, 1 << (sigusr1 - 1));
        assert_eq!(
            call_as(
                &process,
                &mut first,
                number::KILL,
                [pid, sigusr1, 0, 0, 0, 0]
            ),
            0
        );
        assert_eq!(first.signal_state().pending, 1 << (sigusr1 - 1));
        assert_eq!(process.signals().lock().left_for(0), 0);
    }
    #[test]
    fn readlink_reads_only_links() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let readlink = |path: &[u8], size| {
            let path = at_path(&process, path);
            call(
                &process,
                number::READLINKAT,
                [FDCWD, path, READ_BUFFER, size, 0, 0],
            )
        };
        assert_eq!(readlink(b"/etc/hostname", 100), Errno::EINVAL.result());
        assert_eq!(readlink(b"/etc", 100), Errno::EINVAL.result());
        assert_eq!(readlink(b"/nowhere", 100), Errno::ENOENT.result());
        assert_eq!(readlink(b"", 100), Errno::ENOENT.result());
        assert_eq!(readlink(b"/etc/hostname", 0), Errno::EINVAL.result());
        assert_eq!(readlink(b"/etc/hostname", u64::MAX), Errno::EINVAL.result());
        let path = at_path(&process, b"/etc/hostname/x");
        assert_eq!(
            call(
                &process,
                number::READLINKAT,
                [FDCWD, path, READ_BUFFER, 100, 0, 0]
            ),
            Errno::ENOTDIR.result()
        );
    }

    #[test]
    fn access_checks_that_a_file_is_there_and_may_be_used() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let access = |path: &[u8], mode, flags| {
            let path = at_path(&process, path);
            call(
                &process,
                number::FACCESSAT2,
                [FDCWD, path, mode, flags, 0, 0],
            )
        };
        use stat::access_mode::{EXECUTE, READ, WRITE};
        let (read, write, execute) = (u64::from(READ), u64::from(WRITE), u64::from(EXECUTE));
        assert_eq!(access(b"/etc/hostname", 0, 0), 0);
        assert_eq!(access(b"/etc/hostname", read, 0), 0);
        assert_eq!(access(b"/etc", read | execute, 0), 0);
        assert_eq!(access(b"/etc/hostname", write, 0), Errno::EROFS.result());
        assert_eq!(access(b"/nowhere", 0, 0), Errno::ENOENT.result());
        assert_eq!(access(b"/etc/hostname", 8, 0), Errno::EINVAL.result());
        assert_eq!(access(b"/etc/hostname", read, 1), Errno::EINVAL.result());
        assert_eq!(access(b"", 0, 0), Errno::ENOENT.result());
        assert_eq!(access(b"", 0, stat::at::EMPTY_PATH), 0);
        assert_eq!(
            access(
                b"/etc/hostname",
                read,
                stat::AT_EACCESS | stat::at::SYMLINK_NOFOLLOW
            ),
            0
        );
        let path = at_path(&process, b"/etc/hostname");
        assert_eq!(
            call(&process, number::FACCESSAT, [FDCWD, path, read, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(
                &process,
                number::FACCESSAT2,
                [1, EMPTY, execute, stat::at::EMPTY_PATH, 0, 0]
            ),
            Errno::EACCES.result()
        );
    }

    #[test]
    fn statfs_and_fstatfs_describe_the_file_system_a_path_or_descriptor_is_on() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let word = |at: usize| {
            let bytes = read_bytes(&process, READ_BUFFER + at as u64, 8);
            u64::from_ne_bytes(bytes.try_into().unwrap())
        };
        let path = at_path(&process, b"/etc");
        assert_eq!(
            call(&process, number::STATFS, [path, READ_BUFFER, 0, 0, 0, 0]),
            0
        );
        assert_eq!(word(0), crate::vfs::magic::RAMFS);
        assert_eq!(word(64), 255);
        assert_eq!(word(80), statfs::flag::VALID | statfs::flag::RDONLY);
        let missing = at_path(&process, b"/nowhere");
        assert_eq!(
            call(&process, number::STATFS, [missing, READ_BUFFER, 0, 0, 0, 0]),
            Errno::ENOENT.result()
        );
        assert_eq!(
            call(&process, number::STATFS, [path, 0, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(
            call(&process, number::FSTATFS, [1, READ_BUFFER, 0, 0, 0, 0]),
            0
        );
        assert_eq!(word(0), crate::vfs::magic::TMPFS);
        assert_eq!(
            call(&process, number::FSTATFS, [99, READ_BUFFER, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
    }

    #[test]
    fn sysinfo_reports_memory_in_bytes_and_fadvise_takes_valid_advice_on_files() {
        use descriptors::open_flag;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let word = |at: usize| {
            let bytes = read_bytes(&process, READ_BUFFER + at as u64, 8);
            u64::from_ne_bytes(bytes.try_into().unwrap())
        };
        assert_eq!(
            call(&process, number::SYSINFO, [READ_BUFFER, 0, 0, 0, 0, 0]),
            0
        );
        assert!(
            word(32) >= word(40),
            "total memory is at least the free memory"
        );
        assert_eq!(word(104) & 0xffff_ffff, 1, "mem_unit is one byte");
        assert_eq!(
            call(&process, number::SYSINFO, [0, 0, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        let fd = open_at(&process, FDCWD, b"/etc/hostname", open_flag::CLOEXEC);
        let fadvise = |fd: u64, length: u64, advice: u64| {
            call(&process, number::FADVISE64, [fd, 0, length, advice, 0, 0])
        };
        for advice in 0..=files::advice::NOREUSE {
            assert_eq!(fadvise(fd, 0, advice), 0, "{advice}");
        }
        assert_eq!(fadvise(fd, 0, 6), Errno::EINVAL.result());
        assert_eq!(fadvise(fd, u64::MAX, 0), Errno::EINVAL.result());
        assert_eq!(fadvise(99, 0, 0), Errno::EBADF.result());
    }

    #[test]
    fn flock_takes_and_lets_go_of_a_whole_file_lock() {
        use file_locks::operation::{EXCLUSIVE, NONBLOCK, SHARED, UNLOCK};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let flock = |fd: u64, operation: u32| {
            call(
                &process,
                number::FLOCK,
                [fd, u64::from(operation), 0, 0, 0, 0],
            )
        };
        assert_eq!(flock(1, EXCLUSIVE | NONBLOCK), 0);
        assert_eq!(
            flock(2, SHARED | NONBLOCK),
            0,
            "the same description converts"
        );
        assert_eq!(flock(1, UNLOCK), 0);
        assert_eq!(flock(1, 99), Errno::EINVAL.result());
        assert_eq!(flock(1, SHARED | EXCLUSIVE), Errno::EINVAL.result());
        assert_eq!(flock(77, SHARED), Errno::EBADF.result());
        let mut described = [0u8; file_locks::FLOCK_SIZE];
        described[0..2].copy_from_slice(&file_locks::lock_type::WRITE.to_ne_bytes());
        process.write_user_memory(READ_BUFFER, &described).unwrap();
        let fcntl = |command: u32| {
            call(
                &process,
                number::FCNTL,
                [1, u64::from(command), READ_BUFFER, 0, 0, 0],
            )
        };
        assert_eq!(fcntl(file_locks::command::SETLK), 0);
        assert_eq!(fcntl(file_locks::command::GETLK), 0);
        assert_eq!(
            read_bytes(&process, READ_BUFFER, 2),
            file_locks::lock_type::UNLOCK.to_ne_bytes(),
            "a process's own lock is not in its way"
        );
        assert_eq!(
            call(&process, number::CLOSE, [3, 0, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
    }

    #[test]
    fn a_privileged_caller_renames_the_machine_for_uname() {
        let _frames = frames_for_test();
        let _names = identity::tests::NAMES_TEST.lock().unwrap();
        identity::forget_changed_names();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let name = at_path(&process, b"eggbox");
        assert_eq!(
            call(&process, number::SETHOSTNAME, [name, 6, 0, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::UNAME, [READ_BUFFER, 0, 0, 0, 0, 0]),
            0
        );
        let field = identity::utsname::FIELD as u64;
        assert_eq!(read_bytes(&process, READ_BUFFER + field, 7), b"eggbox\0");
        assert_eq!(
            call(&process, number::SETDOMAINNAME, [name, 3, 0, 0, 0, 0]),
            0
        );
        assert_eq!(
            call(&process, number::UNAME, [READ_BUFFER, 0, 0, 0, 0, 0]),
            0
        );
        assert_eq!(read_bytes(&process, READ_BUFFER + 5 * field, 4), b"egg\0");
        for length in [65, u64::MAX] {
            assert_eq!(
                call(&process, number::SETHOSTNAME, [name, length, 0, 0, 0, 0]),
                Errno::EINVAL.result()
            );
        }
        assert_eq!(
            call(&process, number::SETHOSTNAME, [UNMAPPED, 4, 0, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        assert_eq!(call(&process, number::SETUID, [1000, 0, 0, 0, 0, 0]), 0);
        assert_eq!(
            call(&process, number::SETHOSTNAME, [name, 6, 0, 0, 0, 0]),
            Errno::EPERM.result()
        );
        identity::forget_changed_names();
    }

    #[test]
    fn proc_describes_a_running_program_and_self_names_its_directory() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process_for_status(&kernel);
        let id = process.id().get();
        let mut thread = take_first_thread(&process);
        wait::record_running(&process, &thread, |_, _| {});
        let path = at_path(&process, b"/usr/libexec/program");
        let (_, current) = exec_in(&process, &mut thread, number::EXECVE, [path, 0, 0, 0, 0, 0]);
        let proc: &'static proc::Proc<P> = Box::leak(Box::new(proc::Proc::new()));
        let root = crate::vfs::Node::root_of(proc);
        let walk = |path: &[u8], follow| crate::vfs::resolve_as(root, root, path, id, follow);
        let text = |path: &[u8]| {
            let bytes = walk(path, true).unwrap().generate().unwrap().unwrap();
            alloc::string::String::from_utf8(bytes).unwrap()
        };
        let status = text(b"self/status");
        assert!(status.starts_with("Name:\tprogram\n"), "{status}");
        assert!(
            status.contains(&alloc::format!("\nPid:\t{id}\n")),
            "{status}"
        );
        assert!(status.contains("\nUid:\t0\t0\t0\t0\n"), "{status}");
        assert!(status.contains("\nThreads:\t1\n"), "{status}");
        assert_eq!(text(b"self/cmdline"), "\0");
        let exe = walk(b"self/exe", false).unwrap();
        assert_eq!(exe.kind(), crate::vfs::NodeKind::SymbolicLink);
        assert_eq!(
            exe.link_target(id).as_deref(),
            Some(&b"/usr/libexec/program"[..])
        );
        let maps = text(b"self/maps");
        assert!(
            maps.lines()
                .any(|line| line.ends_with(" /usr/libexec/program")),
            "{maps}"
        );
        assert!(
            maps.lines().any(|line| line.ends_with(" [stack]")),
            "{maps}"
        );
        let stat = text(alloc::format!("{id}/stat").as_bytes());
        assert!(
            stat.starts_with(&alloc::format!("{id} (program) R ")),
            "{stat}"
        );
        assert_eq!(stat.trim_end().split(' ').count(), 52, "{stat}");
        let descriptors = walk(b"self/fd", true).unwrap();
        let names: alloc::vec::Vec<alloc::vec::Vec<u8>> = (0..)
            .map_while(|position| descriptors.entry(position))
            .map(|(name, _, _)| name.as_bytes().to_vec())
            .collect();
        assert_eq!(names, [&b"0"[..], b"1", b"2"]);
        let standard_output = walk(b"self/fd/1", false).unwrap();
        assert_eq!(
            standard_output.link_target(id).as_deref(),
            Some(&b"/dev/console"[..])
        );
        assert_eq!(walk(b"self/fd/3", false), Err(crate::vfs::Error::NotFound));
        assert_eq!(
            crate::vfs::resolve(root, root, b"self/status"),
            Err(crate::vfs::Error::NotFound),
            "the kernel has no directory of its own"
        );
        let listed = (0..)
            .map_while(|position| root.entry(position))
            .any(|(name, _, _)| name.as_bytes() == alloc::format!("{id}").as_bytes());
        assert!(listed);
        drop(current);
    }

    struct EventProcess {
        process: Arc<Process<P>>,
        writable: u64,
    }

    impl EventProcess {
        fn call(&self, number: u64, arguments: [u64; 6]) -> u64 {
            call(&self.process, number, arguments)
        }

        fn put(&self, offset: u64, bytes: &[u8]) {
            self.process
                .write_user_memory(self.writable + offset, bytes)
                .unwrap();
        }

        fn get(&self, offset: u64, length: usize) -> alloc::vec::Vec<u8> {
            read_bytes(&self.process, self.writable + offset, length)
        }

        fn word(&self, offset: u64) -> u64 {
            u64::from_ne_bytes(self.get(offset, 8).try_into().unwrap())
        }

        fn read(&self, fd: u64, offset: u64, count: u64) -> u64 {
            self.call(number::READ, [fd, self.writable + offset, count, 0, 0, 0])
        }

        fn write_word(&self, fd: u64, value: u64) -> u64 {
            self.put(0, &value.to_ne_bytes());
            self.call(number::WRITE, [fd, self.writable, 8, 0, 0, 0])
        }

        fn epoll_ctl(&self, epfd: u64, op: u32, fd: u64, events: u32, data: u64) -> u64 {
            let mut event = [0u8; epoll::EVENT_SIZE];
            event[..4].copy_from_slice(&events.to_ne_bytes());
            event[epoll::EVENT_SIZE - 8..].copy_from_slice(&data.to_ne_bytes());
            self.put(EPOLL_EVENT, &event);
            let event = self.writable + EPOLL_EVENT;
            self.call(number::EPOLL_CTL, [epfd, u64::from(op), fd, event, 0, 0])
        }

        fn epoll_pwait(&self, epfd: u64, max: u64, timeout: i32) -> u64 {
            let events = self.writable + EVENTS_OUT;
            let timeout = u64::from(timeout as u32);
            self.call(number::EPOLL_PWAIT, [epfd, events, max, timeout, 0, 0])
        }

        fn reported(&self, index: u64) -> (u32, u64) {
            let at = EVENTS_OUT + index * epoll::EVENT_SIZE as u64;
            let event = self.get(at, epoll::EVENT_SIZE);
            (
                u32::from_ne_bytes(event[..4].try_into().unwrap()),
                u64::from_ne_bytes(event[epoll::EVENT_SIZE - 8..].try_into().unwrap()),
            )
        }
    }

    const EPOLL_EVENT: u64 = 64;
    const ITIMERSPEC: u64 = 128;
    const SIGSET: u64 = 192;
    const EVENTS_OUT: u64 = 256;
    const PIPE_FDS: u64 = 512;
    const FD_SETS: u64 = 528;
    const TIMESPEC: u64 = 560;
    const SIGSET_ARGUMENTS: u64 = 600;
    const RECORD: u64 = 1024;

    fn event_process(kernel: &PageTables<<P as Platform>::PageTableFormat>) -> EventProcess {
        EventProcess {
            process: process_with_iovecs(kernel, b"", &[]),
            writable: DATA + 4096,
        }
    }

    #[test]
    fn eventfd_counts_writes_and_takes_them_whole_or_one_at_a_time() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let nonblocking = u64::from(eventfd::flag::NONBLOCK);
        let counter = test.call(number::EVENTFD2, [0, nonblocking, 0, 0, 0, 0]);
        assert!(counter < 1024, "{counter:#x}");
        assert_eq!(test.read(counter, 8, 8), Errno::EAGAIN.result());
        assert_eq!(test.write_word(counter, 3), 8);
        assert_eq!(test.write_word(counter, 4), 8);
        assert_eq!(test.write_word(counter, u64::MAX), Errno::EINVAL.result());
        assert_eq!(
            test.call(number::WRITE, [counter, test.writable, 7, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(test.read(counter, 8, 7), Errno::EINVAL.result());
        assert_eq!(test.read(counter, 8, 64), 8);
        assert_eq!(test.word(8), 7);
        assert_eq!(test.read(counter, 8, 8), Errno::EAGAIN.result());
        assert_eq!(test.write_word(counter, eventfd::MAX_VALUE), 8);
        assert_eq!(test.write_word(counter, 1), Errno::EAGAIN.result());
        let semaphore = u64::from(eventfd::flag::SEMAPHORE | eventfd::flag::NONBLOCK);
        let tokens = test.call(number::EVENTFD2, [2, semaphore, 0, 0, 0, 0]);
        assert_eq!(test.read(tokens, 8, 8), 8);
        assert_eq!(test.word(8), 1);
        assert_eq!(test.read(tokens, 8, 8), 8);
        assert_eq!(test.read(tokens, 8, 8), Errno::EAGAIN.result());
        assert_eq!(
            test.call(number::EVENTFD2, [0, 4, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::LSEEK, [counter, 0, 0, 0, 0, 0]),
            Errno::ESPIPE.result()
        );
    }

    #[test]
    fn epoll_reports_an_eventfd_level_and_edge_triggered_and_checks_its_arguments() {
        use epoll::{event, op};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let epfd = test.call(number::EPOLL_CREATE1, [0, 0, 0, 0, 0, 0]);
        let nonblocking = u64::from(eventfd::flag::NONBLOCK);
        let counter = test.call(number::EVENTFD2, [0, nonblocking, 0, 0, 0, 0]);
        assert_eq!(
            test.call(number::EPOLL_CREATE1, [1, 0, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(test.epoll_ctl(epfd, op::ADD, counter, event::IN, 0x1234), 0);
        let errors = [
            (epfd, op::ADD, counter, event::IN, Errno::EEXIST),
            (epfd, op::MOD, 1, event::IN, Errno::ENOENT),
            (epfd, op::DEL, 1, 0, Errno::ENOENT),
            (counter, op::ADD, 1, event::IN, Errno::EINVAL),
            (epfd, op::ADD, epfd, event::IN, Errno::EINVAL),
            (epfd, op::ADD, 99, event::IN, Errno::EBADF),
            (99, op::ADD, counter, event::IN, Errno::EBADF),
            (epfd, 7, counter, event::IN, Errno::EINVAL),
            (
                epfd,
                op::ADD,
                1,
                event::EXCLUSIVE | event::ONESHOT,
                Errno::EINVAL,
            ),
            (
                epfd,
                op::MOD,
                counter,
                event::EXCLUSIVE | event::IN,
                Errno::EINVAL,
            ),
        ];
        for (epoll, operation, fd, events, errno) in errors {
            assert_eq!(
                test.epoll_ctl(epoll, operation, fd, events, 0),
                errno.result(),
                "{operation} {fd} {events:#x}"
            );
        }
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 0);
        assert_eq!(test.write_word(counter, 1), 8);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 1);
        assert_eq!(test.reported(0), (event::IN, 0x1234));
        assert_eq!(test.epoll_pwait(epfd, 4, -1), 1);
        assert_eq!(test.epoll_pwait(epfd, 0, 0), Errno::EINVAL.result());
        assert_eq!(test.epoll_pwait(counter, 4, 0), Errno::EINVAL.result());
        assert_eq!(test.epoll_pwait(99, 4, 0), Errno::EBADF.result());
        let read_only = DATA;
        assert_eq!(
            test.call(number::EPOLL_PWAIT, [epfd, read_only, 4, 0, 0, 0]),
            Errno::EFAULT.result()
        );
        let edge = event::IN | event::ET;
        assert_eq!(test.epoll_ctl(epfd, op::MOD, counter, edge, 7), 0);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 1);
        assert_eq!(test.reported(0), (event::IN, 7));
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 0);
        assert_eq!(test.write_word(counter, 1), 8);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 1);
        assert_eq!(test.epoll_ctl(epfd, op::DEL, counter, 0, 0), 0);
        assert_eq!(test.write_word(counter, 1), 8);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 0);
        let started = <P as Platform>::clock_ticks();
        assert_eq!(test.epoll_pwait(epfd, 4, 2), 0);
        let waited = <P as Platform>::clock_ticks() - started;
        assert!(
            waited >= <P as Platform>::clock_frequency() / 500,
            "{waited}"
        );
    }

    #[test]
    fn epoll_instances_nest_without_loops_and_poll_reports_them_readable() {
        use epoll::{event, op};
        use poll::event::IN;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let outer = test.call(number::EPOLL_CREATE1, [0, 0, 0, 0, 0, 0]);
        let inner = test.call(number::EPOLL_CREATE1, [0, 0, 0, 0, 0, 0]);
        let nonblocking = u64::from(eventfd::flag::NONBLOCK);
        let counter = test.call(number::EVENTFD2, [0, nonblocking, 0, 0, 0, 0]);
        assert_eq!(test.epoll_ctl(inner, op::ADD, counter, event::IN, 1), 0);
        assert_eq!(test.epoll_ctl(outer, op::ADD, inner, event::IN, 2), 0);
        assert_eq!(
            test.epoll_ctl(inner, op::ADD, outer, event::IN, 3),
            Errno::ELOOP.result()
        );
        assert_eq!(
            test.epoll_ctl(outer, op::ADD, inner, event::IN | event::EXCLUSIVE, 3),
            Errno::EINVAL.result()
        );
        let fds = test.writable + 640;
        test.put(640, &pollfds(&[(outer as i32, IN)]));
        let timeout = test.writable + TIMESPEC;
        test.put(TIMESPEC, &[0; 16]);
        assert_eq!(test.call(number::PPOLL, [fds, 1, timeout, 0, 0, 0]), 0);
        assert_eq!(test.write_word(counter, 5), 8);
        assert_eq!(test.call(number::PPOLL, [fds, 1, timeout, 0, 0, 0]), 1);
        assert_eq!(test.epoll_pwait(outer, 4, 0), 1);
        assert_eq!(test.reported(0), (event::IN, 2));
        assert_eq!(test.epoll_pwait(inner, 4, 0), 1);
        assert_eq!(test.reported(0), (event::IN, 1));
    }

    #[test]
    fn timerfd_expires_on_time_and_reports_its_setting() {
        use epoll::{event, op};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let nonblocking = u64::from(timerfd::flag::NONBLOCK);
        let monotonic = time::clock::MONOTONIC as u64;
        let timer = test.call(number::TIMERFD_CREATE, [monotonic, nonblocking, 0, 0, 0, 0]);
        assert_eq!(
            test.call(number::TIMERFD_CREATE, [99, 0, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::TIMERFD_CREATE, [monotonic, 1, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        let two_milliseconds = 2_000_000u64;
        let fifty_milliseconds = 50_000_000u64;
        let mut setting = [0u8; 32];
        setting[8..16].copy_from_slice(&two_milliseconds.to_ne_bytes());
        setting[24..32].copy_from_slice(&fifty_milliseconds.to_ne_bytes());
        test.put(ITIMERSPEC, &setting);
        let new_value = test.writable + ITIMERSPEC;
        let settime = |flags: u64, old: u64| {
            test.call(
                number::TIMERFD_SETTIME,
                [timer, flags, new_value, old, 0, 0],
            )
        };
        assert_eq!(settime(2, 0), Errno::EINVAL.result());
        assert_eq!(settime(4, 0), Errno::EINVAL.result());
        assert_eq!(settime(0, 0), 0);
        assert_eq!(test.read(timer, 8, 8), Errno::EAGAIN.result());
        let epfd = test.call(number::EPOLL_CREATE1, [0, 0, 0, 0, 0, 0]);
        assert_eq!(test.epoll_ctl(epfd, op::ADD, timer, event::IN, 9), 0);
        assert_eq!(test.epoll_pwait(epfd, 4, 1000), 1);
        assert_eq!(test.reported(0), (event::IN, 9));
        assert_eq!(test.read(timer, 8, 8), 8);
        assert!(test.word(8) >= 1);
        let current = test.writable + ITIMERSPEC + 32;
        assert_eq!(
            test.call(number::TIMERFD_GETTIME, [timer, current, 0, 0, 0, 0]),
            0
        );
        let reported = test.get(ITIMERSPEC + 32, 32);
        assert_eq!(&reported[8..16], &two_milliseconds.to_ne_bytes());
        test.put(ITIMERSPEC, &[0; 32]);
        assert_eq!(settime(0, current), 0);
        assert_eq!(
            &test.get(ITIMERSPEC + 32, 16)[8..],
            &two_milliseconds.to_ne_bytes()
        );
        assert_eq!(test.read(timer, 8, 8), Errno::EAGAIN.result());
        assert_eq!(test.epoll_pwait(epfd, 4, 3), 0);
        assert_eq!(
            test.call(number::TIMERFD_GETTIME, [epfd, current, 0, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::TIMERFD_GETTIME, [99, current, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
    }

    #[test]
    fn signalfd_reads_a_blocked_signal_and_poll_and_epoll_see_it_pending() {
        use epoll::{event, op};
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let sigusr1 = u64::from(signal::SIGUSR1);
        test.put(SIGSET, &(1u64 << (sigusr1 - 1)).to_ne_bytes());
        let set = test.writable + SIGSET;
        assert_eq!(test.call(number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0]), 0);
        let nonblocking = u64::from(signalfd::flag::NONBLOCK);
        let minus_one = u64::from(u32::MAX);
        let signal_file = test.call(number::SIGNALFD4, [minus_one, set, 8, nonblocking, 0, 0]);
        assert!(signal_file < 1024, "{signal_file:#x}");
        assert_eq!(
            test.call(number::SIGNALFD4, [minus_one, set, 7, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::SIGNALFD4, [minus_one, set, 8, 1, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::SIGNALFD4, [1, set, 8, 0, 0, 0]),
            Errno::EINVAL.result()
        );
        assert_eq!(
            test.call(number::SIGNALFD4, [signal_file, set, 8, 0, 0, 0]),
            signal_file
        );
        assert_eq!(test.read(signal_file, RECORD, 128), Errno::EAGAIN.result());
        assert_eq!(test.read(signal_file, RECORD, 127), Errno::EINVAL.result());
        let epfd = test.call(number::EPOLL_CREATE1, [0, 0, 0, 0, 0, 0]);
        assert_eq!(test.epoll_ctl(epfd, op::ADD, signal_file, event::IN, 3), 0);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 0);
        let pid = u64::from(test.process.id().get());
        assert_eq!(test.call(number::KILL, [pid, sigusr1, 0, 0, 0, 0]), 0);
        assert_eq!(test.process.exit_status(), None);
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 1);
        assert_eq!(test.reported(0), (event::IN, 3));
        assert_eq!(test.read(signal_file, RECORD, 256), 128);
        let record = test.get(RECORD, 128);
        assert_eq!(&record[..4], &signal::SIGUSR1.to_ne_bytes());
        assert_eq!(&record[8..12], &signals::code::USER.to_ne_bytes());
        assert_eq!(&record[12..16], &test.process.id().get().to_ne_bytes());
        assert_eq!(test.read(signal_file, RECORD, 128), Errno::EAGAIN.result());
        assert_eq!(test.epoll_pwait(epfd, 4, 0), 0);
    }

    fn fd_set(fds: &[u64]) -> [u8; 8] {
        fds.iter().fold(0u64, |set, fd| set | 1 << fd).to_ne_bytes()
    }

    #[test]
    fn pselect6_reports_a_pipe_s_ends_and_writes_the_time_left_back() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let test = event_process(&kernel);
        let pipe_fds = test.writable + PIPE_FDS;
        assert_eq!(test.call(number::PIPE2, [pipe_fds, 0, 0, 0, 0, 0]), 0);
        let fds = test.get(PIPE_FDS, 8);
        let reading = u64::from(u32::from_ne_bytes(fds[..4].try_into().unwrap()));
        let writing = u64::from(u32::from_ne_bytes(fds[4..].try_into().unwrap()));
        let nfds = reading.max(writing) + 1;
        let (read_set, write_set) = (test.writable + FD_SETS, test.writable + FD_SETS + 8);
        let timeout = test.writable + TIMESPEC;
        let pselect6 = |read: &[u64], write: &[u64], seconds: i64, nanoseconds: i64| {
            test.put(FD_SETS, &fd_set(read));
            test.put(FD_SETS + 8, &fd_set(write));
            let mut time = [0u8; 16];
            time[..8].copy_from_slice(&seconds.to_ne_bytes());
            time[8..].copy_from_slice(&nanoseconds.to_ne_bytes());
            test.put(TIMESPEC, &time);
            let sets = [nfds, read_set, write_set, 0, timeout, 0];
            (
                test.call(number::PSELECT6, sets),
                test.get(FD_SETS, 8),
                test.get(FD_SETS + 8, 8),
            )
        };
        let (ready, read, write) = pselect6(&[reading], &[writing], 0, 0);
        assert_eq!(ready, 1);
        assert_eq!(read, fd_set(&[]));
        assert_eq!(write, fd_set(&[writing]));
        assert_eq!(test.write_word(writing, 0x41), 8);
        let (ready, read, write) = pselect6(&[reading], &[writing], 0, 0);
        assert_eq!(ready, 2);
        assert_eq!(read, fd_set(&[reading]));
        assert_eq!(write, fd_set(&[writing]));
        assert_eq!(test.read(reading, 8, 8), 8);
        let (ready, read, _) = pselect6(&[reading], &[], 0, 2_000_000);
        assert_eq!(ready, 0);
        assert_eq!(read, fd_set(&[]));
        assert_eq!(test.get(TIMESPEC, 16), [0u8; 16]);
        let not_open = 50;
        test.put(FD_SETS, &fd_set(&[not_open]));
        assert_eq!(
            test.call(number::PSELECT6, [not_open + 1, read_set, 0, 0, 0, 0]),
            Errno::EBADF.result()
        );
        assert_eq!(
            test.call(
                number::PSELECT6,
                [u64::from(u32::MAX), read_set, 0, 0, 0, 0]
            ),
            Errno::EINVAL.result()
        );
        let mut arguments = [0u8; 16];
        arguments[..8].copy_from_slice(&(test.writable + SIGSET).to_ne_bytes());
        arguments[8..].copy_from_slice(&7u64.to_ne_bytes());
        test.put(SIGSET_ARGUMENTS, &arguments);
        let sig = test.writable + SIGSET_ARGUMENTS;
        assert_eq!(
            test.call(number::PSELECT6, [nfds, read_set, 0, 0, timeout, sig]),
            Errno::EINVAL.result()
        );
        #[cfg(target_arch = "x86_64")]
        {
            test.put(FD_SETS, &fd_set(&[reading]));
            let mut time = [0u8; 16];
            time[8..].copy_from_slice(&1_500i64.to_ne_bytes());
            test.put(TIMESPEC, &time);
            assert_eq!(
                test.call(number::SELECT, [nfds, read_set, 0, 0, timeout, 0]),
                0
            );
            assert_eq!(test.get(TIMESPEC, 16), [0u8; 16]);
            time[8..].copy_from_slice(&(-1i64).to_ne_bytes());
            test.put(TIMESPEC, &time);
            assert_eq!(
                test.call(number::SELECT, [nfds, read_set, 0, 0, timeout, 0]),
                Errno::EINVAL.result()
            );
        }
    }
}
