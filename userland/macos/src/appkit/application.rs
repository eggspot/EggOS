//! NSApplication and `NSApp`: the delegate's launch and termination, the event loop over the
//! window server's records (each made an NSEvent and sent through `sendEvent:`, the main menu's
//! key equivalents first, Cmd+Q always quitting), the app's windows, and actions sent to their
//! target or along the responder chain.

use super::event::{self, kind};
use super::{NSAPPLICATION, NSAPPLICATION_META, NSRESPONDER, menu, responds, server, view, window};
use crate::foundation::{NSARRAY, class_pointer, info_string};
use crate::libsystem::{Exclusive, write_error};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::{Runtime, autorelease, methods, sel, send, send_bool, send1, send2};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use eggos_macos_gui::geometry::point_of_pixel;
use eggos_macos_gui::keys::modifier;
use eggos_macos_gui::menu::Chosen;
use eggos_toolkit::input::{self, Input, MouseButton, RECORD_SIZE};

const NS_TERMINATE_CANCEL: usize = 0;
const RECORDS_PER_READ: usize = 16;
const NO_WINDOW_SERVER: i32 = 1;
const NIB_APP: i32 = 1;

static APPLICATION: AtomicUsize = AtomicUsize::new(0);
static DELEGATE: AtomicUsize = AtomicUsize::new(0);
static KEY_WINDOW: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static LAUNCHED: AtomicBool = AtomicBool::new(false);
static LEFT_HELD: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);
static WINDOWS: Exclusive<Buffer<Id>> = Exclusive::new(Buffer::new());

/// The NSApplication instance, nil before `sharedApplication`.
pub fn application() -> Id {
    APPLICATION.load(Ordering::Acquire) as Id
}

/// The app's delegate.
pub fn delegate() -> Id {
    DELEGATE.load(Ordering::Acquire) as Id
}

/// The window that has the keyboard, nil for none.
pub fn key_window() -> Id {
    KEY_WINDOW.load(Ordering::Acquire) as Id
}

/// The app's shown window at `index`, in the order they were first shown.
pub fn window_at(index: usize) -> Option<Id> {
    WINDOWS.with(|windows| windows.as_slice().get(index).copied())
}

/// The app's window the server knows as `id`.
pub fn window_with_server_id(id: u32) -> Option<Id> {
    let mut index = 0;
    while let Some(window) = window_at(index) {
        if window::server_id(window) == id && id != 0 {
            return Some(window);
        }
        index += 1;
    }
    None
}

/// Says once that there is no window server to show windows on.
pub fn report_no_window_server() {
    if !REPORTED.swap(true, Ordering::Relaxed) {
        write_error(b"eggos-macos: no window server (/dev/window): this app needs the desktop\n");
    }
}

/// Notes that `window` got a server window: the first time it joins the app's windows (kept for
/// as long as the app runs); each time its server window gets the main menu.
pub fn window_shown(window: Id) {
    let listed = WINDOWS.with(|windows| windows.as_slice().contains(&window));
    if !listed {
        objc_retain(window);
        WINDOWS.with(|windows| windows.push(window));
    }
    menu::send_to(window);
}

/// The server took the keyboard from `window`.
pub fn focus_out(window: Id) {
    LEFT_HELD.store(false, Ordering::Relaxed);
    window::set_key(window, false);
}

/// Gives `window` the keyboard.
pub fn make_key(window: Id) {
    let previous = KEY_WINDOW.swap(window as usize, Ordering::AcqRel) as Id;
    if !previous.is_null() && previous != window {
        window::set_key(previous, false);
    }
    window::set_key(window, true);
}

fn any_shown() -> Option<Id> {
    let mut index = 0;
    while let Some(window) = window_at(index) {
        if window::server_id(window) != 0 {
            return Some(window);
        }
        index += 1;
    }
    None
}

