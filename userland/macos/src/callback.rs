//! How the layer calls a function the program gave it (D191, slice 39e2c): every place that is
//! handed one of the program's functions (`atexit` and thread-local destructors, `qsort` and
//! `bsearch` comparators, `pthread_create` and `pthread_once` routines, block functions and
//! helpers, Objective-C methods) calls it through [`native_callable`], which is the address
//! itself for a native program and a thunk entering the translated code for an x86_64 program
//! on aarch64; signal handlers go through [`call_signal_handler`], which takes no lock.

use core::ffi::c_int;

/// A native function the layer can call for the program's function at `address`.
#[cfg(target_arch = "aarch64")]
pub fn native_callable(address: usize) -> usize {
    crate::translation::native_callable(address)
}

/// A native function the layer can call for the program's function at `address`.
#[cfg(not(target_arch = "aarch64"))]
pub fn native_callable(address: usize) -> usize {
    address
}

/// Calls the program's signal handler `handler` with `arguments` when it is translated code;
/// false when the caller is to call it natively. `context` is the kernel's `ucontext_t`.
#[cfg(target_arch = "aarch64")]
pub fn call_signal_handler(handler: usize, arguments: [u64; 3], context: *mut u8) -> bool {
    crate::translation::call_signal_handler(handler, arguments, context)
}

/// Calls the program's signal handler `handler` with `arguments` when it is translated code;
/// false when the caller is to call it natively. `context` is the kernel's `ucontext_t`.
#[cfg(not(target_arch = "aarch64"))]
pub fn call_signal_handler(_: usize, _: [u64; 3], _: *mut u8) -> bool {
    false
}

/// Completes a translated program's misaligned ordered access that raised the Linux signal
/// `signal` (`SIGBUS`, `BUS_ADRALN`), so the program resumes after it (D207); false for any
/// other signal. `info` and `context` are the kernel's `siginfo_t` and `ucontext_t`.
#[cfg(target_arch = "aarch64")]
pub fn completed_translated_fault(signal: c_int, info: *const u8, context: *mut u8) -> bool {
    crate::translation::completed_misaligned_fault(signal, info, context)
}

/// Completes a translated program's misaligned ordered access that raised the Linux signal
/// `signal` (`SIGBUS`, `BUS_ADRALN`), so the program resumes after it (D207); false for any
/// other signal. `info` and `context` are the kernel's `siginfo_t` and `ucontext_t`.
#[cfg(not(target_arch = "aarch64"))]
pub fn completed_translated_fault(_: c_int, _: *const u8, _: *mut u8) -> bool {
    false
}

/// The handler Linux runs for the fault signal `linux_signal` when the program asks for its
/// default action while a translated program runs, if the translation needs one.
#[cfg(target_arch = "aarch64")]
pub fn translated_fault_handler(linux_signal: usize) -> Option<usize> {
    crate::translation::fault_handler(linux_signal)
}

/// The handler Linux runs for the fault signal `linux_signal` when the program asks for its
/// default action while a translated program runs, if the translation needs one.
#[cfg(not(target_arch = "aarch64"))]
pub fn translated_fault_handler(_: usize) -> Option<usize> {
    None
}
