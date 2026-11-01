//! NSAlert (`docs/ux/macos-gui-v0.3.md` section 4): `runModal` opens a dialog window of its own on
//! the key window, draws the toolkit's dialog panel in it (`eggos_macos_gui::alert`), and runs
//! its own event loop until a button is chosen, while the app's other windows still redraw,
//! follow the focus and take new sizes but no input; a close asked of one of them, or a menu
//! choice that reached the app as the alert opened, is carried out once the alert is answered.

use super::{
    NSALERT, NSALERT_META, NSBUTTON, application, elements, menu, server, store_copy, string, view,
    window, with_text,
};
use crate::foundation::{NSARRAY, class_pointer};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, autorelease, methods, sel, send, send1, send2};
use eggos_element_tree::{Query, tree_answer};
use eggos_macos_gui::alert::{Alert, FIRST_BUTTON_RETURN, MAX_BUTTONS};
use eggos_macos_gui::geometry::Frame;
use eggos_macos_gui::menu::Chosen;
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::input::{self, Input, MouseButton, RECORD_SIZE, key};
use eggos_toolkit::text::Text;

const LINE_CAPACITY: usize = 128;
const TITLE_CAPACITY: usize = 32;

#[repr(C)]
struct AlertFields {
    isa: usize,
    message: Id,
    informative: Id,
    buttons: Buffer<Id>,
    style: usize,
}

fn fields(alert: Id) -> &'static mut AlertFields {
    // SAFETY: the callers pass NSAlert instances.
    unsafe { &mut *alert.cast::<AlertFields>() }
}

extern "C" fn message_text(this: Id, _: Sel) -> Id {
    match fields(this).message {
        none if none.is_null() => string(""),
        text => text,
    }
}

extern "C" fn set_message_text(this: Id, _: Sel, text: Id) {
    store_copy(&mut fields(this).message, text);
}

extern "C" fn informative_text(this: Id, _: Sel) -> Id {
    match fields(this).informative {
        none if none.is_null() => string(""),
        text => text,
    }
}

extern "C" fn set_informative_text(this: Id, _: Sel, text: Id) {
    store_copy(&mut fields(this).informative, text);
}

extern "C" fn add_button_with_title(this: Id, _: Sel, title: Id) -> Id {
    let button = view::new_with_frame(&NSBUTTON, Frame::default());
    if button.is_null() {
        return button;
    }
    send1(button, sel!("setTitle:"), title as usize);
    fields(this).buttons.push(button);
    button
}

extern "C" fn buttons(this: Id, _: Sel) -> Id {
    let buttons = fields(this).buttons.as_slice();
    send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        buttons.as_ptr() as usize,
        buttons.len(),
    )
}

extern "C" fn alert_style(this: Id, _: Sel) -> usize {
    fields(this).style
}

extern "C" fn set_alert_style(this: Id, _: Sel, style: usize) {
    fields(this).style = style;
}

