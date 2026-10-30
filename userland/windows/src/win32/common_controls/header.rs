//! `SysHeader32` (D178, `docs/ux/windows-controls-v0.3.md` section 24): the header a list view
//! makes for its columns at `WM_CREATE`, and a header window a program makes itself. A list's
//! header shows and changes that list's columns, which the list keeps (`list_view.rs`), so a
//! width set through either is seen through both, and it tells the list of a click with
//! `HDN_ITEMCLICKW`; a program's own header keeps its items itself and tells its parent.

use super::super::controls::point;
use super::super::dialog::{release_capture, set_capture, set_focus};
use super::super::gui::{Gui, with};
use super::super::window_text;
use super::list_view::{self, Win32Rect, text_at, write_rect};
use super::{
    Model, NotifyHeader, allocate, default_procedure, invalidate, item_element, notify_parent,
    paint_with, read_state, utf8, with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, Window};
use eggos_element_tree::{Action, Bounds, Element, MAX_LABEL_BYTES, Outcome, Role, TreeWriter};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::common::COMMON_LIGHT;
use eggos_windows_gui::controls::list_view::{
    Columns, HEADER_HEIGHT, HeaderLook, header_item_at, header_item_rect, paint_header,
};
use eggos_windows_gui::surface::Rect;

const HDM_FIRST: u32 = 0x1200;
const HDM_LAST: u32 = 0x12ff;
const HDM_GETITEMCOUNT: u32 = 0x1200;
const HDM_DELETEITEM: u32 = 0x1202;
const HDM_LAYOUT: u32 = 0x1205;
const HDM_HITTEST: u32 = 0x1206;
const HDM_GETITEMRECT: u32 = 0x1207;
const HDM_INSERTITEMW: u32 = 0x120a;
const HDM_GETITEMW: u32 = 0x120b;
const HDM_SETITEMW: u32 = 0x120c;
const HDM_ORDERTOINDEX: u32 = 0x120f;
const HDM_GETORDERARRAY: u32 = 0x1211;
const HDM_SETORDERARRAY: u32 = 0x1212;
const CCM_GETUNICODEFORMAT: u32 = 0x2006;
const ACT_ON_COLUMN: u32 = 0x03f1;
const HDN_ITEMCLICKW: i32 = -322;
const HDI_WIDTH: u32 = 0x1;
const HDI_TEXT: u32 = 0x2;
const HDI_FORMAT: u32 = 0x4;
const HDI_LPARAM: u32 = 0x8;
const HHT_NOWHERE: u32 = 0x1;
const HHT_ONHEADER: u32 = 0x2;
const HHT_ABOVE: u32 = 0x100;
const HHT_BELOW: u32 = 0x200;
const HHT_TORIGHT: u32 = 0x400;
const HHT_TOLEFT: u32 = 0x800;
const HDS_BUTTONS: u32 = 0x2;
const FAILED: usize = usize::MAX;

/// The first version of Win32's `HDITEMW`, every field this layer reads or writes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HeaderItem {
    mask: u32,
    width: i32,
    text: usize,
    bitmap: usize,
    text_max: i32,
    format: u32,
    lparam: usize,
}

/// Win32's `NMHEADERW`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NmHeader {
    header: NotifyHeader,
    item: i32,
    button: i32,
    item_data: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HitTestInfo {
    x: i32,
    y: i32,
    flags: u32,
    item: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct LayoutRequest {
    rect: usize,
    position: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct WindowPosition {
    window: usize,
    after: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: u32,
}

struct HeaderState {
    list: usize,
    columns: Columns,
    pressed: Option<usize>,
    inside: bool,
}

impl Model for HeaderState {
    const KIND: CommonKind = CommonKind::Header;
}

/// A new header's state, its own items empty; `None` without memory.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(HeaderState {
        list: 0,
        columns: Columns::new(),
        pressed: None,
        inside: false,
    })
}

/// Makes `header` list view `list`'s: it shows and changes that list's columns.
pub(super) fn adopt(header: usize, list: usize) {
    with_state::<HeaderState, _>(header, |_, control| control.list = list);
}

/// A header's role in agents' tree: a group of buttons.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::Group, 0)
}

fn owner(window: usize) -> usize {
    with_state::<HeaderState, _>(window, |_, control| control.list).unwrap_or(0)
}

