//! shell32's desktop calls (D178): files dropped on a window come as a `DROPFILES` block (the
//! list's offset, the drop point, whether it fell outside the client area, whether the names
//! are UTF-16, then the names each ended by a NUL and the list by another), which
//! `DragQueryFileW` and `DragQueryPoint` read and `DragFinish` frees; EggOS does not drop files
//! on Windows programs' windows yet, so no such block comes. Icons are not read from files yet
//! (`ExtractIconW` finds none). `Shell_NotifyIconW` puts a program's notification-area icons in
//! the bar (D357, `docs/ux/windows-tooltips-tray-v0.5.md` sections 11 to 13) and posts the
//! pointer's presses, double clicks and moves on them, the keyboard's choices and the balloon
//! and popup messages to the icon's window as its callback message, packed by the icon's
//! version (D719); `Shell_NotifyIconGetRect` answers the place the bar reported.

use super::gui::with;
use super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap};
use crate::library::Export;
use crate::text;
use core::sync::atomic::{AtomicU32, Ordering};

const FILES_OFFSET: usize = 0;
const POINT: usize = 4;
const OUTSIDE_CLIENT: usize = 12;
const WIDE: usize = 16;
const COUNT_FILES: u32 = u32::MAX;

unsafe fn read<T>(drop: *const u8, offset: usize) -> T {
    // SAFETY: the callers' contract: a `DROPFILES` block holding a `T` at `offset`.
    unsafe { drop.wrapping_add(offset).cast::<T>().read_unaligned() }
}

unsafe fn for_each_name(drop: *const u8, visit: &mut dyn FnMut(&[u16]) -> bool) {
    // SAFETY: the callers' contract: a `DROPFILES` block.
    let offset: u32 = unsafe { read(drop, FILES_OFFSET) };
    // SAFETY: as above.
    let wide: Bool = unsafe { read(drop, WIDE) };
    let names = drop.wrapping_add(offset as usize);
    if wide != FALSE {
        // SAFETY: as above: UTF-16 names each ended by a NUL, the list by another.
        let units = unsafe { super::processes::listed(names.cast::<u16>()) };
        for name in units
            .split(|unit| *unit == 0)
            .filter(|name| !name.is_empty())
        {
            if !visit(name) {
                return;
            }
        }
        return;
    }
    // SAFETY: as above, in bytes (the layer's ANSI code page is UTF-8).
    let bytes = unsafe { super::processes::listed(names) };
    for name in bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let mut units = [0u16; text::PATH_CAPACITY];
        let length = text::write_utf16(name, &mut units);
        if !visit(&units[..length]) {
            return;
        }
    }
}

unsafe extern "efiapi" fn drag_query_file_w(
    drop: *const u8,
    index: u32,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    if drop.is_null() {
        return 0;
    }
    let measure = buffer.is_null();
    // SAFETY: the caller passes `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    let mut seen = 0u32;
    let mut answer = 0u32;
    let mut visit = |name: &[u16]| {
        if index == COUNT_FILES || seen < index {
            seen += 1;
            return true;
        }
        let copied = name.len().min(out.len().saturating_sub(1));
        answer = match measure {
            true => name.len() as u32,
            false => copied as u32,
        };
        if !measure && !out.is_empty() {
            out[..copied].copy_from_slice(&name[..copied]);
            out[copied] = 0;
        }
        false
    };
    // SAFETY: the caller passes an `HDROP`, its `DROPFILES` block.
    unsafe { for_each_name(drop, &mut visit) };
    match index {
        COUNT_FILES => seen,
        _ => answer,
    }
}

unsafe extern "efiapi" fn drag_query_point(drop: *const u8, point: *mut [i32; 2]) -> Bool {
    if drop.is_null() || point.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes an `HDROP`, its `DROPFILES` block.
    let at: [i32; 2] = unsafe { read(drop, POINT) };
    // SAFETY: as above.
    let outside: Bool = unsafe { read(drop, OUTSIDE_CLIENT) };
    // SAFETY: the caller passes a writable `POINT`.
    unsafe { point.write_unaligned(at) };
    Bool::from(outside == FALSE)
}

unsafe extern "efiapi" fn drag_finish(drop: *mut u8) {
    // SAFETY: an `HDROP` is a global memory block of the layer's heap, which the heap checks.
    unsafe { heap::free(drop) };
}

