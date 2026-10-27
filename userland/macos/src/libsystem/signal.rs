//! Signals, as far as console tools and Rust's standard library need them: `sigaction` and
//! `signal` record each handler, and pass ignoring or the default action on to Linux (so an
//! ignored `SIGPIPE` turns a write to a closed pipe into `EPIPE`); handlers are recorded but
//! not delivered yet. `sigaltstack` records each thread's alternate stack, signal masks are
//! accepted and empty, and `pause` sleeps until a signal arrives.

use super::{Exclusive, set_errno, thread};
use crate::darwin::{self, EINVAL};
use crate::linux;
use core::ffi::c_int;

const SIGNALS: usize = 32;
const SIGKILL: c_int = 9;
const SIGSTOP: c_int = 17;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
const SIG_ERR: usize = usize::MAX;
const SS_DISABLE: c_int = 4;
const MINSIGSTKSZ: usize = 32 << 10;
const ENOMEM: c_int = 12;
const EINTR: c_int = 4;

/// Darwin's `struct sigaction`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Action {
    handler: usize,
    mask: u32,
    flags: c_int,
}

/// Darwin's `stack_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Stack {
    pointer: usize,
    size: usize,
    flags: c_int,
}

static ACTIONS: Exclusive<[(usize, u32, c_int); SIGNALS]> = Exclusive::new([(0, 0, 0); SIGNALS]);

fn change(signal: c_int, new: Option<Action>) -> Option<Action> {
    if !(1..SIGNALS as c_int).contains(&signal)
        || (new.is_some() && (signal == SIGKILL || signal == SIGSTOP))
    {
        return None;
    }
    let previous = ACTIONS.with(|actions| {
        let slot = &mut actions[signal as usize];
        let previous = *slot;
        if let Some(new) = new {
            *slot = (new.handler, new.mask, new.flags);
        }
        previous
    });
    if let Some(new) = new
        && (new.handler == SIG_DFL || new.handler == SIG_IGN)
        && let Some(linux_signal) = darwin::linux_signal(signal)
    {
        let action = [new.handler, 0, 0, 0];
        // SAFETY: `rt_sigaction` reads one `struct sigaction` from the borrowed words.
        let _ = unsafe {
            linux::syscall(
                linux::number::RT_SIGACTION,
                [linux_signal, action.as_ptr() as usize, 0, 8, 0, 0],
            )
        };
    }
    Some(Action {
        handler: previous.0,
        mask: previous.1,
        flags: previous.2,
    })
}

/// # Safety
/// `action` must be null or readable and `old` null or writable.
pub unsafe extern "C" fn sigaction(
    signal: c_int,
    action: *const Action,
    old: *mut Action,
) -> c_int {
    let new = if action.is_null() {
        None
    } else {
        // SAFETY: the caller's promise.
        Some(unsafe { action.read() })
    };
    let Some(previous) = change(signal, new) else {
        set_errno(EINVAL);
        return -1;
    };
    if !old.is_null() {
        // SAFETY: the caller's promise.
        unsafe { old.write(previous) };
    }
    0
}

pub extern "C" fn signal(signal: c_int, handler: usize) -> usize {
    let new = Action {
        handler,
        mask: 0,
        flags: 0,
    };
    match change(signal, Some(new)) {
        Some(previous) => previous.handler,
        None => {
            set_errno(EINVAL);
            SIG_ERR
        }
    }
}

/// # Safety
/// `new` must be null or readable and `old` null or writable.
pub unsafe extern "C" fn sigaltstack(new: *const Stack, old: *mut Stack) -> c_int {
    // SAFETY: the calling thread's own control block.
    let current = unsafe { &mut (*thread::current()).alternate_stack };
    let [pointer, size, enabled] = *current;
    if !new.is_null() {
        // SAFETY: the caller's promise.
        let new = unsafe { new.read() };
        if new.flags & SS_DISABLE != 0 {
            *current = [0, 0, 0];
        } else if new.size < MINSIGSTKSZ {
            set_errno(ENOMEM);
            return -1;
        } else {
            *current = [new.pointer, new.size, 1];
        }
    }
    if !old.is_null() {
        let flags = if enabled == 0 { SS_DISABLE } else { 0 };
        // SAFETY: the caller's promise.
        unsafe {
            old.write(Stack {
                pointer,
                size,
                flags,
            })
        };
    }
    0
}

/// # Safety
/// `old` must be null or writable.
pub unsafe extern "C" fn sigprocmask(_: c_int, _: *const u32, old: *mut u32) -> c_int {
    if !old.is_null() {
        // SAFETY: the caller's promise.
        unsafe { old.write(0) };
    }
    0
}

/// # Safety
/// `set` must be writable.
pub unsafe extern "C" fn sigemptyset(set: *mut u32) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { set.write(0) };
    0
}

/// # Safety
/// As [`sigemptyset`].
pub unsafe extern "C" fn sigfillset(set: *mut u32) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { set.write(u32::MAX) };
    0
}

fn bit(signal: c_int) -> Option<u32> {
    (1..=SIGNALS as c_int)
        .contains(&signal)
        .then(|| 1u32 << (signal - 1))
}

/// # Safety
/// As [`sigemptyset`].
pub unsafe extern "C" fn sigaddset(set: *mut u32, signal: c_int) -> c_int {
    let Some(bit) = bit(signal) else {
        set_errno(EINVAL);
        return -1;
    };
    // SAFETY: the caller's promise.
    unsafe { *set |= bit };
    0
}

/// # Safety
/// As [`sigemptyset`].
pub unsafe extern "C" fn sigdelset(set: *mut u32, signal: c_int) -> c_int {
    let Some(bit) = bit(signal) else {
        set_errno(EINVAL);
        return -1;
    };
    // SAFETY: the caller's promise.
    unsafe { *set &= !bit };
    0
}

/// # Safety
/// `set` must be readable.
pub unsafe extern "C" fn sigismember(set: *const u32, signal: c_int) -> c_int {
    let Some(bit) = bit(signal) else {
        set_errno(EINVAL);
        return -1;
    };
    // SAFETY: the caller's promise.
    c_int::from(unsafe { set.read() } & bit != 0)
}

pub extern "C" fn pause() -> c_int {
    // SAFETY: `ppoll` with no descriptors and no timeout reads no memory; it returns when a
    // signal arrives.
    let _ = unsafe { linux::syscall(linux::number::PPOLL, [0, 0, 0, 0, 8, 0]) };
    set_errno(EINTR);
    -1
}