fn view<R>(gui: &Gui, window: usize, work: impl FnOnce(&Columns, i32, bool) -> R) -> Option<R> {
    let list = read_state::<HeaderState, _>(gui, window, |_, control| control.list)?;
    match list {
        0 => read_state::<HeaderState, R>(gui, window, |entry, control| {
            work(&control.columns, 0, entry.style & HDS_BUTTONS != 0)
        }),
        list => list_view::header_view(gui, list, work),
    }
}

fn read<R>(window: usize, work: impl FnOnce(&Columns, i32, bool) -> R) -> Option<R> {
    with(|gui| view(gui, window, work)).flatten()
}

fn change<R>(window: usize, work: impl FnOnce(&mut Columns) -> R) -> Option<R> {
    match owner(window) {
        0 => {
            let result =
                with_state::<HeaderState, _>(window, |_, control| work(&mut control.columns));
            invalidate(window);
            result
        }
        list => list_view::with_columns(list, work),
    }
}

fn paint(window: usize) {
    paint_with::<HeaderState>(window, |surface, rect, _, control, gui| {
        let enabled = windows::enabled_in_tree(&gui.windows, window);
        let pressed = control.pressed.filter(|_| control.inside);
        let of_list = control.list != 0;
        view(gui, window, |columns, hpos, buttons| {
            let look = HeaderLook {
                pressed,
                hot: None,
                buttons,
                enabled,
                of_list,
            };
            paint_header(surface, rect, columns, hpos, look, &LIGHT, &COMMON_LIGHT);
        });
    });
}

fn client_of(window: usize) -> Rect {
    with(|gui| gui.windows.get(window).map(Window::client))
        .flatten()
        .unwrap_or_default()
}

fn item_at(window: usize, x: i32) -> Option<(Option<usize>, bool)> {
    read(window, |columns, hpos, buttons| {
        (header_item_at(columns, hpos, x), buttons)
    })
}

fn click(window: usize, item: usize) {
    notify_parent(window, HDN_ITEMCLICKW, |header| NmHeader {
        header,
        item: item as i32,
        button: 0,
        item_data: 0,
    });
}

fn button_down(window: usize, (x, _): (i32, i32)) {
    let list = owner(window);
    if list != 0 {
        set_focus(list);
    }
    let Some((Some(item), true)) = item_at(window, x) else {
        return;
    };
    with_state::<HeaderState, _>(window, |_, control| {
        control.pressed = Some(item);
        control.inside = true;
    });
    set_capture(window);
    invalidate(window);
}

fn over_item(window: usize, (x, y): (i32, i32), item: usize) -> bool {
    client_of(window).contains(x, y)
        && item_at(window, x).and_then(|(found, _)| found) == Some(item)
}

fn pointer_moved(window: usize, point: (i32, i32)) {
    let Some(pressed) =
        with_state::<HeaderState, _>(window, |_, control| control.pressed).flatten()
    else {
        return;
    };
    let inside = over_item(window, point, pressed);
    let changed = with_state::<HeaderState, _>(window, |_, control| {
        core::mem::replace(&mut control.inside, inside) != inside
    })
    .unwrap_or(false);
    if changed {
        invalidate(window);
    }
}

fn button_up(window: usize, point: (i32, i32)) {
    let Some(pressed) =
        with_state::<HeaderState, _>(window, |_, control| control.pressed.take()).flatten()
    else {
        return;
    };
    release_capture();
    invalidate(window);
    if over_item(window, point, pressed) {
        click(window, pressed);
    }
}

fn insert_item(window: usize, at: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return FAILED;
    }
    // SAFETY: `HDM_INSERTITEMW`'s `lParam` is the program's `HDITEMW`.
    let item = unsafe { (lparam as *const HeaderItem).read_unaligned() };
    let text = match item.mask & HDI_TEXT {
        0 => &[][..],
        // SAFETY: with `HDI_TEXT`, `pszText` is a NUL-terminated string.
        _ => unsafe { text_at(item.text) },
    };
    let width = match item.mask & HDI_WIDTH {
        0 => 0,
        _ => item.width,
    };
    let alignment = match item.mask & HDI_FORMAT {
        0 => 0,
        _ => item.format,
    };
    let data = match item.mask & HDI_LPARAM {
        0 => 0,
        _ => item.lparam,
    };
    let inserted = match owner(window) {
        0 => {
            let inserted = with_state::<HeaderState, _>(window, |_, control| {
                control.columns.insert(at, text, width, alignment, data)
            })
            .flatten();
            invalidate(window);
            inserted
        }
        list => list_view::insert_column(list, at, text, width, alignment, data),
    };
    inserted.unwrap_or(FAILED)
}

