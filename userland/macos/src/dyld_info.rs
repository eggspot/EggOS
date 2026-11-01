//! The fixups of programs linked without chained fixups (`LC_DYLD_INFO` and
//! `LC_DYLD_INFO_ONLY`, D190): byte-coded rebase and bind opcodes, as `mach-o/loader.h`
//! documents them, that name each pointer to slide by the image's slide or to bind to an
//! imported symbol, by a segment's load-command index and an offset in it. The lazy-bind
//! opcodes are a run of entries, each ended by `DONE`, that the layer binds when it loads the
//! program instead of on the first call. Every number is checked, and a table names at most
//! as many pointers as a 1 GiB image holds, so a malformed table ends instead of looping.

use crate::fixups::Source;
use crate::macho::{InvalidMachO, MAX_IMAGE_SIZE, Range};

const OPCODE: u8 = 0xf0;
const IMMEDIATE: u8 = 0x0f;
const POINTER_SIZE: u64 = 8;
const POINTER_TYPE: u8 = 1;
const MAX_POINTERS: u64 = MAX_IMAGE_SIZE / POINTER_SIZE;
const WEAK_IMPORT: u8 = 1;

mod rebase {
    pub const DONE: u8 = 0x00;
    pub const SET_TYPE_IMM: u8 = 0x10;
    pub const SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x20;
    pub const ADD_ADDR_ULEB: u8 = 0x30;
    pub const ADD_ADDR_IMM_SCALED: u8 = 0x40;
    pub const DO_REBASE_IMM_TIMES: u8 = 0x50;
    pub const DO_REBASE_ULEB_TIMES: u8 = 0x60;
    pub const DO_REBASE_ADD_ADDR_ULEB: u8 = 0x70;
    pub const DO_REBASE_ULEB_TIMES_SKIPPING_ULEB: u8 = 0x80;
}

mod bind {
    pub const DONE: u8 = 0x00;
    pub const SET_DYLIB_ORDINAL_IMM: u8 = 0x10;
    pub const SET_DYLIB_ORDINAL_ULEB: u8 = 0x20;
    pub const SET_DYLIB_SPECIAL_IMM: u8 = 0x30;
    pub const SET_SYMBOL_TRAILING_FLAGS_IMM: u8 = 0x40;
    pub const SET_TYPE_IMM: u8 = 0x50;
    pub const SET_ADDEND_SLEB: u8 = 0x60;
    pub const SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x70;
    pub const ADD_ADDR_ULEB: u8 = 0x80;
    pub const DO_BIND: u8 = 0x90;
    pub const DO_BIND_ADD_ADDR_ULEB: u8 = 0xa0;
    pub const DO_BIND_ADD_ADDR_IMM_SCALED: u8 = 0xb0;
    pub const DO_BIND_ULEB_TIMES_SKIPPING_ULEB: u8 = 0xc0;
    pub const THREADED: u8 = 0xd0;
}

/// The tables of the slice `LC_DYLD_INFO` names that the layer applies: rebases, binds and
/// lazy binds. Its weak binds only matter between images that each define a symbol weakly,
/// and the executable, first in the load order, keeps its own definitions (D190).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DyldInfo {
    pub rebase: Range,
    pub bind: Range,
    pub lazy_bind: Range,
}

/// A pointer the opcodes name: in the segment with load-command index `segment`, at `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub segment: usize,
    pub offset: u64,
}

/// A pointer to bind to `name` from `source`, plus `addend`; a weak import binds to 0 when
/// nothing defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bind<'a> {
    pub location: Location,
    pub source: Source,
    pub weak_import: bool,
    pub name: &'a [u8],
    pub addend: i64,
}

/// Which bind table the opcodes are: the bind table ends at its first `DONE`, the lazy-bind
/// table is a run of entries that each end with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Binds,
    LazyBinds,
}

