//! The system's control classes (D145, `docs/ux/windows-controls-v0.3.md`): `BUTTON`, `EDIT`,
//! `STATIC` and `LISTBOX` are child windows whose procedure is the layer's, over the models of
//! `eggos_windows_gui::controls`. Each control's state is a heap block its window's entry
//! points to, reached only under the GUI state's lock and looked up again by handle after every
//! call into the program, so a control the program destroys from a notification is never
//! touched again. Notifications are `WM_COMMAND`s sent to the parent.

use super::dialog::{has_focus, release_capture, set_capture, set_focus};
use super::gui::{self, Gui, with};
use super::paint;
use super::user32::def_window_proc_w;
use super::{heap, window_text};
use crate::gui::message;
use crate::gui::windows::{ControlKind, Procedure, Window};
use crate::text::equal_ignoring_ascii_case;
use eggos_windows_gui::controls::button::{self, Button, ButtonKind, ButtonLook, ButtonOutcome};
use eggos_windows_gui::controls::edit::{self, Edit, EditLook, EditOutcome, visible_cells};
use eggos_windows_gui::controls::label::{self, LabelLook};
use eggos_windows_gui::controls::list_box::{self, ITEM_HEIGHT, ListBox, ListLook, visible_rows};
use eggos_windows_gui::controls::{DARK, LIGHT, Palette};

const VK_SPACE: usize = 0x20;
const LB_ERR: usize = usize::MAX;

/// The class messages and notification codes the controls answer and send.
pub mod code {
    pub const BM_GETCHECK: u32 = 0x00f0;
    pub const BM_SETCHECK: u32 = 0x00f1;
    pub const BM_GETSTATE: u32 = 0x00f2;
    pub const BM_SETSTATE: u32 = 0x00f3;
    pub const BM_SETSTYLE: u32 = 0x00f4;
    pub const BM_CLICK: u32 = 0x00f5;
    pub const EM_LIMITTEXT: u32 = 0x00c5;
    pub const EM_SETREADONLY: u32 = 0x00cf;
    pub const EM_GETLIMITTEXT: u32 = 0x00d5;
    pub const LB_ADDSTRING: u32 = 0x0180;
    pub const LB_INSERTSTRING: u32 = 0x0181;
    pub const LB_DELETESTRING: u32 = 0x0182;
    pub const LB_RESETCONTENT: u32 = 0x0184;
    pub const LB_SETCURSEL: u32 = 0x0186;
    pub const LB_GETCURSEL: u32 = 0x0188;
    pub const LB_GETTEXT: u32 = 0x0189;
    pub const LB_GETTEXTLEN: u32 = 0x018a;
    pub const LB_GETCOUNT: u32 = 0x018b;
    pub const LB_GETITEMHEIGHT: u32 = 0x01a1;
    pub const BN_CLICKED: usize = 0;
    pub const STN_CLICKED: usize = 0;
    pub const EN_SETFOCUS: usize = 0x0100;
    pub const EN_KILLFOCUS: usize = 0x0200;
    pub const EN_CHANGE: usize = 0x0300;
    pub const EN_UPDATE: usize = 0x0400;
    pub const LBN_SELCHANGE: usize = 1;
    pub const LBN_SETFOCUS: usize = 4;
    pub const LBN_KILLFOCUS: usize = 5;
}

const BST_CHECKED: usize = 1;
const BST_PUSHED: usize = 4;
const BST_FOCUS: usize = 8;

/// The system class a class name names, compared ignoring case.
pub fn system_class(name: &[u16]) -> Option<ControlKind> {
    let is = |wanted: &str| {
        name.len() == wanted.len()
            && equal_ignoring_ascii_case(name.iter().map(|unit| *unit as u8), wanted.bytes())
            && name.iter().all(|unit| *unit < 0x80)
    };
    match () {
        () if is("BUTTON") => Some(ControlKind::Button),
        () if is("EDIT") => Some(ControlKind::Edit),
        () if is("STATIC") => Some(ControlKind::Static),
        () if is("LISTBOX") => Some(ControlKind::ListBox),
        () => None,
    }
}

