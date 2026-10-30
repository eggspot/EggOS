//! The Win32 functions Windows programs import, implemented natively on EggOS's Linux system
//! calls and, for windows, the window server, the C runtime of MSVC's `/MD` programs (`crt/`)
//! and exception handling (`exception/`, D177): each is an `extern "efiapi"` function (Windows'
//! calling convention on x86_64 and aarch64, `abi.rs`) the program calls through its import
//! address table, directly on x86_64 and through an entry that restores `x18` on aarch64.
//! comctl32's common controls (`common_controls/`), GDI+ (`gdiplus.rs`), COM (`com/`) and the
//! smaller system libraries a real app imports came with AutoHotkey (D178).

#[macro_use]
mod abi;

macro_rules! exports {
    ($($name:literal => $function:path $([$entry_kind:ident])?),* $(,)?) => {
        &[$(crate::library::Export {
            name: $name,
            address: export_entry!($function $(, $entry_kind)?),
        }),*]
    };
}

mod accelerator;
mod advapi32;
mod ansi;
mod clipboard;
mod com;
mod combo_box;
mod common_controls;
mod console;
mod controls;
mod crt;
mod dialog;
mod dialog_box;
mod dialog_window;
mod dib;
mod directory;
mod dwmapi;
mod elements;
mod environment;
mod exception;
mod file;
mod file_information;
mod file_picker;
mod gdi;
mod gdiplus;
mod gui;
mod heap;
mod icon;
mod keyboard;
mod memory;
mod menu;
mod menu_items;
mod message_box;
mod messages;
mod module;
mod monitor;
mod native_files;
mod nls;
mod overlapped;
mod paint;
mod pipe;
mod placement;
mod private_profile;
mod process;
mod processes;
mod psapi;
mod random;
mod resource;
mod send_input;
mod shell;
mod shell_desktop;
mod shell_files;
mod shell_folders;
mod shlwapi;
mod strings;
mod subclass;
mod sync;
mod system;
mod thread;
pub mod thread_environment;
mod time;
mod timer;
mod toolhelp;
mod user32;
mod userenv;
mod uxtheme;
mod version;
mod volume;
mod window_class;
mod window_device;
mod window_state;
mod window_text;
mod window_tree;
mod wininet;
mod winmm;
mod winsock;

use crate::library::{self, Export, Library};
use eggos_runtime::errno::Errno;

pub use abi::call_program;
pub use console::set_gui_program;
pub use crt::set_processor_features;
pub use exception::{deliver_fault, in_layer as in_layer_code, set_image as set_exception_image};
pub use heap::{allocate as heap_allocate, free as heap_free, process_heap};
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

    /// Runs `work` as [`SpinLock::with`] does, unless `give_up`, asked each time the wait
    /// yields, says to stop waiting for the lock: it then returns `None` without running `work`.
    pub fn try_with<R>(
        &self,
        mut give_up: impl FnMut() -> bool,
        work: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        use core::sync::atomic::Ordering;
        let mut spins = 0u32;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(SPINS_BEFORE_YIELD) {
                if give_up() {
                    return None;
                }
                crate::linux::yield_now();
            } else {
                core::hint::spin_loop();
            }
        }
        // SAFETY: the lock is held, so this is the only reference to the value.
        let result = work(unsafe { &mut *self.value.get() });
        self.locked.store(false, Ordering::Release);
        Some(result)
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
    crt::KERNEL32_EXPORTS,
    exception::KERNEL32_EXPORTS,
    resource::EXPORTS,
    nls::EXPORTS,
    private_profile::EXPORTS,
    processes::EXPORTS,
    pipe::EXPORTS,
    toolhelp::EXPORTS,
    volume::EXPORTS,
    overlapped::EXPORTS,
    file_information::EXPORTS,
];

const NTDLL: &[&[Export]] = &[
    memory::NTDLL_EXPORTS,
    system::NTDLL_EXPORTS,
    exception::NTDLL_EXPORTS,
    native_files::EXPORTS,
];

const SHELL32: &[&[Export]] = &[
    shell::EXPORTS,
    shell_folders::EXPORTS,
    shell_files::EXPORTS,
    shell_desktop::EXPORTS,
];

