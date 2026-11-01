//! `SysListView32` (D178, `docs/ux/windows-controls-v0.3.md` section 24): the list view's
//! procedure over `eggos_windows_gui`'s [`ListView`] model. It creates its header, a real
//! `SysHeader32` child window (`header.rs`) that shows the list's columns, at `WM_CREATE`.
//! Every row's state change is a refusable `LVN_ITEMCHANGING` and an `LVN_ITEMCHANGED`, asked of
//! the model one row at a time and sent with the GUI lock let go; `LVM_SORTITEMS` copies the
//! rows' data out and calls the program's comparison function the same way. Its rows are
//! elements of agents' tree, which `select`, `invoke` and `focus` as clicks would.

use super::super::controls::point;
use super::super::dialog::{has_focus, release_capture, set_capture, set_focus};
use super::super::gui::{self, Delivery, Gui, with};
use super::super::user32::{create_child, now_ms};
use super::super::{call_program, file, thread_environment, window_text};
use super::{
    Model, NotifyHeader, allocate, code, default_procedure, image_list, invalidate, item_element,
    notify_parent, paint_with, read_state, utf8, with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, ControlKind, Window};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, Outcome, Role, TreeWriter, flags,
};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::common::{COMMON_LIGHT, plain_width_cells};
use eggos_windows_gui::controls::list_view::{
    self as model, CellPart, Columns, HEADER_HEIGHT, HORIZONTAL_LINE, ItemPart, ListHit,
    ListLayout, ListLook, ListView, MAX_ROWS, ROW_HEIGHT, Selection, StateChange, Wanted, extended,
    format, key, state, style,
};
use eggos_windows_gui::controls::scroll_bar::{self, Part};
use eggos_windows_gui::surface::Rect;
use eggos_windows_gui::text::CELL_WIDTH;

const LVM_FIRST: u32 = 0x1000;
const LVM_LAST: u32 = 0x10ff;
const LVM_GETBKCOLOR: u32 = 0x1000;
const LVM_SETBKCOLOR: u32 = 0x1001;
const LVM_GETIMAGELIST: u32 = 0x1002;
const LVM_SETIMAGELIST: u32 = 0x1003;
const LVM_GETITEMCOUNT: u32 = 0x1004;
const LVM_DELETEITEM: u32 = 0x1008;
const LVM_DELETEALLITEMS: u32 = 0x1009;
const LVM_GETNEXTITEM: u32 = 0x100c;
const LVM_GETITEMRECT: u32 = 0x100e;
const LVM_HITTEST: u32 = 0x1012;
const LVM_ENSUREVISIBLE: u32 = 0x1013;
const LVM_SCROLL: u32 = 0x1014;
const LVM_REDRAWITEMS: u32 = 0x1015;
const LVM_DELETECOLUMN: u32 = 0x101c;
const LVM_GETCOLUMNWIDTH: u32 = 0x101d;
const LVM_SETCOLUMNWIDTH: u32 = 0x101e;
const LVM_GETHEADER: u32 = 0x101f;
const LVM_GETTEXTCOLOR: u32 = 0x1023;
const LVM_SETTEXTCOLOR: u32 = 0x1024;
const LVM_GETTEXTBKCOLOR: u32 = 0x1025;
const LVM_SETTEXTBKCOLOR: u32 = 0x1026;
const LVM_GETTOPINDEX: u32 = 0x1027;
const LVM_GETCOUNTPERPAGE: u32 = 0x1028;
const LVM_UPDATE: u32 = 0x102a;
const LVM_SETITEMSTATE: u32 = 0x102b;
const LVM_GETITEMSTATE: u32 = 0x102c;
const LVM_SETITEMCOUNT: u32 = 0x102f;
const LVM_SORTITEMS: u32 = 0x1030;
const LVM_GETSELECTEDCOUNT: u32 = 0x1032;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
const LVM_GETEXTENDEDLISTVIEWSTYLE: u32 = 0x1037;
const LVM_GETSUBITEMRECT: u32 = 0x1038;
const LVM_SUBITEMHITTEST: u32 = 0x1039;
const LVM_SETCOLUMNORDERARRAY: u32 = 0x103a;
const LVM_GETCOLUMNORDERARRAY: u32 = 0x103b;
const LVM_GETSELECTIONMARK: u32 = 0x1042;
const LVM_SETSELECTIONMARK: u32 = 0x1043;
const LVM_GETITEMW: u32 = 0x104b;
const LVM_SETITEMW: u32 = 0x104c;
const LVM_INSERTITEMW: u32 = 0x104d;
const LVM_SORTITEMSEX: u32 = 0x1051;
const LVM_FINDITEMW: u32 = 0x1053;
const LVM_GETSTRINGWIDTHW: u32 = 0x1057;
const LVM_GETCOLUMNW: u32 = 0x105f;
const LVM_SETCOLUMNW: u32 = 0x1060;
const LVM_INSERTCOLUMNW: u32 = 0x1061;
const LVM_GETITEMTEXTW: u32 = 0x1073;
const LVM_SETITEMTEXTW: u32 = 0x1074;
const LVM_SETVIEW: u32 = 0x108e;
const LVM_GETVIEW: u32 = 0x108f;
const CCM_GETUNICODEFORMAT: u32 = 0x2006;
const WM_NOTIFY: u32 = super::WM_NOTIFY;
const WM_STYLECHANGED: u32 = 0x007d;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const ACT_ON_ROW: u32 = 0x03f0;

const LVN_ITEMCHANGING: i32 = -100;
const LVN_ITEMCHANGED: i32 = -101;
const LVN_INSERTITEM: i32 = -102;
const LVN_DELETEITEM: i32 = -103;
const LVN_DELETEALLITEMS: i32 = -104;
const LVN_COLUMNCLICK: i32 = -108;
const LVN_ITEMACTIVATE: i32 = -114;
const LVN_KEYDOWN: i32 = -155;
const HDN_ITEMCLICKW: i32 = -322;

