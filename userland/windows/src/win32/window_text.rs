//! Window text (D145): what `SetWindowTextW` gives a window and `GetWindowTextW` reads back,
//! through `WM_SETTEXT` and `WM_GETTEXT`, kept in a heap block made when a window first has
//! text; a top-level window's text is also its title on the desktop.

use super::gui::{self, Gui, try_with, with};
use super::{Bool, FALSE, TRUE, fail, heap};
use crate::gui::message;
use crate::library::Export;
use crate::text;

/// The longest text a window keeps, in UTF-16 units.
pub const WINDOW_TEXT_CAPACITY: usize = 256;
const TITLE_BYTES: usize = 64 * 4;

struct WindowText {
    length: usize,
    units: [u16; WINDOW_TEXT_CAPACITY],
}

/// `window`'s text.
pub fn of(gui: &Gui, window: usize) -> &[u16] {
    let Some(address) = gui.windows.get(window).map(|window| window.text) else {
        return &[];
    };
    if address == 0 {
        return &[];
    }
    // SAFETY: a window's text block is the heap block `store` made for it, freed only after
    // the window left the table; the caller holds the lock through `gui`.
    let block = unsafe { &*(address as *const WindowText) };
    &block.units[..block.length]
}

/// Gives `window` the text `units`, cut at the capacity; whether there was memory for it.
pub fn store(gui: &mut Gui, window: usize, units: &[u16]) -> bool {
    let Some(entry) = gui.windows.get_mut(window) else {
        return false;
    };
    if entry.text == 0 {
        if units.is_empty() {
            return true;
        }
        let block = heap::allocate(core::mem::size_of::<WindowText>());
        if block.is_null() {
            return false;
        }
        entry.text = block as usize;
    }
    // SAFETY: as `of`, and the caller's `&mut Gui` makes this the only reference.
    let block = unsafe { &mut *(entry.text as *mut WindowText) };
    let length = units.len().min(WINDOW_TEXT_CAPACITY);
    block.units[..length].copy_from_slice(&units[..length]);
    block.length = length;
    true
}

/// Frees a text block a window that left the table had.
pub fn free(address: usize) {
    if address != 0 {
        // SAFETY: the block is the one `store` made; its window is gone, so nothing reaches it.
        unsafe { heap::free(address as *mut u8) };
    }
}

unsafe fn units_at<'a>(text: usize) -> &'a [u16] {
    match text {
        0 => &[],
        // SAFETY: the caller's duty: `WM_SETTEXT`'s `lParam` is a NUL-terminated string.
        text => unsafe { super::file::wide_string(text as *const u16) },
    }
}

/// `DefWindowProcW`'s `WM_SETTEXT`: keeps the text and, for a top-level window, shows it as
/// the title.
///
/// # Safety
/// `text` is 0 or a NUL-terminated UTF-16 string.
pub unsafe fn set_default(window: usize, text: usize) -> usize {
    // SAFETY: the caller's duty.
    let units = unsafe { units_at(text) };
    let title = with(|gui| {
        if !store(gui, window, units) {
            return None;
        }
        let entry = gui.windows.get(window)?;
        (!entry.is_child()).then_some((gui.device?, entry.server))
    })
    .flatten();
    if let Some((device, server)) = title {
        let mut bytes = [0u8; TITLE_BYTES];
        let (length, _) = text::write_utf8(units, &mut bytes);
        let _ = device.set_title(server, core::str::from_utf8(&bytes[..length]).unwrap_or(""));
    }
    1
}

/// Copies `units` into the program's buffer of `capacity` units, NUL-terminated and cut to
/// fit; the units copied.
///
/// # Safety
/// `buffer` holds `capacity` writable units, or `capacity` is 0.
pub unsafe fn copy_out(units: &[u16], buffer: usize, capacity: usize) -> usize {
    if buffer == 0 || capacity == 0 {
        return 0;
    }
    let count = units.len().min(capacity - 1);
    // SAFETY: the caller's duty: `capacity` writable units, of which `count + 1` are written.
    unsafe {
        let out = core::slice::from_raw_parts_mut(buffer as *mut u16, count + 1);
        out[..count].copy_from_slice(&units[..count]);
        out[count] = 0;
    }
    count
}

/// `DefWindowProcW`'s `WM_GETTEXT`.
///
/// # Safety
/// `buffer` holds `capacity` writable units.
pub unsafe fn get_default(window: usize, capacity: usize, buffer: usize) -> usize {
    let mut copy = [0u16; WINDOW_TEXT_CAPACITY];
    let length = with(|gui| {
        let text = of(gui, window);
        copy[..text.len()].copy_from_slice(text);
        text.len()
    })
    .unwrap_or(0);
    // SAFETY: the caller's duty.
    unsafe { copy_out(&copy[..length], buffer, capacity) }
}

/// `DefWindowProcW`'s `WM_GETTEXTLENGTH`.
pub fn length_default(window: usize) -> usize {
    with(|gui| of(gui, window).len()).unwrap_or(0)
}

fn known(window: usize) -> Result<(), u32> {
    try_with(|gui| gui.window(window).map(|_| ()))
}

unsafe extern "efiapi" fn set_window_text_w(window: usize, text: *const u16) -> Bool {
    if let Err(code) = known(window) {
        return fail(code, FALSE);
    }
    match gui::send(window, message::SET_TEXT, 0, text as usize) {
        0 => FALSE,
        _ => TRUE,
    }
}

unsafe extern "efiapi" fn get_window_text_w(window: usize, buffer: *mut u16, capacity: i32) -> i32 {
    if let Err(code) = known(window) {
        return fail(code, 0);
    }
    gui::send(
        window,
        message::GET_TEXT,
        capacity.max(0) as usize,
        buffer as usize,
    ) as i32
}

extern "efiapi" fn get_window_text_length_w(window: usize) -> i32 {
    if let Err(code) = known(window) {
        return fail(code, 0);
    }
    gui::send(window, message::GET_TEXT_LENGTH, 0, 0) as i32
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetWindowTextW" => set_window_text_w,
    b"GetWindowTextW" => get_window_text_w,
    b"GetWindowTextLengthW" => get_window_text_length_w,
];