fn allocate<T>(value: T) -> Option<usize> {
    let block = heap::allocate(core::mem::size_of::<T>()).cast::<T>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, 16-byte aligned heap block the size of `T`.
    unsafe { block.write(value) };
    Some(block as usize)
}

/// A new control's state for a window of `style`; `None` without memory (0 for a static,
/// which keeps none).
pub fn new_state(kind: ControlKind, style: u32) -> Option<usize> {
    match kind {
        ControlKind::Static => Some(0),
        ControlKind::Button => allocate(Button::new(ButtonKind::of(style))),
        ControlKind::Edit => {
            let address = heap::allocate(core::mem::size_of::<Edit>()).cast::<Edit>();
            if address.is_null() {
                return None;
            }
            let mut edit = Edit::new();
            edit.read_only = style & edit::style::READ_ONLY != 0;
            // SAFETY: a new, 16-byte aligned heap block the size of an `Edit`.
            unsafe { address.write(edit) };
            Some(address as usize)
        }
        ControlKind::ListBox => {
            let address = heap::allocate(core::mem::size_of::<ListBox>()).cast::<ListBox>();
            if address.is_null() {
                return None;
            }
            // SAFETY: as for the edit.
            unsafe { address.write(ListBox::new()) };
            Some(address as usize)
        }
    }
}

/// Frees the state of a control whose window left the table.
pub fn free_state(state: usize) {
    if state != 0 {
        // SAFETY: the block is the one `new_state` made; its window is gone, so nothing
        // reaches it any more.
        unsafe { heap::free(state as *mut u8) };
    }
}

