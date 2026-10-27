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
    pub const PIPE2: usize = 293;
    pub const MKDIRAT: usize = 258;
    pub const UNLINKAT: usize = 263;
    pub const UTIMENSAT: usize = 280;
    pub const RENAMEAT2: usize = 316;
    pub const SYNC: usize = 162;
    pub const SCHED_GETAFFINITY: usize = 204;
    pub const FSYNC: usize = 74;
}

/// Linux's system-call numbers: the generic table AArch64 and RISC-V share.
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
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
    pub const PIPE2: usize = 59;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
    pub const UTIMENSAT: usize = 88;
    pub const RENAMEAT2: usize = 276;
    pub const SYNC: usize = 81;
    pub const SCHED_GETAFFINITY: usize = 123;
    pub const FSYNC: usize = 82;
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
