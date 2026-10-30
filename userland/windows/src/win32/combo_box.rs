//! `COMBOBOX` with `CBS_DROPDOWNLIST` (D174, `docs/ux/windows-controls-v0.3.md` section 10): a
//! child window whose procedure is the layer's over `eggos_windows_gui`'s combo box model. Its
//! closed box paints on `WM_PAINT`; its list, once open, is drawn over the top-level window's
//! content in the same buffer, as a menu's drop-down is, and takes every input of that window
//! until it closes, whichever thread reads the server's records: the close is handed to the
//! combo box's own thread as a private message, which sends the parent its notifications.

use super::controls::{invalidate, notify, palette_of};
use super::dialog::{has_focus, set_focus};
use super::gui::{self, Delivery, Gui, with};
use super::user32::def_window_proc_w;
use super::window_device::Device;
use super::{heap, paint, window_text};
use crate::gui::message;
use crate::gui::windows::{self, ControlKind, Procedure};
use eggos_windows_gui::controls::combo_box::{
    self as model, Closed, ComboBox, ComboLook, ListPart, key,
};
use eggos_windows_gui::controls::scroll_bar::Part;
use eggos_windows_gui::surface::{Rect, Surface};

const CB_ERR: usize = usize::MAX;
const CB_ERRSPACE: usize = usize::MAX - 1;
const LIST_CLOSED: u32 = 0x03fe;
const CLOSED_CANCELLED: usize = 0;
const CLOSED_CHANGED: usize = 1;
const CLOSED_UNCHANGED: usize = 2;
const VK_TAB: u32 = 0x09;
const COMBO_ITEM_HEIGHT: usize = 20;
const COMBO_BOX_ITEM_HEIGHT: usize = 22;
const MOST_VISIBLE: usize = 30;
const SHOWN_TEXT_CAPACITY: usize = 256;

/// The messages a combo box answers and the notifications it sends.
pub mod code {
    pub const CB_GETEDITSEL: u32 = 0x0140;
    pub const CB_ADDSTRING: u32 = 0x0143;
    pub const CB_DELETESTRING: u32 = 0x0144;
    pub const CB_GETCOUNT: u32 = 0x0146;
    pub const CB_GETCURSEL: u32 = 0x0147;
    pub const CB_GETLBTEXT: u32 = 0x0148;
    pub const CB_GETLBTEXTLEN: u32 = 0x0149;
    pub const CB_INSERTSTRING: u32 = 0x014a;
    pub const CB_RESETCONTENT: u32 = 0x014b;
    pub const CB_FINDSTRING: u32 = 0x014c;
    pub const CB_SELECTSTRING: u32 = 0x014d;
    pub const CB_SETCURSEL: u32 = 0x014e;
    pub const CB_SHOWDROPDOWN: u32 = 0x014f;
    pub const CB_SETITEMHEIGHT: u32 = 0x0153;
    pub const CB_GETITEMHEIGHT: u32 = 0x0154;
    pub const CB_GETDROPPEDSTATE: u32 = 0x0157;
    pub const CB_FINDSTRINGEXACT: u32 = 0x0158;
    pub const CB_LAST: u32 = 0x0165;
    pub const CB_SETMINVISIBLE: u32 = 0x1701;
    pub const CB_GETMINVISIBLE: u32 = 0x1702;
    pub const CBN_SELCHANGE: usize = 1;
    pub const CBN_SETFOCUS: usize = 3;
    pub const CBN_KILLFOCUS: usize = 4;
    pub const CBN_DROPDOWN: usize = 7;
    pub const CBN_CLOSEUP: usize = 8;
    pub const CBN_SELENDOK: usize = 9;
    pub const CBN_SELENDCANCEL: usize = 10;
}

/// An open list: the top-level window it is drawn over, the combo box, and where it is in the
/// window's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracking {
    pub root: usize,
    pub combo: usize,
    pub rect: Rect,
}

/// What an input did to an open list: the content rectangle to show, and, when it closed, the
/// combo box, its thread and how it closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effect {
    pub commit: Option<Rect>,
    pub closed: Option<(usize, u32, usize)>,
}

/// A new combo box's state for a list the program gave `list_height` pixels; `None` without
/// memory.
pub fn new_state(list_height: i32) -> Option<usize> {
    let block = heap::allocate(core::mem::size_of::<ComboBox>()).cast::<ComboBox>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, 16-byte aligned heap block the size of a `ComboBox`.
    unsafe { block.write(ComboBox::new(list_height)) };
    Some(block as usize)
}

