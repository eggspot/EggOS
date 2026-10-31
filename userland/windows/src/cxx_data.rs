//! The data MSVC writes into a program for C++ exceptions (D177), read from the image: a
//! function's `FuncInfo`, which `__CxxFrameHandler3` reads (fixed-size records), or the
//! compressed `FuncInfo4` of `__CxxFrameHandler4`, the default for x64 since Visual Studio
//! 2019; and a thrown object's `ThrowInfo` with its catchable types. Both kinds of function
//! data answer the same questions: the state of a program counter, each state's unwind action
//! and the state it falls back to, the try blocks and their catch handlers. Pure logic with
//! host tests; `win32/exception/cxx.rs` runs the handlers.

use crate::unwind::Image;

/// The exception code of a C++ `throw` (`'msc'` | 0xE0000000).
pub const CXX_EXCEPTION: u32 = 0xe06d_7363;
/// The first parameter of a C++ exception's record.
pub const CXX_MAGIC: u64 = 0x1993_0520;
const FUNC_INFO_MAGIC_FIRST: u32 = 0x1993_0520;
const FUNC_INFO_MAGIC_LAST: u32 = 0x1993_0522;
const FUNC_INFO_MAGIC_WITH_FLAGS: u32 = 0x1993_0522;
const MAGIC_BITS: u32 = 0x1fff_ffff;
const EHS_FLAG: u32 = 1;
const NOEXCEPT_FLAG: u32 = 4;

mod v4 {
    pub const IS_CATCH: u8 = 0x01;
    pub const IS_SEPARATED: u8 = 0x02;
    pub const BBT: u8 = 0x04;
    pub const UNWIND_MAP: u8 = 0x08;
    pub const TRY_BLOCK_MAP: u8 = 0x10;
    pub const EHS: u8 = 0x20;
    pub const NO_EXCEPT: u8 = 0x40;
    pub const HANDLER_ADJECTIVES: u8 = 0x01;
    pub const HANDLER_TYPE: u8 = 0x02;
    pub const HANDLER_CATCH_OBJECT: u8 = 0x04;
    pub const HANDLER_CONTINUATION_IS_RVA: u8 = 0x08;
    pub const HANDLER_CONTINUATIONS: u8 = 0x30;
    pub const UNWIND_NONE: u32 = 0;
    pub const UNWIND_DESTRUCTOR: u32 = 1;
    pub const UNWIND_DESTRUCTOR_THROUGH_POINTER: u32 = 2;
}

/// The adjectives of a catch handler (`HandlerType`) and the properties of a catchable type
/// and a thrown type, as `ehdata.h` names them.
pub mod flags {
    pub const HANDLER_CONST: u32 = 0x01;
    pub const HANDLER_VOLATILE: u32 = 0x02;
    pub const HANDLER_UNALIGNED: u32 = 0x04;
    pub const HANDLER_REFERENCE: u32 = 0x08;
    pub const HANDLER_STD_DOT_DOT: u32 = 0x40;
    pub const THROW_CONST: u32 = 0x01;
    pub const THROW_VOLATILE: u32 = 0x02;
    pub const THROW_UNALIGNED: u32 = 0x04;
    pub const CATCHABLE_SIMPLE_TYPE: u32 = 0x01;
    pub const CATCHABLE_BY_REFERENCE_ONLY: u32 = 0x02;
    pub const CATCHABLE_VIRTUAL_BASE: u32 = 0x04;
}

const TYPE_NAME_OFFSET: u64 = 16;
const MAX_ENTRIES: u32 = 1 << 16;

/// What unwinding a state does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Calls the unwind funclet at this address with the frame.
    Funclet(u64),
    /// Calls the destructor at `function` on the object at the frame plus `object`, or on
    /// the object whose address is stored there.
    Destructor {
        function: u64,
        object: u64,
        through_pointer: bool,
    },
}

/// A state's unwind action and the state the function falls back to after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnwindEntry {
    pub to_state: i32,
    pub action: Action,
}