/// Notes that `window` was closed (`closing`) or ordered out; when the last one closes the app
/// ends unless its delegate's `applicationShouldTerminateAfterLastWindowClosed:` answers NO (an
/// app without it ends too: with no window it would have no menu to quit from).
pub fn window_closed(window: Id, closing: bool) {
    if key_window() == window {
        let next = any_shown().unwrap_or(core::ptr::null_mut());
        KEY_WINDOW.store(next as usize, Ordering::Release);
    }
    if !closing || any_shown().is_some() {
        return;
    }
    let delegate = delegate();
    let should = sel!("applicationShouldTerminateAfterLastWindowClosed:");
    if responds(delegate, should) && !send_bool(delegate, should, application() as usize) {
        return;
    }
    terminate();
}

/// Draws every window in which something changed.
pub fn display_all() {
    let mut index = 0;
    while let Some(window) = window_at(index) {
        window::display_if_needed(window);
        index += 1;
    }
}

fn notify(sel: Sel) {
    let delegate = delegate();
    if responds(delegate, sel) {
        send1(delegate, sel, 0);
    }
}

fn finish_launching() {
    if LAUNCHED.swap(true, Ordering::AcqRel) {
        return;
    }
    notify(sel!("applicationWillFinishLaunching:"));
    notify(sel!("applicationDidFinishLaunching:"));
}

/// Ends the app as `terminate:` does, unless the delegate cancels.
pub fn terminate() {
    let delegate = delegate();
    let should = sel!("applicationShouldTerminate:");
    if responds(delegate, should)
        && send1(delegate, should, application() as usize) as usize == NS_TERMINATE_CANCEL
    {
        return;
    }
    notify(sel!("applicationWillTerminate:"));
    crate::libsystem::exit(0)
}

fn responder_for(window: Id, action: Sel) -> Option<Id> {
    if !window.is_null() {
        let mut current = window::first_responder(window);
        while !current.is_null() {
            if responds(current, action) {
                return Some(current);
            }
            current = view::fields(current).superview;
        }
        if responds(window, action) {
            return Some(window);
        }
        let window_delegate = window::fields(window).delegate;
        if responds(window_delegate, action) {
            return Some(window_delegate);
        }
    }
    [application(), delegate()]
        .into_iter()
        .find(|&candidate| responds(candidate, action))
}

/// Sends `action` from `sender` to `target`, or along the responder chain when `target` is
/// nil: the key window's first responder and its superviews, the window, its delegate, the
/// application and its delegate. Whether something took it.
pub fn send_action(action: Sel, target: Id, sender: Id) -> bool {
    send_action_in(key_window(), action, target, sender)
}

/// Sends `action` as [`send_action`] does, but along `window`'s responder chain (nil for none):
/// a menu item chosen in a window's menu acts in that window, key or not.
pub fn send_action_in(window: Id, action: Sel, target: Id, sender: Id) -> bool {
    if action.is_null() {
        return false;
    }
    let receiver = match target.is_null() {
        true => responder_for(window, action),
        false => responds(target, action).then_some(target),
    };
    match receiver {
        Some(receiver) => {
            send1(receiver, action, sender as usize);
            true
        }
        None => false,
    }
}

fn content_height(window: Id) -> i32 {
    window::fields(window).buffer.height as i32
}

fn deliver(event: Id) {
    let application = application();
    if !event.is_null() && !application.is_null() {
        send1(application, sel!("sendEvent:"), event as usize);
    }
}

fn mouse(window: Id, event_kind: usize, x: i32, y: i32, button: isize, delta: f64, time: u32) {
    let location = point_of_pixel(x, y, content_height(window));
    deliver(event::mouse(
        event_kind, window, location, button, delta, time,
    ));
}