const LVIF_TEXT: u32 = 0x1;
const LVIF_IMAGE: u32 = 0x2;
const LVIF_PARAM: u32 = 0x4;
const LVIF_STATE: u32 = 0x8;
const LVCF_FMT: u32 = 0x1;
const LVCF_WIDTH: u32 = 0x2;
const LVCF_TEXT: u32 = 0x4;
const LVCF_SUBITEM: u32 = 0x8;
const LVNI_FOCUSED: usize = 0x1;
const LVNI_SELECTED: usize = 0x2;
const LVNI_ABOVE: usize = 0x100;
const LVIR_ICON: i32 = 1;
const LVIR_LABEL: i32 = 2;
const LVIR_SELECTBOUNDS: i32 = 3;
const LVHT_NOWHERE: u32 = 0x1;
const LVHT_ONITEMICON: u32 = 0x2;
const LVHT_ONITEMLABEL: u32 = 0x4;
const LVHT_ONITEMSTATEICON: u32 = 0x8;
const LVHT_ABOVE: u32 = 0x8;
const LVHT_BELOW: u32 = 0x10;
const LVHT_TORIGHT: u32 = 0x20;
const LVHT_TOLEFT: u32 = 0x40;
const LVSIL_NORMAL: usize = 0;
const LVSIL_SMALL: usize = 1;
const LVSIL_STATE: usize = 2;
const LVSCW_AUTOSIZE: i32 = -1;
const LVSCW_AUTOSIZE_USEHEADER: i32 = -2;
const LVFI_PARAM: u32 = 0x1;
const LVFI_PARTIAL: u32 = 0x8;
const LVFI_WRAP: u32 = 0x20;
const LVKF_CONTROL: u32 = 0x2;
const LVKF_SHIFT: u32 = 0x4;
const CLR_NONE: u32 = 0xffff_ffff;
const CLR_DEFAULT: u32 = 0xff00_0000;
const DEFAULT_BACKGROUND: u32 = 0x00ff_ffff;
const DEFAULT_TEXT: u32 = 0;
const TEXT_CALLBACK: usize = usize::MAX;
const FAILED: usize = usize::MAX;
const MK_SHIFT: usize = 0x4;
const MK_CONTROL: usize = 0x8;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const HELD: u8 = 0x80;
const SB_LINEUP: usize = 0;
const SB_LINEDOWN: usize = 1;
const SB_PAGEUP: usize = 2;
const SB_PAGEDOWN: usize = 3;
const SB_THUMBPOSITION: usize = 4;
const SB_THUMBTRACK: usize = 5;
const SB_TOP: usize = 6;
const SB_BOTTOM: usize = 7;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const HDS_BUTTONS: u32 = 0x2;
const DOUBLE_CLICK_MS: u32 = 400;
const DOUBLE_CLICK_DISTANCE: i32 = 4;
const JOINED_UNITS: usize = 512;
const TAB: u16 = 0x09;
const LAST_PRINTABLE_CONTROL: u32 = 0x20;
const DELETE_CHARACTER: u32 = 0x7f;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ItemHead {
    mask: u32,
    item: i32,
    sub_item: i32,
    state: u32,
    state_mask: u32,
    text: usize,
    text_max: i32,
    image: i32,
    lparam: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ColumnHead {
    mask: u32,
    format: u32,
    width: i32,
    text: usize,
    text_max: i32,
    sub_item: i32,
}

/// Win32's `RECT`, as programs pass it.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct Win32Rect {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HitTestInfo {
    x: i32,
    y: i32,
    flags: u32,
    item: i32,
    sub_item: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FindInfo {
    flags: u32,
    text: usize,
    lparam: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NmListView {
    header: NotifyHeader,
    item: i32,
    sub_item: i32,
    new_state: u32,
    old_state: u32,
    changed: u32,
    x: i32,
    y: i32,
    lparam: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NmItemActivate {
    header: NotifyHeader,
    item: i32,
    sub_item: i32,
    new_state: u32,
    old_state: u32,
    changed: u32,
    x: i32,
    y: i32,
    lparam: usize,
    key_flags: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct NmListKeyDown {
    header: NotifyHeader,
    key: u16,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HeaderNotification {
    header: NotifyHeader,
    item: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    vertical: bool,
    grab: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gesture {
    Move,
    Click,
}

struct ListState {
    list: ListView,
    header: usize,
    pressed: Option<(i32, i32)>,
    last_down: Option<(u32, i32, i32, usize)>,
    drag: Option<Drag>,
}

impl Model for ListState {
    const KIND: CommonKind = CommonKind::ListView;
}

/// A new list view's state; `None` without memory.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(ListState {
        list: ListView::new(),
        header: 0,
        pressed: None,
        last_down: None,
        drag: None,
    })
}

/// A list view's role in agents' tree: a focusable list.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::List, flags::FOCUSABLE)
}

fn single(entry: &Window) -> bool {
    entry.style & style::SINGLE_SELECTION != 0
}

fn layout_of(entry: &Window, list: &ListView) -> ListLayout {
    list.layout(entry.client(), entry.style)
}

fn index_parameter(value: usize) -> Option<usize> {
    match value as u32 as i32 {
        index if index < 0 => None,
        index => Some(index as usize),
    }
}

fn swap_red_blue(colour: u32) -> u32 {
    ((colour & 0xff) << 16) | (colour & 0xff00) | ((colour >> 16) & 0xff)
}

fn colour_parameter(value: usize) -> Option<u32> {
    match value as u32 {
        CLR_NONE | CLR_DEFAULT => None,
        colour => Some(swap_red_blue(colour)),
    }
}

fn refresh(window: usize) {
    with(|gui| {
        let (header, bounds) = read_state::<ListState, _>(gui, window, |entry, control| {
            (control.header, layout_of(entry, &control.list).header)
        })?;
        windows::invalidate_tree(&mut gui.windows, window, None, true);
        let entry = gui.windows.get_mut(header)?;
        let shown = !bounds.is_empty();
        if shown && entry.bounds != bounds {
            entry.bounds = bounds;
            entry.width = bounds.width() as u32;
            entry.height = bounds.height() as u32;
        }
        entry.shown = shown;
        entry.invalidate(None, true);
        Some(())
    });
    gui::wake();
}

fn create_header(window: usize) {
    let found = with(|gui| {
        let entry = gui.windows.get(window)?;
        Some((entry.style, entry.client()))
    })
    .flatten();
    let Some((list_style, client)) = found else {
        return;
    };
    let mut header_style = WS_CHILD;
    if model::shows_header(list_style) {
        header_style |= WS_VISIBLE;
    }
    if list_style & style::NO_SORT_HEADER == 0 {
        header_style |= HDS_BUTTONS;
    }
    let bounds = Rect::sized(1, 1, (client.width() - 2).max(0), HEADER_HEIGHT);
    let kind = ControlKind::Common(CommonKind::Header);
    let Some(header) = create_child(window, kind, header_style, bounds, 0, &[]) else {
        return;
    };
    super::header::adopt(header, window);
    with_state::<ListState, _>(window, |_, control| control.header = header);
    refresh(window);
}

fn paint(window: usize) {
    paint_with::<ListState>(window, |surface, rect, entry, control, gui| {
        let header_window = gui
            .windows
            .get(control.header)
            .is_some_and(|header| header.shown);
        let look = ListLook {
            style: entry.style,
            focused: has_focus(gui, window),
            enabled: windows::enabled_in_tree(&gui.windows, window),
            header_window,
        };
        model::paint(
            surface,
            rect,
            &control.list,
            look,
            &image_list::LayerImages,
            &LIGHT,
            &COMMON_LIGHT,
        );
    });
}

fn send_change(window: usize, code: i32, change: &StateChange) -> usize {
    notify_parent(window, code, |header| NmListView {
        header,
        item: change.index as i32,
        sub_item: 0,
        new_state: change.new,
        old_state: change.old,
        changed: LVIF_STATE,
        x: 0,
        y: 0,
        lparam: change.lparam,
    })
}

fn send_row(window: usize, code: i32, item: i32, sub_item: i32) -> usize {
    let lparam = with_state::<ListState, _>(window, |_, control| {
        control.list.lparam(item.max(0) as usize)
    })
    .unwrap_or(0);
    notify_parent(window, code, |header| NmListView {
        header,
        item,
        sub_item,
        new_state: 0,
        old_state: 0,
        changed: 0,
        x: 0,
        y: 0,
        lparam: match item {
            item if item < 0 => 0,
            _ => lparam,
        },
    })
}

fn send_activation(
    window: usize,
    code: i32,
    item: i32,
    sub_item: i32,
    (x, y): (i32, i32),
    key_flags: u32,
) {
    let (row_state, lparam) = match usize::try_from(item) {
        Ok(index) => with_state::<ListState, _>(window, |_, control| {
            (control.list.state(index), control.list.lparam(index))
        })
        .unwrap_or((0, 0)),
        Err(_) => (0, 0),
    };
    notify_parent(window, code, |header| NmItemActivate {
        header,
        item,
        sub_item,
        new_state: row_state,
        old_state: row_state,
        changed: 0,
        x,
        y,
        lparam,
        key_flags,
    });
}

fn change_states(window: usize, wanted: Wanted) {
    for gaining in [false, true] {
        let mut from = 0;
        while let Some(change) = with_state::<ListState, _>(window, |entry, control| {
            control
                .list
                .next_change(from, &wanted, single(entry), gaining)
        })
        .flatten()
        {
            from = change.index + 1;
            if send_change(window, LVN_ITEMCHANGING, &change) != 0 {
                continue;
            }
            let applied =
                with_state::<ListState, _>(window, |_, control| control.list.apply(&change))
                    .unwrap_or(false);
            if applied {
                invalidate(window);
                send_change(window, LVN_ITEMCHANGED, &change);
            }
        }
    }
}

fn scroll_into_view(window: usize, index: usize) {
    with_state::<ListState, _>(window, |entry, control| {
        let visible = layout_of(entry, &control.list).visible;
        control.list.ensure_visible(index, visible);
    });
    refresh(window);
}

fn user_select(window: usize, target: usize, gesture: Gesture, ctrl: bool, shift: bool) {
    let wanted = with_state::<ListState, _>(window, |entry, control| {
        let single = single(entry);
        let selected = control.list.state(target) & state::SELECTED != 0;
        let anchor = control.list.mark().unwrap_or(target);
        let range = |a: usize, b: usize| Selection::Only {
            first: a.min(b),
            last: a.max(b),
        };
        let (selection, mark) = match gesture {
            _ if shift && !single => (range(anchor, target), None),
            Gesture::Move if ctrl => (Selection::Keep, None),
            Gesture::Click if ctrl => (
                Selection::Row {
                    index: target,
                    selected: !selected,
                },
                Some(target),
            ),
            _ => (range(target, target), Some(target)),
        };
        if mark.is_some() {
            control.list.set_mark(mark);
        }
        Wanted {
            focus: Some(target),
            selection,
        }
    });
    if let Some(wanted) = wanted {
        change_states(window, wanted);
    }
    scroll_into_view(window, target);
}

fn toggle_check(window: usize, index: usize) {
    let current = with_state::<ListState, _>(window, |_, control| control.list.state(index));
    let Some(current) = current else {
        return;
    };
    let image = match state::image_of(current) {
        state::CHECKED => state::UNCHECKED,
        _ => state::CHECKED,
    };
    change_states(
        window,
        Wanted {
            focus: None,
            selection: Selection::Set {
                row: Some(index),
                state: state::image_bits(image),
                mask: state::STATE_IMAGE_MASK,
            },
        },
    );
}

fn modifiers() -> (bool, bool) {
    with(|gui| {
        let held = gui.keyboard_state();
        (held[VK_CONTROL] & HELD != 0, held[VK_SHIFT] & HELD != 0)
    })
    .unwrap_or((false, false))
}

fn key_flags(ctrl: bool, shift: bool) -> u32 {
    let mut flags = 0;
    if ctrl {
        flags |= LVKF_CONTROL;
    }
    if shift {
        flags |= LVKF_SHIFT;
    }
    flags
}

fn key_down(window: usize, virtual_key: u32) {
    notify_parent(window, LVN_KEYDOWN, |header| NmListKeyDown {
        header,
        key: virtual_key as u16,
        flags: 0,
    });
    let (ctrl, shift) = modifiers();
    let found = with_state::<ListState, _>(window, |entry, control| {
        let visible = layout_of(entry, &control.list).visible;
        let focused = control.list.focused();
        (
            control.list.step(virtual_key, visible),
            focused,
            focused.is_some_and(|index| control.list.state(index) & state::SELECTED != 0),
            single(entry),
            control.list.extended & extended::CHECK_BOXES != 0,
        )
    });
    let Some((target, focused, focused_selected, single, checks)) = found else {
        return;
    };
    match virtual_key {
        key::RETURN => {
            notify_parent(window, code::NM_RETURN, |header| header);
            if let Some(focused) = focused
                && focused_selected
            {
                let flags = key_flags(ctrl, shift);
                send_activation(window, LVN_ITEMACTIVATE, focused as i32, 0, (0, 0), flags);
            }
        }
        key::SPACE => {
            let Some(focused) = focused else {
                return;
            };
            match (ctrl, checks) {
                (true, _) => change_states(
                    window,
                    Wanted {
                        focus: Some(focused),
                        selection: Selection::Row {
                            index: focused,
                            selected: !focused_selected,
                        },
                    },
                ),
                (false, true) => toggle_check(window, focused),
                (false, false) => user_select(window, focused, Gesture::Move, false, false),
            }
        }
        key::LETTER_A if ctrl && !single => change_states(
            window,
            Wanted {
                focus: None,
                selection: Selection::All,
            },
        ),
        _ => {
            if let Some(target) = target {
                user_select(window, target, Gesture::Move, ctrl, shift);
            }
        }
    }
}

fn character(window: usize, character: u32) {
    if character <= LAST_PRINTABLE_CONTROL || character == DELETE_CHARACTER {
        return;
    }
    let target = with_state::<ListState, _>(window, |_, control| {
        control.list.find_by_first_glyph(character)
    })
    .flatten();
    if let Some(target) = target {
        user_select(window, target, Gesture::Move, false, false);
    }
}

fn focus_changed(window: usize, gained: bool) {
    if gained {
        let first = with_state::<ListState, _>(window, |_, control| {
            control.list.focused().is_none() && control.list.count() > 0
        })
        .unwrap_or(false);
        if first {
            change_states(
                window,
                Wanted {
                    focus: Some(0),
                    selection: Selection::Keep,
                },
            );
        }
    }
    invalidate(window);
    let notification = match gained {
        true => code::NM_SETFOCUS,
        false => code::NM_KILLFOCUS,
    };
    notify_parent(window, notification, |header| header);
}

fn press(window: usize, item: i32, column: i32) {
    with_state::<ListState, _>(window, |_, control| control.pressed = Some((item, column)));
    set_capture(window);
}

fn scroll_part(list: &mut ListView, layout: &ListLayout, vertical: bool, part: Part) {
    let page = layout.visible as isize;
    let width = layout.rows.width();
    match (vertical, part) {
        (_, Part::Thumb) => {}
        (true, Part::LineUp) => list.scroll_rows(-1, layout.visible),
        (true, Part::LineDown) => list.scroll_rows(1, layout.visible),
        (true, Part::PageUp) => list.scroll_rows(-page, layout.visible),
        (true, Part::PageDown) => list.scroll_rows(page, layout.visible),
        (false, Part::LineUp) => list.set_hpos(list.hpos - HORIZONTAL_LINE, layout),
        (false, Part::LineDown) => list.set_hpos(list.hpos + HORIZONTAL_LINE, layout),
        (false, Part::PageUp) => list.set_hpos(list.hpos - width, layout),
        (false, Part::PageDown) => list.set_hpos(list.hpos + width, layout),
    }
}

fn bar_down(window: usize, vertical: bool, part: Part, along: i32) {
    let dragging = with_state::<ListState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.list);
        if part != Part::Thumb {
            scroll_part(&mut control.list, &layout, vertical, part);
            return Some(false);
        }
        let (bar, info) = match vertical {
            true => (layout.vertical?, control.list.vertical_info(&layout)),
            false => (layout.horizontal?, control.list.horizontal_info(&layout)),
        };
        let (start, length) = match vertical {
            true => (bar.top, bar.height()),
            false => (bar.left, bar.width()),
        };
        let (thumb, _) = scroll_bar::thumb(length, &info)?;
        control.drag = Some(Drag {
            vertical,
            grab: along - start - thumb,
        });
        Some(true)
    })
    .flatten();
    if dragging == Some(true) {
        set_capture(window);
    }
    refresh(window);
}