enum State<'a> {
    Button(&'a mut Button),
    Edit(&'a mut Edit),
    ListBox(&'a mut ListBox),
    Static,
}

fn state_of(gui: &mut Gui, window: usize) -> Option<(&mut Window, State<'_>)> {
    let entry = gui.windows.get_mut(window)?;
    let Procedure::Control(kind, address) = entry.procedure else {
        return None;
    };
    let state = match kind {
        // SAFETY: a control's state is the heap block `new_state` made for its kind, freed
        // only after its window left the table; the caller holds the lock through `gui`, and
        // every access goes through here.
        ControlKind::Button => State::Button(unsafe { &mut *(address as *mut Button) }),
        // SAFETY: as for the button.
        ControlKind::Edit => State::Edit(unsafe { &mut *(address as *mut Edit) }),
        // SAFETY: as for the button.
        ControlKind::ListBox => State::ListBox(unsafe { &mut *(address as *mut ListBox) }),
        ControlKind::Static => State::Static,
    };
    Some((entry, state))
}

fn with_state<R>(window: usize, work: impl FnOnce(&mut Window, State<'_>) -> R) -> Option<R> {
    with(|gui| {
        let (entry, state) = state_of(gui, window)?;
        Some(work(entry, state))
    })
    .flatten()
}

fn invalidate(window: usize) {
    with(|gui| {
        if let Some(entry) = gui.windows.get_mut(window) {
            entry.invalidate(None, true);
        }
    });
    gui::wake();
}

fn notify(window: usize, notification: usize) {
    let target = with(|gui| {
        let entry = gui.windows.get(window)?;
        Some((entry.parent, entry.id))
    })
    .flatten();
    if let Some((parent, id)) = target
        && parent != 0
    {
        gui::send(
            parent,
            message::COMMAND,
            message::word_pair(id, notification),
            window,
        );
    }
}

fn palette_of(gui: &Gui, window: usize) -> &'static Palette {
    let root = crate::gui::windows::root_of(&gui.windows, window);
    match root
        .and_then(|root| gui.windows.get(root))
        .map(|root| root.procedure)
    {
        Some(Procedure::FilePicker(_)) => &DARK,
        _ => &LIGHT,
    }
}

fn is_default_button(gui: &Gui, window: usize, kind: ButtonKind) -> bool {
    let Some(root) = crate::gui::windows::root_of(&gui.windows, window) else {
        return false;
    };
    let focus = gui.key_target(root);
    let focus_is_push = gui.windows.get(focus).is_some_and(|entry| {
        matches!(entry.procedure, Procedure::Control(ControlKind::Button, _))
            && ButtonKind::of(entry.style).is_push()
    });
    match kind {
        ButtonKind::DefaultPush => !focus_is_push || focus == window,
        ButtonKind::Push => focus == window,
        _ => false,
    }
}

fn paint_control(window: usize) {
    let Ok((hdc, _, _)) = paint::begin(window) else {
        return;
    };
    let _ = paint::draw_in(hdc, |surface, _, gui| {
        let Some(surface) = surface else {
            return;
        };
        let Some(entry) = gui.windows.get(window) else {
            return;
        };
        let Procedure::Control(kind, address) = entry.procedure else {
            return;
        };
        let rect = entry.client();
        let palette = palette_of(gui, window);
        let focused = has_focus(gui, window);
        let text = window_text::of(gui, window);
        match kind {
            ControlKind::Button => {
                // SAFETY: as `state_of`: the block is this control's, alive while its window is
                // in the table, and the lock is held; only a shared reference is made here.
                let model = unsafe { &*(address as *const Button) };
                let look = ButtonLook {
                    kind: model.kind,
                    checked: model.checked,
                    pressed: model.pressed,
                    focused,
                    default: is_default_button(gui, window, model.kind),
                    enabled: entry.enabled,
                };
                button::paint(surface, rect, text, look, palette);
            }
            ControlKind::Edit => {
                // SAFETY: as above.
                let model = unsafe { &*(address as *const Edit) };
                let look = EditLook {
                    focused,
                    enabled: entry.enabled,
                };
                edit::paint(surface, rect, model, look, palette);
            }
            ControlKind::ListBox => {
                // SAFETY: as above.
                let model = unsafe { &*(address as *const ListBox) };
                let look = ListLook {
                    focused,
                    enabled: entry.enabled,
                };
                list_box::paint(surface, rect, model, look, palette);
            }
            ControlKind::Static => {
                let look = LabelLook {
                    style: entry.style,
                    enabled: entry.enabled,
                };
                label::paint(surface, rect, text, look, palette);
            }
        }
    });
    paint::finish(hdc);
}

fn point(lparam: usize) -> (i32, i32) {
    (
        lparam as u16 as i16 as i32,
        (lparam >> 16) as u16 as i16 as i32,
    )
}

fn button_outcome(window: usize, outcome: ButtonOutcome) {
    match outcome {
        ButtonOutcome::Unchanged => {}
        ButtonOutcome::Repaint => invalidate(window),
        ButtonOutcome::Clicked => {
            invalidate(window);
            notify(window, code::BN_CLICKED);
        }
    }
}

fn edit_outcome(window: usize, outcome: Option<EditOutcome>) {
    match outcome {
        Some(EditOutcome::Changed) => {
            invalidate(window);
            notify(window, code::EN_UPDATE);
            notify(window, code::EN_CHANGE);
        }
        Some(EditOutcome::Moved) => invalidate(window),
        _ => {}
    }
}

fn list_changed(window: usize, changed: Option<bool>) {
    if changed.is_none() {
        return;
    }
    invalidate(window);
    let notifies = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| entry.style & list_box::style::NOTIFY != 0)
    })
    .unwrap_or(false);
    if changed == Some(true) && notifies {
        notify(window, code::LBN_SELCHANGE);
    }
}

