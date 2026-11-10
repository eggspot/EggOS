//! A hive: one tree of keys and values, and its file. The file is a 24-byte header (the magic
//! `EGGREGv1`, a format version, the payload's CRC-32 and its length) and a payload of every key
//! in pre-order, each with its depth below the root, its name, its last write time and its
//! values. Subkeys are kept sorted ignoring case, as Windows enumerates them; values keep the
//! order they were made in. Every file is untrusted: lengths, depths, order and names are
//! checked, and a damaged file is [`Error::HiveDamaged`], never a panic.

use alloc::vec::Vec;

use crate::{
    Error, MAX_DATA_BYTES, MAX_DEPTH, MAX_HIVE_BYTES, MAX_KEY_NAME_UNITS, MAX_VALUE_NAME_UNITS,
    compare_names, same_name,
};

const MAGIC: [u8; 8] = *b"EGGREGv1";
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 24;
const KEY_FIXED_BYTES: usize = 2 + 2 + 8 + 4;
const VALUE_FIXED_BYTES: usize = 2 + 4 + 4;

/// One value: its name (empty for the key's default value), its type and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// Its name in UTF-16 units.
    pub name: Vec<u16>,
    /// Its type, `REG_SZ` and the others (see [`crate::value`]).
    pub kind: u32,
    /// Its data, as Windows stores it (strings in UTF-16 with their NULs).
    pub data: Vec<u8>,
}

/// One key: its name, when it last changed (a `FILETIME`), its values and its subkeys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Key {
    /// Its name in UTF-16 units; empty for a hive's root.
    pub name: Vec<u16>,
    /// When it or its values last changed, in 100 ns since 1601.
    pub last_write: u64,
    /// Its values, in the order they were made.
    pub values: Vec<Value>,
    /// Its subkeys, sorted ignoring case.
    pub subkeys: Vec<Key>,
}

impl Key {
    /// A key with this name and nothing in it.
    pub fn named(name: &[u16], last_write: u64) -> Key {
        Key {
            name: name.to_vec(),
            last_write,
            values: Vec::new(),
            subkeys: Vec::new(),
        }
    }

    /// Where the subkey named `name` is, or where it would go.
    pub fn subkey_index(&self, name: &[u16]) -> Result<usize, usize> {
        self.subkeys
            .binary_search_by(|key| compare_names(&key.name, name))
    }

    /// The subkey named `name`.
    pub fn subkey(&self, name: &[u16]) -> Option<&Key> {
        self.subkey_index(name)
            .ok()
            .map(|index| &self.subkeys[index])
    }

    /// The key `names` below this one.
    pub fn find(&self, names: &[Vec<u16>]) -> Option<&Key> {
        let mut key = self;
        for name in names {
            key = key.subkey(name)?;
        }
        Some(key)
    }

    /// The key `names` below this one, to change.
    pub fn find_mut(&mut self, names: &[Vec<u16>]) -> Option<&mut Key> {
        let mut key = self;
        for name in names {
            let index = key.subkey_index(name).ok()?;
            key = &mut key.subkeys[index];
        }
        Some(key)
    }

    /// The value named `name`.
    pub fn value(&self, name: &[u16]) -> Option<&Value> {
        self.values
            .iter()
            .find(|value| same_name(&value.name, name))
    }

    /// How many keys and values are below this key, itself not counted.
    pub fn count_below(&self) -> (usize, usize) {
        let mut keys = 0;
        let mut values = 0;
        let mut pending: Vec<&Key> = self.subkeys.iter().collect();
        while let Some(key) = pending.pop() {
            keys += 1;
            values += key.values.len();
            pending.extend(key.subkeys.iter());
        }
        (keys, values)
    }

    /// How deep the deepest key below this one is, counting this key's subkeys as 1.
    pub fn depth_below(&self) -> usize {
        let mut deepest = 0;
        let mut pending: Vec<(&Key, usize)> = self.subkeys.iter().map(|key| (key, 1)).collect();
        while let Some((key, depth)) = pending.pop() {
            deepest = deepest.max(depth);
            pending.extend(key.subkeys.iter().map(|key| (key, depth + 1)));
        }
        deepest
    }
}

/// A whole hive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hive {
    /// Its root key, which has no name.
    pub root: Key,
}