extern "efiapi" fn extract_icon_w(_instance: usize, _file: *const u16, _index: u32) -> usize {
    0
}

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_INFO: u32 = 0x10;
const NIF_SHOWTIP: u32 = 0x80;
const DATA_INFO: usize = 304;
const DATA_INFO_TITLE: usize = 820;
const INFO_UNITS: usize = 256;
const INFO_TITLE_UNITS: usize = 64;
const TRAY_BALLOON: u32 = 4;
const NIS_HIDDEN: u32 = 0x01;
const NIN_SELECT: u32 = 0x0400;
const NIN_KEYSELECT: u32 = 0x0401;
const NIN_POPUPOPEN: u32 = 0x0406;
const NIN_POPUPCLOSE: u32 = 0x0407;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const DOUBLE_CLICK_AFTER_DOWN: u32 = 2;
const WM_CONTEXTMENU: u32 = 0x007b;
const TRAY_PLACE: u32 = 0x0001_0000;
const PLACE_HALF_BITS: u32 = 16;
const PLACE_HALF_MASK: u32 = 0xffff;
const S_OK: i32 = 0;
const E_FAIL: i32 = 0x8000_4005_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const IDENTIFIER_WINDOW: usize = 8;
const IDENTIFIER_ID: usize = 16;
const IDENTIFIER_MINIMUM_SIZE: usize = 20;
const DATA_WINDOW: usize = 8;
const DATA_ID: usize = 16;
const DATA_FLAGS: usize = 20;
const DATA_CALLBACK: usize = 24;
const DATA_ICON: usize = 32;
const DATA_TIP: usize = 40;
const DATA_STATE: usize = 296;
const DATA_STATE_MASK: usize = 300;
const DATA_VERSION: usize = 816;
const SHORT_TIP_UNITS: usize = 64;
const TIP_UNITS: usize = 128;
const MOST_ICONS: usize = 16;
const TIP_BYTES: usize = TIP_UNITS * 3;
const TRAY_HIDDEN: u32 = 1;
const TRAY_REMOVE: u32 = 2;
const TRAY_RICH_POPUP: u32 = 8;
const TRAY_FOCUS: u32 = 16;
const OPAQUE: u32 = 0xff00_0000;

#[derive(Clone, Copy)]
struct TrayEntry {
    window: usize,
    id: u32,
    key: u32,
    callback: u32,
    version: u32,
    icon: usize,
    hidden: bool,
    show_tip: bool,
    place: [i32; 4],
    tip: [u16; TIP_UNITS],
    tip_length: usize,
}

#[derive(Clone, Copy)]
struct TrayClick {
    key: u32,
    down: u32,
    time: u32,
    point: (i32, i32),
    doubled: bool,
}

static TRAY: SpinLock<[Option<TrayEntry>; MOST_ICONS]> = SpinLock::new([None; MOST_ICONS]);
static NEXT_KEY: AtomicU32 = AtomicU32::new(1);
static LAST_TRAY_CLICK: SpinLock<Option<TrayClick>> = SpinLock::new(None);

unsafe fn field<T: Copy>(data: *const u8, offset: usize) -> T {
    // SAFETY: the callers' contract: a `NOTIFYICONDATAW` at least `offset + size_of::<T>()`
    // bytes long.
    unsafe { data.wrapping_add(offset).cast::<T>().read_unaligned() }
}

fn tray_pixels(icon: usize) -> Option<[u32; 256]> {
    let (colour, mask) = super::icon::icon_bitmaps(icon)?;
    let mut out = [0u32; 256];
    let sample = |width: usize, height: usize, x: usize, y: usize| -> Option<usize> {
        let source = |at: usize, length: usize| match length >= 16 {
            true => Some(at * length / 16),
            false => at.checked_sub((16 - length) / 2).filter(|&at| at < length),
        };
        Some(source(y, height)? * width + source(x, width)?)
    };
    let alpha = super::gdi::read_bitmap(colour, |bitmap, pixels| {
        let (width, height) = (bitmap.width as usize, bitmap.height as usize);
        let has_alpha = pixels.iter().any(|pixel| pixel >> 24 != 0);
        for (at, slot) in out.iter_mut().enumerate() {
            let Some(index) = sample(width, height, at % 16, at / 16) else {
                continue;
            };
            let pixel = pixels.get(index).copied().unwrap_or(0);
            let opaque = !has_alpha || pixel >> 24 >= 0x80;
            *slot = if opaque {
                OPAQUE | (pixel & 0x00ff_ffff)
            } else {
                0
            };
        }
        has_alpha
    })
    .ok()?;
    if !alpha && mask != 0 {
        let _ = super::gdi::read_bitmap(mask, |bitmap, pixels| {
            let (width, height) = (bitmap.width as usize, bitmap.height as usize);
            for (at, slot) in out.iter_mut().enumerate() {
                let index = sample(width, height, at % 16, at / 16);
                let masked = index.and_then(|index| pixels.get(index));
                if masked.is_some_and(|&pixel| pixel & 0x00ff_ffff != 0) {
                    *slot = 0;
                }
            }
        });
    }
    Some(out)
}