fn invalid(reason: &'static str) -> InvalidMachO {
    InvalidMachO(reason)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn unsigned(&mut self) -> Result<u64, InvalidMachO> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = self
                .byte()
                .ok_or(invalid("a fixup number runs past its table"))?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || (shift == 63 && bits > 1) {
                return Err(invalid("a fixup number does not fit in 64 bits"));
            }
            value |= bits << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
    }

    fn signed(&mut self) -> Result<i64, InvalidMachO> {
        let mut value = 0i64;
        let mut shift = 0;
        loop {
            let byte = self
                .byte()
                .ok_or(invalid("a fixup number runs past its table"))?;
            if shift >= 64 {
                return Err(invalid("a fixup number does not fit in 64 bits"));
            }
            value |= i64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1i64 << shift;
                }
                return Ok(value);
            }
        }
    }

    fn name(&mut self) -> Result<&'a [u8], InvalidMachO> {
        let rest = self.bytes.get(self.at..).unwrap_or_default();
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(invalid("a bound symbol's name does not end"))?;
        self.at += end + 1;
        Ok(&rest[..end])
    }
}

struct Cursor {
    location: Option<Location>,
    pointers: u64,
}

impl Cursor {
    fn place(&mut self, segment: u8, offset: u64) {
        self.location = Some(Location {
            segment: usize::from(segment),
            offset,
        });
    }

    fn advance(&mut self, by: u64) -> Result<(), InvalidMachO> {
        let location = self
            .location
            .as_mut()
            .ok_or(invalid("a fixup opcode moves before naming a segment"))?;
        location.offset = location.offset.wrapping_add(by);
        Ok(())
    }

    fn take(&mut self) -> Result<Location, InvalidMachO> {
        self.pointers += 1;
        if self.pointers > MAX_POINTERS {
            return Err(invalid("a fixup table names too many pointers"));
        }
        let location = self
            .location
            .ok_or(invalid("a fixup opcode acts before naming a segment"))?;
        self.advance(POINTER_SIZE)?;
        Ok(location)
    }
}

/// Calls `each` with every pointer the rebase opcodes `opcodes` name, in order.
pub fn rebases(
    opcodes: &[u8],
    mut each: impl FnMut(Location) -> Result<(), InvalidMachO>,
) -> Result<(), InvalidMachO> {
    let mut reader = Reader {
        bytes: opcodes,
        at: 0,
    };
    let mut cursor = Cursor {
        location: None,
        pointers: 0,
    };
    while let Some(byte) = reader.byte() {
        let immediate = byte & IMMEDIATE;
        match byte & OPCODE {
            rebase::DONE => break,
            rebase::SET_TYPE_IMM => {
                if immediate != POINTER_TYPE {
                    return Err(invalid("a rebase of a 32-bit code address (i386 only)"));
                }
            }
            rebase::SET_SEGMENT_AND_OFFSET_ULEB => {
                let offset = reader.unsigned()?;
                cursor.place(immediate, offset);
            }
            rebase::ADD_ADDR_ULEB => {
                let by = reader.unsigned()?;
                cursor.advance(by)?;
            }
            rebase::ADD_ADDR_IMM_SCALED => cursor.advance(u64::from(immediate) * POINTER_SIZE)?,
            rebase::DO_REBASE_IMM_TIMES => {
                for _ in 0..immediate {
                    each(cursor.take()?)?;
                }
            }
            rebase::DO_REBASE_ULEB_TIMES => {
                let times = reader.unsigned()?;
                for _ in 0..times {
                    each(cursor.take()?)?;
                }
            }
            rebase::DO_REBASE_ADD_ADDR_ULEB => {
                let by = reader.unsigned()?;
                each(cursor.take()?)?;
                cursor.advance(by)?;
            }
            rebase::DO_REBASE_ULEB_TIMES_SKIPPING_ULEB => {
                let times = reader.unsigned()?;
                let skip = reader.unsigned()?;
                for _ in 0..times {
                    each(cursor.take()?)?;
                    cursor.advance(skip)?;
                }
            }
            _ => return Err(invalid("an unknown rebase opcode")),
        }
    }
    Ok(())
}

