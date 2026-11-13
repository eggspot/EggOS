//! Linux system calls, made directly with the architecture's instruction.

use core::arch::asm;

/// Linux's system-call numbers: x86_64's own table.
#[cfg(target_arch = "x86_64")]
pub mod number {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const CLOSE: usize = 3;
    pub const FSTAT: usize = 5;
    pub const MMAP: usize = 9;
    pub const MPROTECT: usize = 10;
    pub const MUNMAP: usize = 11;
    pub const BRK: usize = 12;
    pub const RT_SIGACTION: usize = 13;
    pub const RT_SIGRETURN: usize = 15;
    pub const IOCTL: usize = 16;
    pub const GETPID: usize = 39;
    pub const CLONE: usize = 56;
    pub const EXECVE: usize = 59;
    pub const UMASK: usize = 95;
    pub const TIMES: usize = 100;
    pub const WAIT4: usize = 61;
    pub const UNAME: usize = 63;
    pub const SETSID: usize = 112;
    pub const RT_SIGSUSPEND: usize = 130;
    pub const SIGALTSTACK: usize = 131;
    pub const ARCH_PRCTL: usize = 158;
    pub const REBOOT: usize = 169;
    pub const GETTID: usize = 186;
    pub const FUTEX: usize = 202;
    pub const SET_TID_ADDRESS: usize = 218;
    pub const CLOCK_SETTIME: usize = 227;
    pub const CLOCK_GETTIME: usize = 228;
    pub const CLOCK_NANOSLEEP: usize = 230;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT: usize = 257;
    pub const PPOLL: usize = 271;
    pub const SET_ROBUST_LIST: usize = 273;
    pub const PRLIMIT64: usize = 302;
    pub const GETRANDOM: usize = 318;
    pub const RT_SIGPROCMASK: usize = 14;
    pub const GETCWD: usize = 79;
    pub const CHDIR: usize = 80;
    pub const GETEUID: usize = 107;
    pub const SETPGID: usize = 109;
    pub const GETPGID: usize = 121;
    pub const NEWFSTATAT: usize = 262;
    pub const GETDENTS64: usize = 217;
    pub const STATX: usize = 332;
    pub const DUP3: usize = 292;
    pub const CLOSE_RANGE: usize = 436;
    pub const FCNTL: usize = 72;
    pub const PIPE2: usize = 293;
    pub const MKDIRAT: usize = 258;
    pub const UNLINKAT: usize = 263;
    pub const UTIMENSAT: usize = 280;
    pub const RENAMEAT2: usize = 316;
    pub const SYNC: usize = 162;
    pub const SCHED_GETAFFINITY: usize = 204;
    pub const SCHED_SETAFFINITY: usize = 203;
    pub const FSYNC: usize = 74;
    pub const KILL: usize = 62;
    pub const SOCKET: usize = 41;
    pub const CONNECT: usize = 42;
    pub const SENDTO: usize = 44;
    pub const RECVFROM: usize = 45;
    pub const GETSOCKNAME: usize = 51;
    pub const SETSOCKOPT: usize = 54;
    pub const SENDMSG: usize = 46;
    pub const RECVMSG: usize = 47;
    pub const BIND: usize = 49;
    pub const LISTEN: usize = 50;
    pub const ACCEPT4: usize = 288;
    pub const MEMFD_CREATE: usize = 319;
    pub const FTRUNCATE: usize = 77;
    pub const PREAD64: usize = 17;
    pub const PWRITE64: usize = 18;
    pub const READLINKAT: usize = 267;
    pub const SYMLINKAT: usize = 266;
    pub const LINKAT: usize = 265;
    pub const OPENAT2: usize = 437;
    pub const FCHMODAT: usize = 268;
    pub const FCHMODAT2: usize = 452;
    pub const STATFS: usize = 137;
    pub const FSTATFS: usize = 138;
    pub const LSEEK: usize = 8;
    pub const FACCESSAT: usize = 269;
}