fn address_of(gui: &Gui, window: usize) -> Option<usize> {
    match gui.windows.get(window)?.procedure {
        Procedure::Control(ControlKind::ComboBox, address) if address != 0 => Some(address),
        _ => None,
    }
}

unsafe fn model<'a>(address: usize) -> &'a mut ComboBox {
    // SAFETY: the caller's duty: `address` is a combo box's state from the table, alive while
    // its window is in it, reached under the state's lock with no other reference made.
    unsafe { &mut *(address as *mut ComboBox) }
}

/// The text a combo box shows: its selected item's.
pub fn shown_text(gui: &Gui, window: usize) -> &[u16] {
    let Some(address) = address_of(gui, window) else {
        return &[];
    };
    // SAFETY: as `model`, a shared reference for the time `gui` is borrowed.
    let combo = unsafe { &*(address as *const ComboBox) };
    combo
        .selected()
        .and_then(|index| combo.items.text(index))
        .unwrap_or(&[])
}

fn with_root_surface<R>(
    gui: &Gui,
    root: usize,
    work: impl FnOnce(&mut Surface<'_>) -> R,
) -> Option<R> {
    let window = gui.windows.get(root)?;
    if window.pixels == 0 {
        return None;
    }
    // SAFETY: the top-level window's pixels are its mapped buffer of `buffer_width *
    // buffer_height` pixels, mapped while it is in the table; the caller holds the lock.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            window.pixels as *mut u32,
            window.buffer_width as usize * window.buffer_height as usize,
        )
    };
    let mut surface = Surface::new(pixels, window.buffer_width, window.buffer_height)?;
    Some(work(&mut surface))
}

/// Paints `root`'s open combo list over its content, if one is open; its rectangle.
pub fn paint_open_list(gui: &Gui, root: usize) -> Option<Rect> {
    let tracking = gui.combo.filter(|tracking| tracking.root == root)?;
    let address = address_of(gui, tracking.combo)?;
    let palette = palette_of(gui, tracking.combo);
    // SAFETY: as `model`, a shared reference while the lock is held through `gui`.
    let combo = unsafe { &*(address as *const ComboBox) };
    with_root_surface(gui, root, |surface| {
        model::paint_list(surface, tracking.rect, combo, palette)
    })?;
    Some(tracking.rect)
}

fn open(window: usize) {
    let shown = with(|gui| {
        let address = address_of(gui, window)?;
        let placement = windows::client_placement(&gui.windows, window)?;
        let width = gui.windows.get(window)?.width as i32;
        let top = gui.windows.get(placement.root)?;
        let content = Rect::new(
            0,
            top.menu_height,
            top.buffer_width as i32,
            top.buffer_height as i32,
        );
        let closed = Rect::sized(
            placement.origin.0,
            placement.origin.1,
            width,
            model::BOX_HEIGHT,
        );
        // SAFETY: as `model`; no other reference to the state is made meanwhile.
        let combo = unsafe { model(address) };
        combo.open();
        let rect = model::list_rect(closed, combo.rows(), content);
        if gui
            .tracking
            .is_some_and(|tracking| tracking.root == placement.root)
        {
            super::menu::close(gui, placement.root);
        }
        gui.combo = Some(Tracking {
            root: placement.root,
            combo: window,
            rect,
        });
        let painted = paint_open_list(gui, placement.root)?;
        let top = gui.windows.get(placement.root)?;
        let server = top.server;
        let committed = top.committed;
        Some((gui.device?, server, painted, committed))
    })
    .flatten();
    if let Some((device, server, rect, true)) = shown {
        let _ = device.commit(server, Some(rect));
    }
    invalidate(window);
}

fn close_list(gui: &mut Gui, window: usize, choose: bool) -> Option<Closed> {
    let tracking = gui.combo.filter(|tracking| tracking.combo == window)?;
    gui.combo = None;
    let address = address_of(gui, window)?;
    // SAFETY: as `model`; no other reference to the state is made meanwhile.
    let closed = unsafe { model(address) }.close(choose);
    let menu_height = gui.windows.get(tracking.root)?.menu_height;
    let under = tracking.rect.offset(0, -menu_height);
    windows::invalidate_tree(&mut gui.windows, tracking.root, Some(under), true);
    if let Some(entry) = gui.windows.get_mut(window) {
        entry.invalidate(None, true);
    }
    Some(closed)
}

fn closed_code(closed: Closed) -> usize {
    match closed {
        Closed::Chosen { changed: true } => CLOSED_CHANGED,
        Closed::Chosen { changed: false } => CLOSED_UNCHANGED,
        Closed::Cancelled => CLOSED_CANCELLED,
    }
}