/// A try block: the states its body covers and its catch handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryBlock {
    pub low: i32,
    pub high: i32,
    pub handler_count: u32,
    handlers: u64,
}

/// One catch handler: what it catches (no type descriptor for `catch (...)`), where its
/// catch object lives relative to the frame (0 for none), its funclet and, for
/// `__CxxFrameHandler4`, the continuation addresses its funclet may name by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handler {
    pub adjectives: u32,
    pub type_descriptor: u64,
    pub catch_object: u64,
    pub funclet: u64,
    pub continuations: [u64; 2],
    pub continuation_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fixed {
        max_state: u32,
        unwind_map: u64,
        try_count: u32,
        try_map: u64,
        ip_count: u32,
        ip_map: u64,
        flags: u32,
    },
    Compressed {
        header: u8,
        unwind_map: Option<u64>,
        try_map: Option<u64>,
        ip_map: Option<u64>,
    },
}

/// A function's C++ exception data.
#[derive(Debug, Clone, Copy)]
pub struct FunctionData<'a> {
    image: Image<'a>,
    kind: Kind,
}

struct Reader<'a> {
    image: Image<'a>,
    at: u64,
}

impl Reader<'_> {
    fn unsigned(&mut self) -> Option<u32> {
        let first = self.image.u8_at(self.at)?;
        let length = u64::from(u32::from(first & 0x0f).trailing_ones().min(4)) + 1;
        let value = match length {
            5 => self.image.u32_at(self.at + 1)?,
            _ => {
                let mut value = 0u32;
                for index in (0..length).rev() {
                    value = value << 8 | u32::from(self.image.u8_at(self.at + index)?);
                }
                value >> length
            }
        };
        self.at += length;
        Some(value)
    }

    fn int(&mut self) -> Option<i32> {
        let value = self.image.u32_at(self.at)? as i32;
        self.at += 4;
        Some(value)
    }

    fn rva(&mut self) -> Option<u64> {
        self.int().map(|value| u64::from(value as u32))
    }
}

fn address(image: &Image<'_>, rva: u64) -> u64 {
    match rva {
        0 => 0,
        _ => image.base + rva,
    }
}

impl<'a> FunctionData<'a> {
    /// `__CxxFrameHandler3`'s `FuncInfo` at `rva`, if its magic number is one MSVC writes.
    pub fn fixed(image: Image<'a>, rva: u64) -> Option<Self> {
        let word = |offset: u64| image.u32_at(rva + offset);
        let magic = word(0)? & MAGIC_BITS;
        if !(FUNC_INFO_MAGIC_FIRST..=FUNC_INFO_MAGIC_LAST).contains(&magic) {
            return None;
        }
        let flags = match magic >= FUNC_INFO_MAGIC_WITH_FLAGS {
            true => word(36)?,
            false => 0,
        };
        Some(Self {
            image,
            kind: Kind::Fixed {
                max_state: word(4)?.min(MAX_ENTRIES),
                unwind_map: u64::from(word(8)?),
                try_count: word(12)?.min(MAX_ENTRIES),
                try_map: u64::from(word(16)?),
                ip_count: word(20)?.min(MAX_ENTRIES),
                ip_map: u64::from(word(24)?),
                flags,
            },
        })
    }

    /// `__CxxFrameHandler4`'s compressed `FuncInfo4` at `rva`, for the function (or funclet)
    /// whose code starts at `function_begin`.
    pub fn compressed(image: Image<'a>, rva: u64, function_begin: u32) -> Option<Self> {
        let mut reader = Reader { image, at: rva };
        let header = image.u8_at(rva)?;
        reader.at += 1;
        if header & v4::BBT != 0 {
            reader.unsigned()?;
        }
        let unwind_map = match header & v4::UNWIND_MAP != 0 {
            true => Some(reader.rva()?),
            false => None,
        };
        let try_map = match header & v4::TRY_BLOCK_MAP != 0 {
            true => Some(reader.rva()?),
            false => None,
        };
        let mut ip_map = Some(reader.rva()?);
        if header & v4::IS_SEPARATED != 0 {
            ip_map = ip_map.and_then(|table| separated_map(image, table, function_begin));
        }
        Some(Self {
            image,
            kind: Kind::Compressed {
                header,
                unwind_map,
                try_map,
                ip_map,
            },
        })
    }