/// Linux's system-call numbers: the generic table AArch64, RISC-V and LoongArch share.
#[cfg(any(
    target_arch = "aarch64",
    target_arch = "loongarch64",
    target_arch = "riscv64"
))]
pub mod number {
    pub const IOCTL: usize = 29;
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const PPOLL: usize = 73;
    pub const FSTAT: usize = 80;
    pub const EXIT_GROUP: usize = 94;
    pub const SET_TID_ADDRESS: usize = 96;
    pub const FUTEX: usize = 98;
    pub const SET_ROBUST_LIST: usize = 99;
    pub const CLOCK_SETTIME: usize = 112;
    pub const CLOCK_GETTIME: usize = 113;
    pub const CLOCK_NANOSLEEP: usize = 115;
    pub const SIGALTSTACK: usize = 132;
    pub const RT_SIGSUSPEND: usize = 133;
    pub const RT_SIGACTION: usize = 134;
    pub const RT_SIGRETURN: usize = 139;
    pub const REBOOT: usize = 142;
    pub const SETSID: usize = 157;
    pub const UNAME: usize = 160;
    pub const GETPID: usize = 172;
    pub const GETTID: usize = 178;
    pub const BRK: usize = 214;
    pub const MUNMAP: usize = 215;
    pub const CLONE: usize = 220;
    pub const EXECVE: usize = 221;
    pub const UMASK: usize = 166;
    pub const TIMES: usize = 153;
    pub const MMAP: usize = 222;
    pub const MPROTECT: usize = 226;
    pub const WAIT4: usize = 260;
    pub const PRLIMIT64: usize = 261;
    pub const GETRANDOM: usize = 278;
    pub const GETCWD: usize = 17;
    pub const CHDIR: usize = 49;
    pub const NEWFSTATAT: usize = 79;
    pub const RT_SIGPROCMASK: usize = 135;
    pub const SETPGID: usize = 154;
    pub const GETPGID: usize = 155;
    pub const GETEUID: usize = 175;
    pub const GETDENTS64: usize = 61;
    pub const STATX: usize = 291;
    pub const DUP3: usize = 24;
    pub const CLOSE_RANGE: usize = 436;
    pub const FCNTL: usize = 25;
    pub const PIPE2: usize = 59;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
    pub const UTIMENSAT: usize = 88;
    pub const RENAMEAT2: usize = 276;
    pub const SYNC: usize = 81;
    pub const SCHED_SETAFFINITY: usize = 122;
    pub const SCHED_GETAFFINITY: usize = 123;
    pub const FSYNC: usize = 82;
    pub const KILL: usize = 129;
    pub const SOCKET: usize = 198;
    pub const BIND: usize = 200;
    pub const LISTEN: usize = 201;
    pub const CONNECT: usize = 203;
    pub const GETSOCKNAME: usize = 204;
    pub const SETSOCKOPT: usize = 208;
    pub const SENDTO: usize = 206;
    pub const RECVFROM: usize = 207;
    pub const SENDMSG: usize = 211;
    pub const RECVMSG: usize = 212;
    pub const ACCEPT4: usize = 242;
    pub const MEMFD_CREATE: usize = 279;
    pub const FTRUNCATE: usize = 46;
    pub const PREAD64: usize = 67;
    pub const PWRITE64: usize = 68;
    pub const READLINKAT: usize = 78;
    pub const SYMLINKAT: usize = 36;
    pub const LINKAT: usize = 37;
    pub const OPENAT2: usize = 437;
    pub const FCHMODAT: usize = 53;
    pub const FCHMODAT2: usize = 452;
    pub const STATFS: usize = 43;
    pub const FSTATFS: usize = 44;
    pub const LSEEK: usize = 62;
    pub const FACCESSAT: usize = 48;
}

