//! Where windows are and what they can learn of the desktop (D174): `GetWindowRect`,
//! `ClientToScreen` and `ScreenToClient` from the window server's `GET_PLACEMENT`,
//! `SetWindowPos` and `MoveWindow` resizing and moving top-level windows as well as child
//! windows, the rectangle helpers, `SystemParametersInfo`'s work area, the keyboard state,
//! and `SetForegroundWindow`, `MessageBeep`, `WinHelp` and `GetDialogBaseUnits` as far as EggOS
//! has what they stand for.

use super::gui::{self, try_with, with};
use super::user32::{CAPTION, FRAME, Win32Rect, show_window};
use super::{Bool, FALSE, TRUE, error, error_for, fail};
use crate::gui::input::KEY_STATES;
use crate::gui::message::{self, point_parameter};
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::surface::Rect;
use eggos_windows_gui::text::{CELL_HEIGHT, CELL_WIDTH};

const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_SHOWWINDOW: u32 = 0x40;
const SWP_HIDEWINDOW: u32 = 0x80;
const SW_HIDE: i32 = 0;
const SW_SHOW_NO_ACTIVATE: i32 = 8;
const SPI_GETWORKAREA: u32 = 0x30;
const HELD_BIT: i16 = i16::MIN;
const TOGGLED_BIT: i16 = 1;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

fn rect_of(rect: &Win32Rect) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

