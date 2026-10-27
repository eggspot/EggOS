//! The Linux system calls the Windows layer makes, on x86_64 and aarch64.

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
    pub const IOCTL: usize = 16;
    pub const SCHED_YIELD: usize = 24;
    pub const MADVISE: usize = 28;
    pub const GETPID: usize = 39;
    pub const CLONE: usize = 56;
    pub const EXIT: usize = 60;
    pub const GETCWD: usize = 79;
    pub const CHDIR: usize = 80;
    pub const ARCH_PRCTL: usize = 158;
    pub const GETTID: usize = 186;
    pub const FUTEX: usize = 202;
    pub const SCHED_GETAFFINITY: usize = 204;
    pub const GETDENTS64: usize = 217;
    pub const CLOCK_GETTIME: usize = 228;
    pub const CLOCK_NANOSLEEP: usize = 230;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT: usize = 257;
    pub const MKDIRAT: usize = 258;
    pub const NEWFSTATAT: usize = 262;
    pub const UNLINKAT: usize = 263;
    pub const PPOLL: usize = 271;
    pub const PIPE2: usize = 293;
}

#[cfg(target_arch = "aarch64")]
pub mod number {
    pub const GETCWD: usize = 17;
    pub const IOCTL: usize = 29;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
    pub const CHDIR: usize = 49;
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const PIPE2: usize = 59;
    pub const GETDENTS64: usize = 61;
    pub const LSEEK: usize = 62;
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
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
    pub const GETPID: usize = 172;
    pub const GETTID: usize = 178;
    pub const MUNMAP: usize = 215;
    pub const CLONE: usize = 220;
    pub const MMAP: usize = 222;
    pub const MPROTECT: usize = 226;
    pub const MADVISE: usize = 233;
}

pub const PAGE_SIZE: usize = 4096;
pub const AT_FDCWD: usize = -100isize as usize;
pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0o100;
pub const O_EXCL: usize = 0o200;
pub const O_TRUNC: usize = 0o1000;
#[cfg(target_arch = "x86_64")]
pub const O_DIRECTORY: usize = 0o200_000;
#[cfg(target_arch = "aarch64")]
pub const O_DIRECTORY: usize = 0o40_000;
pub const O_NONBLOCK: usize = 0o4000;
pub const O_CLOEXEC: usize = 0o2_000_000;
pub const AT_REMOVEDIR: usize = 0x200;
pub const FUTEX_WAIT_PRIVATE: usize = 128;
pub const FUTEX_WAKE_PRIVATE: usize = 129;
pub const PROT_NONE: usize = 0;
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
pub const PROT_EXEC: usize = 4;
pub const MAP_SHARED: usize = 0x01;
pub const MAP_PRIVATE: usize = 0x02;
pub const MAP_ANONYMOUS: usize = 0x20;
pub const MAP_FIXED_NOREPLACE: usize = 0x10_0000;
pub const MADV_DONTNEED: usize = 4;
#[cfg(target_arch = "x86_64")]
pub const ARCH_SET_GS: usize = 0x1001;
pub const CLOCK_REALTIME: usize = 0;
pub const CLOCK_MONOTONIC: usize = 1;
pub const TCGETS: usize = 0x5401;
pub const TERMIOS_SIZE: usize = 60;
pub const STAT_WORDS: usize = 18;
pub const STAT_SIZE_WORD: usize = 6;
pub const STAT_DEVICE_WORD: usize = 0;
pub const STAT_INODE_WORD: usize = 1;
pub const STAT_ACCESS_TIME_WORD: usize = 9;
pub const STAT_MODIFY_TIME_WORD: usize = 11;
pub const STAT_CHANGE_TIME_WORD: usize = 13;
pub const FILE_TYPE: u32 = 0o170_000;
pub const DIRECTORY: u32 = 0o040_000;
pub const REGULAR_FILE: u32 = 0o100_000;
pub const FIFO: u32 = 0o010_000;
pub const CHARACTER_DEVICE: u32 = 0o020_000;

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
pub fn open(path: &[u8], flags: usize) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `openat` reads the NUL-terminated path, which the slice holds.
    unsafe {
        syscall(
            number::OPENAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                flags | O_CLOEXEC,
                0o666,
                0,
                0,
            ],
        )
    }
}

