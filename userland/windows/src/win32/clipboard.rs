//! The clipboard of the program's own windows (D174): `OpenClipboard`, `EmptyClipboard`,
//! `SetClipboardData`, `GetClipboardData`, `IsClipboardFormatAvailable`,
//! `CountClipboardFormats` and `CloseClipboard`. The data are the `GlobalAlloc` blocks the
//! program hands over, which the clipboard then owns and frees when emptied or replaced. It is
//! the process's own: the system-wide clipboard every app shares is the plan's part 45, and
//! until it exists what one program copies is pasted only within that program. Its windows that
//! listen (`AddClipboardFormatListener`, D178) get `WM_CLIPBOARDUPDATE` when a change is closed,
//! `EnumClipboardFormats` lists what it holds and `GetClipboardFormatNameW` names a registered
//! format. Its `CF_UNICODETEXT` is the clipboard every app shares (D197): closing the clipboard
//! after a change gives that text to the window server, and opening it takes a text another app
//! copied since in its place.

use super::gui::{self, try_with, with};
use super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap, thread_environment};
use crate::gui::atoms::FIRST_ATOM;
use crate::gui::message;
use crate::library::Export;

const MOST_FORMATS: usize = 8;
const MOST_LISTENERS: usize = 8;

#[derive(Debug, Clone, Copy)]
struct Entry {
    format: u32,
    data: usize,
}

struct Clipboard {
    opened_by: Option<u32>,
    entries: [Option<Entry>; MOST_FORMATS],
    changed: bool,
    listeners: [usize; MOST_LISTENERS],
    shared_serial: Option<u32>,
}

static CLIPBOARD: SpinLock<Clipboard> = SpinLock::new(Clipboard {
    opened_by: None,
    entries: [None; MOST_FORMATS],
    changed: false,
    listeners: [0; MOST_LISTENERS],
    shared_serial: None,
});

const UNICODE_TEXT: u32 = 13;

fn shared_device() -> Option<super::window_device::Device> {
    gui::device().ok()
}

fn take_shared_text() {
    let Some(device) = shared_device() else {
        return;
    };
    let Ok((length, serial)) = device.clipboard(&mut []) else {
        return;
    };
    let unseen = CLIPBOARD.with(|clipboard| {
        let unseen = clipboard.shared_serial != Some(serial);
        clipboard.shared_serial = Some(serial);
        unseen
    });
    if !unseen || length == 0 {
        return;
    }
    let bytes = heap::allocate(length);
    if bytes.is_null() {
        return;
    }
    // SAFETY: `allocate` returned `length` writable bytes that only this function uses.
    let text = unsafe { core::slice::from_raw_parts_mut(bytes, length) };
    let read = device.clipboard(text);
    let units = crate::text::utf16_length(text);
    let block = match read {
        Ok((now, again)) if now == length && again == serial => heap::allocate((units + 1) * 2),
        _ => core::ptr::null_mut(),
    };
    if !block.is_null() {
        // SAFETY: `block` holds `units + 1` UTF-16 units, zeroed, the last one the NUL.
        let wide = unsafe { core::slice::from_raw_parts_mut(block.cast::<u16>(), units + 1) };
        crate::text::write_utf16(text, &mut wide[..units]);
    }
    free(bytes as usize);
    if block.is_null() {
        return;
    }
    let replaced = CLIPBOARD.with(|clipboard| {
        let mut freed = [0usize; MOST_FORMATS];
        for (slot, entry) in freed.iter_mut().zip(clipboard.entries.iter_mut()) {
            *slot = entry.take().map_or(0, |entry| entry.data);
        }
        clipboard.entries[0] = Some(Entry {
            format: UNICODE_TEXT,
            data: block as usize,
        });
        freed
    });
    replaced.into_iter().for_each(free);
}

fn give_shared_text() {
    let data = CLIPBOARD.with(|clipboard| {
        clipboard
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.format == UNICODE_TEXT)
            .map(|entry| entry.data)
    });
    let Some(device) = shared_device() else {
        return;
    };
    let Some(data) = data else {
        if let Ok((_, serial)) = device.clipboard(&mut []) {
            CLIPBOARD.with(|clipboard| clipboard.shared_serial = Some(serial));
        }
        return;
    };
    // SAFETY: clipboard data are `GlobalAlloc` blocks; `global_units` reads only within the
    // block's size, which the heap's header gives.
    let units = unsafe { heap::global_units(data as *mut u8) };
    let length = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    let units = &units[..length];
    let size = crate::text::utf8_length(units);
    let bytes = heap::allocate(size.max(1));
    if bytes.is_null() {
        return;
    }
    // SAFETY: `allocate` returned `size.max(1)` writable bytes that only this function uses.
    let text = unsafe { core::slice::from_raw_parts_mut(bytes, size) };
    let (written, _) = crate::text::write_utf8(units, text);
    if device.set_clipboard(&text[..written]).is_ok()
        && let Ok((_, serial)) = device.clipboard(&mut [])
    {
        CLIPBOARD.with(|clipboard| clipboard.shared_serial = Some(serial));
    }
    free(bytes as usize);
}

