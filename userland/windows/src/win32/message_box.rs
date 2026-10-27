//! `MessageBoxW` (D129): the box of `docs/ux/windows-gui-v0.3.md` section 1, a window of the
//! layer's own painted with `eggos_windows_gui::message_box`, a dialog of its owner's window (a
//! fixed-size window of its own without one). The call is modal: it runs a message loop, in
//! which the calling thread's other windows keep getting their messages, until a button closes
//! the box, and returns that button's result. The box's state is a heap block its window's
//! table entry points to, reached only under the GUI state's lock, so whichever thread reads the
//! server's records for the box can act on them; the call frees it after taking the window out
//! of the table.

use super::gui::{self, Gui, Next, device, send, with};
use super::user32::{remove_window, translate};
use super::window_device::{FIXED_SIZE, ROLE_DIALOG, ROLE_NORMAL};
use super::{error, error_for, fail, heap, thread_environment};
use crate::gui::input::{Record, kind};
use crate::gui::message;
use crate::gui::queue::Filter;
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
const RESTORED: u32 = 0;

struct MessageBoxState {
    controller: Controller,
    cells: [Cell; TEXT_CELLS],
    cell_count: usize,
    owner: usize,
    result: Option<i32>,
    help_asked: bool,
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

fn state_of(gui: &mut Gui, window: usize) -> Option<(&Window, &mut MessageBoxState)> {
    let entry = gui.windows.get(window)?;
    let Procedure::MessageBox(address) = entry.procedure else {
        return None;
    };
    // SAFETY: a box's state is the heap block its `MessageBoxW` call made and frees only after
    // taking the box's window out of the table under the lock, which the caller holds through
    // `gui`; every access to it goes through here under that lock.
    let state = unsafe { &mut *(address as *mut MessageBoxState) };
    Some((entry, state))
}

fn paint_box(window: &Window, state: &MessageBoxState) {
    // SAFETY: the window's pixels are its mapped buffer of `width * height` pixels, mapped while
    // it is in the table; the caller holds the table's lock.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            window.pixels as *mut u32,
            window.width as usize * window.height as usize,
        )
    };
    if let Some(mut surface) = Surface::new(pixels, window.width, window.height) {
        let controller = &state.controller;
        let cells = &state.cells[..state.cell_count];
        paint(&mut surface, controller.layout(), cells, controller.look());
    }
}

fn repaint(window: usize) {
    let shown = with(|gui| {
        let device = gui.device?;
        let (entry, state) = state_of(gui, window)?;
        paint_box(entry, state);
        Some((device, entry.server))
    })
    .flatten();
    if let Some((device, server)) = shown {
        let _ = device.commit(server, None);
    }
}

