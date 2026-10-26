//! The shell's variables: each kept as `NAME=value` and a NUL, as the environment it hands
//! a program holds them, so starting one copies nothing. Kept sorted by name, so `set` and
//! `export` list them in order.

use crate::syntax::is_name;

/// The most bytes the variables take, each `NAME=value` with its NUL.
pub const VARIABLES_CAPACITY: usize = 8192;

/// The most variables the shell has.
pub const MAX_VARIABLES: usize = 128;

/// The variables are full: [`VARIABLES_CAPACITY`] or [`MAX_VARIABLES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    start: u16,
    length: u16,
    name_length: u16,
    exported: bool,
    set: bool,
}

/// A variable as [`Variables::iter`] lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variable<'a> {
    /// Its name.
    pub name: &'a [u8],
    /// Its value, `None` if it was exported without one.
    pub value: Option<&'a [u8]>,
    /// Whether programs the shell starts get it in their environment.
    pub exported: bool,
}

/// The shell's variables.
#[derive(Debug, Clone)]
pub struct Variables {
    bytes: [u8; VARIABLES_CAPACITY],
    length: usize,
    entries: [Entry; MAX_VARIABLES],
    count: usize,
}

impl Default for Variables {
    fn default() -> Self {
        Self {
            bytes: [0; VARIABLES_CAPACITY],
            length: 0,
            entries: [Entry::default(); MAX_VARIABLES],
            count: 0,
        }
    }
}

impl Variables {
    fn name(&self, entry: &Entry) -> &[u8] {
        let start = usize::from(entry.start);
        &self.bytes[start..start + usize::from(entry.name_length)]
    }

    fn find(&self, name: &[u8]) -> Result<usize, usize> {
        self.entries[..self.count].binary_search_by(|entry| self.name(entry).cmp(name))
    }

    /// The value of `name`, if set.
    pub fn get(&self, name: &[u8]) -> Option<&[u8]> {
        let entry = &self.entries[self.find(name).ok()?];
        if !entry.set {
            return None;
        }
        let start = usize::from(entry.start + entry.name_length) + 1;
        Some(&self.bytes[start..usize::from(entry.start + entry.length)])
    }

    /// Sets `name` to `value`, exported if `export` or if it was already.
    pub fn set(&mut self, name: &[u8], value: &[u8], export: bool) -> Result<(), Full> {
        let (index, exported) = match self.find(name) {
            Ok(index) => (index, self.entries[index].exported || export),
            Err(index) => (index, export),
        };
        let length = name.len() + 1 + value.len();
        let old = self.find(name).ok().map(|index| self.entries[index]);
        let freed = old.map_or(0, |entry| usize::from(entry.length) + 1);
        if self.length - freed + length + 1 > VARIABLES_CAPACITY
            || (old.is_none() && self.count == MAX_VARIABLES)
        {
            return Err(Full);
        }
        if let Some(entry) = old {
            self.remove_bytes(&entry);
        } else {
            self.entries.copy_within(index..self.count, index + 1);
            self.count += 1;
        }
        let start = self.length;
        self.bytes[start..start + name.len()].copy_from_slice(name);
        self.bytes[start + name.len()] = b'=';
        self.bytes[start + name.len() + 1..start + length].copy_from_slice(value);
        self.bytes[start + length] = 0;
        self.length += length + 1;
        self.entries[index] = Entry {
            start: start as u16,
            length: length as u16,
            name_length: name.len() as u16,
            exported,
            set: true,
        };
        Ok(())
    }

    /// Sets the variable an assignment word `NAME=value` names; `false` if it is not one.
    pub fn assign(&mut self, assignment: &[u8], export: bool) -> Result<bool, Full> {
        let Some(equals) = assignment.iter().position(|&byte| byte == b'=') else {
            return Ok(false);
        };
        let name = &assignment[..equals];
        if !is_name(name) {
            return Ok(false);
        }
        self.set(name, &assignment[equals + 1..], export)?;
        Ok(true)
    }

    /// Marks `name` exported, set or not.
    pub fn export(&mut self, name: &[u8]) -> Result<(), Full> {
        match self.find(name) {
            Ok(index) => {
                self.entries[index].exported = true;
                Ok(())
            }
            Err(_) => {
                self.set(name, b"", true)?;
                let index = self.find(name).map_err(|_| Full)?;
                self.entries[index].set = false;
                Ok(())
            }
        }
    }

    /// Removes `name`.
    pub fn unset(&mut self, name: &[u8]) {
        if let Ok(index) = self.find(name) {
            let entry = self.entries[index];
            self.remove_bytes(&entry);
            self.entries.copy_within(index + 1..self.count, index);
            self.count -= 1;
        }
    }