fn free(data: usize) {
    if data != 0 {
        // SAFETY: clipboard data are `GlobalAlloc` blocks whose ownership the program handed to
        // the clipboard, which nothing else frees.
        unsafe { heap::free(data as *mut u8) };
    }
}

extern "efiapi" fn open_clipboard(_owner: usize) -> Bool {
    take_shared_text();
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
        if !opened_here(clipboard) {
            return None;
        }
        clipboard.opened_by = None;
        Some(core::mem::take(&mut clipboard.changed).then_some(clipboard.listeners))
    });
    let Some(changed) = closed else {
        return fail(error::ACCESS_DENIED, FALSE);
    };
    let Some(listeners) = changed else {
        return TRUE;
    };
    give_shared_text();
    for listener in listeners.into_iter().filter(|&listener| listener != 0) {
        let _ = gui::post(listener, message::CLIPBOARD_UPDATE, 0, 0);
    }
    TRUE
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
        clipboard.changed = true;
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
        clipboard.changed = true;
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

extern "efiapi" fn add_clipboard_format_listener(window: usize) -> Bool {
    if let Err(code) = try_with(|gui| gui.window(window).map(|_| ())) {
        return fail(code, FALSE);
    }
    let added = CLIPBOARD.with(|clipboard| {
        if clipboard.listeners.contains(&window) {
            return true;
        }
        match clipboard.listeners.iter_mut().find(|slot| **slot == 0) {
            Some(slot) => {
                *slot = window;
                true
            }
            None => false,
        }
    });
    match added {
        true => TRUE,
        false => fail(error::NOT_ENOUGH_MEMORY, FALSE),
    }
}

extern "efiapi" fn remove_clipboard_format_listener(window: usize) -> Bool {
    let removed = CLIPBOARD.with(|clipboard| {
        let slot = clipboard
            .listeners
            .iter_mut()
            .find(|slot| **slot == window && window != 0)?;
        *slot = 0;
        Some(())
    });
    match removed {
        Some(()) => TRUE,
        None => fail(error::INVALID_PARAMETER, FALSE),
    }
}

extern "efiapi" fn enum_clipboard_formats(format: u32) -> u32 {
    let next = CLIPBOARD.with(|clipboard| {
        if !opened_here(clipboard) {
            return Err(error::CLIPBOARD_NOT_OPEN);
        }
        let mut formats = clipboard.entries.iter().flatten().map(|entry| entry.format);
        Ok(match format {
            0 => formats.next(),
            format => formats
                .by_ref()
                .find(|&held| held == format)
                .and_then(|_| formats.next()),
        })
    });
    match next {
        Ok(Some(format)) => format,
        Ok(None) => {
            thread_environment::set_last_error(error::SUCCESS);
            0
        }
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn get_clipboard_format_name_w(
    format: u32,
    buffer: *mut u16,
    capacity: i32,
) -> i32 {
    let Ok(atom) = u16::try_from(format) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if atom < FIRST_ATOM || buffer.is_null() || capacity <= 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let copied = with(|gui| {
        let name = gui.made_extras()?.atoms.name(atom)?;
        // SAFETY: `GetClipboardFormatNameW`'s contract: `buffer` holds `capacity` writable
        // units.
        Some(unsafe { super::window_text::copy_out(name, buffer as usize, capacity as usize) })
    })
    .flatten();
    match copied {
        Some(copied) => copied as i32,
        None => fail(error::INVALID_PARAMETER, 0),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"AddClipboardFormatListener" => add_clipboard_format_listener,
    b"RemoveClipboardFormatListener" => remove_clipboard_format_listener,
    b"EnumClipboardFormats" => enum_clipboard_formats,
    b"GetClipboardFormatNameW" => get_clipboard_format_name_w,
    b"OpenClipboard" => open_clipboard,
    b"CloseClipboard" => close_clipboard,
    b"EmptyClipboard" => empty_clipboard,
    b"SetClipboardData" => set_clipboard_data,
    b"GetClipboardData" => get_clipboard_data,
    b"IsClipboardFormatAvailable" => is_clipboard_format_available,
    b"CountClipboardFormats" => count_clipboard_formats,
];
