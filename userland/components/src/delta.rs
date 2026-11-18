//! Binary deltas between two copies of a component's file (D698): what a release store sends
//! instead of a changed file's whole bytes. A delta names the old and the new copy by length
//! and SHA-256 and holds a list of steps, each copying a run of the old copy or inserting new
//! bytes. [`encode`] finds the runs the two copies share through a table of the old copy's
//! blocks and a rolling hash over the new one; [`apply`] checks everything a delta says
//! against hard limits and both digests, so a damaged or hostile delta never yields a file.

use crate::index::{
    MAX_FILE_BYTES, MAX_INDEX_BYTES, SECTOR, is_component_path, parse_digest, parse_number, sha256,
};
use alloc::string::String;
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

/// The first bytes of a deltas file, the one a release store's deltas are published in.
pub const DELTAS_MAGIC: &[u8; 16] = b"EGGOS-DELTAS-V1\0";
/// The first line of a deltas file's index.
pub const DELTAS_TAG: &str = "eggos-deltas 1";
/// The most deltas one deltas file names.
pub const MOST_DELTAS: usize = 512;

const DELTAS_LENGTH: usize = 16;
const DELTAS_DIGEST: usize = 24;

/// One delta a deltas file holds: for the file `path`, from the copy whose SHA-256 is
/// `old_digest` to the one whose SHA-256 is `new_digest`, at `offset` for `length` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEntry {
    pub path: String,
    pub old_digest: [u8; 32],
    pub new_digest: [u8; 32],
    pub offset: u64,
    pub length: u64,
}

/// A deltas file's index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeltaIndex {
    pub entries: Vec<DeltaEntry>,
}

impl DeltaIndex {
    /// The delta that turns `path`'s copy `old_digest` into its copy `new_digest`.
    pub fn find(&self, path: &str, old_digest: &[u8; 32], new_digest: &[u8; 32]) -> Option<&DeltaEntry> {
        self.entries.iter().find(|entry| {
            entry.path == path && &entry.old_digest == old_digest && &entry.new_digest == new_digest
        })
    }
}

fn hex(out: &mut String, digest: &[u8; 32]) {
    use core::fmt::Write;
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
}

/// A deltas file: a [`SECTOR`]-byte header (magic, the index's length and SHA-256), the index
/// (`delta <offset> <length> <old> <new> <path>` lines) and each delta's bytes.
pub fn deltas_image(deltas: &[(String, [u8; 32], [u8; 32], Vec<u8>)]) -> Vec<u8> {
    let mut data_start = SECTOR * 2;
    let text = loop {
        let mut text = String::from(DELTAS_TAG);
        text.push('\n');
        let mut offset = data_start;
        for (path, old_digest, new_digest, bytes) in deltas {
            use core::fmt::Write;
            let _ = write!(text, "delta {offset} {} ", bytes.len());
            hex(&mut text, old_digest);
            text.push(' ');
            hex(&mut text, new_digest);
            let _ = writeln!(text, " {path}");
            offset += (bytes.len() as u64).div_ceil(SECTOR).max(1) * SECTOR;
        }
        let needed = (SECTOR + text.len() as u64).div_ceil(SECTOR) * SECTOR;
        if needed <= data_start {
            break text;
        }
        data_start = needed;
    };
    let mut image = alloc::vec![0u8; SECTOR as usize];
    image[..DELTAS_MAGIC.len()].copy_from_slice(DELTAS_MAGIC);
    image[DELTAS_LENGTH..DELTAS_LENGTH + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
    image[DELTAS_DIGEST..DELTAS_DIGEST + 32].copy_from_slice(&sha256(text.as_bytes()));
    image.extend_from_slice(text.as_bytes());
    image.resize(data_start as usize, 0);
    for (_, _, _, bytes) in deltas {
        image.extend_from_slice(bytes);
        image.resize(image.len().next_multiple_of(SECTOR as usize), 0);
    }
    image
}

fn delta_line(line: &str) -> Option<DeltaEntry> {
    let mut words = line.splitn(6, ' ');
    if words.next()? != "delta" {
        return None;
    }
    let offset = parse_number(words.next()?)?;
    let length = parse_number(words.next()?)?;
    let old_digest = parse_digest(words.next()?)?;
    let new_digest = parse_digest(words.next()?)?;
    let path = words.next()?;
    let sound = offset % SECTOR == 0
        && length <= MAX_FILE_BYTES
        && offset.checked_add(length).is_some()
        && is_component_path(path);
    sound.then(|| DeltaEntry {
        path: path.into(),
        old_digest,
        new_digest,
        offset,
        length,
    })
}

/// Reads a deltas file's index from its first bytes (`head`, the header and at least the
/// index); every line is checked against the format's limits.
pub fn parse_deltas(head: &[u8]) -> Result<DeltaIndex, DeltaError> {
    if head.len() < SECTOR as usize || &head[..DELTAS_MAGIC.len()] != DELTAS_MAGIC {
        return Err(DeltaError::NotADelta);
    }
    let mut length = [0u8; 4];
    length.copy_from_slice(&head[DELTAS_LENGTH..DELTAS_LENGTH + 4]);
    let length = u32::from_le_bytes(length);
    if length == 0 || length > MAX_INDEX_BYTES {
        return Err(DeltaError::Malformed);
    }
    let text = head
        .get(SECTOR as usize..SECTOR as usize + length as usize)
        .ok_or(DeltaError::Malformed)?;
    if sha256(text) != head[DELTAS_DIGEST..DELTAS_DIGEST + 32] {
        return Err(DeltaError::Damaged);
    }
    let text = core::str::from_utf8(text).map_err(|_| DeltaError::Malformed)?;
    let mut lines = text.lines();
    if lines.next() != Some(DELTAS_TAG) {
        return Err(DeltaError::NotADelta);
    }
    let mut index = DeltaIndex::default();
    for line in lines {
        let entry = delta_line(line).ok_or(DeltaError::Malformed)?;
        if index.entries.len() == MOST_DELTAS {
            return Err(DeltaError::Malformed);
        }
        index.entries.push(entry);
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deltas_file_names_each_delta_where_its_bytes_are() {
        let old = sample(8192, 21);
        let mut new = old.clone();
        new[100] = new[100].wrapping_add(1);
        let delta = encode(&old, &new);
        let path = String::from("usr/lib/eggos/windows/ntdll.dll");
        let image = deltas_image(&[(path.clone(), sha256(&old), sha256(&new), delta.clone())]);
        let index = parse_deltas(&image).unwrap();
        let entry = index.find(&path, &sha256(&old), &sha256(&new)).unwrap();
        let start = entry.offset as usize;
        assert_eq!(&image[start..start + entry.length as usize], delta.as_slice());
        assert!(index.find(&path, &sha256(&new), &sha256(&old)).is_none());
        let mut damaged = image.clone();
        damaged[SECTOR as usize + 3] ^= 1;
        assert_eq!(parse_deltas(&damaged), Err(DeltaError::Damaged));
    }

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