impl Hive {
    /// The file's bytes, or [`Error::HiveFull`] when they would pass [`MAX_HIVE_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let payload_bytes = self.payload_bytes();
        if payload_bytes > MAX_HIVE_BYTES - HEADER_BYTES {
            return Err(Error::HiveFull);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(HEADER_BYTES + payload_bytes)
            .map_err(|_| Error::HiveFull)?;
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(payload_bytes as u64).to_le_bytes());
        let mut pending: Vec<(&Key, u16)> = Vec::new();
        pending.push((&self.root, 0));
        while let Some((key, depth)) = pending.pop() {
            out.extend_from_slice(&depth.to_le_bytes());
            put_name(&mut out, &key.name);
            out.extend_from_slice(&key.last_write.to_le_bytes());
            out.extend_from_slice(&(key.values.len() as u32).to_le_bytes());
            for value in &key.values {
                put_name(&mut out, &value.name);
                out.extend_from_slice(&value.kind.to_le_bytes());
                out.extend_from_slice(&(value.data.len() as u32).to_le_bytes());
                out.extend_from_slice(&value.data);
            }
            pending.extend(key.subkeys.iter().rev().map(|key| (key, depth + 1)));
        }
        let checksum = crc32(&out[HEADER_BYTES..]);
        out[12..16].copy_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }

    fn payload_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut pending: Vec<&Key> = Vec::new();
        pending.push(&self.root);
        while let Some(key) = pending.pop() {
            total = total.saturating_add(KEY_FIXED_BYTES + key.name.len() * 2);
            for value in &key.values {
                total = total
                    .saturating_add(VALUE_FIXED_BYTES + value.name.len() * 2 + value.data.len());
            }
            pending.extend(key.subkeys.iter());
        }
        total
    }

    /// The hive a file holds, or [`Error::HiveDamaged`].
    pub fn decode(bytes: &[u8]) -> Result<Hive, Error> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_HIVE_BYTES || bytes[..8] != MAGIC {
            return Err(Error::HiveDamaged);
        }
        let mut header = Reader {
            bytes: &bytes[8..HEADER_BYTES],
        };
        let version = header.u32()?;
        let checksum = header.u32()?;
        let payload_bytes = header.u64()?;
        let payload = &bytes[HEADER_BYTES..];
        if version != VERSION || payload_bytes != payload.len() as u64 || crc32(payload) != checksum
        {
            return Err(Error::HiveDamaged);
        }
        let mut reader = Reader { bytes: payload };
        let mut open: Vec<Key> = Vec::new();
        while !reader.bytes.is_empty() {
            let depth = usize::from(reader.u16()?);
            let key = read_key(&mut reader)?;
            if depth != open.len() || open.is_empty() {
                if depth == 0 && open.is_empty() {
                    if !key.name.is_empty() {
                        return Err(Error::HiveDamaged);
                    }
                    open.push(key);
                    continue;
                }
                if depth == 0 || depth > open.len() {
                    return Err(Error::HiveDamaged);
                }
                while open.len() > depth {
                    close_innermost(&mut open)?;
                }
            }
            if depth > MAX_DEPTH || !is_key_name(&key.name) {
                return Err(Error::HiveDamaged);
            }
            open.push(key);
        }
        while open.len() > 1 {
            close_innermost(&mut open)?;
        }
        let root = open.pop().ok_or(Error::HiveDamaged)?;
        Ok(Hive { root })
    }
}

fn close_innermost(open: &mut Vec<Key>) -> Result<(), Error> {
    let child = open.pop().ok_or(Error::HiveDamaged)?;
    let parent = open.last_mut().ok_or(Error::HiveDamaged)?;
    if let Some(last) = parent.subkeys.last()
        && !compare_names(&last.name, &child.name).is_lt()
    {
        return Err(Error::HiveDamaged);
    }
    parent.subkeys.push(child);
    Ok(())
}

fn read_key(reader: &mut Reader<'_>) -> Result<Key, Error> {
    let name = reader.name(MAX_KEY_NAME_UNITS)?;
    let last_write = reader.u64()?;
    let count = reader.u32()? as usize;
    if count > reader.bytes.len() / VALUE_FIXED_BYTES {
        return Err(Error::HiveDamaged);
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.name(MAX_VALUE_NAME_UNITS)?;
        let kind = reader.u32()?;
        let length = reader.u32()? as usize;
        if length > MAX_DATA_BYTES {
            return Err(Error::HiveDamaged);
        }
        let data = reader.take(length)?.to_vec();
        values.push(Value { name, kind, data });
    }
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_unstable_by(|a, b| compare_names(&values[*a].name, &values[*b].name));
    if order
        .windows(2)
        .any(|pair| same_name(&values[pair[0]].name, &values[pair[1]].name))
    {
        return Err(Error::HiveDamaged);
    }
    Ok(Key {
        name,
        last_write,
        values,
        subkeys: Vec::new(),
    })
}

/// Whether `name` may name a key: 1 to [`MAX_KEY_NAME_UNITS`] units, no `\` and no NUL.
pub fn is_key_name(name: &[u16]) -> bool {
    (1..=MAX_KEY_NAME_UNITS).contains(&name.len())
        && !name
            .iter()
            .any(|unit| *unit == u16::from(b'\\') || *unit == 0)
}

/// Whether `name` may name a value: at most [`MAX_VALUE_NAME_UNITS`] units and no NUL.
pub fn is_value_name(name: &[u16]) -> bool {
    name.len() <= MAX_VALUE_NAME_UNITS && !name.contains(&0)
}

fn put_name(out: &mut Vec<u8>, name: &[u16]) {
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    for unit in name {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.bytes.len() {
            return Err(Error::HiveDamaged);
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let mut word = [0u8; 4];
        word.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(word))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let mut word = [0u8; 8];
        word.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(word))
    }

    fn name(&mut self, longest: usize) -> Result<Vec<u16>, Error> {
        let length = usize::from(self.u16()?);
        if length > longest {
            return Err(Error::HiveDamaged);
        }
        let bytes = self.take(length * 2)?;
        let name: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        if name.contains(&0) {
            return Err(Error::HiveDamaged);
        }
        Ok(name)
    }
}

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut entry = index as u32;
        let mut bit = 0;
        while bit < 8 {
            entry = if entry & 1 != 0 {
                0xedb8_8320 ^ (entry >> 1)
            } else {
                entry >> 1
            };
            bit += 1;
        }
        table[index] = entry;
        index += 1;
    }
    table
}

/// The CRC-32 (IEEE 802.3) of `bytes`.
pub fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, byte| {
        CRC_TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8)
    })
}
