//! Menus (D145, `docs/ux/windows-controls-v0.3.md` section 6): `CreateMenu`, `AppendMenuW`,
//! `SetMenu` and their companions over the layer's menu table, the bar painted at the top of a
//! window's content, and its drop-downs, which the pointer drives: they are painted over the
//! client area in the same buffer and committed by themselves, whichever thread reads the
//! server's records, and a chosen item is `WM_COMMAND` with its id. The table is a heap block
//! made at the first menu, so programs without menus pay nothing for it.

use super::gui::{self, Gui, try_with, with};
use super::window_device::Device;
use super::{Bool, FALSE, TRUE, error, fail, heap};
use crate::gui::menus::{self, MAX_ITEMS, Menu, MenuTable, flag};
use crate::gui::message;
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::menu::{self as layout, BAR_HEIGHT, MenuLabel};
use eggos_windows_gui::surface::{Rect, Surface};

const VK_ESCAPE: usize = 0x1b;
const NO_ID: u32 = u32::MAX;

/// An open drop-down: the window, its bar entry, the item under the pointer and where it is
/// in the window's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracking {
    pub root: usize,
    pub entry: usize,
    pub highlighted: Option<usize>,
    pub rect: Rect,
}

/// What an input did to a menu: the content rectangle to show and the item chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effect {
    pub commit: Option<Rect>,
    pub command: Option<usize>,
}

fn table(gui: &Gui) -> Option<&MenuTable> {
    // SAFETY: a published table is the heap block `table_mut` made and never frees; it is
    // reached only under the state's lock, which the caller holds through `gui`.
    (gui.menus != 0).then(|| unsafe { &*(gui.menus as *const MenuTable) })
}

fn table_mut(gui: &mut Gui) -> Option<&mut MenuTable> {
    if gui.menus == 0 {
        let block = heap::allocate(core::mem::size_of::<MenuTable>()).cast::<MenuTable>();
        if block.is_null() {
            return None;
        }
        // SAFETY: a new, 16-byte aligned heap block the size of the table.
        unsafe { block.write(menus::new_table()) };
        gui.menus = block as usize;
    }
    // SAFETY: as `table`, and the caller's `&mut Gui` makes this the only reference.
    Some(unsafe { &mut *(gui.menus as *mut MenuTable) })
}

fn labels(menu: &Menu, nested: bool) -> [MenuLabel<'_>; MAX_ITEMS] {
    let mut labels = [MenuLabel {
        label: &[],
        separator: false,
        grayed: false,
        checked: false,
    }; MAX_ITEMS];
    for (label, item) in labels.iter_mut().zip(menu.items()) {
        *label = MenuLabel {
            label: item.label(),
            separator: item.is_separator(),
            grayed: item.is_grayed() || (nested && item.popup().is_some()),
            checked: item.flags & flag::CHECKED != 0,
        };
    }
    labels
}

fn with_root_surface<R>(
    gui: &Gui,
    root: usize,
    work: impl FnOnce(&mut Surface<'_>) -> R,
) -> Option<R> {
    let window = gui.windows.get(root)?;
    if window.pixels == 0 {
        return None;
    }
    // SAFETY: the top-level window's pixels are its mapped buffer of `buffer_width *
    // buffer_height` pixels, mapped while it is in the table; the caller holds the lock.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            window.pixels as *mut u32,
            window.buffer_width as usize * window.buffer_height as usize,
        )
    };
    let mut surface = Surface::new(pixels, window.buffer_width, window.buffer_height)?;
    Some(work(&mut surface))
}

/// Paints `root`'s menu bar, if it has one, with the open entry highlighted.
pub fn paint_bar(gui: &mut Gui, root: usize) -> Option<Rect> {
    let window = gui.windows.get(root)?;
    let (menu, width) = (window.menu, window.buffer_width as i32);
    if menu == 0 || window.menu_height == 0 {
        return None;
    }
    let open = gui
        .tracking
        .filter(|tracking| tracking.root == root)
        .map(|tracking| tracking.entry);
    let bar = table(gui)?.get(menu)?;
    let entries = labels(bar, false);
    with_root_surface(gui, root, |surface| {
        layout::paint_bar(surface, width, &entries[..bar.count], open, &LIGHT);
    })?;
    Some(Rect::new(0, 0, width, BAR_HEIGHT))
}