fn write_rect(out: *mut Win32Rect, rect: Rect) {
    // SAFETY: every caller passes the program's writable `RECT`.
    unsafe {
        out.write(Win32Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    };
}

/// The top-left of `root`'s content (its menu bar and client area) on the screen.
pub(super) fn content_origin(root: usize) -> Result<(i32, i32), u32> {
    let device = gui::device()?;
    let server = try_with(|gui| gui.window(root).map(|window| window.server))?;
    let outside = device.placement(server).map_err(error_for)?;
    Ok((
        outside.left.saturating_add(FRAME),
        outside.top.saturating_add(CAPTION + FRAME),
    ))
}

/// The top-left of `window`'s client area on the screen.
pub(super) fn client_origin(window: usize) -> Result<(i32, i32), u32> {
    let (root, origin) = try_with(|gui| {
        let placement =
            windows::client_placement(&gui.windows, window).ok_or(error::INVALID_WINDOW_HANDLE)?;
        Ok((placement.root, placement.origin))
    })?;
    let (x, y) = content_origin(root)?;
    Ok((x.saturating_add(origin.0), y.saturating_add(origin.1)))
}

/// `GetWindowRect`'s rectangle: a top-level window's outside on the screen, a child window's
/// own rectangle on the screen.
pub(super) fn screen_rect(window: usize) -> Result<Rect, u32> {
    let (is_child, server, size) = try_with(|gui| {
        let root = windows::root_of(&gui.windows, window).ok_or(error::INVALID_WINDOW_HANDLE)?;
        let entry = gui.window(window)?;
        let server = gui.window(root)?.server;
        Ok((
            entry.is_child(),
            server,
            (entry.width as i32, entry.height as i32),
        ))
    })?;
    if is_child {
        let (x, y) = client_origin(window)?;
        return Ok(Rect::sized(x, y, size.0, size.1));
    }
    let device = gui::device()?;
    device.placement(server).map_err(error_for)
}

unsafe extern "efiapi" fn get_window_rect(window: usize, rect: *mut Win32Rect) -> Bool {
    match screen_rect(window) {
        Ok(found) => {
            write_rect(rect, found);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn client_to_screen(window: usize, point: *mut Point) -> Bool {
    match client_origin(window) {
        Ok((x, y)) => {
            // SAFETY: the caller passes a readable and writable `POINT`.
            let point = unsafe { &mut *point };
            point.x = point.x.saturating_add(x);
            point.y = point.y.saturating_add(y);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn screen_to_client(window: usize, point: *mut Point) -> Bool {
    match client_origin(window) {
        Ok((x, y)) => {
            // SAFETY: the caller passes a readable and writable `POINT`.
            let point = unsafe { &mut *point };
            point.x = point.x.saturating_sub(x);
            point.y = point.y.saturating_sub(y);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

fn place_child(
    window: usize,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> Result<(), u32> {
    let resized = try_with(|gui| {
        let entry = gui.window_mut(window)?;
        let old = entry.bounds;
        let (x, y) = position.unwrap_or((old.left, old.top));
        let (width, height) = size.unwrap_or((old.width(), old.height()));
        let bounds = Rect::sized(x, y, width.max(0), height.max(0));
        entry.bounds = bounds;
        entry.width = bounds.width() as u32;
        entry.height = bounds.height() as u32;
        let parent = entry.parent;
        let changed = old.width() != bounds.width() || old.height() != bounds.height();
        windows::invalidate_tree(&mut gui.windows, parent, Some(old.union(&bounds)), true);
        Ok(changed.then_some(point_parameter(bounds.width(), bounds.height())))
    })?;
    if let Some(size) = resized {
        gui::send(window, message::SIZE, message::SIZE_RESTORED, size);
    }
    gui::wake();
    Ok(())
}

fn place_top_level(
    window: usize,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> Result<(), u32> {
    let device = gui::device()?;
    let server = try_with(|gui| gui.window(window).map(|entry| entry.server))?;
    if let Some((width, height)) = size {
        let content_width = width.saturating_sub(2 * FRAME).max(1) as u32;
        let content_height = height.saturating_sub(CAPTION + 2 * FRAME).max(1) as u32;
        if let Some((size_kind, client)) =
            gui::resize(&device, window, content_width, content_height)
        {
            gui::send(window, message::SIZE, size_kind, client);
        }
    }
    if let Some((x, y)) = position {
        let work = device.work_area;
        device
            .set_position(
                server,
                x.saturating_sub(work.left),
                y.saturating_sub(work.top),
            )
            .map_err(error_for)?;
    }
    gui::wake();
    Ok(())
}

/// Moves `window` to `position` and gives it `size` (each when given): a child window's
/// rectangle in its parent's client area, a top-level window's outside rectangle on the screen,
/// whose content the window server resizes and places.
pub(super) fn place(
    window: usize,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> Result<(), u32> {
    let is_child = try_with(|gui| gui.window(window).map(|entry| entry.is_child()))?;
    match is_child {
        true => place_child(window, position, size),
        false => place_top_level(window, position, size),
    }
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn set_window_pos(
    window: usize,
    _insert_after: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: u32,
) -> Bool {
    let position = (flags & SWP_NOMOVE == 0).then_some((x, y));
    let size = (flags & SWP_NOSIZE == 0).then_some((width, height));
    if let Err(code) = place(window, position, size) {
        return fail(code, FALSE);
    }
    if flags & SWP_SHOWWINDOW != 0 {
        show_window(window, SW_SHOW_NO_ACTIVATE);
    }
    if flags & SWP_HIDEWINDOW != 0 {
        show_window(window, SW_HIDE);
    }
    TRUE
}

unsafe extern "efiapi" fn set_rect(
    rect: *mut Win32Rect,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Bool {
    if rect.is_null() {
        return FALSE;
    }
    write_rect(rect, Rect::new(left, top, right, bottom));
    TRUE
}

unsafe extern "efiapi" fn set_rect_empty(rect: *mut Win32Rect) -> Bool {
    if rect.is_null() {
        return FALSE;
    }
    write_rect(rect, Rect::default());
    TRUE
}

unsafe extern "efiapi" fn copy_rect(out: *mut Win32Rect, from: *const Win32Rect) -> Bool {
    if out.is_null() || from.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes a readable `RECT`.
    write_rect(out, rect_of(unsafe { &*from }));
    TRUE
}

unsafe extern "efiapi" fn offset_rect(rect: *mut Win32Rect, dx: i32, dy: i32) -> Bool {
    if rect.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes a readable and writable `RECT`.
    let moved = rect_of(unsafe { &*rect }).offset(dx, dy);
    write_rect(rect, moved);
    TRUE
}

unsafe extern "efiapi" fn inflate_rect(rect: *mut Win32Rect, dx: i32, dy: i32) -> Bool {
    if rect.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes a readable and writable `RECT`.
    let old = rect_of(unsafe { &*rect });
    write_rect(
        rect,
        Rect::new(
            old.left.saturating_sub(dx),
            old.top.saturating_sub(dy),
            old.right.saturating_add(dx),
            old.bottom.saturating_add(dy),
        ),
    );
    TRUE
}

fn emptied(rect: Rect) -> Rect {
    match rect.is_empty() {
        true => Rect::default(),
        false => rect,
    }
}

unsafe extern "efiapi" fn intersect_rect(
    out: *mut Win32Rect,
    first: *const Win32Rect,
    second: *const Win32Rect,
) -> Bool {
    if out.is_null() || first.is_null() || second.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes two readable `RECT`s.
    let (first, second) = unsafe { (rect_of(&*first), rect_of(&*second)) };
    let both = emptied(first.intersection(&second));
    write_rect(out, both);
    Bool::from(!both.is_empty())
}

unsafe extern "efiapi" fn union_rect(
    out: *mut Win32Rect,
    first: *const Win32Rect,
    second: *const Win32Rect,
) -> Bool {
    if out.is_null() || first.is_null() || second.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes two readable `RECT`s.
    let (first, second) = unsafe { (rect_of(&*first), rect_of(&*second)) };
    let either = match (first.is_empty(), second.is_empty()) {
        (true, true) => Rect::default(),
        (true, false) => second,
        (false, true) => first,
        (false, false) => first.union(&second),
    };
    write_rect(out, either);
    Bool::from(!either.is_empty())
}

unsafe extern "efiapi" fn is_rect_empty(rect: *const Win32Rect) -> Bool {
    // SAFETY: the caller passes a readable `RECT` or null.
    Bool::from(rect.is_null() || rect_of(unsafe { &*rect }).is_empty())
}

unsafe extern "efiapi" fn equal_rect(first: *const Win32Rect, second: *const Win32Rect) -> Bool {
    if first.is_null() || second.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes two readable `RECT`s.
    let (first, second) = unsafe { (rect_of(&*first), rect_of(&*second)) };
    Bool::from(first == second)
}

extern "efiapi" fn pt_in_rect(rect: *const Win32Rect, point: u64) -> Bool {
    if rect.is_null() {
        return FALSE;
    }
    let (x, y) = (point as u32 as i32, (point >> 32) as u32 as i32);
    // SAFETY: the caller passes a readable `RECT`.
    Bool::from(rect_of(unsafe { &*rect }).contains(x, y))
}

unsafe extern "efiapi" fn system_parameters_info(
    action: u32,
    _parameter: u32,
    out: *mut Win32Rect,
    _update: u32,
) -> Bool {
    if action != SPI_GETWORKAREA || out.is_null() {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    match gui::device() {
        Ok(device) => {
            write_rect(out, device.work_area);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_keyboard_state(out: *mut u8) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let held = with(|gui| gui.keyboard_state()).unwrap_or([0; KEY_STATES]);
    // SAFETY: `GetKeyboardState`'s contract: a writable array of 256 bytes.
    unsafe { core::ptr::copy_nonoverlapping(held.as_ptr(), out, KEY_STATES) };
    TRUE
}

fn key_state(virtual_key: i32) -> i16 {
    let Ok(key) = usize::try_from(virtual_key) else {
        return 0;
    };
    let held = with(|gui| gui.keyboard_state()).unwrap_or([0; KEY_STATES]);
    let Some(&state) = held.get(key) else {
        return 0;
    };
    let mut answer = 0;
    if state & 0x80 != 0 {
        answer |= HELD_BIT;
    }
    if state & 0x01 != 0 {
        answer |= TOGGLED_BIT;
    }
    answer
}

extern "efiapi" fn get_key_state(virtual_key: i32) -> i16 {
    key_state(virtual_key)
}

extern "efiapi" fn get_async_key_state(virtual_key: i32) -> i16 {
    key_state(virtual_key) & HELD_BIT
}

extern "efiapi" fn set_foreground_window(window: usize) -> Bool {
    let active = with(|gui| {
        let root = windows::root_of(&gui.windows, window)?;
        Some(gui.active_root == root)
    })
    .flatten();
    match active {
        Some(active) => Bool::from(active),
        None => fail(error::INVALID_WINDOW_HANDLE, FALSE),
    }
}

extern "efiapi" fn get_foreground_window() -> usize {
    with(|gui| gui.active_root).unwrap_or(0)
}

extern "efiapi" fn message_beep(_kind: u32) -> Bool {
    TRUE
}

extern "efiapi" fn win_help(_window: usize, _file: usize, _command: u32, _data: usize) -> Bool {
    fail(error::FILE_NOT_FOUND, FALSE)
}

extern "efiapi" fn get_dialog_base_units() -> i32 {
    (CELL_HEIGHT << 16) | CELL_WIDTH
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetWindowRect" => get_window_rect,
    b"ClientToScreen" => client_to_screen,
    b"ScreenToClient" => screen_to_client,
    b"SetWindowPos" => set_window_pos,
    b"SetRect" => set_rect,
    b"SetRectEmpty" => set_rect_empty,
    b"CopyRect" => copy_rect,
    b"OffsetRect" => offset_rect,
    b"InflateRect" => inflate_rect,
    b"IntersectRect" => intersect_rect,
    b"UnionRect" => union_rect,
    b"IsRectEmpty" => is_rect_empty,
    b"EqualRect" => equal_rect,
    b"PtInRect" => pt_in_rect,
    b"SystemParametersInfoA" => system_parameters_info,
    b"SystemParametersInfoW" => system_parameters_info,
    b"GetKeyboardState" => get_keyboard_state,
    b"GetKeyState" => get_key_state,
    b"GetAsyncKeyState" => get_async_key_state,
    b"SetForegroundWindow" => set_foreground_window,
    b"GetForegroundWindow" => get_foreground_window,
    b"MessageBeep" => message_beep,
    b"WinHelpA" => win_help,
    b"WinHelpW" => win_help,
    b"GetDialogBaseUnits" => get_dialog_base_units,
];
