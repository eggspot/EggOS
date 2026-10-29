//! What windows built from controls need (D145): the keyboard focus and the mouse capture of
//! each top-level window's tree, enabling, moving child windows, the dialog functions that name
//! controls by id, and `IsDialogMessageW`'s Tab, Enter and Esc.

use super::gui::{self, try_with, with};
use super::user32::{Msg, dispatch, read_message, translate};
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::gui::message;
use crate::gui::windows::{self, ControlKind, Procedure};
use crate::library::Export;
use eggos_windows_gui::controls::button::{ButtonKind, style as button_style};
use eggos_windows_gui::surface::Rect;

const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0d;
const VK_ESCAPE: usize = 0x1b;
const BN_CLICKED: usize = 0;

fn push_kind(gui: &gui::Gui, window: usize) -> Option<ButtonKind> {
    let entry = gui.windows.get(window)?;
    match entry.procedure {
        Procedure::Control(ControlKind::Button, _) => {
            Some(ButtonKind::of(entry.style)).filter(|kind| kind.is_push())
        }
        _ => None,
    }
}

fn repaint_push_buttons(gui: &mut gui::Gui, root: usize) {
    let (handles, count) = windows::tree(&gui.windows, root);
    for &handle in &handles[..count] {
        if push_kind(gui, handle).is_some() {
            windows::invalidate_tree(&mut gui.windows, handle, None, true);
        }
    }
}

/// `SetFocus`: `window` gets the keys of its top-level window's tree; the window that had them
/// gets `WM_KILLFOCUS` and `window` `WM_SETFOCUS`, unless it already had them. The previous
/// one.
pub fn set_focus(window: usize) -> usize {
    let change = with(|gui| {
        let root = windows::root_of(&gui.windows, window)?;
        let old = gui.key_target(root);
        if old == window {
            return Some((old, false));
        }
        gui.windows.get_mut(root)?.focus = window;
        if push_kind(gui, old).is_some() || push_kind(gui, window).is_some() {
            repaint_push_buttons(gui, root);
        }
        Some((old, true))
    })
    .flatten();
    let Some((old, changed)) = change else {
        return fail(error::INVALID_WINDOW_HANDLE, 0);
    };
    if changed {
        gui::send(old, message::KILL_FOCUS, window, 0);
        gui::send(window, message::SET_FOCUS, old, 0);
        gui::wake();
    }
    old
}

/// Whether `window` has the keys of its tree.
pub fn has_focus(gui: &gui::Gui, window: usize) -> bool {
    windows::root_of(&gui.windows, window).is_some_and(|root| gui.key_target(root) == window)
}

extern "efiapi" fn set_focus_export(window: usize) -> usize {
    match window {
        0 => 0,
        window => set_focus(window),
    }
}

extern "efiapi" fn get_focus() -> usize {
    with(|gui| match gui.windows.get(gui.active_root) {
        Some(_) => gui.key_target(gui.active_root),
        None => 0,
    })
    .unwrap_or(0)
}

/// `SetCapture`: the pointer's messages of `window`'s tree go to `window` until released.
pub fn set_capture(window: usize) -> usize {
    with(|gui| {
        let root = windows::root_of(&gui.windows, window)?;
        let entry = gui.windows.get_mut(root)?;
        Some(core::mem::replace(&mut entry.capture, window))
    })
    .flatten()
    .unwrap_or(0)
}

/// `ReleaseCapture` for the calling thread's windows.
pub fn release_capture() -> Bool {
    let thread = thread_environment::thread_id();
    with(|gui| {
        let mut roots = [0; windows::MAX_WINDOWS];
        let mut count = 0;
        for (handle, window) in gui.windows.iter() {
            if !window.is_child() && window.capture != 0 {
                roots[count] = handle;
                count += 1;
            }
        }
        for &root in &roots[..count] {
            let capture = gui.windows.get(root).map_or(0, |window| window.capture);
            let mine = gui
                .windows
                .get(capture)
                .is_none_or(|window| window.thread == thread);
            if mine && let Some(window) = gui.windows.get_mut(root) {
                window.capture = 0;
            }
        }
    });
    TRUE
}

extern "efiapi" fn set_capture_export(window: usize) -> usize {
    set_capture(window)
}

extern "efiapi" fn release_capture_export() -> Bool {
    release_capture()
}