/// Paints `root`'s open drop-down over its content, if one is open; its rectangle.
pub fn paint_open_drop_down(gui: &mut Gui, root: usize) -> Option<Rect> {
    let tracking = gui.tracking.filter(|tracking| tracking.root == root)?;
    let menu = gui.windows.get(root)?.menu;
    let bar = table(gui)?.get(menu)?;
    let popup = bar.items().get(tracking.entry)?.popup()?;
    let items = table(gui)?.get(popup)?;
    let labels = labels(items, true);
    with_root_surface(gui, root, |surface| {
        layout::paint_drop_down(
            surface,
            tracking.rect,
            &labels[..items.count],
            tracking.highlighted,
            &LIGHT,
        );
    })?;
    Some(tracking.rect)
}

fn bar_command(gui: &Gui, root: usize, entry: usize) -> Option<usize> {
    let menu = gui.windows.get(root)?.menu;
    let item = table(gui)?.get(menu)?.items().get(entry)?;
    item.popup().is_none().then_some(item.id)
}

fn open(gui: &mut Gui, root: usize, entry: usize) -> Option<Rect> {
    if let Some(other) = gui.tracking.filter(|tracking| tracking.root != root) {
        close(gui, other.root);
    }
    let window = gui.windows.get(root)?;
    let (menu, width) = (window.menu, window.buffer_width as i32);
    let bar = table(gui)?.get(menu)?;
    let entries = labels(bar, false);
    let entry_rect = layout::entry_rect(&entries[..bar.count], entry, width)?;
    let item = bar.items().get(entry)?;
    if item.is_grayed() {
        return None;
    }
    let popup = item.popup()?;
    let items = table(gui)?.get(popup)?;
    let drop_labels = labels(items, true);
    let rect = layout::drop_down_rect(&drop_labels[..items.count], entry_rect.left, width);
    gui.tracking = Some(Tracking {
        root,
        entry,
        highlighted: None,
        rect,
    });
    let bar_rect = paint_bar(gui, root).unwrap_or_default();
    let drop = paint_open_drop_down(gui, root).unwrap_or_default();
    Some(bar_rect.union(&drop))
}

/// Closes `root`'s drop-down, if one is open: the bar repainted and the client area under the
/// drop-down left for the program to repaint; the bar's rectangle to show.
pub fn close(gui: &mut Gui, root: usize) -> Option<Rect> {
    let tracking = gui.tracking.filter(|tracking| tracking.root == root)?;
    gui.tracking = None;
    let menu_height = gui.windows.get(root)?.menu_height;
    windows::invalidate_tree(
        &mut gui.windows,
        root,
        Some(tracking.rect.offset(0, -menu_height)),
        true,
    );
    let bar = paint_bar(gui, root)?;
    let window = gui.windows.get_mut(root)?;
    window.pending = window.pending.union(&bar);
    Some(bar)
}

fn drop_down_item_at(gui: &Gui, tracking: &Tracking, point: (i32, i32)) -> Option<(usize, usize)> {
    let menu = gui.windows.get(tracking.root)?.menu;
    let bar = table(gui)?.get(menu)?;
    let popup = bar.items().get(tracking.entry)?.popup()?;
    let items = table(gui)?.get(popup)?;
    let labels = labels(items, true);
    let index = layout::item_at(tracking.rect, &labels[..items.count], point.0, point.1)?;
    Some((index, items.items()[index].id))
}

fn bar_entry_at(gui: &Gui, root: usize, point: (i32, i32)) -> Option<usize> {
    let window = gui.windows.get(root)?;
    let bar = table(gui)?.get(window.menu)?;
    let entries = labels(bar, false);
    let entry = layout::entry_at(
        &entries[..bar.count],
        window.buffer_width as i32,
        point.0,
        point.1,
    )?;
    entries[entry].is_enabled().then_some(entry)
}

