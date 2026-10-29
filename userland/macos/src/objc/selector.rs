//! Selectors: one canonical `SEL` per name, the address of the first copy of the name the
//! runtime saw. The table is open addressing over the names' FNV-1a hashes, grown at three
//! quarters full; its slots hold the canonical names' addresses.

use super::abi::Sel;

/// The FNV-1a hash of `name`.
pub fn hash(name: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in name {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The bytes of the NUL-terminated name at `name`, without the NUL.
///
/// # Safety
/// `name` must be a NUL-terminated string that outlives the result.
pub unsafe fn name_bytes<'a>(name: *const u8) -> &'a [u8] {
    // SAFETY: the caller's promise.
    unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes()
}

/// The selector table. Its slots are memory the caller hands it, so it has no allocator of
/// its own.
#[derive(Debug)]
pub struct SelectorTable {
    slots: *mut usize,
    capacity: usize,
    count: usize,
}

// SAFETY: the table is only reached through the runtime's lock.
unsafe impl Send for SelectorTable {}

pub const FIRST_CAPACITY: usize = 1024;

impl SelectorTable {
    pub const fn new() -> Self {
        Self {
            slots: core::ptr::null_mut(),
            capacity: 0,
            count: 0,
        }
    }

    fn slot(&self, index: usize) -> usize {
        // SAFETY: `slots` holds `capacity` words and `index` is below it.
        unsafe { self.slots.wrapping_add(index).read() }
    }

    fn position(&self, name: &[u8]) -> Option<(usize, bool)> {
        if self.capacity == 0 {
            return None;
        }
        let mut index = hash(name) as usize & (self.capacity - 1);
        loop {
            match self.slot(index) {
                0 => return Some((index, false)),
                found => {
                    // SAFETY: every filled slot holds a registered, permanent name.
                    if unsafe { name_bytes(found as *const u8) } == name {
                        return Some((index, true));
                    }
                }
            }
            index = (index + 1) & (self.capacity - 1);
        }
    }

    /// The canonical selector for `name`, if it is registered.
    pub fn find(&self, name: &[u8]) -> Option<Sel> {
        match self.position(name)? {
            (index, true) => Some(self.slot(index) as Sel),
            _ => None,
        }
    }

    /// The canonical selector for the name at `name`, which becomes canonical if it is new.
    /// `allocate(capacity)` gives zeroed, permanent room for `capacity` slots when the table
    /// grows; `None` if it fails.
    ///
    /// # Safety
    /// `name` must be a NUL-terminated string that lives as long as the program.
    pub unsafe fn register(
        &mut self,
        name: *const u8,
        allocate: &mut dyn FnMut(usize) -> *mut usize,
    ) -> Option<Sel> {
        // SAFETY: the caller's promise.
        let bytes = unsafe { name_bytes(name) };
        if let Some(found) = self.find(bytes) {
            return Some(found);
        }
        if (self.count + 1) * 4 > self.capacity * 3 {
            self.grow(allocate)?;
        }
        let (index, _) = self.position(bytes)?;
        // SAFETY: `index` is a free slot below `capacity`.
        unsafe { self.slots.wrapping_add(index).write(name as usize) };
        self.count += 1;
        Some(name)
    }

    fn grow(&mut self, allocate: &mut dyn FnMut(usize) -> *mut usize) -> Option<()> {
        let capacity = (self.capacity * 2).max(FIRST_CAPACITY);
        let slots = allocate(capacity);
        if slots.is_null() {
            return None;
        }
        let old = core::mem::replace(
            self,
            Self {
                slots,
                capacity,
                count: 0,
            },
        );
        for index in 0..old.capacity {
            let name = old.slot(index);
            if name != 0 {
                // SAFETY: a permanent name from the old table.
                let (free, _) = self.position(unsafe { name_bytes(name as *const u8) })?;
                // SAFETY: `free` is a free slot of the new table.
                unsafe { self.slots.wrapping_add(free).write(name) };
                self.count += 1;
            }
        }
        Some(())
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocate(capacity: usize) -> *mut usize {
        vec![0usize; capacity].leak().as_mut_ptr()
    }

    #[test]
    fn each_name_has_one_selector_whichever_copy_is_registered() {
        let mut table = SelectorTable::new();
        let first = c"length".as_ptr().cast::<u8>();
        let copy = String::from("length\0").leak().as_ptr();
        // SAFETY: both names are permanent (static or leaked).
        let one = unsafe { table.register(first, &mut allocate) };
        // SAFETY: as above.
        let two = unsafe { table.register(copy, &mut allocate) };
        assert_eq!(one, Some(first));
        assert_eq!(two, Some(first));
        assert_eq!(table.find(b"length"), Some(first));
        assert_eq!(table.find(b"count"), None);
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn the_table_grows_and_keeps_every_selector() {
        let mut table = SelectorTable::new();
        let names: Vec<&'static [u8]> = (0..3000)
            .map(|index| format!("selector{index}:\0").leak().as_bytes())
            .collect();
        for name in &names {
            // SAFETY: leaked, NUL-terminated names.
            unsafe { table.register(name.as_ptr(), &mut allocate) }.unwrap();
        }
        assert_eq!(table.count(), 3000);
        for name in &names {
            assert_eq!(table.find(&name[..name.len() - 1]), Some(name.as_ptr()));
        }
    }

    #[test]
    fn fnv_1a_matches_its_reference_values() {
        assert_eq!(hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