extern "efiapi" fn get_capture() -> usize {
    let thread = thread_environment::thread_id();
    with(|gui| {
        gui.windows
            .iter()
            .filter(|(_, window)| !window.is_child())
            .map(|(_, window)| window.capture)
            .find(|&capture| {
                gui.windows
                    .get(capture)
                    .is_some_and(|window| window.thread == thread)
            })
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// `EnableWindow`: whether the window was disabled before.
pub fn enable(window: usize, enabled: bool) -> Result<bool, u32> {
    let mut lost_focus = 0;
    let (was_disabled, changed) = try_with(|gui| {
        let entry = gui.window_mut(window)?;
        let was_disabled = !entry.enabled;
        entry.enabled = enabled;
        if was_disabled == enabled {
            windows::invalidate_tree(&mut gui.windows, window, None, true);
        }
        if !enabled && let Some(root) = windows::root_of(&gui.windows, window) {
            let (focus, capture) = gui
                .windows
                .get(root)
                .map_or((0, 0), |top| (top.focus, top.capture));
            let inside =
                |handle: usize| handle != 0 && windows::is_in_tree(&gui.windows, window, handle);
            let (drop_focus, drop_capture) = (inside(focus), inside(capture));
            if let Some(top) = gui.windows.get_mut(root) {
                if drop_focus {
                    lost_focus = top.focus;
                    top.focus = 0;
                }
                if drop_capture {
                    top.capture = 0;
                }
            }
        }
        Ok((was_disabled, was_disabled == enabled))
    })?;
    if lost_focus != 0 {
        gui::send(lost_focus, message::KILL_FOCUS, 0, 0);
    }
    if changed {
        gui::send(window, message::ENABLE, usize::from(enabled), 0);
        gui::wake();
    }
    Ok(was_disabled)
}

extern "efiapi" fn enable_window(window: usize, enabled: Bool) -> Bool {
    match enable(window, enabled != 0) {
        Ok(was_disabled) => Bool::from(was_disabled),
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn is_window_enabled(window: usize) -> Bool {
    match try_with(|gui| gui.window(window).map(|window| window.enabled)) {
        Ok(enabled) => Bool::from(enabled),
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn is_window_visible(window: usize) -> Bool {
    let visible = with(|gui| {
        let (chain, length) = windows::ancestry(&gui.windows, window)?;
        Some(
            chain[..length]
                .iter()
                .all(|&at| gui.windows.get(at).is_some_and(|window| window.shown)),
        )
    })
    .flatten();
    Bool::from(visible == Some(true))
}

extern "efiapi" fn get_parent(window: usize) -> usize {
    match try_with(|gui| {
        gui.window(window).map(|entry| match entry.is_child() {
            true => entry.parent,
            false => entry.owner,
        })
    }) {
        Ok(parent) => parent,
        Err(code) => fail(code, 0),
    }
}

/// The child of `dialog` with control id `id`.
pub fn dialog_item(dialog: usize, id: i32) -> Option<usize> {
    with(|gui| {
        let (kids, count) = windows::children(&gui.windows, dialog);
        kids[..count].iter().copied().find(|&kid| {
            gui.windows
                .get(kid)
                .is_some_and(|window| window.id as i32 == id)
        })
    })
    .flatten()
}

extern "efiapi" fn get_dlg_item(dialog: usize, id: i32) -> usize {
    match dialog_item(dialog, id) {
        Some(item) => item,
        None => fail(error::CONTROL_ID_NOT_FOUND, 0),
    }
}

extern "efiapi" fn get_dlg_ctrl_id(window: usize) -> i32 {
    match try_with(|gui| gui.window(window).map(|window| window.id as i32)) {
        Ok(id) => id,
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn set_dlg_item_text_w(dialog: usize, id: i32, text: *const u16) -> Bool {
    match dialog_item(dialog, id) {
        Some(item) => Bool::from(gui::send(item, message::SET_TEXT, 0, text as usize) != 0),
        None => fail(error::CONTROL_ID_NOT_FOUND, FALSE),
    }
}

unsafe extern "efiapi" fn get_dlg_item_text_w(
    dialog: usize,
    id: i32,
    buffer: *mut u16,
    capacity: i32,
) -> u32 {
    match dialog_item(dialog, id) {
        Some(item) => gui::send(
            item,
            message::GET_TEXT,
            capacity.max(0) as usize,
            buffer as usize,
        ) as u32,
        None => fail(error::CONTROL_ID_NOT_FOUND, 0),
    }
}

extern "efiapi" fn send_dlg_item_message_w(
    dialog: usize,
    id: i32,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match dialog_item(dialog, id) {
        Some(item) => gui::send(item, number, wparam, lparam),
        None => fail(error::CONTROL_ID_NOT_FOUND, 0),
    }
}

extern "efiapi" fn move_window(
    window: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    _repaint: Bool,
) -> Bool {
    let moved = try_with(|gui| {
        let entry = gui.window_mut(window)?;
        if !entry.is_child() {
            return Err(error::NOT_SUPPORTED);
        }
        let old = entry.bounds;
        let bounds = Rect::sized(x, y, width.max(0), height.max(0));
        entry.bounds = bounds;
        entry.width = bounds.width() as u32;
        entry.height = bounds.height() as u32;
        let parent = entry.parent;
        let resized = old.width() != bounds.width() || old.height() != bounds.height();
        windows::invalidate_tree(&mut gui.windows, parent, Some(old.union(&bounds)), true);
        Ok(resized.then_some(message::point_parameter(bounds.width(), bounds.height())))
    });
    match moved {
        Ok(resized) => {
            if let Some(size) = resized {
                gui::send(window, message::SIZE, message::SIZE_RESTORED, size);
            }
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

fn default_button(dialog: usize) -> Option<(usize, usize)> {
    with(|gui| {
        let root = windows::root_of(&gui.windows, dialog)?;
        let focus = gui.key_target(root);
        if push_kind(gui, focus).is_some() && windows::is_in_tree(&gui.windows, dialog, focus) {
            return Some((focus, gui.windows.get(focus)?.id));
        }
        let (handles, count) = windows::tree(&gui.windows, dialog);
        handles[..count].iter().find_map(|&handle| {
            let entry = gui.windows.get(handle)?;
            let default = push_kind(gui, handle) == Some(ButtonKind::DefaultPush)
                && entry.style & button_style::TYPE == button_style::DEFAULT_PUSH_BUTTON;
            default.then_some((handle, entry.id))
        })
    })
    .flatten()
}

/// What `IsDialogMessageW` does with `message` for `dialog`: Tab and Shift+Tab move the focus
/// over the tab stops, Enter is the default button or `IDOK`, Esc `IDCANCEL`; any other message
/// of the dialog's tree is translated and dispatched. Whether it was the dialog's.
pub fn dialog_message(dialog: usize, message: &crate::gui::queue::Message) -> bool {
    let inside =
        with(|gui| windows::is_in_tree(&gui.windows, dialog, message.window)).unwrap_or(false);
    if !inside {
        return false;
    }
    match (message.message, message.wparam) {
        (message::KEY_DOWN, VK_TAB) => {
            let next = with(|gui| {
                let root = windows::root_of(&gui.windows, dialog)?;
                let from = gui.key_target(root);
                windows::next_tab_stop(&gui.windows, dialog, from, gui.shift_held())
            })
            .flatten();
            if let Some(next) = next {
                set_focus(next);
            }
        }
        (message::KEY_DOWN, VK_RETURN) => {
            let (button, id) = default_button(dialog).unwrap_or((0, message::ID_OK));
            gui::send(
                dialog,
                message::COMMAND,
                message::word_pair(id, BN_CLICKED),
                button,
            );
        }
        (message::KEY_DOWN, VK_ESCAPE) => {
            gui::send(dialog, message::COMMAND, message::ID_CANCEL, 0);
        }
        (message::CHAR, VK_TAB | VK_RETURN | VK_ESCAPE) => {}
        _ => {
            translate(message);
            dispatch(message);
        }
    }
    true
}

unsafe extern "efiapi" fn is_dialog_message_w(dialog: usize, message: *const Msg) -> Bool {
    // SAFETY: the caller passes a readable `MSG`.
    let message = unsafe { read_message(message) };
    Bool::from(dialog_message(dialog, &message))
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetFocus" => set_focus_export,
    b"GetFocus" => get_focus,
    b"SetCapture" => set_capture_export,
    b"ReleaseCapture" => release_capture_export,
    b"GetCapture" => get_capture,
    b"EnableWindow" => enable_window,
    b"IsWindowEnabled" => is_window_enabled,
    b"IsWindowVisible" => is_window_visible,
    b"GetParent" => get_parent,
    b"GetDlgItem" => get_dlg_item,
    b"GetDlgCtrlID" => get_dlg_ctrl_id,
    b"SetDlgItemTextW" => set_dlg_item_text_w,
    b"GetDlgItemTextW" => get_dlg_item_text_w,
    b"SendDlgItemMessageW" => send_dlg_item_message_w,
    b"MoveWindow" => move_window,
    b"IsDialogMessageW" => is_dialog_message_w,
];