/// Linux's system-call numbers on 64-bit PowerPC (D304).
#[cfg(target_arch = "powerpc64")]
pub mod number {
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const CLOSE: usize = 6;
    pub const FSTAT: usize = 108;
    pub const MMAP: usize = 90;
    pub const MPROTECT: usize = 125;
    pub const MUNMAP: usize = 91;
    pub const BRK: usize = 45;
    pub const RT_SIGACTION: usize = 173;
    pub const RT_SIGRETURN: usize = 172;
    pub const IOCTL: usize = 54;
    pub const GETPID: usize = 20;
    pub const CLONE: usize = 120;
    pub const EXECVE: usize = 11;
    pub const UMASK: usize = 60;
    pub const TIMES: usize = 43;
    pub const WAIT4: usize = 114;
    pub const UNAME: usize = 122;
    pub const SETSID: usize = 66;
    pub const RT_SIGSUSPEND: usize = 178;
    pub const SIGALTSTACK: usize = 185;
    pub const REBOOT: usize = 88;
    pub const GETTID: usize = 207;
    pub const FUTEX: usize = 221;
    pub const SET_TID_ADDRESS: usize = 232;
    pub const CLOCK_SETTIME: usize = 245;
    pub const CLOCK_GETTIME: usize = 246;
    pub const CLOCK_NANOSLEEP: usize = 248;
    pub const EXIT_GROUP: usize = 234;
    pub const OPENAT: usize = 286;
    pub const PPOLL: usize = 281;
    pub const SET_ROBUST_LIST: usize = 300;
    pub const PRLIMIT64: usize = 325;
    pub const GETRANDOM: usize = 359;
    pub const RT_SIGPROCMASK: usize = 174;
    pub const GETCWD: usize = 182;
    pub const CHDIR: usize = 12;
    pub const GETEUID: usize = 49;
    pub const SETPGID: usize = 57;
    pub const GETPGID: usize = 132;
    pub const NEWFSTATAT: usize = 291;
    pub const GETDENTS64: usize = 202;
    pub const STATX: usize = 383;
    pub const DUP3: usize = 316;
    pub const CLOSE_RANGE: usize = 436;
    pub const FCNTL: usize = 55;
    pub const PIPE2: usize = 317;
    pub const MKDIRAT: usize = 287;
    pub const UNLINKAT: usize = 292;
    pub const UTIMENSAT: usize = 304;
    pub const RENAMEAT2: usize = 357;
    pub const SYNC: usize = 36;
    pub const SCHED_GETAFFINITY: usize = 223;
    pub const SCHED_SETAFFINITY: usize = 222;
    pub const FSYNC: usize = 118;
    pub const KILL: usize = 37;
    pub const SOCKET: usize = 326;
    pub const CONNECT: usize = 328;
    pub const SENDTO: usize = 335;
    pub const RECVFROM: usize = 337;
    pub const GETSOCKNAME: usize = 331;
    pub const SETSOCKOPT: usize = 339;
    pub const SENDMSG: usize = 341;
    pub const RECVMSG: usize = 342;
    pub const BIND: usize = 327;
    pub const LISTEN: usize = 329;
    pub const ACCEPT4: usize = 344;
    pub const MEMFD_CREATE: usize = 360;
    pub const FTRUNCATE: usize = 93;
    pub const PREAD64: usize = 179;
    pub const PWRITE64: usize = 180;
    pub const READLINKAT: usize = 296;
    pub const SYMLINKAT: usize = 295;
    pub const LINKAT: usize = 294;
    pub const OPENAT2: usize = 437;
    pub const FCHMODAT: usize = 297;
    pub const FCHMODAT2: usize = 452;
    pub const STATFS: usize = 99;
    pub const FSTATFS: usize = 100;
    pub const LSEEK: usize = 19;
    pub const FACCESSAT: usize = 298;
}

