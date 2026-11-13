//! The program's hooks (D349): `SetWindowsHookExW` and `A` install a hook procedure that sees
//! the process's own messages and input, for one thread or all of them, and `CallNextHookEx`
//! passes each call down the chain, newest hook first, as on Windows. The layer calls:
//! `WH_GETMESSAGE`, `WH_KEYBOARD` and `WH_MOUSE` as `GetMessage` and `PeekMessage` return a
//! message (the keyboard and mouse hooks may discard it); `WH_CALLWNDPROC` and
//! `WH_CALLWNDPROCRET` around every sent message; `WH_KEYBOARD_LL` and `WH_MOUSE_LL` for the
//! window server's input to the process before it is routed (they may swallow it); and
//! `WH_CBT` as a window is made (which it may refuse) and destroyed. Hooks of the other kinds
//! are kept and chained but never called. EggOS has no hooks into other processes.

use super::user32::Msg;
use super::{SpinLock, call_program, thread_environment};
use crate::gui::message;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The hook kinds (`WH_*`).
pub mod kind {
    pub const KEYBOARD: i32 = 2;
    pub const GET_MESSAGE: i32 = 3;
    pub const CALL_WND_PROC: i32 = 4;
    pub const CBT: i32 = 5;
    pub const MOUSE: i32 = 7;
    pub const CALL_WND_PROC_RET: i32 = 12;
    pub const KEYBOARD_LL: i32 = 13;
    pub const MOUSE_LL: i32 = 14;
}

const MOST_HOOKS: usize = 32;
const HC_ACTION: i32 = 0;
const HC_NOREMOVE: i32 = 3;
const HCBT_CREATEWND: i32 = 3;
const HCBT_DESTROYWND: i32 = 4;
const PM_REMOVE: usize = 1;
const HTCLIENT: u32 = 1;
const FIRST_KEY: u32 = 0x0100;
const LAST_KEY: u32 = 0x0109;
const FIRST_MOUSE: u32 = 0x0200;
const LAST_MOUSE: u32 = 0x020e;
const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_ALTDOWN: u32 = 0x20;
const LLKHF_UP: u32 = 0x80;
const FIRST_HANDLE: usize = 0x6_0001;

#[derive(Debug, Clone, Copy)]
struct Hook {
    handle: usize,
    kind: i32,
    procedure: usize,
    thread: u32,
}

static HOOKS: SpinLock<[Option<Hook>; MOST_HOOKS]> = SpinLock::new([None; MOST_HOOKS]);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(FIRST_HANDLE);

/// Installs `procedure` as a hook of `kind` for `thread` (0: every thread of the process); its
/// handle, or `None` when the table is full.
pub fn install(kind: i32, procedure: usize, thread: u32) -> Option<usize> {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let hook = Hook {
        handle,
        kind,
        procedure,
        thread,
    };
    let installed = HOOKS.with(|hooks| {
        let slot = hooks.iter_mut().find(|slot| slot.is_none())?;
        *slot = Some(hook);
        Some(handle)
    });
    if installed.is_some() {
        INSTALLED.fetch_add(1, Ordering::AcqRel);
    }
    installed
}

/// Removes hook `handle`; whether there was one.
pub fn remove(handle: usize) -> bool {
    let removed = HOOKS.with(|hooks| {
        let slot = hooks
            .iter_mut()
            .find(|slot| slot.is_some_and(|hook| hook.handle == handle))?;
        *slot = None;
        Some(())
    });
    if removed.is_some() {
        INSTALLED.fetch_sub(1, Ordering::AcqRel);
    }
    removed.is_some()
}

fn next(kind: i32, before: Option<usize>) -> Option<Hook> {
    if INSTALLED.load(Ordering::Acquire) == 0 {
        return None;
    }
    let thread = thread_environment::thread_id();
    HOOKS.with(|hooks| {
        hooks
            .iter()
            .flatten()
            .filter(|hook| hook.kind == kind && (hook.thread == 0 || hook.thread == thread))
            .filter(|hook| before.is_none_or(|before| hook.handle < before))
            .max_by_key(|hook| hook.handle)
            .copied()
    })
}

fn run(hook: Hook, code: i32, wparam: usize, lparam: usize) -> usize {
    // SAFETY: the program's `HOOKPROC`, called as Windows calls it.
    unsafe { call_program(hook.procedure, [code as isize as usize, wparam, lparam, 0]) }
}

fn call(kind: i32, code: i32, wparam: usize, lparam: usize) -> Option<usize> {
    next(kind, None).map(|hook| run(hook, code, wparam, lparam))
}

/// `CallNextHookEx`: the next hook after `handle` in its chain, or 0 at the chain's end.
pub fn call_next(handle: usize, code: i32, wparam: usize, lparam: usize) -> usize {
    let kind = HOOKS.with(|hooks| {
        hooks
            .iter()
            .flatten()
            .find(|hook| hook.handle == handle)
            .map(|hook| hook.kind)
    });
    kind.and_then(|kind| next(kind, Some(handle)))
        .map_or(0, |hook| run(hook, code, wparam, lparam))
}

#[repr(C)]
struct MouseHook {
    x: i32,
    y: i32,
    window: usize,
    hit_test: u32,
    extra: usize,
}

