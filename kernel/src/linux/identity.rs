//! Who a program is and where it runs (M6, D55): `getpid`, `getppid`, `gettid`, `uname`,
//! `getcpu` and `sched_getaffinity` (D117), and the thread bookkeeping C libraries and Rust's std do at startup, `set_tid_address`,
//! `set_robust_list` and `get_robust_list`.
//!
//! A process's first thread has the process's id, as on Linux, and the threads `clone` starts
//! ids of their own (D91). The two bookkeeping calls register user memory the kernel touches
//! when the thread exits: `set_tid_address`'s word is cleared and a futex there woken, which
//! is how a thread waiting to join it learns it ended (see [`super::exit_thread`]); the locks
//! on its robust list that it still holds are released, marked as their owner died, first
//! ([`super::futex::release_robust_list`], D93).

use super::Errno;
use super::thread::LinuxThread;
use crate::cpu;
use crate::process::Process;
use eggos_hal::Platform;

/// What `uname` reports (D55): Linux's identity, so build scripts and libraries that look at
/// `uname -s` take their Linux path, with EggOS's in the release suffix and the version.
pub mod utsname {
    /// The kernel's name.
    pub const SYSNAME: &str = "Linux";
    /// The machine's name, until `sethostname` sets one: "localhost" resolves without DNS,
    /// where Linux's "(none)" does not.
    pub const NODENAME: &str = "localhost";
    /// A long-term Linux release, high enough for every C library's minimum on every
    /// architecture (loongarch64 needs 5.19), with EggOS's suffix. Raised only by a recorded
    /// decision, when what we implement or what major software requires calls for it.
    pub const RELEASE: &str = "6.1.0-eggos";
    /// The build: EggOS's version. No build date, so builds are reproducible.
    pub const VERSION: &str = concat!("#1 SMP EggOS ", env!("CARGO_PKG_VERSION"));
    /// The architecture, as Linux names it.
    #[cfg(target_arch = "aarch64")]
    pub const MACHINE: &str = "aarch64";
    #[cfg(target_arch = "riscv64")]
    pub const MACHINE: &str = "riscv64";
    #[cfg(target_arch = "x86_64")]
    pub const MACHINE: &str = "x86_64";
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("utsname::MACHINE needs Linux's name for this architecture");
    /// The NIS domain name: Linux's default.
    pub const DOMAINNAME: &str = "(none)";

    /// The length of each of `struct utsname`'s six fields, its terminating zero included.
    pub const FIELD: usize = 65;
}

/// `struct utsname` as `uname` copies it out, built when the kernel is compiled: each field
/// zero-padded to [`utsname::FIELD`] bytes.
const UTSNAME: [u8; 6 * utsname::FIELD] = {
    use utsname::*;
    let fields = [SYSNAME, NODENAME, RELEASE, VERSION, MACHINE, DOMAINNAME];
    let mut image = [0u8; 6 * FIELD];
    let mut field = 0;
    while field < fields.len() {
        let bytes = fields[field].as_bytes();
        assert!(bytes.len() < FIELD, "a utsname field is too long");
        let mut byte = 0;
        while byte < bytes.len() {
            image[field * FIELD + byte] = bytes[byte];
            byte += 1;
        }
        field += 1;
    }
    image
};

/// The size of Linux's `struct robust_list_head` on 64-bit architectures: three words.
pub const ROBUST_LIST_HEAD_SIZE: u64 = 24;

/// `getpid()`: the process's id.
pub fn getpid<P: Platform>(process: &Process<P>) -> u64 {
    u64::from(process.id().get())
}

/// `gettid()`: the thread's id, which for a process's first thread is the process's.
pub fn gettid(thread: &LinuxThread) -> u64 {
    u64::from(thread.id())
}

/// `getppid()`: the id of the caller's parent in the process table (D105): the process that
/// started it (`fork`, `clone`), or the one its orphans went to once that one ended, 0 for the
/// kernel (a process it started itself, as Linux's `init`). `execve` keeps it.
pub fn getppid<P: Platform>(process: &Process<P>) -> u64 {
    u64::from(process.parent_id())
}

/// `getcpu(cpu, node, cache)`: writes the number of the CPU the thread runs on to `cpu` and
/// its NUMA node, 0 until there are nodes, to `node`, each unless it is null, or `EFAULT` if
/// it is not the program's writable memory; `cache` is ignored, as on Linux. The thread may
/// be on another CPU by the time the program reads it.
pub fn getcpu<P: Platform>(process: &Process<P>, cpu: u64, node: u64) -> Result<u64, Errno> {
    let index = cpu::current::<P>().index() as u32;
    for (address, value) in [(cpu, index), (node, 0)] {
        if address != 0 {
            process
                .write_user_memory(address, &value.to_ne_bytes())
                .map_err(|_| Errno::EFAULT)?;
        }
    }
    Ok(0)
}

