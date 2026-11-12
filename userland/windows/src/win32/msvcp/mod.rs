//! msvcp140, the C++ standard library's runtime of MSVC's `/MD` programs (D230), under its own
//! name and those of its companions (`msvcp140_1`, `msvcp140_2`, `msvcp140_atomic_wait`): the
//! throwers the headers call (`errors.rs`), the threads, mutexes, condition variables and
//! clocks (`threads.rs`), `std::exception_ptr` and `std::uncaught_exceptions` (`win32/exception`),
//! and iostreams for `char` (`iostreams.rs`, D405).

mod errors;
mod iostreams;
mod threads;

use crate::library::Export;

/// msvcp140's functions.
pub(super) const MSVCP140: &[&[Export]] = &[
    errors::EXPORTS,
    threads::EXPORTS,
    iostreams::EXPORTS,
    iostreams::DATA_EXPORTS,
    super::exception::EXCEPTION_PTR_EXPORTS,
    super::exception::UNCAUGHT_EXPORTS,
];

/// The Universal C Runtime's thread functions, which `std::thread` starts its threads with.
pub(super) const UCRT_THREAD_EXPORTS: &[Export] = threads::UCRT_EXPORTS;