    /// Whether the function is a catch funclet (`__CxxFrameHandler4` marks them).
    pub fn is_catch_funclet(&self) -> bool {
        matches!(self.kind, Kind::Compressed { header, .. } if header & v4::IS_CATCH != 0)
    }

    /// Whether the function was compiled with `/EHs`, so only C++ exceptions reach its catch
    /// handlers, `catch (...)` included.
    pub fn synchronous_only(&self) -> bool {
        match self.kind {
            Kind::Fixed { flags, .. } => flags & EHS_FLAG != 0,
            Kind::Compressed { header, .. } => header & v4::EHS != 0,
        }
    }

    /// Whether the function is `noexcept`: an exception may not leave it.
    pub fn is_noexcept(&self) -> bool {
        match self.kind {
            Kind::Fixed { flags, .. } => flags & NOEXCEPT_FLAG != 0,
            Kind::Compressed { header, .. } => header & v4::NO_EXCEPT != 0,
        }
    }

    /// The state at the image-relative `pc` in the function or funclet that starts at
    /// `function_begin`: the last entry of the IP-to-state map at or before it, or -1.
    pub fn state_at(&self, pc: u64, function_begin: u32) -> i32 {
        let mut state = -1;
        match self.kind {
            Kind::Fixed {
                ip_count, ip_map, ..
            } => {
                for index in 0..u64::from(ip_count) {
                    let entry = ip_map + index * 8;
                    let (Some(ip), Some(entry_state)) =
                        (self.image.u32_at(entry), self.image.u32_at(entry + 4))
                    else {
                        break;
                    };
                    if u64::from(ip) > pc {
                        break;
                    }
                    state = entry_state as i32;
                }
            }
            Kind::Compressed {
                ip_map: Some(ip_map),
                ..
            } => {
                let mut reader = Reader {
                    image: self.image,
                    at: ip_map,
                };
                let count = reader.unsigned().unwrap_or(0).min(MAX_ENTRIES);
                let mut ip = u64::from(function_begin);
                for _ in 0..count {
                    let (Some(delta), Some(encoded)) = (reader.unsigned(), reader.unsigned())
                    else {
                        break;
                    };
                    ip += u64::from(delta);
                    if ip > pc {
                        break;
                    }
                    state = encoded as i32 - 1;
                }
            }
            Kind::Compressed { ip_map: None, .. } => {}
        }
        state
    }

    /// The unwind action of `state` and the state after it.
    pub fn unwind_entry(&self, state: i32) -> Option<UnwindEntry> {
        let index = u32::try_from(state).ok()?;
        match self.kind {
            Kind::Fixed {
                max_state,
                unwind_map,
                ..
            } => {
                if index >= max_state {
                    return None;
                }
                let entry = unwind_map + u64::from(index) * 8;
                let to_state = self.image.u32_at(entry)? as i32;
                let funclet = self.image.u32_at(entry + 4)?;
                Some(UnwindEntry {
                    to_state,
                    action: match funclet {
                        0 => Action::Nothing,
                        rva => Action::Funclet(self.image.base + u64::from(rva)),
                    },
                })
            }
            Kind::Compressed {
                unwind_map: Some(map),
                ..
            } => {
                let mut reader = Reader {
                    image: self.image,
                    at: map,
                };
                let count = reader.unsigned()?.min(MAX_ENTRIES);
                if index >= count {
                    return None;
                }
                let first_entry = reader.at;
                let mut entry = None;
                for current in 0..=index {
                    let start = reader.at;
                    let decoded = self.compressed_unwind_entry(&mut reader)?;
                    if current == index {
                        entry = Some((start, decoded));
                    }
                }
                let (start, (next_offset, action)) = entry?;
                let to_state = match next_offset {
                    0 => -1,
                    offset => match start.checked_sub(u64::from(offset)) {
                        Some(target) if target >= first_entry => {
                            self.compressed_state_at_offset(first_entry, target, count)
                        }
                        _ => -1,
                    },
                };
                Some(UnwindEntry { to_state, action })
            }
            Kind::Compressed {
                unwind_map: None, ..
            } => None,
        }
    }

