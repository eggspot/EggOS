//! Chained fixups (`LC_DYLD_CHAINED_FIXUPS`): the table of imported symbols and, for each page
//! of each segment, a chain of pointers to rebase by the image's slide or bind to an import.
//! The pointer formats are those of arm64 and x86_64 executables, `DYLD_CHAINED_PTR_64` and
//! `DYLD_CHAINED_PTR_64_OFFSET`; every offset, count and chain link is checked, and a chain
//! only moves forward inside its page, so a malformed one ends instead of looping.

use crate::macho::{InvalidMachO, MachO, Range, read_u32, read_u64};

pub(crate) const POINTER_64: u16 = 2;
const POINTER_64_OFFSET: u16 = 6;
const PAGE_START_NONE: u16 = 0xffff;
const STRIDE: usize = 4;
const IMPORT: u32 = 1;
const IMPORT_ADDEND: u32 = 2;
const IMPORT_ADDEND64: u32 = 3;
const MAX_IMPORTS: u32 = 1 << 16;

/// Which library an import comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The 1-based ordinal of one of the executable's `LC_LOAD_DYLIB` libraries.
    Library(usize),
    /// The executable itself.
    Executable,
    /// Every loaded image (flat namespace).
    Flat,
    /// Weak definitions in any image.
    Weak,
}

/// One imported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Import<'a> {
    pub source: Source,
    pub weak: bool,
    pub name: &'a [u8],
    pub addend: i64,
}

/// The parsed chained-fixups table.
#[derive(Debug, Clone, Copy)]
pub struct ChainedFixups<'a> {
    data: &'a [u8],
    starts: usize,
    imports: usize,
    symbols: usize,
    import_count: usize,
    import_format: u32,
}

fn invalid(reason: &'static str) -> InvalidMachO {
    InvalidMachO(reason)
}

impl<'a> ChainedFixups<'a> {
    /// Reads the header of the table at `range` of `macho`'s bytes.
    pub fn parse(macho: &MachO<'a>, range: Range) -> Result<Self, InvalidMachO> {
        let data = macho
            .bytes
            .get(range.offset..range.offset + range.size)
            .ok_or(invalid("the chained fixups are outside the file"))?;
        let field = |offset| read_u32(data, offset).ok_or(invalid("truncated chained fixups"));
        if field(0)? != 0 {
            return Err(invalid("an unknown chained-fixups version"));
        }
        if field(24)? != 0 {
            return Err(invalid(
                "compressed import names, which EggOS does not read yet",
            ));
        }
        let import_format = field(20)?;
        let import_size = match import_format {
            IMPORT => 4,
            IMPORT_ADDEND => 8,
            IMPORT_ADDEND64 => 16,
            _ => return Err(invalid("an unknown import format")),
        };
        let import_count = field(16)?;
        if import_count > MAX_IMPORTS {
            return Err(invalid("too many imports"));
        }
        let fixups = Self {
            data,
            starts: field(4)? as usize,
            imports: field(8)? as usize,
            symbols: field(12)? as usize,
            import_count: import_count as usize,
            import_format,
        };
        let imports_end = (import_count as usize)
            .checked_mul(import_size)
            .and_then(|size| size.checked_add(fixups.imports));
        if imports_end.is_none_or(|end| end > data.len()) || fixups.symbols > data.len() {
            return Err(invalid("the import table is outside the chained fixups"));
        }
        read_u32(data, fixups.starts).ok_or(invalid("the chain starts are outside the table"))?;
        Ok(fixups)
    }

    pub fn import_count(&self) -> usize {
        self.import_count
    }

    /// Import number `index`.
    pub fn import(&self, index: usize) -> Result<Import<'a>, InvalidMachO> {
        if index >= self.import_count {
            return Err(invalid("a bind names an import that does not exist"));
        }
        let truncated = invalid("truncated import");
        let (ordinal, weak, name_offset, addend) = match self.import_format {
            IMPORT | IMPORT_ADDEND => {
                let size = if self.import_format == IMPORT { 4 } else { 8 };
                let at = self.imports + index * size;
                let word = read_u32(self.data, at).ok_or(truncated)?;
                let addend = if self.import_format == IMPORT_ADDEND {
                    i64::from(read_u32(self.data, at + 4).ok_or(truncated)? as i32)
                } else {
                    0
                };
                let ordinal = i64::from((word & 0xff) as u8 as i8);
                (ordinal, word & 0x100 != 0, (word >> 9) as usize, addend)
            }
            _ => {
                let at = self.imports + index * 16;
                let word = read_u64(self.data, at).ok_or(truncated)?;
                let addend = read_u64(self.data, at + 8).ok_or(truncated)? as i64;
                let ordinal = i64::from((word & 0xffff) as u16 as i16);
                (ordinal, word & 0x1_0000 != 0, (word >> 32) as usize, addend)
            }
        };
        let source = match ordinal {
            0 => Source::Executable,
            -1 => Source::Executable,
            -2 => Source::Flat,
            -3 => Source::Weak,
            positive if positive > 0 => Source::Library(positive as usize),
            _ => return Err(invalid("an import from an unknown library ordinal")),
        };
        let name_start = self
            .symbols
            .checked_add(name_offset)
            .filter(|&start| start < self.data.len())
            .ok_or(invalid("an import's name is outside the table"))?;
        let rest = &self.data[name_start..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(invalid("an import's name does not end"))?;
        Ok(Import {
            source,
            weak,
            name: &rest[..end],
            addend,
        })
    }