/// Linux's system-call numbers on s390x (D323).
#[cfg(target_arch = "s390x")]
pub mod number {
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const CLOSE: usize = 6;
    pub const FSTAT: usize = 108;
    pub const MMAP: usize = 90;
    pub const MPROTECT: usize = 125;
    pub const MUNMAP: usize = 91;
    pub const BRK: usize = 45;
    pub const RT_SIGACTION: usize = 174;
    pub const RT_SIGRETURN: usize = 173;
    pub const IOCTL: usize = 54;
    pub const GETPID: usize = 20;
    pub const CLONE: usize = 120;
    pub const EXECVE: usize = 11;
    pub const UMASK: usize = 60;
    pub const TIMES: usize = 43;
    pub const WAIT4: usize = 114;
    pub const UNAME: usize = 122;
    pub const SETSID: usize = 66;
    pub const RT_SIGSUSPEND: usize = 179;
    pub const SIGALTSTACK: usize = 186;
    pub const REBOOT: usize = 88;
    pub const GETTID: usize = 236;
    pub const FUTEX: usize = 238;
    pub const SET_TID_ADDRESS: usize = 252;
    pub const CLOCK_SETTIME: usize = 259;
    pub const CLOCK_GETTIME: usize = 260;
    pub const CLOCK_NANOSLEEP: usize = 262;
    pub const EXIT_GROUP: usize = 248;
    pub const OPENAT: usize = 288;
    pub const PPOLL: usize = 302;
    pub const SET_ROBUST_LIST: usize = 304;
    pub const PRLIMIT64: usize = 334;
    pub const GETRANDOM: usize = 349;
    pub const RT_SIGPROCMASK: usize = 175;
    pub const GETCWD: usize = 183;
    pub const CHDIR: usize = 12;
    pub const GETEUID: usize = 201;
    pub const SETPGID: usize = 57;
    pub const GETPGID: usize = 132;
    pub const NEWFSTATAT: usize = 293;
    pub const GETDENTS64: usize = 220;
    pub const STATX: usize = 379;
    pub const DUP3: usize = 326;
    pub const CLOSE_RANGE: usize = 436;
    pub const FCNTL: usize = 55;
    pub const PIPE2: usize = 325;
    pub const MKDIRAT: usize = 289;
    pub const UNLINKAT: usize = 294;
    pub const UTIMENSAT: usize = 315;
    pub const RENAMEAT2: usize = 347;
    pub const SYNC: usize = 36;
    pub const SCHED_GETAFFINITY: usize = 240;
    pub const SCHED_SETAFFINITY: usize = 239;
    pub const FSYNC: usize = 118;
    pub const KILL: usize = 37;
    pub const SOCKET: usize = 359;
    pub const CONNECT: usize = 362;
    pub const SENDTO: usize = 369;
    pub const RECVFROM: usize = 371;
    pub const GETSOCKNAME: usize = 367;
    pub const SETSOCKOPT: usize = 366;
    pub const SENDMSG: usize = 370;
    pub const RECVMSG: usize = 372;
    pub const BIND: usize = 361;
    pub const LISTEN: usize = 363;
    pub const ACCEPT4: usize = 364;
    pub const MEMFD_CREATE: usize = 350;
    pub const FTRUNCATE: usize = 93;
    pub const PREAD64: usize = 180;
    pub const PWRITE64: usize = 181;
    pub const READLINKAT: usize = 298;
    pub const SYMLINKAT: usize = 297;
    pub const LINKAT: usize = 296;
    pub const OPENAT2: usize = 437;
    pub const FCHMODAT: usize = 299;
    pub const FCHMODAT2: usize = 452;
    pub const STATFS: usize = 99;
    pub const FSTATFS: usize = 100;
    pub const LSEEK: usize = 19;
    pub const FACCESSAT: usize = 300;
}