/// The window procedure of every control of `kind`.
pub fn procedure(
    window: usize,
    kind: ControlKind,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::CREATE => 0,
        message::PAINT => {
            paint_control(window);
            0
        }
        message::ERASE_BACKGROUND => 1,
        message::SET_FONT | message::GET_FONT => 0,
        message::ENABLE => {
            invalidate(window);
            0
        }
        message::SET_TEXT => set_text(window, kind, lparam),
        message::GET_TEXT => get_text(window, kind, wparam, lparam),
        message::GET_TEXT_LENGTH => match kind {
            ControlKind::Edit => with_state(window, |_, state| match state {
                State::Edit(edit) => edit.text().len(),
                _ => 0,
            })
            .unwrap_or(0),
            _ => window_text::length_default(window),
        },
        message::SET_FOCUS | message::KILL_FOCUS => {
            let gained = number == message::SET_FOCUS;
            if !gained {
                with_state(window, |_, state| {
                    if let State::Button(button) = state {
                        button.cancel();
                    }
                });
            }
            invalidate(window);
            match (kind, gained) {
                (ControlKind::Edit, true) => notify(window, code::EN_SETFOCUS),
                (ControlKind::Edit, false) => notify(window, code::EN_KILLFOCUS),
                (ControlKind::ListBox, focus) => {
                    let notifies = with(|gui| {
                        gui.windows
                            .get(window)
                            .is_some_and(|entry| entry.style & list_box::style::NOTIFY != 0)
                    });
                    if notifies == Some(true) {
                        notify(
                            window,
                            if focus {
                                code::LBN_SETFOCUS
                            } else {
                                code::LBN_KILLFOCUS
                            },
                        );
                    }
                }
                _ => {}
            }
            0
        }
        message::LEFT_BUTTON_DOWN => {
            left_button_down(window, kind, point(lparam));
            0
        }
        message::MOUSE_MOVE => {
            let (x, y) = point(lparam);
            let outcome = with_state(window, |entry, state| match state {
                State::Button(button) => button.pointer_moved(entry.client().contains(x, y)),
                _ => ButtonOutcome::Unchanged,
            });
            button_outcome(window, outcome.unwrap_or(ButtonOutcome::Unchanged));
            0
        }
        message::LEFT_BUTTON_UP => {
            left_button_up(window, kind, point(lparam));
            0
        }
        message::KEY_DOWN | message::KEY_UP => {
            key(window, number == message::KEY_DOWN, wparam);
            0
        }
        message::CHAR => {
            let outcome = with_state(window, |entry, state| match state {
                State::Edit(edit) => {
                    Some(edit.character(wparam as u32, visible_cells(entry.width as i32)))
                }
                _ => None,
            })
            .flatten();
            edit_outcome(window, outcome);
            0
        }
        number if (code::BM_GETCHECK..=code::BM_CLICK).contains(&number) => {
            button_message(window, number, wparam)
        }
        code::EM_LIMITTEXT | code::EM_GETLIMITTEXT | code::EM_SETREADONLY => {
            with_state(window, |_, state| match (state, number) {
                (State::Edit(edit), code::EM_LIMITTEXT) => {
                    edit.set_limit(wparam);
                    0
                }
                (State::Edit(edit), code::EM_GETLIMITTEXT) => edit.limit(),
                (State::Edit(edit), _) => {
                    edit.read_only = wparam != 0;
                    1
                }
                _ => 0,
            })
            .inspect(|_| invalidate(window))
            .unwrap_or(0)
        }
        number
            if (code::LB_ADDSTRING..=code::LB_GETITEMHEIGHT).contains(&number)
                && kind == ControlKind::ListBox =>
        {
            list_message(window, number, wparam, lparam)
        }
        _ => def_window_proc_w(window, number, wparam, lparam),
    }
}

fn set_text(window: usize, kind: ControlKind, text: usize) -> usize {
    if kind != ControlKind::Edit {
        // SAFETY: `WM_SETTEXT`'s `lParam` is 0 or a NUL-terminated string.
        let result = unsafe { window_text::set_default(window, text) };
        invalidate(window);
        return result;
    }
    let units = match text {
        0 => &[][..],
        // SAFETY: as above.
        text => unsafe { super::file::wide_string(text as *const u16) },
    };
    let set = with_state(window, |_, state| {
        if let State::Edit(edit) = state {
            edit.set_text(units);
        }
    });
    if set.is_some() {
        edit_outcome(window, Some(EditOutcome::Changed));
    }
    1
}

fn get_text(window: usize, kind: ControlKind, capacity: usize, buffer: usize) -> usize {
    if kind != ControlKind::Edit {
        // SAFETY: `WM_GETTEXT`'s contract: `buffer` holds `capacity` writable units.
        return unsafe { window_text::get_default(window, capacity, buffer) };
    }
    with_state(window, |_, state| match state {
        // SAFETY: as above.
        State::Edit(edit) => unsafe { window_text::copy_out(edit.text(), buffer, capacity) },
        _ => 0,
    })
    .unwrap_or(0)
}

