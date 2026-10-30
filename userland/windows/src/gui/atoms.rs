//! The names a program registers (D178): `RegisterWindowMessageW`'s messages, the string names
//! of `SetPropW`'s properties and the clipboard formats `GetClipboardFormatNameW` names, each a
//! number from `0xc000` up, found again by its name compared ignoring case, as Windows' atom
//! table gives them. Names are never taken out, as registered messages never are.

use crate::text;

/// The first atom; an atom is this plus its entry's index.
pub const FIRST_ATOM: u16 = 0xc000;
/// The most names registered.
pub const MAX_ATOMS: usize = 128;
/// The longest name, in UTF-16 units (Windows' limit).
pub const MAX_NAME: usize = 255;
const TEXT_CAPACITY: usize = 4096;

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    start: u16,
    length: u16,
}

/// The registered names, their text kept together.
#[derive(Debug)]
pub struct AtomTable {
    text: [u16; TEXT_CAPACITY],
    used: usize,
    entries: [Entry; MAX_ATOMS],
    count: usize,
}

impl Default for AtomTable {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomTable {
    pub const fn new() -> Self {
        Self {
            text: [0; TEXT_CAPACITY],
            used: 0,
            entries: [Entry {
                start: 0,
                length: 0,
            }; MAX_ATOMS],
            count: 0,
        }
    }

    fn text_of(&self, entry: &Entry) -> &[u16] {
        let start = usize::from(entry.start);
        &self.text[start..start + usize::from(entry.length)]
    }

    /// The atom of `name`, if it is registered.
    pub fn find(&self, name: &[u16]) -> Option<u16> {
        self.entries[..self.count]
            .iter()
            .position(|entry| text::compare_ordinal(self.text_of(entry), name, true).is_eq())
            .map(|index| FIRST_ATOM + index as u16)
    }

    /// The atom of `name`, registered now if it was not; `None` for an empty or too long name
    /// or a full table.
    pub fn add(&mut self, name: &[u16]) -> Option<u16> {
        if name.is_empty() || name.len() > MAX_NAME {
            return None;
        }
        if let Some(atom) = self.find(name) {
            return Some(atom);
        }
        if self.count == MAX_ATOMS || self.used + name.len() > TEXT_CAPACITY {
            return None;
        }
        self.text[self.used..self.used + name.len()].copy_from_slice(name);
        self.entries[self.count] = Entry {
            start: self.used as u16,
            length: name.len() as u16,
        };
        self.used += name.len();
        self.count += 1;
        Some(FIRST_ATOM + (self.count - 1) as u16)
    }

    /// The name of `atom`.
    pub fn name(&self, atom: u16) -> Option<&[u16]> {
        let index = usize::from(atom.checked_sub(FIRST_ATOM)?);
        self.entries[..self.count]
            .get(index)
            .map(|entry| self.text_of(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn names_get_atoms_from_0xc000_found_again_ignoring_case() {
        let mut table = Box::new(AtomTable::new());
        let taskbar = table.add(&wide("TaskbarCreated")).unwrap();
        assert_eq!(taskbar, FIRST_ATOM);
        assert_eq!(table.add(&wide("other")), Some(FIRST_ATOM + 1));
        assert_eq!(table.add(&wide("TASKBARCREATED")), Some(taskbar));
        assert_eq!(table.find(&wide("taskbarcreated")), Some(taskbar));
        assert_eq!(table.find(&wide("Taskbar")), None);
        assert_eq!(table.name(FIRST_ATOM + 1), Some(&wide("other")[..]));
        assert_eq!(table.name(FIRST_ATOM + 2), None);
        assert_eq!(table.name(5), None);
        assert_eq!(table.add(&[]), None);
        assert_eq!(table.add(&[0x41; MAX_NAME + 1]), None);
    }

    #[test]
    fn the_table_fills_by_names_and_by_text() {
        let mut table = Box::new(AtomTable::new());
        for index in 0..MAX_ATOMS {
            assert!(table.add(&wide(&format!("n{index}"))).is_some());
        }
        assert_eq!(table.add(&wide("full")), None);
        let mut long = Box::new(AtomTable::new());
        let mut added = 0;
        while long.add(&wide(&format!("{added:0>200}"))).is_some() {
            added += 1;
        }
        assert_eq!(added, TEXT_CAPACITY / 200);
    }
}