const USER32: &[&[Export]] = &[
    strings::USER32_EXPORTS,
    user32::EXPORTS,
    paint::USER32_EXPORTS,
    message_box::EXPORTS,
    window_text::EXPORTS,
    dialog::EXPORTS,
    menu::EXPORTS,
    timer::EXPORTS,
    ansi::EXPORTS,
    dialog_window::EXPORTS,
    placement::EXPORTS,
    clipboard::EXPORTS,
    icon::EXPORTS,
    subclass::EXPORTS,
    window_tree::EXPORTS,
    window_state::EXPORTS,
    window_class::EXPORTS,
    messages::EXPORTS,
    keyboard::EXPORTS,
    send_input::EXPORTS,
    monitor::EXPORTS,
    accelerator::EXPORTS,
    dialog_box::EXPORTS,
    menu_items::EXPORTS,
];

const COMDLG32: &[&[Export]] = &[file_picker::EXPORTS];

const GDI32: &[&[Export]] = &[paint::GDI32_EXPORTS, gdi::EXPORTS];

const COMCTL32: &[&[Export]] = common_controls::EXPORT_LISTS;

const GDIPLUS: &[&[Export]] = &[gdiplus::EXPORTS];

const OLE32: &[&[Export]] = &[com::OLE32_EXPORTS];

const OLEAUT32: &[&[Export]] = &[com::OLEAUT32_EXPORTS];

const ADVAPI32: &[&[Export]] = &[advapi32::EXPORTS];

const WSOCK32: &[&[Export]] = &[winsock::EXPORTS];

const WINMM: &[&[Export]] = &[winmm::EXPORTS];

const VERSION: &[&[Export]] = &[version::EXPORTS];

const PSAPI: &[&[Export]] = &[psapi::EXPORTS];

const WININET: &[&[Export]] = &[wininet::EXPORTS];

const SHLWAPI: &[&[Export]] = &[shlwapi::EXPORTS];

const UXTHEME: &[&[Export]] = &[uxtheme::EXPORTS];

const DWMAPI: &[&[Export]] = &[dwmapi::EXPORTS];

const BCRYPTPRIMITIVES: &[&[Export]] = &[random::EXPORTS];

const USERENV: &[&[Export]] = &[userenv::EXPORTS];

fn exports(library: Library) -> &'static [&'static [Export]] {
    match library {
        Library::Kernel32 => KERNEL32,
        Library::Ntdll => NTDLL,
        Library::Shell32 => SHELL32,
        Library::User32 => USER32,
        Library::Gdi32 => GDI32,
        Library::Comdlg32 => COMDLG32,
        Library::Comctl32 => COMCTL32,
        Library::VcRuntime => crt::VCRUNTIME,
        Library::Ucrt => crt::UCRT,
        Library::GdiPlus => GDIPLUS,
        Library::Ole32 => OLE32,
        Library::OleAut32 => OLEAUT32,
        Library::Advapi32 => ADVAPI32,
        Library::Wsock32 => WSOCK32,
        Library::Winmm => WINMM,
        Library::Version => VERSION,
        Library::Psapi => PSAPI,
        Library::Wininet => WININET,
        Library::Shlwapi => SHLWAPI,
        Library::UxTheme => UXTHEME,
        Library::Dwmapi => DWMAPI,
        Library::BcryptPrimitives => BCRYPTPRIMITIVES,
        Library::Userenv => USERENV,
        Library::Msvcrt => crt::MSVCRT,
    }
}

/// The address of `function` in the library an import or `LoadLibrary` names `library`.
pub fn resolve(library: &[u8], function: &[u8]) -> Option<u64> {
    let exports = exports(library::library(library)?);
    library::find(exports, function).map(|address| address as u64)
}

/// The address of the function an import names by `ordinal` in `library` (D174: comctl32's
/// `InitCommonControls`; D178: oleaut32's and wsock32's functions, which programs import only
/// so).
pub fn resolve_ordinal(library: &[u8], ordinal: u16) -> Option<u64> {
    let address = match library::library(library)? {
        Library::Comctl32 => common_controls::by_ordinal(ordinal),
        Library::OleAut32 => com::oleaut32_by_ordinal(ordinal),
        Library::Wsock32 => winsock::by_ordinal(ordinal),
        _ => None,
    };
    address.map(|address| address as u64)
}
