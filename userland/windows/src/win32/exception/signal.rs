//! C's `signal` and `raise`, and `_seh_filter_exe`, the filter around `main` in MSVC's startup
//! (D177), as Microsoft documents them: a program sets a handler for `SIGINT`, `SIGILL`,
//! `SIGFPE`, `SIGSEGV`, `SIGTERM`, `SIGBREAK` or `SIGABRT`; `raise` calls it (the handler is
//! reset to the default first), or ends the program for the default. An access violation, an
//! illegal instruction or an arithmetic exception no handler took reaches the startup's filter,
//! which calls the matching signal's handler (`SIGFPE`'s with the kind of error) and continues
//! the program where it stopped, ignores it for `SIG_IGN`, and lets it go on otherwise.

use super::super::crt::{end_abnormally, errno, set_errno};
use super::filter;
use super::frames::{self, Bridge};
use crate::fault::status;
use crate::library::Export;
use core::sync::atomic::{AtomicU64, Ordering};

const SIGINT: i32 = 2;
const SIGILL: i32 = 4;
const SIGABRT_COMPATIBLE: i32 = 6;
const SIGFPE: i32 = 8;
const SIGSEGV: i32 = 11;
const SIGTERM: i32 = 15;
const SIGBREAK: i32 = 21;
const SIGABRT: i32 = 22;
const SIGNAL_COUNT: usize = 23;
const DEFAULT: u64 = 0;
const IGNORE: u64 = 1;
const ERROR: u64 = u64::MAX;
const FPE_ZERO_DIVIDE: u64 = 0x83;
const FPE_OVERFLOW: u64 = 0x84;
const FPE_UNDERFLOW: u64 = 0x85;
const FPE_INEXACT: u64 = 0x86;
const FPE_INVALID: u64 = 0x81;
const PRIVILEGED_INSTRUCTION: u32 = 0xc000_0096;
const CONTROL_C_EXIT: u32 = 0xc000_013a;
const ABORT_STATUS: u32 = 3;

static HANDLERS: [AtomicU64; SIGNAL_COUNT] = [const { AtomicU64::new(DEFAULT) }; SIGNAL_COUNT];

fn index(number: i32) -> Option<usize> {
    match number {
        SIGINT | SIGILL | SIGABRT_COMPATIBLE | SIGFPE | SIGSEGV | SIGTERM | SIGBREAK | SIGABRT => {
            Some(number as usize)
        }
        _ => None,
    }
}

fn call(handler: u64, number: i32, detail: u64) {
    let mut bridge = Bridge::new(None, None);
    // SAFETY: the program set `handler` with `signal`; it takes the signal's number (and for
    // `SIGFPE` the kind of error).
    unsafe { frames::call_program(&mut bridge, handler, [number as u64, detail, 0, 0]) };
}

extern "efiapi" fn signal(number: i32, handler: u64) -> u64 {
    match index(number) {
        Some(slot) => HANDLERS[slot].swap(handler, Ordering::AcqRel),
        None => {
            set_errno(errno::EINVAL);
            ERROR
        }
    }
}

extern "efiapi" fn raise(number: i32) -> i32 {
    let Some(slot) = index(number) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    match HANDLERS[slot].swap(DEFAULT, Ordering::AcqRel) {
        IGNORE => {
            HANDLERS[slot].store(IGNORE, Ordering::Release);
            0
        }
        DEFAULT => match number {
            SIGINT | SIGBREAK => end_abnormally(b"the program raised SIGINT", CONTROL_C_EXIT),
            _ => end_abnormally(
                b"the program raised a signal it has no handler for",
                ABORT_STATUS,
            ),
        },
        handler => {
            call(handler, number, 0);
            0
        }
    }
}

fn signal_of(code: u32) -> Option<(i32, u64)> {
    match code {
        status::ACCESS_VIOLATION => Some((SIGSEGV, 0)),
        status::ILLEGAL_INSTRUCTION | PRIVILEGED_INSTRUCTION => Some((SIGILL, 0)),
        status::INTEGER_DIVIDE_BY_ZERO | status::FLOAT_DIVIDE_BY_ZERO => {
            Some((SIGFPE, FPE_ZERO_DIVIDE))
        }
        status::INTEGER_OVERFLOW | status::FLOAT_OVERFLOW => Some((SIGFPE, FPE_OVERFLOW)),
        status::FLOAT_UNDERFLOW => Some((SIGFPE, FPE_UNDERFLOW)),
        status::FLOAT_INEXACT_RESULT => Some((SIGFPE, FPE_INEXACT)),
        status::FLOAT_INVALID_OPERATION => Some((SIGFPE, FPE_INVALID)),
        _ => None,
    }
}

extern "efiapi" fn seh_filter_exe(code: u32, _pointers: u64) -> i32 {
    let Some((number, detail)) = signal_of(code) else {
        return filter::CONTINUE_SEARCH;
    };
    let slot = number as usize;
    match HANDLERS[slot].load(Ordering::Acquire) {
        DEFAULT => filter::CONTINUE_SEARCH,
        IGNORE => filter::CONTINUE_EXECUTION,
        handler => {
            HANDLERS[slot].store(DEFAULT, Ordering::Release);
            call(handler, number, detail);
            filter::CONTINUE_EXECUTION
        }
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"signal" => signal,
    b"raise" => raise,
    b"_seh_filter_exe" => seh_filter_exe,
];

/// The old C runtime's (`msvcrt.dll`, D189) name for the startup's filter.
pub(super) const MSVCRT_EXPORTS: &[Export] = exports![
    b"_XcptFilter" => seh_filter_exe,
];
