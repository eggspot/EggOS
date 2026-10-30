//! Signals with Darwin's numbers and structures (D190): `sigaction`, `__sigaction` and `signal`
//! record each action and install it in Linux, where every caught signal's handler is the
//! layer's `deliver`, which calls the program's handler with Darwin's signal number and, with
//! `SA_SIGINFO`, Darwin's `siginfo_t`; the handler returns through the kernel's trampoline and
//! `rt_sigreturn`. Ignoring and the default action go to Linux as they are (so an ignored
//! `SIGPIPE` turns a write to a closed pipe into `EPIPE`). Masks, `sigsuspend`, `sigpending` and
//! `sigaltstack` translate Darwin's sets and `stack_t`.

use super::{Exclusive, errno, errno_from, set_errno};
use crate::darwin::{self, EINVAL};
use crate::darwin_signals::{
    self, LAST_SIGNAL, LINUX_INFO_READ, SIGSTOP, SignalInfo, action, linux_action,
};
use crate::linux;
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use eggos_runtime::errno::Errno;

const SIGNALS: usize = LAST_SIGNAL as usize + 1;
const SIGKILL: c_int = 9;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
const SIG_ERR: usize = usize::MAX;
const SS_ONSTACK: c_int = 1;
const SS_DISABLE: c_int = 4;
const LINUX_SS_ONSTACK: i32 = 1;
const LINUX_SS_DISABLE: i32 = 2;
const MINSIGSTKSZ: usize = 32 << 10;
const ENOMEM: c_int = 12;
const EINTR: c_int = 4;
const SIGSET_SIZE: usize = 8;
const LINUX_SIG_BLOCK: usize = 0;
const LINUX_SIG_SETMASK: usize = 2;
const ALL_SIGNALS: u64 = u64::MAX;

/// Darwin's `struct sigaction`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Action {
    handler: usize,
    mask: u32,
    flags: c_int,
}

/// Darwin's `struct __sigaction`, which its `__sigaction` system call reads: `sigaction`'s
/// fields with the trampoline libSystem passes, which the layer does not need.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrampolineAction {
    handler: usize,
    trampoline: usize,
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct LinuxStack {
    pointer: usize,
    flags: i32,
    size: usize,
}

type Handler = extern "C" fn(c_int, *mut SignalInfo, *mut c_void);

static HANDLERS: [AtomicUsize; SIGNALS] = [const { AtomicUsize::new(SIG_DFL) }; SIGNALS];
static MASKS: [AtomicU32; SIGNALS] = [const { AtomicU32::new(0) }; SIGNALS];
static FLAGS: [AtomicI32; SIGNALS] = [const { AtomicI32::new(0) }; SIGNALS];
static CHANGING: Exclusive<()> = Exclusive::new(());

fn recorded(index: usize) -> Action {
    Action {
        handler: HANDLERS[index].load(Ordering::Acquire),
        mask: MASKS[index].load(Ordering::Relaxed),
        flags: FLAGS[index].load(Ordering::Relaxed),
    }
}

fn record(index: usize, action: Action) {
    MASKS[index].store(action.mask, Ordering::Relaxed);
    FLAGS[index].store(action.flags, Ordering::Relaxed);
    HANDLERS[index].store(action.handler, Ordering::Release);
}

fn linux_words(action: &Action) -> [usize; 4] {
    let flags = darwin_signals::linux_action_flags(action.flags);
    let (handler, flags) = match action.handler {
        SIG_DFL | SIG_IGN => (action.handler, flags & !linux_action::SIGINFO),
        _ => (deliver as *const () as usize, flags | linux_action::SIGINFO),
    };
    let mask = darwin_signals::linux_signal_set(action.mask);
    [handler, flags as usize, 0, mask as usize]
}

fn install_linux(
    linux_signal: usize,
    words: &[usize; 4],
    old: Option<&mut [usize; 4]>,
) -> Result<usize, Errno> {
    let old = old.map_or(0, |old| old.as_mut_ptr() as usize);
    // SAFETY: `rt_sigaction` reads one `struct sigaction` from the borrowed words and writes
    // the previous one into `old`'s, if given.
    unsafe {
        linux::syscall(
            linux::number::RT_SIGACTION,
            [
                linux_signal,
                words.as_ptr() as usize,
                old,
                SIGSET_SIZE,
                0,
                0,
            ],
        )
    }
}