    fn compressed_unwind_entry(&self, reader: &mut Reader<'_>) -> Option<(u32, Action)> {
        let next_and_type = reader.unsigned()?;
        let kind = next_and_type & 3;
        let action = match kind {
            v4::UNWIND_NONE => Action::Nothing,
            v4::UNWIND_DESTRUCTOR | v4::UNWIND_DESTRUCTOR_THROUGH_POINTER => {
                let function = reader.rva()?;
                let object = u64::from(reader.unsigned()?);
                Action::Destructor {
                    function: address(&self.image, function),
                    object,
                    through_pointer: kind == v4::UNWIND_DESTRUCTOR_THROUGH_POINTER,
                }
            }
            _ => Action::Funclet(address(&self.image, reader.rva()?)),
        };
        Some((next_and_type >> 2, action))
    }

    fn compressed_state_at_offset(&self, first_entry: u64, target: u64, count: u32) -> i32 {
        let mut reader = Reader {
            image: self.image,
            at: first_entry,
        };
        for state in 0..count {
            if reader.at == target {
                return state as i32;
            }
            if reader.at > target || self.compressed_unwind_entry(&mut reader).is_none() {
                break;
            }
        }
        -1
    }

    /// The number of try blocks, innermost first.
    pub fn try_count(&self) -> u32 {
        match self.kind {
            Kind::Fixed { try_count, .. } => try_count,
            Kind::Compressed {
                try_map: Some(map), ..
            } => Reader {
                image: self.image,
                at: map,
            }
            .unsigned()
            .unwrap_or(0)
            .min(MAX_ENTRIES),
            Kind::Compressed { try_map: None, .. } => 0,
        }
    }

    /// Try block `index`.
    pub fn try_block(&self, index: u32) -> Option<TryBlock> {
        match self.kind {
            Kind::Fixed {
                try_count, try_map, ..
            } => {
                if index >= try_count {
                    return None;
                }
                let entry = try_map + u64::from(index) * 20;
                let word = |offset: u64| self.image.u32_at(entry + offset);
                Some(TryBlock {
                    low: word(0)? as i32,
                    high: word(4)? as i32,
                    handler_count: word(12)?.min(MAX_ENTRIES),
                    handlers: u64::from(word(16)?),
                })
            }
            Kind::Compressed {
                try_map: Some(map), ..
            } => {
                let mut reader = Reader {
                    image: self.image,
                    at: map,
                };
                let count = reader.unsigned()?;
                if index >= count {
                    return None;
                }
                let mut block = None;
                for _ in 0..=index {
                    let low = reader.unsigned()? as i32;
                    let high = reader.unsigned()? as i32;
                    reader.unsigned()?;
                    let handlers = reader.rva()?;
                    block = Some((low, high, handlers));
                }
                let (low, high, handlers) = block?;
                let handler_count = Reader {
                    image: self.image,
                    at: handlers,
                }
                .unsigned()?
                .min(MAX_ENTRIES);
                Some(TryBlock {
                    low,
                    high,
                    handler_count,
                    handlers,
                })
            }
            Kind::Compressed { try_map: None, .. } => None,
        }
    }