fn delete_item(window: usize, index: usize) -> usize {
    let deleted = match owner(window) {
        0 => {
            let deleted =
                with_state::<HeaderState, _>(window, |_, control| control.columns.delete(index))
                    .unwrap_or(false);
            invalidate(window);
            deleted
        }
        list => list_view::delete_column(list, index),
    };
    usize::from(deleted)
}

fn set_item(window: usize, index: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `HDM_SETITEMW`'s `lParam` is the program's `HDITEMW`.
    let item = unsafe { (lparam as *const HeaderItem).read_unaligned() };
    let text = match item.mask & HDI_TEXT {
        0 => &[][..],
        // SAFETY: with `HDI_TEXT`, `pszText` is a NUL-terminated string.
        _ => unsafe { text_at(item.text) },
    };
    let done = change(window, |columns| {
        if index >= columns.count() {
            return false;
        }
        let mut done = true;
        if item.mask & HDI_TEXT != 0 {
            done &= columns.set_text(index, text);
        }
        if item.mask & HDI_WIDTH != 0 {
            columns.set_width(index, item.width);
        }
        if item.mask & HDI_FORMAT != 0 {
            columns.set_format(index, item.format);
        }
        if item.mask & HDI_LPARAM != 0 {
            columns.set_lparam(index, item.lparam);
        }
        done
    });
    usize::from(done == Some(true))
}

fn get_item(window: usize, index: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `HDM_GETITEMW`'s `lParam` is the program's `HDITEMW`.
    let item = unsafe { (lparam as *const HeaderItem).read_unaligned() };
    let target = lparam as *mut HeaderItem;
    read(window, |columns, _, _| {
        if index >= columns.count() {
            return 0;
        }
        // SAFETY: `target` is the program's `HDITEMW`, as read above; `pszText` holds
        // `cchTextMax` writable units when `HDI_TEXT` asks for the text.
        unsafe {
            if item.mask & HDI_WIDTH != 0 {
                (&raw mut (*target).width).write_unaligned(columns.width(index));
            }
            if item.mask & HDI_FORMAT != 0 {
                (&raw mut (*target).format).write_unaligned(columns.format(index));
            }
            if item.mask & HDI_LPARAM != 0 {
                (&raw mut (*target).lparam).write_unaligned(columns.lparam(index));
            }
            if item.mask & HDI_TEXT != 0 {
                window_text::copy_out(
                    columns.text(index),
                    item.text,
                    item.text_max.max(0) as usize,
                );
            }
        }
        1
    })
    .unwrap_or(0)
}

fn item_rect(window: usize, index: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    let client = client_of(window);
    let rect = read(window, |columns, hpos, _| {
        (index < columns.count()).then(|| header_item_rect(columns, client, hpos, index))
    })
    .flatten();
    match rect {
        Some(rect) => {
            // SAFETY: `HDM_GETITEMRECT`'s `lParam` is the program's `RECT`.
            unsafe { write_rect(lparam, rect) };
            1
        }
        None => 0,
    }
}

fn hit_test(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return FAILED;
    }
    let info = lparam as *mut HitTestInfo;
    // SAFETY: `HDM_HITTEST`'s `lParam` is the program's `HDHITTESTINFO`, its point set.
    let (x, y) = unsafe {
        (
            (&raw const (*info).x).read_unaligned(),
            (&raw const (*info).y).read_unaligned(),
        )
    };
    let client = client_of(window);
    let (found, item) = match () {
        () if y < client.top => (HHT_ABOVE, -1),
        () if y >= client.bottom => (HHT_BELOW, -1),
        () if x < client.left => (HHT_TOLEFT, -1),
        () if x >= client.right => (HHT_TORIGHT, -1),
        () => match item_at(window, x) {
            Some((Some(item), _)) => (HHT_ONHEADER, item as i32),
            _ => (HHT_NOWHERE, -1),
        },
    };
    // SAFETY: as above.
    unsafe {
        (&raw mut (*info).flags).write_unaligned(found);
        (&raw mut (*info).item).write_unaligned(item);
    }
    item as isize as usize
}

