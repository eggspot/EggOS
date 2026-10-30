//! Input a program sends itself and what it learns of the pointer and its threads' input
//! (D178): `SendInput`, `keybd_event` and `mouse_event` reach the program's own window that has
//! the keyboard, as the window server's input would, through its hot keys, menus and combo
//! lists; input meant for another app is not allowed (the window server keeps each client to
//! its own windows), so it fails with `ERROR_ACCESS_DENIED`. The pointer's place is known while
//! it is over one of the program's windows (`GetCursorPos`, `GetCursorInfo`, `GetMessagePos`).
//! `GetLastInputInfo`, `GetGUIThreadInfo` and `AttachThreadInput` answer for the program's own
//! threads, which share one input state; `BlockInput` is not allowed.

use super::gui::{self, Input, with};
use super::{Bool, FALSE, TRUE, error, fail, placement, thread_environment};
use crate::gui::input::mouse_keys;
use crate::gui::keyboard::{self, Modifiers};
use crate::gui::message::{self, point_parameter};
use crate::gui::objects::{self, cursor};
use crate::gui::windows;
use crate::library::Export;

const INPUT_MOUSE: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_EXTENDEDKEY: u32 = 0x1;
const KEYEVENTF_KEYUP: u32 = 0x2;
const KEYEVENTF_UNICODE: u32 = 0x4;
const KEYEVENTF_SCANCODE: u32 = 0x8;
const MOUSEEVENTF_MOVE: u32 = 0x1;
const MOUSEEVENTF_WHEEL: u32 = 0x800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSE_BUTTONS: [(u32, usize, u32, bool); 6] = [
    (0x2, mouse_keys::LEFT, message::LEFT_BUTTON_DOWN, true),
    (0x4, mouse_keys::LEFT, message::LEFT_BUTTON_UP, false),
    (0x8, mouse_keys::RIGHT, message::RIGHT_BUTTON_DOWN, true),
    (0x10, mouse_keys::RIGHT, message::RIGHT_BUTTON_UP, false),
    (0x20, mouse_keys::MIDDLE, message::MIDDLE_BUTTON_DOWN, true),
    (0x40, mouse_keys::MIDDLE, message::MIDDLE_BUTTON_UP, false),
];
const NORMALISED: i64 = 65536;
const VK_PACKET: u8 = 0xe7;
const VK_MENU: u8 = 0x12;
const VK_F10: u8 = 0x79;
const EXTENDED: u16 = 0xe000;
const CURSOR_SHOWING: u32 = 0x1;
const GUI_INMENUMODE: u32 = 0x4;
const GUI_POPUPMENUMODE: u32 = 0x10;

#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KeyboardInput {
    key: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

