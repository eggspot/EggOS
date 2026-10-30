//! A Windows program's windows as element trees for agents (D161, D162; `wm show`, `wm act`):
//! the window server's `Query` record reaches the thread reading its records, which answers
//! it once the records are handled. A top-level window is the root element, named by its
//! text; its child windows follow in pre-order, each an element whose id is its `HWND`, with
//! its control's role (a button, check box, radio button or group box by its style, an edit
//! control as a text field, a static as a label, a list box or combo box as a list, any other
//! window as a group), its text (an edit control's and a combo box's chosen item as the value),
//! its rectangle in the content, and whether it is disabled, focused or the default button;
//! below each window, the text the program drew in it with `TextOutW` and `DrawTextW`
//! (`gui::drawn`). `invoke` clicks a button as `BM_CLICK` does, `focus` is `SetFocus` and
//! `set-text` is `WM_SETTEXT` on an edit control, the last two for windows of the thread that
//! reads the records.

use super::gui::{self, Delivery, Gui, with};
use super::window_device::Device;
use super::{dialog, heap, thread_environment, window_text};
use crate::gui::drawn::MAX_DRAWN;
use crate::gui::input::Record;
use crate::gui::message;
use crate::gui::windows::{self, ControlKind, MAX_DEPTH, Procedure, Window};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, Role,
    TreeWriter, flags, query_kind, tree_answer,
};

/// The top-level window's element.
pub const ROOT_ELEMENT: u32 = 1;
/// Drawn text piece `i` of the program's store is element `FIRST_DRAWN_ELEMENT + i`.
pub const FIRST_DRAWN_ELEMENT: u32 = 0x100;

const BUTTON_TYPE: u32 = 0x0f;
const DEFAULT_PUSH_BUTTON: u32 = 1;
const CHECK_BOXES: [u32; 4] = [2, 3, 5, 6];
const RADIO_BUTTONS: [u32; 2] = [4, 9];
const GROUP_BOX: u32 = 7;
const BUTTON_CLICK: u32 = 0x00f5;

/// Answers the query in `record` for its window, read by thread `thread`.
pub fn answer(device: &Device, record: &Record, thread: u32) {
    let serial = record.a as u32;
    let root = with(|gui| windows::by_server(&gui.windows, u32::from(record.window))).flatten();
    if record.b as u32 == query_kind::TREE {
        match root {
            Some(root) => answer_tree(device, serial, root),
            None => {
                let _ = device.answer_query(serial, tree_answer::PIXELS_ONLY, &[]);
            }
        }
        return;
    }
    let outcome = match (root, Action::from_value(record.d)) {
        (None, _) => Outcome::NoSuchElement,
        (Some(_), None) => Outcome::NotSupported,
        (Some(root), Some(action)) => act(device, serial, root, record.c, action, thread),
    };
    let _ = device.answer_query(serial, outcome.value(), &[]);
}

fn answer_tree(device: &Device, serial: u32, root: usize) {
    let block = heap::allocate(MAX_TREE_BYTES);
    if block.is_null() {
        let _ = device.answer_query(serial, tree_answer::PIXELS_ONLY, &[]);
        return;
    }
    // SAFETY: `allocate` gave a block of `MAX_TREE_BYTES` bytes, this function's until it is
    // freed below.
    let bytes = unsafe { core::slice::from_raw_parts_mut(block, MAX_TREE_BYTES) };
    let length = with(|gui| {
        let mut tree = TreeWriter::new(&mut *bytes);
        describe(gui, root, &mut tree);
        tree.bytes().len()
    })
    .unwrap_or(0);
    let _ = device.answer_query(serial, tree_answer::TREE, &bytes[..length]);
    // SAFETY: the block came from `allocate` and nothing uses it any more.
    unsafe { heap::free(block) };
}

fn utf8<'b>(units: &[u16], out: &'b mut [u8; MAX_LABEL_BYTES]) -> &'b [u8] {
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

fn describe(gui: &Gui, root: usize, tree: &mut TreeWriter<'_>) {
    let Some(window) = gui.windows.get(root) else {
        return;
    };
    let mut title = [0u8; MAX_LABEL_BYTES];
    let title = utf8(window_text::of(gui, root), &mut title);
    let content = Bounds::new(0, 0, window.width as i32, window.height as i32);
    let element = Element::new(ROOT_ELEMENT, 0, Role::Window, content).named(title);
    if tree.push(&element) && push_drawn(gui, root, ROOT_ELEMENT, tree) {
        push_children(gui, root, root, ROOT_ELEMENT, 1, tree);
    }
}

fn role_of(window: &Window) -> (Role, u16) {
    match window.procedure {
        Procedure::Control(ControlKind::Button, _) => match window.style & BUTTON_TYPE {
            kind if CHECK_BOXES.contains(&kind) => (Role::Checkbox, flags::FOCUSABLE),
            kind if RADIO_BUTTONS.contains(&kind) => (Role::Radio, flags::FOCUSABLE),
            GROUP_BOX => (Role::Group, 0),
            DEFAULT_PUSH_BUTTON => (Role::Button, flags::FOCUSABLE | flags::DEFAULT),
            _ => (Role::Button, flags::FOCUSABLE),
        },
        Procedure::Control(ControlKind::Edit, _) => (Role::TextField, flags::FOCUSABLE),
        Procedure::Control(ControlKind::Static, _) => (Role::Label, 0),
        Procedure::Control(ControlKind::ListBox | ControlKind::ComboBox, _) => {
            (Role::List, flags::FOCUSABLE)
        }
        _ => (Role::Group, 0),
    }
}