fn server_window(window: usize) -> Option<u32> {
    with(|gui| {
        let root = crate::gui::windows::root_of(&gui.windows, window)?;
        let server = gui.windows.get(root)?.server;
        (server != 0).then_some(server)
    })
    .flatten()
}

fn show(entry: &TrayEntry, remove: bool) -> Result<(), u32> {
    let server = server_window(entry.window).ok_or(error::INVALID_WINDOW_HANDLE)?;
    let device = super::gui::device()?;
    let mut tip = [0u8; TIP_BYTES];
    let (length, _) = text::write_utf8(&entry.tip[..entry.tip_length], &mut tip);
    let tip = core::str::from_utf8(&tip[..length]).unwrap_or("");
    let pixels = match entry.icon {
        0 => None,
        icon => tray_pixels(icon),
    };
    let mut flags = 0;
    if entry.hidden {
        flags |= TRAY_HIDDEN;
    }
    if entry.version == 4 && !entry.show_tip {
        flags |= TRAY_RICH_POPUP;
    }
    if remove {
        flags |= TRAY_REMOVE;
    }
    device
        .set_tray_icon((server, entry.key, flags), tip, pixels.as_ref())
        .map_err(|_| error::NOT_ENOUGH_MEMORY)
}

unsafe extern "efiapi" fn shell_notify_icon_w(message: u32, data: *const u8) -> Bool {
    if data.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `Shell_NotifyIconW`'s contract: a `NOTIFYICONDATAW` whose first word is its size.
    let size = unsafe { field::<u32>(data, 0) } as usize;
    if size < DATA_TIP + SHORT_TIP_UNITS * 2 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: as above, the fields of the oldest structure.
    let (window, id, flags) = unsafe {
        (
            field::<usize>(data, DATA_WINDOW),
            field::<u32>(data, DATA_ID),
            field::<u32>(data, DATA_FLAGS),
        )
    };
    let found = TRAY.with(|icons| {
        icons
            .iter()
            .position(|icon| icon.is_some_and(|icon| icon.window == window && icon.id == id))
    });
    let mut entry = match (message, found) {
        (NIM_ADD, Some(_)) => return fail(error::INVALID_PARAMETER, FALSE),
        (NIM_ADD, None) => TrayEntry {
            window,
            id,
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            callback: 0,
            version: 0,
            icon: 0,
            hidden: false,
            show_tip: false,
            place: [0; 4],
            tip: [0; TIP_UNITS],
            tip_length: 0,
        },
        (_, Some(at)) => match TRAY.with(|icons| icons[at]) {
            Some(entry) => entry,
            None => return fail(error::INVALID_PARAMETER, FALSE),
        },
        (_, None) => return fail(error::INVALID_PARAMETER, FALSE),
    };
    match message {
        NIM_DELETE => {
            if let Some(at) = found {
                TRAY.with(|icons| icons[at] = None);
            }
            let _ = show(&entry, true);
            return TRUE;
        }
        NIM_SETFOCUS => {
            let focused = server_window(entry.window).zip(super::gui::device().ok());
            if let Some((server, device)) = focused {
                let _ = device.set_tray_icon((server, entry.key, TRAY_FOCUS), "", None);
            }
            return TRUE;
        }
        NIM_SETVERSION => {
            if size <= DATA_VERSION {
                return fail(error::INVALID_PARAMETER, FALSE);
            }
            // SAFETY: as above; the structure is long enough for the version.
            let version = unsafe { field::<u32>(data, DATA_VERSION) };
            if !matches!(version, 0 | 3 | 4) {
                return FALSE;
            }
            entry.version = version;
            if let Some(at) = found {
                TRAY.with(|icons| icons[at] = Some(entry));
                let _ = show(&entry, false);
            }
            return TRUE;
        }
        NIM_ADD | NIM_MODIFY => {}
        _ => return fail(error::INVALID_PARAMETER, FALSE),
    }
    entry.show_tip = flags & NIF_SHOWTIP != 0;
    if flags & NIF_MESSAGE != 0 {
        // SAFETY: as above.
        entry.callback = unsafe { field::<u32>(data, DATA_CALLBACK) };
    }
    if flags & NIF_ICON != 0 {
        // SAFETY: as above.
        entry.icon = unsafe { field::<usize>(data, DATA_ICON) };
    }
    if flags & NIF_TIP != 0 {
        let room = match size >= DATA_TIP + TIP_UNITS * 2 {
            true => TIP_UNITS,
            false => SHORT_TIP_UNITS,
        };
        let mut length = 0;
        while length < room {
            // SAFETY: as above, within the structure's tip array.
            let unit = unsafe { field::<u16>(data, DATA_TIP + length * 2) };
            if unit == 0 {
                break;
            }
            entry.tip[length] = unit;
            length += 1;
        }
        entry.tip_length = length;
    }
    if flags & NIF_STATE != 0 && size > DATA_STATE_MASK {
        // SAFETY: as above; the structure is long enough for the state.
        let (state, mask) = unsafe {
            (
                field::<u32>(data, DATA_STATE),
                field::<u32>(data, DATA_STATE_MASK),
            )
        };
        if mask & NIS_HIDDEN != 0 {
            entry.hidden = state & NIS_HIDDEN != 0;
        }
    }
    if let Err(code) = show(&entry, false) {
        return fail(code, FALSE);
    }
    if flags & NIF_INFO != 0 && size >= DATA_INFO_TITLE + INFO_TITLE_UNITS * 2 {
        // SAFETY: as above; the structure is long enough for the balloon's text and title.
        let _ = unsafe { show_balloon(&entry, data) };
    }
    let stored = TRAY.with(|icons| {
        let slot = match found {
            Some(at) => &mut icons[at],
            None => icons.iter_mut().find(|slot| slot.is_none())?,
        };
        *slot = Some(entry);
        Some(())
    });
    match stored {
        Some(()) => TRUE,
        None => {
            let _ = show(&entry, true);
            fail(error::NOT_ENOUGH_MEMORY, FALSE)
        }
    }
}

