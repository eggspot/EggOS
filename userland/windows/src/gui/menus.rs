//! The program's menus (`CreateMenu`, `CreatePopupMenu`, `AppendMenuW`, D145): each a list of
//! items with a label, flags and an id or a drop-down menu, by handle.

use super::slots::Slots;

/// The most menus a program has at once.
pub const MAX_MENUS: usize = 16;
/// The most items in one menu.
pub const MAX_ITEMS: usize = 24;
/// The longest label kept, in UTF-16 units.
pub const MAX_LABEL: usize = 40;
const HANDLE_BASE: usize = 0x4_0000;

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
        assert!(entry.append(flag::POPUP | flag::GRAYED, 0x4_1234, &[]));
        let items = table.get(menu).unwrap().items();
        assert_eq!(items[0].label(), &label[..]);
        assert!(items[1].is_separator() && !items[0].is_separator());
        assert_eq!(items[2].popup(), Some(0x4_1234));
        assert!(items[2].is_grayed());
        let entry = table.get_mut(menu).unwrap();
        while entry.append(0, 1, &[0x41; 60]) {}
        assert_eq!(entry.count, MAX_ITEMS);
        assert_eq!(entry.items[MAX_ITEMS - 1].length, MAX_LABEL);
    }
}
