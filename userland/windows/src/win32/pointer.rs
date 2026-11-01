//! The pointer's shape set by the program (D227, `docs/ux/windows-controls-v0.3.md` section
//! 16): `SetCursor`, `GetCursor` and `ShowCursor`, applied to the program's top-level windows;
//! a null cursor, or a show count below 0, shows the server's `Hidden` shape.

use super::Bool;
use super::gui::{device, with};
use crate::gui::objects::{self, cursor};
use crate::gui::windows::MAX_WINDOWS;
use crate::library::Export;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

const HIDDEN: u32 = 6;
const NEVER_SET: usize = usize::MAX;

static CURRENT: AtomicUsize = AtomicUsize::new(NEVER_SET);
static SHOW_COUNT: AtomicI32 = AtomicI32::new(0);

fn current() -> usize {
    match CURRENT.load(Ordering::Relaxed) {
        NEVER_SET => objects::cursor_handle(true, cursor::ARROW),
        handle => handle,
    }
}

fn shape_for(handle: usize, count: i32) -> u32 {
    match (count < 0, handle) {
        (true, _) | (false, 0) => HIDDEN,
        (false, handle) => objects::pointer_shape(handle),
    }
}

fn apply() {
    let shape = shape_for(current(), SHOW_COUNT.load(Ordering::Relaxed));
    let mut servers = [0u32; MAX_WINDOWS];
    let count = with(|gui| {
        let mut count = 0;
        for (_, window) in gui.windows.iter() {
            if !window.is_child() && window.server != 0 && count < servers.len() {
                servers[count] = window.server;
                count += 1;
            }
        }
        count
    })
    .unwrap_or(0);
    let Ok(device) = device() else {
        return;
    };
    for &server in &servers[..count] {
        let _ = device.set_pointer(server, shape);
    }
}

/// `SetCursor`: makes `handle` the program's cursor, applied at once when it changes; the
/// previous cursor.
pub(super) fn replace(handle: usize) -> usize {
    let previous = current();
    CURRENT.store(handle, Ordering::Relaxed);
    if previous != handle {
        apply();
    }
    previous
}

extern "efiapi" fn set_cursor(handle: usize) -> usize {
    replace(handle)
}

extern "efiapi" fn get_cursor() -> usize {
    current()
}

extern "efiapi" fn show_cursor(show: Bool) -> i32 {
    let step = match show {
        0 => -1,
        _ => 1,
    };
    let before = SHOW_COUNT.fetch_add(step, Ordering::Relaxed);
    let after = before + step;
    if (before < 0) != (after < 0) {
        apply();
    }
    after
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetCursor" => set_cursor,
    b"GetCursor" => get_cursor,
    b"ShowCursor" => show_cursor,
];