/// Handles one of the window server's records outside a modal alert.
pub fn dispatch(record: &[u8; RECORD_SIZE]) {
    if let Some(chosen) = Chosen::from_record(record) {
        if let Some(window) = window_with_server_id(chosen.window) {
            menu::chosen(window, chosen.menu, chosen.entry);
        }
        return;
    }
    let Some(decoded) = input::decode(record) else {
        return;
    };
    let Some(window) = window_with_server_id(u32::from(decoded.window)) else {
        return;
    };
    let time = decoded.time_ms;
    match decoded.input {
        Input::PointerMove { x, y } => {
            let dragging = LEFT_HELD.load(Ordering::Relaxed);
            let event_kind = match dragging {
                true => kind::LEFT_MOUSE_DRAGGED,
                false => kind::MOUSE_MOVED,
            };
            mouse(window, event_kind, x, y, 0, 0.0, time);
        }
        Input::Button {
            x,
            y,
            button,
            pressed,
        } => {
            let (event_kind, number) = match (button, pressed) {
                (MouseButton::Left, true) => (kind::LEFT_MOUSE_DOWN, 0),
                (MouseButton::Left, false) => (kind::LEFT_MOUSE_UP, 0),
                (MouseButton::Right, true) => (kind::RIGHT_MOUSE_DOWN, 1),
                (MouseButton::Right, false) => (kind::RIGHT_MOUSE_UP, 1),
                (MouseButton::Middle, _) => return,
            };
            if button == MouseButton::Left {
                LEFT_HELD.store(pressed, Ordering::Relaxed);
            }
            mouse(window, event_kind, x, y, number, 0.0, time);
        }
        Input::Wheel { x, y, notches } => {
            mouse(
                window,
                kind::SCROLL_WHEEL,
                x,
                y,
                0,
                f64::from(notches),
                time,
            );
        }
        Input::Key {
            code,
            pressed,
            modifiers,
            character,
        } => deliver(event::key(
            window,
            code,
            pressed,
            modifiers.0,
            character,
            time,
        )),
        Input::FocusIn => make_key(window),
        Input::FocusOut => focus_out(window),
        Input::CloseRequest => {
            send1(window, sel!("performClose:"), 0);
        }
        Input::Resize { width, height } => window::resize(window, width, height),
        Input::PointerLeave | Input::StateChanged { .. } | Input::Overflow => {}
    }
}

/// Reads the waiting records (waiting for one) and hands each to `handle`; ends the app when
/// there is no window server.
pub fn read_records(handle: &mut dyn FnMut(&[u8; RECORD_SIZE])) {
    let mut records = [0u8; RECORD_SIZE * RECORDS_PER_READ];
    let read = match server::read_events(&mut records) {
        Ok(read) => read,
        Err(_) => {
            report_no_window_server();
            crate::libsystem::exit(NO_WINDOW_SERVER)
        }
    };
    for record in records[..read].as_chunks::<RECORD_SIZE>().0 {
        handle(record);
    }
}

fn run() {
    finish_launching();
    RUNNING.store(true, Ordering::Release);
    while RUNNING.load(Ordering::Acquire) {
        let pool = autorelease::push();
        menu::send_if_changed();
        display_all();
        read_records(&mut |record| dispatch(record));
        autorelease::pop(pool);
    }
}

extern "C" fn shared_application(this: Id, _: Sel) -> Id {
    let existing = application();
    if !existing.is_null() {
        return existing;
    }
    let made = send(send(this, sel!("alloc")), sel!("init"));
    if made.is_null() {
        return made;
    }
    APPLICATION.store(made as usize, Ordering::Release);
    made
}

extern "C" fn run_method(_: Id, _: Sel) {
    run();
}

extern "C" fn stop(_: Id, _: Sel, _: Id) {
    RUNNING.store(false, Ordering::Release);
}

extern "C" fn terminate_method(_: Id, _: Sel, _: Id) {
    terminate();
}

extern "C" fn delegate_method(_: Id, _: Sel) -> Id {
    delegate()
}

extern "C" fn set_delegate(_: Id, _: Sel, delegate: Id) {
    if !delegate.is_null() {
        objc_retain(delegate);
    }
    let old = DELEGATE.swap(delegate as usize, Ordering::AcqRel) as Id;
    if !old.is_null() {
        objc_release(old);
    }
}

extern "C" fn yes_for(_: Id, _: Sel, _: isize) -> u8 {
    1
}

