//! `ToolbarWindow32` (D178, design section 26): the layer's procedure over
//! `eggos_windows_gui`'s toolbar model, docked at its parent's top (or bottom) at creation, on
//! `WM_SIZE` and on `TB_AUTOSIZE`, answering the `TB_*` messages, drawing its buttons' images
//! from the layer's image lists, and sending the parent `WM_COMMAND` when a held button is
//! released over itself. It is never focused and takes no keys; its buttons are buttons in
//! agents' tree.

use super::super::dialog::{release_capture, set_capture};
use super::super::gui::{self, Delivery, Gui, with};
use super::super::user32::{Win32Rect, create_child};
use super::super::{FALSE, TRUE, file, thread_environment, window_text};
use super::image_list::{LayerImages, image_size};
use super::status_bar::dock;
use super::style::CCS_BOTTOM;
use super::{
    Model, allocate, command_parent, default_procedure, invalidate, item_element, read_state, utf8,
    with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, ControlKind, Window};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, Outcome, Role, TreeWriter, flags,
};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::common::COMMON_LIGHT;
use eggos_windows_gui::controls::list_view::RowDraw;
use eggos_windows_gui::controls::toolbar::{
    self, Button, ButtonText, Layout, Toolbar, ToolbarLook, state as button_state,
};
use eggos_windows_gui::surface::Rect;

const WM_USER: u32 = 0x0400;
const TB_ENABLEBUTTON: u32 = WM_USER + 1;
const TB_CHECKBUTTON: u32 = WM_USER + 2;
const TB_PRESSBUTTON: u32 = WM_USER + 3;
const TB_HIDEBUTTON: u32 = WM_USER + 4;
const TB_ISBUTTONENABLED: u32 = WM_USER + 9;
const TB_ISBUTTONCHECKED: u32 = WM_USER + 10;
const TB_ISBUTTONPRESSED: u32 = WM_USER + 11;
const TB_ISBUTTONHIDDEN: u32 = WM_USER + 12;
const TB_SETSTATE: u32 = WM_USER + 17;
const TB_GETSTATE: u32 = WM_USER + 18;
const TB_ADDBITMAP: u32 = WM_USER + 19;
const TB_DELETEBUTTON: u32 = WM_USER + 22;
const TB_GETBUTTON: u32 = WM_USER + 23;
const TB_BUTTONCOUNT: u32 = WM_USER + 24;
const TB_COMMANDTOINDEX: u32 = WM_USER + 25;
const TB_GETITEMRECT: u32 = WM_USER + 29;
const TB_BUTTONSTRUCTSIZE: u32 = WM_USER + 30;
const TB_SETBUTTONSIZE: u32 = WM_USER + 31;
const TB_SETBITMAPSIZE: u32 = WM_USER + 32;
const TB_AUTOSIZE: u32 = WM_USER + 33;
const TB_SETCMDID: u32 = WM_USER + 42;
const TB_SETIMAGELIST: u32 = WM_USER + 48;
const TB_GETIMAGELIST: u32 = WM_USER + 49;
const TB_GETRECT: u32 = WM_USER + 51;
const TB_SETHOTIMAGELIST: u32 = WM_USER + 52;
const TB_GETHOTIMAGELIST: u32 = WM_USER + 53;
const TB_SETDISABLEDIMAGELIST: u32 = WM_USER + 54;
const TB_GETDISABLEDIMAGELIST: u32 = WM_USER + 55;
const TB_SETSTYLE: u32 = WM_USER + 56;
const TB_GETSTYLE: u32 = WM_USER + 57;
const TB_GETBUTTONSIZE: u32 = WM_USER + 58;
const TB_SETMAXTEXTROWS: u32 = WM_USER + 60;
const TB_GETTEXTROWS: u32 = WM_USER + 61;
const TB_GETBUTTONINFOW: u32 = WM_USER + 63;
const TB_SETBUTTONINFOW: u32 = WM_USER + 64;
const TB_INSERTBUTTONW: u32 = WM_USER + 67;
const TB_ADDBUTTONSW: u32 = WM_USER + 68;
const TB_HITTEST: u32 = WM_USER + 69;
const TB_GETHOTITEM: u32 = WM_USER + 71;
const TB_GETBUTTONTEXTW: u32 = WM_USER + 75;
const TB_ADDSTRINGW: u32 = WM_USER + 77;
const TB_GETMAXSIZE: u32 = WM_USER + 83;
const TB_SETEXTENDEDSTYLE: u32 = WM_USER + 84;
const TB_GETEXTENDEDSTYLE: u32 = WM_USER + 85;
const TB_GETPADDING: u32 = WM_USER + 86;
const TB_SETPADDING: u32 = WM_USER + 87;
const TB_GETSTRINGW: u32 = WM_USER + 91;
const TB_LAST: u32 = WM_USER + 92;
const TB_GETTOOLTIPS: u32 = WM_USER + 35;
const TB_SETTOOLTIPS: u32 = WM_USER + 36;
const TBSTYLE_TOOLTIPS: u32 = 0x0100;
const TBN_GETINFOTIPW: i32 = -719;
const TTN_GETDISPINFOW: u32 = -530i32 as u32;
const INFO_TIP_UNITS: usize = 80;
const WS_CHILD: u32 = 0x4000_0000;
const TBIF_IMAGE: u32 = 0x1;
const TBIF_TEXT: u32 = 0x2;
const TBIF_STATE: u32 = 0x4;
const TBIF_STYLE: u32 = 0x8;
const TBIF_LPARAM: u32 = 0x10;
const TBIF_COMMAND: u32 = 0x20;
const TBIF_SIZE: u32 = 0x40;
const TBIF_BYINDEX: u32 = 0x8000_0000;
const CCS_ALIGN: u32 = 0x3;
const OWN_STYLES: u32 = 0xffff;
const STRING_INDICES: isize = 0x1_0000;
const NONE: usize = usize::MAX;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct TbButton {
    image: i32,
    command: i32,
    state: u8,
    style: u8,
    reserved: [u8; 6],
    data: usize,
    string: isize,
}

