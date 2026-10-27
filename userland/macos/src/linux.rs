//! The Linux system calls the macOS layer and its libSystem make.

use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::call;

#[cfg(target_arch = "x86_64")]
pub mod number {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const CLOSE: usize = 3;
    pub const FSTAT: usize = 5;
    pub const LSEEK: usize = 8;
    pub const MMAP: usize = 9;
    pub const MPROTECT: usize = 10;
    pub const MUNMAP: usize = 11;
    pub const RT_SIGACTION: usize = 13;
    pub const IOCTL: usize = 16;
    pub const WRITEV: usize = 20;
    pub const SCHED_YIELD: usize = 24;
    pub const DUP: usize = 32;
    pub const GETPID: usize = 39;
    pub const CLONE: usize = 56;
    pub const EXIT: usize = 60;
    pub const FCNTL: usize = 72;
    pub const ARCH_PRCTL: usize = 158;
    pub const GETTID: usize = 186;
    pub const FUTEX: usize = 202;
    pub const SCHED_GETAFFINITY: usize = 204;
    pub const PPOLL: usize = 271;
    pub const KILL: usize = 62;
    pub const GETCWD: usize = 79;
    pub const CHDIR: usize = 80;
    pub const GETUID: usize = 102;
    pub const GETPPID: usize = 110;
    pub const GETDENTS64: usize = 217;
    pub const CLOCK_GETTIME: usize = 228;
    pub const CLOCK_NANOSLEEP: usize = 230;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT: usize = 257;
    pub const MKDIRAT: usize = 258;
    pub const NEWFSTATAT: usize = 262;
    pub const UNLINKAT: usize = 263;
    pub const FACCESSAT: usize = 269;
    pub const GETRANDOM: usize = 318;
}

#[cfg(target_arch = "aarch64")]
pub mod number {
    pub const GETCWD: usize = 17;
    pub const DUP: usize = 23;
    pub const FCNTL: usize = 25;
    pub const IOCTL: usize = 29;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
    pub const FACCESSAT: usize = 48;
    pub const CHDIR: usize = 49;
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const GETDENTS64: usize = 61;
    pub const LSEEK: usize = 62;
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const WRITEV: usize = 66;
    pub const PPOLL: usize = 73;
    pub const NEWFSTATAT: usize = 79;
    pub const FSTAT: usize = 80;
    pub const EXIT: usize = 93;
    pub const EXIT_GROUP: usize = 94;
    pub const FUTEX: usize = 98;
    pub const CLOCK_GETTIME: usize = 113;
    pub const CLOCK_NANOSLEEP: usize = 115;
    pub const SCHED_GETAFFINITY: usize = 123;
    pub const SCHED_YIELD: usize = 124;
    pub const KILL: usize = 129;
    pub const RT_SIGACTION: usize = 134;
    pub const GETPID: usize = 172;
    pub const GETPPID: usize = 173;
    pub const GETUID: usize = 174;
    pub const GETTID: usize = 178;
    pub const MUNMAP: usize = 215;
    pub const CLONE: usize = 220;
    pub const MMAP: usize = 222;
    pub const MPROTECT: usize = 226;
    pub const GETRANDOM: usize = 278;
}

pub const PAGE_SIZE: usize = 4096;
pub const AT_FDCWD: usize = -100isize as usize;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;
pub const AT_REMOVEDIR: usize = 0x200;
pub const O_RDONLY: usize = 0;
pub const O_CLOEXEC: usize = 0o2_000_000;
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
pub const PROT_EXEC: usize = 4;
pub const MAP_PRIVATE: usize = 0x02;
pub const MAP_ANONYMOUS: usize = 0x20;
pub const MAP_FIXED_NOREPLACE: usize = 0x10_0000;
pub const FUTEX_WAIT: usize = 0;
pub const FUTEX_WAIT_PRIVATE: usize = 128;
pub const FUTEX_WAKE_PRIVATE: usize = 129;
pub const ETIMEDOUT: Errno = Errno(110);
pub const TCGETS: usize = 0x5401;
pub const TERMIOS_SIZE: usize = 60;
pub const SIGABRT: usize = 6;

