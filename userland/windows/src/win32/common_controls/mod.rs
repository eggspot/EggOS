//! comctl32: `InitCommonControls`, which programs import by its ordinal 17 (D174),
//! `InitCommonControlsEx`, and the common controls (D178, `docs/ux/windows-controls-v0.3.md`
//! sections 20-29): the status bar, progress bar, tab control, list view with its header, tree
//! view and toolbar are child windows whose procedure is the layer's, over the models of
//! `eggos_windows_gui::controls`, each model a heap block its window's entry points to, reached
//! only under the GUI state's lock through [`with_state`] and [`read_state`]; image lists
//! (`image_list.rs`) are the layer's own handles. Notifications are `WM_NOTIFY`s sent to the
//! parent ([`notify_parent`]). Their rows, items, tabs, parts and buttons are elements of
//! agents' tree with ids from [`item_element`].

mod header;
mod image_list;
mod list_view;
mod progress;
mod status_bar;
mod tab_control;
mod toolbar;
mod tree_view;

use super::gui::{self, Gui, with};
use super::{Bool, TRUE, heap, paint};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, ControlKind, Procedure, Window};
use crate::library::Export;
use crate::text::equal_ignoring_ascii_case;
use eggos_element_tree::{Action, MAX_LABEL_BYTES, Outcome, Role, TreeWriter};
use eggos_windows_gui::surface::{Rect, Surface};

const INIT_COMMON_CONTROLS_ORDINAL: u16 = 17;
const ITEM_ELEMENT_BASE: u32 = 0x4000_0000;
const ITEM_ELEMENT_SLOT_SHIFT: u32 = 16;
const ITEM_ELEMENT_ITEMS: usize = 1 << ITEM_ELEMENT_SLOT_SHIFT;

/// `WM_NOTIFY`'s number.
pub const WM_NOTIFY: u32 = 0x004e;
/// `WM_NOTIFYFORMAT`'s number and the answer common controls give it: Unicode.
pub const WM_NOTIFYFORMAT: u32 = 0x0055;
const NFR_UNICODE: usize = 2;

/// The notification codes every common control sends (`NM_*`).
pub mod code {
    pub const NM_CLICK: i32 = -2;
    pub const NM_DBLCLK: i32 = -3;
    pub const NM_RETURN: i32 = -4;
    pub const NM_SETFOCUS: i32 = -7;
    pub const NM_KILLFOCUS: i32 = -8;
}

/// The styles every common control reads (`CCS_*`).
pub mod style {
    pub const CCS_TOP: u32 = 0x1;
    pub const CCS_BOTTOM: u32 = 0x3;
    pub const CCS_NORESIZE: u32 = 0x4;
    pub const CCS_NOPARENTALIGN: u32 = 0x8;
}

#[repr(C)]
struct InitCommonControlsEx {
    size: u32,
    classes: u32,
}

/// Win32's `NMHDR`, the start of every notification's structure.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotifyHeader {
    pub from: usize,
    pub id: usize,
    pub code: u32,
}

/// A layer model type and the common control class whose state it is, so [`with_state`] only
/// ever reads a control's block as its own model.
pub trait Model {
    const KIND: CommonKind;
}

/// The common control class `name` names, compared ignoring case.
pub fn class_of(name: &[u16]) -> Option<CommonKind> {
    let is = |wanted: &str| {
        name.len() == wanted.len()
            && name.iter().all(|unit| *unit < 0x80)
            && equal_ignoring_ascii_case(name.iter().map(|unit| *unit as u8), wanted.bytes())
    };
    match () {
        () if is("msctls_statusbar32") => Some(CommonKind::StatusBar),
        () if is("msctls_progress32") => Some(CommonKind::Progress),
        () if is("SysTabControl32") => Some(CommonKind::TabControl),
        () if is("SysListView32") => Some(CommonKind::ListView),
        () if is("SysHeader32") => Some(CommonKind::Header),
        () if is("SysTreeView32") => Some(CommonKind::TreeView),
        () if is("ToolbarWindow32") => Some(CommonKind::Toolbar),
        () => None,
    }
}

/// Puts `value` in a new heap block: a new control's state; `None` without memory.
pub fn allocate<T: Model>(value: T) -> Option<usize> {
    let block = heap::allocate(core::mem::size_of::<T>()).cast::<T>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, 16-byte aligned heap block the size of `T`.
    unsafe { block.write(value) };
    Some(block as usize)
}

/// A new control's state for a window of `kind` with `style`; `None` without memory.
pub fn new_state(kind: CommonKind, style: u32) -> Option<usize> {
    match kind {
        CommonKind::StatusBar => status_bar::new_state(style),
        CommonKind::Progress => progress::new_state(style),
        CommonKind::TabControl => tab_control::new_state(style),
        CommonKind::ListView => list_view::new_state(style),
        CommonKind::Header => header::new_state(style),
        CommonKind::TreeView => tree_view::new_state(style),
        CommonKind::Toolbar => toolbar::new_state(style),
    }
}

fn state_address<T: Model>(window: &Window) -> Option<usize> {
    match window.procedure {
        Procedure::Control(ControlKind::Common(kind), address)
            if kind == T::KIND && address != 0 =>
        {
            Some(address)
        }
        _ => None,
    }
}

