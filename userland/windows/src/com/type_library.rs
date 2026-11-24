//! Type libraries in Microsoft's `MSFT` format (D745), as `.tlb` files and `TYPELIB` resources
//! hold them: the header (library id, locale, system kind, version, type count), the segment
//! directory, and from the segments each type's base record (kind, id, name, flags, function
//! and variable counts, implemented types, vtable size), its functions (member id, name,
//! invoke and function kind, calling convention, vtable slot, return type and parameters with
//! their names and flags) and the type descriptions they name (base kinds, pointers, safe
//! arrays, user types by reference). Read only, from the bytes, with host tests.

use alloc::boxed::Box;
use alloc::vec::Vec;

const MAGIC: u32 = 0x5446_534d;
const HEADER_BYTES: usize = 0x54;
const HELP_DLL_FLAG: u32 = 0x100;
const SEGMENTS: usize = 15;
const SEGMENT_BYTES: usize = 16;
const TYPE_INFO_BYTES: usize = 0x64;
const NAME_INTRO_BYTES: usize = 12;
const GUID_ENTRY_BYTES: usize = 24;
const TYPE_DESC_BYTES: usize = 8;
const PARAMETER_BYTES: usize = 12;
const FUNCTION_FIXED_BYTES: usize = 24;
const DEFAULT_VALUES: u32 = 0x1000;
const TYPE_MASK: u16 = 0x0fff;
const MOST_DESC_DEPTH: usize = 8;
const SYSTEM_64: u32 = 3;

/// Variant kinds a type description names besides the plain ones.
pub mod kind {
    pub const PTR: u16 = 26;
    pub const SAFEARRAY: u16 = 27;
    pub const CARRAY: u16 = 28;
    pub const USERDEFINED: u16 = 29;
}

/// Type kinds (`TYPEKIND`).
pub mod type_kind {
    pub const ENUM: u32 = 0;
    pub const INTERFACE: u32 = 3;
    pub const DISPATCH: u32 = 4;
    pub const COCLASS: u32 = 5;
}

/// The library's segments, by index in the segment directory.
mod segment {
    pub const TYPE_INFOS: usize = 0;
    pub const REFERENCES: usize = 3;
    pub const GUIDS: usize = 5;
    pub const NAMES: usize = 7;
    pub const STRINGS: usize = 8;
    pub const TYPE_DESCS: usize = 9;
}

/// A type as a function's return value or a parameter has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDesc {
    Base(u16),
    Pointer(Box<TypeDesc>),
    SafeArray(Box<TypeDesc>),
    Array,
    UserDefined(i32),
}

impl TypeDesc {
    /// Its variant kind (`VT_PTR` for a pointer, and so on).
    pub fn kind(&self) -> u16 {
        match self {
            TypeDesc::Base(base) => *base,
            TypeDesc::Pointer(_) => kind::PTR,
            TypeDesc::SafeArray(_) => kind::SAFEARRAY,
            TypeDesc::Array => kind::CARRAY,
            TypeDesc::UserDefined(_) => kind::USERDEFINED,
        }
    }
}

/// One parameter of a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub desc: TypeDesc,
    pub name: i32,
    pub flags: u32,
}

/// One function of a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub id: i32,
    pub name: i32,
    pub function_kind: u32,
    pub invoke_kind: u32,
    pub call_convention: u32,
    pub flags: u16,
    pub slot: u16,
    pub optional: u16,
    pub result: TypeDesc,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Segment {
    offset: usize,
    length: usize,
}

/// A type library's bytes and where its parts are.
#[derive(Debug, Clone)]
pub struct Library<'a> {
    bytes: &'a [u8],
    segments: [Segment; SEGMENTS],
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    u32_at(bytes, at).map(|value| value as i32)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn i16_at(bytes: &[u8], at: usize) -> Option<i16> {
    u16_at(bytes, at).map(|value| value as i16)
}

