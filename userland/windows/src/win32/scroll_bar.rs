//! `SCROLLBAR` controls (D228, `docs/ux/windows-controls-v0.3.md` section 14): their `SBM_*`
//! messages, user32's scroll bar functions on `SB_CTL` and on a window's own bars (`SB_HORZ`,
//! `SB_VERT`, D342), and the `WM_VSCROLL`/`WM_HSCROLL` records the controls send their parent.

use super::controls::{invalidate, with_scroll_bar};
use super::gui::{self, with};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::controls::scroll_bar::{Scroll, ScrollBar, ScrollInfo, mask};

pub const SBM_SETPOS: u32 = 0x00e0;
const SBM_GETPOS: u32 = 0x00e1;
const SBM_SETRANGE: u32 = 0x00e2;
const SBM_GETRANGE: u32 = 0x00e3;
const SBM_SETRANGEREDRAW: u32 = 0x00e6;
const SBM_SETSCROLLINFO: u32 = 0x00e9;
pub const SBM_GETSCROLLINFO: u32 = 0x00ea;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const SB_HORZ: i32 = 0;
const SB_VERT: i32 = 1;
const SB_CTL: i32 = 2;
const SB_BOTH: i32 = 3;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
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
    let Some(record) = record_for(number, lparam) else {
        return 0;
    };
    let answer = with_scroll_bar(window, |bar| bar_message(bar, number, wparam, lparam, record));
    if changes(number) {
        invalidate(window);
    }
    answer.unwrap_or(0)
}

fn changes(number: u32) -> bool {
    matches!(
        number,
        SBM_SETPOS | SBM_SETRANGE | SBM_SETRANGEREDRAW | SBM_SETSCROLLINFO
    )
}

fn record_for(number: u32, lparam: usize) -> Option<Option<ScrollInfoRecord>> {
    match number {
        SBM_SETSCROLLINFO | SBM_GETSCROLLINFO => read_record(lparam).map(Some),
        _ => Some(None),
    }
}

fn bar_message(
    bar: &mut ScrollBar,
    number: u32,
    wparam: usize,
    lparam: usize,
    record: Option<ScrollInfoRecord>,
) -> usize {
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
                    // SAFETY: `SBM_GETRANGE`'s contract: each non-null parameter points to a
                    // writable `INT`.
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
}

/// The `SBM_*` message `number` done on window `window`'s own `vertical` (or horizontal) bar
/// (D342). A new range or page that makes the bar needed shows it and one that makes it
/// unneeded hides it, unless `SIF_DISABLENOSCROLL` keeps it, as Windows does.
fn window_bar_message(
    window: usize,
    vertical: bool,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    let Some(record) = record_for(number, lparam) else {
        return 0;
    };
    let fields = record.map_or(0, |record| record.fields);
    let keep = fields & mask::DISABLE_NO_SCROLL != 0;
    let ranged = matches!(number, SBM_SETRANGE | SBM_SETRANGEREDRAW)
        || (number == SBM_SETSCROLLINFO && fields & (mask::RANGE | mask::PAGE) != 0);
    let answer = with(|gui| {
        let entry = gui.windows.get_mut(window)?;
        let bar = entry.bar_mut(vertical);
        let result = bar_message(bar, number, wparam, lparam, record);
        Some((result, bar.info.scrolls()))
    })
    .flatten();
    let Some((result, needed)) = answer else {
        return 0;
    };
    if ranged {
        set_shown(window, vertical, needed || keep);
    }
    if changes(number) {
        super::window_scroll_bars::show(window);
    }
    result
}

/// Shows or hides window `window`'s own `vertical` (or horizontal) bar: its `WS_VSCROLL` (or
/// `WS_HSCROLL`) set or cleared, its client area fitted, and `WM_SIZE` when that changed it.
fn set_shown(window: usize, vertical: bool, shown: bool) {
    let bit = match vertical {
        true => windows::WS_VSCROLL,
        false => windows::WS_HSCROLL,
    };
    let resized = with(|gui| {
        let entry = gui
            .windows
            .get_mut(window)
            .filter(|entry| matches!(entry.procedure, windows::Procedure::Program(_)))?;
        let before = (entry.width, entry.height);
        match shown {
            true => entry.style |= bit,
            false => entry.style &= !bit,
        }
        entry.fit_client();
        let after = (entry.width, entry.height);
        if before == after {
            return None;
        }
        entry.invalidate(None, true);
        let parent = entry.parent;
        let bounds = entry.bounds;
        if parent != 0 {
            windows::invalidate_tree(&mut gui.windows, parent, Some(bounds), true);
        }
        Some(message::word_pair(after.0 as usize, after.1 as usize))
    })
    .flatten();
    if let Some(size) = resized {
        gui::send(window, message::SIZE, message::SIZE_RESTORED, size);
        gui::wake();
    }
}