extern "C" fn nil(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn nothing(_: Id, _: Sel) {}

fn paint(alert: &Alert, buffer: &server::Buffer, focused: bool) {
    // SAFETY: the dialog's buffer is mapped until `run_modal` destroys it, and only this
    // thread draws on it.
    let pixels = unsafe { buffer.pixels() };
    if let Some(mut canvas) = Canvas::new(pixels, buffer.width, buffer.height, 1) {
        alert.paint(&mut canvas, focused);
    }
    server::commit(buffer.window);
}

fn parent_window() -> u32 {
    let key_window = application::key_window();
    if !key_window.is_null() && window::server_id(key_window) != 0 {
        return window::server_id(key_window);
    }
    let mut index = 0;
    while let Some(candidate) = application::window_at(index) {
        if window::server_id(candidate) != 0 {
            return window::server_id(candidate);
        }
        index += 1;
    }
    0
}

struct Modal {
    alert: Alert,
    buffer: server::Buffer,
    focused: bool,
    chosen: Option<usize>,
    closes_asked: Buffer<u32>,
    menu_choices: Buffer<Chosen>,
}

impl Modal {
    fn handle(&mut self, record: &[u8; RECORD_SIZE]) {
        if self.chosen.is_some() {
            return;
        }
        if let Some(query) = Query::from_record(record) {
            match query.window == self.buffer.window {
                true => server::answer_query(query.serial, tree_answer::PIXELS_ONLY, &[]),
                false => elements::answer_while_modal(&query),
            }
            return;
        }
        if let Some(choice) = Chosen::from_record(record) {
            self.menu_choices.push(choice);
            return;
        }
        let Some(event) = input::decode(record) else {
            return;
        };
        if u32::from(event.window) != self.buffer.window {
            if let Some(other) = application::window_with_server_id(u32::from(event.window)) {
                match event.input {
                    Input::FocusIn => application::make_key(other),
                    Input::FocusOut => application::focus_out(other),
                    Input::Resize { width, height } => window::resize(other, width, height),
                    Input::CloseRequest
                        if !self
                            .closes_asked
                            .as_slice()
                            .contains(&u32::from(event.window)) =>
                    {
                        self.closes_asked.push(u32::from(event.window));
                    }
                    _ => {}
                }
            }
            return;
        }
        let chosen = match event.input {
            Input::Key {
                code,
                pressed,
                modifiers,
                ..
            } => self.alert.key(code, modifiers.shift(), pressed),
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => self.alert.button(x, y, pressed),
            Input::CloseRequest => self.alert.key(key::ESC, false, true),
            Input::FocusIn => {
                self.focused = true;
                None
            }
            Input::FocusOut => {
                self.focused = false;
                None
            }
            _ => return,
        };
        self.chosen = chosen;
        if chosen.is_none() {
            paint(&self.alert, &self.buffer, self.focused);
        }
    }
}

extern "C" fn run_modal(this: Id, _: Sel) -> isize {
    let mut message = Text::<LINE_CAPACITY>::new();
    let mut informative = Text::<LINE_CAPACITY>::new();
    with_text(fields(this).message, |text| message.set(text));
    with_text(fields(this).informative, |text| informative.set(text));
    let mut titles = [Text::<TITLE_CAPACITY>::new(); MAX_BUTTONS];
    let added = fields(this).buttons.len().min(MAX_BUTTONS);
    for (index, title) in titles.iter_mut().enumerate().take(added) {
        let button = fields(this).buttons.as_slice()[index];
        let name = send(button, sel!("title"));
        with_text(name, |text| title.set(text));
    }
    let names: [&str; MAX_BUTTONS] = [titles[0].as_str(), titles[1].as_str(), titles[2].as_str()];
    let alert = Alert::new(message.as_str(), informative.as_str(), &names[..added]);
    let (width, height) = alert.size();
    let parent = parent_window();
    let (role, flags) = match parent {
        0 => (server::ROLE_NORMAL, server::FIXED_SIZE),
        _ => (server::ROLE_DIALOG, 0),
    };
    let Ok(buffer) = server::create(role, parent, width, height, flags) else {
        application::report_no_window_server();
        return FIRST_BUTTON_RETURN;
    };
    server::set_title(buffer.window, super::app_name());
    let mut modal = Modal {
        alert,
        buffer,
        focused: true,
        chosen: None,
        closes_asked: Buffer::new(),
        menu_choices: Buffer::new(),
    };
    paint(&modal.alert, &modal.buffer, modal.focused);
    let chosen = loop {
        let pool = autorelease::push();
        menu::send_if_changed();
        application::display_all();
        application::read_records(&mut |record| modal.handle(record));
        autorelease::pop(pool);
        if let Some(chosen) = modal.chosen {
            break chosen;
        }
    };
    // SAFETY: the dialog is done; nothing draws on its buffer any more.
    unsafe { server::destroy(&modal.buffer) };
    for &id in modal.closes_asked.as_slice() {
        if let Some(other) = application::window_with_server_id(id) {
            send1(other, sel!("performClose:"), 0);
        }
    }
    modal.closes_asked.free();
    for &choice in modal.menu_choices.as_slice() {
        if let Some(other) = application::window_with_server_id(choice.window) {
            menu::chosen(other, choice.menu, choice.entry);
        }
    }
    modal.menu_choices.free();
    Alert::answer(chosen)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "messageText" => message_text,
        "setMessageText:" => set_message_text,
        "informativeText" => informative_text,
        "setInformativeText:" => set_informative_text,
        "addButtonWithTitle:" => add_button_with_title,
        "buttons" => buttons,
        "alertStyle" => alert_style,
        "setAlertStyle:" => set_alert_style,
        "runModal" => run_modal,
        "window" => nil,
        "icon" => nil,
        "setIcon:" => ignore_word,
        "setShowsHelp:" => ignore_word,
        "layout" => nothing,
    ];
    super::define(
        runtime,
        b"NSAlert\0",
        (&NSALERT, &NSALERT_META),
        &NSOBJECT,
        core::mem::size_of::<AlertFields>(),
        &instance,
        &[],
    )
}