impl<'a> Library<'a> {
    /// The library in `bytes`, if they start with a well-formed `MSFT` header.
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if u32_at(bytes, 0)? != MAGIC {
            return None;
        }
        let count = u32_at(bytes, 0x20)? as usize;
        let help_dll = usize::from(u32_at(bytes, 0x14)? & HELP_DLL_FLAG != 0) * 4;
        let directory = HEADER_BYTES + help_dll + count.checked_mul(4)?;
        let mut segments = [Segment::default(); SEGMENTS];
        for (index, segment) in segments.iter_mut().enumerate() {
            let at = directory + index * SEGMENT_BYTES;
            let offset = i32_at(bytes, at)?;
            let length = i32_at(bytes, at + 4)?;
            if offset >= 0 && length > 0 {
                let (offset, length) = (offset as usize, length as usize);
                if offset.checked_add(length)? > bytes.len() {
                    return None;
                }
                *segment = Segment { offset, length };
            }
        }
        Some(Self { bytes, segments })
    }

    fn segment(&self, index: usize) -> Segment {
        self.segments[index]
    }

    /// The library's id.
    pub fn guid(&self) -> Option<[u8; 16]> {
        self.guid_at(i32_at(self.bytes, 8)?)
    }

    pub fn locale(&self) -> u32 {
        u32_at(self.bytes, 0x0c).unwrap_or(0)
    }

    /// `SYSKIND`: 0 Win16, 1 Win32, 2 Mac, 3 Win64.
    pub fn system(&self) -> u32 {
        u32_at(self.bytes, 0x14).unwrap_or(0) & 0xf
    }

    /// Major and minor version.
    pub fn version(&self) -> (u16, u16) {
        let version = u32_at(self.bytes, 0x18).unwrap_or(0);
        (version as u16, (version >> 16) as u16)
    }

    pub fn flags(&self) -> u32 {
        u32_at(self.bytes, 0x1c).unwrap_or(0)
    }

    pub fn name(&self) -> Option<&'a [u8]> {
        self.name_at(i32_at(self.bytes, 0x38)?)
    }

    fn pointer_bytes(&self) -> u16 {
        match self.system() {
            SYSTEM_64 => 8,
            _ => 4,
        }
    }

    /// How many types the library describes.
    pub fn type_count(&self) -> usize {
        u32_at(self.bytes, 0x20).map_or(0, |count| count as usize)
    }

    /// Type `index`.
    pub fn type_info(&self, index: usize) -> Option<TypeInfo<'_, 'a>> {
        let table = self.segment(segment::TYPE_INFOS);
        let at = index.checked_mul(TYPE_INFO_BYTES)?;
        (index < self.type_count() && at + TYPE_INFO_BYTES <= table.length).then_some(TypeInfo {
            library: self,
            base: table.offset + at,
        })
    }

    /// The index of the type with id `guid`.
    pub fn find(&self, guid: &[u8; 16]) -> Option<usize> {
        (0..self.type_count()).find(|&index| {
            self.type_info(index)
                .and_then(|found| found.guid())
                .is_some_and(|found| found == *guid)
        })
    }

    /// The index of the type a local `HREFTYPE` names; `None` for an imported one.
    pub fn type_of_reference(&self, reference: i32) -> Option<usize> {
        if reference < 0 || reference & 3 != 0 {
            return None;
        }
        let index = reference as usize / TYPE_INFO_BYTES;
        (index < self.type_count()).then_some(index)
    }

    /// The id at `offset` in the GUID table.
    pub fn guid_at(&self, offset: i32) -> Option<[u8; 16]> {
        let table = self.segment(segment::GUIDS);
        let offset = usize::try_from(offset).ok()?;
        if offset + GUID_ENTRY_BYTES > table.length {
            return None;
        }
        self.bytes
            .get(table.offset + offset..table.offset + offset + 16)?
            .try_into()
            .ok()
    }

    /// The name at `offset` in the name table, in its 8-bit encoding.
    pub fn name_at(&self, offset: i32) -> Option<&'a [u8]> {
        let table = self.segment(segment::NAMES);
        let offset = usize::try_from(offset).ok()?;
        let intro = table.offset + offset;
        let length = (u32_at(self.bytes, intro + 8)? & 0xff) as usize;
        let start = intro + NAME_INTRO_BYTES;
        (offset + NAME_INTRO_BYTES + length <= table.length)
            .then(|| self.bytes.get(start..start + length))
            .flatten()
    }

    /// The text at `offset` in the string table (help strings).
    pub fn string_at(&self, offset: i32) -> Option<&'a [u8]> {
        let table = self.segment(segment::STRINGS);
        let offset = usize::try_from(offset).ok()?;
        let start = table.offset + offset;
        let length = usize::from(u16_at(self.bytes, start)?);
        (offset + 2 + length <= table.length)
            .then(|| self.bytes.get(start + 2..start + 2 + length))
            .flatten()
    }

    fn desc_at(&self, offset: usize, depth: usize) -> Option<TypeDesc> {
        let table = self.segment(segment::TYPE_DESCS);
        if depth >= MOST_DESC_DEPTH || offset + TYPE_DESC_BYTES > table.length {
            return None;
        }
        let at = table.offset + offset;
        let base = u16_at(self.bytes, at)? & TYPE_MASK;
        let (third, fourth) = (i16_at(self.bytes, at + 4)?, i16_at(self.bytes, at + 6)?);
        let inner = |library: &Self| -> Option<Box<TypeDesc>> {
            Some(Box::new(match fourth < 0 {
                true => TypeDesc::Base(third as u16 & TYPE_MASK),
                false => library.desc_at(third as u16 as usize, depth + 1)?,
            }))
        };
        Some(match base {
            kind::PTR => TypeDesc::Pointer(inner(self)?),
            kind::SAFEARRAY => TypeDesc::SafeArray(inner(self)?),
            kind::CARRAY => TypeDesc::Array,
            kind::USERDEFINED => {
                TypeDesc::UserDefined((third as u16 as u32 | (fourth as u16 as u32) << 16) as i32)
            }
            other => TypeDesc::Base(other),
        })
    }

    /// The type a record's data-type word describes: a base kind when negative, else an
    /// offset in the type description table.
    pub fn type_desc(&self, data_type: i32) -> Option<TypeDesc> {
        match data_type < 0 {
            true => Some(TypeDesc::Base(data_type as u16 & TYPE_MASK)),
            false => self.desc_at(data_type as usize, 0),
        }
    }
}

