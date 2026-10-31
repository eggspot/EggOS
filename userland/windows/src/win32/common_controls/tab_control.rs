//! `SysTabControl32` (D178, design section 23): the layer's procedure over
//! `eggos_windows_gui`'s tab control model. A click or the keys select a tab with
//! `TCN_SELCHANGING` (which the parent may refuse) and `TCN_SELCHANGE`, `TCM_SETCURSEL` selects
//! silently; the program shows and hides each page's controls, whether they are the tab
//! control's children or its siblings. Painting fills the page, so after each paint the later
//! siblings over it (a page's controls) are repainted on top. Its tabs are radio buttons in
//! agents' tree.

use super::super::dialog::{has_focus, set_focus};
use super::super::gui::{self, Delivery, Gui, with};
use super::super::user32::Win32Rect;
use super::super::{FALSE, TRUE, thread_environment};
use super::{
    Model, NotifyHeader, allocate, default_procedure, item_element, notify_parent, paint_with,
    read_state, utf8, with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, Window};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, Outcome, Role, TreeWriter, flags,
};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::common::COMMON_LIGHT;
use eggos_windows_gui::controls::tab_control::{
    self, MAX_TABS, TabControl, TabLook, adjust_rect, style as tab_style,
};
use eggos_windows_gui::surface::Rect;

const TCM_FIRST: u32 = 0x1300;
const TCM_GETIMAGELIST: u32 = TCM_FIRST + 2;
const TCM_SETIMAGELIST: u32 = TCM_FIRST + 3;
const TCM_GETITEMCOUNT: u32 = TCM_FIRST + 4;
const TCM_DELETEITEM: u32 = TCM_FIRST + 8;
const TCM_DELETEALLITEMS: u32 = TCM_FIRST + 9;
const TCM_GETITEMRECT: u32 = TCM_FIRST + 10;
const TCM_GETCURSEL: u32 = TCM_FIRST + 11;
const TCM_SETCURSEL: u32 = TCM_FIRST + 12;
const TCM_HITTEST: u32 = TCM_FIRST + 13;
const TCM_SETITEMEXTRA: u32 = TCM_FIRST + 14;
const TCM_ADJUSTRECT: u32 = TCM_FIRST + 40;
const TCM_SETITEMSIZE: u32 = TCM_FIRST + 41;
const TCM_GETROWCOUNT: u32 = TCM_FIRST + 44;
const TCM_GETCURFOCUS: u32 = TCM_FIRST + 47;
const TCM_SETCURFOCUS: u32 = TCM_FIRST + 48;
const TCM_GETITEMW: u32 = TCM_FIRST + 60;
const TCM_SETITEMW: u32 = TCM_FIRST + 61;
const TCM_INSERTITEMW: u32 = TCM_FIRST + 62;
const TCM_LAST: u32 = TCM_FIRST + 62;
const TCN_SELCHANGE: i32 = -551;
const TCN_SELCHANGING: i32 = -552;
const TCIF_TEXT: u32 = 0x1;
const TCIF_IMAGE: u32 = 0x2;
const TCIF_PARAM: u32 = 0x8;
const TCIF_STATE: u32 = 0x10;
const TCHT_NOWHERE: u32 = 0x1;
const TCHT_ONITEM: u32 = 0x6;
const TEXT_CALLBACK: usize = usize::MAX;
const NONE: usize = usize::MAX;
const VK_CONTROL: usize = 0x11;
const HELD: u8 = 0x80;
const LPARAM_BYTES: usize = core::mem::size_of::<usize>();

#[repr(C)]
struct TabItem {
    mask: u32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_capacity: i32,
    image: i32,
    data: usize,
}

#[repr(C)]
struct HitTest {
    x: i32,
    y: i32,
    flags: u32,
}

/// A tab control's model with the settings it keeps but does not draw.
pub(super) struct TabState {
    tabs: TabControl,
    image_list: usize,
    extra_bytes: usize,
}

impl Model for TabState {
    const KIND: CommonKind = CommonKind::TabControl;
}

/// A new tab control's state.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(TabState {
        tabs: TabControl::new(),
        image_list: 0,
        extra_bytes: LPARAM_BYTES,
    })
}

