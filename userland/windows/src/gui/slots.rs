//! A fixed table whose entries are named by handles that carry a generation, so a handle to an
//! entry that is gone never reaches the entry that took its place.

const GENERATIONS: usize = 4095;

/// `N` entries of `T`, their handles `base + (generation + 1) * N + slot`.
#[derive(Debug)]
pub struct Slots<T, const N: usize> {
    base: usize,
    entries: [Option<T>; N],
    generations: [u16; N],
}

impl<T, const N: usize> Slots<T, N> {
    pub const fn new(base: usize) -> Self {
        Self {
            base,
            entries: [const { None }; N],
            generations: [0; N],
        }
    }

    fn handle(&self, slot: usize) -> usize {
        self.base + (usize::from(self.generations[slot]) + 1) * N + slot
    }

    fn slot(&self, handle: usize) -> Option<usize> {
        let offset = handle.checked_sub(self.base)?;
        let (generation, slot) = (offset / N, offset % N);
        let current = usize::from(*self.generations.get(slot)?) + 1;
        (generation == current && self.entries[slot].is_some()).then_some(slot)
    }

    /// Stores `value` and returns its handle; `None` when the table is full.
    pub fn insert(&mut self, value: T) -> Option<usize> {
        let slot = self.entries.iter().position(Option::is_none)?;
        self.entries[slot] = Some(value);
        Some(self.handle(slot))
    }

    pub fn get(&self, handle: usize) -> Option<&T> {
        self.slot(handle)
            .and_then(|slot| self.entries[slot].as_ref())
    }

    pub fn get_mut(&mut self, handle: usize) -> Option<&mut T> {
        let slot = self.slot(handle)?;
        self.entries[slot].as_mut()
    }

    /// Takes the entry out; its handle never names an entry again until the generations wrap.
    pub fn remove(&mut self, handle: usize) -> Option<T> {
        let slot = self.slot(handle)?;
        self.generations[slot] = (self.generations[slot] + 1) % GENERATIONS as u16;
        self.entries[slot].take()
    }

    /// Removes every entry `keep` says no to.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        for slot in 0..N {
            if self.entries[slot]
                .as_ref()
                .is_some_and(|value| !keep(value))
            {
                self.generations[slot] = (self.generations[slot] + 1) % GENERATIONS as u16;
                self.entries[slot] = None;
            }
        }
    }

    /// Every entry with its handle, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> + '_ {
        (0..N).filter_map(|slot| {
            self.entries[slot]
                .as_ref()
                .map(|value| (self.handle(slot), value))
        })
    }

    /// Every entry with its handle, in slot order, to change.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut T)> + '_ {
        let (base, generations) = (self.base, self.generations);
        self.entries
            .iter_mut()
            .enumerate()
            .filter_map(move |(slot, value)| {
                value.as_mut().map(|value| {
                    (
                        base + (usize::from(generations[slot]) + 1) * N + slot,
                        value,
                    )
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_name_their_entry_until_it_is_removed() {
        let mut slots: Slots<u32, 4> = Slots::new(0x1000);
        let first = slots.insert(10).unwrap();
        let second = slots.insert(20).unwrap();
        assert_eq!(first, 0x1000 + 4);
        assert_eq!(second, 0x1000 + 5);
        assert_eq!(slots.get(second), Some(&20));
        *slots.get_mut(first).unwrap() = 11;
        assert_eq!(slots.remove(first), Some(11));
        assert_eq!(slots.get(first), None);
        let reused = slots.insert(30).unwrap();
        assert_ne!(reused, first);
        assert_eq!(slots.get(reused), Some(&30));
        assert_eq!(slots.get(0x0fff), None);
        assert_eq!(slots.get(0x1003), None);
        assert_eq!(slots.iter().map(|(_, v)| *v).collect::<Vec<_>>(), [30, 20]);
        slots.insert(1).unwrap();
        let two = slots.insert(2).unwrap();
        assert_eq!(slots.insert(3), None);
        slots.retain(|value| *value != 2);
        assert_eq!(slots.get(two), None);
        assert_eq!(slots.iter().count(), 3);
    }

    #[test]
    fn generations_wrap_without_reaching_another_table() {
        let mut slots: Slots<u8, 2> = Slots::new(0x100);
        let mut handle = 0;
        for _ in 0..GENERATIONS + 3 {
            handle = slots.insert(1).unwrap();
            assert!(handle >= 0x100 + 2);
            assert_eq!(slots.remove(handle), Some(1));
        }
        assert!(handle < 0x100 + (GENERATIONS + 1) * 2);
    }
}