fn button_down(window: usize, keys: usize, (x, y): (i32, i32), double_message: bool) {
    set_focus(window);
    let (ctrl, shift) = (keys & MK_CONTROL != 0, keys & MK_SHIFT != 0);
    let now = now_ms();
    let found = with_state::<ListState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.list);
        let hit = control.list.hit(&layout, x, y);
        let row = match hit {
            ListHit::Cell { index, .. } => Some(index),
            _ => None,
        };
        let repeated = control
            .last_down
            .is_some_and(|(time, last_x, last_y, last)| {
                Some(last) == row
                    && now.wrapping_sub(time) <= DOUBLE_CLICK_MS
                    && (x - last_x).abs() <= DOUBLE_CLICK_DISTANCE
                    && (y - last_y).abs() <= DOUBLE_CLICK_DISTANCE
            });
        let double = row.is_some() && (double_message || repeated);
        control.last_down = match (double, row) {
            (false, Some(row)) => Some((now, x, y, row)),
            _ => None,
        };
        let full_row = control.list.extended & extended::FULL_ROW_SELECT != 0 || !layout.report;
        (hit, double, full_row)
    });
    let Some((hit, double, full_row)) = found else {
        return;
    };
    match hit {
        ListHit::Vertical(part) => bar_down(window, true, part, y),
        ListHit::Horizontal(part) => bar_down(window, false, part, x),
        ListHit::Cell {
            index,
            column,
            part: CellPart::CheckBox,
        } => {
            toggle_check(window, index);
            press(window, index as i32, column as i32);
        }
        ListHit::Cell { index, column, .. } if full_row || column == 0 => {
            user_select(window, index, Gesture::Click, ctrl, shift);
            if double {
                let flags = key_flags(ctrl, shift);
                send_activation(
                    window,
                    code::NM_DBLCLK,
                    index as i32,
                    column as i32,
                    (x, y),
                    flags,
                );
                send_activation(
                    window,
                    LVN_ITEMACTIVATE,
                    index as i32,
                    column as i32,
                    (x, y),
                    flags,
                );
            } else {
                press(window, index as i32, column as i32);
            }
        }
        ListHit::Cell { column, .. } => empty_down(window, Some(column), ctrl, shift),
        ListHit::Empty { column } => empty_down(window, column, ctrl, shift),
        ListHit::Header | ListHit::Nowhere => {}
    }
}