/// What the menu of `root` makes of an input at `point` of its content: `None` when it is the
/// program's, else the effect (the input is then the menu's alone).
pub fn input(
    gui: &mut Gui,
    root: usize,
    number: u32,
    wparam: usize,
    point: (i32, i32),
) -> Option<Effect> {
    let window = gui.windows.get(root)?;
    if window.menu == 0 || window.menu_height == 0 {
        return None;
    }
    let menu_height = window.menu_height;
    if !window.enabled || window.modal_disabled {
        return close(gui, root).map(|commit| Effect {
            commit: Some(commit),
            command: None,
        });
    }
    let Some(tracking) = gui.tracking.filter(|tracking| tracking.root == root) else {
        let pressed = match number {
            message::LEFT_BUTTON_DOWN => true,
            message::LEFT_BUTTON_UP => false,
            _ => return None,
        };
        if point.1 >= menu_height {
            return None;
        }
        let Some(entry) = bar_entry_at(gui, root, point) else {
            return Some(Effect::default());
        };
        return Some(match (bar_command(gui, root, entry), pressed) {
            (Some(_), true) => Effect::default(),
            (Some(id), false) => Effect {
                commit: None,
                command: Some(id),
            },
            (None, true) => Effect {
                commit: open(gui, root, entry),
                command: None,
            },
            (None, false) => Effect::default(),
        });
    };
    let over_item = drop_down_item_at(gui, &tracking, point);
    let over_entry = (point.1 < menu_height)
        .then(|| bar_entry_at(gui, root, point))
        .flatten();
    let mut effect = Effect::default();
    match number {
        message::MOUSE_MOVE => {
            if let Some(entry) = over_entry.filter(|entry| *entry != tracking.entry) {
                let closed = close(gui, root).unwrap_or_default();
                effect.commit = open(gui, root, entry).map(|opened| opened.union(&closed));
                return Some(effect);
            }
            let highlighted = over_item.map(|(index, _)| index);
            if highlighted != tracking.highlighted {
                gui.tracking = Some(Tracking {
                    highlighted,
                    ..tracking
                });
                effect.commit = paint_open_drop_down(gui, root);
            }
        }
        message::LEFT_BUTTON_DOWN => {
            if tracking.rect.contains(point.0, point.1) {
                return Some(effect);
            }
            let closed = close(gui, root);
            effect.commit = match over_entry {
                Some(entry) if entry != tracking.entry => {
                    open(gui, root, entry).map(|opened| opened.union(&closed.unwrap_or_default()))
                }
                _ => closed,
            };
        }
        message::LEFT_BUTTON_UP => {
            if let Some((_, id)) = over_item {
                effect.commit = close(gui, root);
                effect.command = Some(id);
            }
        }
        message::KEY_DOWN if wparam == VK_ESCAPE => effect.commit = close(gui, root),
        _ => {}
    }
    Some(effect)
}

/// Shows what [`input`] changed and gives the chosen item's `WM_COMMAND` to deliver.
pub fn finish_input(
    device: &Device,
    root: usize,
    thread: u32,
    effect: Effect,
) -> Option<gui::Delivery> {
    let shown = with(|gui| {
        let window = gui.windows.get(root)?;
        Some((window.server, window.committed))
    })
    .flatten();
    if let (Some(rect), Some((server, true))) = (effect.commit, shown) {
        let _ = device.commit(server, Some(rect));
    }
    gui::wake();
    let id = effect.command?;
    Some(gui::Delivery {
        window: root,
        thread,
        message: message::COMMAND,
        wparam: id & 0xffff,
        lparam: 0,
    })
}

/// Forgets `root`'s open drop-down and destroys its menu, as destroying the window does.
pub fn forget_window(gui: &mut Gui, root: usize, menu: usize) {
    if gui.tracking.is_some_and(|tracking| tracking.root == root) {
        gui.tracking = None;
    }
    if menu != 0 {
        destroy(gui, menu);
    }
}

fn destroy(gui: &mut Gui, menu: usize) -> bool {
    let Some(removed) = table_mut(gui).and_then(|table| table.remove(menu)) else {
        return false;
    };
    for item in removed.items() {
        if let Some(popup) = item.popup() {
            destroy(gui, popup);
        }
    }
    true
}