/// Runs `work` on `window`'s entry and its model of type `T` under the state's lock; `None`
/// when the window is gone or is not a control of `T`'s class. `work` must not call the
/// program.
pub fn with_state<T: Model, R>(
    window: usize,
    work: impl FnOnce(&mut Window, &mut T) -> R,
) -> Option<R> {
    with(|gui| {
        let entry = gui.windows.get_mut(window)?;
        let address = state_address::<T>(entry)?;
        // SAFETY: a common control's state is the heap block `new_state` made for its class
        // (`T::KIND` checked above), freed only after its window left the table; the lock is
        // held through `gui`, and every mutable access goes through here.
        let model = unsafe { &mut *(address as *mut T) };
        Some(work(entry, model))
    })
    .flatten()
}

/// Runs `work` on `window`'s entry and its model of type `T` while the caller holds the lock
/// through `gui`.
pub fn read_state<'g, T: Model, R>(
    gui: &'g Gui,
    window: usize,
    work: impl FnOnce(&'g Window, &'g T) -> R,
) -> Option<R> {
    let entry = gui.windows.get(window)?;
    let address = state_address::<T>(entry)?;
    // SAFETY: as `with_state`; only a shared reference is made, for as long as `gui` is
    // borrowed, so no mutable one exists meanwhile.
    let model = unsafe { &*(address as *const T) };
    Some(work(entry, model))
}

/// Paints `window` on `WM_PAINT`: `draw` gets the window's surface (translated to its client
/// area and clipped to it), its client rectangle, its entry, its model and the GUI state.
pub fn paint_with<T: Model>(
    window: usize,
    draw: impl FnOnce(&mut Surface<'_>, Rect, &Window, &T, &Gui),
) {
    let Ok((hdc, _, _)) = paint::begin(window) else {
        return;
    };
    let _ = paint::draw_in(hdc, |surface, _, gui| {
        let Some(surface) = surface else {
            return;
        };
        read_state::<T, ()>(gui, window, |entry, model| {
            draw(surface, entry.client(), entry, model, gui)
        });
    });
    paint::finish(hdc);
}

/// Adds `window`'s whole client area to what waits to be painted and wakes the message loop.
pub fn invalidate(window: usize) {
    super::controls::invalidate(window);
}

/// Sends `window`'s parent `WM_NOTIFY` with notification `code`: `build` makes the structure
/// from the filled `NMHDR` (`hwndFrom` the control, `idFrom` its id); the parent's answer, 0
/// when the window has no parent.
pub fn notify_parent<T>(window: usize, code: i32, build: impl FnOnce(NotifyHeader) -> T) -> usize {
    let target = with(|gui| {
        let entry = gui.windows.get(window)?;
        Some((entry.parent, entry.id))
    })
    .flatten();
    let Some((parent, id)) = target.filter(|(parent, _)| *parent != 0) else {
        return 0;
    };
    let header = NotifyHeader {
        from: window,
        id,
        code: code as u32,
    };
    let mut data = build(header);
    gui::send(parent, WM_NOTIFY, id, &raw mut data as usize)
}

/// `WM_COMMAND` from `window` to its parent with `command` in its low word, as a toolbar's
/// button click sends it.
pub fn command_parent(window: usize, command: usize) {
    let parent = with(|gui| gui.windows.get(window).map(|entry| entry.parent)).flatten();
    if let Some(parent) = parent.filter(|parent| *parent != 0) {
        gui::send(
            parent,
            message::COMMAND,
            message::word_pair(command, 0),
            window,
        );
    }
}

/// What every common control answers alike, for the messages its own procedure leaves:
/// `WM_NOTIFYFORMAT` with Unicode, then `DefWindowProcW`.
pub fn default_procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        WM_NOTIFYFORMAT => NFR_UNICODE,
        message::ERASE_BACKGROUND => 1,
        _ => super::user32::def_window_proc_w(window, number, wparam, lparam),
    }
}

/// The window procedure of every common control of `kind`.
pub fn procedure(
    window: usize,
    kind: CommonKind,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match kind {
        CommonKind::StatusBar => status_bar::procedure(window, number, wparam, lparam),
        CommonKind::Progress => progress::procedure(window, number, wparam, lparam),
        CommonKind::TabControl => tab_control::procedure(window, number, wparam, lparam),
        CommonKind::ListView => list_view::procedure(window, number, wparam, lparam),
        CommonKind::Header => header::procedure(window, number, wparam, lparam),
        CommonKind::TreeView => tree_view::procedure(window, number, wparam, lparam),
        CommonKind::Toolbar => toolbar::procedure(window, number, wparam, lparam),
    }
}

/// A common control's role and flags in agents' tree (design section 29).
pub fn role_of(kind: CommonKind, window: &Window) -> (Role, u16) {
    match kind {
        CommonKind::StatusBar => status_bar::role(window),
        CommonKind::Progress => progress::role(window),
        CommonKind::TabControl => tab_control::role(window),
        CommonKind::ListView => list_view::role(window),
        CommonKind::Header => header::role(window),
        CommonKind::TreeView => tree_view::role(window),
        CommonKind::Toolbar => toolbar::role(window),
    }
}