fn refresh(window: usize) {
    with(|gui| windows::invalidate_tree(&mut gui.windows, window, None, true));
    gui::wake();
}

fn repaint_siblings_above(window: usize) {
    let woke = with(|gui| {
        let entry = gui.windows.get(window)?;
        let (parent, bounds, sequence) = (entry.parent, entry.bounds, entry.creation_sequence);
        let (kids, count) = windows::children(&gui.windows, parent);
        let mut any = false;
        for &kid in &kids[..count] {
            let Some(sibling) = gui.windows.get(kid) else {
                continue;
            };
            if kid == window || !sibling.shown || sibling.creation_sequence < sequence {
                continue;
            }
            let over = sibling.bounds;
            let covered = bounds.intersection(&over);
            if !covered.is_empty() {
                let local = covered.offset(-over.left, -over.top);
                windows::invalidate_tree(&mut gui.windows, kid, Some(local), true);
                any = true;
            }
        }
        Some(any)
    })
    .flatten();
    if woke == Some(true) {
        gui::wake();
    }
}

fn index_result(index: Option<usize>) -> usize {
    index.unwrap_or(NONE)
}

fn select_with_notifications(window: usize, index: usize) -> bool {
    let current = with_state::<TabState, _>(window, |_, state| {
        (index < state.tabs.count()).then_some(state.tabs.selected())
    })
    .flatten();
    match current {
        Some(current) if current != Some(index) => {}
        _ => return false,
    }
    if notify_parent(window, TCN_SELCHANGING, |header: NotifyHeader| header) != 0 {
        return false;
    }
    let changed =
        with_state::<TabState, _>(window, |_, state| state.tabs.select(Some(index)).is_some());
    if changed != Some(true) {
        return false;
    }
    refresh(window);
    notify_parent(window, TCN_SELCHANGE, |header: NotifyHeader| header);
    true
}

/// The procedure of every tab control.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::PAINT => {
            paint(window);
            repaint_siblings_above(window);
            0
        }
        message::SIZE | message::ENABLE | message::SET_FOCUS | message::KILL_FOCUS => {
            refresh(window);
            0
        }
        message::LEFT_BUTTON_DOWN | message::LEFT_BUTTON_DOUBLE_CLICK => {
            left_button_down(window, super::super::controls::point(lparam));
            0
        }
        message::KEY_DOWN => {
            key_down(window, wparam);
            0
        }
        TCM_SETCURFOCUS => {
            select_with_notifications(window, wparam);
            0
        }
        TCM_INSERTITEMW | TCM_SETITEMW => {
            let item = lparam as *const TabItem;
            if item.is_null() {
                return item_failed(number);
            }
            // SAFETY: `TCM_INSERTITEMW`'s and `TCM_SETITEMW`'s contract: a readable `TCITEMW`.
            let item = unsafe { &*item };
            let text = match (item.mask & TCIF_TEXT, item.text as usize) {
                (0, _) => None,
                (_, 0 | TEXT_CALLBACK) => Some(&[][..]),
                // SAFETY: as above; `pszText` is a NUL-terminated string.
                (_, text) => Some(unsafe { super::super::file::wide_string(text as *const u16) }),
            };
            let result = with_state::<TabState, _>(window, |_, state| {
                let data = (item.mask & TCIF_PARAM != 0).then(|| item_data(item, state));
                match number {
                    TCM_INSERTITEMW => index_result(state.tabs.insert(
                        wparam,
                        text.unwrap_or(&[]),
                        data.unwrap_or(0),
                    )),
                    _ => usize::from(state.tabs.set(wparam, text, data)),
                }
            })
            .unwrap_or_else(|| item_failed(number));
            refresh(window);
            result
        }
        TCM_FIRST..=TCM_LAST => {
            let answer = with_state::<TabState, _>(window, |entry, state| {
                tab_message(entry, state, number, wparam, lparam)
            });
            let Some((result, changed)) = answer else {
                return 0;
            };
            if changed {
                refresh(window);
            }
            result
        }
        _ => default_procedure(window, number, wparam, lparam),
    }
}

fn item_failed(number: u32) -> usize {
    match number {
        TCM_INSERTITEMW => NONE,
        _ => FALSE as usize,
    }
}

