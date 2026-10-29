//! user32's windows and messages (D129): classes, windows on the desktop through the window
//! server, the message loop's functions, window procedures and `DefWindowProcW`, the window
//! values `GetWindowLongPtrW` reads, the system metrics and the cursor and icon handles; child
//! windows and the system's control classes (D145), which have no server window of their own.
//! Painting is in `paint.rs`, the message box in `message_box.rs`, the controls in
//! `controls.rs`, menus in `menu.rs`.

use super::gui::{self, device, send, try_with, with};
use super::window_device::{Buffer, FIXED_SIZE, ROLE_NORMAL};
use super::{
    Bool, FALSE, TRUE, call_program, controls, error, error_for, fail, paint, thread_environment,
    window_text,
};
use crate::gui::classes::{Class, ClassError, ClassName};
use crate::gui::message::{self, point_parameter};
use crate::gui::objects;
use crate::gui::queue::{Filter, Message};
use crate::gui::windows::{self, ControlKind, MAX_EXTRA, Procedure, Window};
use crate::library::Export;
use crate::linux;
use crate::text;
use eggos_windows_gui::surface::Rect;

const CW_USE_DEFAULT: i32 = i32::MIN;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_DISABLED: u32 = 0x0800_0000;
const WS_MAXIMIZE: u32 = 0x0100_0000;
const WS_THICK_FRAME: u32 = 0x0004_0000;
const FRAME: i32 = 1;
const CAPTION: i32 = 28;
const DEFAULT_SHARE: (i32, i32) = (3, 4);
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_SHOW_MAXIMIZED: i32 = 3;
const PM_REMOVE: u32 = 1;
const ATOM_LIMIT: usize = 0x1_0000;
const TITLE_BYTES: usize = 64 * 4;
const MINIMUM_OUTSIDE: (i32, i32) = (160, 92);
const DOUBLE_CLICK_SQUARE: i32 = 8;
const MOUSE_BUTTONS: i32 = 3;
const ICON_SIZE: i32 = 32;
const SMALL_ICON_SIZE: i32 = 16;

#[repr(C)]
struct WindowClassEx {
    size: u32,
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu_name: *const u16,
    class_name: *const u16,
    small_icon: usize,
}

#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct CreateStruct {
    create_parameters: usize,
    instance: usize,
    menu: usize,
    parent: usize,
    height: i32,
    width: i32,
    y: i32,
    x: i32,
    style: u32,
    name: *const u16,
    class_name: *const u16,
    ex_style: u32,
}

/// Win32's `MSG`, as programs pass it.
#[repr(C)]
pub struct Msg {
    window: usize,
    message: u32,
    wparam: usize,
    lparam: usize,
    time: u32,
    x: i32,
    y: i32,
    private: u32,
}

/// Win32's `RECT`, as programs pass it.
#[repr(C)]
pub struct Win32Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The tick count messages are stamped with, in milliseconds.
pub fn now_ms() -> u32 {
    (linux::clock_nanoseconds(linux::CLOCK_MONOTONIC) / 1_000_000) as u32
}

unsafe fn class_name<'a>(name: *const u16) -> ClassName<'a> {
    match (name as usize) < ATOM_LIMIT {
        true => ClassName::Atom(name as usize as u16),
        // SAFETY: the caller's duty: a NUL-terminated UTF-16 class name.
        false => ClassName::Name(unsafe { super::file::wide_string(name) }),
    }
}

fn class_error(error: ClassError) -> u32 {
    match error {
        ClassError::AlreadyExists => error::CLASS_ALREADY_EXISTS,
        ClassError::NotFound => error::CLASS_DOES_NOT_EXIST,
        ClassError::TableFull => error::NOT_ENOUGH_MEMORY,
        ClassError::InvalidName => error::INVALID_PARAMETER,
    }
}

