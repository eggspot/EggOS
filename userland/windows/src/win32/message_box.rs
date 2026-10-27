//! `MessageBoxW` (D129): the box of `docs/ux/windows-gui-v0.3.md` section 1, a window of the
//! layer's own painted with `eggos_windows_gui::message_box`, a dialog of its owner's window (a
//! fixed-size window of its own without one). The call is modal: it runs a message loop, in
//! which the program's other windows keep getting their messages, until a button closes the box,
//! and returns that button's result.

use super::gui::{self, device, send, with};
use super::user32::{remove_window, translate};
use super::window_device::{FIXED_SIZE, ROLE_DIALOG, ROLE_NORMAL};
use super::{error, error_for, fail};
use crate::gui::input::{Record, kind};
use crate::gui::message;
use crate::gui::queue::{Filter, Message};
use crate::gui::windows::{Procedure, Window};
use crate::text as layer_text;
use eggos_windows_gui::message_box::{Controller, Layout, Outcome, paint};
use eggos_windows_gui::surface::Surface;
use eggos_windows_gui::text::{self, Cell, Reading};

const TEXT_CELLS: usize = 4096;
const BUTTON_LEFT: u32 = 0x110;
const DEFAULT_CAPTION: &str = "Error";
const TITLE_BYTES: usize = 256;
const HELP_FOR_WINDOW: i32 = 1;
const CHROME: (i32, i32) = (2, 30);

struct BoxState {
    controller: Controller,
    cells: *const Cell,
    cell_count: usize,
    window: usize,
    owner: usize,
    result: Option<i32>,
}

#[repr(C)]
struct HelpInfo {
    size: u32,
    context_type: i32,
    control_id: i32,
    item: usize,
    context_id: usize,
    x: i32,
    y: i32,
}

fn caption_text(caption: *const u16, out: &mut [u8; TITLE_BYTES]) -> &str {
    if caption.is_null() {
        return DEFAULT_CAPTION;
    }
    // SAFETY: the caption is the caller's NUL-terminated string.
    let units = unsafe { super::file::wide_string(caption) };
    let (length, _) = layer_text::write_utf8(units, out);
    core::str::from_utf8(&out[..length]).unwrap_or(DEFAULT_CAPTION)
}

