//! A window's state beyond its place (D178): `IsZoomed` and `IsIconic`, activation as far as a
//! client of the window server may change it (`SetActiveWindow`, `BringWindowToTop`,
//! `FlashWindow`), the shapes and translucency the server does not draw (`SetWindowRgn`,
//! `SetLayeredWindowAttributes`), what waits to be painted (`RedrawWindow`, `GetUpdateRect`,
//! `ScrollWindow`) and window properties (`SetPropW`, `GetPropW`, `RemovePropW`).

use super::gui::{self, Gui, try_with, with};
use super::user32::Win32Rect;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::gui::windows::{self, MAX_WINDOWS};
use crate::library::Export;
use eggos_windows_gui::surface::Rect;

const ATOM_LIMIT: usize = 0x1_0000;
const RDW_INVALIDATE: u32 = 0x1;
const RDW_ERASE: u32 = 0x4;
const RDW_VALIDATE: u32 = 0x8;
const RDW_NOCHILDREN: u32 = 0x40;
const RDW_ALLCHILDREN: u32 = 0x80;
const RDW_UPDATENOW: u32 = 0x100;
const RDW_ERASENOW: u32 = 0x200;

fn size_kind_is(window: usize, kind: usize) -> Bool {
    let matches = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| !entry.is_child() && entry.size_kind == kind)
    });
    Bool::from(matches == Some(true))
}

extern "efiapi" fn is_zoomed(window: usize) -> Bool {
    size_kind_is(window, message::SIZE_MAXIMIZED)
}

extern "efiapi" fn is_iconic(window: usize) -> Bool {
    size_kind_is(window, message::SIZE_MINIMIZED)
}

fn active_root_of(gui: &Gui, window: usize) -> Result<bool, u32> {
    let root = windows::root_of(&gui.windows, window).ok_or(error::INVALID_WINDOW_HANDLE)?;
    Ok(gui.active_root == root)
}