fn layout(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `HDM_LAYOUT`'s `lParam` is the program's `HDLAYOUT`: a `RECT` to place the
    // header in, shrunk by it, and the `WINDOWPOS` to fill.
    unsafe {
        let request = (lparam as *const LayoutRequest).read_unaligned();
        if request.rect == 0 || request.position == 0 {
            return 0;
        }
        let rect = request.rect as *mut Win32Rect;
        let mut room = rect.read_unaligned();
        let position = WindowPosition {
            window,
            after: 0,
            x: room.left,
            y: room.top,
            width: room.right - room.left,
            height: HEADER_HEIGHT,
            flags: 0,
        };
        (request.position as *mut WindowPosition).write_unaligned(position);
        room.top += HEADER_HEIGHT;
        rect.write_unaligned(room);
    }
    1
}

fn header_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        HDM_GETITEMCOUNT => read(window, |columns, _, _| columns.count()).unwrap_or(0),
        HDM_INSERTITEMW => insert_item(window, wparam, lparam),
        HDM_DELETEITEM => delete_item(window, wparam),
        HDM_GETITEMW => get_item(window, wparam, lparam),
        HDM_SETITEMW => set_item(window, wparam, lparam),
        HDM_GETITEMRECT => item_rect(window, wparam, lparam),
        HDM_HITTEST => hit_test(window, lparam),
        HDM_LAYOUT => layout(window, lparam),
        HDM_ORDERTOINDEX => wparam,
        HDM_GETORDERARRAY => {
            let count = read(window, |columns, _, _| columns.count()).unwrap_or(0);
            if lparam == 0 || wparam < count {
                return 0;
            }
            // SAFETY: `HDM_GETORDERARRAY`'s `lParam` holds `wParam` writable `int`s.
            let order = unsafe { core::slice::from_raw_parts_mut(lparam as *mut i32, count) };
            for (index, slot) in order.iter_mut().enumerate() {
                *slot = index as i32;
            }
            1
        }
        HDM_SETORDERARRAY => 0,
        _ => 0,
    }
}

/// Pushes header `window`'s items (section 29) under its element `element`: a `Button` per
/// column (a `Label` when its items are not buttons), named by its title.
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    let Some(client) = gui.windows.get(window).map(Window::client) else {
        return true;
    };
    view(gui, window, |columns, hpos, buttons| {
        let role = match buttons {
            true => Role::Button,
            false => Role::Label,
        };
        for index in 0..columns.count() {
            let rect = header_item_rect(columns, client, hpos, index).intersection(&client);
            let bounds = Bounds::new(
                origin.0 + rect.left,
                origin.1 + rect.top,
                rect.width(),
                rect.height(),
            );
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(columns.text(index), &mut name);
            let item = Element::new(item_element(window, index), element, role, bounds).named(name);
            if !tree.push(&item) {
                return false;
            }
        }
        true
    })
    .unwrap_or(true)
}

/// Does an agent's `action` on column `item` (section 29): `invoke` is a click on its button,
/// which a list view turns into `LVN_COLUMNCLICK`.
pub(super) fn act(window: usize, item: Option<usize>, action: Action, ours: bool) -> Outcome {
    let Some(item) = item else {
        return Outcome::NotSupported;
    };
    if action != Action::Invoke {
        return Outcome::NotSupported;
    }
    match read(window, |columns, _, buttons| {
        (item < columns.count(), buttons)
    }) {
        Some((true, true)) if ours => {
            click(window, item);
            Outcome::Done
        }
        Some((true, true)) => list_view::hand_over(window, ACT_ON_COLUMN, item, action),
        Some((true, false)) => Outcome::NotSupported,
        _ => Outcome::NoSuchElement,
    }
}

/// The window procedure of every header.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::CREATE => 0,
        message::PAINT => {
            paint(window);
            0
        }
        message::SIZE | message::ENABLE => {
            invalidate(window);
            0
        }
        message::LEFT_BUTTON_DOWN | message::LEFT_BUTTON_DOUBLE_CLICK => {
            button_down(window, point(lparam));
            0
        }
        message::MOUSE_MOVE => {
            pointer_moved(window, point(lparam));
            0
        }
        message::LEFT_BUTTON_UP => {
            button_up(window, point(lparam));
            0
        }
        ACT_ON_COLUMN => {
            click(window, wparam);
            0
        }
        CCM_GETUNICODEFORMAT => 1,
        number if (HDM_FIRST..=HDM_LAST).contains(&number) => {
            header_message(window, number, wparam, lparam)
        }
        _ => default_procedure(window, number, wparam, lparam),
    }
}