/// Win32's `INPUT`: its kind, then a `MOUSEINPUT`, `KEYBDINPUT` or `HARDWAREINPUT`.
#[repr(C)]
struct InputRecord {
    kind: u32,
    payload: MouseInput,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

/// The program's top-level window that has the keyboard, which the input it sends reaches.
fn active_root() -> Option<usize> {
    with(|gui| {
        let root = gui.active_root;
        gui.windows.get(root).map(|_| root)
    })
    .flatten()
}

/// Where the pointer is on the screen, as far as the program knows: where it last was over one
/// of its windows.
pub(super) fn cursor_position() -> Option<(i32, i32)> {
    let (server, point) = with(|gui| gui.input.pointer()).flatten()?;
    let root = with(|gui| windows::by_server(&gui.windows, u32::from(server))).flatten()?;
    let (left, top) = placement::content_origin(root).ok()?;
    Some((left.saturating_add(point.0), top.saturating_add(point.1)))
}

fn send_key(root: usize, key: &KeyboardInput) {
    let released = key.flags & KEYEVENTF_KEYUP != 0;
    let unicode = key.flags & KEYEVENTF_UNICODE != 0;
    let (virtual_key, scan) = if unicode {
        (VK_PACKET, 0)
    } else if key.flags & KEYEVENTF_SCANCODE != 0 {
        let scan = match key.flags & KEYEVENTF_EXTENDEDKEY {
            0 => key.scan,
            _ => EXTENDED | (key.scan & 0xff),
        };
        (keyboard::key_of_scan(scan).unwrap_or(0), scan)
    } else {
        let virtual_key = key.key as u8;
        let scan = keyboard::scan_of(virtual_key).unwrap_or(key.scan);
        (virtual_key, scan)
    };
    let held = with(|gui| {
        if !unicode {
            gui.input.press_key(virtual_key, !released);
        }
        let state = gui.keyboard_state();
        Modifiers {
            shift: state[0x10] & 0x80 != 0,
            control: state[0x11] & 0x80 != 0,
            alt: state[0x12] & 0x80 != 0,
            caps_lock: state[0x14] & 0x01 != 0,
        }
    });
    let Some(held) = held else {
        return;
    };
    let system = (held.alt && !held.control) || virtual_key == VK_MENU || virtual_key == VK_F10;
    let number = match (released, system) {
        (false, false) => message::KEY_DOWN,
        (true, false) => message::KEY_UP,
        (false, true) => message::SYS_KEY_DOWN,
        (true, true) => message::SYS_KEY_UP,
    };
    let character = match (released, unicode) {
        (true, _) => None,
        (false, true) => Some(u32::from(key.scan)),
        (false, false) => keyboard::character(virtual_key, held).map(u32::from),
    };
    let input = Input {
        number,
        wparam: usize::from(virtual_key),
        lparam: keyboard::key_lparam(scan, released, held.alt),
        point: (0, 0),
        character,
        time: super::user32::now_ms(),
    };
    gui::inject(root, &input);
}

fn send_mouse(root: usize, mouse: &MouseInput) -> Result<(), u32> {
    let moves = mouse.flags & MOUSEEVENTF_MOVE != 0;
    let (x, y) = match (moves, mouse.flags & MOUSEEVENTF_ABSOLUTE != 0) {
        (true, true) => {
            let screen = gui::device()?.work_area;
            let scale =
                |value: i32, size: i32| (i64::from(value) * i64::from(size) / NORMALISED) as i32;
            (
                scale(mouse.dx, screen.right),
                scale(mouse.dy, screen.bottom),
            )
        }
        (true, false) => {
            let (x, y) = cursor_position().ok_or(error::ACCESS_DENIED)?;
            (x.saturating_add(mouse.dx), y.saturating_add(mouse.dy))
        }
        (false, _) => cursor_position().ok_or(error::ACCESS_DENIED)?,
    };
    let target = super::window_tree::window_at(x, y)
        .and_then(|window| with(|gui| windows::root_of(&gui.windows, window)).flatten());
    if target != Some(root) {
        return Err(error::ACCESS_DENIED);
    }
    let (left, top) = placement::content_origin(root)?;
    let point = (x - left, y - top);
    let server = with(|gui| gui.windows.get(root).map(|window| window.server))
        .flatten()
        .ok_or(error::INVALID_WINDOW_HANDLE)?;
    let send = |number: u32, wparam: usize| {
        let input = Input {
            number,
            wparam,
            lparam: point_parameter(point.0, point.1),
            point,
            character: None,
            time: super::user32::now_ms(),
        };
        gui::inject(root, &input);
    };
    let keys = |button: Option<(usize, bool)>| {
        with(|gui| {
            if let Some((button, pressed)) = button {
                gui.input.press_button(button, pressed);
            }
            gui.input.move_pointer(server as u16, point);
            gui.input.mouse_keys()
        })
        .unwrap_or(0)
    };
    if moves {
        send(message::MOUSE_MOVE, keys(None));
    }
    for (flag, button, number, pressed) in MOUSE_BUTTONS {
        if mouse.flags & flag != 0 {
            send(number, keys(Some((button, pressed))));
        }
    }
    for (flag, number) in [
        (MOUSEEVENTF_WHEEL, message::MOUSE_WHEEL),
        (MOUSEEVENTF_HWHEEL, message::MOUSE_HORIZONTAL_WHEEL),
    ] {
        if mouse.flags & flag != 0 {
            let delta = usize::from(mouse.data as i16 as u16) << 16;
            send(number, delta | keys(None));
        }
    }
    Ok(())
}

unsafe extern "efiapi" fn send_input(count: u32, inputs: *const InputRecord, size: i32) -> u32 {
    if inputs.is_null() || size as usize != core::mem::size_of::<InputRecord>() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let Some(root) = active_root() else {
        return fail(error::ACCESS_DENIED, 0);
    };
    for index in 0..count as usize {
        // SAFETY: the caller passes `count` readable `INPUT`s of the size it gave.
        let record = unsafe { &*inputs.add(index) };
        let sent = match record.kind {
            INPUT_KEYBOARD => {
                // SAFETY: a keyboard `INPUT`'s payload is a `KEYBDINPUT`, which fits in the
                // union and shares its alignment.
                let key = unsafe { &*(&raw const record.payload).cast::<KeyboardInput>() };
                send_key(root, key);
                Ok(())
            }
            INPUT_MOUSE => send_mouse(root, &record.payload),
            _ => Err(error::NOT_SUPPORTED),
        };
        if let Err(code) = sent {
            return match index {
                0 => fail(code, 0),
                done => done as u32,
            };
        }
    }
    count
}

extern "efiapi" fn keybd_event(key: u8, scan: u8, flags: u32, extra: usize) {
    if let Some(root) = active_root() {
        let input = KeyboardInput {
            key: u16::from(key),
            scan: u16::from(scan),
            flags,
            time: 0,
            extra,
        };
        send_key(root, &input);
    }
}

extern "efiapi" fn mouse_event(flags: u32, dx: i32, dy: i32, data: u32, extra: usize) {
    if let Some(root) = active_root() {
        let input = MouseInput {
            dx,
            dy,
            data,
            flags,
            time: 0,
            extra,
        };
        let _ = send_mouse(root, &input);
    }
}

unsafe extern "efiapi" fn get_cursor_pos(out: *mut Point) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match cursor_position() {
        Some((x, y)) => {
            // SAFETY: the caller passes a writable `POINT`.
            unsafe { out.write(Point { x, y }) };
            TRUE
        }
        None => fail(error::ACCESS_DENIED, FALSE),
    }
}

