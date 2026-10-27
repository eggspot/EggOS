//! The Win32 functions Windows console programs import, implemented natively on EggOS's Linux
//! system calls: each is an `extern "win64"` function the program calls directly through its
//! import address table, with no translation in between.

macro_rules! exports {
    ($($name:literal => $function:path),* $(,)?) => {
        &[$(crate::library::Export { name: $name, address: $function as *const () }),*]
    };
}

mod console;
mod directory;
mod environment;
mod file;
mod heap;
mod memory;
mod module;
mod process;
mod shell;
mod strings;
mod sync;
mod system;
mod thread;
pub mod thread_environment;
mod time;

use crate::library::{self, Export, Library};
use eggos_runtime::errno::Errno;

pub use heap::{allocate as heap_allocate, process_heap};
pub use process::set_process;
pub use thread::finish as end_thread;

pub type Bool = i32;
pub const TRUE: Bool = 1;
pub const FALSE: Bool = 0;
pub const INVALID_HANDLE_VALUE: usize = usize::MAX;
pub const CURRENT_PROCESS: usize = usize::MAX;
pub const CURRENT_THREAD: usize = usize::MAX - 1;

pub use crate::errors::code as error;

/// The Win32 error code for a Linux error number.
pub fn error_for(errno: Errno) -> u32 {
    crate::errors::for_errno(errno.0)
}

const SPINS_BEFORE_YIELD: u32 = 128;

/// A lock around `T` for the layer's process-wide tables: uncontended, it is one atomic
/// exchange each way; contended, it yields the CPU every 128 tries, so a holder that was
/// preempted gets to run.
pub struct SpinLock<T> {
    locked: core::sync::atomic::AtomicBool,
    value: core::cell::UnsafeCell<T>,
}

// SAFETY: `value` is only reached through `with`, while `locked` is held by one thread.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: core::sync::atomic::AtomicBool::new(false),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    pub fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        use core::sync::atomic::Ordering;
        let mut spins = 0u32;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spins += 1;
            if spins.is_multiple_of(SPINS_BEFORE_YIELD) {
                crate::linux::yield_now();
            } else {
                core::hint::spin_loop();
            }
        }
        // SAFETY: the lock is held, so this is the only reference to the value.
        let result = work(unsafe { &mut *self.value.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

/// Sets the calling thread's last error and returns `value`: how a failing function ends.
pub fn fail<T>(code: u32, value: T) -> T {
    thread_environment::set_last_error(code);
    value
}

const KERNEL32: &[&[Export]] = &[
    console::EXPORTS,
    directory::EXPORTS,
    environment::EXPORTS,
    file::EXPORTS,
    heap::EXPORTS,
    memory::EXPORTS,
    module::EXPORTS,
    process::EXPORTS,
    strings::EXPORTS,
    sync::EXPORTS,
    system::EXPORTS,
    thread::EXPORTS,
    thread_environment::EXPORTS,
    time::EXPORTS,
];

const NTDLL: &[&[Export]] = &[memory::NTDLL_EXPORTS, system::NTDLL_EXPORTS];

const SHELL32: &[&[Export]] = &[shell::EXPORTS];

const USER32: &[&[Export]] = &[strings::USER32_EXPORTS];

fn exports(library: Library) -> &'static [&'static [Export]] {
    match library {
        Library::Kernel32 => KERNEL32,
        Library::Ntdll => NTDLL,
        Library::Shell32 => SHELL32,
        Library::User32 => USER32,
    }
}

/// The address of `function` in the library an import or `LoadLibrary` names `library`.
pub fn resolve(library: &[u8], function: &[u8]) -> Option<u64> {
    let exports = exports(library::library(library)?);
    library::find(exports, function).map(|address| address as u64)
}