#[repr(C)]
struct TbButtonInfo {
    size: u32,
    mask: u32,
    command: i32,
    image: i32,
    state: u8,
    style: u8,
    width: u16,
    data: usize,
    text: *mut u16,
    text_capacity: i32,
}

#[repr(C)]
struct Size {
    width: i32,
    height: i32,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

impl Model for Toolbar {
    const KIND: CommonKind = CommonKind::Toolbar;
}

/// A new toolbar's state.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(Toolbar::new())
}

fn layout_of(model: &Toolbar, style: u32) -> Layout {
    model.layout(style, image_size(model.image_list))
}

fn dock_toolbar(window: usize) {
    let wanted = with_state::<Toolbar, _>(window, |entry, model| {
        let height = Toolbar::bar_height(&layout_of(model, entry.style));
        (height, entry.style & CCS_ALIGN == CCS_BOTTOM)
    });
    if let Some((height, bottom)) = wanted {
        dock(window, height, bottom);
    }
}

/// The procedure of every toolbar.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::CREATE => {
            dock_toolbar(window);
            make_tooltips(window);
            0
        }
        TB_GETTOOLTIPS => with_state::<Toolbar, _>(window, |_, model| model.tooltips).unwrap_or(0),
        TB_SETTOOLTIPS => {
            with_state::<Toolbar, _>(window, |_, model| model.tooltips = wparam);
            invalidate(window);
            0
        }
        super::WM_NOTIFY if is_from_tooltips(window, lparam) => {
            info_tip(window, lparam);
            0
        }
        message::SIZE => {
            dock_toolbar(window);
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
        message::LEFT_BUTTON_DOWN | message::LEFT_BUTTON_DOUBLE_CLICK => {
            left_button_down(window, super::super::controls::point(lparam));
            0
        }
        message::MOUSE_MOVE => {
            let (x, y) = super::super::controls::point(lparam);
            let moved = with_state::<Toolbar, _>(window, |entry, model| {
                let held = model.held()?;
                let rect = model.button_rect(held, entry.style, &layout_of(model, entry.style))?;
                Some(model.pointer_moved(rect.contains(x, y)))
            })
            .flatten();
            if moved == Some(true) {
                invalidate(window);
            }
            0
        }
        message::LEFT_BUTTON_UP => {
            left_button_up(window, super::super::controls::point(lparam));
            0
        }
        TB_AUTOSIZE => {
            dock_toolbar(window);
            invalidate(window);
            0
        }
        TB_ENABLEBUTTON..=TB_LAST => {
            let answer = with_state::<Toolbar, _>(window, |entry, model| {
                toolbar_message(entry, model, number, wparam, lparam)
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

fn left_button_down(window: usize, (x, y): (i32, i32)) {
    let enabled = with(|gui| windows::enabled_in_tree(&gui.windows, window)).unwrap_or(false);
    if !enabled {
        return;
    }
    let pressed = with_state::<Toolbar, _>(window, |entry, model| {
        let layout = layout_of(model, entry.style);
        let index = model.hit(x, y, entry.style, &layout)?;
        Some(model.press(index))
    })
    .flatten();
    if pressed == Some(true) {
        set_capture(window);
        invalidate(window);
    }
}

fn left_button_up(window: usize, (x, y): (i32, i32)) {
    let released = with_state::<Toolbar, _>(window, |entry, model| {
        let held = model.held()?;
        let layout = layout_of(model, entry.style);
        let inside = model
            .button_rect(held, entry.style, &layout)
            .is_some_and(|rect| rect.contains(x, y));
        Some(model.release(inside))
    })
    .flatten();
    let Some(command) = released else {
        return;
    };
    release_capture();
    invalidate(window);
    if let Some(command) = command {
        command_parent(window, command as u16 as usize);
    }
}

unsafe fn string_run<'a>(text: *const u16) -> &'a [u16] {
    let mut length = 0;
    loop {
        // SAFETY: the caller's duty: every unit up to the empty string's NUL is readable.
        let unit = unsafe { text.wrapping_add(length).read() };
        let previous = match length {
            0 => 0,
            // SAFETY: as above, for the unit before.
            _ => unsafe { text.wrapping_add(length - 1).read() },
        };
        if unit == 0 && (length == 0 || previous == 0) {
            break;
        }
        length += 1;
    }
    // SAFETY: as above.
    unsafe { core::slice::from_raw_parts(text, length) }
}

fn button_of(raw: &TbButton) -> (Button, &'static [u16]) {
    let (text, own) = match raw.string {
        string if (0..STRING_INDICES).contains(&string) => {
            (ButtonText::String(string as usize), &[][..])
        }
        string if string < 0 => (ButtonText::None, &[][..]),
        pointer => {
            // SAFETY: an `iString` that is not a string index is a NUL-terminated string.
            let own = unsafe { file::wide_string(pointer as *const u16) };
            (ButtonText::Own, own)
        }
    };
    let button = Button {
        image: raw.image,
        command: raw.command,
        state: raw.state,
        style: raw.style,
        data: raw.data,
        string: raw.string,
        text,
        width: 0,
    };
    (button, own)
}

fn add_buttons(model: &mut Toolbar, at: usize, count: usize, buttons: usize) -> bool {
    if buttons == 0 {
        return false;
    }
    let stride = model.struct_size.max(core::mem::size_of::<TbButton>());
    for offset in 0..count {
        // SAFETY: `TB_ADDBUTTONSW`'s and `TB_INSERTBUTTONW`'s contract: `count` readable
        // `TBBUTTON`s of the size `TB_BUTTONSTRUCTSIZE` gave.
        let raw = unsafe { ((buttons + offset * stride) as *const TbButton).read() };
        let (button, own) = button_of(&raw);
        if model
            .insert(at.saturating_add(offset), button, own)
            .is_none()
        {
            return false;
        }
    }
    true
}

fn write_rect(out: usize, rect: Rect) -> usize {
    if out == 0 {
        return FALSE as usize;
    }
    // SAFETY: the caller passes the program's writable `RECT`, checked not null.
    unsafe {
        (out as *mut Win32Rect).write(Win32Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    };
    TRUE as usize
}

fn signed(value: Option<usize>) -> usize {
    value.unwrap_or(NONE)
}

fn set_state_bit(model: &mut Toolbar, command: usize, bit: u8, on: bool) -> usize {
    let Some(index) = model.index_of(command as i32) else {
        return FALSE as usize;
    };
    if bit == button_state::CHECKED {
        model.set_checked(index, on);
    } else if let Some(button) = model.button_mut(index) {
        match on {
            true => button.state |= bit,
            false => button.state &= !bit,
        }
    }
    TRUE as usize
}

fn state_bit(model: &Toolbar, command: usize, bit: u8) -> usize {
    model
        .index_of(command as i32)
        .and_then(|index| model.button(index))
        .map_or(0, |button| usize::from(button.state & bit != 0))
}

fn toolbar_message(
    entry: &mut Window,
    model: &mut Toolbar,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> (usize, bool) {
    let style = entry.style;
    let low = (lparam & 0xffff) as i32;
    let high = ((lparam >> 16) & 0xffff) as i32;
    match number {
        TB_ENABLEBUTTON => (
            set_state_bit(model, wparam, button_state::ENABLED, low != 0),
            true,
        ),
        TB_CHECKBUTTON => (
            set_state_bit(model, wparam, button_state::CHECKED, low != 0),
            true,
        ),
        TB_PRESSBUTTON => (
            set_state_bit(model, wparam, button_state::PRESSED, low != 0),
            true,
        ),
        TB_HIDEBUTTON => (
            set_state_bit(model, wparam, button_state::HIDDEN, low != 0),
            true,
        ),
        TB_ISBUTTONENABLED => (state_bit(model, wparam, button_state::ENABLED), false),
        TB_ISBUTTONCHECKED => (state_bit(model, wparam, button_state::CHECKED), false),
        TB_ISBUTTONPRESSED => (state_bit(model, wparam, button_state::PRESSED), false),
        TB_ISBUTTONHIDDEN => (state_bit(model, wparam, button_state::HIDDEN), false),
        TB_GETSTATE => (
            signed(
                model
                    .index_of(wparam as i32)
                    .and_then(|index| model.button(index))
                    .map(|button| usize::from(button.state)),
            ),
            false,
        ),
        TB_SETSTATE => {
            let index = model.index_of(wparam as i32);
            match index.and_then(|index| model.button_mut(index)) {
                Some(button) => {
                    button.state = low as u8;
                    (TRUE as usize, true)
                }
                None => (FALSE as usize, false),
            }
        }
        TB_ADDBITMAP => {
            let first = model.bitmaps;
            let added = match lparam {
                0 => 0,
                _ => wparam as i32,
            };
            model.bitmaps = first.saturating_add(added);
            (first as usize, false)
        }
        TB_DELETEBUTTON => (usize::from(model.delete(wparam)), true),
        TB_GETBUTTON => match model.button(wparam) {
            Some(button) if lparam != 0 => {
                let raw = TbButton {
                    image: button.image,
                    command: button.command,
                    state: button.state,
                    style: button.style,
                    reserved: [0; 6],
                    data: button.data,
                    string: button.string,
                };
                // SAFETY: `TB_GETBUTTON`'s contract: `lParam` is a writable `TBBUTTON`.
                unsafe { (lparam as *mut TbButton).write(raw) };
                (TRUE as usize, false)
            }
            _ => (FALSE as usize, false),
        },
        TB_BUTTONCOUNT => (model.count(), false),
        TB_COMMANDTOINDEX => (signed(model.index_of(wparam as i32)), false),
        TB_GETITEMRECT => match model.button_rect(wparam, style, &layout_of(model, style)) {
            Some(rect) => (write_rect(lparam, rect), false),
            None => (FALSE as usize, false),
        },
        TB_GETRECT => {
            let layout = layout_of(model, style);
            match model
                .index_of(wparam as i32)
                .and_then(|index| model.button_rect(index, style, &layout))
            {
                Some(rect) => (write_rect(lparam, rect), false),
                None => (FALSE as usize, false),
            }
        }
        TB_BUTTONSTRUCTSIZE => {
            model.struct_size = wparam;
            (0, false)
        }
        TB_SETBUTTONSIZE => {
            model.button_size = (low, high);
            (TRUE as usize, true)
        }
        TB_SETBITMAPSIZE => {
            model.bitmap_size = (low, high);
            (TRUE as usize, true)
        }
        TB_SETCMDID => match model.button_mut(wparam) {
            Some(button) => {
                button.command = lparam as i32;
                (TRUE as usize, false)
            }
            None => (FALSE as usize, false),
        },
        TB_SETIMAGELIST => (core::mem::replace(&mut model.image_list, lparam), true),
        TB_GETIMAGELIST => (model.image_list, false),
        TB_SETHOTIMAGELIST => (core::mem::replace(&mut model.hot_list, lparam), false),
        TB_GETHOTIMAGELIST => (model.hot_list, false),
        TB_SETDISABLEDIMAGELIST => (core::mem::replace(&mut model.disabled_list, lparam), true),
        TB_GETDISABLEDIMAGELIST => (model.disabled_list, false),
        TB_SETSTYLE => {
            entry.style = (entry.style & !OWN_STYLES) | (lparam as u32 & OWN_STYLES);
            (0, true)
        }
        TB_GETSTYLE => ((entry.style & OWN_STYLES) as usize, false),
        TB_GETBUTTONSIZE => {
            let layout = layout_of(model, style);
            (
                message::word_pair(layout.uniform as usize, layout.height as usize),
                false,
            )
        }
        TB_SETMAXTEXTROWS => {
            model.max_text_rows = wparam as i32;
            (TRUE as usize, true)
        }
        TB_GETTEXTROWS => (model.max_text_rows as usize, false),
        TB_GETBUTTONINFOW => (get_button_info(model, style, wparam, lparam), false),
        TB_SETBUTTONINFOW => (set_button_info(model, wparam, lparam), true),
        TB_INSERTBUTTONW => {
            let at = match wparam as isize {
                at if at < 0 => model.count(),
                at => at as usize,
            };
            (usize::from(add_buttons(model, at, 1, lparam)), true)
        }
        TB_ADDBUTTONSW => (
            usize::from(add_buttons(model, model.count(), wparam, lparam)),
            true,
        ),
        TB_HITTEST if lparam != 0 => {
            // SAFETY: `TB_HITTEST`'s contract: `lParam` is a readable `POINT`.
            let point = unsafe { (lparam as *const Point).read() };
            let layout = layout_of(model, style);
            (signed(model.hit(point.x, point.y, style, &layout)), false)
        }
        TB_GETHOTITEM => (NONE, false),
        TB_GETBUTTONTEXTW => {
            let Some(index) = model.index_of(wparam as i32) else {
                return (NONE, false);
            };
            let text = model.text(index);
            if lparam != 0 {
                // SAFETY: `TB_GETBUTTONTEXTW`'s contract: `lParam` has room for the text and
                // its NUL.
                unsafe { window_text::copy_out(text, lparam, text.len() + 1) };
            }
            (text.len(), false)
        }
        TB_ADDSTRINGW => match (wparam, lparam) {
            (0, text) if text != 0 => {
                // SAFETY: `TB_ADDSTRINGW`'s contract with no instance: `lParam` is a run of
                // NUL-terminated strings ending with an empty one.
                let run = unsafe { string_run(text as *const u16) };
                (signed(model.add_strings(run)), true)
            }
            _ => (NONE, false),
        },
        TB_GETSTRINGW => {
            let (capacity, index) = (wparam & 0xffff, (wparam >> 16) & 0xffff);
            match model.string(index) {
                Some(text) => {
                    // SAFETY: `TB_GETSTRINGW`'s contract: `lParam` is 0 or holds
                    // `LOWORD(wParam)` writable units.
                    unsafe { window_text::copy_out(text, lparam, capacity) };
                    (text.len(), false)
                }
                None => (NONE, false),
            }
        }
        TB_GETMAXSIZE if lparam != 0 => {
            let layout = layout_of(model, style);
            let size = Size {
                width: model.total_width(style, &layout),
                height: layout.height,
            };
            // SAFETY: `TB_GETMAXSIZE`'s contract: `lParam` is a writable `SIZE`.
            unsafe { (lparam as *mut Size).write(size) };
            (TRUE as usize, false)
        }
        TB_SETEXTENDEDSTYLE => (
            core::mem::replace(&mut model.extended_style, lparam as u32) as usize,
            true,
        ),
        TB_GETEXTENDEDSTYLE => (model.extended_style as usize, false),
        TB_SETPADDING => {
            let previous = model.padding;
            model.padding = (low, high);
            (
                message::word_pair(previous.0 as usize, previous.1 as usize),
                false,
            )
        }
        TB_GETPADDING => (
            message::word_pair(model.padding.0 as usize, model.padding.1 as usize),
            false,
        ),
        _ => (0, false),
    }
}

fn info_index(model: &Toolbar, id: usize, mask: u32) -> Option<usize> {
    match mask & TBIF_BYINDEX {
        0 => model.index_of(id as i32),
        _ => (id < model.count()).then_some(id),
    }
}

fn get_button_info(model: &Toolbar, style: u32, id: usize, info: usize) -> usize {
    if info == 0 {
        return NONE;
    }
    // SAFETY: `TB_GETBUTTONINFOW`'s contract: `lParam` is a readable, writable `TBBUTTONINFOW`.
    let info = unsafe { &mut *(info as *mut TbButtonInfo) };
    let Some(index) = info_index(model, id, info.mask) else {
        return NONE;
    };
    let Some(button) = model.button(index) else {
        return NONE;
    };
    if info.mask & TBIF_COMMAND != 0 {
        info.command = button.command;
    }
    if info.mask & TBIF_IMAGE != 0 {
        info.image = button.image;
    }
    if info.mask & TBIF_STATE != 0 {
        info.state = button.state;
    }
    if info.mask & TBIF_STYLE != 0 {
        info.style = button.style;
    }
    if info.mask & TBIF_LPARAM != 0 {
        info.data = button.data;
    }
    if info.mask & TBIF_SIZE != 0 {
        info.width = model.width(index, style, &layout_of(model, style)) as u16;
    }
    if info.mask & TBIF_TEXT != 0 && !info.text.is_null() {
        let capacity = usize::try_from(info.text_capacity).unwrap_or(0);
        // SAFETY: `pszText` holds `cchText` writable units.
        unsafe { window_text::copy_out(model.text(index), info.text as usize, capacity) };
    }
    index
}

fn set_button_info(model: &mut Toolbar, id: usize, info: usize) -> usize {
    if info == 0 {
        return FALSE as usize;
    }
    // SAFETY: `TB_SETBUTTONINFOW`'s contract: `lParam` is a readable `TBBUTTONINFOW`.
    let info = unsafe { &*(info as *const TbButtonInfo) };
    let Some(index) = info_index(model, id, info.mask) else {
        return FALSE as usize;
    };
    let text = match (info.mask & TBIF_TEXT, info.text.is_null()) {
        (0, _) => None,
        (_, true) => Some(&[][..]),
        // SAFETY: `pszText` is a NUL-terminated string.
        (_, false) => Some(unsafe { file::wide_string(info.text) }),
    };
    let Some(button) = model.button_mut(index) else {
        return FALSE as usize;
    };
    if info.mask & TBIF_COMMAND != 0 {
        button.command = info.command;
    }
    if info.mask & TBIF_IMAGE != 0 {
        button.image = info.image;
    }
    if info.mask & TBIF_STATE != 0 {
        button.state = info.state;
    }
    if info.mask & TBIF_STYLE != 0 {
        button.style = info.style;
    }
    if info.mask & TBIF_LPARAM != 0 {
        button.data = info.data;
    }
    if info.mask & TBIF_SIZE != 0 {
        button.width = info.width;
    }
    if let Some(text) = text {
        model.set_text(index, text);
    }
    TRUE as usize
}

const NM_CUSTOMDRAW: i32 = -12;
const CDDS_PREPAINT: u32 = 0x1;
const CDDS_POSTPAINT: u32 = 0x2;
const CDDS_ITEMPREPAINT: u32 = 0x1_0001;
const CDDS_ITEMPOSTPAINT: u32 = 0x1_0002;
const CDRF_SKIPDEFAULT: usize = 0x4;
const CDRF_NOTIFYPOSTPAINT: usize = 0x10;
const CDRF_NOTIFYITEMDRAW: usize = 0x20;
const CLR_DEFAULT: u32 = 0xff00_0000;
const MOST_CUSTOM_BUTTONS: usize = 64;

#[repr(C)]
struct NmToolbarCustomDraw {
    header: super::NotifyHeader,
    stage: u32,
    hdc: usize,
    rect: [i32; 4],
    item: usize,
    item_state: u32,
    item_lparam: usize,
    mono_dither: usize,
    lines_brush: usize,
    lines_pen: usize,
    text: u32,
    mark: u32,
    text_highlight: u32,
    face: u32,
    face_highlight: u32,
    hot_track: u32,
    text_rect: [i32; 4],
    background_mode: i32,
    highlight_background_mode: i32,
    list_gap: i32,
}

#[derive(Clone, Copy, Default)]
struct ShownButton {
    index: usize,
    command: usize,
    rect: [i32; 4],
    data: usize,
}

struct CustomButtons {
    buttons: [ShownButton; MOST_CUSTOM_BUTTONS],
    draws: [RowDraw; MOST_CUSTOM_BUTTONS],
    after: [bool; MOST_CUSTOM_BUTTONS],
    count: usize,
    after_all: bool,
}

impl CustomButtons {
    fn draw(&self, index: usize) -> RowDraw {
        self.buttons[..self.count]
            .iter()
            .position(|button| button.index == index)
            .map_or_else(RowDraw::default, |at| self.draws[at])
    }
}

fn custom_draw(
    window: usize,
    stage: u32,
    hdc: usize,
    button: ShownButton,
) -> (usize, Option<NmToolbarCustomDraw>) {
    super::notify_parent_keeping(window, NM_CUSTOMDRAW, |header| NmToolbarCustomDraw {
        header,
        stage,
        hdc,
        rect: button.rect,
        item: button.command,
        item_state: 0,
        item_lparam: button.data,
        mono_dither: 0,
        lines_brush: 0,
        lines_pen: 0,
        text: CLR_DEFAULT,
        mark: CLR_DEFAULT,
        text_highlight: CLR_DEFAULT,
        face: CLR_DEFAULT,
        face_highlight: CLR_DEFAULT,
        hot_track: CLR_DEFAULT,
        text_rect: button.rect,
        background_mode: 0,
        highlight_background_mode: 0,
        list_gap: 0,
    })
}

fn chosen_colour(colour_ref: u32) -> Option<u32> {
    let swapped = (colour_ref & 0xff) << 16 | (colour_ref & 0xff00) | (colour_ref >> 16 & 0xff);
    (colour_ref & CLR_DEFAULT == 0).then_some(swapped)
}

fn custom_buttons(window: usize, hdc: usize) -> CustomButtons {
    let mut custom = CustomButtons {
        buttons: [ShownButton::default(); MOST_CUSTOM_BUTTONS],
        draws: [RowDraw::default(); MOST_CUSTOM_BUTTONS],
        after: [false; MOST_CUSTOM_BUTTONS],
        count: 0,
        after_all: false,
    };
    let found = with_state::<Toolbar, _>(window, |entry, model| {
        let client = entry.client();
        let layout = layout_of(model, entry.style);
        let mut count = 0;
        for index in 0..model.count() {
            let Some(button) = model.button(index).filter(|button| !button.is_separator()) else {
                continue;
            };
            let Some(rect) = model.button_rect(index, entry.style, &layout) else {
                continue;
            };
            if count < MOST_CUSTOM_BUTTONS {
                custom.buttons[count] = ShownButton {
                    index,
                    command: button.command as usize,
                    rect: [rect.left, rect.top, rect.right, rect.bottom],
                    data: button.data,
                };
                count += 1;
            }
        }
        (
            count,
            [client.left, client.top, client.right, client.bottom],
        )
    });
    let Some((count, whole)) = found else {
        return custom;
    };
    let everything = ShownButton {
        rect: whole,
        ..ShownButton::default()
    };
    let (answer, _) = custom_draw(window, CDDS_PREPAINT, hdc, everything);
    custom.after_all = answer & CDRF_NOTIFYPOSTPAINT != 0;
    if answer & CDRF_NOTIFYITEMDRAW == 0 {
        return custom;
    }
    custom.count = count;
    for at in 0..count {
        let (answer, data) = custom_draw(window, CDDS_ITEMPREPAINT, hdc, custom.buttons[at]);
        let Some(data) = data else {
            continue;
        };
        custom.draws[at] = RowDraw {
            text: chosen_colour(data.text),
            background: chosen_colour(data.face),
            skip: answer & CDRF_SKIPDEFAULT != 0,
        };
        custom.after[at] = answer & CDRF_NOTIFYPOSTPAINT != 0;
    }
    custom
}

fn paint(window: usize) {
    sync_tools(window);
    let Ok((hdc, _, _)) = super::super::paint::begin(window) else {
        return;
    };
    let custom = custom_buttons(window, hdc);
    let _ = super::super::paint::draw_in(hdc, |surface, _, gui| {
        let Some(surface) = surface else {
            return;
        };
        read_state::<Toolbar, ()>(gui, window, |entry, model| {
            let look = ToolbarLook {
                style: entry.style,
                enabled: windows::enabled_in_tree(&gui.windows, window),
            };
            toolbar::paint_custom(
                surface,
                entry.client(),
                model,
                look,
                (&LIGHT, &COMMON_LIGHT),
                (&LayerImages, &|index| custom.draw(index)),
            );
        });
    });
    for at in 0..custom.count {
        if custom.after[at] {
            custom_draw(window, CDDS_ITEMPOSTPAINT, hdc, custom.buttons[at]);
        }
    }
    if custom.after_all {
        custom_draw(window, CDDS_POSTPAINT, hdc, ShownButton::default());
    }
    super::super::paint::finish(hdc);
}

/// A toolbar's role in agents' tree.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::Group, 0)
}

/// Pushes a toolbar's shown buttons (not its separators), named by their texts.
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    let enabled = windows::enabled_in_tree(&gui.windows, window);
    read_state::<Toolbar, _>(gui, window, |entry, model| {
        let layout = layout_of(model, entry.style);
        for index in 0..model.count() {
            let Some(button) = model.button(index).filter(|button| !button.is_separator()) else {
                continue;
            };
            let Some(rect) = model.button_rect(index, entry.style, &layout) else {
                continue;
            };
            let mut state = 0;
            if button.is_checked() {
                state |= flags::CHECKED;
            }
            if !enabled || !button.is_enabled() {
                state |= flags::DISABLED;
            }
            let bounds = Bounds::new(
                origin.0 + rect.left,
                origin.1 + rect.top,
                rect.width(),
                rect.height(),
            );
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(model.text(index), &mut name);
            let item = Element::new(item_element(window, index), element, Role::Button, bounds)
                .named(name)
                .flagged(state);
            if !tree.push(&item) {
                return false;
            }
        }
        true
    })
    .unwrap_or(true)
}

/// `invoke` on a toolbar button clicks it: a check button toggles and the parent gets its
/// `WM_COMMAND`, on the parent's own thread.
pub(super) fn act(window: usize, item: Option<usize>, action: Action, ours: bool) -> Outcome {
    let Some(index) = item else {
        return Outcome::NotSupported;
    };
    if action != Action::Invoke {
        return Outcome::NotSupported;
    }
    let clicked = with_state::<Toolbar, _>(window, |_, model| {
        let button = model.button(index).copied();
        match button {
            Some(button) if !button.is_separator() && !button.is_hidden() => {
                model.click(index).ok_or(Outcome::Disabled)
            }
            _ => Err(Outcome::NoSuchElement),
        }
    })
    .unwrap_or(Err(Outcome::NoSuchElement));
    let command = match clicked {
        Ok(command) => command as u16 as usize,
        Err(outcome) => return outcome,
    };
    invalidate(window);
    if ours {
        command_parent(window, command);
        return Outcome::Done;
    }
    let parent = with(|gui| {
        let parent = gui.windows.get(window)?.parent;
        Some((parent, gui.windows.get(parent)?.thread))
    })
    .flatten();
    if let Some((parent, thread)) = parent {
        let delivery = Delivery {
            window: parent,
            thread,
            message: message::COMMAND,
            wparam: command,
            lparam: window,
        };
        gui::deliver(&delivery, thread_environment::thread_id());
    }
    Outcome::Done
}

fn make_tooltips(window: usize) {
    let wanted = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| entry.style & TBSTYLE_TOOLTIPS != 0)
    })
    .unwrap_or(false);
    if !wanted {
        return;
    }
    let kind = ControlKind::Common(CommonKind::Tooltip);
    let made = create_child(window, kind, WS_CHILD, Rect::default(), 0, &[]);
    if let Some(tooltips) = made {
        with_state::<Toolbar, _>(window, |_, model| model.tooltips = tooltips);
    }
}

fn sync_tools(window: usize) {
    let mut tools = [(0usize, Rect::default()); 64];
    let found = with_state::<Toolbar, _>(window, |entry, model| {
        let layout = layout_of(model, entry.style);
        let mut count = 0;
        for index in 0..model.count() {
            let Some(button) = model.button(index).filter(|button| !button.is_separator()) else {
                continue;
            };
            let Some(rect) = model.button_rect(index, entry.style, &layout) else {
                continue;
            };
            if count < tools.len() {
                tools[count] = (button.command as usize, rect);
                count += 1;
            }
        }
        (model.tooltips, count)
    });
    if let Some((tooltips, count)) = found.filter(|(tooltips, _)| *tooltips != 0) {
        super::tooltip::replace_tools(tooltips, window, &tools[..count]);
    }
}

fn is_from_tooltips(window: usize, lparam: usize) -> bool {
    if lparam == 0 {
        return false;
    }
    // SAFETY: `WM_NOTIFY`'s `lParam` is an `NMHDR` at least.
    let header = unsafe { (lparam as *const super::NotifyHeader).read_unaligned() };
    header.code == TTN_GETDISPINFOW
        && with_state::<Toolbar, _>(window, |_, model| model.tooltips == header.from)
            .unwrap_or(false)
}

#[repr(C)]
struct GetInfoTip {
    header: super::NotifyHeader,
    text: usize,
    room: i32,
    item: i32,
    lparam: usize,
}

fn info_tip(window: usize, lparam: usize) {
    let info = lparam as *mut super::tooltip::DisplayInfo;
    // SAFETY: the tooltip control's own `NMTTDISPINFOW`, alive for this call.
    let command = unsafe { (*info).header.id };
    let mut buffer = [0u16; INFO_TIP_UNITS];
    let data = with_state::<Toolbar, _>(window, |_, model| {
        (0..model.count())
            .filter_map(|index| model.button(index))
            .find(|button| button.command as usize == command)
            .map_or(0, |button| button.data)
    })
    .unwrap_or(0);
    super::notify_parent(window, TBN_GETINFOTIPW, |header| GetInfoTip {
        header,
        text: buffer.as_mut_ptr() as usize,
        room: INFO_TIP_UNITS as i32,
        item: command as i32,
        lparam: data,
    });
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(INFO_TIP_UNITS);
    let put = |units: &[u16]| {
        let length = units.len().min(INFO_TIP_UNITS - 1);
        // SAFETY: as above; its 80-unit buffer takes the text and a NUL.
        let tip = unsafe { &mut *info };
        tip.buffer[..length].copy_from_slice(&units[..length]);
        tip.buffer[length] = 0;
        tip.text = tip.buffer.as_ptr() as usize;
    };
    if length > 0 {
        put(&buffer[..length]);
        return;
    }
    let parent = with(|gui| gui.windows.get(window).map(|entry| entry.parent)).flatten();
    if let Some(parent) = parent.filter(|parent| *parent != 0) {
        gui::send(parent, super::WM_NOTIFY, command, lparam);
    }
    // SAFETY: as above.
    let answered =
        unsafe { (*info).text != (*info).buffer.as_ptr() as usize || (*info).buffer[0] != 0 };
    if answered {
        return;
    }
    let mut own = [0u16; INFO_TIP_UNITS];
    let own_length = with_state::<Toolbar, _>(window, |_, model| {
        if model.max_text_rows != 0 {
            return 0;
        }
        let index = (0..model.count()).find(|&index| {
            model
                .button(index)
                .is_some_and(|button| button.command as usize == command)
        });
        let text = index.map_or(&[][..], |index| model.text(index));
        let length = text.len().min(INFO_TIP_UNITS);
        own[..length].copy_from_slice(&text[..length]);
        length
    })
    .unwrap_or(0);
    put(&own[..own_length]);
}