    /// Catch handler `index` of `block`; `function_begin` is the start of the function whose
    /// continuation offsets `__CxxFrameHandler4` may give.
    pub fn handler(&self, block: &TryBlock, index: u32, function_begin: u32) -> Option<Handler> {
        if index >= block.handler_count {
            return None;
        }
        match self.kind {
            Kind::Fixed { .. } => {
                let entry = block.handlers + u64::from(index) * 20;
                let word = |offset: u64| self.image.u32_at(entry + offset);
                Some(Handler {
                    adjectives: word(0)?,
                    type_descriptor: address(&self.image, u64::from(word(4)?)),
                    catch_object: u64::from(word(8)?),
                    funclet: address(&self.image, u64::from(word(12)?)),
                    continuations: [0; 2],
                    continuation_count: 0,
                })
            }
            Kind::Compressed { .. } => {
                let mut reader = Reader {
                    image: self.image,
                    at: block.handlers,
                };
                reader.unsigned()?;
                let mut handler = None;
                for _ in 0..=index {
                    handler = Some(self.compressed_handler(&mut reader, function_begin)?);
                }
                handler
            }
        }
    }

    fn compressed_handler(&self, reader: &mut Reader<'_>, function_begin: u32) -> Option<Handler> {
        let header = self.image.u8_at(reader.at)?;
        reader.at += 1;
        let adjectives = match header & v4::HANDLER_ADJECTIVES != 0 {
            true => reader.unsigned()?,
            false => 0,
        };
        let type_descriptor = match header & v4::HANDLER_TYPE != 0 {
            true => address(&self.image, reader.rva()?),
            false => 0,
        };
        let catch_object = match header & v4::HANDLER_CATCH_OBJECT != 0 {
            true => u64::from(reader.unsigned()?),
            false => 0,
        };
        let funclet = address(&self.image, reader.rva()?);
        let continuation_count = usize::from((header & v4::HANDLER_CONTINUATIONS) >> 4).min(2);
        let mut continuations = [0u64; 2];
        for continuation in continuations.iter_mut().take(continuation_count) {
            *continuation = match header & v4::HANDLER_CONTINUATION_IS_RVA != 0 {
                true => address(&self.image, reader.rva()?),
                false => {
                    self.image.base + u64::from(function_begin) + u64::from(reader.unsigned()?)
                }
            };
        }
        Some(Handler {
            adjectives,
            type_descriptor,
            catch_object,
            funclet,
            continuations,
            continuation_count,
        })
    }
}

fn separated_map(image: Image<'_>, table: u64, function_begin: u32) -> Option<u64> {
    let mut reader = Reader { image, at: table };
    let count = reader.unsigned()?.min(MAX_ENTRIES);
    for _ in 0..count {
        let segment = reader.rva()?;
        let map = reader.rva()?;
        if segment == u64::from(function_begin) {
            return Some(map);
        }
    }
    None
}

/// A thrown object's `ThrowInfo`: its attributes, its destructor and its catchable types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrowInfo {
    pub attributes: u32,
    pub destructor: u64,
    pub catchable_count: u32,
    catchable_array: u64,
}

/// One type a thrown object can be caught as: its type descriptor, how to find that part of
/// the object, its size and its copy constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatchableType {
    pub properties: u32,
    pub type_descriptor: u64,
    pub member_displacement: i32,
    pub virtual_base_pointer: i32,
    pub virtual_base_displacement: i32,
    pub size: u32,
    pub copy_function: u64,
}

/// The `ThrowInfo` at `address` in `image`.
pub fn throw_info(image: &Image<'_>, address: u64) -> Option<ThrowInfo> {
    let rva = image.rva_of(address)?;
    let word = |offset: u64| image.u32_at(rva + offset);
    let catchable_array = u64::from(word(12)?);
    Some(ThrowInfo {
        attributes: word(0)?,
        destructor: match word(4)? {
            0 => 0,
            destructor => image.base + u64::from(destructor),
        },
        catchable_count: image.u32_at(catchable_array)?.min(MAX_ENTRIES),
        catchable_array,
    })
}

/// Catchable type `index` of `info`, most derived first.
pub fn catchable_type(image: &Image<'_>, info: &ThrowInfo, index: u32) -> Option<CatchableType> {
    if index >= info.catchable_count {
        return None;
    }
    let rva = u64::from(image.u32_at(info.catchable_array + 4 + u64::from(index) * 4)?);
    let word = |offset: u64| image.u32_at(rva + offset);
    Some(CatchableType {
        properties: word(0)?,
        type_descriptor: address(image, u64::from(word(4)?)),
        member_displacement: word(8)? as i32,
        virtual_base_pointer: word(12)? as i32,
        virtual_base_displacement: word(16)? as i32,
        size: word(20)?,
        copy_function: address(image, u64::from(word(24)?)),
    })
}

