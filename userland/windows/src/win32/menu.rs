//! Menus (D145, `docs/ux/windows-controls-v0.3.md` section 6): `CreateMenu`, `AppendMenuW`,
//! `SetMenu` and their companions over the layer's menu table, the bar painted at the top of a
//! window's content, and its drop-downs, which the pointer drives: they are painted over the
//! client area in the same buffer and committed by themselves, whichever thread reads the
//! server's records, and a chosen item is `WM_COMMAND` with its id. The table is a heap block
//! made at the first menu, so programs without menus pay nothing for it. A context menu
//! (`TrackPopupMenuEx`, design section 12, D178) is a drop-down opened at a point of a window's
//! content, run in a modal loop until an item is chosen or it is dismissed.

use super::gui::{self, Gui, Next, try_with, with};
use super::window_device::Device;
use super::{Bool, FALSE, TRUE, error, fail, heap};
use crate::gui::menus::{self, MAX_ITEMS, Menu, MenuTable, flag};
use crate::gui::message;
use crate::gui::queue::Filter;
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::menu::{self as layout, BAR_HEIGHT, MenuLabel};
use eggos_windows_gui::surface::{Rect, Surface};

const VK_ESCAPE: usize = 0x1b;
const VK_RETURN: usize = 0x0d;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_F10: usize = 0x79;
const VK_MENU: usize = 0x12;
const NO_ID: u32 = u32::MAX;

/// An open drop-down: the window, its bar entry, the menu it shows, the item under the pointer
/// and where it is in the window's content; a context menu has no bar entry, and the right
/// button's release chooses in it too when `right_button` says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracking {
    pub root: usize,
    pub entry: usize,
    pub popup: usize,
    pub highlighted: Option<usize>,
    pub rect: Rect,
    pub context: bool,
    pub right_button: bool,
    /// The submenu open from one of the drop-down's items (D337).
    pub cascade: Option<Cascade>,
}

/// A submenu open from item `parent` of a bar drop-down (D337): the menu it shows, its
/// rectangle in the window's content and the item under the pointer or the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cascade {
    pub popup: usize,
    pub parent: usize,
    pub rect: Rect,
    pub highlighted: Option<usize>,
}

/// What an input did to a menu: the content rectangle to show and the item chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effect {
    pub commit: Option<Rect>,
    pub command: Option<usize>,
}

/// The program's menus, if it made one.
pub(super) fn table(gui: &Gui) -> Option<&MenuTable> {
    // SAFETY: a published table is the heap block `table_mut` made and never frees; it is
    // reached only under the state's lock, which the caller holds through `gui`.
    (gui.menus != 0).then(|| unsafe { &*(gui.menus as *const MenuTable) })
}

/// The program's menus, made now if it had none.
pub(super) fn table_mut(gui: &mut Gui) -> Option<&mut MenuTable> {
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
    labels_with(menu, nested, false)
}