fn push_children(
    gui: &Gui,
    root: usize,
    parent: usize,
    parent_element: u32,
    depth: usize,
    tree: &mut TreeWriter<'_>,
) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    let focused = gui.windows.get(root).map_or(0, |window| window.focus);
    let (kids, count) = windows::children(&gui.windows, parent);
    for &kid in &kids[..count] {
        let Some(window) = gui.windows.get(kid).filter(|window| window.shown) else {
            continue;
        };
        let Some(placement) = windows::client_placement(&gui.windows, kid) else {
            continue;
        };
        let (role, mut state) = role_of(window);
        if !windows::enabled_in_tree(&gui.windows, kid) {
            state |= flags::DISABLED;
        }
        if kid == focused && gui.active_root == root {
            state |= flags::FOCUSED;
        }
        let is_combo = matches!(
            window.procedure,
            Procedure::Control(ControlKind::ComboBox, _)
        );
        let source = match is_combo {
            true => super::combo_box::shown_text(gui, kid),
            false => window_text::of(gui, kid),
        };
        let mut text = [0u8; MAX_LABEL_BYTES];
        let text = utf8(source, &mut text);
        let (name, value): (&[u8], &[u8]) = match role {
            Role::TextField => (&[], text),
            _ if is_combo => (&[], text),
            _ => (text, &[]),
        };
        let bounds = Bounds::new(
            placement.origin.0,
            placement.origin.1,
            window.bounds.width(),
            window.bounds.height(),
        );
        let id = kid as u32;
        let element = Element::new(id, parent_element, role, bounds)
            .named(name)
            .valued(value)
            .flagged(state);
        if !tree.push(&element)
            || !push_drawn(gui, kid, id, tree)
            || !push_children(gui, root, kid, id, depth + 1, tree)
        {
            return false;
        }
    }
    true
}

fn push_drawn(gui: &Gui, window: usize, parent_element: u32, tree: &mut TreeWriter<'_>) -> bool {
    let Some(placement) = windows::client_placement(&gui.windows, window) else {
        return true;
    };
    for (slot, piece) in gui.drawn.of(window) {
        let bounds = Bounds::new(
            placement.origin.0.saturating_add(piece.rect.left),
            placement.origin.1.saturating_add(piece.rect.top),
            piece.rect.width(),
            piece.rect.height(),
        );
        let element = Element::new(
            FIRST_DRAWN_ELEMENT + slot as u32,
            parent_element,
            Role::Text,
            bounds,
        )
        .named(piece.text());
        if !tree.push(&element) {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy)]
struct Target {
    procedure: Procedure,
    style: u32,
    thread: u32,
    enabled: bool,
}

fn act(
    device: &Device,
    serial: u32,
    root: usize,
    element: u32,
    action: Action,
    thread: u32,
) -> Outcome {
    let drawn = element
        .checked_sub(FIRST_DRAWN_ELEMENT)
        .is_some_and(|slot| (slot as usize) < MAX_DRAWN);
    if element == ROOT_ELEMENT || drawn {
        return Outcome::NotSupported;
    }
    let handle = element as usize;
    let target = with(|gui| {
        if handle == root || !windows::is_in_tree(&gui.windows, root, handle) {
            return None;
        }
        let window = gui.windows.get(handle)?;
        Some(Target {
            procedure: window.procedure,
            style: window.style,
            thread: window.thread,
            enabled: windows::enabled_in_tree(&gui.windows, handle),
        })
    })
    .flatten();
    let Some(target) = target else {
        return Outcome::NoSuchElement;
    };
    if !target.enabled {
        return Outcome::Disabled;
    }
    let ours = target.thread == thread && thread == thread_environment::thread_id();
    match (action, target.procedure) {
        (Action::Invoke, Procedure::Control(ControlKind::Button, _))
            if target.style & BUTTON_TYPE != GROUP_BOX =>
        {
            let click = Delivery {
                window: handle,
                thread: target.thread,
                message: BUTTON_CLICK,
                wparam: 0,
                lparam: 0,
            };
            gui::deliver(&click, thread);
            Outcome::Done
        }
        (Action::Focus, _) if ours => {
            dialog::set_focus(handle);
            Outcome::Done
        }
        (Action::SetText, Procedure::Control(ControlKind::Edit, _)) if ours => {
            let mut text = [0u8; MAX_TEXT_BYTES];
            let length = device.query_text(serial, &mut text).unwrap_or(0);
            let text = core::str::from_utf8(&text[..length]).unwrap_or("");
            let mut units = [0u16; MAX_TEXT_BYTES + 1];
            for (slot, unit) in units.iter_mut().zip(text.encode_utf16()) {
                *slot = unit;
            }
            gui::send(handle, message::SET_TEXT, 0, units.as_ptr() as usize);
            Outcome::Done
        }
        _ => Outcome::NotSupported,
    }
}