unsafe fn register(name: *const u16, class: Class) -> u16 {
    if (name as usize) < ATOM_LIMIT || class.window_extra as usize > MAX_EXTRA {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's duty: a NUL-terminated UTF-16 class name.
    let name = unsafe { super::file::wide_string(name) };
    match try_with(|gui| gui.classes.register(name, class).map_err(class_error)) {
        Ok(atom) => atom,
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn register_class_ex_w(class: *const WindowClassEx) -> u16 {
    // SAFETY: the caller passes a `WNDCLASSEXW`.
    let class = unsafe { &*class };
    if class.size as usize != core::mem::size_of::<WindowClassEx>() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let registered = Class {
        style: class.style,
        procedure: class.procedure,
        class_extra: class.class_extra.max(0) as u32,
        window_extra: class.window_extra.max(0) as u32,
        instance: class.instance,
        icon: class.icon,
        cursor: class.cursor,
        background: class.background,
    };
    // SAFETY: the class's name is the caller's NUL-terminated string.
    unsafe { register(class.class_name, registered) }
}

unsafe extern "efiapi" fn register_class_w(class: *const WindowClass) -> u16 {
    // SAFETY: the caller passes a `WNDCLASSW`.
    let class = unsafe { &*class };
    let registered = Class {
        style: class.style,
        procedure: class.procedure,
        class_extra: class.class_extra.max(0) as u32,
        window_extra: class.window_extra.max(0) as u32,
        instance: class.instance,
        icon: class.icon,
        cursor: class.cursor,
        background: class.background,
    };
    // SAFETY: the class's name is the caller's NUL-terminated string.
    unsafe { register(class.class_name, registered) }
}

unsafe extern "efiapi" fn unregister_class_w(name: *const u16, _instance: usize) -> Bool {
    // SAFETY: the caller's duty: an atom or a NUL-terminated name.
    let name = unsafe { class_name(name) };
    let removed = try_with(|gui| {
        let (atom, _) = gui.classes.find(name).ok_or(error::CLASS_DOES_NOT_EXIST)?;
        if gui
            .windows
            .iter()
            .any(|(_, window)| window.class_atom == atom)
        {
            return Err(error::CLASS_HAS_WINDOWS);
        }
        gui.classes.unregister(name).map_err(class_error)
    });
    match removed {
        Ok(_) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn title(name: *const u16, out: &mut [u8; TITLE_BYTES]) -> &str {
    if name.is_null() {
        return "";
    }
    // SAFETY: the window name is the caller's NUL-terminated string.
    let units = unsafe { super::file::wide_string(name) };
    let (length, _) = text::write_utf8(units, out);
    core::str::from_utf8(&out[..length]).unwrap_or("")
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn create_window_ex_w(
    ex_style: u32,
    class: *const u16,
    window_name: *const u16,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: usize,
    menu: usize,
    instance: usize,
    parameter: usize,
) -> usize {
    // SAFETY: the caller's duty: an atom or a NUL-terminated class name.
    let name = unsafe { class_name(class) };
    let program_class = try_with(|gui| {
        gui.classes
            .find(name)
            .ok_or(error::CANNOT_FIND_WINDOW_CLASS)
    });
    let system_class = match name {
        ClassName::Name(name) => controls::system_class(name),
        ClassName::Atom(_) => None,
    };
    let window_text = match window_name.is_null() {
        true => &[][..],
        // SAFETY: the window name is the caller's NUL-terminated string.
        false => unsafe { super::file::wide_string(window_name) },
    };
    if style & WS_CHILD != 0 {
        let created = match (program_class, system_class) {
            (Ok((atom, class_of)), _) => create_program_child(
                atom,
                &class_of,
                ChildRequest {
                    parent,
                    style,
                    ex_style,
                    bounds: child_bounds(x, y, width, height),
                    id: menu,
                    instance,
                    text: window_text,
                },
            ),
            (Err(_), Some(kind)) => create_child(
                parent,
                kind,
                style,
                child_bounds(x, y, width, height),
                menu,
                window_text,
            )
            .ok_or(error::NOT_ENOUGH_MEMORY),
            (Err(code), None) => Err(code),
        };
        let handle = match created {
            Ok(handle) => handle,
            Err(code) => return fail(code, 0),
        };
        let create = CreateStruct {
            create_parameters: parameter,
            instance,
            menu,
            parent,
            height,
            width,
            y,
            x,
            style,
            name: window_name,
            class_name: class,
            ex_style,
        };
        let create = &raw const create as usize;
        if send(handle, message::NC_CREATE, 0, create) == 0
            || send(handle, message::CREATE, 0, create) as isize == -1
        {
            destroy_tree(handle);
            return 0;
        }
        return handle;
    }
    let Ok((atom, class_of)) = program_class else {
        let code = match system_class {
            Some(_) => error::NOT_SUPPORTED,
            None => error::CANNOT_FIND_WINDOW_CLASS,
        };
        return fail(code, 0);
    };
    let device = match device() {
        Ok(device) => device,
        Err(code) => return fail(code, 0),
    };
    let work = device.work_area;
    let (outside_width, outside_height) = match width {
        CW_USE_DEFAULT => (
            work.width() * DEFAULT_SHARE.0 / DEFAULT_SHARE.1,
            work.height() * DEFAULT_SHARE.0 / DEFAULT_SHARE.1,
        ),
        _ => (width, height),
    };
    let flags = if style & WS_THICK_FRAME == 0 {
        FIXED_SIZE
    } else {
        0
    };
    let buffer = match device.create(
        ROLE_NORMAL,
        0,
        outside_width.saturating_sub(2 * FRAME).max(1) as u32,
        outside_height.saturating_sub(CAPTION + 2 * FRAME).max(1) as u32,
        flags,
    ) {
        Ok(buffer) => buffer,
        Err(errno) => return fail(error_for(errno), 0),
    };
    let mut title_bytes = [0u8; TITLE_BYTES];
    let _ = device.set_title(buffer.window, title(window_name, &mut title_bytes));
    if x != CW_USE_DEFAULT {
        let _ = device.set_position(
            buffer.window,
            x.saturating_sub(work.left),
            y.saturating_sub(work.top),
        );
    }
    let shape = objects::pointer_shape(class_of.cursor);
    if shape != 0 {
        let _ = device.set_pointer(buffer.window, shape);
    }
    let mut window = Window::new(
        Procedure::Program(class_of.procedure),
        buffer.window,
        buffer.pixels,
        buffer.width,
        buffer.height,
    );
    window.class_atom = atom;
    window.style = style;
    window.ex_style = ex_style;
    window.instance = instance;
    window.id = menu;
    window.owner = parent;
    window.extra_length = class_of.window_extra as usize;
    window.background = class_of.background;
    window.class_style = class_of.style;
    window.enabled = style & WS_DISABLED == 0;
    window.thread = thread_environment::thread_id();
    let Some(Some(handle)) = with(|gui| gui.windows.insert(window)) else {
        // SAFETY: the window never reached the table, so nothing holds its buffer.
        unsafe { device.destroy(&buffer) };
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    with(|gui| window_text::store(gui, handle, window_text));
    if menu != 0 && try_with(|gui| super::menu::attach(gui, handle, menu)).is_err() {
        remove_window(handle);
        return fail(error::INVALID_MENU_HANDLE, 0);
    }
    let create = CreateStruct {
        create_parameters: parameter,
        instance,
        menu,
        parent,
        height: outside_height,
        width: outside_width,
        y,
        x,
        style,
        name: window_name,
        class_name: class,
        ex_style,
    };
    let create = &raw const create as usize;
    if send(handle, message::NC_CREATE, 0, create) == 0
        || send(handle, message::CREATE, 0, create) as isize == -1
    {
        destroy_tree(handle);
        return 0;
    }
    if style & WS_VISIBLE != 0 {
        let command = match style & WS_MAXIMIZE {
            0 => SW_SHOW,
            _ => SW_SHOW_MAXIMIZED,
        };
        show_window(handle, command);
    }
    handle
}

/// Takes `handle` out of the program's windows, with what it holds (its timers, its text, a
/// control's state, a top-level window's menu), and has the server destroy a top-level one;
/// a child's area in its parent is left to repaint.
pub fn remove_window(handle: usize) {
    let removed = with(|gui| {
        gui.queue.forget(handle);
        gui.contexts.retain(|context| context.window != handle);
        gui.timers.kill_window(handle);
        gui.drawn.forget_window(handle);
        let window = gui.windows.remove(handle)?;
        if window.is_child() {
            let parent = window.parent;
            if let Some(root) = windows::root_of(&gui.windows, parent)
                && let Some(top) = gui.windows.get_mut(root)
            {
                if top.focus == handle {
                    top.focus = 0;
                }
                if top.capture == handle {
                    top.capture = 0;
                }
            }
            if gui
                .windows
                .get(parent)
                .is_some_and(|parent| !parent.destroying)
            {
                windows::invalidate_tree(&mut gui.windows, parent, Some(window.bounds), true);
            }
        } else {
            super::menu::forget_window(gui, handle, window.menu);
            if gui.active_root == handle {
                gui.active_root = 0;
            }
        }
        let buffer = Buffer {
            window: window.server,
            width: window.buffer_width,
            height: window.buffer_height,
            pixels: window.pixels,
        };
        Some((gui.device, window, buffer))
    })
    .flatten();
    let Some((device, window, buffer)) = removed else {
        return;
    };
    window_text::free(window.text);
    if let Procedure::Control(_, state) = window.procedure {
        controls::free_state(state);
    }
    if !window.is_child()
        && let Some(device) = device
    {
        // SAFETY: the window is out of the table, so nothing draws on its buffer any more.
        unsafe { device.destroy(&buffer) };
    }
    gui::wake();
}

/// Destroys `handle` and its children as `DestroyWindow` does: `WM_DESTROY` to the window and
/// then to its children, `WM_NCDESTROY` to the children first, each removed after its
/// `WM_NCDESTROY`.
pub fn destroy_tree(handle: usize) {
    let tree = with(|gui| {
        let (handles, count) = windows::tree(&gui.windows, handle);
        for &member in &handles[..count] {
            if let Some(window) = gui.windows.get_mut(member) {
                window.destroying = true;
            }
        }
        (handles, count)
    });
    let Some((handles, count)) = tree else {
        return;
    };
    for &member in &handles[..count] {
        send(member, message::DESTROY, 0, 0);
    }
    for &member in handles[..count].iter().rev() {
        send(member, message::NC_DESTROY, 0, 0);
        remove_window(member);
    }
}

struct ChildRequest<'a> {
    parent: usize,
    style: u32,
    ex_style: u32,
    bounds: Rect,
    id: usize,
    instance: usize,
    text: &'a [u16],
}

static NEXT_SEQUENCE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

fn child_bounds(x: i32, y: i32, width: i32, height: i32) -> Rect {
    let or_zero = |value: i32| if value == CW_USE_DEFAULT { 0 } else { value };
    Rect::sized(
        or_zero(x),
        or_zero(y),
        or_zero(width).max(0),
        or_zero(height).max(0),
    )
}

fn insert_child(mut window: Window, text: &[u16]) -> Result<usize, u32> {
    let parent = window.parent;
    try_with(|gui| {
        if gui.window(parent)?.destroying {
            return Err(error::INVALID_WINDOW_HANDLE);
        }
        let depth = windows::ancestry(&gui.windows, parent)
            .map_or(windows::MAX_DEPTH, |(_, length)| length);
        if depth >= windows::MAX_DEPTH {
            return Err(error::NOT_SUPPORTED);
        }
        window.thread = thread_environment::thread_id();
        window.creation_sequence =
            NEXT_SEQUENCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let handle = gui.windows.insert(window).ok_or(error::NOT_ENOUGH_MEMORY)?;
        if !window_text::store(gui, handle, text) {
            gui.windows.remove(handle);
            return Err(error::NOT_ENOUGH_MEMORY);
        }
        if window.shown {
            windows::invalidate_tree(&mut gui.windows, handle, None, true);
        }
        Ok(handle)
    })
}

fn create_program_child(
    atom: u16,
    class_of: &Class,
    request: ChildRequest<'_>,
) -> Result<usize, u32> {
    let mut window = Window::child(
        Procedure::Program(class_of.procedure),
        request.parent,
        request.bounds,
    );
    window.class_atom = atom;
    window.style = request.style;
    window.ex_style = request.ex_style;
    window.instance = request.instance;
    window.id = request.id;
    window.extra_length = class_of.window_extra as usize;
    window.background = class_of.background;
    window.class_style = class_of.style;
    window.shown = request.style & WS_VISIBLE != 0;
    window.enabled = request.style & WS_DISABLED == 0;
    insert_child(window, request.text)
}

/// Creates a control of `kind` as a child of `parent` at `bounds` with `style`, control id
/// `id` and the text `text`; `None` when the parent is gone or there is no memory.
pub fn create_child(
    parent: usize,
    kind: ControlKind,
    style: u32,
    bounds: Rect,
    id: usize,
    text: &[u16],
) -> Option<usize> {
    let state = controls::new_state(kind, style)?;
    let mut window = Window::child(Procedure::Control(kind, state), parent, bounds);
    window.style = style;
    window.id = id;
    window.shown = style & WS_VISIBLE != 0;
    window.enabled = style & WS_DISABLED == 0;
    match insert_child(window, text) {
        Ok(handle) => {
            if kind == ControlKind::Edit {
                controls::start_edit_text(handle, text);
            }
            gui::wake();
            Some(handle)
        }
        Err(_) => {
            controls::free_state(state);
            None
        }
    }
}

extern "efiapi" fn destroy_window(handle: usize) -> Bool {
    let started = try_with(|gui| {
        let window = gui.window_mut(handle)?;
        if matches!(
            window.procedure,
            Procedure::MessageBox(_) | Procedure::FilePicker(_)
        ) {
            return Err(error::ACCESS_DENIED);
        }
        if window.destroying {
            return Err(error::INVALID_WINDOW_HANDLE);
        }
        window.destroying = true;
        Ok(())
    });
    if let Err(code) = started {
        return fail(code, FALSE);
    }
    destroy_tree(handle);
    TRUE
}

extern "efiapi" fn is_window(handle: usize) -> Bool {
    match try_with(|gui| gui.window(handle).map(|window| window.destroying)) {
        Ok(false) => TRUE,
        _ => FALSE,
    }
}

extern "efiapi" fn show_window(handle: usize, command: i32) -> Bool {
    let state = match command {
        2 | 6 | 7 | 11 => Some(1),
        SW_SHOW_MAXIMIZED => Some(2),
        9 => Some(0),
        _ => None,
    };
    if let Ok(Some(result)) = try_with(|gui| Ok(show_child(gui, handle, command))) {
        gui::wake();
        return result;
    }
    let shown = try_with(|gui| {
        let device = gui.device;
        let window = gui.window_mut(handle)?;
        let was_shown = window.shown;
        if command == SW_HIDE {
            return Ok((was_shown, None));
        }
        if !was_shown {
            window.shown = true;
            window.invalidate(None, true);
        }
        let size = point_parameter(window.width as i32, window.height as i32);
        Ok((
            was_shown,
            Some((device, window.server, window.size_kind, size)),
        ))
    });
    let (was_shown, change) = match shown {
        Ok(shown) => shown,
        Err(code) => return fail(code, FALSE),
    };
    let Some((device, server, size_kind, size)) = change else {
        return Bool::from(was_shown);
    };
    if let (Some(device), Some(state)) = (device, state) {
        let _ = device.set_state(server, state);
    }
    if !was_shown {
        send(handle, message::SHOW_WINDOW, 1, 0);
        send(handle, message::SIZE, size_kind, size);
        gui::wake();
    }
    Bool::from(was_shown)
}

fn show_child(gui: &mut gui::Gui, handle: usize, command: i32) -> Option<Bool> {
    let window = gui.windows.get_mut(handle)?;
    if !window.is_child() {
        return None;
    }
    let was_shown = window.shown;
    let shown = command != SW_HIDE;
    window.shown = shown;
    let (parent, bounds) = (window.parent, window.bounds);
    if shown && !was_shown {
        windows::invalidate_tree(&mut gui.windows, handle, None, true);
    }
    if !shown && was_shown {
        windows::invalidate_tree(&mut gui.windows, parent, Some(bounds), true);
    }
    Some(Bool::from(was_shown))
}

extern "efiapi" fn update_window(handle: usize) -> Bool {
    match try_with(|gui| gui.window(handle).map(|window| window.needs_paint())) {
        Ok(true) => {
            send(handle, message::PAINT, 0, 0);
            TRUE
        }
        Ok(false) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_client_rect(handle: usize, rect: *mut Win32Rect) -> Bool {
    match try_with(|gui| {
        gui.window(handle)
            .map(|window| (window.width, window.height))
    }) {
        Ok((width, height)) => {
            // SAFETY: the caller passes a writable `RECT`.
            unsafe {
                rect.write(Win32Rect {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                })
            };
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn adjust_window_rect_ex(
    rect: *mut Win32Rect,
    _style: u32,
    menu: Bool,
    _ex_style: u32,
) -> Bool {
    // SAFETY: the caller passes a readable and writable `RECT`.
    let rect = unsafe { &mut *rect };
    let bar = match menu {
        0 => 0,
        _ => eggos_windows_gui::menu::BAR_HEIGHT,
    };
    rect.left = rect.left.saturating_sub(FRAME);
    rect.top = rect.top.saturating_sub(CAPTION + FRAME + bar);
    rect.right = rect.right.saturating_add(FRAME);
    rect.bottom = rect.bottom.saturating_add(FRAME);
    TRUE
}

unsafe extern "efiapi" fn adjust_window_rect(rect: *mut Win32Rect, style: u32, menu: Bool) -> Bool {
    // SAFETY: the caller's duty, passed on.
    unsafe { adjust_window_rect_ex(rect, style, menu, 0) }
}

fn window_long(handle: usize, index: i32, size: usize) -> usize {
    let value = try_with(|gui| {
        gui.window(handle)?
            .long(index, size)
            .ok_or(error::INVALID_INDEX)
    });
    match value {
        Ok(value) => value,
        Err(code) => fail(code, 0),
    }
}

fn set_window_long(handle: usize, index: i32, size: usize, value: usize) -> usize {
    let previous = try_with(|gui| {
        gui.window_mut(handle)?
            .set_long(index, size, value)
            .ok_or(error::INVALID_INDEX)
    });
    match previous {
        Ok(previous) => {
            thread_environment::set_last_error(error::SUCCESS);
            previous
        }
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_window_long_ptr_w(handle: usize, index: i32) -> usize {
    window_long(handle, index, 8)
}

extern "efiapi" fn set_window_long_ptr_w(handle: usize, index: i32, value: usize) -> usize {
    set_window_long(handle, index, 8, value)
}

extern "efiapi" fn get_window_long_w(handle: usize, index: i32) -> i32 {
    window_long(handle, index, 4) as i32
}

extern "efiapi" fn set_window_long_w(handle: usize, index: i32, value: i32) -> i32 {
    set_window_long(handle, index, 4, value as u32 as usize) as i32
}

/// What `DefWindowProcW` does: `WM_CLOSE` destroys the window, `WM_SYSCOMMAND`'s `SC_CLOSE`
/// sends `WM_CLOSE`, `WM_PAINT` validates (and shows) what waits to be painted,
/// `WM_ERASEBKGND` paints the class's background, `WM_NCCREATE` agrees, the window's text is
/// kept and read, and a child passes system keys to its parent; the rest is 0.
pub extern "efiapi" fn def_window_proc_w(
    handle: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match number {
        // SAFETY: `WM_SETTEXT`'s `lParam` is 0 or a NUL-terminated string.
        message::SET_TEXT => unsafe { window_text::set_default(handle, lparam) },
        // SAFETY: `WM_GETTEXT`'s contract: `lParam` holds `wParam` writable units.
        message::GET_TEXT => unsafe { window_text::get_default(handle, wparam, lparam) },
        message::GET_TEXT_LENGTH => window_text::length_default(handle),
        message::SYS_KEY_DOWN | message::SYS_KEY_UP | message::SYS_CHAR => {
            let parent = with(|gui| gui.windows.get(handle).map_or(0, |window| window.parent));
            match parent {
                Some(parent) if parent != 0 => send(parent, number, wparam, lparam),
                _ => 0,
            }
        }
        message::NC_CREATE => 1,
        message::CLOSE => {
            destroy_window(handle);
            0
        }
        message::SYS_COMMAND if wparam & 0xfff0 == message::SC_CLOSE => {
            send(handle, message::CLOSE, 0, 0);
            0
        }
        message::PAINT => {
            paint::validate(handle);
            0
        }
        message::ERASE_BACKGROUND => usize::from(paint::erase_background(handle, wparam)),
        _ => 0,
    }
}

unsafe fn write_message(out: *mut Msg, message: &Message) {
    // SAFETY: the caller's duty: `out` is a writable `MSG`.
    unsafe {
        out.write(Msg {
            window: message.window,
            message: message.message,
            wparam: message.wparam,
            lparam: message.lparam,
            time: message.time,
            x: message.point.0,
            y: message.point.1,
            private: 0,
        })
    };
}

/// The message a program's `MSG` holds, for the calling thread.
///
/// # Safety
/// `message` is a readable `MSG`.
pub unsafe fn read_message(message: *const Msg) -> Message {
    // SAFETY: the caller's duty: `message` is a readable `MSG`.
    let message = unsafe { &*message };
    Message {
        window: message.window,
        message: message.message,
        wparam: message.wparam,
        lparam: message.lparam,
        time: message.time,
        point: (message.x, message.y),
        thread: thread_environment::thread_id(),
    }
}

fn filter(handle: usize, first: u32, last: u32) -> Result<Filter, u32> {
    let known = handle == 0
        || handle == crate::gui::queue::THREAD_MESSAGES
        || try_with(|gui| gui.window(handle).map(|_| ())).is_ok();
    match known {
        true => Ok(Filter {
            window: handle,
            first,
            last,
            thread: 0,
        }),
        false => Err(error::INVALID_WINDOW_HANDLE),
    }
}

unsafe extern "efiapi" fn get_message_w(
    out: *mut Msg,
    handle: usize,
    first: u32,
    last: u32,
) -> Bool {
    let filter = match filter(handle, first, last) {
        Ok(filter) => filter,
        Err(code) => return fail(code, -1),
    };
    let Some(message) = gui::next_message(filter, true, true) else {
        return fail(error::NOT_ENOUGH_MEMORY, -1);
    };
    // SAFETY: the caller passes a writable `MSG`.
    unsafe { write_message(out, &message) };
    Bool::from(message.message != message::QUIT)
}

unsafe extern "efiapi" fn peek_message_w(
    out: *mut Msg,
    handle: usize,
    first: u32,
    last: u32,
    remove: u32,
) -> Bool {
    let filter = match filter(handle, first, last) {
        Ok(filter) => filter,
        Err(code) => return fail(code, FALSE),
    };
    match gui::next_message(filter, remove & PM_REMOVE != 0, false) {
        Some(message) => {
            // SAFETY: the caller passes a writable `MSG`.
            unsafe { write_message(out, &message) };
            TRUE
        }
        None => FALSE,
    }
}

/// `TranslateMessage` for `key`: a key press with a character posts `WM_CHAR` (`WM_SYSCHAR`
/// with Alt) for it; whether `key` was a key message.
pub fn translate(key: &Message) -> bool {
    let character_message = match key.message {
        message::KEY_DOWN => message::CHAR,
        message::SYS_KEY_DOWN => message::SYS_CHAR,
        message::KEY_UP | message::SYS_KEY_UP => return true,
        _ => return false,
    };
    let posted = with(|gui| {
        let character = gui.take_character(key)?;
        gui.queue.post(Message {
            message: character_message,
            wparam: character as usize,
            ..*key
        });
        Some(())
    })
    .flatten();
    if posted.is_some() {
        gui::wake();
    }
    true
}

unsafe extern "efiapi" fn translate_message(message: *const Msg) -> Bool {
    // SAFETY: the caller passes a readable `MSG`.
    Bool::from(translate(&unsafe { read_message(message) }))
}

/// `DispatchMessageW`'s work: a `WM_TIMER` with a registered timer procedure goes to it, any
/// other message to its window's procedure.
pub fn dispatch(message: &Message) -> usize {
    if message.message == message::TIMER
        && message.lparam != 0
        && super::timer::is_registered(message.window, message.wparam, message.lparam)
    {
        let arguments = [
            message.window,
            message::TIMER as usize,
            message.wparam,
            now_ms() as usize,
        ];
        // SAFETY: the address is the timer procedure the program registered with `SetTimer`
        // for this timer, which Windows calls with these four arguments.
        return unsafe { call_program(message.lparam, arguments) };
    }
    send(
        message.window,
        message.message,
        message.wparam,
        message.lparam,
    )
}

unsafe extern "efiapi" fn dispatch_message_w(message: *const Msg) -> usize {
    // SAFETY: the caller passes a readable `MSG`.
    dispatch(&unsafe { read_message(message) })
}

extern "efiapi" fn post_message_w(
    handle: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> Bool {
    let posted = try_with(|gui| {
        let thread = match handle {
            0 => thread_environment::thread_id(),
            handle => gui.window(handle)?.thread,
        };
        let message = Message {
            window: handle,
            message: number,
            wparam,
            lparam,
            time: now_ms(),
            point: (0, 0),
            thread,
        };
        match gui.queue.post(message) {
            true => Ok(()),
            false => Err(error::NOT_ENOUGH_QUOTA),
        }
    });
    match posted {
        Ok(()) => {
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn post_quit_message(code: i32) {
    let thread = thread_environment::thread_id();
    with(|gui| gui.queue.quit(thread, code as isize as usize));
    gui::wake();
}

extern "efiapi" fn send_message_w(
    handle: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match try_with(|gui| gui.window(handle).map(|_| ())) {
        Ok(()) => send(handle, number, wparam, lparam),
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_system_metrics(index: i32) -> i32 {
    let work = device().map(|device| device.work_area).unwrap_or_default();
    match index {
        0 | 78 => work.right,
        1 | 79 => work.bottom,
        4 => CAPTION,
        5..=8 | 32 | 33 => FRAME,
        11..=14 => ICON_SIZE,
        16 | 61 => work.width(),
        17 => work.height() - CAPTION,
        62 => work.height(),
        19 | 80 | 81 => 1,
        28 | 34 => MINIMUM_OUTSIDE.0,
        29 | 35 => MINIMUM_OUTSIDE.1,
        36 | 37 => DOUBLE_CLICK_SQUARE,
        43 => MOUSE_BUTTONS,
        49 | 50 => SMALL_ICON_SIZE,
        _ => 0,
    }
}

extern "efiapi" fn load_cursor_w(instance: usize, name: usize) -> usize {
    objects::cursor_handle(instance == 0, name)
}

extern "efiapi" fn load_icon_w(instance: usize, name: usize) -> usize {
    objects::icon_handle(instance == 0, name)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"RegisterClassExW" => register_class_ex_w,
    b"RegisterClassW" => register_class_w,
    b"UnregisterClassW" => unregister_class_w,
    b"CreateWindowExW" => create_window_ex_w [four_stack_arguments],
    b"DestroyWindow" => destroy_window,
    b"IsWindow" => is_window,
    b"ShowWindow" => show_window,
    b"UpdateWindow" => update_window,
    b"GetClientRect" => get_client_rect,
    b"AdjustWindowRect" => adjust_window_rect,
    b"AdjustWindowRectEx" => adjust_window_rect_ex,
    b"GetWindowLongPtrW" => get_window_long_ptr_w,
    b"SetWindowLongPtrW" => set_window_long_ptr_w,
    b"GetWindowLongW" => get_window_long_w,
    b"SetWindowLongW" => set_window_long_w,
    b"DefWindowProcW" => def_window_proc_w,
    b"GetMessageW" => get_message_w,
    b"PeekMessageW" => peek_message_w,
    b"TranslateMessage" => translate_message,
    b"DispatchMessageW" => dispatch_message_w,
    b"PostMessageW" => post_message_w,
    b"PostQuitMessage" => post_quit_message,
    b"SendMessageW" => send_message_w,
    b"GetSystemMetrics" => get_system_metrics,
    b"LoadCursorW" => load_cursor_w,
    b"LoadIconW" => load_icon_w,
];