/// Linux's system-call numbers on SPARC64 (D327).
#[cfg(target_arch = "sparc64")]
pub mod number {
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const CLOSE: usize = 6;
    pub const FSTAT: usize = 62;
    pub const MMAP: usize = 71;
    pub const MPROTECT: usize = 74;
    pub const MUNMAP: usize = 73;
    pub const BRK: usize = 17;
    pub const RT_SIGACTION: usize = 102;
    pub const RT_SIGRETURN: usize = 101;
    pub const IOCTL: usize = 54;
    pub const GETPID: usize = 20;
    pub const CLONE: usize = 217;
    pub const EXECVE: usize = 59;
    pub const UMASK: usize = 60;
    pub const TIMES: usize = 43;
    pub const WAIT4: usize = 7;
    pub const UNAME: usize = 189;
    pub const SETSID: usize = 175;
    pub const RT_SIGSUSPEND: usize = 107;
    pub const SIGALTSTACK: usize = 28;
    pub const REBOOT: usize = 55;
    pub const GETTID: usize = 143;
    pub const FUTEX: usize = 142;
    pub const SET_TID_ADDRESS: usize = 166;
    pub const CLOCK_SETTIME: usize = 256;
    pub const CLOCK_GETTIME: usize = 257;
    pub const CLOCK_NANOSLEEP: usize = 259;
    pub const EXIT_GROUP: usize = 188;
    pub const OPENAT: usize = 284;
    pub const PPOLL: usize = 298;
    pub const SET_ROBUST_LIST: usize = 300;
    pub const PRLIMIT64: usize = 331;
    pub const GETRANDOM: usize = 347;
    pub const RT_SIGPROCMASK: usize = 103;
    pub const GETCWD: usize = 119;
    pub const CHDIR: usize = 12;
    pub const GETEUID: usize = 49;
    pub const SETPGID: usize = 185;
    pub const GETPGID: usize = 224;
    pub const NEWFSTATAT: usize = 289;
    pub const GETDENTS64: usize = 154;
    pub const STATX: usize = 360;
    pub const DUP3: usize = 320;
    pub const CLOSE_RANGE: usize = 436;
    pub const FCNTL: usize = 92;
    pub const PIPE2: usize = 321;
    pub const MKDIRAT: usize = 285;
    pub const UNLINKAT: usize = 290;
    pub const UTIMENSAT: usize = 310;
    pub const RENAMEAT2: usize = 345;
    pub const SYNC: usize = 36;
    pub const SCHED_GETAFFINITY: usize = 260;
    pub const SCHED_SETAFFINITY: usize = 261;
    pub const FSYNC: usize = 95;
    pub const KILL: usize = 37;
    pub const SOCKET: usize = 97;
    pub const CONNECT: usize = 98;
    pub const SENDTO: usize = 133;
    pub const RECVFROM: usize = 125;
    pub const GETSOCKNAME: usize = 150;
    pub const SETSOCKOPT: usize = 355;
    pub const SENDMSG: usize = 114;
    pub const RECVMSG: usize = 113;
    pub const BIND: usize = 353;
    pub const LISTEN: usize = 354;
    pub const ACCEPT4: usize = 323;
    pub const MEMFD_CREATE: usize = 348;
    pub const FTRUNCATE: usize = 130;
    pub const PREAD64: usize = 67;
    pub const PWRITE64: usize = 68;
    pub const READLINKAT: usize = 294;
    pub const SYMLINKAT: usize = 293;
    pub const LINKAT: usize = 292;
    pub const OPENAT2: usize = 437;
    pub const FCHMODAT: usize = 295;
    pub const FCHMODAT2: usize = 452;
    pub const STATFS: usize = 157;
    pub const FSTATFS: usize = 158;
    pub const LSEEK: usize = 19;
    pub const FACCESSAT: usize = 296;
}