unsafe extern "efiapi" fn message_box_w(
    owner: usize,
    message_text: *const u16,
    caption: *const u16,
    box_type: u32,
) -> i32 {
    let units = match message_text.is_null() {
        true => &[][..],
        // SAFETY: the caller passes a NUL-terminated string or null.
        false => unsafe { super::file::wide_string(message_text) },
    };
    let mut cells = [Cell::Space; TEXT_CELLS];
    let filled = text::decode(units, Reading::default(), &mut cells);
    let device = match device() {
        Ok(device) => device,
        Err(code) => return fail(code, 0),
    };
    let work = device.work_area;
    let layout = Layout::new(&cells[..filled], box_type, work.width());
    let parent = with(|gui| {
        let window = gui.windows.get(owner)?;
        let shown = window.committed && matches!(window.procedure, Procedure::Program(_));
        shown.then_some(window.server)
    })
    .flatten();
    let (role, parent_server, flags) = match parent {
        Some(server) => (ROLE_DIALOG, server, 0),
        None => (ROLE_NORMAL, 0, FIXED_SIZE),
    };
    let buffer = match device.create(
        role,
        parent_server,
        layout.width as u32,
        layout.height as u32,
        flags,
    ) {
        Ok(buffer) => buffer,
        Err(errno) => return fail(error_for(errno), 0),
    };
    let mut title = [0u8; TITLE_BYTES];
    let _ = device.set_title(buffer.window, caption_text(caption, &mut title));
    if parent.is_none() {
        let _ = device.set_position(
            buffer.window,
            (work.width() - layout.width - CHROME.0) / 2,
            (work.height() - layout.height - CHROME.1) / 2,
        );
    }
    let mut state = BoxState {
        controller: Controller::new(layout),
        cells: cells.as_ptr(),
        cell_count: filled,
        window: 0,
        owner,
        result: None,
    };
    let state = &raw mut state;
    let mut window = Window::new(
        Procedure::MessageBox(state as usize),
        buffer.window,
        buffer.pixels,
        buffer.width,
        buffer.height,
    );
    window.shown = true;
    window.committed = true;
    window.owner = owner;
    let Some(Some(handle)) = with(|gui| gui.windows.insert(window)) else {
        device.destroy(&buffer);
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    // SAFETY: the state is this frame's, and no reference to it is alive.
    unsafe { (*state).window = handle };
    // SAFETY: as above.
    repaint(unsafe { &*state });
    let result = run(state);
    remove_window(handle);
    result
}

fn run(state: *mut BoxState) -> i32 {
    loop {
        // SAFETY: the state lives in the `MessageBoxW` frame that runs this loop, and no
        // reference to it is alive between messages.
        if let Some(result) = unsafe { (*state).result } {
            return result;
        }
        let Some(next) = gui::next_message(Filter::default(), true, true) else {
            return 0;
        };
        if next.message == message::QUIT {
            with(|gui| gui.queue.quit(next.wparam));
            gui::wake();
            // SAFETY: as above.
            let layout = unsafe { *(*state).controller.layout() };
            return layout
                .escape()
                .and_then(|index| layout.buttons()[index].result)
                .unwrap_or(0);
        }
        translate(&next);
        send(next.window, next.message, next.wparam, next.lparam);
    }
}

fn repaint(state: &BoxState) {
    // SAFETY: the cells are the `MessageBoxW` frame's, alive while its box is.
    let cells = unsafe { core::slice::from_raw_parts(state.cells, state.cell_count) };
    let shown = with(|gui| {
        let window = gui.windows.get(state.window)?;
        // SAFETY: the window's pixels are its mapped buffer of `width * height` pixels, mapped
        // while it is in the table; the table's lock is held.
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(
                window.pixels as *mut u32,
                window.width as usize * window.height as usize,
            )
        };
        let mut surface = Surface::new(pixels, window.width, window.height)?;
        let controller = &state.controller;
        paint(&mut surface, controller.layout(), cells, controller.look());
        Some((gui.device?, window.server))
    })
    .flatten();
    if let Some((device, server)) = shown {
        let _ = device.commit(server, None);
    }
}

/// What the box whose state is at `state` does with the window server's `record`.
pub fn handle_event(state: usize, record: &Record) {
    // SAFETY: a box's state lives in the frame of the `MessageBoxW` call whose loop reads this
    // record, and that loop holds no reference to it meanwhile.
    let state = unsafe { &mut *(state as *mut BoxState) };
    let controller = &mut state.controller;
    let outcome = match record.kind {
        kind::POINTER_ENTER | kind::POINTER_MOVE => controller.pointer_moved(record.a, record.b),
        kind::POINTER_LEAVE => controller.pointer_left(),
        kind::BUTTON if record.c == BUTTON_LEFT => {
            controller.left_button(record.a, record.b, record.d != 0)
        }
        kind::KEY => controller.key(record.a as u16, record.b != 0, record.c, record.d),
        kind::CLOSE_REQUEST => controller.close_request(),
        _ => Outcome::Unchanged,
    };
    let activated = match outcome {
        Outcome::Unchanged => return,
        Outcome::Repaint => None,
        Outcome::Activated(index) => Some(controller.layout().buttons()[index]),
    };
    repaint(state);
    let Some(button) = activated else {
        return;
    };
    match button.result {
        Some(result) => {
            state.result = Some(result);
            let window = state.window;
            with(|gui| {
                gui.queue.post(Message {
                    window,
                    message: message::NULL,
                    ..Message::default()
                })
            });
            gui::wake();
        }
        None if state.owner != 0 => {
            let help = HelpInfo {
                size: core::mem::size_of::<HelpInfo>() as u32,
                context_type: HELP_FOR_WINDOW,
                control_id: 0,
                item: state.window,
                context_id: 0,
                x: 0,
                y: 0,
            };
            send(state.owner, message::HELP, 0, &raw const help as usize);
        }
        None => {}
    }
}

pub(super) const EXPORTS: &[crate::library::Export] = exports![
    b"MessageBoxW" => message_box_w,
];
