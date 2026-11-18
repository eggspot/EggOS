//! Binary deltas between two copies of a component's file (D698): what a release store sends
//! instead of a changed file's whole bytes. A delta names the old and the new copy by length
//! and SHA-256 and holds a list of steps, each copying a run of the old copy or inserting new
//! bytes. [`encode`] finds the runs the two copies share through a table of the old copy's
//! blocks and a rolling hash over the new one; [`apply`] checks everything a delta says
//! against hard limits and both digests, so a damaged or hostile delta never yields a file.

use crate::index::{MAX_FILE_BYTES, sha256};
use alloc::vec::Vec;

/// The first bytes of a delta.
pub const MAGIC: &[u8; 8] = b"EGGDELTA";
/// The delta format this code reads and writes.
pub const FORMAT: u8 = 1;
/// The bytes before the first step: magic, format, both lengths and both digests.
pub const HEADER_BYTES: usize = 8 + 1 + 8 + 8 + 32 + 32;

const BLOCK: usize = 16;
const COPY: u8 = 1;
const INSERT: u8 = 2;
const END: u8 = 0;
const HASH_BASE: u64 = 0x0100_0000_01b3;
const NO_BLOCK: u32 = u32::MAX;
const MOST_STEPS: usize = 1 << 24;

/// Why a delta cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaError {
    /// It does not start with [`MAGIC`] and [`FORMAT`].
    NotADelta,
    /// It was made from another copy than the one given.
    WrongBase,
    /// A step reads outside the old copy, the steps overrun the new length, or they end early.
    Malformed,
    /// The result's SHA-256 is not the one the delta names.
    Damaged,
}

/// What a delta says about the copies it joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaHeader {
    /// The old copy's length.
    pub old_bytes: u64,
    /// The new copy's length.
    pub new_bytes: u64,
    /// The old copy's SHA-256.
    pub old_digest: [u8; 32],
    /// The new copy's SHA-256.
    pub new_digest: [u8; 32],
}

impl DeltaHeader {
    /// Reads the header of `delta`.
    pub fn parse(delta: &[u8]) -> Result<Self, DeltaError> {
        if delta.len() < HEADER_BYTES || &delta[..MAGIC.len()] != MAGIC || delta[8] != FORMAT {
            return Err(DeltaError::NotADelta);
        }
        let number = |at: usize| {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&delta[at..at + 8]);
            u64::from_le_bytes(bytes)
        };
        let digest = |at: usize| {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&delta[at..at + 32]);
            bytes
        };
        let header = Self {
            old_bytes: number(9),
            new_bytes: number(17),
            old_digest: digest(25),
            new_digest: digest(57),
        };
        if header.old_bytes > MAX_FILE_BYTES || header.new_bytes > MAX_FILE_BYTES {
            return Err(DeltaError::Malformed);
        }
        Ok(header)
    }
}

fn put_number(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        match value {
            0 => {
                out.push(low);
                return;
            }
            _ => out.push(low | 0x80),
        }
    }
}

fn read_number(delta: &[u8], at: &mut usize) -> Result<u64, DeltaError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *delta.get(*at).ok_or(DeltaError::Malformed)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DeltaError::Malformed)
}

fn block_hash(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0u64, |hash, &byte| hash.wrapping_mul(HASH_BASE).wrapping_add(u64::from(byte)))
}

struct BlockTable {
    slots: Vec<u32>,
    mask: usize,
}

impl BlockTable {
    fn of(old: &[u8]) -> Self {
        let blocks = old.len() / BLOCK;
        let size = (blocks * 2).max(16).next_power_of_two();
        let mut table = Self {
            slots: alloc::vec![NO_BLOCK; size],
            mask: size - 1,
        };
        for block in 0..blocks {
            let at = block * BLOCK;
            let mut slot = table.slot_of(block_hash(&old[at..at + BLOCK]));
            while table.slots[slot] != NO_BLOCK {
                slot = (slot + 1) & table.mask;
            }
            table.slots[slot] = block as u32;
        }
        table
    }

    fn slot_of(&self, hash: u64) -> usize {
        (hash ^ (hash >> 29)) as usize & self.mask
    }

    fn candidates(&self, hash: u64) -> impl Iterator<Item = usize> + '_ {
        let start = self.slot_of(hash);
        (0..=self.mask)
            .map(move |step| self.slots[(start + step) & self.mask])
            .take_while(|&block| block != NO_BLOCK)
            .map(|block| block as usize * BLOCK)
    }
}

fn common_length(old: &[u8], new: &[u8]) -> usize {
    old.iter().zip(new).take_while(|(a, b)| a == b).count()
}

fn longest_match(table: &BlockTable, old: &[u8], new: &[u8], hash: u64) -> Option<(usize, usize)> {
    table
        .candidates(hash)
        .take(8)
        .map(|start| (start, common_length(&old[start..], new)))
        .filter(|&(_, length)| length >= BLOCK)
        .max_by_key(|&(_, length)| length)
}

