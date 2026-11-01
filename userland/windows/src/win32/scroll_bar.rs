//! `SCROLLBAR` controls (D227, `docs/ux/windows-controls-v0.3.md` section 14): their `SBM_*`
//! messages, user32's scroll bar functions on `SB_CTL`, and the `WM_VSCROLL`/`WM_HSCROLL`
//! records they send their parent.

use super::controls::{invalidate, with_scroll_bar};
use super::gui::{self, with};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::library::Export;
use eggos_windows_gui::controls::scroll_bar::{Scroll, ScrollInfo, mask};

pub const SBM_SETPOS: u32 = 0x00e0;
const SBM_GETPOS: u32 = 0x00e1;
const SBM_SETRANGE: u32 = 0x00e2;
const SBM_GETRANGE: u32 = 0x00e3;
const SBM_SETRANGEREDRAW: u32 = 0x00e6;
const SBM_SETSCROLLINFO: u32 = 0x00e9;
pub const SBM_GETSCROLLINFO: u32 = 0x00ea;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const SB_CTL: i32 = 2;
const SCROLL_INFO_SIZE: u32 = core::mem::size_of::<ScrollInfoRecord>() as u32;

#[repr(C)]
#[derive(Clone, Copy)]
struct ScrollInfoRecord {
    size: u32,
    fields: u32,
    minimum: i32,
    maximum: i32,
    page: u32,
    position: i32,
    track_position: i32,
}

/// Sends scroll bar `window`'s parent the `WM_VSCROLL` or `WM_HSCROLL` `scroll` asks for.
pub(super) fn send_scroll(window: usize, scroll: Option<Scroll>) {
    let Some(scroll) = scroll else {
        return;
    };
    let target = with(|gui| {
        let entry = gui.windows.get(window)?;
        let vertical = vertical_of(gui, window).unwrap_or(false);
        Some((entry.parent, vertical))
    })
    .flatten();
    let Some((parent, vertical)) = target.filter(|(parent, _)| *parent != 0) else {
        return;
    };
    let number = match vertical {
        true => WM_VSCROLL,
        false => WM_HSCROLL,
    };
    let wparam = message::word_pair(scroll.code as usize, scroll.position as u16 as usize);
    gui::send(parent, number, wparam, window);
}

fn vertical_of(gui: &gui::Gui, window: usize) -> Option<bool> {
    super::controls::read_scroll_bar(gui, window, |bar| bar.vertical)
}

fn read_record(address: usize) -> Option<ScrollInfoRecord> {
    if address == 0 {
        return None;
    }
    // SAFETY: the caller's contract: a non-null `LPSCROLLINFO` points to a readable
    // `SCROLLINFO`; only its size field is read before it is checked.
    let size = unsafe { (address as *const u32).read_unaligned() };
    if size != SCROLL_INFO_SIZE {
        return None;
    }
    // SAFETY: as above, and `cbSize` says the whole structure is there.
    Some(unsafe { (address as *const ScrollInfoRecord).read_unaligned() })
}