/// Makes system call `number` and returns its result or error.
///
/// # Safety
/// As [`eggos_runtime::syscall::call`].
pub unsafe fn syscall(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: the caller's duty.
    Errno::result(unsafe { call(number, arguments) })
}

pub fn read(fd: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: `read` writes at most `buffer.len()` bytes into the borrowed buffer.
    unsafe {
        syscall(
            number::READ,
            [fd, buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0],
        )
    }
}

pub fn write(fd: usize, bytes: &[u8]) -> Result<usize, Errno> {
    // SAFETY: `write` only reads the borrowed bytes.
    unsafe {
        syscall(
            number::WRITE,
            [fd, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
        )
    }
}

pub fn write_all(fd: usize, mut bytes: &[u8]) -> Result<(), Errno> {
    while !bytes.is_empty() {
        let written = write(fd, bytes)?;
        if written == 0 {
            return Err(Errno::EINVAL);
        }
        bytes = &bytes[written.min(bytes.len())..];
    }
    Ok(())
}

/// Opens the NUL-terminated `path` relative to the working directory.
pub fn open(path: &[u8], flags: usize, mode: usize) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `openat` reads the NUL-terminated path, which the slice holds.
    unsafe {
        syscall(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, flags, mode, 0, 0],
        )
    }
}

pub fn close(fd: usize) -> Result<usize, Errno> {
    // SAFETY: `close` touches no memory.
    unsafe { syscall(number::CLOSE, [fd, 0, 0, 0, 0, 0]) }
}

pub fn lseek(fd: usize, offset: i64, whence: usize) -> Result<usize, Errno> {
    // SAFETY: `lseek` touches no memory.
    unsafe { syscall(number::LSEEK, [fd, offset as usize, whence, 0, 0, 0]) }
}

/// Linux's `struct stat` for an open file.
pub fn fstat(fd: usize, out: &mut [u8; crate::darwin::LINUX_STAT_SIZE]) -> Result<usize, Errno> {
    // SAFETY: `fstat` writes one `struct stat` (at most 144 bytes) into the borrowed buffer.
    unsafe { syscall(number::FSTAT, [fd, out.as_mut_ptr() as usize, 0, 0, 0, 0]) }
}

/// Linux's `struct stat` for the NUL-terminated `path`.
pub fn stat_at(
    path: &[u8],
    flags: usize,
    out: &mut [u8; crate::darwin::LINUX_STAT_SIZE],
) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `newfstatat` reads the NUL-terminated path and writes one `struct stat` into
    // the borrowed buffer.
    unsafe {
        syscall(
            number::NEWFSTATAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                out.as_mut_ptr() as usize,
                flags,
                0,
                0,
            ],
        )
    }
}

pub fn is_terminal(fd: usize) -> bool {
    let mut termios = [0u8; TERMIOS_SIZE];
    // SAFETY: `TCGETS` writes one `struct termios` into the borrowed buffer.
    unsafe {
        syscall(
            number::IOCTL,
            [fd, TCGETS, termios.as_mut_ptr() as usize, 0, 0, 0],
        )
    }
    .is_ok()
}

/// Maps `length` bytes of new zeroed memory with `protection`, at `hint` if it is free.
pub fn map_anonymous(
    hint: usize,
    length: usize,
    protection: usize,
    fixed: bool,
) -> Result<usize, Errno> {
    let flags = MAP_PRIVATE | MAP_ANONYMOUS | if fixed { MAP_FIXED_NOREPLACE } else { 0 };
    // SAFETY: an anonymous mapping that replaces nothing (no `MAP_FIXED`) changes no memory
    // the program uses.
    unsafe {
        syscall(
            number::MMAP,
            [hint, length, protection, flags, usize::MAX, 0],
        )
    }
}