    /// Rebases and binds every pointer of every chain in `image`, the executable's segments laid
    /// out from `macho.base_address()` and placed at `load_address`; `imports[i]` is the address
    /// import `i` binds to.
    pub fn apply(
        &self,
        macho: &MachO<'_>,
        image: &mut [u8],
        load_address: u64,
        imports: &[u64],
    ) -> Result<(), InvalidMachO> {
        let slide = load_address.wrapping_sub(macho.base_address());
        self.walk(image, |_, raw, format| {
            resolve(raw, format, slide, load_address, imports).map(Some)
        })
    }

    /// Visits every pointer of every chain in `image` (laid out as for [`Self::apply`]) in chain
    /// order: `each` gets its offset in the image, its raw chained value and its segment's
    /// pointer format, and returns the value to write there, if any.
    pub(crate) fn walk(
        &self,
        image: &mut [u8],
        mut each: impl FnMut(usize, u64, u16) -> Result<Option<u64>, InvalidMachO>,
    ) -> Result<(), InvalidMachO> {
        let truncated = invalid("truncated chain starts");
        let segment_count = read_u32(self.data, self.starts).ok_or(truncated)? as usize;
        if segment_count > 256 {
            return Err(invalid("too many segments in the chain starts"));
        }
        for index in 0..segment_count {
            let offset = read_u32(self.data, self.starts + 4 + index * 4).ok_or(truncated)?;
            if offset == 0 {
                continue;
            }
            let at = self
                .starts
                .checked_add(offset as usize)
                .ok_or(invalid("a segment's chain starts are outside the table"))?;
            self.walk_segment(at, image, &mut each)?;
        }
        Ok(())
    }

    fn walk_segment(
        &self,
        at: usize,
        image: &mut [u8],
        each: &mut impl FnMut(usize, u64, u16) -> Result<Option<u64>, InvalidMachO>,
    ) -> Result<(), InvalidMachO> {
        let truncated = invalid("truncated segment chain starts");
        let half = |offset: usize| -> Result<u16, InvalidMachO> {
            let bytes = self
                .data
                .get(at + offset..at + offset + 2)
                .ok_or(truncated)?;
            Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
        };
        let page_size = usize::from(half(4)?);
        let format = half(6)?;
        let segment_offset = read_u64(self.data, at + 8).ok_or(truncated)?;
        let page_count = usize::from(half(20)?);
        if !matches!(format, POINTER_64 | POINTER_64_OFFSET) {
            return Err(invalid(
                "a chained-pointer format EggOS does not know (arm64e?)",
            ));
        }
        if page_size != 0x1000 && page_size != 0x4000 {
            return Err(invalid("an unknown chained-fixups page size"));
        }
        for page in 0..page_count {
            let start = half(22 + page * 2)?;
            if start == PAGE_START_NONE {
                continue;
            }
            if start & 0x8000 != 0 {
                return Err(invalid(
                    "several chains in a page, which 64-bit formats never use",
                ));
            }
            let page_start = usize::try_from(segment_offset)
                .ok()
                .and_then(|offset| offset.checked_add(page * page_size))
                .ok_or(invalid("a chain's page is outside the image"))?;
            let page_end = page_start.saturating_add(page_size).min(image.len());
            let mut location = page_start.saturating_add(usize::from(start));
            loop {
                if location >= page_end || location + 8 > image.len() {
                    return Err(invalid(
                        "a chained pointer is outside its page or the image",
                    ));
                }
                let slot = &mut image[location..location + 8];
                let raw = u64::from_le_bytes(slot.try_into().unwrap_or([0; 8]));
                let next = ((raw >> 51) & 0xfff) as usize;
                if let Some(value) = each(location, raw, format)? {
                    slot.copy_from_slice(&value.to_le_bytes());
                }
                if next == 0 {
                    break;
                }
                location += next * STRIDE;
            }
        }
        Ok(())
    }
}

fn resolve(
    raw: u64,
    format: u16,
    slide: u64,
    load_address: u64,
    imports: &[u64],
) -> Result<u64, InvalidMachO> {
    if raw >> 63 != 0 {
        let ordinal = (raw & 0xff_ffff) as usize;
        let addend = (raw >> 24) & 0xff;
        let target = imports
            .get(ordinal)
            .ok_or(invalid("a bind names an import that does not exist"))?;
        return Ok(target.wrapping_add(addend));
    }
    let target = raw & 0xf_ffff_ffff;
    let high8 = ((raw >> 36) & 0xff) << 56;
    Ok(match format {
        POINTER_64 => target.wrapping_add(slide) | high8,
        _ => load_address.wrapping_add(target) | high8,
    })
}
