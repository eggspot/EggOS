use crate::layout::{Block, zeroed_block};
use crate::{Disk, Error};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const CLEAN_BLOCKS_KEPT: usize = 256;

struct Entry {
    data: Box<Block>,
    dirty: bool,
    last_used: u64,
    committed: Option<Box<Block>>,
}

pub struct Cache {
    entries: BTreeMap<u32, Entry>,
    clock: u64,
    dirty: usize,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            clock: 0,
            dirty: 0,
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty
    }

    fn touch(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn evict_one(&mut self) {
        let oldest = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.dirty)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(&block, _)| block);
        if let Some(block) = oldest {
            self.entries.remove(&block);
        }
    }

    fn load(&mut self, disk: &mut dyn Disk, block: u32) -> Result<&mut Entry, Error> {
        let now = self.touch();
        if !self.entries.contains_key(&block) {
            if self.entries.len() - self.dirty >= CLEAN_BLOCKS_KEPT {
                self.evict_one();
            }
            let mut data = zeroed_block();
            disk.read(u64::from(block), &mut data[..])
                .map_err(|_| Error::Io)?;
            self.entries.insert(
                block,
                Entry {
                    data,
                    dirty: false,
                    last_used: now,
                    committed: None,
                },
            );
        }
        let entry = self.entries.get_mut(&block).ok_or(Error::Io)?;
        entry.last_used = now;
        Ok(entry)
    }

    pub fn get(&mut self, disk: &mut dyn Disk, block: u32) -> Result<&Block, Error> {
        Ok(&self.load(disk, block)?.data)
    }

    pub fn get_mut(
        &mut self,
        disk: &mut dyn Disk,
        block: u32,
        keep_committed: bool,
    ) -> Result<&mut Block, Error> {
        let entry = self.load(disk, block)?;
        let newly_dirty = !entry.dirty;
        if newly_dirty {
            entry.dirty = true;
            if keep_committed {
                entry.committed = Some(entry.data.clone());
            }
        }
        if newly_dirty {
            self.dirty += 1;
        }
        let entry = self.entries.get_mut(&block).ok_or(Error::Io)?;
        Ok(&mut entry.data)
    }

    pub fn insert_zeroed(&mut self, block: u32) -> &mut Block {
        let now = self.touch();
        let previous = self.entries.insert(
            block,
            Entry {
                data: zeroed_block(),
                dirty: true,
                last_used: now,
                committed: None,
            },
        );
        if !previous.is_some_and(|entry| entry.dirty) {
            self.dirty += 1;
        }
        &mut self
            .entries
            .get_mut(&block)
            .unwrap_or_else(|| unreachable!())
            .data
    }

    pub fn forget(&mut self, block: u32) {
        if let Some(entry) = self.entries.remove(&block)
            && entry.dirty
        {
            self.dirty -= 1;
        }
    }

    pub fn committed(&self, block: u32) -> Option<&Block> {
        self.entries.get(&block)?.committed.as_deref()
    }

    pub fn dirty_blocks(&self) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.dirty)
            .map(|(&block, _)| block)
            .collect()
    }

    pub fn data(&self, block: u32) -> Option<&Block> {
        self.entries.get(&block).map(|entry| &*entry.data)
    }

    pub fn mark_clean(&mut self) {
        for entry in self.entries.values_mut() {
            entry.dirty = false;
            entry.committed = None;
        }
        self.dirty = 0;
        while self.entries.len() > CLEAN_BLOCKS_KEPT {
            self.evict_one();
        }
    }
}
