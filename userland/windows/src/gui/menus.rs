//! The program's menus (`CreateMenu`, `CreatePopupMenu`, `AppendMenuW`, D145): each a list of
//! items with a label, flags, a command id, a drop-down menu and the program's own value, by
//! handle; items are inserted, changed and removed as `InsertMenuItemW`, `SetMenuItemInfoW` and
//! `RemoveMenu` do (D178).

use super::slots::Slots;

/// The most menus a program has at once (window menus included).
pub const MAX_MENUS: usize = 32;
/// The most items in one menu.
pub const MAX_ITEMS: usize = 24;
/// The longest label kept, in UTF-16 units.
pub const MAX_LABEL: usize = 40;
const HANDLE_BASE: usize = 0x140_0000;

/// `AppendMenuW`'s flags this layer reads.
pub mod flag {
    pub const GRAYED: u32 = 0x1;
    pub const DISABLED: u32 = 0x2;
    pub const CHECKED: u32 = 0x8;
    pub const POPUP: u32 = 0x10;
    pub const OWNER_DRAW: u32 = 0x100;
    pub const SEPARATOR: u32 = 0x800;
    pub const DEFAULT: u32 = 0x1000;
}

/// One item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem {
    pub label: [u16; MAX_LABEL],
    pub length: usize,
    pub flags: u32,
    pub id: usize,
    pub submenu: usize,
    pub data: usize,
}

impl MenuItem {
    /// An item of `flags` with command id `id`, the drop-down `submenu` it opens (with
    /// [`flag::POPUP`]) and `label`, cut at [`MAX_LABEL`].
    pub fn new(flags: u32, id: usize, submenu: usize, label: &[u16]) -> MenuItem {
        let mut item = MenuItem {
            flags,
            id,
            submenu,
            ..EMPTY_ITEM
        };
        item.set_label(label);
        item
    }

    pub fn label(&self) -> &[u16] {
        &self.label[..self.length]
    }

    /// Gives it `label`, cut at [`MAX_LABEL`].
    pub fn set_label(&mut self, label: &[u16]) {
        let length = label.len().min(MAX_LABEL);
        self.label[..length].copy_from_slice(&label[..length]);
        self.length = length;
    }

    pub fn is_separator(&self) -> bool {
        self.flags & flag::SEPARATOR != 0
    }

    pub fn is_grayed(&self) -> bool {
        self.flags & (flag::GRAYED | flag::DISABLED) != 0
    }

    /// The drop-down menu it opens, for an `MF_POPUP` item.
    pub fn popup(&self) -> Option<usize> {
        (self.flags & flag::POPUP != 0).then_some(self.submenu)
    }
}

/// One menu.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    pub items: [MenuItem; MAX_ITEMS],
    pub count: usize,
    /// What `SetMenuInfo` gave it: its `MNS_` style and the program's own value.
    pub style: u32,
    pub data: usize,
}

const EMPTY_ITEM: MenuItem = MenuItem {
    label: [0; MAX_LABEL],
    length: 0,
    flags: 0,
    id: 0,
    submenu: 0,
    data: 0,
};

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

impl Menu {
    pub const fn new() -> Menu {
        Menu {
            items: [EMPTY_ITEM; MAX_ITEMS],
            count: 0,
            style: 0,
            data: 0,
        }
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items[..self.count]
    }

    /// Item `index`, to change.
    pub fn item_mut(&mut self, index: usize) -> Option<&mut MenuItem> {
        self.items[..self.count].get_mut(index)
    }

    /// Clears `clear` and sets `set` in item `index`'s flags (D174, `CheckMenuItem` and
    /// `EnableMenuItem`); the flags it had, `None` when there is no such item.
    pub fn change_flags(&mut self, index: usize, clear: u32, set: u32) -> Option<u32> {
        let item = self.items[..self.count].get_mut(index)?;
        let previous = item.flags;
        item.flags = (previous & !clear) | set;
        Some(previous)
    }

    /// `AppendMenuW`: an `MF_POPUP` item's `id` is the drop-down it opens; whether there was
    /// room.
    pub fn append(&mut self, flags: u32, id: usize, label: &[u16]) -> bool {
        let submenu = match flags & flag::POPUP {
            0 => 0,
            _ => id,
        };
        self.insert(self.count, MenuItem::new(flags, id, submenu, label))
    }

    /// Puts `item` before item `index` (at the end when `index` is past it); whether there was
    /// room.
    pub fn insert(&mut self, index: usize, item: MenuItem) -> bool {
        if self.count == MAX_ITEMS {
            return false;
        }
        let index = index.min(self.count);
        self.items.copy_within(index..self.count, index + 1);
        self.items[index] = item;
        self.count += 1;
        true
    }

    /// Takes item `index` out (`RemoveMenu`, `DeleteMenu`).
    pub fn remove(&mut self, index: usize) -> Option<MenuItem> {
        let item = *self.items().get(index)?;
        self.items.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.items[self.count] = EMPTY_ITEM;
        Some(item)
    }

    /// Makes item `index` the default one (`SetMenuDefaultItem`), none with `None`; whether
    /// there is such an item.
    pub fn set_default(&mut self, index: Option<usize>) -> bool {
        if index.is_some_and(|index| index >= self.count) {
            return false;
        }
        for (at, item) in self.items[..self.count].iter_mut().enumerate() {
            item.flags &= !flag::DEFAULT;
            if Some(at) == index {
                item.flags |= flag::DEFAULT;
            }
        }
        true
    }
}

/// The program's menus by handle.
pub type MenuTable = Slots<Menu, MAX_MENUS>;