/// Linux's system-call numbers on MIPS64 n64, from 5000 (D315).
#[cfg(target_arch = "mips64")]
pub mod number {
    pub const READ: usize = 5000;
    pub const WRITE: usize = 5001;
    pub const CLOSE: usize = 5003;
    pub const FSTAT: usize = 5005;
    pub const MMAP: usize = 5009;
    pub const MPROTECT: usize = 5010;
    pub const MUNMAP: usize = 5011;
    pub const BRK: usize = 5012;
    pub const RT_SIGACTION: usize = 5013;
    pub const RT_SIGRETURN: usize = 5211;
    pub const IOCTL: usize = 5015;
    pub const GETPID: usize = 5038;
    pub const CLONE: usize = 5055;
    pub const EXECVE: usize = 5057;
    pub const UMASK: usize = 5093;
    pub const TIMES: usize = 5098;
    pub const WAIT4: usize = 5059;
    pub const UNAME: usize = 5061;
    pub const SETSID: usize = 5110;
    pub const RT_SIGSUSPEND: usize = 5128;
    pub const SIGALTSTACK: usize = 5129;
    pub const REBOOT: usize = 5164;
    pub const GETTID: usize = 5178;
    pub const FUTEX: usize = 5194;
    pub const SET_TID_ADDRESS: usize = 5212;
    pub const CLOCK_SETTIME: usize = 5221;
    pub const CLOCK_GETTIME: usize = 5222;
    pub const CLOCK_NANOSLEEP: usize = 5224;
    pub const EXIT_GROUP: usize = 5205;
    pub const OPENAT: usize = 5247;
    pub const PPOLL: usize = 5261;
    pub const SET_ROBUST_LIST: usize = 5268;
    pub const PRLIMIT64: usize = 5297;
    pub const GETRANDOM: usize = 5313;
    pub const RT_SIGPROCMASK: usize = 5014;
    pub const GETCWD: usize = 5077;
    pub const CHDIR: usize = 5078;
    pub const GETEUID: usize = 5105;
    pub const SETPGID: usize = 5107;
    pub const GETPGID: usize = 5119;
    pub const NEWFSTATAT: usize = 5252;
    pub const GETDENTS64: usize = 5308;
    pub const STATX: usize = 5326;
    pub const DUP3: usize = 5286;
    pub const CLOSE_RANGE: usize = 5436;
    pub const FCNTL: usize = 5070;
    pub const PIPE2: usize = 5287;
    pub const MKDIRAT: usize = 5248;
    pub const UNLINKAT: usize = 5253;
    pub const UTIMENSAT: usize = 5275;
    pub const RENAMEAT2: usize = 5311;
    pub const SYNC: usize = 5157;
    pub const SCHED_GETAFFINITY: usize = 5196;
    pub const SCHED_SETAFFINITY: usize = 5195;
    pub const FSYNC: usize = 5072;
    pub const KILL: usize = 5060;
    pub const SOCKET: usize = 5040;
    pub const CONNECT: usize = 5041;
    pub const SENDTO: usize = 5043;
    pub const RECVFROM: usize = 5044;
    pub const GETSOCKNAME: usize = 5050;
    pub const SETSOCKOPT: usize = 5053;
    pub const SENDMSG: usize = 5045;
    pub const RECVMSG: usize = 5046;
    pub const BIND: usize = 5048;
    pub const LISTEN: usize = 5049;
    pub const ACCEPT4: usize = 5293;
    pub const MEMFD_CREATE: usize = 5314;
    pub const FTRUNCATE: usize = 5075;
    pub const PREAD64: usize = 5016;
    pub const PWRITE64: usize = 5017;
    pub const READLINKAT: usize = 5257;
    pub const SYMLINKAT: usize = 5256;
    pub const LINKAT: usize = 5255;
    pub const OPENAT2: usize = 5437;
    pub const FCHMODAT: usize = 5258;
    pub const FCHMODAT2: usize = 5452;
    pub const STATFS: usize = 5134;
    pub const FSTATFS: usize = 5135;
    pub const LSEEK: usize = 5008;
    pub const FACCESSAT: usize = 5259;
}

