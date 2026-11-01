//! `msctls_statusbar32` and `CreateStatusWindowW` (D178, design section 21): the layer's
//! procedure over `eggos_windows_gui`'s status bar model, docked at its parent's bottom (or top)
//! at creation and on every `WM_SIZE` it receives, answering the `SB_*` messages, keeping its
//! window text as part 0's text, and painting its parts with their icons; its parts are labels in
//! agents' tree.

use super::super::controls::draw_bitmaps;
use super::super::gui::{Gui, with};
use super::super::user32::{Win32Rect, create_window_ex_w};
use super::super::window_text::{self, WINDOW_TEXT_CAPACITY};
use super::super::{TRUE, placement};
use super::style::{CCS_NOPARENTALIGN, CCS_NORESIZE, CCS_TOP};
use super::{
    Model, allocate, default_procedure, invalidate, item_element, paint_with, read_state, utf8,
    with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, Window};
use crate::library::Export;
use eggos_element_tree::{Bounds, Element, MAX_LABEL_BYTES, Role, TreeWriter};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::status_bar::{
    self, HEIGHT, MAX_PARTS, SIMPLE_PART, StatusBar, StatusLook,
};
use eggos_windows_gui::surface::{Rect, Surface};

const WM_USER: u32 = 0x0400;
const SB_SETPARTS: u32 = WM_USER + 4;
const SB_GETPARTS: u32 = WM_USER + 6;
const SB_GETBORDERS: u32 = WM_USER + 7;
const SB_SETMINHEIGHT: u32 = WM_USER + 8;
const SB_SIMPLE: u32 = WM_USER + 9;
const SB_GETRECT: u32 = WM_USER + 10;
const SB_SETTEXTW: u32 = WM_USER + 11;
const SB_GETTEXTLENGTHW: u32 = WM_USER + 12;
const SB_GETTEXTW: u32 = WM_USER + 13;
const SB_ISSIMPLE: u32 = WM_USER + 14;
const SB_SETICON: u32 = WM_USER + 15;
const SB_GETICON: u32 = WM_USER + 20;
const SB_SETBKCOLOR: u32 = 0x2001;
const CCS_ALIGN: u32 = 0x3;
const PART_BYTE: usize = 0xff;
const DRAWING_TYPE: usize = 0xff00;
const TAB: u16 = 0x09;
const SPACE: u16 = 0x20;
const ICON_SIZE: i32 = 16;
const CLASS: [u16; 19] = ascii_wide(b"msctls_statusbar32\0");

const fn ascii_wide<const N: usize>(text: &[u8; N]) -> [u16; N] {
    let mut wide = [0u16; N];
    let mut at = 0;
    while at < N {
        wide[at] = text[at] as u16;
        at += 1;
    }
    wide
}

/// A status bar's model with the bitmaps of the icons its parts show (an icon's colour and mask
/// bitmaps, found when `SB_SETICON` gives it).
pub(super) struct StatusState {
    bar: StatusBar,
    icons: [(usize, usize); MAX_PARTS],
}

impl Model for StatusState {
    const KIND: CommonKind = CommonKind::StatusBar;
}

/// A new status bar's state.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(StatusState {
        bar: StatusBar::new(),
        icons: [(0, 0); MAX_PARTS],
    })
}

/// Puts `window`, a status bar or toolbar `height` tall, where design section 20 docks it in its
/// parent's client area: the top or bottom edge (`bottom`) across the whole width, unless
/// `CCS_NOPARENTALIGN` keeps its position and `CCS_NORESIZE` its size.
pub(super) fn dock(window: usize, height: i32, bottom: bool) {
    let wanted = with(|gui| {
        let entry = gui.windows.get(window)?;
        let parent = gui.windows.get(entry.parent)?;
        let (client_width, client_height) = (parent.width as i32, parent.height as i32);
        let bounds = entry.bounds;
        let (width, height) = match entry.style & CCS_NORESIZE {
            0 => (client_width, height),
            _ => (bounds.width(), bounds.height()),
        };
        let (x, y) = match (entry.style & CCS_NOPARENTALIGN, bottom) {
            (0, true) => (0, client_height - height),
            (0, false) => (0, 0),
            _ => (bounds.left, bounds.top),
        };
        let wanted = Rect::sized(x, y, width, height);
        (wanted != bounds).then_some(wanted)
    })
    .flatten();
    if let Some(wanted) = wanted {
        let _ = placement::place(
            window,
            Some((wanted.left, wanted.top)),
            Some((wanted.width(), wanted.height())),
        );
    }
}

fn dock_bar(window: usize) {
    let style = with(|gui| gui.windows.get(window).map(|entry| entry.style)).flatten();
    if let Some(style) = style {
        dock(window, HEIGHT, style & CCS_ALIGN != CCS_TOP);
    }
}