/// # Safety
/// The range must hold no memory the program still uses.
pub unsafe fn unmap(address: usize, length: usize) -> Result<usize, Errno> {
    // SAFETY: the caller's duty.
    unsafe { syscall(number::MUNMAP, [address, length, 0, 0, 0, 0]) }
}

/// # Safety
/// The range must hold no memory the program still uses in a way the new protection forbids.
pub unsafe fn protect(address: usize, length: usize, protection: usize) -> Result<usize, Errno> {
    // SAFETY: the caller's duty.
    unsafe { syscall(number::MPROTECT, [address, length, protection, 0, 0, 0]) }
}

/// The clock's time as seconds and nanoseconds.
pub fn clock_time(clock: usize) -> Result<[i64; 2], Errno> {
    let mut time = [0i64; 2];
    // SAFETY: `clock_gettime` writes one `struct timespec` into the borrowed words.
    unsafe {
        syscall(
            number::CLOCK_GETTIME,
            [clock, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        )
    }?;
    Ok(time)
}

/// Sleeps on the monotonic clock for `time` (seconds, nanoseconds).
pub fn sleep(time: [i64; 2]) -> Result<usize, Errno> {
    // SAFETY: `clock_nanosleep` reads one `struct timespec` from the borrowed words.
    unsafe {
        syscall(
            number::CLOCK_NANOSLEEP,
            [1, 0, time.as_ptr() as usize, 0, 0, 0],
        )
    }
}

pub fn process_id() -> usize {
    // SAFETY: `getpid` touches no memory.
    unsafe { syscall(number::GETPID, [0; 6]) }.unwrap_or(0)
}

pub fn thread_id() -> u32 {
    // SAFETY: `gettid` touches no memory.
    unsafe { syscall(number::GETTID, [0; 6]) }.unwrap_or(0) as u32
}

pub fn yield_now() {
    // SAFETY: `sched_yield` touches no memory.
    let _ = unsafe { syscall(number::SCHED_YIELD, [0; 6]) };
}

/// How many CPUs the process may run on.
pub fn cpu_count() -> u32 {
    let mut mask = [0u64; 16];
    // SAFETY: `sched_getaffinity` writes at most the borrowed mask's 128 bytes.
    let result = unsafe {
        syscall(
            number::SCHED_GETAFFINITY,
            [0, size_of_val(&mask), mask.as_mut_ptr() as usize, 0, 0, 0],
        )
    };
    match result {
        Ok(_) => mask
            .iter()
            .map(|word| word.count_ones())
            .sum::<u32>()
            .max(1),
        Err(_) => 1,
    }
}

/// Sleeps while the 32-bit word holds `expected`, at most `timeout` nanoseconds (forever if
/// `None`); returns early on a wake, a signal or a changed value, and `Err(ETIMEDOUT)` when
/// the time ran out.
pub fn futex_wait(
    word: &core::sync::atomic::AtomicU32,
    expected: u32,
    timeout: Option<u64>,
) -> Result<usize, Errno> {
    let time = timeout.map(|nanoseconds| {
        [
            (nanoseconds / 1_000_000_000).min(i64::MAX as u64) as i64,
            (nanoseconds % 1_000_000_000) as i64,
        ]
    });
    let time_pointer = time.as_ref().map_or(0, |time| time.as_ptr() as usize);
    // SAFETY: `futex` reads the borrowed word and, with a timeout, one `struct timespec` from
    // the borrowed local.
    unsafe {
        syscall(
            number::FUTEX,
            [
                word.as_ptr() as usize,
                FUTEX_WAIT_PRIVATE,
                expected as usize,
                time_pointer,
                0,
                0,
            ],
        )
    }
}

/// Sleeps while the 32-bit word holds `expected`, as a shared futex: the kind the kernel wakes
/// when it clears a thread's `CLONE_CHILD_CLEARTID` word.
pub fn futex_wait_shared(word: &core::sync::atomic::AtomicU32, expected: u32) {
    // SAFETY: `futex` reads the borrowed word.
    let _ = unsafe {
        syscall(
            number::FUTEX,
            [
                word.as_ptr() as usize,
                FUTEX_WAIT,
                expected as usize,
                0,
                0,
                0,
            ],
        )
    };
}

/// Wakes one or every thread sleeping on the 32-bit word.
pub fn futex_wake(word: &core::sync::atomic::AtomicU32, count: eggos_runtime::futex::WakeCount) {
    // SAFETY: a wake touches no memory.
    let _ = unsafe {
        syscall(
            number::FUTEX,
            [
                word.as_ptr() as usize,
                FUTEX_WAKE_PRIVATE,
                count.count() as usize,
                0,
                0,
                0,
            ],
        )
    };
}

const THREAD_FLAGS: usize =
    0x100 | 0x200 | 0x400 | 0x800 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x20_0000;

/// Starts a thread of this process on `stack_top`, with `thread_pointer` as its thread
/// pointer, that calls `entry(argument)`; `id_word` holds its id from before this returns until
/// the thread has ended and no longer uses its stack, when the kernel clears it.
///
/// # Safety
/// `stack_top` must be the 16-byte aligned end of memory only the new thread uses, for as long
/// as `id_word` is not 0; `id_word` must stay valid as long too.
pub unsafe fn spawn_thread(
    stack_top: usize,
    thread_pointer: usize,
    id_word: *mut u32,
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> Result<usize, Errno> {
    let result: isize;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: Linux's x86_64 `clone`: the caller's registers and memory are as before but
    // `rax`, `rcx` and `r11`; the child starts on its own stack and never comes back here.
    unsafe {
        core::arch::asm!(
            "syscall",
            "test rax, rax",
            "jnz 2f",
            "mov rdi, r13",
            "call r12",
            "ud2",
            "2:",
            inlateout("rax") number::CLONE as isize => result,
            in("rdi") THREAD_FLAGS, in("rsi") stack_top, in("rdx") id_word,
            in("r10") id_word, in("r8") thread_pointer, in("r12") entry, in("r13") argument,
            lateout("rcx") _, lateout("r11") _,
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: Linux's aarch64 `clone`: the caller's registers and memory are as before but
    // `x0`; the child starts on its own stack and never comes back here.
    unsafe {
        core::arch::asm!(
            "svc #0",
            "cbnz x0, 2f",
            "mov x0, x10",
            "blr x9",
            "brk #0",
            "2:",
            inlateout("x0") THREAD_FLAGS as isize => result,
            in("x1") stack_top, in("x2") id_word, in("x3") thread_pointer, in("x4") id_word,
            in("x8") number::CLONE, in("x9") entry, in("x10") argument,
        );
    }
    Errno::result(result)
}

/// Points the calling thread's thread pointer (`tpidr_el0`, or the FS base) at `address`.
///
/// # Safety
/// Nothing else in the process may use the thread pointer.
pub unsafe fn set_thread_pointer(address: usize) {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: the caller's promise; writing `tpidr_el0` touches no memory.
    unsafe {
        core::arch::asm!("msr tpidr_el0, {}", in(reg) address, options(nostack, nomem));
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the caller's promise; `arch_prctl(ARCH_SET_FS)` touches no memory.
    let _ = unsafe { syscall(number::ARCH_PRCTL, [0x1002, address, 0, 0, 0, 0]) };
}

/// Ends the calling thread only.
pub fn exit_thread() -> ! {
    loop {
        // SAFETY: `exit` ends the calling thread; the rest of the process goes on.
        let _ = unsafe { syscall(number::EXIT, [0; 6]) };
    }
}

pub fn exit(status: i32) -> ! {
    loop {
        // SAFETY: `exit_group` ends the process.
        let _ = unsafe { syscall(number::EXIT_GROUP, [status as usize, 0, 0, 0, 0, 0]) };
    }
}