fn empty_down(window: usize, column: Option<usize>, ctrl: bool, shift: bool) {
    if !ctrl && !shift {
        change_states(
            window,
            Wanted {
                focus: None,
                selection: Selection::Clear,
            },
        );
    }
    press(window, -1, column.map_or(-1, |column| column as i32));
}

fn button_up(window: usize, keys: usize, (x, y): (i32, i32)) {
    let taken = with_state::<ListState, _>(window, |_, control| {
        (control.pressed.take(), control.drag.take())
    });
    let Some((pressed, drag)) = taken else {
        return;
    };
    if pressed.is_some() || drag.is_some() {
        release_capture();
    }
    if let Some((item, column)) = pressed {
        let flags = key_flags(keys & MK_CONTROL != 0, keys & MK_SHIFT != 0);
        send_activation(window, code::NM_CLICK, item, column, (x, y), flags);
    }
}

fn pointer_moved(window: usize, (x, y): (i32, i32)) {
    let moved = with_state::<ListState, _>(window, |entry, control| {
        let drag = control.drag?;
        let layout = layout_of(entry, &control.list);
        match drag.vertical {
            true => {
                let bar = layout.vertical?;
                let info = control.list.vertical_info(&layout);
                let top = model::thumb_position(bar.height(), &info, y - bar.top - drag.grab);
                control.list.set_top(top.max(0) as usize, layout.visible);
            }
            false => {
                let bar = layout.horizontal?;
                let info = control.list.horizontal_info(&layout);
                let hpos = model::thumb_position(bar.width(), &info, x - bar.left - drag.grab);
                control.list.set_hpos(hpos, &layout);
            }
        }
        Some(())
    })
    .flatten();
    if moved.is_some() {
        refresh(window);
    }
}

fn scroll_message(window: usize, vertical: bool, wparam: usize) {
    let request = wparam & 0xffff;
    let position = ((wparam >> 16) & 0xffff) as i32;
    with_state::<ListState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.list);
        let list = &mut control.list;
        let part = match request {
            SB_LINEUP => Some(Part::LineUp),
            SB_LINEDOWN => Some(Part::LineDown),
            SB_PAGEUP => Some(Part::PageUp),
            SB_PAGEDOWN => Some(Part::PageDown),
            _ => None,
        };
        if let Some(part) = part {
            scroll_part(list, &layout, vertical, part);
            return;
        }
        let target = match request {
            SB_THUMBPOSITION | SB_THUMBTRACK => position,
            SB_TOP => 0,
            SB_BOTTOM => i32::MAX,
            _ => return,
        };
        match vertical {
            true => list.set_top(target.max(0) as usize, layout.visible),
            false => list.set_hpos(target, &layout),
        }
    });
    refresh(window);
}

fn header_notified(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `WM_NOTIFY`'s `lParam` is a structure that starts with an `NMHDR`.
    let header = unsafe { (lparam as *const NotifyHeader).read_unaligned() };
    let from_ours = with_state::<ListState, _>(window, |entry, control| {
        control.header == header.from && entry.style & style::NO_SORT_HEADER == 0
    })
    .unwrap_or(false);
    if from_ours && header.code as i32 == HDN_ITEMCLICKW {
        // SAFETY: `HDN_ITEMCLICKW` carries an `NMHEADERW`, whose `iItem` follows the `NMHDR`.
        let clicked = unsafe { (lparam as *const HeaderNotification).read_unaligned() };
        column_click(window, clicked.item);
    }
    0
}

/// `LVN_COLUMNCLICK` for `column` of list `window`: its header's item was clicked.
pub(super) fn column_click(window: usize, column: i32) {
    notify_parent(window, LVN_COLUMNCLICK, |header| NmListView {
        header,
        item: -1,
        sub_item: column,
        new_state: 0,
        old_state: 0,
        changed: 0,
        x: 0,
        y: 0,
        lparam: 0,
    });
}

/// Hands an agent's `action` on `item` of `window` to the window's own thread as the private
/// message `message`, as the thread reading the server's records may not call the program.
pub(super) fn hand_over(window: usize, message: u32, item: usize, action: Action) -> Outcome {
    let thread = with(|gui| gui.windows.get(window).map(|entry| entry.thread)).flatten();
    let Some(thread) = thread else {
        return Outcome::NoSuchElement;
    };
    let delivery = Delivery {
        window,
        thread,
        message,
        wparam: item,
        lparam: action.value() as usize,
    };
    gui::deliver(&delivery, thread_environment::thread_id());
    Outcome::Done
}

fn act_here(window: usize, item: usize, action: Action) {
    let exists = with_state::<ListState, _>(window, |_, control| item < control.list.count())
        .unwrap_or(false);
    if !exists {
        return;
    }
    set_focus(window);
    match action {
        Action::Focus => {
            change_states(
                window,
                Wanted {
                    focus: Some(item),
                    selection: Selection::Keep,
                },
            );
            scroll_into_view(window, item);
        }
        Action::Select | Action::Invoke => {
            user_select(window, item, Gesture::Click, false, false);
            send_activation(window, code::NM_CLICK, item as i32, 0, (0, 0), 0);
            if action == Action::Invoke {
                send_activation(window, code::NM_DBLCLK, item as i32, 0, (0, 0), 0);
                send_activation(window, LVN_ITEMACTIVATE, item as i32, 0, (0, 0), 0);
            }
        }
        Action::SetText => {}
    }
}

/// Does an agent's `action` on row `item` (section 29): `select` as a plain click, `invoke` as
/// a double click, `focus` making it the focused row.
pub(super) fn act(window: usize, item: Option<usize>, action: Action, ours: bool) -> Outcome {
    let Some(item) = item else {
        return Outcome::NotSupported;
    };
    if !matches!(action, Action::Select | Action::Invoke | Action::Focus) {
        return Outcome::NotSupported;
    }
    let exists = with_state::<ListState, _>(window, |_, control| item < control.list.count())
        .unwrap_or(false);
    if !exists {
        return Outcome::NoSuchElement;
    }
    match ours {
        true => {
            act_here(window, item, action);
            Outcome::Done
        }
        false => hand_over(window, ACT_ON_ROW, item, action),
    }
}

fn join_cells<'b>(list: &ListView, index: usize, out: &'b mut [u16; JOINED_UNITS]) -> &'b [u16] {
    let mut length = 0;
    for column in 1..list.columns().count() {
        if column > 1 && length < out.len() {
            out[length] = TAB;
            length += 1;
        }
        for &unit in list.text(index, column) {
            if length == out.len() {
                break;
            }
            out[length] = unit;
            length += 1;
        }
    }
    &out[..length]
}