fn change(signal: c_int, new: Option<Action>) -> Result<Action, c_int> {
    let changes_fixed = signal == SIGKILL || signal == SIGSTOP;
    if !(1..SIGNALS as c_int).contains(&signal) || (new.is_some() && changes_fixed) {
        return Err(EINVAL);
    }
    let index = signal as usize;
    CHANGING.with(|_| {
        let previous = recorded(index);
        let Some(new) = new else {
            return Ok(previous);
        };
        record(index, new);
        if let Some(linux_signal) = darwin::linux_signal(signal)
            && let Err(errno) = install_linux(linux_signal, &linux_words(&new), None)
        {
            record(index, previous);
            return Err(darwin::darwin_errno(errno.0));
        }
        Ok(previous)
    })
}

extern "C" fn deliver(
    linux_signal: c_int,
    info: *const [u8; LINUX_INFO_READ],
    context: *mut c_void,
) {
    let Some(signal) = darwin_signals::darwin_signal(linux_signal as usize) else {
        return;
    };
    let index = signal as usize;
    let handler = HANDLERS[index].load(Ordering::Acquire);
    let flags = FLAGS[index].load(Ordering::Relaxed);
    if handler == SIG_DFL || handler == SIG_IGN {
        return;
    }
    if flags & action::RESETHAND != 0 {
        FLAGS[index].store(flags & !action::SIGINFO, Ordering::Relaxed);
        HANDLERS[index].store(SIG_DFL, Ordering::Release);
    }
    let mut darwin_info = if info.is_null() {
        SignalInfo::default()
    } else {
        // SAFETY: Linux passes a handler installed with `SA_SIGINFO` its 128-byte
        // `siginfo_t`, whose start this reads.
        darwin_signals::darwin_signal_info(unsafe { &*info })
    };
    darwin_info.signal = signal;
    let saved_errno = errno();
    let arguments = [signal as u64, (&raw mut darwin_info) as u64, 0];
    if !crate::callback::call_signal_handler(handler, arguments, context.cast()) {
        // SAFETY: the program installed `handler` as a signal handler, which takes the signal
        // number, Darwin's `siginfo_t` and a context (a handler that takes only the number
        // ignores the rest).
        let handler = unsafe { core::mem::transmute::<usize, Handler>(handler) };
        handler(signal, &mut darwin_info, core::ptr::null_mut());
    }
    set_errno(saved_errno);
}

fn finish(signal: c_int, new: Option<Action>, old: *mut Action) -> c_int {
    match change(signal, new) {
        Ok(previous) => {
            if !old.is_null() {
                // SAFETY: the program passes a writable `old`, or null.
                unsafe { old.write(previous) };
            }
            0
        }
        Err(errno) => {
            set_errno(errno);
            -1
        }
    }
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
    finish(signal, new, old)
}

/// `__sigaction`, the system call libSystem's own `sigaction` makes.
///
/// # Safety
/// As [`sigaction`].
pub unsafe extern "C" fn trampoline_sigaction(
    signal: c_int,
    action: *const TrampolineAction,
    old: *mut Action,
) -> c_int {
    let new = if action.is_null() {
        None
    } else {
        // SAFETY: the caller's promise.
        let action = unsafe { action.read() };
        Some(Action {
            handler: action.handler,
            mask: action.mask,
            flags: action.flags,
        })
    };
    finish(signal, new, old)
}

/// BSD's `signal`: the handler, with no signals blocked while it runs, and system calls it
/// interrupts restarted.
pub extern "C" fn signal(signal: c_int, handler: usize) -> usize {
    let new = Action {
        handler,
        mask: 0,
        flags: action::RESTART,
    };
    match change(signal, Some(new)) {
        Ok(previous) => previous.handler,
        Err(errno) => {
            set_errno(errno);
            SIG_ERR
        }
    }
}