/// The hooks a message `GetMessage` or `PeekMessage` returns in `out` goes through, `removed`
/// from the queue or not: `WH_GETMESSAGE`, which may change it, then `WH_KEYBOARD` or
/// `WH_MOUSE`; whether one of those discards it.
pub fn retrieved(out: *mut Msg, removed: bool) -> bool {
    if INSTALLED.load(Ordering::Acquire) == 0 {
        return false;
    }
    let removal = if removed { PM_REMOVE } else { 0 };
    call(kind::GET_MESSAGE, HC_ACTION, removal, out as usize);
    // SAFETY: `out` is the caller's `MSG`, which the hook may have changed.
    let message = unsafe { super::user32::read_message(out) };
    let code = if removed { HC_ACTION } else { HC_NOREMOVE };
    let answer = match message.message {
        FIRST_KEY..=LAST_KEY => call(kind::KEYBOARD, code, message.wparam, message.lparam),
        FIRST_MOUSE..=LAST_MOUSE => {
            let info = MouseHook {
                x: message.point.0,
                y: message.point.1,
                window: message.window,
                hit_test: HTCLIENT,
                extra: 0,
            };
            call(
                kind::MOUSE,
                code,
                message.message as usize,
                &raw const info as usize,
            )
        }
        _ => None,
    };
    answer.is_some_and(|answer| answer != 0)
}

#[repr(C)]
struct SentMessage {
    lparam: usize,
    wparam: usize,
    message: u32,
    window: usize,
}

#[repr(C)]
struct SentResult {
    result: usize,
    lparam: usize,
    wparam: usize,
    message: u32,
    window: usize,
}

/// Whether sent messages go through hooks, so the send path pays nothing otherwise.
pub fn watching_sends() -> bool {
    INSTALLED.load(Ordering::Acquire) != 0
}

/// `WH_CALLWNDPROC`, before `number` goes to `window`'s procedure.
pub fn before_send(window: usize, number: u32, wparam: usize, lparam: usize) {
    let sent = SentMessage {
        lparam,
        wparam,
        message: number,
        window,
    };
    call(kind::CALL_WND_PROC, HC_ACTION, 1, &raw const sent as usize);
}

/// `WH_CALLWNDPROCRET`, after `window`'s procedure answered `number` with `result`.
pub fn after_send(window: usize, number: u32, wparam: usize, lparam: usize, result: usize) {
    let answered = SentResult {
        result,
        lparam,
        wparam,
        message: number,
        window,
    };
    call(
        kind::CALL_WND_PROC_RET,
        HC_ACTION,
        1,
        &raw const answered as usize,
    );
}

#[repr(C)]
struct KeyboardLowLevel {
    virtual_key: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
struct MouseLowLevel {
    x: i32,
    y: i32,
    data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

/// `WH_KEYBOARD_LL` or `WH_MOUSE_LL` for an input of the window server before it is routed:
/// `number` its message, `wparam` and `lparam` as the message carries them, at `point` and
/// `time`; whether a hook swallowed it.
pub fn input_swallowed(
    number: u32,
    wparam: usize,
    lparam: usize,
    point: (i32, i32),
    time: u32,
) -> bool {
    if INSTALLED.load(Ordering::Acquire) == 0 {
        return false;
    }
    let answer = match number {
        message::KEY_DOWN | message::KEY_UP | message::SYS_KEY_DOWN | message::SYS_KEY_UP => {
            let flag = |bit: u32, set: u32| if lparam >> bit & 1 != 0 { set } else { 0 };
            let key = KeyboardLowLevel {
                virtual_key: wparam as u32,
                scan_code: (lparam >> 16 & 0xff) as u32,
                flags: flag(24, LLKHF_EXTENDED) | flag(29, LLKHF_ALTDOWN) | flag(31, LLKHF_UP),
                time,
                extra: 0,
            };
            call(
                kind::KEYBOARD_LL,
                HC_ACTION,
                number as usize,
                &raw const key as usize,
            )
        }
        FIRST_MOUSE..=LAST_MOUSE => {
            let mouse = MouseLowLevel {
                x: point.0,
                y: point.1,
                data: (wparam >> 16) as u32 & 0xffff,
                flags: 0,
                time,
                extra: 0,
            };
            call(
                kind::MOUSE_LL,
                HC_ACTION,
                number as usize,
                &raw const mouse as usize,
            )
        }
        _ => None,
    };
    answer.is_some_and(|answer| answer != 0)
}

#[repr(C)]
struct CbtCreate {
    create: usize,
    insert_after: usize,
}

/// `WH_CBT`'s `HCBT_CREATEWND` for `window`, made with the `CREATESTRUCT` at `create`, before
/// its `WM_NCCREATE`; whether a hook refused it.
pub fn creation_refused(window: usize, create: usize) -> bool {
    let cbt = CbtCreate {
        create,
        insert_after: 0,
    };
    call(kind::CBT, HCBT_CREATEWND, window, &raw const cbt as usize)
        .is_some_and(|answer| answer != 0)
}

/// `WH_CBT`'s `HCBT_DESTROYWND` for `window`, before its `WM_DESTROY`.
pub fn destroying(window: usize) {
    call(kind::CBT, HCBT_DESTROYWND, window, 0);
}
