//! Who a program is and where it runs (M6, D55): `getpid`, `getppid`, `gettid`, `uname`,
//! `sethostname` and `setdomainname` (D168), `getcpu` and `sched_getaffinity` (D117), and the
//! thread bookkeeping C libraries and Rust's std do at startup, `set_tid_address`,
//! `set_robust_list` and `get_robust_list`.
//!
//! The machine's name and NIS domain name start as [`utsname::NODENAME`] and
//! [`utsname::DOMAINNAME`]; a privileged process may change them, for every process (EggOS has
//! one UTS namespace), and `uname` and `/proc/sys/kernel/hostname` report the change.
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
use crate::sync::LeafSpinLock;
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

/// `sched_getscheduler(pid)`: `SCHED_OTHER` (0), the only scheduling policy threads have
/// yet. `EINVAL` for a negative `pid`; otherwise `pid` names the caller as for
/// [`sched_getaffinity`], and another id is `ESRCH`.
pub fn sched_getscheduler<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pid: u64,
) -> Result<u64, Errno> {
    scheduling_target(process, thread, pid)?;
    Ok(SCHED_OTHER)
}

/// `sched_getparam(pid, param)`: writes `SCHED_OTHER`'s priority, 0, to the
/// `struct sched_param` at `param`. `EINVAL` for a null `param` or a negative `pid`, then
/// `ESRCH` as for [`sched_getscheduler`]; `EFAULT` if `param` is not the program's writable
/// memory.
pub fn sched_getparam<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pid: u64,
    param: u64,
) -> Result<u64, Errno> {
    if param == 0 {
        return Err(Errno::EINVAL);
    }
    scheduling_target(process, thread, pid)?;
    process
        .write_user_memory(param, &SCHED_OTHER_PRIORITY.to_ne_bytes())
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

const SCHED_OTHER: u64 = 0;
const SCHED_OTHER_PRIORITY: i32 = 0;

fn scheduling_target<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pid: u64,
) -> Result<(), Errno> {
    let pid = pid as i32;
    if pid < 0 {
        return Err(Errno::EINVAL);
    }
    let pid = pid as u32;
    if pid != 0 && pid != thread.id() && pid != process.id().get() {
        return Err(Errno::ESRCH);
    }
    Ok(())
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

/// The longest machine or domain name (`__NEW_UTS_LEN`): a field without its zero.
pub const MAX_NAME: usize = utsname::FIELD - 1;

/// A name `sethostname` or `setdomainname` gave, if one did (all zero until then, so the
/// static that holds them stays in `.bss`).
#[derive(Debug, Clone, Copy)]
struct ChangedName {
    set: bool,
    length: u8,
    bytes: [u8; MAX_NAME],
}

const UNCHANGED: ChangedName = ChangedName {
    set: false,
    length: 0,
    bytes: [0; MAX_NAME],
};

/// Which name, an index into [`CHANGED_NAMES`] and the field of `struct utsname` it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Host,
    Domain,
}

impl Which {
    fn index(self) -> usize {
        match self {
            Self::Host => 0,
            Self::Domain => 1,
        }
    }

    fn field(self) -> usize {
        match self {
            Self::Host => 1,
            Self::Domain => 5,
        }
    }
}

static CHANGED_NAMES: LeafSpinLock<[ChangedName; 2]> = LeafSpinLock::new_leaf([UNCHANGED; 2]);

/// `struct utsname` as `uname` copies it out now: [`UTSNAME`] with the names changed so far.
fn current_utsname() -> [u8; 6 * utsname::FIELD] {
    let mut image = UTSNAME;
    let changed = *CHANGED_NAMES.lock();
    for which in [Which::Host, Which::Domain] {
        let name = changed[which.index()];
        if name.set {
            let field = &mut image[which.field() * utsname::FIELD..][..utsname::FIELD];
            let length = usize::from(name.length);
            field.fill(0);
            field[..length].copy_from_slice(&name.bytes[..length]);
        }
    }
    image
}