fn item_data(item: &TabItem, state: &TabState) -> usize {
    let bytes = state.extra_bytes.min(LPARAM_BYTES);
    let mut data = [0u8; LPARAM_BYTES];
    data[..bytes].copy_from_slice(&item.data.to_le_bytes()[..bytes]);
    usize::from_le_bytes(data)
}

fn write_rect(out: usize, rect: Rect) {
    // SAFETY: every caller passes the program's writable `RECT`, checked not null.
    unsafe {
        (out as *mut Win32Rect).write(Win32Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    };
}

fn tab_message(
    entry: &mut Window,
    state: &mut TabState,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> (usize, bool) {
    let tabs = &mut state.tabs;
    let style = entry.style;
    match number {
        TCM_GETITEMCOUNT => (tabs.count(), false),
        TCM_GETCURSEL | TCM_GETCURFOCUS => (index_result(tabs.selected()), false),
        TCM_SETCURSEL => match tabs.select(Some(wparam)) {
            Some(previous) => (index_result(previous), true),
            None => (NONE, false),
        },
        TCM_DELETEITEM => (usize::from(tabs.delete(wparam)), true),
        TCM_DELETEALLITEMS => {
            tabs.clear();
            (TRUE as usize, true)
        }
        TCM_GETITEMRECT => match tabs.item_rect(wparam, style) {
            Some(rect) if lparam != 0 => {
                write_rect(lparam, rect);
                (TRUE as usize, false)
            }
            _ => (FALSE as usize, false),
        },
        TCM_HITTEST if lparam != 0 => {
            // SAFETY: `TCM_HITTEST`'s contract: a readable and writable `TCHITTESTINFO`.
            let test = unsafe { &mut *(lparam as *mut HitTest) };
            let hit = tabs.hit(test.x, test.y, style, entry.width as i32);
            test.flags = match hit {
                Some(_) => TCHT_ONITEM,
                None => TCHT_NOWHERE,
            };
            (index_result(hit), false)
        }
        TCM_ADJUSTRECT if lparam != 0 => {
            // SAFETY: `TCM_ADJUSTRECT`'s contract: a readable and writable `RECT`.
            let given = unsafe { (lparam as *const Win32Rect).read() };
            let rect = Rect::new(given.left, given.top, given.right, given.bottom);
            write_rect(lparam, adjust_rect(rect, wparam != 0));
            (0, false)
        }
        TCM_GETROWCOUNT => (1, false),
        TCM_SETITEMSIZE => {
            let previous =
                message::word_pair(tabs.fixed_width as usize, tabs.fixed_height as usize);
            tabs.fixed_width = (lparam & 0xffff) as i32;
            tabs.fixed_height = ((lparam >> 16) & 0xffff) as i32;
            (previous, style & tab_style::FIXED_WIDTH != 0)
        }
        TCM_SETIMAGELIST => (core::mem::replace(&mut state.image_list, lparam), false),
        TCM_GETIMAGELIST => (state.image_list, false),
        TCM_SETITEMEXTRA if tabs.count() == 0 && wparam <= LPARAM_BYTES => {
            state.extra_bytes = wparam;
            (TRUE as usize, false)
        }
        TCM_GETITEMW if lparam != 0 => {
            if wparam >= tabs.count() {
                return (FALSE as usize, false);
            }
            // SAFETY: `TCM_GETITEMW`'s contract: a readable and writable `TCITEMW`.
            let item = unsafe { &mut *(lparam as *mut TabItem) };
            if item.mask & TCIF_TEXT != 0 && !item.text.is_null() {
                let capacity = usize::try_from(item.text_capacity).unwrap_or(0);
                // SAFETY: `pszText` holds `cchTextMax` writable units.
                unsafe {
                    super::super::window_text::copy_out(
                        tabs.text(wparam),
                        item.text as usize,
                        capacity,
                    )
                };
            }
            if item.mask & TCIF_PARAM != 0 {
                let bytes = state.extra_bytes.min(LPARAM_BYTES);
                let data = tabs.data(wparam).unwrap_or(0).to_le_bytes();
                let mut kept = item.data.to_le_bytes();
                kept[..bytes].copy_from_slice(&data[..bytes]);
                item.data = usize::from_le_bytes(kept);
            }
            if item.mask & TCIF_IMAGE != 0 {
                item.image = -1;
            }
            if item.mask & TCIF_STATE != 0 {
                item.state = 0;
            }
            (TRUE as usize, false)
        }
        _ => (0, false),
    }
}

fn left_button_down(window: usize, (x, y): (i32, i32)) {
    let target = with(|gui| {
        let entry = gui.windows.get(window)?;
        windows::enabled_in_tree(&gui.windows, window).then_some((entry.style, entry.width))
    })
    .flatten();
    let Some((style, width)) = target else {
        return;
    };
    if style & tab_style::FOCUS_NEVER == 0 {
        set_focus(window);
    }
    let hit =
        with_state::<TabState, _>(window, |_, state| state.tabs.hit(x, y, style, width as i32))
            .flatten();
    if let Some(index) = hit {
        select_with_notifications(window, index);
    }
}

fn key_down(window: usize, virtual_key: usize) {
    let (control, shift) = with(|gui| {
        let keys = gui.keyboard_state();
        (keys[VK_CONTROL] & HELD != 0, gui.shift_held())
    })
    .unwrap_or((false, false));
    let target = with_state::<TabState, _>(window, |_, state| {
        state.tabs.key_target(virtual_key as u32, control, shift)
    })
    .flatten();
    if let Some(index) = target {
        select_with_notifications(window, index);
    }
}

fn paint(window: usize) {
    paint_with::<TabState>(window, |surface, rect, entry, state, gui| {
        let look = TabLook {
            style: entry.style,
            focused: has_focus(gui, window),
            enabled: windows::enabled_in_tree(&gui.windows, window),
        };
        tab_control::paint(surface, rect, &state.tabs, look, &LIGHT, &COMMON_LIGHT);
    });
}

/// A tab control's role in agents' tree.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::RadioGroup, flags::FOCUSABLE)
}

/// Pushes a tab control's tabs, one radio button each, the selected one `SELECTED`; a tab the
/// row has no room for is there with height 0.
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    read_state::<TabState, _>(gui, window, |entry, state| {
        let tabs = &state.tabs;
        for index in 0..tabs.count().min(MAX_TABS) {
            let Some(rect) = tabs.item_rect(index, entry.style) else {
                break;
            };
            let height = match tabs.is_drawn(index, entry.style, entry.width as i32) {
                true => rect.height(),
                false => 0,
            };
            let bounds = Bounds::new(
                origin.0 + rect.left,
                origin.1 + rect.top,
                rect.width(),
                height,
            );
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(tabs.text(index), &mut name);
            let selected = match tabs.selected() == Some(index) {
                true => flags::SELECTED,
                false => 0,
            };
            let item = Element::new(item_element(window, index), element, Role::Radio, bounds)
                .named(name)
                .flagged(selected);
            if !tree.push(&item) {
                return false;
            }
        }
        true
    })
    .unwrap_or(true)
}

/// `select` or `invoke` on a tab selects it as a click does, with the notifications, on the
/// control's own thread; a refused selection still answers `Done`.
pub(super) fn act(window: usize, item: Option<usize>, action: Action, ours: bool) -> Outcome {
    let Some(index) = item else {
        return Outcome::NotSupported;
    };
    if !matches!(action, Action::Select | Action::Invoke) {
        return Outcome::NotSupported;
    }
    let found = with(|gui| {
        let thread = gui.windows.get(window)?.thread;
        read_state::<TabState, _>(gui, window, |_, state| index < state.tabs.count())
            .map(|exists| (thread, exists))
    })
    .flatten();
    let Some((thread, true)) = found else {
        return Outcome::NoSuchElement;
    };
    match ours {
        true => {
            gui::send(window, TCM_SETCURFOCUS, index, 0);
        }
        false => {
            let delivery = Delivery {
                window,
                thread,
                message: TCM_SETCURFOCUS,
                wparam: index,
                lparam: 0,
            };
            gui::deliver(&delivery, thread_environment::thread_id());
        }
    }
    Outcome::Done
}