#[repr(C)]
struct CursorInfo {
    size: u32,
    flags: u32,
    cursor: usize,
    position: Point,
}

unsafe extern "efiapi" fn get_cursor_info(out: *mut CursorInfo) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `CURSORINFO`.
    let info = unsafe { &mut *out };
    if info.size as usize != core::mem::size_of::<CursorInfo>() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let Some((x, y)) = cursor_position() else {
        return fail(error::ACCESS_DENIED, FALSE);
    };
    info.flags = CURSOR_SHOWING;
    info.cursor = objects::cursor_handle(true, cursor::ARROW);
    info.position = Point { x, y };
    TRUE
}

extern "efiapi" fn get_message_pos() -> u32 {
    cursor_position().map_or(0, |(x, y)| {
        (u32::from(y as u16) << 16) | u32::from(x as u16)
    })
}

#[repr(C)]
struct LastInputInfo {
    size: u32,
    time: u32,
}

unsafe extern "efiapi" fn get_last_input_info(out: *mut LastInputInfo) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `LASTINPUTINFO`.
    let info = unsafe { &mut *out };
    if info.size as usize != core::mem::size_of::<LastInputInfo>() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match with(|gui| gui.last_input) {
        Some(time) => {
            info.time = time;
            TRUE
        }
        None => fail(error::NOT_ENOUGH_MEMORY, FALSE),
    }
}

extern "efiapi" fn block_input(_block: Bool) -> Bool {
    fail(error::ACCESS_DENIED, FALSE)
}

fn is_own_thread(thread: u32) -> bool {
    thread == thread_environment::thread_id()
        || with(|gui| {
            gui.windows
                .iter()
                .any(|(_, window)| window.thread == thread)
        })
        .unwrap_or(false)
}

extern "efiapi" fn attach_thread_input(from: u32, to: u32, _attach: Bool) -> Bool {
    match is_own_thread(from) && is_own_thread(to) {
        true => TRUE,
        false => fail(error::INVALID_PARAMETER, FALSE),
    }
}

#[repr(C)]
struct GuiThreadInfo {
    size: u32,
    flags: u32,
    active: usize,
    focus: usize,
    capture: usize,
    menu_owner: usize,
    move_size: usize,
    caret: usize,
    caret_rect: [i32; 4],
}

unsafe extern "efiapi" fn get_gui_thread_info(thread: u32, out: *mut GuiThreadInfo) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `GUITHREADINFO`.
    let info = unsafe { &mut *out };
    if info.size as usize != core::mem::size_of::<GuiThreadInfo>() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if thread != 0 && !is_own_thread(thread) {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let found = with(|gui| {
        let active = gui.active_root;
        let owner = gui.windows.get(active).map(|window| window.thread)?;
        if thread != 0 && thread != owner {
            return None;
        }
        let capture = gui.windows.get(active).map_or(0, |window| window.capture);
        let menu = gui
            .tracking
            .filter(|tracking| tracking.root == active)
            .map(|tracking| (tracking.root, tracking.context));
        Some((active, gui.key_target(active), capture, menu))
    })
    .flatten();
    let (active, focus, capture, menu) = found.unwrap_or((0, 0, 0, None));
    info.flags = match menu {
        Some((_, true)) => GUI_INMENUMODE | GUI_POPUPMENUMODE,
        Some((_, false)) => GUI_INMENUMODE,
        None => 0,
    };
    info.active = active;
    info.focus = focus;
    info.capture = capture;
    info.menu_owner = menu.map_or(0, |(owner, _)| owner);
    info.move_size = 0;
    info.caret = 0;
    info.caret_rect = [0; 4];
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SendInput" => send_input,
    b"keybd_event" => keybd_event,
    b"mouse_event" => mouse_event,
    b"GetCursorPos" => get_cursor_pos,
    b"GetCursorInfo" => get_cursor_info,
    b"GetMessagePos" => get_message_pos,
    b"GetLastInputInfo" => get_last_input_info,
    b"BlockInput" => block_input,
    b"AttachThreadInput" => attach_thread_input,
    b"GetGUIThreadInfo" => get_gui_thread_info,
];