unsafe fn units_at(data: *const u8, offset: usize, room: usize, out: &mut [u16]) -> usize {
    let mut length = 0;
    while length < room.min(out.len()) {
        // SAFETY: the caller's contract: `room` units at `offset`.
        let unit = unsafe { field::<u16>(data, offset + length * 2) };
        if unit == 0 {
            break;
        }
        out[length] = unit;
        length += 1;
    }
    length
}

unsafe fn show_balloon(entry: &TrayEntry, data: *const u8) -> Result<(), u32> {
    let mut info = [0u16; INFO_UNITS];
    let mut title = [0u16; INFO_TITLE_UNITS];
    // SAFETY: the caller's contract.
    let info_length = unsafe { units_at(data, DATA_INFO, INFO_UNITS, &mut info) };
    // SAFETY: as above.
    let title_length = unsafe { units_at(data, DATA_INFO_TITLE, INFO_TITLE_UNITS, &mut title) };
    let mut joined = [0u16; INFO_UNITS + INFO_TITLE_UNITS + 2];
    let mut length = 0;
    let mut put = |units: &[u16]| {
        for &unit in units {
            if length < joined.len() {
                joined[length] = unit;
                length += 1;
            }
        }
    };
    if info_length > 0 {
        put(&title[..title_length]);
        if title_length > 0 {
            put(&[u16::from(b':'), u16::from(b' ')]);
        }
        put(&info[..info_length]);
    }
    let server = server_window(entry.window).ok_or(error::INVALID_WINDOW_HANDLE)?;
    let device = super::gui::device()?;
    let mut text = [0u8; TIP_BYTES];
    let (written, _) = text::write_utf8(&joined[..length], &mut text);
    let text = core::str::from_utf8(&text[..written]).unwrap_or("");
    device
        .set_tray_icon((server, entry.key, TRAY_BALLOON), text, None)
        .map_err(|_| error::NOT_ENOUGH_MEMORY)
}
unsafe extern "efiapi" fn shell_notify_icon_get_rect(
    identifier: *const u8,
    rect: *mut i32,
) -> i32 {
    if identifier.is_null() || rect.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's `NOTIFYICONIDENTIFIER` starts with its size.
    let size = unsafe { field::<u32>(identifier, 0) } as usize;
    if size < IDENTIFIER_MINIMUM_SIZE {
        return E_INVALIDARG;
    }
    // SAFETY: as above, the window and the id within the structure.
    let (window, id) = unsafe {
        (
            field::<usize>(identifier, IDENTIFIER_WINDOW),
            field::<u32>(identifier, IDENTIFIER_ID),
        )
    };
    let place = TRAY.with(|icons| {
        icons
            .iter()
            .flatten()
            .find(|icon| icon.window == window && icon.id == id)
            .map(|icon| icon.place)
    });
    let Some([left, top, width, height]) = place.filter(|place| place[2] > 0) else {
        return E_FAIL;
    };
    for (at, value) in [left, top, left + width, top + height].into_iter().enumerate() {
        // SAFETY: the caller's `RECT`, four `LONG`s.
        unsafe { rect.add(at).write_unaligned(value) };
    }
    S_OK
}