fn put_insert(out: &mut Vec<u8>, bytes: &[u8]) {
    if !bytes.is_empty() {
        out.push(INSERT);
        put_number(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }
}

/// The delta that turns `old` into `new`.
pub fn encode(old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + new.len() / 8);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.extend_from_slice(&(old.len() as u64).to_le_bytes());
    out.extend_from_slice(&(new.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha256(old));
    out.extend_from_slice(&sha256(new));
    let table = BlockTable::of(old);
    let top_power = (0..BLOCK - 1).fold(1u64, |power, _| power.wrapping_mul(HASH_BASE));
    let mut pending = 0;
    let mut at = 0;
    let mut hash = new.get(..BLOCK).map_or(0, block_hash);
    while at + BLOCK <= new.len() {
        if let Some((start, length)) = longest_match(&table, old, &new[at..], hash) {
            let back = old[..start]
                .iter()
                .rev()
                .zip(new[pending..at].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            put_insert(&mut out, &new[pending..at - back]);
            out.push(COPY);
            put_number(&mut out, (start - back) as u64);
            put_number(&mut out, (length + back) as u64);
            at += length;
            pending = at;
            hash = new.get(at..at + BLOCK).map_or(0, block_hash);
            continue;
        }
        if at + BLOCK < new.len() {
            hash = hash
                .wrapping_sub(u64::from(new[at]).wrapping_mul(top_power))
                .wrapping_mul(HASH_BASE)
                .wrapping_add(u64::from(new[at + BLOCK]));
        }
        at += 1;
    }
    put_insert(&mut out, &new[pending..]);
    out.push(END);
    out
}

/// The new copy `delta` makes from `old`, checked against both of its digests.
pub fn apply(old: &[u8], delta: &[u8]) -> Result<Vec<u8>, DeltaError> {
    let header = DeltaHeader::parse(delta)?;
    if old.len() as u64 != header.old_bytes || sha256(old) != header.old_digest {
        return Err(DeltaError::WrongBase);
    }
    let wanted = header.new_bytes as usize;
    let mut new = Vec::new();
    new.try_reserve_exact(wanted).map_err(|_| DeltaError::Malformed)?;
    let mut at = HEADER_BYTES;
    for _ in 0..MOST_STEPS {
        let step = *delta.get(at).ok_or(DeltaError::Malformed)?;
        at += 1;
        match step {
            END if new.len() == wanted && at == delta.len() => {
                return match sha256(&new) == header.new_digest {
                    true => Ok(new),
                    false => Err(DeltaError::Damaged),
                };
            }
            COPY => {
                let start = read_number(delta, &mut at)?;
                let length = read_number(delta, &mut at)?;
                let end = start.checked_add(length).ok_or(DeltaError::Malformed)?;
                let run = old
                    .get(start as usize..end as usize)
                    .ok_or(DeltaError::Malformed)?;
                if new.len() + run.len() > wanted {
                    return Err(DeltaError::Malformed);
                }
                new.extend_from_slice(run);
            }
            INSERT => {
                let length = read_number(delta, &mut at)? as usize;
                let run = delta
                    .get(at..at.checked_add(length).ok_or(DeltaError::Malformed)?)
                    .ok_or(DeltaError::Malformed)?;
                if new.len() + run.len() > wanted {
                    return Err(DeltaError::Malformed);
                }
                new.extend_from_slice(run);
                at += length;
            }
            _ => return Err(DeltaError::Malformed),
        }
    }
    Err(DeltaError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(length: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) as u8
            })
            .collect()
    }

    #[test]
    fn a_small_change_makes_a_small_delta_that_rebuilds_the_new_copy() {
        let old = sample(64 * 1024, 7);
        let mut new = old.clone();
        new[1000..1010].copy_from_slice(b"0123456789");
        new.splice(30_000..30_000, sample(300, 9));
        new.drain(50_000..50_500);
        let delta = encode(&old, &new);
        assert!(delta.len() < 1024, "{} bytes", delta.len());
        assert_eq!(apply(&old, &delta), Ok(new));
    }

    #[test]
    fn unrelated_and_empty_copies_still_round_trip() {
        for (old, new) in [
            (sample(5000, 1), sample(7000, 2)),
            (Vec::new(), sample(100, 3)),
            (sample(100, 4), Vec::new()),
            (sample(10, 5), sample(10, 5)),
        ] {
            assert_eq!(apply(&old, &encode(&old, &new)), Ok(new));
        }
    }

    #[test]
    fn a_wrong_base_or_a_damaged_delta_is_refused() {
        let old = sample(4096, 11);
        let mut new = old.clone();
        new[10] ^= 0xff;
        let delta = encode(&old, &new);
        let mut other = old.clone();
        other[0] ^= 1;
        assert_eq!(apply(&other, &delta), Err(DeltaError::WrongBase));
        let mut damaged = delta.clone();
        let last_insert = damaged.len() - 2;
        damaged[last_insert] ^= 0x55;
        assert!(apply(&old, &damaged).is_err());
        assert_eq!(apply(&old, &delta[..HEADER_BYTES + 1]), Err(DeltaError::Malformed));
        assert_eq!(apply(&old, b"not a delta"), Err(DeltaError::NotADelta));
    }
}
