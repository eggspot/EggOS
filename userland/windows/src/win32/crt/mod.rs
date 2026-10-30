//! The C runtime of programs MSVC builds with `/MD` (D146): `vcruntime140.dll` and the
//! Universal C Runtime (`ucrtbase.dll`, imported through the `api-ms-win-crt-*` API sets),
//! implemented natively from Microsoft's public documentation of each function. MSVC links its
//! startup code (`wWinMainCRTStartup` and the rest) into the program itself; it calls the
//! runtime's startup and exit functions here (`runtime.rs`) and a few kernel32 functions of its
//! own (`startup.rs`). The rest is the C library: the heap (`heap.rs`), strings and characters
//! (`string.rs`), streams and `printf` (`stdio.rs`), and the utility and time functions
//! (`utility.rs`). Exception handling, C's and C++'s, and `signal` are in `win32/exception`
//! (D177); both runtimes' lists include their functions.

pub(super) use runtime::{end_abnormally, errno, set_errno};

mod heap;
mod runtime;
mod startup;
mod stdio;
mod string;
mod utility;

use crate::library::Export;

pub use startup::set_processor_features;

/// Writes out the C runtime's stream buffers as `ExitProcess` ends the process, as the Universal
/// C Runtime does when the process detaches it; nothing when the program opened no stream.
pub(super) fn flush_streams_at_process_exit() {
    stdio::flush_all_at_exit();
}

/// The kernel32 functions MSVC's startup code imports that kernel32 had not yet.
pub(super) const KERNEL32_EXPORTS: &[Export] = startup::KERNEL32_EXPORTS;

/// `vcruntime140.dll`'s functions.
pub(super) const VCRUNTIME: &[&[Export]] = &[
    super::memory::NTDLL_EXPORTS,
    startup::VCRUNTIME_EXPORTS,
    string::VCRUNTIME_EXPORTS,
    super::exception::C_SPECIFIC_EXPORTS,
    super::exception::CXX_EXPORTS,
    super::exception::TYPE_INFO_EXPORTS,
];

/// The Universal C Runtime's functions, whichever API set names them.
pub(super) const UCRT: &[&[Export]] = &[
    super::memory::NTDLL_EXPORTS,
    runtime::EXPORTS,
    heap::EXPORTS,
    string::EXPORTS,
    stdio::EXPORTS,
    utility::EXPORTS,
    super::exception::CXX_UCRT_EXPORTS,
    super::exception::SIGNAL_EXPORTS,
];
