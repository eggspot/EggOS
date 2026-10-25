//! Who a program is and where it runs (M6, D55): `getpid`, `getppid`, `gettid` and `uname`,
//! and the thread bookkeeping C libraries and Rust's std do at startup, `set_tid_address`
//! and `set_robust_list`.
//!
//! A process's first thread has the process's id, as on Linux, and the threads `clone` starts
//! ids of their own (D91). The two bookkeeping calls register user memory the kernel touches
//! when the thread exits: `set_tid_address`'s word is cleared and a futex there woken, which
//! is how a thread waiting to join it learns it ended (see [`super::exit_thread`]); the robust
//! list's futexes would be released, but the list is not remembered yet: it matters only for
//! a thread that exits holding a lock shared with another.

use super::Errno;
use super::thread::LinuxThread;
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

/// `getppid()`: 0, as for a process the kernel starts itself (Linux's `init`). Parents come
/// with `fork` and `clone`.
pub fn getppid() -> u64 {
    0
}

/// `set_tid_address(address)`: the thread's exit clears the word at `address` and wakes a
/// futex there (0 for none); returns the thread's id.
pub fn set_tid_address(thread: &mut LinuxThread, address: u64) -> u64 {
    thread.set_clear_child_tid(address);
    gettid(thread)
}

/// `set_robust_list(head, length)`: `EINVAL` unless `length` is the size of Linux's list head
/// ([`ROBUST_LIST_HEAD_SIZE`]), as Linux; the head is not remembered yet (see the module
/// documentation).
pub fn set_robust_list(_head: u64, length: u64) -> Result<u64, Errno> {
    if length == ROBUST_LIST_HEAD_SIZE {
        Ok(0)
    } else {
        Err(Errno::EINVAL)
    }
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

    #[test]
    fn set_robust_list_takes_only_linux_s_head_size() {
        assert_eq!(set_robust_list(0x1000, 24), Ok(0));
        for length in [0, 16, 25, u64::MAX] {
            assert_eq!(set_robust_list(0x1000, length), Err(Errno::EINVAL));
        }
    }
}
