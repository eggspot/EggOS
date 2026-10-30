//! The shell's functions (D182): `name() compound-command` keeps the command's text under its
//! name, and a call runs that text again with the call's arguments as `$1`, `$2`, .... The
//! texts are kept in fixed room, one store for every function, closed up when a definition
//! replaces or removes another.

/// The most bytes every function's name and text take.
pub const FUNCTIONS_CAPACITY: usize = 16384;

/// The most functions the shell keeps.
pub const MAX_FUNCTIONS: usize = 64;

/// The functions are full: [`FUNCTIONS_CAPACITY`] or [`MAX_FUNCTIONS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    start: usize,
    name_length: usize,
    length: usize,
}

/// The functions defined.
#[derive(Debug)]
pub struct Functions {
    bytes: [u8; FUNCTIONS_CAPACITY],
    length: usize,
    entries: [Entry; MAX_FUNCTIONS],
    count: usize,
}

impl Default for Functions {
    fn default() -> Self {
        Self {
            bytes: [0; FUNCTIONS_CAPACITY],
            length: 0,
            entries: [Entry::default(); MAX_FUNCTIONS],
            count: 0,
        }
    }
}

impl Functions {
    fn name(&self, entry: &Entry) -> &[u8] {
        &self.bytes[entry.start..entry.start + entry.name_length]
    }

    fn find(&self, name: &[u8]) -> Option<usize> {
        self.entries[..self.count]
            .iter()
            .position(|entry| self.name(entry) == name)
    }

    /// The text of function `name`, if it is defined.
    pub fn get(&self, name: &[u8]) -> Option<&[u8]> {
        let entry = &self.entries[self.find(name)?];
        Some(&self.bytes[entry.start + entry.name_length..entry.start + entry.length])
    }

    /// Whether function `name` is defined.
    pub fn contains(&self, name: &[u8]) -> bool {
        self.find(name).is_some()
    }

    /// Defines function `name` as `text`, replacing an earlier definition; nothing changes if
    /// it does not fit.
    pub fn define(&mut self, name: &[u8], text: &[u8]) -> Result<(), Full> {
        let old = self.find(name).map(|index| self.entries[index]);
        let freed = old.map_or(0, |entry| entry.length);
        let length = name.len() + text.len();
        if self.length - freed + length > FUNCTIONS_CAPACITY
            || (old.is_none() && self.count == MAX_FUNCTIONS)
        {
            return Err(Full);
        }
        self.remove(name);
        let start = self.length;
        self.bytes[start..start + name.len()].copy_from_slice(name);
        self.bytes[start + name.len()..start + length].copy_from_slice(text);
        self.length += length;
        self.entries[self.count] = Entry {
            start,
            name_length: name.len(),
            length,
        };
        self.count += 1;
        Ok(())
    }

    /// Removes function `name` (`unset -f`).
    pub fn remove(&mut self, name: &[u8]) {
        let Some(index) = self.find(name) else {
            return;
        };
        let entry = self.entries[index];
        self.bytes
            .copy_within(entry.start + entry.length..self.length, entry.start);
        self.length -= entry.length;
        self.entries.copy_within(index + 1..self.count, index);
        self.count -= 1;
        for other in &mut self.entries[..self.count] {
            if other.start > entry.start {
                other.start -= entry.length;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_are_defined_replaced_and_removed() {
        let mut functions = Functions::default();
        functions.define(b"greet", b"{ echo hi; }").unwrap();
        functions.define(b"other", b"(ls)").unwrap();
        assert_eq!(functions.get(b"greet"), Some(&b"{ echo hi; }"[..]));
        functions.define(b"greet", b"{ echo hello there; }").unwrap();
        assert_eq!(functions.get(b"greet"), Some(&b"{ echo hello there; }"[..]));
        assert_eq!(functions.get(b"other"), Some(&b"(ls)"[..]));
        functions.remove(b"other");
        assert!(!functions.contains(b"other"));
        assert_eq!(functions.get(b"greet"), Some(&b"{ echo hello there; }"[..]));
    }

    #[test]
    fn a_definition_that_does_not_fit_leaves_the_others() {
        let mut functions = Functions::default();
        functions.define(b"kept", b"{ :; }").unwrap();
        let big = [b' '; FUNCTIONS_CAPACITY];
        assert_eq!(functions.define(b"big", &big), Err(Full));
        assert_eq!(functions.define(b"kept", &big), Err(Full));
        assert_eq!(functions.get(b"kept"), Some(&b"{ :; }"[..]));
        let mut many = Functions::default();
        for index in 0..MAX_FUNCTIONS {
            many.define(format!("f{index}").as_bytes(), b"{ :; }").unwrap();
        }
        assert_eq!(many.define(b"one_more", b"{ :; }"), Err(Full));
        many.define(b"f3", b"{ true; }").unwrap();
        assert_eq!(many.get(b"f3"), Some(&b"{ true; }"[..]));
    }
}
