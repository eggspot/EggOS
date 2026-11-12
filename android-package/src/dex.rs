//! DEX files (D450): the header (its magic and version, its size and endianness tag) and the
//! id tables it locates: strings (MUTF-8, each after its ULEB128 length), types (a string id
//! each), methods and class definitions. [`Dex::parse`] checks the header and reads the
//! counts; [`Dex::class_names`] the descriptors of the classes the file defines, which ahead
//! of time compilation starts from.

use alloc::string::String;
use alloc::vec::Vec;

const MAGIC: &[u8; 4] = b"dex\n";
const HEADER_BYTES: usize = 0x70;
const ENDIAN_TAG: u32 = 0x1234_5678;
const CLASS_DEF_BYTES: usize = 32;

/// A DEX file's header and tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dex<'a> {
    bytes: &'a [u8],
    /// The format version, `35` to `41`.
    pub version: u32,
    pub strings: (u32, u32),
    pub types: (u32, u32),
    pub methods: (u32, u32),
    pub classes: (u32, u32),
}

/// Why a DEX file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexError {
    NotDex,
    BigEndian,
    Truncated,
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn uleb(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

impl<'a> Dex<'a> {
    /// The DEX file `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, DexError> {
        if bytes.len() < HEADER_BYTES || &bytes[..4] != MAGIC || bytes[7] != 0 {
            return Err(DexError::NotDex);
        }
        let version = core::str::from_utf8(&bytes[4..7])
            .ok()
            .and_then(|digits| digits.parse().ok())
            .ok_or(DexError::NotDex)?;
        if word(bytes, 0x28) != Some(ENDIAN_TAG) {
            return Err(DexError::BigEndian);
        }
        let size = word(bytes, 0x20).ok_or(DexError::Truncated)? as usize;
        if size > bytes.len() {
            return Err(DexError::Truncated);
        }
        let pair = |at: usize| (word(bytes, at).unwrap_or(0), word(bytes, at + 4).unwrap_or(0));
        Ok(Self {
            bytes: &bytes[..size],
            version,
            strings: pair(0x38),
            types: pair(0x40),
            methods: pair(0x58),
            classes: pair(0x60),
        })
    }

    /// String `index`, decoded from MUTF-8 (its surrogate pairs and `0xc0 0x80` NULs kept as
    /// the lossy UTF-8 reading gives them).
    pub fn string(&self, index: u32) -> Option<String> {
        let (count, offset) = self.strings;
        if index >= count {
            return None;
        }
        let mut at = word(self.bytes, offset as usize + 4 * index as usize)? as usize;
        let _length = uleb(self.bytes, &mut at)?;
        let end = at + self.bytes.get(at..)?.iter().position(|&byte| byte == 0)?;
        Some(String::from_utf8_lossy(&self.bytes[at..end]).into_owned())
    }

    /// Type `index`'s descriptor (`Lorg/example/Main;`).
    pub fn type_name(&self, index: u32) -> Option<String> {
        let (count, offset) = self.types;
        if index >= count {
            return None;
        }
        self.string(word(self.bytes, offset as usize + 4 * index as usize)?)
    }

    /// The descriptors of the classes the file defines, in its order.
    pub fn class_names(&self) -> Vec<String> {
        let (count, offset) = self.classes;
        (0..count)
            .filter_map(|index| {
                let at = offset as usize + CLASS_DEF_BYTES * index as usize;
                self.type_name(word(self.bytes, at)?)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut file = vec![0u8; 0x100];
        file[..8].copy_from_slice(b"dex\n035\0");
        file[0x28..0x2c].copy_from_slice(&ENDIAN_TAG.to_le_bytes());
        let put = |file: &mut Vec<u8>, at: usize, value: u32| file[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(&mut file, 0x20, 0x100);
        put(&mut file, 0x38, 1);
        put(&mut file, 0x3c, 0x70);
        put(&mut file, 0x40, 1);
        put(&mut file, 0x44, 0x74);
        put(&mut file, 0x60, 1);
        put(&mut file, 0x64, 0x78);
        put(&mut file, 0x70, 0xa0);
        put(&mut file, 0x74, 0);
        put(&mut file, 0x78, 0);
        let name = b"Lorg/example/Main;";
        file[0xa0] = name.len() as u8;
        file[0xa1..0xa1 + name.len()].copy_from_slice(name);
        file
    }

    #[test]
    fn a_dex_file_names_the_classes_it_defines() {
        let file = sample();
        let dex = Dex::parse(&file).unwrap();
        assert_eq!(dex.version, 35);
        assert_eq!(dex.class_names(), ["Lorg/example/Main;"]);
        assert_eq!(dex.type_name(1), None);
    }

    #[test]
    fn other_files_are_refused() {
        assert_eq!(Dex::parse(&[0; 0x70]), Err(DexError::NotDex));
        let mut file = sample();
        file[0x28..0x2c].copy_from_slice(&0x7856_3412u32.to_le_bytes());
        assert_eq!(Dex::parse(&file), Err(DexError::BigEndian));
        file = sample();
        file[0x20..0x24].copy_from_slice(&0x1000u32.to_le_bytes());
        assert_eq!(Dex::parse(&file), Err(DexError::Truncated));
    }
}