extern "C" fn regular(_: Id, _: Sel) -> isize {
    0
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn nothing(_: Id, _: Sel) {}

extern "C" fn finish_launching_method(_: Id, _: Sel) {
    finish_launching();
}

extern "C" fn key_window_method(_: Id, _: Sel) -> Id {
    key_window()
}

extern "C" fn windows(_: Id, _: Sel) -> Id {
    let mut snapshot = Buffer::new();
    let mut index = 0;
    while let Some(window) = window_at(index) {
        snapshot.push(window);
        index += 1;
    }
    let array = send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        snapshot.as_slice().as_ptr() as usize,
        snapshot.len(),
    );
    snapshot.free();
    array
}

extern "C" fn send_action_method(_: Id, _: Sel, action: Sel, target: Id, sender: Id) -> u8 {
    u8::from(send_action(action, target, sender))
}

extern "C" fn send_event(this: Id, _: Sel, event: Id) {
    if event.is_null() {
        return;
    }
    let fields = event::fields(event);
    if fields.kind == kind::KEY_DOWN {
        if menu::perform_key_equivalent(event) {
            return;
        }
        let only_command = fields.flags & modifier::KEY_EQUIVALENT == modifier::COMMAND;
        if only_command && event::typed(event) == u32::from(b'q') {
            send1(this, sel!("terminate:"), this as usize);
            return;
        }
    }
    let window = fields.window;
    if !window.is_null() {
        send1(window, sel!("sendEvent:"), event as usize);
    }
}

extern "C" fn is_running(_: Id, _: Sel) -> u8 {
    u8::from(RUNNING.load(Ordering::Acquire))
}

extern "C" fn main_menu(_: Id, _: Sel) -> Id {
    menu::main_menu()
}

extern "C" fn set_main_menu(_: Id, _: Sel, main: Id) {
    menu::set_main_menu(main);
}

extern "C" fn nil(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn ignore_object(_: Id, _: Sel, _: Id) {}

extern "C" fn application_main(_: i32, _: usize) -> i32 {
    if [&b"NSMainNibFile"[..], &b"NSMainStoryboardFile"[..]]
        .into_iter()
        .any(|key| info_string(key).is_some_and(|name| !name.is_empty()))
    {
        write_error(
            b"eggos-macos: this app builds its interface from a nib or storyboard \
              (NSMainNibFile or NSMainStoryboardFile in its Info.plist), which EggOS cannot \
              load yet\n",
        );
        crate::libsystem::exit(NIB_APP);
    }
    let class = class_pointer(&NSAPPLICATION).cast();
    let application = send(class, sel!("sharedApplication"));
    send(application, sel!("run"));
    0
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_NSApp" => &raw const APPLICATION as usize,
        b"_NSApplicationMain" => application_main as *const () as usize,
        b"_NSBeep" => beep as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

extern "C" fn beep() {}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "run" => run_method,
        "stop:" => stop,
        "terminate:" => terminate_method,
        "delegate" => delegate_method,
        "setDelegate:" => set_delegate,
        "setActivationPolicy:" => yes_for,
        "activationPolicy" => regular,
        "activateIgnoringOtherApps:" => ignore_word,
        "activate" => nothing,
        "finishLaunching" => finish_launching_method,
        "mainMenu" => main_menu,
        "setMainMenu:" => set_main_menu,
        "keyWindow" => key_window_method,
        "mainWindow" => key_window_method,
        "windows" => windows,
        "sendAction:to:from:" => send_action_method,
        "sendEvent:" => send_event,
        "isRunning" => is_running,
        "orderFrontStandardAboutPanel:" => ignore_object,
        "hide:" => ignore_object,
        "unhide:" => ignore_object,
        "arrangeInFront:" => ignore_object,
        "setApplicationIconImage:" => ignore_object,
        "setServicesMenu:" => ignore_object,
        "setWindowsMenu:" => ignore_object,
        "setHelpMenu:" => ignore_object,
        "servicesMenu" => nil,
        "windowsMenu" => nil,
        "helpMenu" => nil,
        "currentEvent" => nil,
    ];
    let class = methods![
        "sharedApplication" => shared_application,
    ];
    super::define(
        runtime,
        b"NSApplication\0",
        (&NSAPPLICATION, &NSAPPLICATION_META),
        &NSRESPONDER,
        core::mem::size_of::<usize>(),
        &instance,
        &class,
    )
}