/// Calls `each` with every pointer the bind opcodes `opcodes` of `table` name, in order.
pub fn binds<'a>(
    opcodes: &'a [u8],
    table: Table,
    mut each: impl FnMut(Bind<'a>) -> Result<(), InvalidMachO>,
) -> Result<(), InvalidMachO> {
    let mut reader = Reader {
        bytes: opcodes,
        at: 0,
    };
    let mut cursor = Cursor {
        location: None,
        pointers: 0,
    };
    let mut symbol = Symbol {
        source: Source::Executable,
        name: None,
        weak_import: false,
        addend: 0,
    };
    while let Some(byte) = reader.byte() {
        let immediate = byte & IMMEDIATE;
        match byte & OPCODE {
            bind::DONE if table == Table::Binds => break,
            bind::DONE => {}
            bind::SET_DYLIB_ORDINAL_IMM => symbol.source = library(u64::from(immediate))?,
            bind::SET_DYLIB_ORDINAL_ULEB => symbol.source = library(reader.unsigned()?)?,
            bind::SET_DYLIB_SPECIAL_IMM => {
                symbol.source = match immediate {
                    0 | 0x0f => Source::Executable,
                    0x0e => Source::Flat,
                    0x0d => Source::Weak,
                    _ => return Err(invalid("an unknown special library in a bind")),
                }
            }
            bind::SET_SYMBOL_TRAILING_FLAGS_IMM => {
                symbol.weak_import = immediate & WEAK_IMPORT != 0;
                symbol.name = Some(reader.name()?);
            }
            bind::SET_TYPE_IMM => {
                if immediate != POINTER_TYPE {
                    return Err(invalid("a bind of a 32-bit code address (i386 only)"));
                }
            }
            bind::SET_ADDEND_SLEB => symbol.addend = reader.signed()?,
            bind::SET_SEGMENT_AND_OFFSET_ULEB => {
                let offset = reader.unsigned()?;
                cursor.place(immediate, offset);
            }
            bind::ADD_ADDR_ULEB => {
                let by = reader.unsigned()?;
                cursor.advance(by)?;
            }
            bind::DO_BIND => each(symbol.at(cursor.take()?)?)?,
            bind::DO_BIND_ADD_ADDR_ULEB => {
                let by = reader.unsigned()?;
                each(symbol.at(cursor.take()?)?)?;
                cursor.advance(by)?;
            }
            bind::DO_BIND_ADD_ADDR_IMM_SCALED => {
                each(symbol.at(cursor.take()?)?)?;
                cursor.advance(u64::from(immediate) * POINTER_SIZE)?;
            }
            bind::DO_BIND_ULEB_TIMES_SKIPPING_ULEB => {
                let times = reader.unsigned()?;
                let skip = reader.unsigned()?;
                for _ in 0..times {
                    each(symbol.at(cursor.take()?)?)?;
                    cursor.advance(skip)?;
                }
            }
            bind::THREADED => {
                return Err(invalid("threaded binds (arm64e), which EggOS does not run"));
            }
            _ => return Err(invalid("an unknown bind opcode")),
        }
    }
    Ok(())
}

struct Symbol<'a> {
    source: Source,
    name: Option<&'a [u8]>,
    weak_import: bool,
    addend: i64,
}

impl<'a> Symbol<'a> {
    fn at(&self, location: Location) -> Result<Bind<'a>, InvalidMachO> {
        Ok(Bind {
            location,
            source: self.source,
            weak_import: self.weak_import,
            name: self
                .name
                .ok_or(invalid("a bind opcode binds before naming a symbol"))?,
            addend: self.addend,
        })
    }
}

fn library(ordinal: u64) -> Result<Source, InvalidMachO> {
    match usize::try_from(ordinal) {
        Ok(0) => Ok(Source::Executable),
        Ok(ordinal) => Ok(Source::Library(ordinal)),
        Err(_) => Err(invalid("a bind from an unknown library ordinal")),
    }
}