/// Linux's terminal `ioctl` numbers: the generic ones, or PowerPC's (D304), MIPS's (D315) or
/// SPARC's (D327) own.
pub mod tty {
    #[cfg(not(any(
        target_arch = "powerpc64",
        target_arch = "mips64",
        target_arch = "sparc64"
    )))]
    mod values {
        pub const TCGETS: usize = 0x5401;
        pub const TCSETS: usize = 0x5402;
        pub const TCSETSW: usize = 0x5403;
        pub const TIOCSCTTY: usize = 0x540e;
        pub const TIOCGPGRP: usize = 0x540f;
        pub const TIOCSPGRP: usize = 0x5410;
        pub const TIOCGWINSZ: usize = 0x5413;
        pub const TIOCSWINSZ: usize = 0x5414;
        pub const FIONREAD: usize = 0x541b;
        pub const TIOCGPTN: usize = 0x8004_5430;
        pub const TIOCSPTLCK: usize = 0x4004_5431;
    }
    #[cfg(target_arch = "powerpc64")]
    mod values {
        pub const TCGETS: usize = 0x402c_7413;
        pub const TCSETS: usize = 0x802c_7414;
        pub const TCSETSW: usize = 0x802c_7415;
        pub const TIOCSCTTY: usize = 0x540e;
        pub const TIOCGPGRP: usize = 0x4004_7477;
        pub const TIOCSPGRP: usize = 0x8004_7476;
        pub const TIOCGWINSZ: usize = 0x4008_7468;
        pub const TIOCSWINSZ: usize = 0x8008_7467;
        pub const FIONREAD: usize = 0x4004_667f;
        pub const TIOCGPTN: usize = 0x4004_5430;
        pub const TIOCSPTLCK: usize = 0x8004_5431;
    }
    #[cfg(target_arch = "sparc64")]
    mod values {
        pub const TCGETS: usize = 0x4024_5408;
        pub const TCSETS: usize = 0x8024_5409;
        pub const TCSETSW: usize = 0x8024_540a;
        pub const TIOCSCTTY: usize = 0x2000_7484;
        pub const TIOCGPGRP: usize = 0x4004_7483;
        pub const TIOCSPGRP: usize = 0x8004_7482;
        pub const TIOCGWINSZ: usize = 0x4008_7468;
        pub const TIOCSWINSZ: usize = 0x8008_7467;
        pub const FIONREAD: usize = 0x4004_667f;
        pub const TIOCGPTN: usize = 0x4004_7486;
        pub const TIOCSPTLCK: usize = 0x8004_7487;
    }
    #[cfg(target_arch = "mips64")]
    mod values {
        pub const TCGETS: usize = 0x540d;
        pub const TCSETS: usize = 0x540e;
        pub const TCSETSW: usize = 0x540f;
        pub const TIOCSCTTY: usize = 0x5480;
        pub const TIOCGPGRP: usize = 0x4004_7477;
        pub const TIOCSPGRP: usize = 0x8004_7476;
        pub const TIOCGWINSZ: usize = 0x4008_7468;
        pub const TIOCSWINSZ: usize = 0x8008_7467;
        pub const FIONREAD: usize = 0x467f;
        pub const TIOCGPTN: usize = 0x4004_5430;
        pub const TIOCSPTLCK: usize = 0x8004_5431;
    }
    pub use values::*;
}