    fn remove_bytes(&mut self, entry: &Entry) {
        let start = usize::from(entry.start);
        let removed = usize::from(entry.length) + 1;
        self.bytes.copy_within(start + removed..self.length, start);
        self.length -= removed;
        for other in &mut self.entries[..self.count] {
            if usize::from(other.start) > start {
                other.start -= removed as u16;
            }
        }
    }

    /// Every variable, in name order.
    pub fn iter(&self) -> impl Iterator<Item = Variable<'_>> {
        self.entries[..self.count].iter().map(|entry| {
            let start = usize::from(entry.start + entry.name_length) + 1;
            Variable {
                name: self.name(entry),
                value: entry
                    .set
                    .then(|| &self.bytes[start..usize::from(entry.start + entry.length)]),
                exported: entry.exported,
            }
        })
    }

    /// The environment programs get: every exported variable that is set, as `NAME=value`
    /// with its NUL.
    pub fn environment(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.entries[..self.count]
            .iter()
            .filter(|entry| entry.exported && entry.set)
            .map(|entry| {
                let start = usize::from(entry.start);
                (
                    self.name(entry),
                    &self.bytes[start..start + usize::from(entry.length) + 1],
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(variables: &Variables) -> Vec<&[u8]> {
        variables.environment().map(|(_, entry)| entry).collect()
    }

    #[test]
    fn variables_are_set_replaced_and_unset() {
        let mut variables = Variables::default();
        variables.set(b"B", b"2", false).unwrap();
        variables.set(b"A", b"1", true).unwrap();
        variables.set(b"C", b"3", false).unwrap();
        assert_eq!(variables.get(b"A"), Some(&b"1"[..]));
        variables.set(b"A", b"longer value", false).unwrap();
        assert_eq!(variables.get(b"A"), Some(&b"longer value"[..]));
        assert_eq!(variables.get(b"B"), Some(&b"2"[..]));
        assert_eq!(variables.get(b"C"), Some(&b"3"[..]));
        let names: Vec<&[u8]> = variables.iter().map(|variable| variable.name).collect();
        assert_eq!(names, [&b"A"[..], b"B", b"C"]);
        assert_eq!(environment(&variables), [&b"A=longer value\0"[..]]);
        variables.unset(b"B");
        variables.unset(b"missing");
        assert_eq!(variables.get(b"B"), None);
        assert_eq!(variables.get(b"C"), Some(&b"3"[..]));
        assert_eq!(variables.get(b"A"), Some(&b"longer value"[..]));
    }

    #[test]
    fn assignments_and_exports() {
        let mut variables = Variables::default();
        assert_eq!(variables.assign(b"PATH=/bin:/usr/bin", false), Ok(true));
        assert_eq!(variables.assign(b"1X=a", false), Ok(false));
        assert_eq!(variables.assign(b"noequals", false), Ok(false));
        assert_eq!(variables.get(b"PATH"), Some(&b"/bin:/usr/bin"[..]));
        assert!(environment(&variables).is_empty());
        variables.export(b"PATH").unwrap();
        variables.export(b"LATER").unwrap();
        assert_eq!(variables.get(b"LATER"), None);
        assert_eq!(environment(&variables), [&b"PATH=/bin:/usr/bin\0"[..]]);
        variables.assign(b"LATER=x=y", false).unwrap();
        assert_eq!(
            environment(&variables),
            [&b"LATER=x=y\0"[..], b"PATH=/bin:/usr/bin\0"]
        );
        let later = variables.iter().next().unwrap();
        assert_eq!(later.value, Some(&b"x=y"[..]));
        assert!(later.exported);
    }

    #[test]
    fn the_store_refuses_what_does_not_fit_and_keeps_what_it_has() {
        let mut variables = Variables::default();
        let big = [b'v'; VARIABLES_CAPACITY / 2];
        variables.set(b"A", &big, false).unwrap();
        assert_eq!(variables.set(b"B", &big, false), Err(Full));
        variables.set(b"A", &big[..10], false).unwrap();
        variables.set(b"B", &big, false).unwrap();
        assert_eq!(variables.get(b"A"), Some(&big[..10]));
        let mut many = Variables::default();
        for index in 0..MAX_VARIABLES {
            let name = format!("V{index}");
            many.set(name.as_bytes(), b"", false).unwrap();
        }
        assert_eq!(many.set(b"ONE_MORE", b"", false), Err(Full));
        many.set(b"V7", b"replaced", false).unwrap();
        assert_eq!(many.get(b"V7"), Some(&b"replaced"[..]));
    }
}
