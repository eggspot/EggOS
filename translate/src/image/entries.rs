use alloc::vec;
use alloc::vec::Vec;

use super::runtime::entries_table;

/// The multiplier of the entries table's hash (Knuth's multiplicative hashing with the
/// golden ratio): the slot of a relative address is the top `bits` bits of `rva * HASH`.
pub const HASH_MULTIPLIER: u32 = 0x9E37_79B1;

/// The key of an empty slot (no image is 4 GiB, so no relative address is this).
pub const EMPTY_KEY: u32 = u32::MAX;

/// The fewest slot bits, so that a table always has empty slots and probing stops.
pub const MIN_BITS: u8 = 4;

/// The most slot bits; the dispatch shifts a 32-bit product right by `32 - bits`.
pub const MAX_BITS: u8 = 31;

/// The image's indirect-branch table: guest relative address to translated code offset, an
/// open-addressed hash with linear probing that the indirect dispatch reads (D191).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntriesTable {
    bits: u8,
    slots: Vec<u64>,
    count: usize,
}

impl EntriesTable {
    /// The slot bits for `count` entries: at least twice as many slots as entries.
    pub fn bits_for(count: usize) -> u8 {
        let mut bits = MIN_BITS;
        while bits < MAX_BITS && (1usize << bits) < count.saturating_mul(2) {
            bits += 1;
        }
        bits
    }

    /// An empty table of `2^bits` slots (`bits` clamped to [`MIN_BITS`]..=[`MAX_BITS`]).
    pub fn with_bits(bits: u8) -> EntriesTable {
        let bits = bits.clamp(MIN_BITS, MAX_BITS);
        EntriesTable {
            bits,
            slots: vec![u64::from(EMPTY_KEY); 1 << bits],
            count: 0,
        }
    }

    /// A table from serialized slots; `None` when their number is not `2^bits`, a key repeats
    /// or no slot is empty.
    pub fn from_slots(bits: u8, slots: Vec<u64>) -> Option<EntriesTable> {
        if !(MIN_BITS..=MAX_BITS).contains(&bits) || slots.len() != 1 << bits {
            return None;
        }
        let mut table = EntriesTable {
            bits,
            slots,
            count: 0,
        };
        let mut empty = false;
        for (index, &slot) in table.slots.iter().enumerate() {
            let key = slot as u32;
            if key == EMPTY_KEY {
                empty = true;
            } else if table.find(key) == Some(index) {
                table.count += 1;
            } else {
                return None;
            }
        }
        empty.then_some(table)
    }

    /// The slot bits.
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// The slots, as the dispatch reads them.
    pub fn slots(&self) -> &[u64] {
        &self.slots
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds no entry.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The slot the probe for `rva` starts at.
    pub fn home(&self, rva: u32) -> usize {
        (rva.wrapping_mul(HASH_MULTIPLIER) >> (32 - u32::from(self.bits))) as usize
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    fn find(&self, rva: u32) -> Option<usize> {
        let mut index = self.home(rva);
        for _ in 0..self.slots.len() {
            let key = self.slots[index] as u32;
            if key == rva {
                return Some(index);
            }
            if key == EMPTY_KEY {
                return None;
            }
            index = (index + 1) & self.mask();
        }
        None
    }

    /// Maps `rva` to the code offset `host`, replacing an earlier entry for it. Returns false
    /// when the table would have no empty slot left (or `rva` is [`EMPTY_KEY`]).
    pub fn insert(&mut self, rva: u32, host: u32) -> bool {
        if rva == EMPTY_KEY {
            return false;
        }
        let value = u64::from(rva) | (u64::from(host) << 32);
        if let Some(index) = self.find(rva) {
            self.slots[index] = value;
            return true;
        }
        if self.len() + 1 >= self.slots.len() {
            return false;
        }
        let mut index = self.home(rva);
        while self.slots[index] as u32 != EMPTY_KEY {
            index = (index + 1) & self.mask();
        }
        self.slots[index] = value;
        self.count += 1;
        true
    }

    /// The code offset of `rva`, probing as the dispatch does.
    pub fn lookup(&self, rva: u32) -> Option<u32> {
        self.find(rva).map(|index| (self.slots[index] >> 32) as u32)
    }

    /// The bytes the loader places where `x19` points: the two base addresses, then the
    /// slots, little-endian.
    pub fn runtime_bytes(&self, guest_base: u64, host_base: u64) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(entries_table::SLOTS as usize + 8 * self.slots.len());
        bytes.extend_from_slice(&guest_base.to_le_bytes());
        bytes.extend_from_slice(&host_base.to_le_bytes());
        for slot in &self.slots {
            bytes.extend_from_slice(&slot.to_le_bytes());
        }
        bytes
    }
}
