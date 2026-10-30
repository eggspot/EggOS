//! Window classes (`RegisterClassExW`): what each window of a class starts with, found by its
//! atom or by its name compared ignoring case, as Windows compares class names.

use crate::text;

/// The most classes a program registers.
pub const MAX_CLASSES: usize = 32;
/// The longest class name, in UTF-16 units (Windows' limit).
pub const MAX_NAME: usize = 256;
/// The first class atom; a class's atom is this plus its slot.
pub const FIRST_ATOM: u16 = 0xc000;

/// What a class gives its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Class {
    pub style: u32,
    pub procedure: usize,
    pub class_extra: u32,
    pub window_extra: u32,
    pub instance: usize,
    pub icon: usize,
    pub cursor: usize,
    pub background: usize,
    /// Registered by `RegisterClassA`: its windows' procedures take ANSI text (D174).
    pub ansi: bool,
}

/// How a program names a class: by its atom (`MAKEINTATOM`) or by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassName<'a> {
    Atom(u16),
    Name(&'a [u16]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassError {
    AlreadyExists,
    NotFound,
    TableFull,
    InvalidName,
}

#[derive(Debug)]
struct Entry {
    name: [u16; MAX_NAME],
    length: usize,
    class: Class,
}

/// The registered classes.
#[derive(Debug)]
pub struct ClassTable {
    entries: [Option<Entry>; MAX_CLASSES],
}

impl Default for ClassTable {
    fn default() -> Self {
        Self::new()
    }
}

fn same_name(a: &[u16], b: &[u16]) -> bool {
    text::compare_ordinal(a, b, true).is_eq()
}

impl ClassTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_CLASSES],
        }
    }

    /// Registers `class` as `name` and returns its atom.
    pub fn register(&mut self, name: &[u16], class: Class) -> Result<u16, ClassError> {
        if name.is_empty() || name.len() > MAX_NAME {
            return Err(ClassError::InvalidName);
        }
        if self.slot_of(ClassName::Name(name)).is_some() {
            return Err(ClassError::AlreadyExists);
        }
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(ClassError::TableFull)?;
        let mut stored = [0; MAX_NAME];
        stored[..name.len()].copy_from_slice(name);
        self.entries[slot] = Some(Entry {
            name: stored,
            length: name.len(),
            class,
        });
        Ok(FIRST_ATOM + slot as u16)
    }

    fn slot_of(&self, name: ClassName<'_>) -> Option<usize> {
        match name {
            ClassName::Atom(atom) => {
                let slot = usize::from(atom.checked_sub(FIRST_ATOM)?);
                self.entries.get(slot)?.as_ref().map(|_| slot)
            }
            ClassName::Name(name) => self.entries.iter().position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|entry| same_name(&entry.name[..entry.length], name))
            }),
        }
    }

    /// The class `name` names, with its atom.
    pub fn find(&self, name: ClassName<'_>) -> Option<(u16, Class)> {
        let slot = self.slot_of(name)?;
        let entry = self.entries[slot].as_ref()?;
        Some((FIRST_ATOM + slot as u16, entry.class))
    }

    /// The name of the class with atom `atom` (`GetClassNameW`).
    pub fn name(&self, atom: u16) -> Option<&[u16]> {
        let slot = self.slot_of(ClassName::Atom(atom))?;
        let entry = self.entries[slot].as_ref()?;
        Some(&entry.name[..entry.length])
    }

    /// Removes the class `name` names.
    pub fn unregister(&mut self, name: ClassName<'_>) -> Result<Class, ClassError> {
        let slot = self.slot_of(name).ok_or(ClassError::NotFound)?;
        self.entries[slot]
            .take()
            .map(|entry| entry.class)
            .ok_or(ClassError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn classes_are_found_by_atom_and_by_name_ignoring_case() {
        let mut table = ClassTable::new();
        let class = Class {
            procedure: 0x1234,
            window_extra: 16,
            ..Class::default()
        };
        let atom = table.register(&wide("PaintWindow"), class).unwrap();
        assert_eq!(atom, FIRST_ATOM);
        assert_eq!(
            table.find(ClassName::Name(&wide("paintwindow"))),
            Some((atom, class))
        );
        assert_eq!(table.find(ClassName::Atom(atom)), Some((atom, class)));
        assert_eq!(table.name(atom), Some(&wide("PaintWindow")[..]));
        assert_eq!(table.name(atom + 1), None);
        assert_eq!(table.find(ClassName::Atom(atom + 1)), None);
        assert_eq!(table.find(ClassName::Atom(1)), None);
        assert_eq!(table.find(ClassName::Name(&wide("Paint"))), None);
        assert_eq!(
            table.register(&wide("PAINTWINDOW"), class),
            Err(ClassError::AlreadyExists)
        );
        let other = table.register(&wide("Other"), class).unwrap();
        assert_eq!(other, FIRST_ATOM + 1);
    }

    #[test]
    fn names_are_checked_and_the_table_fills_and_empties() {
        let mut table = ClassTable::new();
        assert_eq!(
            table.register(&[], Class::default()),
            Err(ClassError::InvalidName)
        );
        assert_eq!(
            table.register(&[b'a' as u16; MAX_NAME + 1], Class::default()),
            Err(ClassError::InvalidName)
        );
        for index in 0..MAX_CLASSES {
            table
                .register(&wide(&format!("c{index}")), Class::default())
                .unwrap();
        }
        assert_eq!(
            table.register(&wide("full"), Class::default()),
            Err(ClassError::TableFull)
        );
        assert!(table.unregister(ClassName::Name(&wide("C3"))).is_ok());
        assert_eq!(
            table.unregister(ClassName::Name(&wide("c3"))),
            Err(ClassError::NotFound)
        );
        assert_eq!(
            table.register(&wide("again"), Class::default()),
            Ok(FIRST_ATOM + 3)
        );
    }
}