fn create() -> usize {
    match try_with(|gui| {
        table_mut(gui)
            .and_then(|table| table.insert(Menu::new()))
            .ok_or(error::NOT_ENOUGH_MEMORY)
    }) {
        Ok(menu) => menu,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn create_menu() -> usize {
    create()
}

extern "efiapi" fn create_popup_menu() -> usize {
    create()
}

unsafe extern "efiapi" fn append_menu_w(
    menu: usize,
    flags: u32,
    id: usize,
    label: *const u16,
) -> Bool {
    let has_text = flags & (flag::SEPARATOR | flag::OWNER_DRAW | BITMAP) == 0 && !label.is_null();
    let label = match has_text {
        // SAFETY: an `MF_STRING` item's label is the caller's NUL-terminated string.
        true => unsafe { super::file::wide_string(label) },
        false => &[],
    };
    let appended = try_with(|gui| {
        let entry = table_mut(gui)
            .and_then(|table| table.get_mut(menu))
            .ok_or(error::INVALID_MENU_HANDLE)?;
        match entry.append(flags, id, label) {
            true => Ok(()),
            false => Err(error::NOT_ENOUGH_MEMORY),
        }
    });
    match appended {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

const BITMAP: u32 = 0x4;

/// `SetMenu`'s and `CreateWindowExW`'s work: gives top-level window `root` the menu `menu` (or
/// none): the client area changes under the bar and the whole window is repainted; the new
/// client size when it changed.
pub fn attach(gui: &mut Gui, root: usize, menu: usize) -> Result<Option<(u32, u32)>, u32> {
    if menu != 0 && table(gui).and_then(|table| table.get(menu)).is_none() {
        return Err(error::INVALID_MENU_HANDLE);
    }
    let window = gui.window_mut(root)?;
    if window.is_child() {
        return Err(error::CHILD_WINDOW_MENU);
    }
    let before = (window.width, window.height);
    window.menu = menu;
    window.set_menu_height(if menu == 0 { 0 } else { BAR_HEIGHT });
    let after = (window.width, window.height);
    window.pending = Rect::new(
        0,
        0,
        window.buffer_width as i32,
        window.buffer_height as i32,
    );
    if gui.tracking.is_some_and(|tracking| tracking.root == root) {
        gui.tracking = None;
    }
    windows::invalidate_tree(&mut gui.windows, root, None, true);
    paint_bar(gui, root);
    Ok((before != after).then_some(after))
}

extern "efiapi" fn set_menu(window: usize, menu: usize) -> Bool {
    match try_with(|gui| attach(gui, window, menu)) {
        Ok(resized) => {
            if let Some((width, height)) = resized {
                gui::send(
                    window,
                    message::SIZE,
                    message::SIZE_RESTORED,
                    message::point_parameter(width as i32, height as i32),
                );
            }
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn get_menu(window: usize) -> usize {
    match try_with(|gui| gui.window(window).map(|window| window.menu)) {
        Ok(menu) => menu,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn draw_menu_bar(window: usize) -> Bool {
    let shown = try_with(|gui| {
        let rect = paint_bar(gui, window);
        let target = gui.window(window)?;
        Ok(rect
            .filter(|_| target.committed)
            .zip(gui.device)
            .map(|(rect, device)| (device, target.server, rect)))
    });
    match shown {
        Ok(Some((device, server, rect))) => {
            let _ = device.commit(server, Some(rect));
            TRUE
        }
        Ok(None) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn destroy_menu(menu: usize) -> Bool {
    match with(|gui| destroy(gui, menu)) {
        Some(true) => TRUE,
        _ => fail(error::INVALID_MENU_HANDLE, FALSE),
    }
}

fn item_of<R>(menu: usize, position: i32, work: impl FnOnce(&menus::MenuItem) -> R) -> Option<R> {
    with(|gui| {
        let menu = table(gui)?.get(menu)?;
        let item = menu.items().get(usize::try_from(position).ok()?)?;
        Some(work(item))
    })
    .flatten()
}

extern "efiapi" fn get_sub_menu(menu: usize, position: i32) -> usize {
    item_of(menu, position, |item| item.popup())
        .flatten()
        .unwrap_or(0)
}

extern "efiapi" fn get_menu_item_count(menu: usize) -> i32 {
    match with(|gui| {
        table(gui)
            .and_then(|table| table.get(menu))
            .map(|menu| menu.count)
    }) {
        Some(Some(count)) => count as i32,
        _ => fail(error::INVALID_MENU_HANDLE, -1),
    }
}

extern "efiapi" fn get_menu_item_id(menu: usize, position: i32) -> u32 {
    item_of(menu, position, |item| match item.popup() {
        Some(_) => NO_ID,
        None => item.id as u32,
    })
    .unwrap_or(NO_ID)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateMenu" => create_menu,
    b"CreatePopupMenu" => create_popup_menu,
    b"AppendMenuW" => append_menu_w,
    b"SetMenu" => set_menu,
    b"GetMenu" => get_menu,
    b"DrawMenuBar" => draw_menu_bar,
    b"DestroyMenu" => destroy_menu,
    b"GetSubMenu" => get_sub_menu,
    b"GetMenuItemCount" => get_menu_item_count,
    b"GetMenuItemID" => get_menu_item_id,
];