/// The machine's name now (`uname -n`, `/proc/sys/kernel/hostname`), copied into `into`: its
/// length.
pub fn host_name(into: &mut [u8; MAX_NAME]) -> usize {
    let image = current_utsname();
    let field = &image[utsname::FIELD..][..MAX_NAME];
    let length = field.iter().position(|&byte| byte == 0).unwrap_or(MAX_NAME);
    into[..length].copy_from_slice(&field[..length]);
    length
}

/// `uname(buffer)`: copies `struct utsname` to `buffer`, the names `sethostname` and
/// `setdomainname` changed included, or `EFAULT` if it is not all the program's writable memory
/// (part may be written by then, as on Linux).
pub fn uname<P: Platform>(process: &Process<P>, buffer: u64) -> Result<u64, Errno> {
    process
        .write_user_memory(buffer, &current_utsname())
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

/// `sethostname(name, length)`: the machine's name becomes the `length` bytes at `name` (D168).
/// As Linux, in this order: `EPERM` unless the caller is privileged (effective user id 0);
/// `EINVAL` for a `length` (an `int`) that is negative or above [`MAX_NAME`]; `EFAULT` if the
/// bytes are not the program's memory.
pub fn sethostname<P: Platform>(
    process: &Process<P>,
    name: u64,
    length: u64,
) -> Result<u64, Errno> {
    set_name(process, Which::Host, name, length)
}

/// `setdomainname(name, length)`: the NIS domain name, as [`sethostname`] sets the machine's.
pub fn setdomainname<P: Platform>(
    process: &Process<P>,
    name: u64,
    length: u64,
) -> Result<u64, Errno> {
    set_name(process, Which::Domain, name, length)
}

fn set_name<P: Platform>(
    process: &Process<P>,
    which: Which,
    name: u64,
    length: u64,
) -> Result<u64, Errno> {
    if !process
        .credentials_and_limits()
        .lock()
        .credentials
        .is_privileged()
    {
        return Err(Errno::EPERM);
    }
    let length = usize::try_from(length as i32).map_err(|_| Errno::EINVAL)?;
    if length > MAX_NAME {
        return Err(Errno::EINVAL);
    }
    let mut bytes = [0; MAX_NAME];
    process
        .read_user_memory(name, &mut bytes[..length])
        .map_err(|_| Errno::EFAULT)?;
    CHANGED_NAMES.lock()[which.index()] = ChangedName {
        set: true,
        length: length as u8,
        bytes,
    };
    Ok(0)
}

/// Gives back the names `uname` started with, for tests that change them.
#[cfg(test)]
pub fn forget_changed_names() {
    *CHANGED_NAMES.lock() = [UNCHANGED; 2];
}

#[cfg(test)]
pub mod tests {
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

    #[test]
    fn a_changed_name_replaces_its_field_whole() {
        let _guard = NAMES_TEST.lock().unwrap();
        forget_changed_names();
        let mut name = [0; MAX_NAME];
        assert_eq!(&name[..host_name(&mut name)], b"localhost");
        CHANGED_NAMES.lock()[Which::Host.index()] = ChangedName {
            set: true,
            length: 3,
            bytes: {
                let mut bytes = [0; MAX_NAME];
                bytes[..3].copy_from_slice(b"egg");
                bytes
            },
        };
        assert_eq!(&name[..host_name(&mut name)], b"egg");
        let image = current_utsname();
        assert_eq!(&image[utsname::FIELD..][..4], b"egg\0");
        assert!(
            image[utsname::FIELD..][..utsname::FIELD]
                .iter()
                .skip(3)
                .all(|&byte| byte == 0)
        );
        CHANGED_NAMES.lock()[Which::Domain.index()] = ChangedName {
            set: true,
            length: 0,
            bytes: [0; MAX_NAME],
        };
        assert!(
            current_utsname()[5 * utsname::FIELD..]
                .iter()
                .all(|&byte| byte == 0)
        );
        forget_changed_names();
        assert_eq!(current_utsname(), UTSNAME);
    }

    /// Serializes the tests that change the names, which every process shares.
    pub static NAMES_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