/// Makes Linux system call `number` with `arguments` and returns its result: a negative errno
/// on failure.
///
/// # Safety
///
/// The call may read and write the memory its arguments name and change the process's
/// mappings, signal handling and thread state: the caller must own all of it, and the call
/// must not unmap or change memory the program still uses.
#[inline(always)]
pub unsafe fn call(number: usize, arguments: [usize; 6]) -> isize {
    let result: isize;
    let [a, b, c, d, e, f] = arguments;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: Linux's x86_64 system-call convention: the kernel changes only `rax`, `rcx` and
    // `r11`, and only the memory the arguments name, which the caller owns.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e, in("r9") f,
            lateout("rcx") _, lateout("r11") _,
            options(nostack),
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: Linux's AArch64 system-call convention: the kernel changes only `x0`, and only
    // the memory the arguments name, which the caller owns.
    unsafe {
        asm!(
            "svc 0",
            in("x8") number,
            inlateout("x0") a as isize => result,
            in("x1") b, in("x2") c, in("x3") d, in("x4") e, in("x5") f,
            options(nostack),
        );
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: Linux's RISC-V system-call convention: the kernel changes only `a0`, and only
    // the memory the arguments name, which the caller owns.
    unsafe {
        asm!(
            "ecall",
            in("a7") number,
            inlateout("a0") a as isize => result,
            in("a1") b, in("a2") c, in("a3") d, in("a4") e, in("a5") f,
            options(nostack),
        );
    }
    #[cfg(target_arch = "powerpc64")]
    // SAFETY: Linux's PowerPC system-call convention (D304): a failure sets CR0.SO with the
    // error number positive in r3, which becomes the negative errno the callers expect; the
    // kernel may change r0 and r4 to r12, CR0 and the memory the arguments name, which the
    // caller owns.
    unsafe {
        asm!(
            "sc",
            "bns 1f",
            "neg 3, 3",
            "1:",
            inlateout("r0") number => _,
            inlateout("r3") a as isize => result,
            inlateout("r4") b => _, inlateout("r5") c => _, inlateout("r6") d => _,
            inlateout("r7") e => _, inlateout("r8") f => _,
            lateout("r9") _, lateout("r10") _, lateout("r11") _, lateout("r12") _,
            lateout("cr0") _,
            options(nostack),
        );
    }
    #[cfg(target_arch = "loongarch64")]
    // SAFETY: Linux's LoongArch system-call convention: the kernel changes only `$a0`, and only
    // the memory the arguments name, which the caller owns.
    unsafe {
        asm!(
            "syscall 0",
            in("$a7") number,
            inlateout("$a0") a as isize => result,
            in("$a1") b, in("$a2") c, in("$a3") d, in("$a4") e, in("$a5") f,
            options(nostack),
        );
    }
    #[cfg(target_arch = "s390x")]
    {
        let block = [a, b, c, d, e, f];
        let [a, b, c, d, e, f] = match number {
            number::MMAP => [block.as_ptr() as usize, 0, 0, 0, 0, 0],
            number::CLONE => [b, a, c, d, e, f],
            _ => block,
        };
        // SAFETY: Linux's s390x system-call convention (D323): the number in `r1`, the
        // arguments in `r2` to `r7`, the result in `r2`; the kernel changes only `r2` and the
        // memory the arguments name (for `mmap`, it reads `block`, which lives across the call),
        // which the caller owns.
        unsafe {
            asm!(
                "svc 0",
                in("r1") number,
                inlateout("r2") a as isize => result,
                in("r3") b, in("r4") c, in("r5") d, in("r6") e, in("r7") f,
                options(nostack),
            );
        }
    }
    #[cfg(target_arch = "sparc64")]
    {
        let [a, b, c, d, e, f] = crate::sparc_abi::arguments_out(number, [a, b, c, d, e, f]);
        let (value, failed): (isize, usize);
        // SAFETY: Linux's SPARC64 system-call convention (D327): the number in `%g1` (which
        // LLVM keeps nothing in across an instruction), the arguments in `%o0` to `%o5`; the
        // kernel changes `%o0` and `%o1`, the condition codes (carry set when the call failed,
        // with the error number positive in `%o0`) and the memory the arguments name, which
        // the caller owns.
        unsafe {
            asm!(
                "mov {number}, %g1",
                "ta 0x6d",
                "movcs %xcc, 1, {failed}",
                number = in(reg) number,
                failed = inout(reg) 0usize => failed,
                inlateout("o0") a as isize => value,
                inlateout("o1") b => _,
                in("o2") c, in("o3") d, in("o4") e, in("o5") f,
                options(nostack),
            );
        }
        result = match failed {
            0 => crate::sparc_abi::result_in(number, [a, b, c, d, e, f], value),
            _ => -(crate::sparc_abi::generic_errno(value as usize) as isize),
        };
    }
    #[cfg(target_arch = "mips64")]
    {
        let [a, b, c, d, e, f] = crate::mips_abi::arguments_out(number, [a, b, c, d, e, f]);
        let (value, failed): (isize, usize);
        // SAFETY: Linux's MIPS n64 system-call convention (D315): the number in `v0`, the
        // arguments in `a0` to `a5`; the kernel changes `v0` and `a3` (set when the call failed,
        // with the error number positive in `v0`) and the memory the arguments name, which the
        // caller owns.
        unsafe {
            asm!(
                "syscall",
                inlateout("$2") number as isize => value,
                in("$4") a, in("$5") b, in("$6") c,
                inlateout("$7") d => failed,
                in("$8") e, in("$9") f,
                options(nostack),
            );
        }
        result = match failed {
            0 => crate::mips_abi::result_in(number, [a, b, c, d, e, f], value),
            _ => -(crate::mips_abi::generic_errno(value as usize) as isize),
        };
    }
    result
}

/// Writes all of `bytes` to descriptor `descriptor`; `false` if a write failed or wrote
/// nothing.
pub fn write_all(descriptor: usize, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: `write` only reads `bytes`, which this function borrows.
        let written = unsafe {
            call(
                number::WRITE,
                [descriptor, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
            )
        };
        if written <= 0 {
            return false;
        }
        bytes = &bytes[(written as usize).min(bytes.len())..];
    }
    true
}

/// Ends the process with `status` (`exit_group`).
pub fn exit(status: i32) -> ! {
    loop {
        // SAFETY: `exit_group` ends the process and touches no memory.
        unsafe { call(number::EXIT_GROUP, [status as usize, 0, 0, 0, 0, 0]) };
    }
}