fn is_scroll_bar_control(window: usize) -> bool {
    with(|gui| vertical_of(gui, window).is_some()).unwrap_or(false)
}

fn is_window(window: usize) -> bool {
    with(|gui| gui.windows.get(window).is_some()).unwrap_or(false)
}

#[derive(Clone, Copy)]
enum Target {
    Control(usize),
    Window(usize, bool),
}

fn target_of(window: usize, bar: i32) -> Result<Target, u32> {
    match bar {
        SB_CTL if is_scroll_bar_control(window) => Ok(Target::Control(window)),
        SB_HORZ | SB_VERT if is_window(window) => Ok(Target::Window(window, bar == SB_VERT)),
        SB_CTL | SB_HORZ | SB_VERT => Err(error::INVALID_WINDOW_HANDLE),
        _ => Err(error::INVALID_PARAMETER),
    }
}

fn send_to(target: Target, number: u32, wparam: usize, lparam: usize) -> usize {
    match target {
        Target::Control(control) => gui::send(control, number, wparam, lparam),
        Target::Window(window, vertical) => {
            window_bar_message(window, vertical, number, wparam, lparam)
        }
    }
}

extern "efiapi" fn set_scroll_info(window: usize, bar: i32, info: usize, _redraw: Bool) -> i32 {
    match target_of(window, bar) {
        Ok(target) => send_to(target, SBM_SETSCROLLINFO, 0, info) as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_scroll_info(window: usize, bar: i32, info: usize) -> Bool {
    if read_record(info).is_none() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match target_of(window, bar) {
        Ok(target) => match send_to(target, SBM_GETSCROLLINFO, 0, info) {
            0 => FALSE,
            _ => TRUE,
        },
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn set_scroll_pos(window: usize, bar: i32, position: i32, _redraw: Bool) -> i32 {
    match target_of(window, bar) {
        Ok(target) => send_to(target, SBM_SETPOS, position as usize, 0) as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_scroll_pos(window: usize, bar: i32) -> i32 {
    match target_of(window, bar) {
        Ok(target) => send_to(target, SBM_GETPOS, 0, 0) as i32,
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
    match target_of(window, bar) {
        Ok(target) => {
            send_to(target, SBM_SETRANGE, minimum as usize, maximum as usize);
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
    match target_of(window, bar) {
        Ok(target) => {
            send_to(target, SBM_GETRANGE, minimum, maximum);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

/// `ShowScrollBar`: a window's own bars (`SB_HORZ`, `SB_VERT`, `SB_BOTH`) shown or hidden
/// (D342); a scroll bar control (`SB_CTL`) is shown or hidden as a window.
extern "efiapi" fn show_scroll_bar(window: usize, bar: i32, show: Bool) -> Bool {
    let shown = show != FALSE;
    let bars: &[bool] = match bar {
        SB_HORZ => &[false],
        SB_VERT => &[true],
        SB_BOTH => &[false, true],
        SB_CTL if is_scroll_bar_control(window) => {
            super::user32::show_window(window, if shown { SW_SHOW } else { SW_HIDE });
            return TRUE;
        }
        SB_CTL => return fail(error::INVALID_WINDOW_HANDLE, FALSE),
        _ => return fail(error::INVALID_PARAMETER, FALSE),
    };
    if !is_window(window) {
        return fail(error::INVALID_WINDOW_HANDLE, FALSE);
    }
    for &vertical in bars {
        set_shown(window, vertical, shown);
    }
    super::window_scroll_bars::show(window);
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetScrollInfo" => set_scroll_info,
    b"GetScrollInfo" => get_scroll_info,
    b"SetScrollPos" => set_scroll_pos,
    b"GetScrollPos" => get_scroll_pos,
    b"SetScrollRange" => set_scroll_range,
    b"GetScrollRange" => get_scroll_range,
    b"ShowScrollBar" => show_scroll_bar,
];