fn send_closed_notifications(window: usize, code: usize) {
    match code {
        CLOSED_CANCELLED => {
            notify(window, code::CBN_SELENDCANCEL);
            notify(window, code::CBN_CLOSEUP);
        }
        changed => {
            notify(window, code::CBN_SELENDOK);
            notify(window, code::CBN_CLOSEUP);
            if changed == CLOSED_CHANGED {
                notify(window, code::CBN_SELCHANGE);
            }
        }
    }
}

fn close_and_notify(window: usize, choose: bool) {
    let closed = with(|gui| close_list(gui, window, choose)).flatten();
    if let Some(closed) = closed {
        gui::wake();
        send_closed_notifications(window, closed_code(closed));
    }
}

fn scroll(combo: &mut ComboBox, part: Part) -> bool {
    let page = combo.rows().saturating_sub(1).max(1) as isize;
    match part {
        Part::LineUp => combo.scroll_by(-1),
        Part::LineDown => combo.scroll_by(1),
        Part::PageUp => combo.scroll_by(-page),
        Part::PageDown => combo.scroll_by(page),
        Part::Thumb => false,
    }
}

/// What `root`'s open combo list makes of an input at `point` of its content (`character`, the
/// one a key press makes): `None` when no list is open there, else the effect (the input is
/// then the list's alone).
pub fn input(
    gui: &mut Gui,
    root: usize,
    number: u32,
    wparam: usize,
    point: (i32, i32),
    character: Option<u32>,
) -> Option<Effect> {
    let tracking = gui.combo.filter(|tracking| tracking.root == root)?;
    let address = address_of(gui, tracking.combo)?;
    let thread = gui.windows.get(tracking.combo)?.thread;
    // SAFETY: as `model`; the reference ends before `close_list` makes another.
    let combo = unsafe { model(address) };
    let part = model::list_part_at(tracking.rect, combo, point.0, point.1);
    let mut repaint = false;
    let mut close = None;
    match number {
        message::MOUSE_MOVE => {
            if let Some(ListPart::Row(row)) = part {
                repaint = combo.point_at_row(Some(row));
            }
        }
        message::LEFT_BUTTON_DOWN => match part {
            Some(ListPart::ScrollBar(bar)) => repaint = scroll(combo, bar),
            Some(ListPart::Row(row)) => repaint = combo.point_at_row(Some(row)),
            None => close = Some(false),
        },
        message::LEFT_BUTTON_UP => {
            if let Some(ListPart::Row(row)) = part {
                combo.point_at_row(Some(row));
                close = Some(true);
            }
        }
        message::SYS_KEY_DOWN if matches!(wparam as u32, key::UP | key::DOWN) => {
            close = Some(false)
        }
        message::KEY_DOWN | message::SYS_KEY_DOWN => match wparam as u32 {
            key::RETURN | VK_TAB => close = Some(true),
            key::ESCAPE | key::F4 => close = Some(false),
            virtual_key => {
                repaint = combo.key_open(virtual_key);
                if let Some(character) = character.filter(|&character| character >= 0x20) {
                    repaint |= combo.character(character);
                }
            }
        },
        _ => {}
    }
    let mut effect = Effect::default();
    if let Some(choose) = close {
        let closed = close_list(gui, tracking.combo, choose)?;
        effect.closed = Some((tracking.combo, thread, closed_code(closed)));
        return Some(effect);
    }
    if repaint {
        effect.commit = paint_open_list(gui, root);
    }
    Some(effect)
}

/// Shows what [`input`] changed and gives the closed combo box's private message to deliver.
pub fn finish_input(device: &Device, root: usize, effect: Effect) -> Option<Delivery> {
    let shown = with(|gui| {
        let window = gui.windows.get(root)?;
        Some((window.server, window.committed))
    })
    .flatten();
    if let (Some(rect), Some((server, true))) = (effect.commit, shown) {
        let _ = device.commit(server, Some(rect));
    }
    gui::wake();
    let (window, thread, code) = effect.closed?;
    Some(Delivery {
        window,
        thread,
        message: LIST_CLOSED,
        wparam: code,
        lparam: 0,
    })
}

fn paint_box(window: usize) {
    let Ok((hdc, _, _)) = paint::begin(window) else {
        return;
    };
    let _ = paint::draw_in(hdc, |surface, _, gui| {
        let (Some(surface), Some(entry), Some(address)) =
            (surface, gui.windows.get(window), address_of(gui, window))
        else {
            return;
        };
        // SAFETY: as `model`, a shared reference while the lock is held through `gui`.
        let combo = unsafe { &*(address as *const ComboBox) };
        let look = ComboLook {
            focused: has_focus(gui, window),
            enabled: entry.enabled,
        };
        model::paint_closed(
            surface,
            entry.client(),
            combo,
            look,
            palette_of(gui, window),
        );
    });
    paint::finish(hdc);
}

