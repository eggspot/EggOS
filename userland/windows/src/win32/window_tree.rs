//! How the program's windows relate and are found (D178): `IsChild`, `GetWindow`,
//! `GetAncestor`, `GetLastActivePopup`, `SetParent`, `EnumWindows`, `EnumChildWindows`,
//! `FindWindowW`, `WindowFromPoint`, `MapWindowPoints`, `GetWindowThreadProcessId` and
//! `GetShellWindow`. A program sees its own windows: those of other apps are the window server's
//! and are not listed, found or reached, and EggOS has no shell window of the Windows kind.

use super::gui::{self, Gui, try_with, with};
use super::window_class::class_label;
use super::{Bool, FALSE, TRUE, call_program, error, fail, placement, window_text};
use crate::gui::classes::ClassName;
use crate::gui::windows::{self, MAX_WINDOWS, Relation};
use crate::library::Export;
use crate::linux;
use crate::text;
use eggos_windows_gui::surface::Rect;

const ATOM_LIMIT: usize = 0x1_0000;
const GW_HWNDFIRST: u32 = 0;
const GW_HWNDLAST: u32 = 1;
const GW_HWNDNEXT: u32 = 2;
const GW_HWNDPREV: u32 = 3;
const GW_OWNER: u32 = 4;
const GW_CHILD: u32 = 5;
const GW_ENABLEDPOPUP: u32 = 6;
const GA_PARENT: u32 = 1;
const GA_ROOT: u32 = 2;
const GA_ROOTOWNER: u32 = 3;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

extern "efiapi" fn is_child(parent: usize, window: usize) -> Bool {
    let inside = with(|gui| parent != window && windows::is_in_tree(&gui.windows, parent, window))
        .unwrap_or(false);
    Bool::from(inside)
}

/// The top-level window `window` belongs to, then its owner's, up to one without an owner.
fn root_owner(gui: &Gui, window: usize) -> Option<usize> {
    let mut root = windows::root_of(&gui.windows, window)?;
    for _ in 0..MAX_WINDOWS {
        let owner = gui.windows.get(root)?.owner;
        match windows::root_of(&gui.windows, owner) {
            Some(next) if owner != 0 && next != root => root = next,
            _ => break,
        }
    }
    Some(root)
}

fn owned_by(gui: &Gui, window: usize, owner: usize) -> bool {
    let mut at = window;
    for _ in 0..MAX_WINDOWS {
        let Some(next) = gui
            .windows
            .get(at)
            .map(|entry| entry.owner)
            .and_then(|owner| windows::root_of(&gui.windows, owner))
        else {
            return false;
        };
        if next == owner {
            return true;
        }
        at = next;
    }
    false
}