fn left_button_down(window: usize, kind: ControlKind, (x, y): (i32, i32)) {
    let enabled = with(|gui| gui.windows.get(window).is_some_and(|entry| entry.enabled));
    if enabled != Some(true) {
        return;
    }
    if kind != ControlKind::Static {
        set_focus(window);
    }
    match kind {
        ControlKind::Button => {
            set_capture(window);
            let outcome = with_state(window, |entry, state| match state {
                State::Button(button) => button.left_button(entry.client().contains(x, y), true),
                _ => ButtonOutcome::Unchanged,
            });
            button_outcome(window, outcome.unwrap_or(ButtonOutcome::Unchanged));
        }
        ControlKind::Edit => {
            let outcome = with_state(window, |entry, state| match state {
                State::Edit(edit) => Some(edit.click(x, visible_cells(entry.width as i32))),
                _ => None,
            })
            .flatten();
            edit_outcome(window, outcome);
        }
        ControlKind::ListBox => {
            let changed = with_state(window, |entry, state| match state {
                State::ListBox(list) => {
                    let height = entry.height as i32;
                    list_box::row_at(y, height).map(|row| list.click_row(row, visible_rows(height)))
                }
                _ => None,
            })
            .flatten();
            list_changed(window, changed);
        }
        ControlKind::Static => {}
    }
}

fn left_button_up(window: usize, kind: ControlKind, (x, y): (i32, i32)) {
    match kind {
        ControlKind::Button => {
            release_capture();
            let outcome = with_state(window, |entry, state| match state {
                State::Button(button) => button.left_button(entry.client().contains(x, y), false),
                _ => ButtonOutcome::Unchanged,
            });
            button_outcome(window, outcome.unwrap_or(ButtonOutcome::Unchanged));
        }
        ControlKind::Static => {
            let notifies = with(|gui| {
                gui.windows
                    .get(window)
                    .is_some_and(|entry| entry.style & label::style::NOTIFY != 0)
            });
            if notifies == Some(true) {
                notify(window, code::STN_CLICKED);
            }
        }
        _ => {}
    }
}

fn key(window: usize, down: bool, virtual_key: usize) {
    enum Effect {
        Button(ButtonOutcome),
        Edit(Option<EditOutcome>),
        List(Option<bool>),
        None,
    }
    let effect = with_state(window, |entry, state| match state {
        State::Button(button) if virtual_key == VK_SPACE => Effect::Button(button.space(down)),
        State::Edit(edit) if down => Effect::Edit(Some(
            edit.key(virtual_key as u32, visible_cells(entry.width as i32)),
        )),
        State::ListBox(list) if down => {
            let changed = list.key(virtual_key as u32, visible_rows(entry.height as i32));
            Effect::List(Some(changed))
        }
        _ => Effect::None,
    });
    match effect {
        Some(Effect::Button(outcome)) => button_outcome(window, outcome),
        Some(Effect::Edit(outcome)) => edit_outcome(window, outcome),
        Some(Effect::List(Some(true))) => list_changed(window, Some(true)),
        _ => {}
    }
}

fn button_message(window: usize, number: u32, wparam: usize) -> usize {
    let focused = with(|gui| has_focus(gui, window)).unwrap_or(false);
    let answer = with_state(window, |entry, state| {
        let State::Button(button) = state else {
            return (0, ButtonOutcome::Unchanged);
        };
        match number {
            code::BM_GETCHECK => (usize::from(button.checked), ButtonOutcome::Unchanged),
            code::BM_SETCHECK => {
                button.checked = wparam == BST_CHECKED;
                (0, ButtonOutcome::Repaint)
            }
            code::BM_GETSTATE => {
                let mut state = usize::from(button.checked);
                if button.pressed {
                    state |= BST_PUSHED;
                }
                if focused {
                    state |= BST_FOCUS;
                }
                (state, ButtonOutcome::Unchanged)
            }
            code::BM_SETSTATE => {
                button.pressed = wparam != 0;
                (0, ButtonOutcome::Repaint)
            }
            code::BM_SETSTYLE => {
                entry.style =
                    (entry.style & !button::style::TYPE) | (wparam as u32 & button::style::TYPE);
                button.kind = ButtonKind::of(entry.style);
                (0, ButtonOutcome::Repaint)
            }
            _ => (0, button.click()),
        }
    });
    let (result, outcome) = answer.unwrap_or((0, ButtonOutcome::Unchanged));
    button_outcome(window, outcome);
    result
}

