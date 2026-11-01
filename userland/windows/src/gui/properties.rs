//! Window properties (`SetPropW`, `GetPropW`, `RemovePropW`, D178): values a program hangs on a
//! window by name, each name an atom (a string name is registered in the atom table first), in
//! one fixed table that forgets a window's properties when it is destroyed.

/// The most properties all the program's windows have at once.
pub const MAX_PROPERTIES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Property {
    window: usize,
    atom: u16,
    value: usize,
}

/// Every window's properties.
#[derive(Debug)]
pub struct PropertyTable {
    entries: [Option<Property>; MAX_PROPERTIES],
}

impl Default for PropertyTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PropertyTable {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PROPERTIES],
        }
    }

    fn index(&self, window: usize, atom: u16) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry.is_some_and(|entry| entry.window == window && entry.atom == atom)
        })
    }

    /// Gives `window` the property `atom` with `value`, replacing one it had; whether there was
    /// room.
    pub fn set(&mut self, window: usize, atom: u16, value: usize) -> bool {
        let Some(index) = self
            .index(window, atom)
            .or_else(|| self.entries.iter().position(Option::is_none))
        else {
            return false;
        };
        self.entries[index] = Some(Property {
            window,
            atom,
            value,
        });
        true
    }

    /// The value of `window`'s property `atom`.
    pub fn get(&self, window: usize, atom: u16) -> Option<usize> {
        let index = self.index(window, atom)?;
        self.entries[index].map(|entry| entry.value)
    }

    /// Takes `window`'s property `atom` away and gives its value.
    pub fn remove(&mut self, window: usize, atom: u16) -> Option<usize> {
        let index = self.index(window, atom)?;
        self.entries[index].take().map(|entry| entry.value)
    }

    /// Forgets every property of `window`, which is gone.
    pub fn forget(&mut self, window: usize) {
        for entry in &mut self.entries {
            if entry.is_some_and(|entry| entry.window == window) {
                *entry = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_are_set_read_replaced_removed_and_forgotten_by_window() {
        let mut table = PropertyTable::new();
        assert!(table.set(1, 0xc000, 10));
        assert!(table.set(1, 0xc001, 11));
        assert!(table.set(2, 0xc000, 20));
        assert!(table.set(1, 0xc000, 12));
        assert_eq!(table.get(1, 0xc000), Some(12));
        assert_eq!(table.get(2, 0xc000), Some(20));
        assert_eq!(table.get(3, 0xc000), None);
        assert_eq!(table.remove(1, 0xc001), Some(11));
        assert_eq!(table.remove(1, 0xc001), None);
        table.forget(1);
        assert_eq!(table.get(1, 0xc000), None);
        assert_eq!(table.get(2, 0xc000), Some(20));
        for value in 0..MAX_PROPERTIES - 1 {
            assert!(table.set(5, value as u16, value));
        }
        assert!(!table.set(6, 1, 1));
        assert!(table.set(5, 3, 99));
        assert_eq!(table.get(5, 3), Some(99));
    }
}
