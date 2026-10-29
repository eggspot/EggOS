//! `SetTimer` and `KillTimer` (D145) over the layer's timer table: a timer's `WM_TIMER` comes
//! after the paints, and a thread with a timer sleeps only until it is due; `DispatchMessageW`
//! calls a timer procedure only when it is the one registered for that timer.

use super::gui::{self, with};
use super::user32::now_ms;
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::library::Export;

extern "efiapi" fn set_timer(window: usize, id: usize, interval: u32, procedure: usize) -> usize {
    let thread = thread_environment::thread_id();
    let set = with(|gui| {
        let owner = match window {
            0 => thread,
            window => gui.windows.get(window)?.thread,
        };
        gui.timers
            .set(window, id, interval, procedure, owner, now_ms())
    })
    .flatten();
    match set {
        Some(id) => {
            gui::wake();
            match (window, id) {
                (0, id) => id,
                (_, 0) => 1,
                (_, id) => id,
            }
        }
        None => fail(error::INVALID_WINDOW_HANDLE, 0),
    }
}

extern "efiapi" fn kill_timer(window: usize, id: usize) -> Bool {
    let thread = thread_environment::thread_id();
    let killed = with(|gui| gui.timers.kill(window, id, thread)).unwrap_or(false);
    gui::wake();
    match killed {
        true => TRUE,
        false => fail(error::INVALID_PARAMETER, FALSE),
    }
}

/// Whether `procedure` is the timer procedure of the calling thread's timer `id` of `window`.
pub fn is_registered(window: usize, id: usize, procedure: usize) -> bool {
    let thread = thread_environment::thread_id();
    with(|gui| {
        let owner = match window {
            0 => thread,
            window => gui.windows.get(window).map_or(0, |window| window.thread),
        };
        owner == thread && gui.timers.has_procedure(window, id, procedure, owner)
    })
    .unwrap_or(false)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetTimer" => set_timer,
    b"KillTimer" => kill_timer,
];