fn list_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let text = |lparam: usize| match lparam {
        0 => &[][..],
        // SAFETY: `LB_ADDSTRING`'s and `LB_INSERTSTRING`'s `lParam` is a NUL-terminated string.
        lparam => unsafe { super::file::wide_string(lparam as *const u16) },
    };
    let answer = with_state(window, |entry, state| {
        let State::ListBox(list) = state else {
            return (LB_ERR, false);
        };
        let visible = visible_rows(entry.height as i32);
        let sorted = entry.style & list_box::style::SORT != 0;
        let index = |value: Option<usize>| value.unwrap_or(LB_ERR);
        match number {
            code::LB_ADDSTRING => (index(list.add(text(lparam), sorted)), true),
            code::LB_INSERTSTRING => {
                let at = match wparam as isize {
                    -1 => Some(list.count()),
                    at if (0..=list.count() as isize).contains(&at) => Some(at as usize),
                    _ => None,
                };
                (index(at.and_then(|at| list.insert(at, text(lparam)))), true)
            }
            code::LB_DELETESTRING => (index(list.delete(wparam)), true),
            code::LB_RESETCONTENT => {
                list.clear();
                (0, true)
            }
            code::LB_SETCURSEL => {
                let wanted = match wparam as isize {
                    -1 => None,
                    at => Some(at as usize),
                };
                let valid = list.select(wanted, visible);
                (
                    if valid && wanted.is_some() {
                        wparam
                    } else {
                        LB_ERR
                    },
                    true,
                )
            }
            code::LB_GETCURSEL => (index(list.selected()), false),
            code::LB_GETTEXT => match list.text(wparam) {
                Some(item) => {
                    // SAFETY: `LB_GETTEXT`'s contract: `lParam` has room for the item and a NUL.
                    unsafe {
                        let out =
                            core::slice::from_raw_parts_mut(lparam as *mut u16, item.len() + 1);
                        out[..item.len()].copy_from_slice(item);
                        out[item.len()] = 0;
                    }
                    (item.len(), false)
                }
                None => (LB_ERR, false),
            },
            code::LB_GETTEXTLEN => (index(list.text(wparam).map(<[u16]>::len)), false),
            code::LB_GETCOUNT => (list.count(), false),
            code::LB_GETITEMHEIGHT => (ITEM_HEIGHT as usize, false),
            _ => (LB_ERR, false),
        }
    });
    let (result, changed) = answer.unwrap_or((LB_ERR, false));
    if changed {
        invalidate(window);
    }
    result
}

/// Fills list `window` with `entries` (the folder mark on those that are folders), replacing
/// what it held, as the file picker shows a folder.
pub fn fill_list(window: usize, entries: &mut dyn Iterator<Item = (&[u16], bool)>) {
    with_state(window, |entry, state| {
        if let State::ListBox(list) = state {
            list.clear();
            list.marks = true;
            for (name, folder) in entries {
                if let Some(index) = list.add(name, false)
                    && folder
                {
                    list.mark(index);
                }
            }
            entry.invalidate(None, true);
        }
    });
    gui::wake();
}

/// The text of `window`'s selected item and whether it is marked as a folder.
pub fn selected_item(window: usize, out: &mut [u16]) -> Option<(usize, bool)> {
    with_state(window, |_, state| {
        let State::ListBox(list) = state else {
            return None;
        };
        let index = list.selected()?;
        let text = list.text(index)?;
        let length = text.len().min(out.len());
        out[..length].copy_from_slice(&text[..length]);
        Some((length, list.is_marked(index)))
    })
    .flatten()
}

/// Gives a new edit `window` the text it was created with, without notifications.
pub fn start_edit_text(window: usize, text: &[u16]) {
    with_state(window, |_, state| {
        if let State::Edit(edit) = state {
            edit.set_text(text);
        }
    });
}

/// Puts the caret of edit `window` after its text.
pub fn caret_to_end(window: usize) {
    with_state(window, |entry, state| {
        if let State::Edit(edit) = state {
            edit.caret_to_end(visible_cells(entry.width as i32));
        }
    });
    invalidate(window);
}