fn new_state(controller: Controller, cells: &[Cell], owner: usize) -> *mut MessageBoxState {
    let state = heap::allocate(core::mem::size_of::<MessageBoxState>()).cast::<MessageBoxState>();
    if state.is_null() {
        return state;
    }
    let mut stored = [Cell::Space; TEXT_CELLS];
    stored[..cells.len()].copy_from_slice(cells);
    // SAFETY: `state` is a new, 16-byte aligned heap block the size of the state.
    unsafe {
        state.write(MessageBoxState {
            controller,
            cells: stored,
            cell_count: cells.len(),
            owner,
            result: None,
            help_asked: false,
        })
    };
    state
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
        shown.then_some((window.server, window.size_kind == message::SIZE_MINIMIZED))
    })
    .flatten()
    .map(|(server, minimised)| {
        if minimised {
            let _ = device.set_state(server, RESTORED);
        }
        server
    });
    let (role, parent_server, flags) = match parent {
        Some(server) => (ROLE_DIALOG, server, 0),
        None => (ROLE_NORMAL, 0, FIXED_SIZE),
    };
    let state = new_state(Controller::new(layout), &cells[..filled], owner);
    if state.is_null() {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    let buffer = match device.create(
        role,
        parent_server,
        layout.width as u32,
        layout.height as u32,
        flags,
    ) {
        Ok(buffer) => buffer,
        Err(errno) => {
            // SAFETY: the state was never published.
            unsafe { heap::free(state.cast()) };
            return fail(error_for(errno), 0);
        }
    };
    let mut title = [0u8; TITLE_BYTES];
    let _ = device.set_title(buffer.window, caption_text(caption, &mut title));
    if parent.is_none() {
        let _ = device.set_position(
            buffer.window,
            work.width().saturating_sub(layout.width + CHROME.0) / 2,
            work.height().saturating_sub(layout.height + CHROME.1) / 2,
        );
    }
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
    window.thread = thread_environment::thread_id();
    let Some(Some(handle)) = with(|gui| gui.windows.insert(window)) else {
        // SAFETY: the window never reached the table, so nothing holds its buffer.
        unsafe { device.destroy(&buffer) };
        // SAFETY: the state was never published.
        unsafe { heap::free(state.cast()) };
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    repaint(handle);
    let result = run(handle);
    remove_window(handle);
    // SAFETY: the box's window is out of the table, so no thread can reach the state any more,
    // and every earlier access was under the lock `remove_window` took after it.
    unsafe { heap::free(state.cast()) };
    result
}

#[derive(Debug, Clone, Copy)]
enum Wanted {
    Answer(i32),
    Help(usize),
    Gone,
    Nothing,
}

fn wanted(gui: &mut Gui, window: usize) -> Wanted {
    let Some((_, state)) = state_of(gui, window) else {
        return Wanted::Gone;
    };
    if let Some(result) = state.result {
        return Wanted::Answer(result);
    }
    match core::mem::take(&mut state.help_asked) {
        true => Wanted::Help(state.owner),
        false => Wanted::Nothing,
    }
}

fn run(window: usize) -> i32 {
    let help = HelpInfo {
        size: core::mem::size_of::<HelpInfo>() as u32,
        context_type: HELP_FOR_WINDOW,
        control_id: 0,
        item: window,
        context_id: 0,
        x: 0,
        y: 0,
    };
    loop {
        match with(|gui| wanted(gui, window)).unwrap_or(Wanted::Gone) {
            Wanted::Answer(result) => return result,
            Wanted::Gone => return 0,
            Wanted::Help(owner) => {
                send(owner, message::HELP, 0, &raw const help as usize);
            }
            Wanted::Nothing => {}
        }
        let attention = |gui: &Gui| {
            let entry = gui.windows.get(window);
            let Some(Procedure::MessageBox(address)) = entry.map(|entry| entry.procedure) else {
                return true;
            };
            // SAFETY: as `state_of`: the caller holds the lock and the window is in the table.
            let state = unsafe { &*(address as *const MessageBoxState) };
            state.result.is_some() || state.help_asked
        };
        match gui::wait_for(Filter::default(), true, true, &attention) {
            Next::Done => {}
            Next::Nothing => return 0,
            Next::Message(next) if next.message == message::QUIT => {
                with(|gui| gui.queue.quit(next.thread, next.wparam));
                gui::wake();
                let escape = with(|gui| {
                    let (_, state) = state_of(gui, window)?;
                    let layout = state.controller.layout();
                    layout
                        .escape()
                        .and_then(|index| layout.buttons()[index].result)
                })
                .flatten();
                return escape.unwrap_or(0);
            }
            Next::Message(next) => {
                translate(&next);
                send(next.window, next.message, next.wparam, next.lparam);
            }
        }
    }
}

/// What the box `window` does with the window server's `record`, on whichever thread read it:
/// its buttons' state changes, and the box is repainted and shown, under the lock; a button's
/// answer or a help request wakes the thread running the box's `MessageBoxW`.
pub fn handle_event(window: usize, record: &Record) {
    let handled = with(|gui| {
        let device = gui.device?;
        let (entry, state) = state_of(gui, window)?;
        let controller = &mut state.controller;
        let outcome = match record.kind {
            kind::POINTER_ENTER | kind::POINTER_MOVE => {
                controller.pointer_moved(record.a, record.b)
            }
            kind::POINTER_LEAVE => controller.pointer_left(),
            kind::BUTTON if record.c == BUTTON_LEFT => {
                controller.left_button(record.a, record.b, record.d != 0)
            }
            kind::KEY => controller.key(record.a as u16, record.b != 0, record.c, record.d),
            kind::CLOSE_REQUEST => controller.close_request(),
            _ => Outcome::Unchanged,
        };
        let answered = match outcome {
            Outcome::Unchanged => return None,
            Outcome::Repaint => false,
            Outcome::Activated(index) => {
                match controller.layout().buttons()[index].result {
                    Some(result) => state.result = Some(result),
                    None => state.help_asked = state.owner != 0,
                }
                true
            }
        };
        paint_box(entry, state);
        Some((device, entry.server, answered))
    })
    .flatten();
    if let Some((device, server, answered)) = handled {
        let _ = device.commit(server, None);
        if answered {
            gui::wake();
        }
    }
}

pub(super) const EXPORTS: &[crate::library::Export] = exports![
    b"MessageBoxW" => message_box_w,
];