pub const fn new_table() -> MenuTable {
    Slots::new(HANDLE_BASE)
}

/// The deepest drop-down a command id is searched in.
pub const DEEPEST_SEARCH: usize = 4;

/// The menu and position of the item `item` names in `menu` (D174): with `by_position`, the
/// item at that position of `menu`; else the first item with that command id, searched in
/// `menu` and then its drop-downs, depth first, at most [`DEEPEST_SEARCH`] deep.
pub fn locate(
    table: &MenuTable,
    menu: usize,
    item: usize,
    by_position: bool,
) -> Option<(usize, usize)> {
    let entries = table.get(menu)?.items();
    if by_position {
        return (item < entries.len()).then_some((menu, item));
    }
    locate_command(table, menu, item, DEEPEST_SEARCH)
}

fn locate_command(
    table: &MenuTable,
    menu: usize,
    id: usize,
    depth: usize,
) -> Option<(usize, usize)> {
    let entries = table.get(menu)?.items();
    if let Some(index) = entries
        .iter()
        .position(|entry| entry.popup().is_none() && !entry.is_separator() && entry.id == id)
    {
        return Some((menu, index));
    }
    if depth == 0 {
        return None;
    }
    entries
        .iter()
        .filter_map(MenuItem::popup)
        .find_map(|popup| locate_command(table, popup, id, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_keep_their_labels_flags_and_ids_and_a_menu_fills_up() {
        let mut table = new_table();
        let menu = table.insert(Menu::new()).unwrap();
        let label: Vec<u16> = "&Open...".encode_utf16().collect();
        let entry = table.get_mut(menu).unwrap();
        assert!(entry.append(0, 101, &label));
        assert!(entry.append(flag::SEPARATOR, 0, &[]));
        assert!(entry.append(flag::POPUP | flag::GRAYED, 0x140_1234, &[]));
        let items = table.get(menu).unwrap().items();
        assert_eq!(items[0].label(), &label[..]);
        assert!(items[1].is_separator() && !items[0].is_separator());
        assert_eq!(items[2].popup(), Some(0x140_1234));
        assert!(items[2].is_grayed());
        let entry = table.get_mut(menu).unwrap();
        while entry.append(0, 1, &[0x41; 60]) {}
        assert_eq!(entry.count, MAX_ITEMS);
        assert_eq!(entry.items[MAX_ITEMS - 1].length, MAX_LABEL);
    }

    #[test]
    fn items_are_inserted_removed_and_made_the_default() {
        let mut menu = Menu::new();
        assert!(menu.append(0, 10, &[0x41]));
        assert!(menu.append(0, 30, &[0x43]));
        assert!(menu.insert(1, MenuItem::new(0, 20, 0, &[0x42])));
        assert!(menu.insert(99, MenuItem::new(flag::POPUP, 40, 0x140_0001, &[0x44])));
        let ids: Vec<usize> = menu.items().iter().map(|item| item.id).collect();
        assert_eq!(ids, [10, 20, 30, 40]);
        assert_eq!(menu.items()[3].popup(), Some(0x140_0001));
        assert_eq!(menu.remove(0).map(|item| item.id), Some(10));
        assert_eq!(menu.remove(5), None);
        assert_eq!(menu.items()[0].label(), &[0x42]);
        assert!(menu.set_default(Some(1)));
        assert!(menu.set_default(Some(2)));
        assert_eq!(menu.items()[1].flags & flag::DEFAULT, 0);
        assert_eq!(menu.items()[2].flags & flag::DEFAULT, flag::DEFAULT);
        assert!(!menu.set_default(Some(3)));
        assert!(menu.set_default(None));
        assert!(
            menu.items()
                .iter()
                .all(|item| item.flags & flag::DEFAULT == 0)
        );
        menu.item_mut(0).unwrap().set_label(&[0x5a; 50]);
        assert_eq!(menu.items()[0].length, MAX_LABEL);
        while menu.insert(0, MenuItem::new(0, 1, 0, &[])) {}
        assert_eq!(menu.count, MAX_ITEMS);
    }

    #[test]
    fn items_are_found_by_position_or_by_command_in_drop_downs_and_their_flags_change() {
        let mut table = new_table();
        let bar = table.insert(Menu::new()).unwrap();
        let game = table.insert(Menu::new()).unwrap();
        let presets = table.insert(Menu::new()).unwrap();
        table.get_mut(presets).unwrap().append(0, 300, &[0x41]);
        let entry = table.get_mut(game).unwrap();
        entry.append(0, 100, &[0x4e]);
        entry.append(flag::SEPARATOR, 0, &[]);
        entry.append(flag::POPUP, presets, &[0x50]);
        table
            .get_mut(bar)
            .unwrap()
            .append(flag::POPUP, game, &[0x47]);
        assert_eq!(locate(&table, bar, 100, false), Some((game, 0)));
        assert_eq!(locate(&table, bar, 300, false), Some((presets, 0)));
        assert_eq!(locate(&table, bar, 0, true), Some((bar, 0)));
        assert_eq!(locate(&table, bar, 1, true), None);
        assert_eq!(locate(&table, bar, 999, false), None);
        let entry = table.get_mut(game).unwrap();
        assert_eq!(entry.change_flags(0, 0, flag::CHECKED), Some(0));
        assert_eq!(
            entry.change_flags(0, flag::CHECKED, flag::GRAYED),
            Some(flag::CHECKED)
        );
        assert!(entry.items()[0].is_grayed());
        assert_eq!(entry.change_flags(7, 0, 0), None);
    }
}