/// A drop-down's labels: in a bar drop-down (`submenus`) an item that opens a submenu shows its
/// arrow (D337); in a context menu or a submenu it stays grayed, as no deeper level opens.
fn labels_with(menu: &Menu, nested: bool, submenus: bool) -> [MenuLabel<'_>; MAX_ITEMS] {
    let mut labels = [MenuLabel {
        label: &[],
        separator: false,
        grayed: false,
        checked: false,
        radio: false,
        submenu: false,
    }; MAX_ITEMS];
    for (label, item) in labels.iter_mut().zip(menu.items()) {
        let opens = item.popup().is_some();
        *label = MenuLabel {
            label: item.label(),
            separator: item.is_separator(),
            grayed: item.is_grayed() || (nested && opens && !submenus),
            checked: item.flags & flag::CHECKED != 0,
            radio: item.flags & flag::RADIO_CHECK != 0,
            submenu: nested && opens && submenus,
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
        .filter(|tracking| tracking.root == root && !tracking.context)
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
    let main = paint_drop_down_of(gui, root, &tracking)?;
    let Some(cascade) = tracking.cascade else {
        return Some(main);
    };
    let items = table(gui)?.get(cascade.popup)?;
    let labels = labels(items, true);
    with_root_surface(gui, root, |surface| {
        layout::paint_drop_down(surface, cascade.rect, &labels[..items.count], cascade.highlighted, &LIGHT);
    })?;
    Some(main.union(&cascade.rect))
}

fn paint_drop_down_of(gui: &mut Gui, root: usize, tracking: &Tracking) -> Option<Rect> {
    let items = table(gui)?.get(tracking.popup)?;
    let labels = labels_with(items, true, !tracking.context);
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
        popup,
        highlighted: None,
        rect,
        context: false,
        right_button: false,
        cascade: None,
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
    let covered = match tracking.cascade {
        Some(cascade) => tracking.rect.union(&cascade.rect),
        None => tracking.rect,
    };
    windows::invalidate_tree(&mut gui.windows, root, Some(covered.offset(0, -menu_height)), true);
    let bar = paint_bar(gui, root)?;
    let window = gui.windows.get_mut(root)?;
    window.pending = window.pending.union(&bar);
    Some(bar)
}

fn drop_down_item_at(gui: &Gui, tracking: &Tracking, point: (i32, i32)) -> Option<(usize, usize)> {
    let items = table(gui)?.get(tracking.popup)?;
    let labels = labels(items, true);
    let index = layout::item_at(tracking.rect, &labels[..items.count], point.0, point.1)?;
    Some((index, items.items()[index].id))
}

/// The bar drop-down's item at `point` that opens a submenu, if any (D337).
fn submenu_item_at(gui: &Gui, tracking: &Tracking, point: (i32, i32)) -> Option<usize> {
    let items = table(gui)?.get(tracking.popup)?;
    let labels = labels_with(items, true, true);
    let index = layout::item_at(tracking.rect, &labels[..items.count], point.0, point.1)?;
    labels[index].submenu.then_some(index)
}

/// The submenu's command item at `point`, if one is open there.
fn cascade_item_at(gui: &Gui, cascade: &Cascade, point: (i32, i32)) -> Option<(usize, usize)> {
    let items = table(gui)?.get(cascade.popup)?;
    let labels = labels(items, true);
    let index = layout::item_at(cascade.rect, &labels[..items.count], point.0, point.1)?;
    Some((index, items.items()[index].id))
}

/// Opens the submenu of item `parent` of the open bar drop-down, with `highlighted` in it;
/// the content to show.
fn open_cascade(gui: &mut Gui, tracking: &Tracking, parent: usize, highlighted: Option<usize>) -> Option<Rect> {
    let width = gui.windows.get(tracking.root)?.buffer_width as i32;
    let menu = table(gui)?.get(tracking.popup)?;
    let popup = menu.items().get(parent)?.popup()?;
    let parent_labels = labels_with(menu, true, true);
    let row_top = layout::item_top(tracking.rect, &parent_labels[..menu.count], parent)?;
    let items = table(gui)?.get(popup)?;
    let cascade_labels = labels(items, true);
    let rect = layout::cascade_rect(&cascade_labels[..items.count], tracking.rect, row_top, width);
    let previous = tracking.cascade.map(|cascade| cascade.rect);
    gui.tracking = Some(Tracking {
        highlighted: Some(parent),
        cascade: Some(Cascade {
            popup,
            parent,
            rect,
            highlighted,
        }),
        ..*tracking
    });
    let shown = paint_open_drop_down(gui, tracking.root)?;
    Some(previous.map_or(shown, |previous| shown.union(&previous)))
}

/// Closes the submenu, if one is open, keeping the drop-down; the content to show.
fn close_cascade(gui: &mut Gui, tracking: &Tracking) -> Option<Rect> {
    let cascade = tracking.cascade?;
    gui.tracking = Some(Tracking {
        cascade: None,
        ..*tracking
    });
    let menu_height = gui.windows.get(tracking.root)?.menu_height;
    windows::invalidate_tree(&mut gui.windows, tracking.root, Some(cascade.rect.offset(0, -menu_height)), true);
    let shown = paint_open_drop_down(gui, tracking.root)?;
    Some(shown.union(&cascade.rect))
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
    if let Some(tracking) = gui
        .tracking
        .filter(|tracking| tracking.root == root && tracking.context)
    {
        return Some(context_input(gui, &tracking, number, wparam, point));
    }
    let window = gui.windows.get(root)?;
    if window.menu == 0 || window.menu_height == 0 {
        return None;
    }
    let menu_height = window.menu_height;
    if !window.enabled() || window.modal_disabled {
        return close(gui, root).map(|commit| Effect {
            commit: Some(commit),
            command: None,
        });
    }
    let Some(tracking) = gui.tracking.filter(|tracking| tracking.root == root) else {
        let alt = wparam == VK_MENU;
        match number {
            message::SYS_KEY_DOWN if alt => {
                gui.alt_alone = Some(root);
                return None;
            }
            message::SYS_KEY_UP | message::KEY_UP if alt && gui.alt_alone == Some(root) => {
                gui.alt_alone = None;
                let entry = first_open_entry(gui, root)?;
                return Some(Effect {
                    commit: open(gui, root, entry),
                    command: None,
                });
            }
            message::KEY_DOWN | message::SYS_KEY_DOWN | message::LEFT_BUTTON_DOWN => gui.alt_alone = None,
            _ => {}
        }
        if number == message::SYS_KEY_DOWN && wparam == VK_F10 {
            let entry = first_open_entry(gui, root)?;
            return Some(Effect {
                commit: open(gui, root, entry),
                command: None,
            });
        }
        if number == message::SYS_KEY_DOWN {
            let entry = mnemonic_entry(gui, root, wparam)?;
            return Some(match bar_command(gui, root, entry) {
                Some(id) => Effect {
                    commit: None,
                    command: Some(id),
                },
                None => Effect {
                    commit: open(gui, root, entry),
                    command: None,
                },
            });
        }
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
            if let Some(cascade) = tracking.cascade.filter(|cascade| cascade.rect.contains(point.0, point.1)) {
                let highlighted = cascade_item_at(gui, &cascade, point).map(|(index, _)| index);
                if highlighted != cascade.highlighted {
                    gui.tracking = Some(Tracking {
                        cascade: Some(Cascade { highlighted, ..cascade }),
                        ..tracking
                    });
                    effect.commit = paint_open_drop_down(gui, root);
                }
                return Some(effect);
            }
            if let Some(parent) = submenu_item_at(gui, &tracking, point) {
                if tracking.cascade.map(|cascade| cascade.parent) != Some(parent) {
                    effect.commit = open_cascade(gui, &tracking, parent, None);
                }
                return Some(effect);
            }
            let highlighted = over_item.map(|(index, _)| index);
            if highlighted != tracking.highlighted || (tracking.cascade.is_some() && tracking.rect.contains(point.0, point.1)) {
                let closed = match tracking.rect.contains(point.0, point.1) {
                    true => close_cascade(gui, &tracking),
                    false => None,
                };
                let current = gui.tracking.unwrap_or(tracking);
                gui.tracking = Some(Tracking {
                    highlighted,
                    ..current
                });
                effect.commit = paint_open_drop_down(gui, root).map(|shown| shown.union(&closed.unwrap_or_default()));
            }
        }
        message::LEFT_BUTTON_DOWN => {
            let in_cascade = tracking.cascade.is_some_and(|cascade| cascade.rect.contains(point.0, point.1));
            if tracking.rect.contains(point.0, point.1) || in_cascade {
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
            let chosen = match tracking.cascade.filter(|cascade| cascade.rect.contains(point.0, point.1)) {
                Some(cascade) => cascade_item_at(gui, &cascade, point).map(|(_, id)| id),
                None if submenu_item_at(gui, &tracking, point).is_some() => None,
                None => over_item.map(|(_, id)| id),
            };
            if let Some(id) = chosen {
                effect.commit = close(gui, root);
                effect.command = Some(id);
            }
        }
        message::KEY_DOWN if wparam == VK_ESCAPE && tracking.cascade.is_none() => {
            effect.commit = close(gui, root)
        }
        message::KEY_DOWN | message::SYS_KEY_DOWN => return Some(drop_down_key(gui, &tracking, wparam)),
        _ => {}
    }
    Some(effect)
}

/// The first item that can be chosen in the submenu of item `parent` of the drop-down.
fn first_choosable(gui: &Gui, tracking: &Tracking, parent: usize) -> Option<usize> {
    let popup = table(gui)?.get(tracking.popup)?.items().get(parent)?.popup()?;
    table(gui)?
        .get(popup)?
        .items()
        .iter()
        .position(|item| !item.is_grayed() && !item.is_separator() && item.popup().is_none())
}

/// A key while a submenu is open (D337): Up and Down move in it, Left and Escape close it,
/// Enter or a mnemonic choose in it.
fn cascade_key(gui: &mut Gui, tracking: &Tracking, cascade: &Cascade, key: usize) -> Effect {
    if matches!(key, VK_LEFT | VK_ESCAPE) {
        return Effect {
            commit: close_cascade(gui, tracking),
            command: None,
        };
    }
    let Some(items) = table(gui).and_then(|table| table.get(cascade.popup)) else {
        return Effect::default();
    };
    let count = items.count.min(MAX_ITEMS);
    let mut entries = [(false, 0usize, None::<usize>); MAX_ITEMS];
    for (entry, item) in entries.iter_mut().zip(items.items()) {
        let choosable = !item.is_grayed() && !item.is_separator() && item.popup().is_none();
        *entry = (choosable, item.id, mnemonic(item.label()));
    }
    let entries = &entries[..count];
    let chosen = match key {
        VK_DOWN | VK_UP if count > 0 => {
            let mut at = cascade.highlighted.unwrap_or(if key == VK_DOWN { count - 1 } else { 0 });
            let next = (0..count).find_map(|_| {
                at = match key {
                    VK_DOWN => (at + 1) % count,
                    _ => (at + count - 1) % count,
                };
                entries[at].0.then_some(at)
            });
            gui.tracking = Some(Tracking {
                cascade: Some(Cascade {
                    highlighted: next,
                    ..*cascade
                }),
                ..*tracking
            });
            return Effect {
                commit: paint_open_drop_down(gui, tracking.root),
                command: None,
            };
        }
        VK_RETURN => cascade
            .highlighted
            .and_then(|at| entries.get(at))
            .filter(|entry| entry.0)
            .map(|entry| entry.1),
        _ => entries.iter().find(|entry| entry.0 && entry.2 == Some(key)).map(|entry| entry.1),
    };
    match chosen {
        Some(id) => Effect {
            commit: close(gui, tracking.root),
            command: Some(id),
        },
        None => Effect::default(),
    }
}

/// The key of a mnemonic (`&File`'s `F`), as a virtual-key code: an ASCII letter's capital or a
/// digit.
fn mnemonic(label: &[u16]) -> Option<usize> {
    let at = label.windows(2).position(|pair| pair[0] == u16::from(b'&') && pair[1] != u16::from(b'&'))?;
    let character = char::from_u32(u32::from(label[at + 1]))?;
    character.is_ascii_alphanumeric().then(|| character.to_ascii_uppercase() as usize)
}

/// The first enabled bar entry of `root` that opens a drop-down, which F10 opens (D338).
fn first_open_entry(gui: &Gui, root: usize) -> Option<usize> {
    let window = gui.windows.get(root)?;
    let bar = table(gui)?.get(window.menu)?;
    bar.items()
        .iter()
        .position(|item| !item.is_grayed() && item.popup().is_some())
}

/// The enabled bar entry of `root` whose mnemonic is key `key` (D335).
fn mnemonic_entry(gui: &Gui, root: usize, key: usize) -> Option<usize> {
    let window = gui.windows.get(root)?;
    let bar = table(gui)?.get(window.menu)?;
    bar.items()
        .iter()
        .position(|item| !item.is_grayed() && mnemonic(item.label()) == Some(key))
}

/// A key while a bar drop-down is open (D335): Up and Down move the highlight over the items
/// that can be chosen, Left and Right open the next bar entry, Enter chooses the highlighted
/// item, and an item's mnemonic chooses it.
fn drop_down_key(gui: &mut Gui, tracking: &Tracking, key: usize) -> Effect {
    let root = tracking.root;
    if let Some(cascade) = tracking.cascade {
        return cascade_key(gui, tracking, &cascade, key);
    }
    if matches!(key, VK_RIGHT | VK_RETURN)
        && let Some(parent) = tracking.highlighted.filter(|&at| {
            table(gui)
                .and_then(|table| table.get(tracking.popup))
                .and_then(|menu| menu.items().get(at))
                .is_some_and(|item| item.popup().is_some() && !item.is_grayed())
        })
    {
        return Effect {
            commit: open_cascade(gui, tracking, parent, first_choosable(gui, tracking, parent)),
            command: None,
        };
    }
    let Some(items) = table(gui).and_then(|table| table.get(tracking.popup)) else {
        return Effect::default();
    };
    let choosable = |item: &menus::MenuItem| !item.is_grayed() && !item.is_separator() && item.popup().is_none();
    let count = items.count;
    let mut entries = [(false, 0usize, None::<usize>); MAX_ITEMS];
    for (entry, item) in entries.iter_mut().zip(items.items()) {
        *entry = (choosable(item), item.id, mnemonic(item.label()));
    }
    let entries = &entries[..count.min(MAX_ITEMS)];
    let chosen = match key {
        VK_DOWN | VK_UP => {
            let step = |at: usize| match key {
                VK_DOWN => (at + 1) % count.max(1),
                _ => (at + count.max(1) - 1) % count.max(1),
            };
            let start = tracking.highlighted.unwrap_or(if key == VK_DOWN { count.max(1) - 1 } else { 0 });
            let mut at = start;
            let next = (0..count).find_map(|_| {
                at = step(at);
                entries.get(at).is_some_and(|entry| entry.0).then_some(at)
            });
            if next.is_some() && next != tracking.highlighted {
                gui.tracking = Some(Tracking {
                    highlighted: next,
                    ..*tracking
                });
                return Effect {
                    commit: paint_open_drop_down(gui, root),
                    command: None,
                };
            }
            None
        }
        VK_LEFT | VK_RIGHT => {
            let bar_count = gui
                .windows
                .get(root)
                .and_then(|window| table(gui)?.get(window.menu))
                .map_or(0, |bar| bar.count);
            if bar_count < 2 {
                return Effect::default();
            }
            let entry = match key {
                VK_RIGHT => (tracking.entry + 1) % bar_count,
                _ => (tracking.entry + bar_count - 1) % bar_count,
            };
            let closed = close(gui, root).unwrap_or_default();
            return Effect {
                commit: Some(open(gui, root, entry).map_or(closed, |opened| opened.union(&closed))),
                command: None,
            };
        }
        VK_RETURN => tracking
            .highlighted
            .and_then(|at| entries.get(at))
            .filter(|entry| entry.0)
            .map(|entry| entry.1),
        _ => entries
            .iter()
            .find(|entry| entry.0 && entry.2 == Some(key))
            .map(|entry| entry.1),
    };
    match chosen {
        Some(id) => Effect {
            commit: close(gui, root),
            command: Some(id),
        },
        None => Effect::default(),
    }
}

fn context_item(gui: &Gui, tracking: &Tracking, index: usize) -> Option<usize> {
    let item = table(gui)?.get(tracking.popup)?.items().get(index)?;
    (!item.is_grayed() && !item.is_separator() && item.popup().is_none()).then_some(item.id)
}

fn context_input(
    gui: &mut Gui,
    tracking: &Tracking,
    number: u32,
    wparam: usize,
    point: (i32, i32),
) -> Effect {
    let mut effect = Effect::default();
    let over_item = drop_down_item_at(gui, tracking, point);
    let chosen = match number {
        message::MOUSE_MOVE => {
            let highlighted = over_item.map(|(index, _)| index);
            if highlighted != tracking.highlighted {
                gui.tracking = Some(Tracking {
                    highlighted,
                    ..*tracking
                });
                effect.commit = paint_open_drop_down(gui, tracking.root);
            }
            None
        }
        message::LEFT_BUTTON_DOWN | message::RIGHT_BUTTON_DOWN | message::MIDDLE_BUTTON_DOWN => {
            (!tracking.rect.contains(point.0, point.1)).then_some(0)
        }
        message::LEFT_BUTTON_UP => over_item.map(|(_, id)| id),
        message::RIGHT_BUTTON_UP if tracking.right_button => over_item.map(|(_, id)| id),
        message::KEY_DOWN | message::SYS_KEY_DOWN if wparam == VK_ESCAPE => Some(0),
        message::KEY_DOWN if wparam == VK_RETURN => tracking
            .highlighted
            .and_then(|index| context_item(gui, tracking, index)),
        _ => None,
    };
    if let Some(id) = chosen {
        effect.commit = close(gui, tracking.root);
        gui.menu_choice = Some(id);
    }
    effect
}

/// The command `command` of `menu` (searched in its drop-downs) is a grayed or disabled item:
/// an accelerator for it does nothing (design section 12).
pub(super) fn command_grayed(gui: &Gui, menu: usize, command: usize) -> bool {
    let Some(table) = table(gui) else {
        return false;
    };
    menus::locate(table, menu, command, false)
        .and_then(|(owner, index)| table.get(owner)?.items().get(index).copied())
        .is_some_and(|item| item.is_grayed())
}

const TPM_CENTERALIGN: u32 = 0x4;
const TPM_RIGHTALIGN: u32 = 0x8;
const TPM_VCENTERALIGN: u32 = 0x10;
const TPM_BOTTOMALIGN: u32 = 0x20;
const TPM_RIGHTBUTTON: u32 = 0x2;
const TPM_RETURNCMD: u32 = 0x100;

fn open_context(
    gui: &mut Gui,
    root: usize,
    popup: usize,
    point: (i32, i32),
    flags: u32,
) -> Result<Option<Rect>, u32> {
    let window = gui.window(root)?;
    if !window.shown {
        return Err(error::INVALID_WINDOW_HANDLE);
    }
    let (width, height) = (window.buffer_width as i32, window.buffer_height as i32);
    let size = {
        let items = table(gui)
            .and_then(|table| table.get(popup))
            .ok_or(error::INVALID_MENU_HANDLE)?;
        let labels = labels(items, true);
        let laid = layout::drop_down_rect(&labels[..items.count], 0, width);
        (laid.width(), laid.height())
    };
    let x = match flags & (TPM_CENTERALIGN | TPM_RIGHTALIGN) {
        TPM_RIGHTALIGN => point.0 - size.0,
        TPM_CENTERALIGN => point.0 - size.0 / 2,
        _ => point.0,
    };
    let y = match flags & (TPM_VCENTERALIGN | TPM_BOTTOMALIGN) {
        TPM_BOTTOMALIGN => point.1 - size.1,
        TPM_VCENTERALIGN => point.1 - size.1 / 2,
        _ => point.1,
    };
    let left = x.min(width - size.0).max(0);
    let top = match y + size.1 > height && y - size.1 >= 0 {
        true => y - size.1,
        false => y.max(0),
    };
    if let Some(other) = gui.tracking {
        close(gui, other.root);
    }
    gui.menu_choice = None;
    gui.tracking = Some(Tracking {
        root,
        entry: 0,
        popup,
        highlighted: None,
        rect: Rect::sized(left, top, size.0, size.1),
        context: true,
        right_button: flags & TPM_RIGHTBUTTON != 0,
        cascade: None,
    });
    Ok(paint_open_drop_down(gui, root))
}

fn show(root: usize, rect: Option<Rect>) {
    let target = with(|gui| {
        let window = gui.windows.get(root)?;
        Some((gui.device?, window.server, window.committed))
    })
    .flatten();
    if let (Some(rect), Some((device, server, true))) = (rect, target) {
        let _ = device.commit(server, Some(rect));
    }
}

extern "efiapi" fn track_popup_menu_ex(
    menu: usize,
    flags: u32,
    x: i32,
    y: i32,
    window: usize,
    _parameters: usize,
) -> i32 {
    let Some(root) = with(|gui| windows::root_of(&gui.windows, window)).flatten() else {
        return fail(error::INVALID_WINDOW_HANDLE, 0);
    };
    if super::shell_menu::is_shells(root, (x, y)) {
        let choice = match super::shell_menu::track(root, menu) {
            Ok(choice) => choice,
            Err(code) => return fail(code, 0),
        };
        return finish_tracking(window, choice, flags);
    }
    let (left, top) = match super::placement::content_origin(root) {
        Ok(origin) => origin,
        Err(code) => return fail(code, 0),
    };
    match try_with(|gui| open_context(gui, root, menu, (x - left, y - top), flags)) {
        Ok(rect) => show(root, rect),
        Err(code) => return fail(code, 0),
    }
    gui::wake();
    let finished = |gui: &Gui| {
        gui.menu_choice.is_some()
            || !gui
                .tracking
                .is_some_and(|tracking| tracking.root == root && tracking.context)
    };
    loop {
        match gui::wait_for(Filter::default(), true, true, &finished) {
            Next::Done | Next::Nothing => break,
            Next::Message(next) if next.message == message::QUIT => {
                let closed = with(|gui| {
                    gui.queue.quit(next.thread, next.wparam);
                    close(gui, root)
                })
                .flatten();
                show(root, closed);
                gui::wake();
                break;
            }
            Next::Message(next) => {
                super::user32::translate(&next);
                super::user32::dispatch(&next);
            }
        }
    }
    let choice = with(|gui| gui.menu_choice.take()).flatten().unwrap_or(0);
    finish_tracking(window, choice, flags)
}

fn finish_tracking(window: usize, choice: usize, flags: u32) -> i32 {
    if flags & TPM_RETURNCMD != 0 {
        return choice as i32;
    }
    if choice != 0 {
        let _ = gui::post(window, message::COMMAND, choice & 0xffff, 0);
    }
    TRUE
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

/// Destroys `menu` and its drop-downs; whether it was one.
pub(super) fn destroy(gui: &mut Gui, menu: usize) -> bool {
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

pub(super) unsafe extern "efiapi" fn append_menu_w(
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

/// A standard menu template's item flag: the last item of its level.
const TEMPLATE_END: u16 = 0x80;
/// The most levels of drop-downs a template may nest.
const TEMPLATE_DEPTH: usize = 8;

/// `LoadMenuW`: the menu a standard `RT_MENU` template of `module` describes (D336), its
/// drop-downs made as `CreatePopupMenu` and `AppendMenuW` would make them. Extended templates
/// (`MENUEX`, version 1) are refused.
unsafe extern "efiapi" fn load_menu_w(module: usize, name: *const u16) -> usize {
    use super::resource::{self, Name, kind, name_from};
    // SAFETY: `LoadMenuW`'s contract: a resource id or a NUL-terminated name.
    let name = unsafe { name_from(name as usize) };
    let Some((address, size)) = resource::find(module, Name::Id(kind::MENU), name) else {
        return fail(error::RESOURCE_NAME_NOT_FOUND, 0);
    };
    // SAFETY: a resource's data is `size` bytes of the program's mapped image.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) };
    menu_from_template(bytes).unwrap_or_else(|| fail(error::RESOURCE_DATA_NOT_FOUND, 0))
}

/// The menu a standard template's `bytes` describe: a header of two words (version 0, offset),
/// then items, each a flags word, a command word unless it opens a drop-down, and a
/// NUL-terminated UTF-16 label; a drop-down's items follow it, and `MF_END` marks each level's
/// last.
fn menu_from_template(bytes: &[u8]) -> Option<usize> {
    let word = |at: usize| bytes.get(at..at + 2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    if word(0)? != 0 {
        return None;
    }
    let mut at = 4 + usize::from(word(2)?);
    let root = create();
    if root == 0 {
        return None;
    }
    let mut levels = [(0usize, false); TEMPLATE_DEPTH];
    levels[0] = (root, false);
    let mut depth = 0;
    loop {
        let flags = word(at)?;
        at += 2;
        let popup = u32::from(flags) & flag::POPUP != 0;
        let last = flags & TEMPLATE_END != 0;
        let id = match popup {
            true => 0,
            false => {
                let id = word(at)?;
                at += 2;
                usize::from(id)
            }
        };
        let start = at;
        while word(at)? != 0 {
            at += 2;
        }
        let mut label = [0u16; 128];
        let units = ((at - start) / 2).min(label.len() - 1);
        for (index, unit) in label.iter_mut().take(units).enumerate() {
            *unit = word(start + index * 2)?;
        }
        at += 2;
        let item_flags = u32::from(flags) & !u32::from(TEMPLATE_END);
        let parent = levels[depth].0;
        if popup {
            let child = create();
            if child == 0 || depth + 1 >= TEMPLATE_DEPTH {
                return None;
            }
            // SAFETY: `label` is NUL-terminated (`units` stops one short of its end).
            if unsafe { append_menu_w(parent, item_flags, child, label.as_ptr()) } == FALSE {
                return None;
            }
            depth += 1;
            levels[depth] = (child, last);
            continue;
        }
        // SAFETY: as above.
        if unsafe { append_menu_w(parent, item_flags, id, label.as_ptr()) } == FALSE {
            return None;
        }
        if !last {
            continue;
        }
        loop {
            if depth == 0 {
                return Some(root);
            }
            let popup_was_last = levels[depth].1;
            depth -= 1;
            if !popup_was_last {
                break;
            }
        }
    }
}

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

const MF_BYPOSITION: u32 = 0x400;
const NO_SUCH_ITEM: u32 = u32::MAX;
const STATE_FLAGS: u32 = flag::GRAYED | flag::DISABLED | flag::CHECKED | flag::POPUP;

fn change_item(menu: usize, item: u32, flags: u32, clear: u32, set: u32) -> u32 {
    with(|gui| {
        let table = table_mut(gui)?;
        let by_position = flags & MF_BYPOSITION != 0;
        let (owner, index) = menus::locate(table, menu, item as usize, by_position)?;
        table.get_mut(owner)?.change_flags(index, clear, set)
    })
    .flatten()
    .map_or(NO_SUCH_ITEM, |previous| previous & clear)
}

extern "efiapi" fn check_menu_item(menu: usize, item: u32, flags: u32) -> u32 {
    change_item(menu, item, flags, flag::CHECKED, flags & flag::CHECKED)
}

/// `CheckMenuRadioItem`: checks item `check` of the items `first` to `last` (commands, or
/// positions with `MF_BYPOSITION`) with a dot (`MFT_RADIOCHECK`) and unchecks the others; `FALSE`
/// when `check` is not one of them or not in the menu (D334).
extern "efiapi" fn check_menu_radio_item(menu: usize, first: u32, last: u32, check: u32, flags: u32) -> Bool {
    if !(first..=last).contains(&check) {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let radio = flag::CHECKED | flag::RADIO_CHECK;
    if change_item(menu, check, flags, radio, radio) == NO_SUCH_ITEM {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    for item in (first..=last).filter(|&item| item != check) {
        change_item(menu, item, flags, flag::CHECKED, 0);
    }
    TRUE
}

extern "efiapi" fn enable_menu_item(menu: usize, item: u32, flags: u32) -> i32 {
    let states = flag::GRAYED | flag::DISABLED;
    change_item(menu, item, flags, states, flags & states) as i32
}

extern "efiapi" fn get_menu_state(menu: usize, item: u32, flags: u32) -> u32 {
    with(|gui| {
        let table = table(gui)?;
        let by_position = flags & MF_BYPOSITION != 0;
        let (owner, index) = menus::locate(table, menu, item as usize, by_position)?;
        table
            .get(owner)?
            .items()
            .get(index)
            .map(|entry| entry.flags & STATE_FLAGS)
    })
    .flatten()
    .unwrap_or(NO_SUCH_ITEM)
}

unsafe extern "efiapi" fn get_menu_item_rect(
    window: usize,
    menu: usize,
    item: u32,
    rect: *mut super::user32::Win32Rect,
) -> Bool {
    if rect.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let found = with(|gui| {
        let target = gui.windows.get(window)?;
        if menu == 0 || target.menu != menu {
            return None;
        }
        let width = target.buffer_width as i32;
        let bar = table(gui)?.get(menu)?;
        let entries = labels(bar, false);
        layout::entry_rect(&entries[..bar.count], item as usize, width)
    })
    .flatten();
    let origin = found.map(|_| super::placement::content_origin(window));
    let placed = match (found, origin) {
        (Some(entry), Some(Ok((x, y)))) => Some(entry.offset(x, y)),
        _ => None,
    };
    let written = placed.unwrap_or_default();
    // SAFETY: the caller passes a writable `RECT`.
    unsafe {
        rect.write(super::user32::Win32Rect {
            left: written.left,
            top: written.top,
            right: written.right,
            bottom: written.bottom,
        })
    };
    match placed {
        Some(_) => TRUE,
        None => fail(error::INVALID_PARAMETER, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CheckMenuItem" => check_menu_item,
    b"LoadMenuW" => load_menu_w,
    b"CheckMenuRadioItem" => check_menu_radio_item,
    b"EnableMenuItem" => enable_menu_item,
    b"GetMenuState" => get_menu_state,
    b"GetMenuItemRect" => get_menu_item_rect,
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
    b"TrackPopupMenuEx" => track_popup_menu_ex,
];