fn adopt_window_text(window: usize) {
    let mut units = [0u16; WINDOW_TEXT_CAPACITY];
    let length = with(|gui| {
        let text = window_text::of(gui, window);
        let length = text.len().min(WINDOW_TEXT_CAPACITY);
        units[..length].copy_from_slice(&text[..length]);
        window_text::store(gui, window, &[]);
        length
    })
    .unwrap_or(0);
    with_state::<StatusState, _>(window, |_, state| {
        state.bar.set_text(0, 0, &units[..length]);
    });
}

fn part_of(wparam: usize) -> usize {
    match wparam as isize {
        -1 => SIMPLE_PART,
        _ => wparam & PART_BYTE,
    }
}

fn length_and_kind(bar: &StatusBar, part: usize) -> usize {
    message::word_pair(bar.text(part).len(), usize::from(bar.kind(part)))
}

/// The procedure of every status bar.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::CREATE => {
            adopt_window_text(window);
            dock_bar(window);
            0
        }
        message::SIZE => {
            dock_bar(window);
            invalidate(window);
            0
        }
        message::ENABLE => {
            invalidate(window);
            0
        }
        message::PAINT => {
            paint(window);
            0
        }
        message::SET_TEXT => set_text(window, 0, lparam),
        message::GET_TEXT => {
            with_state::<StatusState, _>(window, |_, state| {
                // SAFETY: `WM_GETTEXT`'s contract: `lParam` holds `wParam` writable units.
                unsafe { window_text::copy_out(state.bar.text(0), lparam, wparam) }
            })
            .unwrap_or(0)
        }
        message::GET_TEXT_LENGTH => {
            with_state::<StatusState, _>(window, |_, state| state.bar.text(0).len()).unwrap_or(0)
        }
        SB_SETTEXTW => set_text(window, wparam, lparam),
        SB_SETICON => set_icon(window, part_of(wparam), lparam),
        SB_SETPARTS..=SB_GETICON | SB_SETBKCOLOR => {
            let answer = with_state::<StatusState, _>(window, |entry, state| {
                bar_message(entry, state, number, wparam, lparam)
            });
            let Some((result, changed)) = answer else {
                return 0;
            };
            if changed {
                invalidate(window);
            }
            result
        }
        _ => default_procedure(window, number, wparam, lparam),
    }
}

fn set_text(window: usize, wparam: usize, text: usize) -> usize {
    let kind = (wparam & DRAWING_TYPE) as u16;
    let units = match text {
        0 => &[][..],
        _ if kind & status_bar::drawing::OWNER_DRAW != 0 => &[][..],
        // SAFETY: `SB_SETTEXTW`'s and `WM_SETTEXT`'s `lParam` is a NUL-terminated string unless
        // the part is owner-drawn.
        text => unsafe { super::super::file::wide_string(text as *const u16) },
    };
    let set = with_state::<StatusState, _>(window, |_, state| {
        state.bar.set_text(part_of(wparam), kind, units)
    });
    invalidate(window);
    usize::from(set == Some(true))
}

fn set_icon(window: usize, part: usize, icon: usize) -> usize {
    let bitmaps = match icon {
        0 => Some((0, 0)),
        icon => super::super::icon::icon_bitmaps(icon),
    };
    let Some(bitmaps) = bitmaps else {
        return 0;
    };
    let set = with_state::<StatusState, _>(window, |_, state| {
        let set = state.bar.set_icon(part, icon).is_some();
        if set {
            state.icons[part] = bitmaps;
        }
        set
    });
    invalidate(window);
    usize::from(set == Some(true))
}