/// Pushes list `window`'s rows (section 29) under its element `element`: one `ListItem` each,
/// named by its first text, valued by the others joined by tabs.
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    let focused = has_focus(gui, window);
    read_state::<ListState, bool>(gui, window, |entry, control| {
        let list = &control.list;
        let layout = layout_of(entry, list);
        let checks = list.extended & extended::CHECK_BOXES != 0;
        for index in 0..list.count() {
            let rect = list.row_bounds(&layout, index);
            let bounds = Bounds::new(
                origin.0 + rect.left,
                origin.1 + rect.top,
                rect.width(),
                rect.height(),
            );
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(list.text(index, 0), &mut name);
            let mut joined = [0u16; JOINED_UNITS];
            let joined = join_cells(list, index, &mut joined);
            let mut value = [0u8; MAX_LABEL_BYTES];
            let value = utf8(joined, &mut value);
            let row_state = list.state(index);
            let mut bits = 0;
            if row_state & state::SELECTED != 0 {
                bits |= flags::SELECTED;
            }
            if focused && row_state & state::FOCUSED != 0 {
                bits |= flags::FOCUSED;
            }
            if checks && state::image_of(row_state) == state::CHECKED {
                bits |= flags::CHECKED;
            }
            let row = Element::new(item_element(window, index), element, Role::ListItem, bounds)
                .named(name)
                .valued(value)
                .flagged(bits);
            if !tree.push(&row) {
                return false;
            }
        }
        true
    })
    .unwrap_or(true)
}

/// Runs `work` on list `window`'s columns, as its header's messages change them, then repaints
/// the list and the header.
pub(super) fn with_columns<R>(window: usize, work: impl FnOnce(&mut Columns) -> R) -> Option<R> {
    let result = with_state::<ListState, _>(window, |_, control| work(control.list.columns_mut()))?;
    refresh(window);
    Some(result)
}

/// What list `window`'s header draws: the columns, the horizontal scroll and whether its items
/// are buttons.
pub(super) fn header_view<R>(
    gui: &Gui,
    window: usize,
    work: impl FnOnce(&Columns, i32, bool) -> R,
) -> Option<R> {
    read_state::<ListState, R>(gui, window, |entry, control| {
        let buttons = entry.style & style::NO_SORT_HEADER == 0;
        work(control.list.columns(), control.list.hpos, buttons)
    })
}

/// Inserts a column into list `window` (its cells move with it); its index.
pub(super) fn insert_column(
    window: usize,
    at: usize,
    text: &[u16],
    width: i32,
    alignment: u32,
    lparam: usize,
) -> Option<usize> {
    let index = with_state::<ListState, _>(window, |_, control| {
        control
            .list
            .insert_column(at, text, width, alignment, lparam)
    })
    .flatten();
    refresh(window);
    index
}

/// Deletes column `index` of list `window` and its cells.
pub(super) fn delete_column(window: usize, index: usize) -> bool {
    let deleted =
        with_state::<ListState, _>(window, |_, control| control.list.delete_column(index))
            .unwrap_or(false);
    refresh(window);
    deleted
}

/// The text a control message's `pszText` points to: empty for none or
/// `LPSTR_TEXTCALLBACKW`.
///
/// # Safety
/// `pointer` must be 0, `LPSTR_TEXTCALLBACKW` or a NUL-terminated string.
pub(super) unsafe fn text_at<'a>(pointer: usize) -> &'a [u16] {
    match pointer {
        0 | TEXT_CALLBACK => &[],
        // SAFETY: the caller's duty: a NUL-terminated string.
        pointer => unsafe { file::wide_string(pointer as *const u16) },
    }
}

unsafe fn read_item(lparam: usize) -> ItemHead {
    // SAFETY: the caller's duty: `lparam` is the program's `LVITEMW`, at least as long as its
    // first version, which this reads.
    unsafe { (lparam as *const ItemHead).read_unaligned() }
}

fn insert_item(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return FAILED;
    }
    // SAFETY: `LVM_INSERTITEMW`'s `lParam` is the program's `LVITEMW`.
    let item = unsafe { read_item(lparam) };
    if item.sub_item != 0 {
        return FAILED;
    }
    let text = match item.mask & LVIF_TEXT {
        0 => &[][..],
        // SAFETY: with `LVIF_TEXT`, `pszText` is a NUL-terminated string or the callback value.
        _ => unsafe { text_at(item.text) },
    };
    let inserted = with_state::<ListState, _>(window, |entry, control| {
        let sorting = entry.style & (style::SORT_ASCENDING | style::SORT_DESCENDING);
        let at = match sorting {
            0 => item.item.max(0) as usize,
            _ => control
                .list
                .sorted_position(text, sorting & style::SORT_DESCENDING != 0),
        };
        let image = match item.mask & LVIF_IMAGE {
            0 => 0,
            _ => item.image,
        };
        let data = match item.mask & LVIF_PARAM {
            0 => 0,
            _ => item.lparam,
        };
        let index = control.list.insert(at, text, image, data)?;
        if control.list.extended & extended::CHECK_BOXES != 0 {
            let change = StateChange {
                index,
                old: 0,
                new: state::image_bits(state::UNCHECKED),
                lparam: data,
            };
            control.list.apply(&change);
        }
        Some(index)
    })
    .flatten();
    let Some(index) = inserted else {
        return FAILED;
    };
    send_row(window, LVN_INSERTITEM, index as i32, 0);
    if item.mask & LVIF_STATE != 0 && item.state_mask != 0 {
        change_states(
            window,
            Wanted {
                focus: None,
                selection: Selection::Set {
                    row: Some(index),
                    state: item.state,
                    mask: item.state_mask,
                },
            },
        );
    }
    refresh(window);
    index
}

fn set_item(window: usize, index: Option<usize>, lparam: usize, text_only: bool) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `LVM_SETITEMW`'s and `LVM_SETITEMTEXTW`'s `lParam` is the program's `LVITEMW`.
    let item = unsafe { read_item(lparam) };
    let index = match (index, text_only) {
        (Some(index), true) => index,
        (_, true) => return 0,
        (_, false) => match usize::try_from(item.item) {
            Ok(index) => index,
            Err(_) => return 0,
        },
    };
    let mask = match text_only {
        true => LVIF_TEXT,
        false => item.mask,
    };
    let Ok(column) = usize::try_from(item.sub_item) else {
        return 0;
    };
    let text = match mask & LVIF_TEXT {
        0 => &[][..],
        // SAFETY: with `LVIF_TEXT`, `pszText` is a NUL-terminated string or the callback value.
        _ => unsafe { text_at(item.text) },
    };
    let done = with_state::<ListState, _>(window, |_, control| {
        if index >= control.list.count() {
            return false;
        }
        let mut done = true;
        if mask & LVIF_TEXT != 0 {
            done &= control.list.set_text(index, column, text);
        }
        if column == 0 && mask & LVIF_IMAGE != 0 {
            control.list.set_image(index, item.image);
        }
        if column == 0 && mask & LVIF_PARAM != 0 {
            control.list.set_lparam(index, item.lparam);
        }
        done
    })
    .unwrap_or(false);
    if done && column == 0 && mask & LVIF_STATE != 0 && item.state_mask != 0 {
        change_states(
            window,
            Wanted {
                focus: None,
                selection: Selection::Set {
                    row: Some(index),
                    state: item.state,
                    mask: item.state_mask,
                },
            },
        );
    }
    refresh(window);
    usize::from(done)
}

