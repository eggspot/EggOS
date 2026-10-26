//! The Win32 functions Windows console programs import, implemented natively on EggOS's Linux
//! system calls: each is an `extern "win64"` function the program calls directly through its
//! import address table, with no translation in between.

macro_rules! exports {
    ($($name:literal => $function:path),* $(,)?) => {
        &[$(crate::library::Export { name: $name, address: $function as *const () }),*]
    };
}

mod console;
mod environment;
mod file;
mod heap;
mod memory;
mod module;
mod process;
pub mod thread_environment;
mod time;

use crate::library::{self, Export, Library};
use eggos_runtime::errno::Errno;

pub use heap::{allocate as heap_allocate, initialize as initialize_heap};
pub use process::set_process;

pub type Bool = i32;
pub const TRUE: Bool = 1;
pub const FALSE: Bool = 0;
pub const INVALID_HANDLE_VALUE: usize = usize::MAX;

pub mod error {
    pub const SUCCESS: u32 = 0;
    pub const FILE_NOT_FOUND: u32 = 2;
    pub const PATH_NOT_FOUND: u32 = 3;
    pub const TOO_MANY_OPEN_FILES: u32 = 4;
    pub const ACCESS_DENIED: u32 = 5;
    pub const INVALID_HANDLE: u32 = 6;
    pub const NOT_ENOUGH_MEMORY: u32 = 8;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const FILE_EXISTS: u32 = 80;
    pub const INVALID_PARAMETER: u32 = 87;
    pub const INSUFFICIENT_BUFFER: u32 = 122;
    pub const MOD_NOT_FOUND: u32 = 126;
    pub const PROC_NOT_FOUND: u32 = 127;
    pub const NEGATIVE_SEEK: u32 = 131;
    pub const ENVVAR_NOT_FOUND: u32 = 203;
    pub const INVALID_ADDRESS: u32 = 487;
}

/// The Win32 error code for a Linux error number.
pub fn error_for(errno: Errno) -> u32 {
    match errno.0 {
        2 => error::FILE_NOT_FOUND,
        20 => error::PATH_NOT_FOUND,
        1 | 13 | 21 | 30 => error::ACCESS_DENIED,
        9 => error::INVALID_HANDLE,
        12 => error::NOT_ENOUGH_MEMORY,
        17 => error::FILE_EXISTS,
        23 | 24 => error::TOO_MANY_OPEN_FILES,
        38 | 95 => error::NOT_SUPPORTED,
        _ => error::INVALID_PARAMETER,
    }
}

/// A lock around `T` for the layer's process-wide tables: uncontended, it is one atomic
/// exchange each way.
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
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
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
    environment::EXPORTS,
    file::EXPORTS,
    heap::EXPORTS,
    memory::EXPORTS,
    module::EXPORTS,
    process::EXPORTS,
    thread_environment::EXPORTS,
    time::EXPORTS,
];

const NTDLL: &[&[Export]] = &[memory::NTDLL_EXPORTS];

fn exports(library: Library) -> &'static [&'static [Export]] {
    match library {
        Library::Kernel32 => KERNEL32,
        Library::Ntdll => NTDLL,
    }
}

/// The address of `function` in the library an import or `LoadLibrary` names `library`.
pub fn resolve(library: &[u8], function: &[u8]) -> Option<u64> {
    let exports = exports(library::library(library)?);
    library::find(exports, function).map(|address| address as u64)
}