pub fn close(fd: usize) -> Result<usize, Errno> {
    // SAFETY: `close` touches no memory.
    unsafe { syscall(number::CLOSE, [fd, 0, 0, 0, 0, 0]) }
}

/// The file's `stat` words: its mode is [`file_mode`], its size [`STAT_SIZE_WORD`].
pub fn fstat(fd: usize) -> Result<[u64; STAT_WORDS], Errno> {
    let mut words = [0u64; STAT_WORDS];
    // SAFETY: `fstat` writes one `struct stat` (144 bytes on x86_64, 128 on aarch64) into the
    // borrowed words.
    unsafe { syscall(number::FSTAT, [fd, words.as_mut_ptr() as usize, 0, 0, 0, 0]) }?;
    Ok(words)
}

#[cfg(target_arch = "x86_64")]
pub fn file_mode(words: &[u64; STAT_WORDS]) -> u32 {
    words[3] as u32
}

#[cfg(target_arch = "aarch64")]
pub fn file_mode(words: &[u64; STAT_WORDS]) -> u32 {
    words[2] as u32
}

#[cfg(target_arch = "x86_64")]
pub fn file_links(words: &[u64; STAT_WORDS]) -> u32 {
    words[2] as u32
}

#[cfg(target_arch = "aarch64")]
pub fn file_links(words: &[u64; STAT_WORDS]) -> u32 {
    (words[2] >> 32) as u32
}

/// Makes the device request `request` on `fd` with `argument`, whose length must be the size
/// the request number encodes (Linux's `_IOC`): the kernel reads or writes that many bytes.
pub fn device_control(fd: usize, request: usize, argument: &mut [u8]) -> Result<usize, Errno> {
    if (request >> 16) & 0x3fff != argument.len() {
        return Err(Errno::EINVAL);
    }
    // SAFETY: the request reads and writes at most its encoded size, which is the borrowed
    // argument's length.
    unsafe {
        syscall(
            number::IOCTL,
            [fd, request, argument.as_mut_ptr() as usize, 0, 0, 0],
        )
    }
}

/// Maps `length` bytes of `fd` at `offset`, shared and readable and writable.
pub fn map_shared(fd: usize, length: usize, offset: usize) -> Result<usize, Errno> {
    // SAFETY: a new mapping at an address the kernel chooses replaces nothing the program uses.
    unsafe {
        syscall(
            number::MMAP,
            [0, length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset],
        )
    }
}

/// A pipe whose ends do not block and close on `execve`: its read end, then its write end.
pub fn pipe() -> Result<(usize, usize), Errno> {
    let mut ends = [0i32; 2];
    // SAFETY: `pipe2` writes two descriptors into the borrowed array.
    unsafe {
        syscall(
            number::PIPE2,
            [
                ends.as_mut_ptr() as usize,
                O_NONBLOCK | O_CLOEXEC,
                0,
                0,
                0,
                0,
            ],
        )
    }?;
    Ok((ends[0] as usize, ends[1] as usize))
}

const POLLIN: u16 = 1;

/// Sleeps until one of `fds` can be read (a negative one is left out), a signal or an error.
pub fn wait_readable(fds: [i32; 2]) -> Result<usize, Errno> {
    let mut entries = [0u64; 2];
    for (entry, fd) in entries.iter_mut().zip(fds) {
        *entry = u64::from(fd as u32) | (u64::from(POLLIN) << 32);
    }
    // SAFETY: `ppoll` reads and writes two borrowed `struct pollfd`s (an `int` and two
    // `short`s each, as each `u64` lays them out in the machine's little-endian order) and takes
    // no time limit or signal mask.
    unsafe {
        syscall(
            number::PPOLL,
            [entries.as_mut_ptr() as usize, 2, 0, 0, 0, 0],
        )
    }
}

