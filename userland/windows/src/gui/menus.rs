//! The program's menus (`CreateMenu`, `CreatePopupMenu`, `AppendMenuW`, D145): each a list of
//! items with a label, flags and an id or a drop-down menu, by handle.

use super::slots::Slots;

/// The most menus a program has at once.
pub const MAX_MENUS: usize = 16;
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
}

/// One item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem {
    pub label: [u16; MAX_LABEL],
    pub length: usize,
    pub flags: u32,
    pub id: usize,
}

impl MenuItem {
    pub fn label(&self) -> &[u16] {
        &self.label[..self.length]
    }

    pub fn is_separator(&self) -> bool {
        self.flags & flag::SEPARATOR != 0
    }

    pub fn is_grayed(&self) -> bool {
        self.flags & (flag::GRAYED | flag::DISABLED) != 0
    }

    /// The drop-down menu it opens, for an `MF_POPUP` item.
    pub fn popup(&self) -> Option<usize> {
        (self.flags & flag::POPUP != 0).then_some(self.id)
    }
}

/// One menu.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    pub items: [MenuItem; MAX_ITEMS],
    pub count: usize,
}

const EMPTY_ITEM: MenuItem = MenuItem {
    label: [0; MAX_LABEL],
    length: 0,
    flags: 0,
    id: 0,
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
        }
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items[..self.count]
    }

    /// Clears `clear` and sets `set` in item `index`'s flags (D174, `CheckMenuItem` and
    /// `EnableMenuItem`); the flags it had, `None` when there is no such item.
    pub fn change_flags(&mut self, index: usize, clear: u32, set: u32) -> Option<u32> {
        let item = self.items[..self.count].get_mut(index)?;
        let previous = item.flags;
        item.flags = (previous & !clear) | set;
        Some(previous)
    }

    /// `AppendMenuW`; whether there was room.
    pub fn append(&mut self, flags: u32, id: usize, label: &[u16]) -> bool {
        if self.count == MAX_ITEMS {
            return false;
        }
        let length = label.len().min(MAX_LABEL);
        let mut item = MenuItem {
            flags,
            id,
            length,
            ..EMPTY_ITEM
        };
        item.label[..length].copy_from_slice(&label[..length]);
        self.items[self.count] = item;
        self.count += 1;
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