/// The decorated name of the type descriptor at `address` (`.H` for `int`,
/// `.?AVBase@@` for a class), without its terminating NUL.
pub fn type_name<'a>(image: &Image<'a>, address: u64) -> &'a [u8] {
    let Some(start) = image.rva_of(address).map(|rva| rva + TYPE_NAME_OFFSET) else {
        return &[];
    };
    let bytes = image.bytes.get(start as usize..).unwrap_or(&[]);
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    &bytes[..length]
}

/// Whether `handler` catches an object thrown with `throw_attributes` as `catchable`: a
/// `catch (...)` catches every C++ object; otherwise the types must be the same (the same
/// descriptor, or descriptors with the same decorated name), a type that may only be caught by
/// reference needs a reference handler, and a handler drops no `const`, `volatile` or
/// `__unaligned` the thrown pointer has.
pub fn handler_catches(
    image: &Image<'_>,
    handler: &Handler,
    catchable: &CatchableType,
    throw_attributes: u32,
) -> bool {
    if handler.type_descriptor == 0 || handler.adjectives & flags::HANDLER_STD_DOT_DOT != 0 {
        return true;
    }
    let same = handler.type_descriptor == catchable.type_descriptor || {
        let wanted = type_name(image, handler.type_descriptor);
        !wanted.is_empty() && wanted == type_name(image, catchable.type_descriptor)
    };
    if !same {
        return false;
    }
    if catchable.properties & flags::CATCHABLE_BY_REFERENCE_ONLY != 0
        && handler.adjectives & flags::HANDLER_REFERENCE == 0
    {
        return false;
    }
    let dropped =
        |thrown: u32, kept: u32| throw_attributes & thrown != 0 && handler.adjectives & kept == 0;
    !(dropped(flags::THROW_CONST, flags::HANDLER_CONST)
        || dropped(flags::THROW_VOLATILE, flags::HANDLER_VOLATILE)
        || dropped(flags::THROW_UNALIGNED, flags::HANDLER_UNALIGNED))
}