/// One type of a library.
#[derive(Debug, Clone, Copy)]
pub struct TypeInfo<'l, 'a> {
    library: &'l Library<'a>,
    base: usize,
}

impl<'a> TypeInfo<'_, 'a> {
    fn word(&self, offset: usize) -> i32 {
        i32_at(self.library.bytes, self.base + offset).unwrap_or(0)
    }

    pub fn kind(&self) -> u32 {
        self.word(0) as u32 & 0xf
    }

    pub fn alignment(&self) -> u32 {
        (self.word(0) as u32 >> 11) & 0x1f
    }

    pub fn function_count(&self) -> usize {
        (self.word(0x18) as u32 & 0xffff) as usize
    }

    pub fn variable_count(&self) -> usize {
        (self.word(0x18) as u32 >> 16) as usize
    }

    pub fn guid(&self) -> Option<[u8; 16]> {
        self.library.guid_at(self.word(0x2c))
    }

    pub fn flags(&self) -> u32 {
        self.word(0x30) as u32
    }

    pub fn name(&self) -> Option<&'a [u8]> {
        self.library.name_at(self.word(0x34))
    }

    pub fn version(&self) -> (u16, u16) {
        let version = self.word(0x38) as u32;
        (version as u16, (version >> 16) as u16)
    }

    pub fn documentation(&self) -> Option<&'a [u8]> {
        self.library.string_at(self.word(0x3c))
    }

    pub fn implemented_count(&self) -> usize {
        (self.word(0x4c) as u32 & 0xffff) as usize
    }

    /// The vtable's size in slots, inherited ones included.
    pub fn vtable_slots(&self) -> usize {
        let bytes = (self.word(0x4c) as u32 >> 16) as usize;
        bytes / usize::from(self.library.pointer_bytes())
    }

    pub fn instance_size(&self) -> u32 {
        self.word(0x50) as u32
    }

    /// The `HREFTYPE` of implemented type `index`: an interface's base, or a coclass's
    /// interfaces from the reference table.
    pub fn implemented(&self, index: usize) -> Option<i32> {
        let first = self.word(0x54);
        if self.kind() != type_kind::COCLASS {
            return (index == 0 && first != -1).then_some(first);
        }
        let table = self.library.segment(segment::REFERENCES);
        let mut at = first;
        for _ in 0..index {
            at = i32_at(self.library.bytes, table.offset + usize::try_from(at).ok()? + 12)?;
        }
        let offset = usize::try_from(at).ok()?;
        (offset + 16 <= table.length)
            .then(|| i32_at(self.library.bytes, table.offset + offset))
            .flatten()
    }

    /// The flags of implemented type `index` of a coclass (`IMPLTYPEFLAGS`).
    pub fn implemented_flags(&self, index: usize) -> Option<u32> {
        if self.kind() != type_kind::COCLASS {
            return Some(0);
        }
        let table = self.library.segment(segment::REFERENCES);
        let mut at = self.word(0x54);
        for _ in 0..index {
            at = i32_at(self.library.bytes, table.offset + usize::try_from(at).ok()? + 12)?;
        }
        u32_at(self.library.bytes, table.offset + usize::try_from(at).ok()? + 4)
    }

    /// Function `index`, in the order the type lists them.
    pub fn function(&self, index: usize) -> Option<Function> {
        let count = self.function_count();
        let total = count + self.variable_count();
        if index >= count {
            return None;
        }
        let bytes = self.library.bytes;
        let block = usize::try_from(self.word(4)).ok()?;
        let records = i32_at(bytes, block)? as usize;
        let tail = block + 4 + records;
        let id = i32_at(bytes, tail + index * 4)?;
        let name = i32_at(bytes, tail + (total + index) * 4)?;
        let mut record_offset = 0usize;
        for _ in 0..index {
            let size = (u32_at(bytes, block + 4 + record_offset)? & 0xffff) as usize;
            if size == 0 {
                return None;
            }
            record_offset += size;
        }
        let record = block + 4 + record_offset;
        let size = (u32_at(bytes, record)? & 0xffff) as usize;
        if size < FUNCTION_FIXED_BYTES || record_offset + size > records {
            return None;
        }
        let data_type = i32_at(bytes, record + 4)?;
        let flags = u32_at(bytes, record + 8)? as u16;
        let vtable_offset = u16_at(bytes, record + 12)?;
        let kinds = u32_at(bytes, record + 16)?;
        let count = usize::from(u16_at(bytes, record + 20)?);
        let optional = u16_at(bytes, record + 22)?;
        let parameters_at = record + size.checked_sub(count * PARAMETER_BYTES)?;
        let needed = FUNCTION_FIXED_BYTES + count * PARAMETER_BYTES;
        if kinds & DEFAULT_VALUES != 0 && size < needed + count * 4 {
            return None;
        }
        let parameters = (0..count)
            .map(|at| {
                let at = parameters_at + at * PARAMETER_BYTES;
                Some(Parameter {
                    desc: self.library.type_desc(i32_at(bytes, at)?)?,
                    name: i32_at(bytes, at + 4)?,
                    flags: u32_at(bytes, at + 8)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Function {
            id,
            name,
            function_kind: kinds & 0x7,
            invoke_kind: (kinds >> 3) & 0xf,
            call_convention: (kinds >> 8) & 0xf,
            flags,
            slot: (vtable_offset & !1) / self.library.pointer_bytes(),
            optional,
            result: self.library.type_desc(data_type)?,
            parameters,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const LIBRARY_ID: [u8; 16] = [0x11; 16];
    pub const INTERFACE_ID: [u8; 16] = [0x22; 16];

    fn put(bytes: &mut Vec<u8>, at: usize, value: u32) {
        if bytes.len() < at + 4 {
            bytes.resize(at + 4, 0);
        }
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A small 64-bit library: one interface `IShape` with one function `Area` at slot 7,
    /// taking a `long` named `scale` and a `double*`.
    pub fn sample() -> Vec<u8> {
        let mut bytes = Vec::new();
        put(&mut bytes, 0, MAGIC);
        put(&mut bytes, 4, 0x0001_0002);
        put(&mut bytes, 8, 0);
        put(&mut bytes, 0x14, SYSTEM_64);
        put(&mut bytes, 0x18, 2 | 1 << 16);
        put(&mut bytes, 0x20, 1);
        put(&mut bytes, 0x38, 0);
        let directory = HEADER_BYTES + 4;
        let segment_at = |index: usize| directory + index * SEGMENT_BYTES;
        let data = directory + SEGMENTS * SEGMENT_BYTES;
        let type_infos = data;
        let guids = type_infos + TYPE_INFO_BYTES;
        let names = guids + 2 * GUID_ENTRY_BYTES;
        let descs = names + 3 * 20;
        let members = descs + TYPE_DESC_BYTES;
        for index in 0..SEGMENTS {
            put(&mut bytes, segment_at(index), u32::MAX);
            put(&mut bytes, segment_at(index) + 4, 0);
        }
        for (index, start, length) in [
            (segment::TYPE_INFOS, type_infos, TYPE_INFO_BYTES),
            (segment::GUIDS, guids, 2 * GUID_ENTRY_BYTES),
            (segment::NAMES, names, 60),
            (segment::TYPE_DESCS, descs, TYPE_DESC_BYTES),
        ] {
            put(&mut bytes, segment_at(index), start as u32);
            put(&mut bytes, segment_at(index) + 4, length as u32);
        }
        bytes.resize(members, 0);
        bytes[guids..guids + 16].copy_from_slice(&LIBRARY_ID);
        bytes[guids + 24..guids + 40].copy_from_slice(&INTERFACE_ID);
        for (index, name) in [&b"Shapes"[..], b"IShape", b"Area"].iter().enumerate() {
            let at = names + index * 20;
            put(&mut bytes, at + 8, name.len() as u32);
            bytes[at + 12..at + 12 + name.len()].copy_from_slice(name);
        }
        put(&mut bytes, descs, u32::from(kind::PTR));
        put(&mut bytes, descs + 4, 5 | 0xffff << 16);
        let base = type_infos;
        put(&mut bytes, base, type_kind::INTERFACE);
        put(&mut bytes, base + 4, members as u32);
        put(&mut bytes, base + 0x18, 1);
        put(&mut bytes, base + 0x2c, 24);
        put(&mut bytes, base + 0x34, 20);
        put(&mut bytes, base + 0x4c, 1 | (8 * 8) << 16);
        put(&mut bytes, base + 0x54, u32::MAX);
        let record_size = FUNCTION_FIXED_BYTES + 2 * PARAMETER_BYTES;
        put(&mut bytes, members, record_size as u32);
        let record = members + 4;
        put(&mut bytes, record, record_size as u32);
        put(&mut bytes, record + 4, 0x8000_0000 | 25);
        put(&mut bytes, record + 12, 7 * 8);
        put(&mut bytes, record + 16, 1 << 3 | 4 << 8);
        put(&mut bytes, record + 20, 2);
        let parameters = record + FUNCTION_FIXED_BYTES;
        put(&mut bytes, parameters, 0x8000_0000 | 3);
        put(&mut bytes, parameters + 4, 40);
        put(&mut bytes, parameters + 12, 0);
        put(&mut bytes, parameters + 16, u32::MAX);
        put(&mut bytes, parameters + 20, 2);
        let tail = record + record_size;
        put(&mut bytes, tail, 0x6002_0000);
        put(&mut bytes, tail + 4, 40);
        put(&mut bytes, tail + 8, 0);
        bytes
    }

    #[test]
    fn the_header_and_the_type_read_back() {
        let bytes = sample();
        let library = Library::parse(&bytes).unwrap();
        assert_eq!(library.guid(), Some(LIBRARY_ID));
        assert_eq!(library.version(), (2, 1));
        assert_eq!(library.name(), Some(&b"Shapes"[..]));
        assert_eq!(library.find(&INTERFACE_ID), Some(0));
        let shape = library.type_info(0).unwrap();
        assert_eq!(shape.kind(), type_kind::INTERFACE);
        assert_eq!(shape.name(), Some(&b"IShape"[..]));
        assert_eq!(shape.vtable_slots(), 8);
        assert_eq!(shape.implemented(0), None);
    }

    #[test]
    fn functions_read_their_slot_and_parameters() {
        let bytes = sample();
        let library = Library::parse(&bytes).unwrap();
        let area = library.type_info(0).unwrap().function(0).unwrap();
        assert_eq!(area.id, 0x6002_0000);
        assert_eq!(library.name_at(area.name), Some(&b"Area"[..]));
        assert_eq!(area.slot, 7);
        assert_eq!(area.invoke_kind, 1);
        assert_eq!(area.call_convention, 4);
        assert_eq!(area.result, TypeDesc::Base(25));
        assert_eq!(area.parameters.len(), 2);
        assert_eq!(area.parameters[0].desc, TypeDesc::Base(3));
        assert_eq!(
            area.parameters[1].desc,
            TypeDesc::Pointer(Box::new(TypeDesc::Base(5)))
        );
        assert_eq!(area.parameters[1].flags, 2);
    }

    #[test]
    fn other_bytes_are_refused() {
        assert!(Library::parse(b"MZ\x90\x00").is_none());
        let mut bytes = sample();
        bytes.truncate(HEADER_BYTES + 20);
        assert!(Library::parse(&bytes).is_none());
    }
}