extern "efiapi" fn set_active_window(window: usize) -> usize {
    match try_with(|gui| Ok((active_root_of(gui, window)?, gui.active_root))) {
        Ok((true, previous)) => previous,
        Ok((false, _)) => fail(error::ACCESS_DENIED, 0),
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn bring_window_to_top(window: usize) -> Bool {
    let raised = try_with(|gui| {
        let entry = gui.window(window)?;
        if !entry.is_child() {
            return active_root_of(gui, window);
        }
        let sequence = super::user32::next_sequence();
        if let Some(entry) = gui.windows.get_mut(window) {
            entry.creation_sequence = sequence;
        }
        windows::invalidate_tree(&mut gui.windows, window, None, true);
        Ok(true)
    });
    match raised {
        Ok(true) => {
            gui::wake();
            TRUE
        }
        Ok(false) => fail(error::ACCESS_DENIED, FALSE),
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn flash_window(window: usize, _invert: Bool) -> Bool {
    match try_with(|gui| active_root_of(gui, window)) {
        Ok(active) => Bool::from(active),
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn set_window_rgn(window: usize, region: usize, _redraw: Bool) -> i32 {
    if let Err(code) = try_with(|gui| gui.window(window).map(|_| ())) {
        return fail(code, 0);
    }
    match region {
        0 => 1,
        _ => fail(error::NOT_SUPPORTED, 0),
    }
}

extern "efiapi" fn set_layered_window_attributes(
    window: usize,
    _key: u32,
    _alpha: u8,
    _flags: u32,
) -> Bool {
    match try_with(|gui| gui.window(window).map(|_| ())) {
        Ok(()) => fail(error::NOT_SUPPORTED, FALSE),
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn get_layered_window_attributes(
    window: usize,
    _key: *mut u32,
    _alpha: *mut u8,
    _flags: *mut u32,
) -> Bool {
    match try_with(|gui| gui.window(window).map(|_| ())) {
        Ok(()) => fail(error::INVALID_PARAMETER, FALSE),
        Err(code) => fail(code, FALSE),
    }
}

fn rect_from(rect: *const Win32Rect) -> Option<Rect> {
    match rect.is_null() {
        true => None,
        // SAFETY: every caller passes the program's readable `RECT` or null.
        false => Some(super::paint::rect_of(unsafe { &*rect })),
    }
}

fn targets(gui: &Gui, window: usize) -> Result<([usize; MAX_WINDOWS], usize), u32> {
    if window != 0 {
        gui.window(window)?;
        let mut one = [0; MAX_WINDOWS];
        one[0] = window;
        return Ok((one, 1));
    }
    Ok(windows::top_levels(&gui.windows))
}

fn redraw(gui: &mut Gui, window: usize, rect: Option<Rect>, flags: u32) {
    let erase = flags & RDW_ERASE != 0;
    if flags & RDW_INVALIDATE != 0 {
        match flags & RDW_NOCHILDREN {
            0 => windows::invalidate_tree(&mut gui.windows, window, rect, erase),
            _ => {
                if let Some(entry) = gui.windows.get_mut(window) {
                    entry.invalidate(rect, erase);
                }
            }
        }
    }
    if flags & RDW_VALIDATE != 0 {
        let (members, count) = match flags & RDW_ALLCHILDREN {
            0 => {
                let mut one = [0; MAX_WINDOWS];
                one[0] = window;
                (one, 1)
            }
            _ => windows::tree(&gui.windows, window),
        };
        for &member in &members[..count] {
            if let Some(entry) = gui.windows.get_mut(member) {
                let covered = rect.is_none_or(|rect| {
                    member != window || entry.update.intersection(&rect) == entry.update
                });
                if covered {
                    entry.update = Rect::default();
                    entry.erase = false;
                }
            }
        }
    }
}

fn paint_now(window: usize) {
    let waiting = with(|gui| {
        let (members, count) = windows::tree(&gui.windows, window);
        let mut waiting = [0; MAX_WINDOWS];
        let mut found = 0;
        for &member in &members[..count] {
            if gui
                .windows
                .get(member)
                .is_some_and(|entry| entry.needs_paint())
            {
                waiting[found] = member;
                found += 1;
            }
        }
        (waiting, found)
    });
    let Some((waiting, found)) = waiting else {
        return;
    };
    for &member in &waiting[..found] {
        let still = with(|gui| {
            gui.windows
                .get(member)
                .is_some_and(|entry| entry.needs_paint())
        });
        if still == Some(true) {
            gui::send(member, message::PAINT, 0, 0);
        }
    }
}

unsafe extern "efiapi" fn redraw_window(
    window: usize,
    rect: *const Win32Rect,
    region: usize,
    flags: u32,
) -> Bool {
    let rect = match region {
        0 => rect_from(rect),
        _ => None,
    };
    let done = try_with(|gui| {
        let (handles, count) = targets(gui, window)?;
        for &handle in &handles[..count] {
            redraw(gui, handle, rect, flags);
        }
        Ok((handles, count))
    });
    let (handles, count) = match done {
        Ok(done) => done,
        Err(code) => return fail(code, FALSE),
    };
    if flags & (RDW_UPDATENOW | RDW_ERASENOW) != 0 {
        for &handle in &handles[..count] {
            paint_now(handle);
        }
    }
    gui::wake();
    TRUE
}

unsafe extern "efiapi" fn get_update_rect(
    window: usize,
    out: *mut Win32Rect,
    _erase: Bool,
) -> Bool {
    let update = try_with(|gui| {
        let entry = gui.window(window)?;
        Ok(entry.update.intersection(&entry.client()))
    });
    let update = match update {
        Ok(update) if update.is_empty() => Rect::default(),
        Ok(update) => update,
        Err(code) => return fail(code, FALSE),
    };
    if !out.is_null() {
        // SAFETY: the caller passes a writable `RECT` or null.
        unsafe {
            out.write(Win32Rect {
                left: update.left,
                top: update.top,
                right: update.right,
                bottom: update.bottom,
            })
        };
    }
    Bool::from(!update.is_empty())
}

unsafe extern "efiapi" fn scroll_window(
    window: usize,
    dx: i32,
    dy: i32,
    rect: *const Win32Rect,
    clip: *const Win32Rect,
) -> Bool {
    let (rect, clip) = (rect_from(rect), rect_from(clip));
    let scrolled = try_with(|gui| {
        let client = gui.window(window)?.client();
        if rect.is_none() {
            let (kids, count) = windows::children(&gui.windows, window);
            for &kid in &kids[..count] {
                if let Some(entry) = gui.windows.get_mut(kid) {
                    entry.bounds = entry.bounds.offset(dx, dy);
                }
            }
        }
        let area = rect
            .unwrap_or(client)
            .intersection(&clip.unwrap_or(client))
            .intersection(&client);
        if !area.is_empty() {
            windows::invalidate_tree(&mut gui.windows, window, Some(area), true);
        }
        Ok(())
    });
    match scrolled {
        Ok(()) => {
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

enum PropertyName<'a> {
    Atom(u16),
    Name(&'a [u16]),
}

unsafe fn property_name<'a>(name: *const u16) -> PropertyName<'a> {
    match (name as usize) < ATOM_LIMIT {
        true => PropertyName::Atom(name as usize as u16),
        // SAFETY: the caller's duty: a NUL-terminated name.
        false => PropertyName::Name(unsafe { super::file::wide_string(name) }),
    }
}

unsafe extern "efiapi" fn set_prop_w(window: usize, name: *const u16, value: usize) -> Bool {
    // SAFETY: `SetPropW`'s contract: an atom or a NUL-terminated name.
    let name = unsafe { property_name(name) };
    let set = try_with(|gui| {
        gui.window(window)?;
        let extras = gui.extras().ok_or(error::NOT_ENOUGH_MEMORY)?;
        let atom = match name {
            PropertyName::Atom(atom) => Some(atom),
            PropertyName::Name(units) => extras.atoms.add(units),
        }
        .ok_or(error::NOT_ENOUGH_MEMORY)?;
        match extras.properties.set(window, atom, value) {
            true => Ok(()),
            false => Err(error::NOT_ENOUGH_MEMORY),
        }
    });
    match set {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn find_property(gui: &mut Gui, window: usize, name: &PropertyName<'_>, remove: bool) -> usize {
    let Some(extras) = gui.made_extras_mut() else {
        return 0;
    };
    let atom = match name {
        PropertyName::Atom(atom) => Some(*atom),
        PropertyName::Name(units) => extras.atoms.find(units),
    };
    let Some(atom) = atom else {
        return 0;
    };
    match remove {
        true => extras.properties.remove(window, atom),
        false => extras.properties.get(window, atom),
    }
    .unwrap_or(0)
}

unsafe extern "efiapi" fn get_prop_w(window: usize, name: *const u16) -> usize {
    // SAFETY: `GetPropW`'s contract: an atom or a NUL-terminated name.
    let name = unsafe { property_name(name) };
    with(|gui| find_property(gui, window, &name, false)).unwrap_or(0)
}

unsafe extern "efiapi" fn remove_prop_w(window: usize, name: *const u16) -> usize {
    // SAFETY: `RemovePropW`'s contract: an atom or a NUL-terminated name.
    let name = unsafe { property_name(name) };
    with(|gui| find_property(gui, window, &name, true)).unwrap_or(0)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"IsZoomed" => is_zoomed,
    b"IsIconic" => is_iconic,
    b"SetActiveWindow" => set_active_window,
    b"BringWindowToTop" => bring_window_to_top,
    b"FlashWindow" => flash_window,
    b"SetWindowRgn" => set_window_rgn,
    b"SetLayeredWindowAttributes" => set_layered_window_attributes,
    b"GetLayeredWindowAttributes" => get_layered_window_attributes,
    b"RedrawWindow" => redraw_window,
    b"GetUpdateRect" => get_update_rect,
    b"ScrollWindow" => scroll_window,
    b"SetPropW" => set_prop_w,
    b"GetPropW" => get_prop_w,
    b"RemovePropW" => remove_prop_w,
];