fn is_open(window: usize) -> bool {
    with(|gui| gui.combo.is_some_and(|tracking| tracking.combo == window)).unwrap_or(false)
}

fn left_button_down(window: usize) {
    let enabled = with(|gui| gui.windows.get(window).is_some_and(|entry| entry.enabled));
    if enabled != Some(true) {
        return;
    }
    set_focus(window);
    if is_open(window) {
        close_and_notify(window, false);
        return;
    }
    notify(window, code::CBN_DROPDOWN);
    open(window);
}

fn toggle(window: usize) {
    match is_open(window) {
        true => close_and_notify(window, false),
        false => {
            notify(window, code::CBN_DROPDOWN);
            open(window);
        }
    }
}

fn selection_moved(window: usize, moved: Option<bool>) {
    if moved == Some(true) {
        invalidate(window);
        notify(window, code::CBN_SELENDOK);
        notify(window, code::CBN_SELCHANGE);
    }
}

fn key_down(window: usize, virtual_key: u32) {
    if virtual_key == key::F4 {
        toggle(window);
        return;
    }
    let moved = with(|gui| {
        let address = address_of(gui, window)?;
        // SAFETY: as `model`.
        Some(unsafe { model(address) }.key_closed(virtual_key))
    })
    .flatten();
    selection_moved(window, moved);
}

fn character(window: usize, character: usize) {
    if character < 0x20 {
        return;
    }
    let moved = with(|gui| {
        let address = address_of(gui, window)?;
        // SAFETY: as `model`.
        Some(unsafe { model(address) }.character(character as u32))
    })
    .flatten();
    selection_moved(window, moved);
}

fn fold(unit: u16) -> u16 {
    match unit {
        0x41..=0x5a => unit + 0x20,
        unit => unit,
    }
}

fn find(combo: &ComboBox, after: usize, wanted: &[u16], exact: bool) -> Option<usize> {
    let count = combo.items.count();
    let start = match after as isize {
        -1 => 0,
        after => (after as usize + 1) % count.max(1),
    };
    (0..count)
        .map(|step| (start + step) % count)
        .find(|&index| {
            combo.items.text(index).is_some_and(|text| {
                let matches = |length: usize| {
                    text.len() >= length
                        && text[..length]
                            .iter()
                            .zip(wanted)
                            .all(|(a, b)| fold(*a) == fold(*b))
                };
                match exact {
                    true => text.len() == wanted.len() && matches(wanted.len()),
                    false => matches(wanted.len()),
                }
            })
        })
}

unsafe fn text_at<'a>(lparam: usize) -> &'a [u16] {
    match lparam {
        0 => &[],
        // SAFETY: the caller's duty: a NUL-terminated string.
        lparam => unsafe { super::file::wide_string(lparam as *const u16) },
    }
}