fn install_default(linux_signal: usize) -> Result<usize, Errno> {
    install_linux(linux_signal, &[SIG_DFL, 0, 0, 0], None)
}

/// Gives each signal of the Darwin set `set` its default action (`POSIX_SPAWN_SETSIGDEF`).
pub fn set_default(set: u32) -> Result<(), Errno> {
    for signal in 1..=LAST_SIGNAL {
        if set & (1 << (signal - 1)) != 0
            && let Some(linux_signal) = darwin::linux_signal(signal)
        {
            install_default(linux_signal)?;
        }
    }
    Ok(())
}

/// Gives each signal the program catches its default action, in a new child that will run
/// another program, so none of the program's handlers runs there.
pub fn reset_caught() {
    for (signal, handler) in (0..).zip(HANDLERS.iter()) {
        let handler = handler.load(Ordering::Relaxed);
        if handler != SIG_DFL
            && handler != SIG_IGN
            && let Some(linux_signal) = darwin::linux_signal(signal)
        {
            let _ = install_default(linux_signal);
        }
    }
}

/// Runs `work` with the Linux signals `signals` ignored, then gives them back their actions:
/// how `system` waits for its shell.
pub fn with_ignored<R>(signals: &[usize; 2], work: impl FnOnce() -> R) -> R {
    let mut saved = [[0; 4]; 2];
    let mut ignored = [false; 2];
    for ((signal, saved), ignored) in signals.iter().zip(&mut saved).zip(&mut ignored) {
        *ignored = install_linux(*signal, &[SIG_IGN, 0, 0, 0], Some(saved)).is_ok();
    }
    let result = work();
    for ((signal, saved), ignored) in signals.iter().zip(&saved).zip(ignored) {
        if ignored {
            let _ = install_linux(*signal, saved, None);
        }
    }
    result
}

/// Sets the calling thread's Linux signal mask to `mask` and returns the one it had.
pub fn swap_linux_mask(mask: u64) -> Result<u64, Errno> {
    let mut previous = 0u64;
    // SAFETY: `rt_sigprocmask` reads the borrowed mask and writes the previous one.
    unsafe {
        linux::syscall(
            linux::number::RT_SIGPROCMASK,
            [
                LINUX_SIG_SETMASK,
                &raw const mask as usize,
                &raw mut previous as usize,
                SIGSET_SIZE,
                0,
                0,
            ],
        )
    }?;
    Ok(previous)
}

/// Blocks every signal that can be blocked and returns the mask the thread had.
pub fn block_all() -> Result<u64, Errno> {
    swap_linux_mask(ALL_SIGNALS)
}

/// Adds the Linux signals of `set` to the calling thread's mask and returns the mask it had.
pub fn block_linux(set: u64) -> Result<u64, Errno> {
    let mut previous = 0u64;
    // SAFETY: `rt_sigprocmask` reads the borrowed set and writes the previous mask.
    unsafe {
        linux::syscall(
            linux::number::RT_SIGPROCMASK,
            [
                LINUX_SIG_BLOCK,
                &raw const set as usize,
                &raw mut previous as usize,
                SIGSET_SIZE,
                0,
                0,
            ],
        )
    }?;
    Ok(previous)
}

/// # Safety
/// `set` must be null or readable and `old` null or writable.
unsafe fn change_mask(how: c_int, set: *const u32, old: *mut u32) -> Result<(), c_int> {
    let new = if set.is_null() {
        None
    } else {
        // SAFETY: the caller's promise.
        Some(darwin_signals::linux_signal_set(unsafe { set.read() }))
    };
    let linux_how = match new {
        Some(_) => darwin_signals::linux_mask_how(how).ok_or(EINVAL)?,
        None => 0,
    };
    let new_pointer = new.as_ref().map_or(0, |set| set as *const u64 as usize);
    let mut previous = 0u64;
    // SAFETY: `rt_sigprocmask` reads the borrowed new set, if any, and writes the previous one.
    unsafe {
        linux::syscall(
            linux::number::RT_SIGPROCMASK,
            [
                linux_how,
                new_pointer,
                &raw mut previous as usize,
                SIGSET_SIZE,
                0,
                0,
            ],
        )
    }
    .map_err(|errno| darwin::darwin_errno(errno.0))?;
    if !old.is_null() {
        // SAFETY: the caller's promise.
        unsafe { old.write(darwin_signals::darwin_signal_set(previous)) };
    }
    Ok(())
}

