//! The Linux system calls the Windows layer makes on x86_64.

use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::call;

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
    pub const ARCH_PRCTL: usize = 158;
    pub const GETTID: usize = 186;
    pub const CLOCK_GETTIME: usize = 228;
    pub const CLOCK_NANOSLEEP: usize = 230;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT: usize = 257;
}

pub const PAGE_SIZE: usize = 4096;
pub const AT_FDCWD: usize = -100isize as usize;
pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0o100;
pub const O_EXCL: usize = 0o200;
pub const O_TRUNC: usize = 0o1000;
pub const O_CLOEXEC: usize = 0o2_000_000;
pub const PROT_NONE: usize = 0;
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
pub const PROT_EXEC: usize = 4;
pub const MAP_PRIVATE: usize = 0x02;
pub const MAP_ANONYMOUS: usize = 0x20;
pub const MAP_FIXED_NOREPLACE: usize = 0x10_0000;
pub const MADV_DONTNEED: usize = 4;
pub const ARCH_SET_GS: usize = 0x1001;
pub const CLOCK_REALTIME: usize = 0;
pub const CLOCK_MONOTONIC: usize = 1;
pub const TCGETS: usize = 0x5401;
pub const TERMIOS_SIZE: usize = 60;
pub const STAT_WORDS: usize = 18;
pub const STAT_MODE_WORD: usize = 3;
pub const STAT_SIZE_WORD: usize = 6;
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

/// The file's `stat` words: its mode is [`STAT_MODE_WORD`]'s low half, its size
/// [`STAT_SIZE_WORD`].
pub fn fstat(fd: usize) -> Result<[u64; STAT_WORDS], Errno> {
    let mut words = [0u64; STAT_WORDS];
    // SAFETY: `fstat` writes one `struct stat` (144 bytes on x86_64) into the borrowed words.
    unsafe { syscall(number::FSTAT, [fd, words.as_mut_ptr() as usize, 0, 0, 0, 0]) }?;
    Ok(words)
}

pub fn file_mode(words: &[u64; STAT_WORDS]) -> u32 {
    words[STAT_MODE_WORD] as u32
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

/// # Safety
/// `address` must be the thread environment block every Win32 function finds through GS.
pub unsafe fn set_gs_base(address: usize) -> Result<usize, Errno> {
    // SAFETY: the caller's duty; `arch_prctl(ARCH_SET_GS)` touches no memory.
    unsafe { syscall(number::ARCH_PRCTL, [ARCH_SET_GS, address, 0, 0, 0, 0]) }
}

pub fn exit(status: u32) -> ! {
    loop {
        // SAFETY: `exit_group` ends the process.
        let _ = unsafe { syscall(number::EXIT_GROUP, [status as usize, 0, 0, 0, 0, 0]) };
    }
}