/// `sched_getaffinity(pid, size, mask)`: writes the CPUs the thread may run on, every online
/// CPU since none can be excluded yet, to `mask`, a bit per CPU in 64-bit words, and returns
/// the bytes written: the kernel's mask, a bit for each of the machine's CPUs rounded up to a
/// word, as Linux's raw call does (C libraries count the bits; Rust's
/// `std::thread::available_parallelism` sizes thread pools by them). `pid` 0, the caller's
/// thread or its process name the caller; another id is `ESRCH`, as for
/// [`get_robust_list`]. `EINVAL` if `size` is smaller than the kernel's mask or not a whole
/// number of words; `EFAULT` if `mask` is not the program's writable memory.
pub fn sched_getaffinity<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pid: u64,
    size: u64,
    mask: u64,
) -> Result<u64, Errno> {
    let cpus = cpu::CPUS.all();
    let words = cpus.len().max(1).div_ceil(AFFINITY_WORD_BITS);
    let bytes = (words * AFFINITY_WORD_BYTES) as u64;
    if size < bytes || !size.is_multiple_of(AFFINITY_WORD_BYTES as u64) {
        return Err(Errno::EINVAL);
    }
    let pid = pid as u32;
    if pid != 0 && pid != thread.id() && pid != process.id().get() {
        return Err(Errno::ESRCH);
    }
    for word in 0..words {
        let address = mask
            .checked_add((word * AFFINITY_WORD_BYTES) as u64)
            .ok_or(Errno::EFAULT)?;
        process
            .write_user_memory(address, &online_cpus(cpus, word).to_ne_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(bytes)
}

const AFFINITY_WORD_BITS: usize = u64::BITS as usize;
const AFFINITY_WORD_BYTES: usize = size_of::<u64>();

fn online_cpus(cpus: &[&cpu::Cpu], word: usize) -> u64 {
    if cpus.is_empty() {
        return u64::from(word == 0);
    }
    cpus.iter()
        .skip(word * AFFINITY_WORD_BITS)
        .take(AFFINITY_WORD_BITS)
        .enumerate()
        .filter(|(_, cpu)| cpu.is_online())
        .fold(0, |bits, (index, _)| bits | 1 << index)
}

/// `set_tid_address(address)`: the thread's exit clears the word at `address` and wakes a
/// futex there (0 for none); returns the thread's id.
pub fn set_tid_address(thread: &mut LinuxThread, address: u64) -> u64 {
    thread.set_clear_child_tid(address);
    gettid(thread)
}

/// `set_robust_list(head, length)`: the thread's robust futex list starts at `head` (0 for
/// none); `EINVAL` unless `length` is the size of Linux's list head
/// ([`ROBUST_LIST_HEAD_SIZE`]), as Linux, which checks nothing else.
pub fn set_robust_list(thread: &mut LinuxThread, head: u64, length: u64) -> Result<u64, Errno> {
    if length != ROBUST_LIST_HEAD_SIZE {
        return Err(Errno::EINVAL);
    }
    thread.set_robust_list(head);
    Ok(0)
}

/// `get_robust_list(tid, head, length)`: writes the robust list head of thread `tid` (0 for
/// the caller; a C `int`, so the low 32 bits) to `head` and the head's size to `length`, or
/// `EFAULT` if either is not the program's writable memory. Another thread's id is `ESRCH`
/// until a thread can reach another's state, as for `tkill` (D91).
pub fn get_robust_list<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    tid: u64,
    head: u64,
    length: u64,
) -> Result<u64, Errno> {
    let tid = tid as u32;
    if tid != 0 && tid != thread.id() {
        return Err(Errno::ESRCH);
    }
    process
        .write_user_memory(head, &thread.robust_list().to_ne_bytes())
        .and_then(|()| process.write_user_memory(length, &ROBUST_LIST_HEAD_SIZE.to_ne_bytes()))
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

/// `uname(buffer)`: copies `UTSNAME` to `buffer`, or `EFAULT` if it is not all the
/// program's writable memory (part may be written by then, as on Linux).
pub fn uname<P: Platform>(process: &Process<P>, buffer: u64) -> Result<u64, Errno> {
    process
        .write_user_memory(buffer, &UTSNAME)
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utsname_fields_are_zero_padded_in_place() {
        let field = |index: usize| {
            let bytes = &UTSNAME[index * utsname::FIELD..][..utsname::FIELD];
            let end = bytes.iter().position(|&byte| byte == 0).unwrap();
            assert!(bytes[end..].iter().all(|&byte| byte == 0));
            core::str::from_utf8(&bytes[..end]).unwrap()
        };
        assert_eq!(field(0), "Linux");
        assert_eq!(field(1), "localhost");
        assert_eq!(field(2), "6.1.0-eggos");
        assert_eq!(
            field(3),
            concat!("#1 SMP EggOS ", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(field(4), utsname::MACHINE);
        assert_eq!(field(5), "(none)");
    }
}