/// Whether a catchable type is a pointer, whose value is adjusted to the base it is caught
/// as rather than copied as it is (its decorated name starts with `.P`).
pub fn is_pointer_type(image: &Image<'_>, catchable: &CatchableType) -> bool {
    type_name(image, catchable.type_descriptor).starts_with(b".P")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x1_4000_0000;

    fn image(bytes: &[u8]) -> Image<'_> {
        Image { base: BASE, bytes }
    }

    fn put(bytes: &mut [u8], at: usize, words: &[u32]) {
        for (index, word) in words.iter().enumerate() {
            bytes[at + index * 4..at + index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }

    #[test]
    fn compressed_numbers_take_one_to_five_bytes() {
        let bytes = [
            0x0a, 0xb1, 0x04, 0x0f, 0x78, 0x56, 0x34, 0x12, 0x83, 0x8b, 0x08,
        ];
        let mut reader = Reader {
            image: image(&bytes),
            at: 0,
        };
        assert_eq!(reader.unsigned(), Some(5));
        assert_eq!(reader.unsigned(), Some(300));
        assert_eq!(reader.unsigned(), Some(0x1234_5678));
        assert_eq!(reader.unsigned(), Some(70_000));
        assert_eq!(reader.at, bytes.len() as u64);
        assert_eq!(reader.unsigned(), None);
    }

    fn fixed_function() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        put(
            &mut bytes,
            0x100,
            &[0x1993_0522, 3, 0x200, 1, 0x240, 3, 0x280, 0, 0, EHS_FLAG],
        );
        put(&mut bytes, 0x200, &[u32::MAX, 0, 0, 0x1500, 1, 0]);
        put(&mut bytes, 0x240, &[1, 1, 2, 1, 0x260]);
        put(
            &mut bytes,
            0x260,
            &[flags::HANDLER_REFERENCE, 0x300, 0x28, 0x1600, 0x38],
        );
        put(&mut bytes, 0x280, &[0x1000, u32::MAX, 0x1010, 0, 0x1020, 1]);
        bytes
    }

    #[test]
    fn fixed_data_gives_states_unwind_actions_and_handlers() {
        let bytes = fixed_function();
        let data = FunctionData::fixed(image(&bytes), 0x100).unwrap();
        assert!(data.synchronous_only());
        assert!(!data.is_noexcept());
        assert!(!data.is_catch_funclet());
        assert_eq!(data.state_at(0x0fff, 0x1000), -1);
        assert_eq!(data.state_at(0x1000, 0x1000), -1);
        assert_eq!(data.state_at(0x1014, 0x1000), 0);
        assert_eq!(data.state_at(0x1100, 0x1000), 1);
        assert_eq!(
            data.unwind_entry(1),
            Some(UnwindEntry {
                to_state: 0,
                action: Action::Funclet(BASE + 0x1500)
            })
        );
        assert_eq!(
            data.unwind_entry(0),
            Some(UnwindEntry {
                to_state: -1,
                action: Action::Nothing
            })
        );
        assert_eq!(data.unwind_entry(3), None);
        assert_eq!(data.unwind_entry(-1), None);
        assert_eq!(data.try_count(), 1);
        let block = data.try_block(0).unwrap();
        assert_eq!((block.low, block.high), (1, 1));
        let handler = data.handler(&block, 0, 0x1000).unwrap();
        assert_eq!(handler.type_descriptor, BASE + 0x300);
        assert_eq!(handler.catch_object, 0x28);
        assert_eq!(handler.funclet, BASE + 0x1600);
        assert_eq!(data.handler(&block, 1, 0x1000), None);
        assert_eq!(data.try_block(1), None);
        let mut wrong = bytes.clone();
        put(&mut wrong, 0x100, &[0x1234]);
        assert!(FunctionData::fixed(image(&wrong), 0x100).is_none());
    }

    fn compressed_function() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        let header = v4::UNWIND_MAP | v4::TRY_BLOCK_MAP | v4::EHS;
        bytes[0x100] = header;
        put(&mut bytes[0x101..], 0, &[0x200, 0x240, 0x280]);
        bytes[0x200..0x20c].copy_from_slice(&[
            3 << 1,
            0,
            (1 << 2 | v4::UNWIND_DESTRUCTOR as u8) << 1,
            0x00,
            0x17,
            0,
            0,
            0x40 << 1,
            ((6 << 2) | 3) << 1,
            0x00,
            0x18,
            0,
        ]);
        bytes[0x20c] = 0;
        bytes[0x240..0x244].copy_from_slice(&[1 << 1, 1 << 1, 1 << 1, 2 << 1]);
        put(&mut bytes[0x244..], 0, &[0x2c0]);
        bytes[0x280..0x287].copy_from_slice(&[3 << 1, 0, 0, 4 << 1, 2 << 1, 8 << 1, 3 << 1]);
        bytes[0x2c0] = 1 << 1;
        bytes[0x2c1] = v4::HANDLER_TYPE | v4::HANDLER_CATCH_OBJECT | 0x10;
        put(&mut bytes[0x2c2..], 0, &[0x300]);
        bytes[0x2c6] = 0x30 << 1;
        put(&mut bytes[0x2c7..], 0, &[0x1700]);
        bytes[0x2cb] = 0x50 << 1;
        bytes
    }

    #[test]
    fn compressed_data_gives_the_same_answers() {
        let bytes = compressed_function();
        let data = FunctionData::compressed(image(&bytes), 0x100, 0x1000).unwrap();
        assert!(data.synchronous_only());
        assert!(!data.is_catch_funclet());
        assert_eq!(data.state_at(0x1000, 0x1000), -1);
        assert_eq!(data.state_at(0x1004, 0x1000), 1);
        assert_eq!(data.state_at(0x1009, 0x1000), 1);
        assert_eq!(data.state_at(0x100c, 0x1000), 2);
        assert_eq!(
            data.unwind_entry(0),
            Some(UnwindEntry {
                to_state: -1,
                action: Action::Nothing
            })
        );
        assert_eq!(
            data.unwind_entry(1),
            Some(UnwindEntry {
                to_state: 0,
                action: Action::Destructor {
                    function: BASE + 0x1700,
                    object: 0x40,
                    through_pointer: false
                }
            })
        );
        assert_eq!(
            data.unwind_entry(2).map(|entry| entry.action),
            Some(Action::Funclet(BASE + 0x1800))
        );
        assert_eq!(data.unwind_entry(2).unwrap().to_state, 1);
        assert_eq!(data.try_count(), 1);
        let block = data.try_block(0).unwrap();
        assert_eq!((block.low, block.high), (1, 1));
        assert_eq!(block.handler_count, 1);
        let handler = data.handler(&block, 0, 0x1000).unwrap();
        assert_eq!(handler.type_descriptor, BASE + 0x300);
        assert_eq!(handler.catch_object, 0x30);
        assert_eq!(handler.funclet, BASE + 0x1700);
        assert_eq!(handler.continuation_count, 1);
        assert_eq!(handler.continuations[0], BASE + 0x1000 + 0x50);
    }

    #[test]
    fn thrown_types_are_matched_by_name_reference_and_qualifiers() {
        let mut bytes = vec![0u8; 0x400];
        put(&mut bytes, 0x100, &[flags::THROW_CONST, 0x1900, 0, 0x120]);
        put(&mut bytes, 0x120, &[2, 0x140, 0x160]);
        put(&mut bytes, 0x140, &[0, 0x200, 0, u32::MAX, 0, 16, 0x1a00]);
        put(
            &mut bytes,
            0x160,
            &[
                flags::CATCHABLE_BY_REFERENCE_ONLY,
                0x240,
                8,
                u32::MAX,
                0,
                16,
                0,
            ],
        );
        bytes[0x210..0x21a].copy_from_slice(b".?AVDer@@\0");
        bytes[0x250..0x25a].copy_from_slice(b".?AVBas@@\0");
        bytes[0x290..0x29a].copy_from_slice(b".?AVBas@@\0");
        let image = image(&bytes);
        let info = throw_info(&image, BASE + 0x100).unwrap();
        assert_eq!(info.destructor, BASE + 0x1900);
        assert_eq!(info.catchable_count, 2);
        let derived = catchable_type(&image, &info, 0).unwrap();
        let base = catchable_type(&image, &info, 1).unwrap();
        assert_eq!(derived.copy_function, BASE + 0x1a00);
        assert_eq!(base.member_displacement, 8);
        assert_eq!(catchable_type(&image, &info, 2), None);
        assert_eq!(type_name(&image, BASE + 0x200), b".?AVDer@@");
        let by_value = Handler {
            adjectives: flags::HANDLER_CONST,
            type_descriptor: BASE + 0x280,
            catch_object: 0,
            funclet: 0,
            continuations: [0; 2],
            continuation_count: 0,
        };
        assert!(!handler_catches(
            &image,
            &by_value,
            &derived,
            info.attributes
        ));
        assert!(!handler_catches(&image, &by_value, &base, info.attributes));
        let by_reference = Handler {
            adjectives: flags::HANDLER_CONST | flags::HANDLER_REFERENCE,
            ..by_value
        };
        assert!(handler_catches(
            &image,
            &by_reference,
            &base,
            info.attributes
        ));
        let not_const = Handler {
            adjectives: flags::HANDLER_REFERENCE,
            ..by_value
        };
        assert!(!handler_catches(&image, &not_const, &base, info.attributes));
        let everything = Handler {
            type_descriptor: 0,
            ..not_const
        };
        assert!(handler_catches(
            &image,
            &everything,
            &derived,
            info.attributes
        ));
        assert!(!is_pointer_type(&image, &derived));
    }
}