fn get_item(window: usize, index: Option<usize>, lparam: usize, text_only: bool) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `LVM_GETITEMW`'s and `LVM_GETITEMTEXTW`'s `lParam` is the program's `LVITEMW`.
    let item = unsafe { read_item(lparam) };
    let index = match (index, text_only) {
        (Some(index), true) => Some(index),
        (_, true) => None,
        (_, false) => usize::try_from(item.item).ok(),
    };
    let Some(index) = index else {
        return 0;
    };
    let column = usize::try_from(item.sub_item).unwrap_or(usize::MAX);
    let mask = match text_only {
        true => LVIF_TEXT,
        false => item.mask,
    };
    let target = lparam as *mut ItemHead;
    with_state::<ListState, _>(window, |_, control| {
        let list = &control.list;
        if index >= list.count() {
            return 0;
        }
        let mut copied = 0;
        if mask & LVIF_TEXT != 0 && item.text != TEXT_CALLBACK {
            // SAFETY: `pszText` holds `cchTextMax` writable units, the messages' contract.
            copied = unsafe {
                window_text::copy_out(
                    list.text(index, column),
                    item.text,
                    item.text_max.max(0) as usize,
                )
            };
        }
        if mask & LVIF_IMAGE != 0 {
            // SAFETY: `target` is the program's `LVITEMW`, as read above; each write is one of
            // its fields.
            let image = unsafe { &raw mut (*target).image };
            // SAFETY: as above.
            unsafe { image.write_unaligned(list.image(index)) };
        }
        if mask & LVIF_PARAM != 0 {
            // SAFETY: as above.
            let lparam = unsafe { &raw mut (*target).lparam };
            // SAFETY: as above.
            unsafe { lparam.write_unaligned(list.lparam(index)) };
        }
        if mask & LVIF_STATE != 0 {
            let bits = list.state(index) & item.state_mask;
            // SAFETY: as above.
            let state = unsafe { &raw mut (*target).state };
            // SAFETY: as above.
            unsafe { state.write_unaligned(bits) };
        }
        match text_only {
            true => copied,
            false => 1,
        }
    })
    .unwrap_or(0)
}

fn set_item_state(window: usize, wparam: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `LVM_SETITEMSTATE`'s `lParam` is the program's `LVITEMW`.
    let item = unsafe { read_item(lparam) };
    let row = index_parameter(wparam);
    let valid = with_state::<ListState, _>(window, |_, control| {
        row.is_none_or(|row| row < control.list.count())
    })
    .unwrap_or(false);
    if !valid {
        return 0;
    }
    change_states(
        window,
        Wanted {
            focus: None,
            selection: Selection::Set {
                row,
                state: item.state,
                mask: item.state_mask,
            },
        },
    );
    refresh(window);
    1
}

fn delete_item(window: usize, index: usize) -> usize {
    let exists = with_state::<ListState, _>(window, |_, control| index < control.list.count())
        .unwrap_or(false);
    if !exists {
        return 0;
    }
    send_row(window, LVN_DELETEITEM, index as i32, 0);
    let deleted = with_state::<ListState, _>(window, |_, control| control.list.delete(index))
        .unwrap_or(false);
    refresh(window);
    usize::from(deleted)
}

fn delete_all(window: usize) -> usize {
    let count = with_state::<ListState, _>(window, |_, control| control.list.count()).unwrap_or(0);
    if count > 0 && send_row(window, LVN_DELETEALLITEMS, -1, 0) == 0 {
        for index in 0..count {
            send_row(window, LVN_DELETEITEM, index as i32, 0);
        }
    }
    with_state::<ListState, _>(window, |_, control| control.list.clear());
    refresh(window);
    1
}

/// Writes `rect` into the program's `RECT` at `target`, a message's answer.
///
/// # Safety
/// `target` must be a writable `RECT`.
pub(super) unsafe fn write_rect(target: usize, rect: Rect) {
    let rect = Win32Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    // SAFETY: the caller's duty.
    unsafe { (target as *mut Win32Rect).write_unaligned(rect) };
}

fn item_rect(window: usize, index: usize, lparam: usize, column: Option<usize>) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `LVM_GETITEMRECT`'s and `LVM_GETSUBITEMRECT`'s `lParam` is the program's `RECT`,
    // its `left` (and `top`, the subitem) set on the way in.
    let asked = unsafe { (lparam as *const Win32Rect).read_unaligned() };
    let column = match column {
        Some(_) => usize::try_from(asked.top).ok(),
        None => Some(0),
    };
    let Some(column) = column else {
        return 0;
    };
    let rect = with_state::<ListState, _>(window, |entry, control| {
        let list = &control.list;
        let layout = layout_of(entry, list);
        if index >= list.count() || (column > 0 && column >= list.columns().count()) {
            return None;
        }
        let part = match asked.left {
            LVIR_ICON => ItemPart::Icon,
            LVIR_LABEL | LVIR_SELECTBOUNDS => ItemPart::Label,
            _ => ItemPart::Bounds,
        };
        Some(match column {
            0 => list.item_rect(&layout, index, part),
            column => list.cell_rect(&layout, index, column),
        })
    })
    .flatten();
    match rect {
        Some(rect) => {
            // SAFETY: `lParam` is the program's `RECT`, as read above.
            unsafe { write_rect(lparam, rect) };
            1
        }
        None => 0,
    }
}

fn hit_test(window: usize, lparam: usize, subitems: bool) -> usize {
    if lparam == 0 {
        return FAILED;
    }
    let info = lparam as *mut HitTestInfo;
    // SAFETY: `LVM_HITTEST`'s and `LVM_SUBITEMHITTEST`'s `lParam` is the program's
    // `LVHITTESTINFO`, whose point is read here.
    let x_at = unsafe { &raw const (*info).x };
    // SAFETY: as above.
    let y_at = unsafe { &raw const (*info).y };
    // SAFETY: as above.
    let x = unsafe { x_at.read_unaligned() };
    // SAFETY: as above.
    let y = unsafe { y_at.read_unaligned() };
    let answer = with_state::<ListState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.list);
        let client = entry.client();
        let full_row =
            control.list.extended & extended::FULL_ROW_SELECT != 0 || !layout.report || subitems;
        let outside = match () {
            () if y < client.top => LVHT_ABOVE,
            () if y >= client.bottom => LVHT_BELOW,
            () if x < client.left => LVHT_TOLEFT,
            () if x >= client.right => LVHT_TORIGHT,
            () => 0,
        };
        if outside != 0 {
            return (outside, -1, -1);
        }
        match control.list.hit(&layout, x, y) {
            ListHit::Cell {
                index,
                column,
                part,
            } if full_row || column == 0 => {
                let flags = match part {
                    CellPart::CheckBox => LVHT_ONITEMSTATEICON,
                    CellPart::Image => LVHT_ONITEMICON,
                    CellPart::Text => LVHT_ONITEMLABEL,
                };
                (flags, index as i32, column as i32)
            }
            ListHit::Cell { column, .. } => (LVHT_NOWHERE, -1, column as i32),
            ListHit::Empty { column } => (LVHT_NOWHERE, -1, column.map_or(-1, |c| c as i32)),
            _ => (LVHT_NOWHERE, -1, -1),
        }
    });
    let (found, item, sub_item) = answer.unwrap_or((LVHT_NOWHERE, -1, -1));
    // SAFETY: the program's `LVHITTESTINFO`, as above.
    let flags_at = unsafe { &raw mut (*info).flags };
    // SAFETY: as above.
    let item_at = unsafe { &raw mut (*info).item };
    // SAFETY: as above.
    unsafe { flags_at.write_unaligned(found) };
    // SAFETY: as above.
    unsafe { item_at.write_unaligned(item) };
    if subitems {
        // SAFETY: as above; `iSubItem` is written only for `LVM_SUBITEMHITTEST`, which needs the
        // longer structure.
        let sub_item_at = unsafe { &raw mut (*info).sub_item };
        // SAFETY: as above.
        unsafe { sub_item_at.write_unaligned(sub_item) };
    }
    item as isize as usize
}

