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
const CLASS_DATA: usize = 24;
const SUPERCLASS: usize = 8;
const NO_INDEX: u32 = u32::MAX;
const METHOD_ID_BYTES: usize = 8;
const METHOD_NAME: usize = 4;
const METHOD_PROTOTYPE: usize = 2;
const FIELD_ID_BYTES: usize = 8;
const CODE_HEADER_BYTES: usize = 16;

/// A method's code (`code_item`, D471): its register count, how many of them hold its
/// arguments (the last ones), the outgoing argument words it needs, its `try` blocks, and its
/// instructions, 16-bit code units in the file's little-endian order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code<'a> {
    pub registers: u16,
    pub ins: u16,
    pub outs: u16,
    pub tries: u16,
    pub instructions: &'a [u8],
}

impl Code<'_> {
    /// How many code units it has.
    pub fn length(&self) -> usize {
        self.instructions.len() / 2
    }

    /// Code unit `at`.
    pub fn unit(&self, at: usize) -> Option<u16> {
        let bytes = self.instructions.get(2 * at..2 * at + 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

/// A method a class defines: its index in the method table, its access flags, and its code
/// unless it is abstract or native.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Method<'a> {
    pub index: u32,
    pub access: u32,
    pub code: Option<Code<'a>>,
}

/// A DEX file's header and tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dex<'a> {
    bytes: &'a [u8],
    /// The format version, `35` to `41`.
    pub version: u32,
    pub strings: (u32, u32),
    pub types: (u32, u32),
    pub fields: (u32, u32),
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

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
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
            fields: pair(0x50),
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

    /// Method `index`'s name (`onCreate`).
    pub fn method_name(&self, index: u32) -> Option<String> {
        let (count, offset) = self.methods;
        if index >= count {
            return None;
        }
        let at = offset as usize + METHOD_ID_BYTES * index as usize + METHOD_NAME;
        self.string(word(self.bytes, at)?)
    }

    /// The descriptor of the class method `index` belongs to (`Landroid/app/Activity;`).
    pub fn method_class(&self, index: u32) -> Option<String> {
        let (count, offset) = self.methods;
        if index >= count {
            return None;
        }
        let at = offset as usize + METHOD_ID_BYTES * index as usize;
        self.type_name(u32::from(half(self.bytes, at)?))
    }

    /// Method `index`'s prototype, its place in the prototype table: two methods with the same
    /// name and prototype have the same signature, as a file keeps each prototype once.
    pub fn method_prototype(&self, index: u32) -> Option<u16> {
        let (count, offset) = self.methods;
        if index >= count {
            return None;
        }
        half(self.bytes, offset as usize + METHOD_ID_BYTES * index as usize + METHOD_PROTOTYPE)
    }

    /// The descriptor of the class field `index` belongs to.
    pub fn field_class(&self, index: u32) -> Option<String> {
        let (count, offset) = self.fields;
        if index >= count {
            return None;
        }
        let at = offset as usize + FIELD_ID_BYTES * index as usize;
        self.type_name(u32::from(half(self.bytes, at)?))
    }

    /// How many methods the method table names.
    pub fn method_count(&self) -> u32 {
        self.methods.0
    }

    /// The place in the class table of the class with descriptor `descriptor`, when the file
    /// defines it.
    pub fn class_index(&self, descriptor: &str) -> Option<u32> {
        let (count, offset) = self.classes;
        (0..count).find(|&index| {
            let at = offset as usize + CLASS_DEF_BYTES * index as usize;
            word(self.bytes, at)
                .and_then(|type_index| self.type_name(type_index))
                .is_some_and(|name| name == descriptor)
        })
    }

    /// The descriptor of class definition `class`'s superclass, if it has one.
    pub fn superclass(&self, class: u32) -> Option<String> {
        let (count, offset) = self.classes;
        if class >= count {
            return None;
        }
        let at = offset as usize + CLASS_DEF_BYTES * class as usize + SUPERCLASS;
        match word(self.bytes, at)? {
            NO_INDEX => None,
            index => self.type_name(index),
        }
    }

    /// How many classes the file defines.
    pub fn class_count(&self) -> u32 {
        self.classes.0
    }

    /// The code item at `offset`.
    pub fn code(&self, offset: u32) -> Option<Code<'a>> {
        let at = offset as usize;
        let units = word(self.bytes, at + 12)? as usize;
        let start = at + CODE_HEADER_BYTES;
        Some(Code {
            registers: half(self.bytes, at)?,
            ins: half(self.bytes, at + 2)?,
            outs: half(self.bytes, at + 4)?,
            tries: half(self.bytes, at + 6)?,
            instructions: self.bytes.get(start..start.checked_add(units.checked_mul(2)?)?)?,
        })
    }

    /// The methods class definition `class` (its place in the class table) defines: direct
    /// methods, then virtual ones, from its class data. Empty for a class without any.
    pub fn methods(&self, class: u32) -> Vec<Method<'a>> {
        self.read_methods(class).unwrap_or_default()
    }

    fn read_methods(&self, class: u32) -> Option<Vec<Method<'a>>> {
        let (count, offset) = self.classes;
        if class >= count {
            return None;
        }
        let definition = offset as usize + CLASS_DEF_BYTES * class as usize;
        let mut at = word(self.bytes, definition + CLASS_DATA)? as usize;
        if at == 0 {
            return Some(Vec::new());
        }
        let static_fields = uleb(self.bytes, &mut at)?;
        let instance_fields = uleb(self.bytes, &mut at)?;
        let direct = uleb(self.bytes, &mut at)?;
        let virtual_methods = uleb(self.bytes, &mut at)?;
        for _ in 0..static_fields.checked_add(instance_fields)? {
            uleb(self.bytes, &mut at)?;
            uleb(self.bytes, &mut at)?;
        }
        let mut methods = Vec::new();
        for list in [direct, virtual_methods] {
            let mut index = 0u32;
            for _ in 0..list {
                index = index.checked_add(uleb(self.bytes, &mut at)?)?;
                let access = uleb(self.bytes, &mut at)?;
                let code = match uleb(self.bytes, &mut at)? {
                    0 => None,
                    offset => Some(self.code(offset)?),
                };
                methods.push(Method {
                    index,
                    access,
                    code,
                });
            }
        }
        Some(methods)
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
    fn a_class_s_methods_come_with_their_code() {
        let mut file = sample();
        file.resize(0x200, 0);
        let put = |file: &mut Vec<u8>, at: usize, value: u32| {
            file[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        put(&mut file, 0x20, 0x200);
        put(&mut file, 0x78 + CLASS_DATA, 0x100);
        file[0x100..0x10b].copy_from_slice(&[0, 0, 1, 1, 2, 1, 0x80, 0x03, 3, 1, 0]);
        put(&mut file, 0x180, 0x0001_0003);
        put(&mut file, 0x184, 0x0000_0001);
        put(&mut file, 0x18c, 2);
        file[0x190..0x194].copy_from_slice(&[0x12, 0x10, 0x0f, 0x00]);
        let dex = Dex::parse(&file).unwrap();
        let methods = dex.methods(0);
        assert_eq!(methods.len(), 2);
        assert_eq!((methods[0].index, methods[0].access), (2, 1));
        let code = methods[0].code.unwrap();
        assert_eq!((code.registers, code.ins, code.outs), (3, 1, 1));
        assert_eq!(code.length(), 2);
        assert_eq!(code.unit(0), Some(0x1012));
        assert_eq!((methods[1].index, methods[1].access), (3, 1));
        assert_eq!(methods[1].code, None);
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