fn combo_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        code::CB_SHOWDROPDOWN => {
            let open_now = is_open(window);
            match (wparam != 0, open_now) {
                (true, false) => {
                    notify(window, code::CBN_DROPDOWN);
                    open(window);
                }
                (false, true) => close_and_notify(window, false),
                _ => {}
            }
            return 1;
        }
        code::CB_GETDROPPEDSTATE => return usize::from(is_open(window)),
        code::CB_GETITEMHEIGHT => {
            return match wparam as isize {
                -1 => COMBO_BOX_ITEM_HEIGHT,
                _ => COMBO_ITEM_HEIGHT,
            };
        }
        code::CB_SETITEMHEIGHT => return 0,
        code::CB_GETMINVISIBLE => return MOST_VISIBLE,
        code::CB_GETEDITSEL => return 0,
        _ => {}
    }
    let answer = with(|gui| {
        let address = address_of(gui, window)?;
        let sorted = gui.windows.get(window)?.style & model::style::SORT != 0;
        // SAFETY: as `model`.
        let combo = unsafe { model(address) };
        let count = combo.items.count();
        // SAFETY: these messages' `lParam` is a NUL-terminated string or 0.
        let text = |lparam: usize| unsafe { text_at(lparam) };
        Some(match number {
            code::CB_ADDSTRING => {
                let added = combo.items.add(text(lparam), sorted);
                (added.unwrap_or(CB_ERRSPACE), true)
            }
            code::CB_INSERTSTRING => {
                let at = match wparam as isize {
                    -1 => count,
                    at => at as usize,
                };
                match at <= count {
                    true => (
                        combo.items.insert(at, text(lparam)).unwrap_or(CB_ERRSPACE),
                        true,
                    ),
                    false => (CB_ERR, false),
                }
            }
            code::CB_DELETESTRING => (combo.items.delete(wparam).unwrap_or(CB_ERR), true),
            code::CB_RESETCONTENT => {
                combo.items.clear();
                (0, true)
            }
            code::CB_GETCOUNT => (count, false),
            code::CB_GETCURSEL => (combo.selected().unwrap_or(CB_ERR), false),
            code::CB_SETCURSEL => {
                let wanted = match wparam as isize {
                    -1 => None,
                    at => Some(at as usize),
                };
                let valid = combo.select(wanted);
                match (valid, wanted) {
                    (true, Some(index)) => (index, true),
                    _ => (CB_ERR, true),
                }
            }
            code::CB_GETLBTEXT => match combo.items.text(wparam) {
                Some(item) if lparam != 0 => {
                    // SAFETY: `CB_GETLBTEXT`'s contract: `lParam` has room for the item and a NUL.
                    unsafe {
                        let out =
                            core::slice::from_raw_parts_mut(lparam as *mut u16, item.len() + 1);
                        out[..item.len()].copy_from_slice(item);
                        out[item.len()] = 0;
                    }
                    (item.len(), false)
                }
                _ => (CB_ERR, false),
            },
            code::CB_GETLBTEXTLEN => (combo.items.text(wparam).map_or(CB_ERR, <[u16]>::len), false),
            code::CB_FINDSTRING | code::CB_FINDSTRINGEXACT => {
                let exact = number == code::CB_FINDSTRINGEXACT;
                (
                    find(combo, wparam, text(lparam), exact).unwrap_or(CB_ERR),
                    false,
                )
            }
            code::CB_SELECTSTRING => match find(combo, wparam, text(lparam), false) {
                Some(index) => {
                    combo.select(Some(index));
                    (index, true)
                }
                None => (CB_ERR, false),
            },
            code::CB_SETMINVISIBLE => (1, false),
            _ => (CB_ERR, false),
        })
    })
    .flatten();
    let (result, changed) = answer.unwrap_or((CB_ERR, false));
    if changed {
        invalidate(window);
    }
    result
}

/// The window procedure of every combo box.
pub fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::CREATE => 0,
        message::PAINT => {
            paint_box(window);
            0
        }
        message::ERASE_BACKGROUND => 1,
        message::SET_FONT | message::GET_FONT => 0,
        message::ENABLE => {
            if wparam == 0 {
                close_and_notify(window, false);
            }
            invalidate(window);
            0
        }
        message::SET_FOCUS => {
            invalidate(window);
            notify(window, code::CBN_SETFOCUS);
            0
        }
        message::KILL_FOCUS => {
            close_and_notify(window, false);
            invalidate(window);
            notify(window, code::CBN_KILLFOCUS);
            0
        }
        message::LEFT_BUTTON_DOWN => {
            left_button_down(window);
            0
        }
        message::KEY_DOWN => {
            key_down(window, wparam as u32);
            0
        }
        message::SYS_KEY_DOWN if matches!(wparam as u32, key::UP | key::DOWN) => {
            toggle(window);
            0
        }
        message::CHAR => {
            character(window, wparam);
            0
        }
        LIST_CLOSED => {
            send_closed_notifications(window, wparam);
            0
        }
        message::GET_TEXT => {
            let mut copy = [0u16; SHOWN_TEXT_CAPACITY];
            let length = with(|gui| {
                let text = shown_text(gui, window);
                let length = text.len().min(copy.len());
                copy[..length].copy_from_slice(&text[..length]);
                length
            })
            .unwrap_or(0);
            // SAFETY: `WM_GETTEXT`'s contract: `lParam` holds `wParam` writable units.
            unsafe { window_text::copy_out(&copy[..length], lparam, wparam) }
        }
        message::GET_TEXT_LENGTH => with(|gui| shown_text(gui, window).len()).unwrap_or(0),
        number if (code::CB_GETEDITSEL..=code::CB_LAST).contains(&number) => {
            combo_message(window, number, wparam, lparam)
        }
        code::CB_SETMINVISIBLE | code::CB_GETMINVISIBLE => {
            combo_message(window, number, wparam, lparam)
        }
        _ => def_window_proc_w(window, number, wparam, lparam),
    }
}