fn column_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    if number == LVM_DELETECOLUMN {
        return usize::from(delete_column(window, wparam));
    }
    if lparam == 0 {
        return match number {
            LVM_INSERTCOLUMNW => FAILED,
            _ => 0,
        };
    }
    // SAFETY: these messages' `lParam` is the program's `LVCOLUMNW`, at least as long as its
    // first version, which this reads.
    let column = unsafe { (lparam as *const ColumnHead).read_unaligned() };
    let text = match column.mask & LVCF_TEXT {
        0 => &[][..],
        _ if number == LVM_GETCOLUMNW => &[][..],
        // SAFETY: with `LVCF_TEXT`, `pszText` is a NUL-terminated string.
        _ => unsafe { text_at(column.text) },
    };
    match number {
        LVM_INSERTCOLUMNW => {
            let width = match column.mask & LVCF_WIDTH {
                0 => 0,
                _ => column.width,
            };
            let alignment = match column.mask & LVCF_FMT {
                0 => format::LEFT,
                _ => column.format,
            };
            insert_column(window, wparam, text, width, alignment, 0).unwrap_or(FAILED)
        }
        LVM_SETCOLUMNW => {
            let done = with_columns(window, |columns| {
                if wparam >= columns.count() {
                    return false;
                }
                let mut done = true;
                if column.mask & LVCF_TEXT != 0 {
                    done &= columns.set_text(wparam, text);
                }
                if column.mask & LVCF_WIDTH != 0 {
                    columns.set_width(wparam, column.width);
                }
                if column.mask & LVCF_FMT != 0 {
                    columns.set_format(wparam, column.format);
                }
                done
            });
            usize::from(done == Some(true))
        }
        _ => {
            let target = lparam as *mut ColumnHead;
            with_state::<ListState, _>(window, |_, control| {
                let columns = control.list.columns();
                if wparam >= columns.count() {
                    return 0;
                }
                if column.mask & LVCF_FMT != 0 {
                    // SAFETY: `target` is the program's `LVCOLUMNW`, as read above.
                    let format = unsafe { &raw mut (*target).format };
                    // SAFETY: as above.
                    unsafe { format.write_unaligned(columns.format(wparam)) };
                }
                if column.mask & LVCF_WIDTH != 0 {
                    // SAFETY: as above.
                    let width = unsafe { &raw mut (*target).width };
                    // SAFETY: as above.
                    unsafe { width.write_unaligned(columns.width(wparam)) };
                }
                if column.mask & LVCF_SUBITEM != 0 {
                    // SAFETY: as above.
                    let sub_item = unsafe { &raw mut (*target).sub_item };
                    // SAFETY: as above.
                    unsafe { sub_item.write_unaligned(wparam as i32) };
                }
                if column.mask & LVCF_TEXT != 0 {
                    // SAFETY: `pszText` holds `cchTextMax` writable units when `LVCF_TEXT` asks
                    // for the title.
                    unsafe {
                        window_text::copy_out(
                            columns.text(wparam),
                            column.text,
                            column.text_max.max(0) as usize,
                        )
                    };
                }
                1
            })
            .unwrap_or(0)
        }
    }
}

fn set_column_width(window: usize, column: usize, lparam: usize) -> usize {
    let asked = (lparam & 0xffff) as u16 as i16 as i32;
    let done = with_state::<ListState, _>(window, |entry, control| {
        let rows_width = layout_of(entry, &control.list).rows.width();
        let width = match asked {
            LVSCW_AUTOSIZE => control.list.auto_width(column, false, rows_width),
            LVSCW_AUTOSIZE_USEHEADER => control.list.auto_width(column, true, rows_width),
            width => width,
        };
        control.list.columns_mut().set_width(column, width)
    })
    .unwrap_or(false);
    refresh(window);
    usize::from(done)
}

fn sort_items(window: usize, extra: usize, compare: usize, by_index: bool) -> usize {
    if compare == 0 {
        return 0;
    }
    let mut keys = [0usize; MAX_ROWS];
    let count = with_state::<ListState, _>(window, |_, control| {
        let count = control.list.count();
        for (index, key) in keys[..count].iter_mut().enumerate() {
            *key = control.list.lparam(index);
        }
        count
    });
    let Some(count) = count else {
        return 0;
    };
    let mut order = [0u16; MAX_ROWS];
    for (index, slot) in order[..count].iter_mut().enumerate() {
        *slot = index as u16;
    }
    let mut scratch = [0u16; MAX_ROWS];
    model::stable_sort(
        &mut order[..count],
        &mut scratch[..count],
        &mut |a: u16, b: u16| {
            let (a, b) = (usize::from(a), usize::from(b));
            let (first, second) = match by_index {
                true => (a, b),
                false => (keys[a], keys[b]),
            };
            // SAFETY: the program's `PFNLVCOMPARE`, `LVM_SORTITEMS`' (and `SORTITEMSEX`') contract:
            // it takes two items' data (or indexes) and `lParamSort`, and returns an `int`; the GUI
            // lock is not held.
            let answer = unsafe { call_program(compare, [first, second, extra, 0]) } as u32 as i32;
            answer.cmp(&0)
        },
    );
    let sorted = with_state::<ListState, _>(window, |_, control| {
        control.list.count() == count && control.list.reorder(&order[..count])
    })
    .unwrap_or(false);
    refresh(window);
    usize::from(sorted)
}

fn fold(unit: u16) -> u16 {
    match unit {
        0x41..=0x5a => unit + 0x20,
        unit => unit,
    }
}

fn find_item(window: usize, wparam: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return FAILED;
    }
    // SAFETY: `LVM_FINDITEMW`'s `lParam` is the program's `LVFINDINFOW`.
    let find = unsafe { (lparam as *const FindInfo).read_unaligned() };
    let wanted = match find.flags & LVFI_PARAM {
        0 => {
            // SAFETY: without `LVFI_PARAM`, `psz` is the NUL-terminated text searched for.
            unsafe { text_at(find.text) }
        }
        _ => &[][..],
    };
    with_state::<ListState, _>(window, |_, control| {
        let list = &control.list;
        let count = list.count();
        let start = index_parameter(wparam).map_or(0, |start| start + 1);
        let steps = match find.flags & LVFI_WRAP {
            0 => count.saturating_sub(start),
            _ => count,
        };
        (0..steps)
            .map(|step| (start + step) % count.max(1))
            .find(|&index| match find.flags & LVFI_PARAM {
                0 => {
                    let text = list.text(index, 0);
                    let prefix = text.len() >= wanted.len()
                        && text.iter().zip(wanted).all(|(a, b)| fold(*a) == fold(*b));
                    match find.flags & LVFI_PARTIAL {
                        0 => prefix && text.len() == wanted.len(),
                        _ => prefix,
                    }
                }
                _ => list.lparam(index) == find.lparam,
            })
            .unwrap_or(FAILED)
    })
    .unwrap_or(FAILED)
}

fn colour_message(window: usize, number: u32, lparam: usize) -> usize {
    let answer = with_state::<ListState, _>(window, |_, control| {
        let list = &mut control.list;
        let shown =
            |colour: Option<u32>, default: u32| colour.map_or(default, swap_red_blue) as usize;
        match number {
            LVM_SETBKCOLOR => {
                list.background = colour_parameter(lparam);
                1
            }
            LVM_SETTEXTCOLOR => {
                list.text_colour = colour_parameter(lparam);
                1
            }
            LVM_SETTEXTBKCOLOR => {
                list.text_background = colour_parameter(lparam);
                1
            }
            LVM_GETBKCOLOR => shown(list.background, DEFAULT_BACKGROUND),
            LVM_GETTEXTCOLOR => shown(list.text_colour, DEFAULT_TEXT),
            _ => shown(list.text_background, CLR_NONE),
        }
    })
    .unwrap_or(0);
    if matches!(
        number,
        LVM_SETBKCOLOR | LVM_SETTEXTCOLOR | LVM_SETTEXTBKCOLOR
    ) {
        refresh(window);
    }
    answer
}

fn image_list_message(window: usize, set: bool, which: usize, list: usize) -> usize {
    let answer = with_state::<ListState, _>(window, |_, control| {
        let shown = &mut control.list;
        let slot = match which {
            LVSIL_NORMAL => &mut shown.normal_images,
            LVSIL_SMALL => &mut shown.small_images,
            LVSIL_STATE => &mut shown.state_images,
            _ => return 0,
        };
        match set {
            true => core::mem::replace(slot, list),
            false => *slot,
        }
    })
    .unwrap_or(0);
    if set {
        refresh(window);
    }
    answer
}