fn record_place(key: u32, (packed_x, packed_y): (u32, u32)) {
    let half = |packed: u32| {
        (
            (packed & PLACE_HALF_MASK) as i32,
            (packed >> PLACE_HALF_BITS) as i32,
        )
    };
    let ((left, width), (top, height)) = (half(packed_x), half(packed_y));
    TRAY.with(|icons| {
        if let Some(icon) = icons.iter_mut().flatten().find(|icon| icon.key == key) {
            icon.place = [left, top, width, height];
        }
    });
}

fn is_second_click(key: u32, down: u32, point: (i32, i32)) -> bool {
    let now = super::user32::now_ms();
    LAST_TRAY_CLICK.with(|last| {
        let second = last.is_some_and(|click| {
            click.key == key
                && click.down == down
                && !click.doubled
                && now.wrapping_sub(click.time) <= super::gui::DOUBLE_CLICK_MS
                && (click.point.0 - point.0).abs() <= super::gui::DOUBLE_CLICK_DISTANCE
                && (click.point.1 - point.1).abs() <= super::gui::DOUBLE_CLICK_DISTANCE
        });
        *last = Some(TrayClick {
            key,
            down,
            time: now,
            point,
            doubled: second,
        });
        second
    })
}

fn ends_double_click(key: u32, up: u32) -> bool {
    LAST_TRAY_CLICK.with(|last| {
        last.is_some_and(|click| click.key == key && click.doubled && click.down + 1 == up)
    })
}

/// A message the server reported on icon `key` (D357, D719): the icon's callback message
/// posted to its window, packed by its version: `uID` and the message for versions 0 and 3, the
/// point and `MAKELONG(message, uID)` for version 4. A second press soon after the first, near
/// it, is that button's double click. Versions 3 and 4 add `NIN_SELECT` after a left up that
/// ends no double click and take the keyboard's choice as `NIN_KEYSELECT` and its menu key as
/// `WM_CONTEXTMENU`, which version 0 gets as a left or right click; version 4 adds
/// `WM_CONTEXTMENU` after a right up and gets `NIN_POPUPOPEN` and `NIN_POPUPCLOSE`. A
/// `TRAY_PLACE` record is the icon's place in the bar, kept for `Shell_NotifyIconGetRect`.
pub(super) fn tray_event(key: u32, message: u32, (x, y): (i32, i32)) {
    if message == TRAY_PLACE {
        record_place(key, (x as u32, y as u32));
        return;
    }
    let found = TRAY.with(|icons| icons.iter().flatten().find(|icon| icon.key == key).copied());
    let Some(entry) = found else {
        return;
    };
    if entry.callback == 0 {
        return;
    }
    let point = (x as u16 as usize) | ((y as u16 as usize) << 16);
    let post = |number: u32| {
        let (wparam, lparam) = match entry.version {
            4 => (
                point,
                (number as usize & 0xffff) | ((entry.id as usize & 0xffff) << 16),
            ),
            _ => (entry.id as usize, number as usize),
        };
        let _ = super::gui::post(entry.window, entry.callback, wparam, lparam);
    };
    let modern = matches!(entry.version, 3 | 4);
    match message {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            match is_second_click(key, message, (x, y)) {
                true => post(message + DOUBLE_CLICK_AFTER_DOWN),
                false => post(message),
            }
        }
        WM_LBUTTONUP => {
            post(message);
            if modern && !ends_double_click(key, message) {
                post(NIN_SELECT);
            }
        }
        WM_RBUTTONUP => {
            post(message);
            if entry.version == 4 {
                post(WM_CONTEXTMENU);
            }
        }
        NIN_KEYSELECT | WM_CONTEXTMENU if modern => post(message),
        NIN_KEYSELECT => {
            post(WM_LBUTTONDOWN);
            post(WM_LBUTTONUP);
        }
        WM_CONTEXTMENU => {
            post(WM_RBUTTONDOWN);
            post(WM_RBUTTONUP);
        }
        NIN_POPUPOPEN | NIN_POPUPCLOSE if entry.version != 4 => {}
        _ => post(message),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DragQueryFileW" => drag_query_file_w,
    b"DragQueryPoint" => drag_query_point,
    b"DragFinish" => drag_finish,
    b"ExtractIconW" => extract_icon_w,
    b"Shell_NotifyIconW" => shell_notify_icon_w,
    b"Shell_NotifyIconGetRect" => shell_notify_icon_get_rect,
];