/// The `SBM_*` messages of scroll bar control `window`.
pub(super) fn control_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let record = match number {
        SBM_SETSCROLLINFO | SBM_GETSCROLLINFO => match read_record(lparam) {
            Some(record) => Some(record),
            None => return 0,
        },
        _ => None,
    };
    let answer = with_scroll_bar(window, |bar| {
        let previous = bar.info.position;
        let page = bar.info.page;
        let given = |minimum: i32, maximum: i32, position: i32| ScrollInfo {
            minimum,
            maximum,
            page,
            position,
        };
        match number {
            SBM_SETPOS => {
                bar.set_info(mask::POSITION, &given(0, 0, wparam as i32));
                previous as usize
            }
            SBM_GETPOS => bar.info.position as usize,
            SBM_SETRANGE | SBM_SETRANGEREDRAW => {
                bar.set_info(mask::RANGE, &given(wparam as i32, lparam as i32, 0));
                previous as usize
            }
            SBM_GETRANGE => {
                for (out, value) in [(wparam, bar.info.minimum), (lparam, bar.info.maximum)] {
                    if out != 0 {
                        // SAFETY: `SBM_GETRANGE`'s contract: each non-null parameter points
                        // to a writable `INT`.
                        unsafe { (out as *mut i32).write_unaligned(value) };
                    }
                }
                0
            }
            SBM_SETSCROLLINFO => {
                let Some(record) = record else {
                    return 0;
                };
                let info = ScrollInfo {
                    minimum: record.minimum,
                    maximum: record.maximum,
                    page: record.page,
                    position: record.position,
                };
                bar.set_info(record.fields, &info) as usize
            }
            SBM_GETSCROLLINFO => {
                let Some(mut record) = record else {
                    return 0;
                };
                if record.fields & mask::RANGE != 0 {
                    record.minimum = bar.info.minimum;
                    record.maximum = bar.info.maximum;
                }
                if record.fields & mask::PAGE != 0 {
                    record.page = bar.info.page;
                }
                if record.fields & mask::POSITION != 0 {
                    record.position = bar.info.position;
                }
                if record.fields & mask::TRACK_POSITION != 0 {
                    record.track_position = bar.track_position();
                }
                // SAFETY: `read_record` checked the pointer and the size; `SBM_GETSCROLLINFO`'s
                // structure is writable.
                unsafe { (lparam as *mut ScrollInfoRecord).write_unaligned(record) };
                1
            }
            _ => 0,
        }
    });
    if matches!(
        number,
        SBM_SETPOS | SBM_SETRANGE | SBM_SETRANGEREDRAW | SBM_SETSCROLLINFO
    ) {
        invalidate(window);
    }
    answer.unwrap_or(0)
}

fn is_scroll_bar_control(window: usize) -> bool {
    with(|gui| vertical_of(gui, window).is_some()).unwrap_or(false)
}

fn control_of(window: usize, bar: i32) -> Result<usize, u32> {
    match bar {
        SB_CTL if is_scroll_bar_control(window) => Ok(window),
        SB_CTL => Err(error::INVALID_WINDOW_HANDLE),
        _ => Err(error::NOT_SUPPORTED),
    }
}

extern "efiapi" fn set_scroll_info(window: usize, bar: i32, info: usize, _redraw: Bool) -> i32 {
    match control_of(window, bar) {
        Ok(control) => gui::send(control, SBM_SETSCROLLINFO, 0, info) as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_scroll_info(window: usize, bar: i32, info: usize) -> Bool {
    if read_record(info).is_none() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match control_of(window, bar) {
        Ok(control) => match gui::send(control, SBM_GETSCROLLINFO, 0, info) {
            0 => FALSE,
            _ => TRUE,
        },
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn set_scroll_pos(window: usize, bar: i32, position: i32, _redraw: Bool) -> i32 {
    match control_of(window, bar) {
        Ok(control) => gui::send(control, SBM_SETPOS, position as usize, 0) as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_scroll_pos(window: usize, bar: i32) -> i32 {
    match control_of(window, bar) {
        Ok(control) => gui::send(control, SBM_GETPOS, 0, 0) as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn set_scroll_range(
    window: usize,
    bar: i32,
    minimum: i32,
    maximum: i32,
    _redraw: Bool,
) -> Bool {
    match control_of(window, bar) {
        Ok(control) => {
            gui::send(
                control,
                SBM_SETRANGE,
                minimum as usize,
                maximum as usize,
            );
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn get_scroll_range(
    window: usize,
    bar: i32,
    minimum: usize,
    maximum: usize,
) -> Bool {
    if minimum == 0 || maximum == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match control_of(window, bar) {
        Ok(control) => {
            gui::send(control, SBM_GETRANGE, minimum, maximum);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetScrollInfo" => set_scroll_info,
    b"GetScrollInfo" => get_scroll_info,
    b"SetScrollPos" => set_scroll_pos,
    b"GetScrollPos" => get_scroll_pos,
    b"SetScrollRange" => set_scroll_range,
    b"GetScrollRange" => get_scroll_range,
];