/// A common control's value in agents' tree (the progress bar's per mille), written into
/// `out`; the bytes written.
pub fn value_of(gui: &Gui, kind: CommonKind, window: usize, out: &mut [u8]) -> usize {
    match kind {
        CommonKind::Progress => progress::value(gui, window, out),
        _ => 0,
    }
}

/// Pushes the elements inside common control `window` (its rows, items, tabs, parts or
/// buttons) under its element `element`, whose content bounds start at `origin`; false when
/// the tree is full.
pub fn push_items(
    gui: &Gui,
    kind: CommonKind,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    match kind {
        CommonKind::StatusBar => status_bar::push_items(gui, window, element, origin, tree),
        CommonKind::Progress => true,
        CommonKind::TabControl => tab_control::push_items(gui, window, element, origin, tree),
        CommonKind::ListView => list_view::push_items(gui, window, element, origin, tree),
        CommonKind::Header => header::push_items(gui, window, element, origin, tree),
        CommonKind::TreeView => tree_view::push_items(gui, window, element, origin, tree),
        CommonKind::Toolbar => toolbar::push_items(gui, window, element, origin, tree),
    }
}

/// Does `action` on common control `window` (`item` one of its items, by the index
/// [`item_element`] encoded) for an agent; `ours` when the control's thread is the one reading
/// the server's records, which may call the program.
pub fn act(
    kind: CommonKind,
    window: usize,
    item: Option<usize>,
    action: Action,
    ours: bool,
) -> Outcome {
    match kind {
        CommonKind::StatusBar | CommonKind::Progress => Outcome::NotSupported,
        CommonKind::TabControl => tab_control::act(window, item, action, ours),
        CommonKind::ListView => list_view::act(window, item, action, ours),
        CommonKind::Header => header::act(window, item, action, ours),
        CommonKind::TreeView => tree_view::act(window, item, action, ours),
        CommonKind::Toolbar => toolbar::act(window, item, action, ours),
    }
}

/// The element id of item `item` (below 65536) of control `window`.
pub fn item_element(window: usize, item: usize) -> u32 {
    let item = item.min(ITEM_ELEMENT_ITEMS - 1) as u32;
    ITEM_ELEMENT_BASE | ((windows::slot_of(window) as u32) << ITEM_ELEMENT_SLOT_SHIFT) | item
}

/// The control in `root`'s tree and the item an element id of [`item_element`] names.
pub fn element_item(gui: &Gui, root: usize, element: u32) -> Option<(usize, usize)> {
    if element & ITEM_ELEMENT_BASE == 0 || element & !(ITEM_ELEMENT_BASE | 0x3fff_ffff) != 0 {
        return None;
    }
    let slot = ((element & !ITEM_ELEMENT_BASE) >> ITEM_ELEMENT_SLOT_SHIFT) as usize;
    let item = (element as usize) & (ITEM_ELEMENT_ITEMS - 1);
    let (handles, count) = windows::tree(&gui.windows, root);
    let window = handles[..count]
        .iter()
        .copied()
        .find(|&handle| windows::slot_of(handle) == slot && handle != root)?;
    Some((window, item))
}

/// The common control kind of `window`, if it is one.
pub fn kind_of(window: &Window) -> Option<CommonKind> {
    match window.procedure {
        Procedure::Control(ControlKind::Common(kind), _) => Some(kind),
        _ => None,
    }
}

/// The UTF-16 `units` as UTF-8 in `out`, cut at a character that would not fit: an element's
/// name or value.
pub fn utf8<'b>(units: &[u16], out: &'b mut [u8; MAX_LABEL_BYTES]) -> &'b [u8] {
    let mut length = 0;
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut encoded = [0u8; 4];
        let encoded = character.encode_utf8(&mut encoded).as_bytes();
        if length + encoded.len() > out.len() {
            break;
        }
        out[length..length + encoded.len()].copy_from_slice(encoded);
        length += encoded.len();
    }
    &out[..length]
}

extern "efiapi" fn init_common_controls() {}

unsafe extern "efiapi" fn init_common_controls_ex(request: *const InitCommonControlsEx) -> Bool {
    if request.is_null() {
        return super::fail(super::error::INVALID_PARAMETER, super::FALSE);
    }
    TRUE
}

/// comctl32's functions by name.
pub(super) const EXPORTS: &[Export] = exports![
    b"InitCommonControls" => init_common_controls,
    b"InitCommonControlsEx" => init_common_controls_ex,
];

/// Every list of comctl32's functions.
pub(super) const EXPORT_LISTS: &[&[Export]] = &[EXPORTS, image_list::EXPORTS, status_bar::EXPORTS];

/// comctl32's function at `ordinal`, if the layer has it.
pub(super) fn by_ordinal(ordinal: u16) -> Option<*const ()> {
    match ordinal {
        INIT_COMMON_CONTROLS_ORDINAL => crate::library::find(&[EXPORTS], b"InitCommonControls"),
        _ => None,
    }
}
