//! The clipboard of the program's own windows (D174): `OpenClipboard`, `EmptyClipboard`,
//! `SetClipboardData`, `GetClipboardData`, `IsClipboardFormatAvailable`,
//! `CountClipboardFormats` and `CloseClipboard`. The data are the `GlobalAlloc` blocks the
//! program hands over, which the clipboard then owns and frees when emptied or replaced. It is
//! the process's own: the system-wide clipboard every app shares is the plan's part 45, and
//! until it exists what one program copies is pasted only within that program.

use super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap, thread_environment};
use crate::library::Export;

const MOST_FORMATS: usize = 8;

#[derive(Debug, Clone, Copy)]
struct Entry {
    format: u32,
    data: usize,
}

struct Clipboard {
    opened_by: Option<u32>,
    entries: [Option<Entry>; MOST_FORMATS],
}

static CLIPBOARD: SpinLock<Clipboard> = SpinLock::new(Clipboard {
    opened_by: None,
    entries: [None; MOST_FORMATS],
});

fn free(data: usize) {
    if data != 0 {
        // SAFETY: clipboard data are `GlobalAlloc` blocks whose ownership the program handed to
        // the clipboard, which nothing else frees.
        unsafe { heap::free(data as *mut u8) };
    }
}

extern "efiapi" fn open_clipboard(_owner: usize) -> Bool {
    let thread = thread_environment::thread_id();
    let opened = CLIPBOARD.with(|clipboard| match clipboard.opened_by {
        Some(holder) if holder != thread => false,
        _ => {
            clipboard.opened_by = Some(thread);
            true
        }
    });
    match opened {
        true => TRUE,
        false => fail(error::ACCESS_DENIED, FALSE),
    }
}

fn opened_here(clipboard: &Clipboard) -> bool {
    clipboard.opened_by == Some(thread_environment::thread_id())
}

extern "efiapi" fn close_clipboard() -> Bool {
    let closed = CLIPBOARD.with(|clipboard| {
        let mine = opened_here(clipboard);
        if mine {
            clipboard.opened_by = None;
        }
        mine
    });
    match closed {
        true => TRUE,
        false => fail(error::ACCESS_DENIED, FALSE),
    }
}

extern "efiapi" fn empty_clipboard() -> Bool {
    let emptied = CLIPBOARD.with(|clipboard| {
        if !opened_here(clipboard) {
            return None;
        }
        let mut freed = [0usize; MOST_FORMATS];
        for (slot, entry) in freed.iter_mut().zip(clipboard.entries.iter_mut()) {
            *slot = entry.take().map_or(0, |entry| entry.data);
        }
        Some(freed)
    });
    match emptied {
        Some(freed) => {
            freed.into_iter().for_each(free);
            TRUE
        }
        None => fail(error::ACCESS_DENIED, FALSE),
    }
}

extern "efiapi" fn set_clipboard_data(format: u32, data: usize) -> usize {
    let stored = CLIPBOARD.with(|clipboard| {
        if !opened_here(clipboard) {
            return Err(error::ACCESS_DENIED);
        }
        if let Some(entry) = clipboard
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.format == format)
        {
            return Ok(core::mem::replace(&mut entry.data, data));
        }
        let free_slot = clipboard
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(error::NOT_ENOUGH_MEMORY)?;
        *free_slot = Some(Entry { format, data });
        Ok(0)
    });
    match stored {
        Ok(replaced) => {
            if replaced != data {
                free(replaced);
            }
            data
        }
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_clipboard_data(format: u32) -> usize {
    let found = CLIPBOARD.with(|clipboard| {
        if !opened_here(clipboard) {
            return Err(error::ACCESS_DENIED);
        }
        Ok(clipboard
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.format == format)
            .map_or(0, |entry| entry.data))
    });
    match found {
        Ok(data) => data,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn is_clipboard_format_available(format: u32) -> Bool {
    let available = CLIPBOARD.with(|clipboard| {
        clipboard
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.format == format)
    });
    Bool::from(available)
}

extern "efiapi" fn count_clipboard_formats() -> i32 {
    CLIPBOARD.with(|clipboard| clipboard.entries.iter().flatten().count() as i32)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"OpenClipboard" => open_clipboard,
    b"CloseClipboard" => close_clipboard,
    b"EmptyClipboard" => empty_clipboard,
    b"SetClipboardData" => set_clipboard_data,
    b"GetClipboardData" => get_clipboard_data,
    b"IsClipboardFormatAvailable" => is_clipboard_format_available,
    b"CountClipboardFormats" => count_clipboard_formats,
];
