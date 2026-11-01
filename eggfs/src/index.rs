use crate::checksum::crc32c;
use alloc::vec::Vec;

pub const INDEXED_ABOVE_BLOCKS: u64 = 4;
pub const MAX_INDEXED_ENTRIES: u32 = 32768;
pub const MAX_CANDIDATES: usize = 8;
const INDEXES_KEPT: usize = 2;
const SPARE_ENTRIES: usize = 64;

pub fn name_hash(name: &[u8]) -> u32 {
    crc32c(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryKey {
    pub inode: u32,
    pub generation: u32,
}

#[derive(Debug)]
pub struct DirectoryIndex {
    key: DirectoryKey,
    slots: Vec<u64>,
    filled: usize,
}

impl DirectoryIndex {
    pub fn for_entries(key: DirectoryKey, entries: usize) -> Self {
        let capacity = ((entries + SPARE_ENTRIES) * 2).next_power_of_two();
        Self {
            key,
            slots: alloc::vec![0; capacity],
            filled: 0,
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    pub fn insert(&mut self, hash: u32, block_index: u64) -> bool {
        let Ok(stored_index) = u32::try_from(block_index + 1) else {
            return false;
        };
        let slot_value = u64::from(hash) << 32 | u64::from(stored_index);
        let mut position = hash as usize & self.mask();
        loop {
            match self.slots[position] {
                0 => break,
                existing if existing == slot_value => return true,
                _ => position = (position + 1) & self.mask(),
            }
        }
        if (self.filled + 1) * 2 > self.slots.len() {
            return false;
        }
        self.slots[position] = slot_value;
        self.filled += 1;
        true
    }

    pub fn candidates(&self, hash: u32, out: &mut [u64]) -> Option<usize> {
        let mut position = hash as usize & self.mask();
        let mut found = 0;
        loop {
            let slot = self.slots[position];
            if slot == 0 {
                return Some(found);
            }
            if (slot >> 32) as u32 == hash {
                *out.get_mut(found)? = (slot & 0xffff_ffff) - 1;
                found += 1;
            }
            position = (position + 1) & self.mask();
        }
    }
}

#[derive(Debug, Default)]
pub struct DirectoryIndexes {
    kept: Vec<DirectoryIndex>,
}

impl DirectoryIndexes {
    pub fn new() -> Self {
        Self { kept: Vec::new() }
    }

    pub fn find(&mut self, key: DirectoryKey) -> Option<&DirectoryIndex> {
        let position = self.kept.iter().position(|index| index.key == key)?;
        if position > 0 {
            self.kept[..=position].rotate_right(1);
        }
        self.kept.first()
    }

    pub fn keep(&mut self, index: DirectoryIndex) {
        self.forget(index.key);
        if self.kept.len() >= INDEXES_KEPT {
            self.kept.pop();
        }
        self.kept.insert(0, index);
    }

    pub fn note_entry(&mut self, key: DirectoryKey, name: &[u8], block_index: u64) {
        let Some(position) = self.kept.iter().position(|index| index.key == key) else {
            return;
        };
        if !self.kept[position].insert(name_hash(name), block_index) {
            self.kept.remove(position);
        }
    }

    pub fn forget(&mut self, key: DirectoryKey) {
        self.kept.retain(|index| index.key != key);
    }

    pub fn clear(&mut self) {
        self.kept.clear();
    }
}
