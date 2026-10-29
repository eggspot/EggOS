//! The C runtime of programs MSVC builds with `/MD` (D145): `vcruntime140.dll` and the
//! Universal C Runtime (`ucrtbase.dll`, imported through the `api-ms-win-crt-*` API sets),
//! implemented natively from Microsoft's public documentation of each function. MSVC links its
//! startup code (`wWinMainCRTStartup` and the rest) into the program itself; it calls the
//! runtime's startup and exit functions here (`runtime.rs`) and a few kernel32 functions of its
//! own (`startup.rs`). The rest is the C library: the heap (`heap.rs`), strings and characters
//! (`string.rs`), streams and `printf` (`stdio.rs`), and the utility and time functions
//! (`utility.rs`).

mod heap;
mod runtime;
mod startup;
mod stdio;
mod string;
mod utility;

use crate::library::Export;

/// The kernel32 functions MSVC's startup code imports that kernel32 had not yet.
pub(super) const KERNEL32_EXPORTS: &[Export] = startup::KERNEL32_EXPORTS;

/// `vcruntime140.dll`'s functions.
pub(super) const VCRUNTIME: &[&[Export]] = &[
    super::memory::NTDLL_EXPORTS,
    startup::VCRUNTIME_EXPORTS,
    string::VCRUNTIME_EXPORTS,
];

/// The Universal C Runtime's functions, whichever API set names them.
pub(super) const UCRT: &[&[Export]] = &[
    super::memory::NTDLL_EXPORTS,
    runtime::EXPORTS,
    heap::EXPORTS,
    string::EXPORTS,
    stdio::EXPORTS,
    utility::EXPORTS,
];
