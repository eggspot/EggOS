//! An APK's resource table, `resources.arsc` (D500): the packages' types and entries, each a
//! resource id (`0xPPTTEEEE`: package, type, entry) with a value per configuration. The
//! Android layer reads the simple string values, which hold both string resources
//! (`R.string.hello`) and the paths of layouts and other files (`R.layout.main` is
//! `res/layout/main.xml`); the first configuration that gives an entry a value wins, which is
//! the default one in the files the build tools write.

use crate::manifest::{half, strings, word};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

const TABLE: u16 = 0x0002;
const STRING_POOL: u16 = 0x0001;
const PACKAGE: u16 = 0x0200;
const TYPE: u16 = 0x0201;
const TYPE_STRING: u8 = 0x03;
const NO_ENTRY: u32 = u32::MAX;
const NO_SHORT_ENTRY: u16 = u16::MAX;
const FLAG_COMPLEX: u16 = 1;
const FLAG_SPARSE: u8 = 1;
const FLAG_OFFSET_16: u8 = 2;
const PACKAGE_HEADER_ID: usize = 8;
const TYPE_ID: usize = 8;
const TYPE_FLAGS: usize = 9;
const TYPE_ENTRY_COUNT: usize = 12;
const TYPE_ENTRIES_START: usize = 16;
const MOST_ENTRIES: usize = 1 << 16;

/// The string values of a resource table, by resource id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceTable {
    strings: BTreeMap<u32, String>,
}