fn view_message(window: usize, set: bool, view: usize) -> usize {
    if !set {
        return with(|gui| {
            gui.windows
                .get(window)
                .map_or(0, |entry| (entry.style & style::TYPE_MASK) as usize)
        })
        .unwrap_or(0);
    }
    let Ok(view) = u32::try_from(view) else {
        return FAILED;
    };
    if view > style::TYPE_MASK {
        return FAILED;
    }
    with(|gui| {
        if let Some(entry) = gui.windows.get_mut(window) {
            entry.style = (entry.style & !style::TYPE_MASK) | view;
        }
    });
    refresh(window);
    1
}

fn read(window: usize, work: impl FnOnce(&Window, &ListView) -> usize) -> usize {
    with_state::<ListState, _>(window, |entry, control| work(entry, &control.list)).unwrap_or(0)
}

fn list_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        LVM_GETITEMCOUNT => read(window, |_, list| list.count()),
        LVM_GETSELECTEDCOUNT => read(window, |_, list| list.selected_count()),
        LVM_GETTOPINDEX => read(window, |_, list| list.top()),
        LVM_GETCOUNTPERPAGE => read(window, |entry, list| layout_of(entry, list).visible),
        LVM_GETITEMSTATE => read(window, |_, list| {
            (list.state(wparam) & lparam as u32) as usize
        }),
        LVM_GETSELECTIONMARK => read(window, |_, list| list.mark().unwrap_or(FAILED)),
        LVM_GETEXTENDEDLISTVIEWSTYLE => read(window, |_, list| list.extended as usize),
        LVM_GETCOLUMNWIDTH => read(window, |_, list| list.columns().width(wparam) as usize),
        LVM_GETHEADER => {
            with_state::<ListState, _>(window, |_, control| control.header).unwrap_or(0)
        }
        LVM_GETNEXTITEM => read(window, |_, list| {
            let mut wanted = 0;
            if lparam & LVNI_FOCUSED != 0 {
                wanted |= state::FOCUSED;
            }
            if lparam & LVNI_SELECTED != 0 {
                wanted |= state::SELECTED;
            }
            let backwards = lparam & LVNI_ABOVE != 0;
            list.next_item(index_parameter(wparam), wanted, backwards)
                .unwrap_or(FAILED)
        }),
        LVM_GETSTRINGWIDTHW => match lparam {
            0 => 0,
            text => {
                // SAFETY: `LVM_GETSTRINGWIDTHW`'s `lParam` is a NUL-terminated string.
                let units = unsafe { text_at(text) };
                plain_width_cells(units) * CELL_WIDTH as usize
            }
        },
        LVM_INSERTITEMW => insert_item(window, lparam),
        LVM_SETITEMW => set_item(window, None, lparam, false),
        LVM_SETITEMTEXTW => set_item(window, Some(wparam), lparam, true),
        LVM_GETITEMW => get_item(window, None, lparam, false),
        LVM_GETITEMTEXTW => get_item(window, Some(wparam), lparam, true),
        LVM_SETITEMSTATE => set_item_state(window, wparam, lparam),
        LVM_DELETEITEM => delete_item(window, wparam),
        LVM_DELETEALLITEMS => delete_all(window),
        LVM_ENSUREVISIBLE => {
            scroll_into_view(window, wparam);
            1
        }
        LVM_SCROLL => {
            with_state::<ListState, _>(window, |entry, control| {
                let layout = layout_of(entry, &control.list);
                let rows = (lparam as u32 as i32) / ROW_HEIGHT;
                control.list.scroll_rows(rows as isize, layout.visible);
                let hpos = control.list.hpos + wparam as u32 as i32;
                control.list.set_hpos(hpos, &layout);
            });
            refresh(window);
            1
        }
        LVM_GETITEMRECT => item_rect(window, wparam, lparam, None),
        LVM_GETSUBITEMRECT => item_rect(window, wparam, lparam, Some(0)),
        LVM_HITTEST => hit_test(window, lparam, false),
        LVM_SUBITEMHITTEST => hit_test(window, lparam, true),
        LVM_INSERTCOLUMNW | LVM_SETCOLUMNW | LVM_GETCOLUMNW | LVM_DELETECOLUMN => {
            column_message(window, number, wparam, lparam)
        }
        LVM_SETCOLUMNWIDTH => set_column_width(window, wparam, lparam),
        LVM_GETCOLUMNORDERARRAY => {
            let count = read(window, |_, list| list.columns().count());
            if lparam == 0 || wparam < count {
                return 0;
            }
            // SAFETY: `LVM_GETCOLUMNORDERARRAY`'s `lParam` holds `wParam` writable `int`s.
            let order = unsafe { core::slice::from_raw_parts_mut(lparam as *mut i32, count) };
            for (index, slot) in order.iter_mut().enumerate() {
                *slot = index as i32;
            }
            1
        }
        LVM_SETCOLUMNORDERARRAY => 0,
        LVM_SETEXTENDEDLISTVIEWSTYLE => {
            let mask = match wparam as u32 {
                0 => u32::MAX,
                mask => mask,
            };
            let old = with_state::<ListState, _>(window, |_, control| {
                let old = control.list.extended;
                control.list.extended = (old & !mask) | (lparam as u32 & mask);
                old
            })
            .unwrap_or(0);
            refresh(window);
            old as usize
        }
        LVM_SETIMAGELIST => image_list_message(window, true, wparam, lparam),
        LVM_GETIMAGELIST => image_list_message(window, false, wparam, 0),
        LVM_SETBKCOLOR | LVM_GETBKCOLOR | LVM_SETTEXTCOLOR | LVM_GETTEXTCOLOR
        | LVM_SETTEXTBKCOLOR | LVM_GETTEXTBKCOLOR => colour_message(window, number, lparam),
        LVM_SORTITEMS => sort_items(window, wparam, lparam, false),
        LVM_SORTITEMSEX => sort_items(window, wparam, lparam, true),
        LVM_SETSELECTIONMARK => {
            let previous = with_state::<ListState, _>(window, |_, control| {
                let previous = control.list.mark();
                control.list.set_mark(index_parameter(lparam));
                previous
            })
            .flatten();
            previous.unwrap_or(FAILED)
        }
        LVM_SETITEMCOUNT | LVM_UPDATE => 1,
        LVM_REDRAWITEMS => {
            refresh(window);
            1
        }
        LVM_SETVIEW => view_message(window, true, wparam),
        LVM_GETVIEW => view_message(window, false, 0),
        LVM_FINDITEMW => find_item(window, wparam, lparam),
        _ => 0,
    }
}

/// The window procedure of every list view.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::CREATE => {
            create_header(window);
            0
        }
        message::PAINT => {
            paint(window);
            0
        }
        message::SIZE | WM_STYLECHANGED | message::ENABLE => {
            refresh(window);
            0
        }
        message::SET_FOCUS | message::KILL_FOCUS => {
            focus_changed(window, number == message::SET_FOCUS);
            0
        }
        message::LEFT_BUTTON_DOWN | message::LEFT_BUTTON_DOUBLE_CLICK => {
            let double = number == message::LEFT_BUTTON_DOUBLE_CLICK;
            button_down(window, wparam, point(lparam), double);
            0
        }
        message::LEFT_BUTTON_UP => {
            button_up(window, wparam, point(lparam));
            0
        }
        message::MOUSE_MOVE => {
            pointer_moved(window, point(lparam));
            0
        }
        message::KEY_DOWN => {
            key_down(window, wparam as u32);
            0
        }
        message::CHAR => {
            character(window, wparam as u32);
            0
        }
        WM_VSCROLL | WM_HSCROLL => {
            scroll_message(window, number == WM_VSCROLL, wparam);
            0
        }
        WM_NOTIFY => header_notified(window, lparam),
        ACT_ON_ROW => {
            if let Some(action) = Action::from_value(lparam as u32) {
                act_here(window, wparam, action);
            }
            0
        }
        CCM_GETUNICODEFORMAT => 1,
        number if (LVM_FIRST..=LVM_LAST).contains(&number) => {
            list_message(window, number, wparam, lparam)
        }
        _ => default_procedure(window, number, wparam, lparam),
    }
}