extern "efiapi" fn get_window(window: usize, command: u32) -> usize {
    let relation = match command {
        GW_HWNDFIRST => Relation::First,
        GW_HWNDLAST => Relation::Last,
        GW_HWNDNEXT => Relation::Next,
        GW_HWNDPREV => Relation::Previous,
        GW_OWNER => Relation::Owner,
        GW_CHILD => Relation::Child,
        GW_ENABLEDPOPUP => {
            let popup = try_with(|gui| {
                gui.window(window)?;
                let (tops, count) = windows::top_levels(&gui.windows);
                let found = tops[..count].iter().copied().find(|&top| {
                    top != window
                        && owned_by(gui, top, window)
                        && gui
                            .windows
                            .get(top)
                            .is_some_and(|entry| entry.shown && entry.enabled)
                });
                Ok(found.unwrap_or(window))
            });
            return match popup {
                Ok(popup) => popup,
                Err(code) => fail(code, 0),
            };
        }
        _ => return fail(error::INVALID_PARAMETER, 0),
    };
    match try_with(|gui| {
        gui.window(window)?;
        Ok(windows::related(&gui.windows, window, relation))
    }) {
        Ok(found) => found.unwrap_or(0),
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_ancestor(window: usize, which: u32) -> usize {
    let found = try_with(|gui| {
        let entry = gui.window(window)?;
        Ok(match which {
            GA_PARENT => match entry.is_child() {
                true => entry.parent,
                false => 0,
            },
            GA_ROOT => windows::root_of(&gui.windows, window).unwrap_or(0),
            GA_ROOTOWNER => root_owner(gui, window).unwrap_or(0),
            _ => return Err(error::INVALID_PARAMETER),
        })
    });
    match found {
        Ok(ancestor) => ancestor,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_last_active_popup(window: usize) -> usize {
    let found = try_with(|gui| {
        let root = windows::root_of(&gui.windows, window).ok_or(error::INVALID_WINDOW_HANDLE)?;
        let active = gui.active_root;
        Ok(
            match active != 0 && active != root && owned_by(gui, active, root) {
                true => active,
                false => window,
            },
        )
    });
    match found {
        Ok(popup) => popup,
        Err(code) => fail(code, 0),
    }
}

fn move_subtree(gui: &mut Gui, window: usize, parent: usize) -> Result<usize, u32> {
    let old_root = windows::root_of(&gui.windows, window).ok_or(error::INVALID_WINDOW_HANDLE)?;
    let bounds = gui.window(window)?.bounds;
    let old =
        windows::reparent(&mut gui.windows, window, parent).ok_or(error::INVALID_PARAMETER)?;
    let new_root = windows::root_of(&gui.windows, parent).unwrap_or(old_root);
    if new_root != old_root
        && let Some(top) = gui.windows.get(old_root)
    {
        let (focus, capture) = (top.focus, top.capture);
        let inside =
            |handle: usize| handle != 0 && windows::is_in_tree(&gui.windows, window, handle);
        let (drop_focus, drop_capture) = (inside(focus), inside(capture));
        if let Some(top) = gui.windows.get_mut(old_root) {
            if drop_focus {
                top.focus = 0;
            }
            if drop_capture {
                top.capture = 0;
            }
        }
    }
    windows::invalidate_tree(&mut gui.windows, old, Some(bounds), true);
    windows::invalidate_tree(&mut gui.windows, window, None, true);
    Ok(old)
}

extern "efiapi" fn set_parent(window: usize, parent: usize) -> usize {
    let moved = try_with(|gui| {
        let entry = gui.window(window)?;
        if !entry.is_child() || parent == 0 {
            return Err(error::NOT_SUPPORTED);
        }
        gui.window(parent)?;
        move_subtree(gui, window, parent)
    });
    match moved {
        Ok(old) => {
            gui::wake();
            old
        }
        Err(code) => fail(code, 0),
    }
}

fn call_back(procedure: usize, window: usize, lparam: usize) -> bool {
    // SAFETY: the program passes an `EnumWindowsProc`/`EnumChildProc`, which Windows calls with
    // a window and the program's value; its `BOOL` result is the low half.
    unsafe { call_program(procedure, [window, lparam, 0, 0]) as u32 != 0 }
}

fn enumerate(procedure: usize, lparam: usize, handles: &[usize]) -> Bool {
    for &handle in handles {
        let alive = with(|gui| gui.windows.get(handle).is_some()).unwrap_or(false);
        if alive && !call_back(procedure, handle, lparam) {
            return FALSE;
        }
    }
    TRUE
}

extern "efiapi" fn enum_windows(procedure: usize, lparam: usize) -> Bool {
    if procedure == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let (handles, count) =
        with(|gui| windows::top_levels(&gui.windows)).unwrap_or(([0; MAX_WINDOWS], 0));
    enumerate(procedure, lparam, &handles[..count])
}

extern "efiapi" fn enum_child_windows(parent: usize, procedure: usize, lparam: usize) -> Bool {
    if parent == 0 {
        return enum_windows(procedure, lparam);
    }
    if procedure == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let (handles, count) =
        with(|gui| windows::tree(&gui.windows, parent)).unwrap_or(([0; MAX_WINDOWS], 0));
    match count {
        0 => fail(error::INVALID_WINDOW_HANDLE, FALSE),
        _ => enumerate(procedure, lparam, &handles[1..count]),
    }
}

unsafe extern "efiapi" fn find_window_w(class: *const u16, title: *const u16) -> usize {
    let class = match class as usize {
        0 => None,
        atom if atom < ATOM_LIMIT => Some(ClassName::Atom(atom as u16)),
        // SAFETY: the caller's duty: a NUL-terminated class name.
        _ => Some(ClassName::Name(unsafe { super::file::wide_string(class) })),
    };
    let title = match title.is_null() {
        true => None,
        // SAFETY: the caller's duty: a NUL-terminated window name.
        false => Some(unsafe { super::file::wide_string(title) }),
    };
    with(|gui| {
        let (tops, count) = windows::top_levels(&gui.windows);
        tops[..count].iter().copied().find(|&top| {
            let Some(entry) = gui.windows.get(top) else {
                return false;
            };
            let class_matches = match class {
                None => true,
                Some(ClassName::Atom(atom)) => entry.class_atom == atom,
                Some(ClassName::Name(name)) => class_label(gui, entry).matches(name),
            };
            let title_matches = title.is_none_or(|title| {
                text::compare_ordinal(window_text::of(gui, top), title, true).is_eq()
            });
            !entry.destroying && class_matches && title_matches
        })
    })
    .flatten()
    .unwrap_or(0)
}

unsafe extern "efiapi" fn get_window_thread_process_id(window: usize, process: *mut u32) -> u32 {
    match try_with(|gui| gui.window(window).map(|entry| entry.thread)) {
        Ok(thread) => {
            if !process.is_null() {
                // SAFETY: the caller passes a writable `DWORD` or null.
                unsafe { process.write(linux::process_id()) };
            }
            thread
        }
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_shell_window() -> usize {
    0
}

/// The program's window at (`x`, `y`) on the screen: the deepest shown one there, the active
/// top-level window's tree first.
pub(super) fn window_at(x: i32, y: i32) -> Option<usize> {
    let (mut tops, count) = with(|gui| windows::top_levels(&gui.windows))?;
    let active = with(|gui| gui.active_root).unwrap_or(0);
    if let Some(at) = tops[..count].iter().position(|&top| top == active) {
        tops[..=at].rotate_right(1);
    }
    tops[..count].iter().find_map(|&top| {
        let shown = with(|gui| {
            let entry = gui.windows.get(top)?;
            (entry.shown && !entry.destroying).then_some((
                entry.buffer_width as i32,
                entry.buffer_height as i32,
                entry.menu_height,
            ))
        })
        .flatten()?;
        let (width, height, menu_height) = shown;
        let (left, top_edge) = placement::content_origin(top).ok()?;
        let (px, py) = (x - left, y - top_edge);
        if !Rect::new(0, 0, width, height).contains(px, py) {
            return None;
        }
        if py < menu_height {
            return Some(top);
        }
        with(|gui| windows::child_at(&gui.windows, top, px, py - menu_height).0)
    })
}

extern "efiapi" fn window_from_point(point: u64) -> usize {
    let (x, y) = (point as u32 as i32, (point >> 32) as u32 as i32);
    window_at(x, y).unwrap_or(0)
}

fn screen_origin(window: usize) -> Result<(i32, i32), u32> {
    match window {
        0 => Ok((0, 0)),
        window => placement::client_origin(window),
    }
}

unsafe extern "efiapi" fn map_window_points(
    from: usize,
    to: usize,
    points: *mut Point,
    count: u32,
) -> i32 {
    let (from_origin, to_origin) = match (screen_origin(from), screen_origin(to)) {
        (Ok(from_origin), Ok(to_origin)) => (from_origin, to_origin),
        (Err(code), _) | (_, Err(code)) => return fail(code, 0),
    };
    let (dx, dy) = (from_origin.0 - to_origin.0, from_origin.1 - to_origin.1);
    if !points.is_null() {
        for index in 0..count as usize {
            // SAFETY: the caller passes `count` readable and writable `POINT`s.
            let point = unsafe { &mut *points.add(index) };
            point.x = point.x.saturating_add(dx);
            point.y = point.y.saturating_add(dy);
        }
    }
    super::thread_environment::set_last_error(error::SUCCESS);
    ((u32::from(dy as u16) << 16) | u32::from(dx as u16)) as i32
}

pub(super) const EXPORTS: &[Export] = exports![
    b"IsChild" => is_child,
    b"GetWindow" => get_window,
    b"GetAncestor" => get_ancestor,
    b"GetLastActivePopup" => get_last_active_popup,
    b"SetParent" => set_parent,
    b"EnumWindows" => enum_windows,
    b"EnumChildWindows" => enum_child_windows,
    b"FindWindowW" => find_window_w,
    b"GetWindowThreadProcessId" => get_window_thread_process_id,
    b"GetShellWindow" => get_shell_window,
    b"WindowFromPoint" => window_from_point,
    b"MapWindowPoints" => map_window_points,
];
