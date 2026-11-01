//! More of the message functions (D178): `SendMessageTimeoutW`, `PostThreadMessageW` and
//! `GetQueueStatus`; the hooks, which EggOS does not offer (`SetWindowsHookExW` fails, so no
//! hook ever exists to unhook or chain to); and `ExitWindowsEx`, which an app may not use to
//! shut EggOS down.

use super::gui::{self, try_with, with};
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::gui::queue::{Filter, status};
use crate::gui::windows;
use crate::library::Export;

const HWND_BROADCAST: usize = 0xffff;
const FIRST_HOOK: i32 = -1;
const LAST_HOOK: i32 = 14;

unsafe extern "efiapi" fn send_message_timeout_w(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
    _flags: u32,
    _timeout: u32,
    result: *mut usize,
) -> usize {
    let targets = try_with(|gui| match window {
        HWND_BROADCAST => Ok(windows::top_levels(&gui.windows)),
        window => {
            gui.window(window)?;
            let mut one = [0; windows::MAX_WINDOWS];
            one[0] = window;
            Ok((one, 1))
        }
    });
    let (targets, count) = match targets {
        Ok(targets) => targets,
        Err(code) => return fail(code, 0),
    };
    let mut answer = 0;
    for &target in &targets[..count] {
        answer = gui::send(target, number, wparam, lparam);
    }
    if !result.is_null() {
        // SAFETY: the caller passes a writable `DWORD_PTR` or null.
        unsafe { result.write(answer) };
    }
    1
}

extern "efiapi" fn post_thread_message_w(
    thread: u32,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> Bool {
    let current = thread_environment::thread_id();
    let posted = try_with(|gui| {
        let known = thread == current
            || gui
                .windows
                .iter()
                .any(|(_, window)| window.thread == thread);
        if !known {
            return Err(error::INVALID_THREAD_ID);
        }
        gui.post(0, thread, number, wparam, lparam)
    });
    match posted {
        Ok(()) => {
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn get_queue_status(flags: u32) -> u32 {
    let thread = thread_environment::thread_id();
    let now = super::user32::now_ms();
    with(|gui| {
        let mut waiting = gui.queue.status(thread);
        if windows::needing_paint(&gui.windows, 0, thread).is_some() {
            waiting |= status::PAINT;
        }
        let mine = Filter {
            thread,
            ..Filter::default()
        };
        if gui.timers.next_due(&mine, now) == Some(0) {
            waiting |= status::TIMER;
        }
        let new = waiting & !gui.queue_reported;
        gui.queue_reported = waiting;
        ((waiting & flags) << 16) | (new & flags)
    })
    .unwrap_or(0)
}

extern "efiapi" fn set_windows_hook_ex_w(
    kind: i32,
    _procedure: usize,
    _module: usize,
    _thread: u32,
) -> usize {
    match (FIRST_HOOK..=LAST_HOOK).contains(&kind) {
        true => fail(error::ACCESS_DENIED, 0),
        false => fail(error::INVALID_HOOK_FILTER, 0),
    }
}

extern "efiapi" fn unhook_windows_hook_ex(_hook: usize) -> Bool {
    fail(error::INVALID_HOOK_HANDLE, FALSE)
}

extern "efiapi" fn call_next_hook_ex(
    _hook: usize,
    _code: i32,
    _wparam: usize,
    _lparam: usize,
) -> usize {
    0
}

extern "efiapi" fn exit_windows_ex(_flags: u32, _reason: u32) -> Bool {
    fail(error::PRIVILEGE_NOT_HELD, FALSE)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SendMessageTimeoutW" => send_message_timeout_w,
    b"PostThreadMessageW" => post_thread_message_w,
    b"GetQueueStatus" => get_queue_status,
    b"SetWindowsHookExW" => set_windows_hook_ex_w,
    b"UnhookWindowsHookEx" => unhook_windows_hook_ex,
    b"CallNextHookEx" => call_next_hook_ex,
    b"ExitWindowsEx" => exit_windows_ex,
];