impl ResourceTable {
    /// The table in `bytes`; `None` when it is not one.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if half(bytes, 0)? != TABLE {
            return None;
        }
        let mut table = Self::default();
        let mut pool = Vec::new();
        let mut at = usize::from(half(bytes, 2)?);
        while at + 8 <= bytes.len() {
            let kind = half(bytes, at)?;
            let size = word(bytes, at + 4)? as usize;
            if size < 8 {
                return None;
            }
            let chunk = bytes.get(at..at + size)?;
            match kind {
                STRING_POOL => pool = strings(chunk)?,
                PACKAGE => table.package(chunk, &pool)?,
                _ => {}
            }
            at += size;
        }
        Some(table)
    }

    fn package(&mut self, chunk: &[u8], pool: &[String]) -> Option<()> {
        let package = word(chunk, PACKAGE_HEADER_ID)? & 0xff;
        let mut at = usize::from(half(chunk, 2)?);
        while at + 8 <= chunk.len() {
            let kind = half(chunk, at)?;
            let size = word(chunk, at + 4)? as usize;
            if size < 8 {
                return None;
            }
            let inner = chunk.get(at..at + size)?;
            if kind == TYPE {
                self.entries(package, inner, pool)?;
            }
            at += size;
        }
        Some(())
    }

    fn entries(&mut self, package: u32, chunk: &[u8], pool: &[String]) -> Option<()> {
        let type_id = u32::from(*chunk.get(TYPE_ID)?);
        let flags = *chunk.get(TYPE_FLAGS)?;
        let count = (word(chunk, TYPE_ENTRY_COUNT)? as usize).min(MOST_ENTRIES);
        let entries_start = word(chunk, TYPE_ENTRIES_START)? as usize;
        let offsets = usize::from(half(chunk, 2)?);
        for slot in 0..count {
            let (entry, offset) = match (flags & FLAG_SPARSE != 0, flags & FLAG_OFFSET_16 != 0) {
                (true, _) => {
                    let index = u32::from(half(chunk, offsets + 4 * slot)?);
                    let offset = usize::from(half(chunk, offsets + 4 * slot + 2)?) * 4;
                    (index, offset)
                }
                (false, true) => match half(chunk, offsets + 2 * slot)? {
                    NO_SHORT_ENTRY => continue,
                    offset => (slot as u32, usize::from(offset) * 4),
                },
                (false, false) => match word(chunk, offsets + 4 * slot)? {
                    NO_ENTRY => continue,
                    offset => (slot as u32, offset as usize),
                },
            };
            let at = entries_start + offset;
            let entry_size = usize::from(half(chunk, at)?);
            if half(chunk, at + 2)? & FLAG_COMPLEX != 0 {
                continue;
            }
            let value = at + entry_size;
            if *chunk.get(value + 3)? != TYPE_STRING {
                continue;
            }
            let Some(text) = pool.get(word(chunk, value + 4)? as usize) else {
                continue;
            };
            let id = package << 24 | type_id << 16 | entry;
            self.strings.entry(id).or_insert_with(|| text.clone());
        }
        Some(())
    }

    /// Resource `id`'s string value: a string resource's text or a file resource's path.
    pub fn string(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pool(strings: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for text in strings {
            offsets.push(data.len() as u32);
            data.push(text.len() as u8);
            data.push(text.len() as u8);
            data.extend_from_slice(text.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let start = 28 + 4 * strings.len();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&STRING_POOL.to_le_bytes());
        chunk.extend_from_slice(&28u16.to_le_bytes());
        chunk.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
        chunk.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        chunk.extend_from_slice(&(1u32 << 8).to_le_bytes());
        chunk.extend_from_slice(&(start as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        for offset in offsets {
            chunk.extend_from_slice(&offset.to_le_bytes());
        }
        chunk.extend_from_slice(&data);
        chunk
    }

    fn type_chunk(type_id: u8, values: &[Option<u32>]) -> Vec<u8> {
        let header = 20 + 64;
        let offsets_bytes = 4 * values.len();
        let entries_start = header + offsets_bytes;
        let mut entries = Vec::new();
        let mut offsets = Vec::new();
        for value in values {
            match value {
                None => offsets.extend_from_slice(&NO_ENTRY.to_le_bytes()),
                Some(string) => {
                    offsets.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                    entries.extend_from_slice(&8u16.to_le_bytes());
                    entries.extend_from_slice(&0u16.to_le_bytes());
                    entries.extend_from_slice(&0u32.to_le_bytes());
                    entries.extend_from_slice(&[8, 0, 0, TYPE_STRING]);
                    entries.extend_from_slice(&string.to_le_bytes());
                }
            }
        }
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&TYPE.to_le_bytes());
        chunk.extend_from_slice(&(header as u16).to_le_bytes());
        chunk.extend_from_slice(&((entries_start + entries.len()) as u32).to_le_bytes());
        chunk.extend_from_slice(&[type_id, 0, 0, 0]);
        chunk.extend_from_slice(&(values.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&(entries_start as u32).to_le_bytes());
        chunk.extend_from_slice(&[0; 64]);
        chunk.extend(offsets);
        chunk.extend(entries);
        chunk
    }

    #[test]
    fn string_and_layout_values_are_found_by_their_ids() {
        let strings = pool(&["Hello", "res/layout/main.xml"]);
        let mut package = Vec::new();
        package.extend(type_chunk(2, &[Some(1)]));
        package.extend(type_chunk(3, &[None, Some(0)]));
        let header = 288;
        let mut package_chunk = Vec::new();
        package_chunk.extend_from_slice(&PACKAGE.to_le_bytes());
        package_chunk.extend_from_slice(&(header as u16).to_le_bytes());
        package_chunk.extend_from_slice(&((header + package.len()) as u32).to_le_bytes());
        package_chunk.extend_from_slice(&0x7fu32.to_le_bytes());
        package_chunk.extend_from_slice(&vec![0u8; header - 12]);
        package_chunk.extend(package);
        let mut body = strings;
        body.extend(package_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&TABLE.to_le_bytes());
        bytes.extend_from_slice(&12u16.to_le_bytes());
        bytes.extend_from_slice(&((12 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend(body);
        let table = ResourceTable::parse(&bytes).unwrap();
        assert_eq!(table.string(0x7f02_0000), Some("res/layout/main.xml"));
        assert_eq!(table.string(0x7f03_0001), Some("Hello"));
        assert_eq!(table.string(0x7f03_0000), None);
    }
}