pub fn lseek(fd: usize, offset: i64, whence: usize) -> Result<usize, Errno> {
    // SAFETY: `lseek` touches no memory.
    unsafe { syscall(number::LSEEK, [fd, offset as usize, whence, 0, 0, 0]) }
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

/// # Safety
/// The range's contents must no longer be needed: its pages read as zero afterwards.
pub unsafe fn discard(address: usize, length: usize) -> Result<usize, Errno> {
    // SAFETY: the caller's duty.
    unsafe { syscall(number::MADVISE, [address, length, MADV_DONTNEED, 0, 0, 0]) }
}

/// The clock's time in nanoseconds.
pub fn clock_nanoseconds(clock: usize) -> u64 {
    let mut time = [0i64; 2];
    // SAFETY: `clock_gettime` writes one `struct timespec` into the borrowed words.
    let _ = unsafe {
        syscall(
            number::CLOCK_GETTIME,
            [clock, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        )
    };
    (time[0] as u64)
        .wrapping_mul(1_000_000_000)
        .wrapping_add(time[1] as u64)
}

pub fn sleep_nanoseconds(nanoseconds: u64) {
    let time = [
        (nanoseconds / 1_000_000_000) as i64,
        (nanoseconds % 1_000_000_000) as i64,
    ];
    // SAFETY: `clock_nanosleep` reads one `struct timespec` from the borrowed words.
    let _ = unsafe {
        syscall(
            number::CLOCK_NANOSLEEP,
            [CLOCK_MONOTONIC, 0, time.as_ptr() as usize, 0, 0, 0],
        )
    };
}

pub fn yield_now() {
    // SAFETY: `sched_yield` touches no memory.
    let _ = unsafe { syscall(number::SCHED_YIELD, [0; 6]) };
}

pub fn process_id() -> u32 {
    // SAFETY: `getpid` touches no memory.
    unsafe { syscall(number::GETPID, [0; 6]) }.unwrap_or(0) as u32
}

pub fn thread_id() -> u32 {
    // SAFETY: `gettid` touches no memory.
    unsafe { syscall(number::GETTID, [0; 6]) }.unwrap_or(0) as u32
}

/// Points the calling thread's register for its thread environment block at `address`: GS's
/// base on x86_64, `tpidr_el0` on aarch64 (the layer's own copy, from which it loads `x18`
/// before it calls into the program, D124).
///
/// # Safety
/// `address` must be the thread environment block every Win32 function finds through it.
pub unsafe fn set_thread_environment_block(address: usize) -> Result<(), Errno> {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the caller's duty; `arch_prctl(ARCH_SET_GS)` touches no memory.
    unsafe { syscall(number::ARCH_PRCTL, [ARCH_SET_GS, address, 0, 0, 0, 0]) }?;
    #[cfg(target_arch = "aarch64")]
    // SAFETY: the caller's duty; writing `tpidr_el0` touches no memory.
    unsafe {
        core::arch::asm!("msr tpidr_el0, {}", in(reg) address, options(nostack, nomem));
    }
    Ok(())
}

/// The `stat` words of the NUL-terminated `path`, relative to directory `directory_fd`
/// ([`AT_FDCWD`] for the working directory).
pub fn stat_at(directory_fd: usize, path: &[u8]) -> Result<[u64; STAT_WORDS], Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    let mut words = [0u64; STAT_WORDS];
    // SAFETY: `newfstatat` reads the NUL-terminated path and writes one `struct stat` into the
    // borrowed words.
    unsafe {
        syscall(
            number::NEWFSTATAT,
            [
                directory_fd,
                path.as_ptr() as usize,
                words.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    }?;
    Ok(words)
}

/// Reads directory entries (`struct linux_dirent64`) from `fd` into `buffer`; 0 at the end.
pub fn directory_entries(fd: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: `getdents64` writes at most `buffer.len()` bytes into the borrowed buffer.
    unsafe {
        syscall(
            number::GETDENTS64,
            [fd, buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0],
        )
    }
}

/// Writes the working directory, NUL-terminated, into `buffer`; its length with the NUL.
pub fn working_directory(buffer: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: `getcwd` writes at most `buffer.len()` bytes into the borrowed buffer.
    unsafe {
        syscall(
            number::GETCWD,
            [buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0, 0],
        )
    }
}

/// Changes the working directory to the NUL-terminated `path`.
pub fn change_directory(path: &[u8]) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `chdir` reads the NUL-terminated path.
    unsafe { syscall(number::CHDIR, [path.as_ptr() as usize, 0, 0, 0, 0, 0]) }
}

/// Makes the directory at the NUL-terminated `path`.
pub fn make_directory(path: &[u8]) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `mkdirat` reads the NUL-terminated path.
    unsafe {
        syscall(
            number::MKDIRAT,
            [AT_FDCWD, path.as_ptr() as usize, 0o777, 0, 0, 0],
        )
    }
}

/// Removes the file (or, with [`AT_REMOVEDIR`], the directory) at the NUL-terminated `path`.
pub fn remove(path: &[u8], flags: usize) -> Result<usize, Errno> {
    if path.last() != Some(&0) {
        return Err(Errno::EINVAL);
    }
    // SAFETY: `unlinkat` reads the NUL-terminated path.
    unsafe {
        syscall(
            number::UNLINKAT,
            [AT_FDCWD, path.as_ptr() as usize, flags, 0, 0, 0],
        )
    }
}

/// How many CPUs the process may run on.
pub fn cpu_count() -> u32 {
    let mut mask = [0u64; 16];
    // SAFETY: `sched_getaffinity` writes at most the borrowed mask's 128 bytes.
    let result = unsafe {
        syscall(
            number::SCHED_GETAFFINITY,
            [
                0,
                core::mem::size_of_val(&mask),
                mask.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
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

/// Sleeps while the 32-bit word at `word` holds `expected`, at most `timeout` nanoseconds
/// (forever if `None`); returns early on a wake, a signal or a changed value, and
/// `Err(ETIMEDOUT)` when the time ran out.
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

/// Wakes one or every thread sleeping on the 32-bit word at `address`.
pub fn futex_wake(address: usize, count: crate::futex::WakeCount) {
    // SAFETY: a wake touches no memory.
    let _ = unsafe {
        syscall(
            number::FUTEX,
            [address, FUTEX_WAKE_PRIVATE, count.count() as usize, 0, 0, 0],
        )
    };
}

const THREAD_FLAGS: usize =
    0x100 | 0x200 | 0x400 | 0x800 | 0x1_0000 | 0x4_0000 | 0x10_0000 | 0x20_0000;

/// Starts a thread of this process on `stack_top` that calls `entry(argument)`; `id_word`
/// holds its id from before this returns until the thread has ended and no longer uses its
/// stack, when the kernel clears it.
///
/// # Safety
/// `stack_top` must be the 16-byte aligned end of memory only the new thread uses, for as long
/// as `id_word` is not 0; `id_word` must stay valid as long too.
pub unsafe fn spawn_thread(
    stack_top: usize,
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
            in("r10") id_word, in("r8") 0usize, in("r12") entry, in("r13") argument,
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
            in("x1") stack_top, in("x2") id_word, in("x3") 0usize, in("x4") id_word,
            in("x8") number::CLONE, in("x9") entry, in("x10") argument,
        );
    }
    Errno::result(result)
}

/// Ends the calling thread only.
pub fn exit_thread() -> ! {
    loop {
        // SAFETY: `exit` ends the calling thread; the rest of the process goes on.
        let _ = unsafe { syscall(number::EXIT, [0; 6]) };
    }
}

pub fn exit(status: u32) -> ! {
    loop {
        // SAFETY: `exit_group` ends the process.
        let _ = unsafe { syscall(number::EXIT_GROUP, [status as usize, 0, 0, 0, 0, 0]) };
    }
}

/// The CPU features Linux's auxiliary vector reports, which compiler-builtins' aarch64 atomics
/// ask for: none, so they use load-exclusive and store-exclusive pairs, which every Armv8 CPU
/// has.
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn getauxval(_: core::ffi::c_ulong) -> core::ffi::c_ulong {
    0
}