/// # Safety
/// `set` must be null or readable and `old` null or writable.
pub unsafe extern "C" fn sigprocmask(how: c_int, set: *const u32, old: *mut u32) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { change_mask(how, set, old) } {
        Ok(()) => 0,
        Err(errno) => {
            set_errno(errno);
            -1
        }
    }
}

/// # Safety
/// As [`sigprocmask`].
pub unsafe extern "C" fn pthread_sigmask(how: c_int, set: *const u32, old: *mut u32) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { change_mask(how, set, old) } {
        Ok(()) => 0,
        Err(errno) => errno,
    }
}

/// # Safety
/// `set` must be readable.
pub unsafe extern "C" fn sigsuspend(set: *const u32) -> c_int {
    // SAFETY: the caller's promise.
    let mask = darwin_signals::linux_signal_set(unsafe { set.read() });
    // SAFETY: `rt_sigsuspend` reads the borrowed mask; it returns once a handler has run.
    let suspended = unsafe {
        linux::syscall(
            linux::number::RT_SIGSUSPEND,
            [&raw const mask as usize, SIGSET_SIZE, 0, 0, 0, 0],
        )
    };
    match suspended {
        Ok(_) => {
            set_errno(EINTR);
            -1
        }
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `set` must be writable.
pub unsafe extern "C" fn sigpending(set: *mut u32) -> c_int {
    let mut pending = 0u64;
    // SAFETY: `rt_sigpending` writes one signal set into the borrowed word.
    let read = unsafe {
        linux::syscall(
            linux::number::RT_SIGPENDING,
            [&raw mut pending as usize, SIGSET_SIZE, 0, 0, 0, 0],
        )
    };
    if let Err(errno) = read {
        return errno_from(errno, -1);
    }
    // SAFETY: the caller's promise.
    unsafe { set.write(darwin_signals::darwin_signal_set(pending)) };
    0
}

/// # Safety
/// `new` must be null or readable and `old` null or writable.
pub unsafe extern "C" fn sigaltstack(new: *const Stack, old: *mut Stack) -> c_int {
    let linux_new = if new.is_null() {
        None
    } else {
        // SAFETY: the caller's promise.
        let new = unsafe { new.read() };
        if new.flags & SS_DISABLE != 0 {
            Some(LinuxStack {
                flags: LINUX_SS_DISABLE,
                ..LinuxStack::default()
            })
        } else if new.size < MINSIGSTKSZ {
            set_errno(ENOMEM);
            return -1;
        } else {
            Some(LinuxStack {
                pointer: new.pointer,
                flags: 0,
                size: new.size,
            })
        }
    };
    let new_pointer = linux_new
        .as_ref()
        .map_or(0, |stack| stack as *const LinuxStack as usize);
    let mut linux_old = LinuxStack::default();
    // SAFETY: `sigaltstack` reads the borrowed new stack, if any, and writes the old one.
    let changed = unsafe {
        linux::syscall(
            linux::number::SIGALTSTACK,
            [new_pointer, &raw mut linux_old as usize, 0, 0, 0, 0],
        )
    };
    if let Err(errno) = changed {
        return errno_from(errno, -1);
    }
    if !old.is_null() {
        let mut flags = 0;
        if linux_old.flags & LINUX_SS_DISABLE != 0 {
            flags |= SS_DISABLE;
        }
        if linux_old.flags & LINUX_SS_ONSTACK != 0 {
            flags |= SS_ONSTACK;
        }
        // SAFETY: the caller's promise.
        unsafe {
            old.write(Stack {
                pointer: linux_old.pointer,
                size: linux_old.size,
                flags,
            })
        };
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
    (1..=LAST_SIGNAL)
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