fn bar_message(
    entry: &mut Window,
    state: &mut StatusState,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> (usize, bool) {
    let bar = &mut state.bar;
    match number {
        SB_SETPARTS => {
            let count = wparam.min(MAX_PARTS + 1);
            if lparam == 0 || count == 0 || count > MAX_PARTS {
                return (0, false);
            }
            // SAFETY: `SB_SETPARTS`' contract: `lParam` points to `wParam` readable `int`s.
            let edges = unsafe { core::slice::from_raw_parts(lparam as *const i32, count) };
            (usize::from(bar.set_parts(edges)), true)
        }
        SB_GETPARTS => {
            let edges = bar.edges();
            if lparam != 0 {
                let count = wparam.min(edges.len());
                // SAFETY: `SB_GETPARTS`' contract: `lParam` is 0 or `wParam` writable `int`s.
                let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut i32, count) };
                out.copy_from_slice(&edges[..count]);
            }
            (edges.len(), false)
        }
        SB_GETBORDERS => {
            if lparam != 0 {
                // SAFETY: `SB_GETBORDERS`' contract: `lParam` is three writable `int`s.
                unsafe { (lparam as *mut [i32; 3]).write([0; 3]) };
            }
            (TRUE as usize, false)
        }
        SB_SETMINHEIGHT => (0, false),
        SB_SIMPLE => {
            bar.simple = wparam != 0;
            (0, true)
        }
        SB_ISSIMPLE => (usize::from(bar.simple), false),
        SB_GETRECT => {
            let rect = bar.part_rect(wparam, entry.width as i32);
            match (rect, lparam) {
                (Some(rect), out) if out != 0 => {
                    // SAFETY: `SB_GETRECT`'s contract: `lParam` is a writable `RECT`.
                    unsafe {
                        (out as *mut Win32Rect).write(Win32Rect {
                            left: rect.left,
                            top: rect.top,
                            right: rect.right,
                            bottom: rect.bottom,
                        })
                    };
                    (TRUE as usize, false)
                }
                _ => (0, false),
            }
        }
        SB_GETTEXTW => {
            let part = part_of(wparam);
            if lparam != 0 {
                let text = bar.text(part);
                // SAFETY: `SB_GETTEXTW`'s contract: `lParam` has room for the text and a NUL.
                unsafe { window_text::copy_out(text, lparam, text.len() + 1) };
            }
            (length_and_kind(bar, part), false)
        }
        SB_GETTEXTLENGTHW => (length_and_kind(bar, part_of(wparam)), false),
        SB_GETICON => (bar.icon(part_of(wparam)), false),
        SB_SETBKCOLOR => (
            core::mem::replace(&mut bar.background, lparam as u32) as usize,
            true,
        ),
        _ => (0, false),
    }
}

fn draw_icon(
    (gui, state): (&Gui, &StatusState),
    surface: &mut Surface<'_>,
    icon: usize,
    x: i32,
    y: i32,
) {
    let Some(part) = (0..MAX_PARTS).find(|&part| state.bar.icon(part) == icon) else {
        return;
    };
    let saved = surface.clip();
    surface.clip_to(Rect::sized(x, y, ICON_SIZE, ICON_SIZE));
    draw_bitmaps(gui, surface, state.icons[part], x, y);
    surface.set_clip(saved);
}

fn paint(window: usize) {
    paint_with::<StatusState>(window, |surface, rect, _, state, gui| {
        let look = StatusLook {
            enabled: windows::enabled_in_tree(&gui.windows, window),
        };
        status_bar::paint(
            surface,
            rect,
            &state.bar,
            look,
            &LIGHT,
            &mut |surface, icon, x, y| draw_icon((gui, state), surface, icon, x, y),
        );
    });
}

/// A status bar's role in agents' tree.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::Group, 0)
}

/// Pushes a status bar's parts, one label each named by its text (tabs as spaces).
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    read_state::<StatusState, _>(gui, window, |entry, state| {
        let bar = &state.bar;
        let width = entry.width as i32;
        for index in 0..bar.shown_parts() {
            let Some(rect) = bar.part_rect(index, width) else {
                break;
            };
            let shown = Rect::new(rect.left.min(width), 0, rect.right.min(width), HEIGHT);
            let mut units = [0u16; WINDOW_TEXT_CAPACITY];
            let text = bar.text(bar.source_part(index));
            let length = text.len().min(units.len());
            for (slot, unit) in units.iter_mut().zip(&text[..length]) {
                *slot = match *unit {
                    TAB => SPACE,
                    unit => unit,
                };
            }
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(&units[..length], &mut name);
            let bounds = Bounds::new(
                origin.0 + shown.left,
                origin.1,
                shown.width().max(0),
                shown.height(),
            );
            let item =
                Element::new(item_element(window, index), element, Role::Label, bounds).named(name);
            if !tree.push(&item) {
                return false;
            }
        }
        true
    })
    .unwrap_or(true)
}

unsafe extern "efiapi" fn create_status_window_w(
    style: i32,
    text: *const u16,
    parent: usize,
    id: u32,
) -> usize {
    // SAFETY: `CLASS` is NUL-terminated and `text` is the caller's NUL-terminated string or
    // null, as `CreateStatusWindowW`'s contract says.
    unsafe {
        create_window_ex_w(
            0,
            CLASS.as_ptr(),
            text,
            style as u32,
            0,
            0,
            0,
            0,
            parent,
            id as usize,
            0,
            0,
        )
    }
}

/// comctl32's `CreateStatusWindowW`.
pub(super) const EXPORTS: &[Export] = exports![
    b"CreateStatusWindowW" => create_status_window_w,
];
