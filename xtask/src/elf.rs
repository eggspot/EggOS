//! Just enough of the ELF format (System V gABI, ELF-64) to find where a kernel symbol lands
//! in physical memory: the symbol table and the loadable segments. Both byte orders are read,
//! for the big-endian architectures to come (ppc64, s390x).

/// A kernel symbol and the physical address the loader places it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol {
    /// Where QEMU's ELF loader puts the symbol's first byte (the segment's `p_paddr` plus the
    /// symbol's offset in it), which is the address QEMU's `loader` device takes.
    pub physical_address: u64,
    pub size: u64,
}

const MAGIC: &[u8; 4] = b"\x7fELF";
const CLASS_64: u8 = 2;
const DATA_LITTLE_ENDIAN: u8 = 1;
const DATA_BIG_ENDIAN: u8 = 2;
const SECTION_TYPE_SYMTAB: u32 = 2;
const SEGMENT_TYPE_LOAD: u32 = 1;
/// `st_shndx` of a symbol that is referenced but not defined in this image.
const SECTION_INDEX_UNDEFINED: u16 = 0;
const SECTION_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const SYMBOL_SIZE: usize = 24;

/// Finds the symbol called `name` in the ELF-64 image `image` and returns where it is loaded.
pub fn find_symbol(image: &[u8], name: &str) -> Result<Symbol, String> {
    let elf = Elf::parse(image)?;
    let (address, size) = elf
        .symbol(name)?
        .ok_or_else(|| format!("no symbol `{name}` in the kernel image"))?;
    let physical_address = elf
        .physical_address(address)?
        .ok_or_else(|| format!("symbol `{name}` ({address:#x}) is in no loadable segment"))?;
    Ok(Symbol {
        physical_address,
        size,
    })
}

struct Elf<'a> {
    image: &'a [u8],
    big_endian: bool,
}

impl<'a> Elf<'a> {
    fn parse(image: &'a [u8]) -> Result<Self, String> {
        if image.get(..4) != Some(MAGIC) {
            return Err("not an ELF image".into());
        }
        if image.get(4) != Some(&CLASS_64) {
            return Err("not an ELF-64 image".into());
        }
        let big_endian = match image.get(5) {
            Some(&DATA_LITTLE_ENDIAN) => false,
            Some(&DATA_BIG_ENDIAN) => true,
            _ => return Err("ELF image of unknown byte order".into()),
        };
        Ok(Self { image, big_endian })
    }

    fn bytes<const N: usize>(&self, offset: u64) -> Result<[u8; N], String> {
        usize::try_from(offset)
            .ok()
            .and_then(|start| self.image.get(start..start.checked_add(N)?))
            .map(|bytes| bytes.try_into().expect("slice of length N"))
            .ok_or_else(|| format!("ELF image truncated at offset {offset:#x}"))
    }

    fn u16(&self, offset: u64) -> Result<u16, String> {
        let bytes = self.bytes(offset)?;
        Ok(if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    fn u32(&self, offset: u64) -> Result<u32, String> {
        let bytes = self.bytes(offset)?;
        Ok(if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    }

    fn u64(&self, offset: u64) -> Result<u64, String> {
        let bytes = self.bytes(offset)?;
        Ok(if self.big_endian {
            u64::from_be_bytes(bytes)
        } else {
            u64::from_le_bytes(bytes)
        })
    }

    /// Offsets of the `count` table entries of `entry_size` bytes starting at `table`, after
    /// checking that the whole table lies inside the image (so adding a field offset within
    /// an entry cannot overflow).
    fn entries(
        &self,
        table: u64,
        count: u64,
        entry_size: usize,
    ) -> Result<impl Iterator<Item = u64> + use<>, String> {
        let size = entry_size as u64;
        let end = count
            .checked_mul(size)
            .and_then(|bytes| table.checked_add(bytes));
        if end.is_none_or(|end| end > self.image.len() as u64) {
            return Err(format!(
                "ELF table at {table:#x} runs past the end of the image"
            ));
        }
        Ok((0..count).map(move |index| table + index * size))
    }

    /// The value (address) and size of the symbol called `name`, from the `.symtab` section.
    /// Undefined symbols (section index 0) do not count.
    fn symbol(&self, name: &str) -> Result<Option<(u64, u64)>, String> {
        let section_table = self.u64(0x28)?;
        let section_count = self.u16(0x3c)?.into();
        for section in self.entries(section_table, section_count, SECTION_HEADER_SIZE)? {
            if self.u32(section + 4)? != SECTION_TYPE_SYMTAB {
                continue;
            }
            let symbols = self.u64(section + 24)?;
            let symbols_size = self.u64(section + 32)?;
            let names_section_from_sh_link = u64::from(self.u32(section + 40)?);
            let strings_section = self
                .entries(section_table, section_count, SECTION_HEADER_SIZE)?
                .nth(names_section_from_sh_link as usize)
                .ok_or_else(|| {
                    format!("symbol table links to missing section {names_section_from_sh_link}")
                })?;
            let strings = self.u64(strings_section + 24)?;
            let count = symbols_size / SYMBOL_SIZE as u64;
            for symbol in self.entries(symbols, count, SYMBOL_SIZE)? {
                let name_offset = strings.saturating_add(self.u32(symbol)?.into());
                if self.u16(symbol + 6)? != SECTION_INDEX_UNDEFINED
                    && self.c_string_is(name_offset, name)?
                {
                    return Ok(Some((self.u64(symbol + 8)?, self.u64(symbol + 16)?)));
                }
            }
        }
        Ok(None)
    }

    /// Whether the NUL-terminated string at `offset` is exactly `name`.
    fn c_string_is(&self, offset: u64, name: &str) -> Result<bool, String> {
        let start = usize::try_from(offset).map_err(|e| e.to_string())?;
        let rest = self
            .image
            .get(start..)
            .ok_or_else(|| format!("ELF string at {offset:#x} is outside the image"))?;
        Ok(rest.strip_prefix(name.as_bytes()).and_then(|r| r.first()) == Some(&0))
    }

    /// Where the loader places virtual address `address`: through the loadable segment that
    /// contains it.
    fn physical_address(&self, address: u64) -> Result<Option<u64>, String> {
        let segment_table = self.u64(0x20)?;
        let segment_count = self.u16(0x38)?.into();
        for segment in self.entries(segment_table, segment_count, PROGRAM_HEADER_SIZE)? {
            if self.u32(segment)? != SEGMENT_TYPE_LOAD {
                continue;
            }
            let virtual_start = self.u64(segment + 16)?;
            let memory_size = self.u64(segment + 40)?;
            if (virtual_start..virtual_start.saturating_add(memory_size)).contains(&address) {
                let physical_start = self.u64(segment + 24)?;
                return physical_start
                    .checked_add(address - virtual_start)
                    .map(Some)
                    .ok_or_else(|| format!("segment at physical {physical_start:#x} overflows"));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E_PHOFF: usize = 0x20;
    const E_SHOFF: usize = 0x28;
    const E_PHNUM: usize = 0x38;
    const E_SHNUM: usize = 0x3c;
    const P_VADDR: usize = 16;
    const P_PADDR: usize = 24;
    const P_MEMSZ: usize = 40;
    const SH_TYPE: usize = 4;
    const SH_OFFSET: usize = 24;
    const SH_SIZE: usize = 32;
    const SH_LINK: usize = 40;
    const ST_NAME: usize = 0;
    const ST_SHNDX: usize = 6;
    const ST_VALUE: usize = 8;
    const ST_SIZE: usize = 16;

    const PROGRAM_HEADERS: usize = 0x40;
    const LOAD_SEGMENT: usize = PROGRAM_HEADERS;
    const SECTION_HEADERS: usize = 0x100;
    const SYMTAB_SECTION_INDEX: u64 = 1;
    const SYMTAB_SECTION: usize = SECTION_HEADERS + SECTION_HEADER_SIZE;
    const STRTAB_SECTION_INDEX: u64 = 2;
    const STRTAB_SECTION: usize = SECTION_HEADERS + 2 * SECTION_HEADER_SIZE;
    const SYMBOLS: usize = 0x80;
    const OTHER_SYMBOL: usize = SYMBOLS + SYMBOL_SIZE;
    const PROBE_SYMBOL: usize = SYMBOLS + 2 * SYMBOL_SIZE;
    const STRINGS: usize = 0x1c0;

    /// A minimal ELF-64 image: one loadable segment at virtual `0xffff_0000_0000_1000`,
    /// physical `0x4000_1000`, 0x2000 bytes; a symbol table with `probe` at `+0x10`, 32
    /// bytes, after `other`.
    fn image(big_endian: bool) -> Vec<u8> {
        let mut out = vec![0u8; 0x200];
        let mut put = |offset: usize, value: u64, size: usize| {
            let bytes = if big_endian {
                value.to_be_bytes()[8 - size..].to_vec()
            } else {
                value.to_le_bytes()[..size].to_vec()
            };
            out[offset..offset + size].copy_from_slice(&bytes);
        };
        put(E_PHOFF, PROGRAM_HEADERS as u64, 8);
        put(E_SHOFF, SECTION_HEADERS as u64, 8);
        put(E_PHNUM, 1, 2);
        put(E_SHNUM, 3, 2);
        put(LOAD_SEGMENT, u64::from(SEGMENT_TYPE_LOAD), 4);
        put(LOAD_SEGMENT + P_VADDR, 0xffff_0000_0000_1000, 8);
        put(LOAD_SEGMENT + P_PADDR, 0x4000_1000, 8);
        put(LOAD_SEGMENT + P_MEMSZ, 0x2000, 8);
        put(SYMTAB_SECTION + SH_TYPE, u64::from(SECTION_TYPE_SYMTAB), 4);
        put(SYMTAB_SECTION + SH_OFFSET, SYMBOLS as u64, 8);
        put(SYMTAB_SECTION + SH_SIZE, 3 * SYMBOL_SIZE as u64, 8);
        put(SYMTAB_SECTION + SH_LINK, STRTAB_SECTION_INDEX, 4);
        put(STRTAB_SECTION + SH_OFFSET, STRINGS as u64, 8);
        put(OTHER_SYMBOL + ST_NAME, 1, 4);
        put(OTHER_SYMBOL + ST_SHNDX, SYMTAB_SECTION_INDEX, 2);
        put(OTHER_SYMBOL + ST_VALUE, 0xffff_0000_0000_1000, 8);
        put(PROBE_SYMBOL + ST_NAME, 7, 4);
        put(PROBE_SYMBOL + ST_SHNDX, SYMTAB_SECTION_INDEX, 2);
        put(PROBE_SYMBOL + ST_VALUE, 0xffff_0000_0000_1010, 8);
        put(PROBE_SYMBOL + ST_SIZE, 32, 8);
        out[..6].copy_from_slice(&[
            0x7f,
            b'E',
            b'L',
            b'F',
            CLASS_64,
            if big_endian {
                DATA_BIG_ENDIAN
            } else {
                DATA_LITTLE_ENDIAN
            },
        ]);
        out[STRINGS..STRINGS + 13].copy_from_slice(b"\0other\0probe\0");
        out
    }

    #[test]
    fn finds_the_symbol_at_its_physical_address_in_both_byte_orders() {
        for big_endian in [false, true] {
            assert_eq!(
                find_symbol(&image(big_endian), "probe"),
                Ok(Symbol {
                    physical_address: 0x4000_1010,
                    size: 32
                }),
                "big endian: {big_endian}"
            );
        }
    }

    #[test]
    fn a_name_that_only_starts_like_a_symbol_does_not_match() {
        assert!(find_symbol(&image(false), "prob").is_err());
        assert!(find_symbol(&image(false), "probes").is_err());
    }

    #[test]
    fn rejects_what_it_cannot_read() {
        assert_eq!(
            find_symbol(b"not elf", "probe"),
            Err("not an ELF image".into())
        );
        let mut truncated = image(false);
        truncated.truncate(0x150);
        assert!(find_symbol(&truncated, "probe").is_err());
        let mut elf32 = image(false);
        elf32[4] = 1;
        assert_eq!(
            find_symbol(&elf32, "probe"),
            Err("not an ELF-64 image".into())
        );
    }

    #[test]
    fn a_symbol_outside_every_segment_is_an_error() {
        let mut outside = image(false);
        let past_the_segment_s_end = 0xffff_0000_0000_3000u64;
        outside[PROBE_SYMBOL + ST_VALUE..PROBE_SYMBOL + ST_SIZE]
            .copy_from_slice(&past_the_segment_s_end.to_le_bytes());
        assert!(find_symbol(&outside, "probe").is_err());
    }

    #[test]
    fn an_undefined_symbol_does_not_count() {
        let mut undefined = image(false);
        undefined[PROBE_SYMBOL + ST_SHNDX..PROBE_SYMBOL + ST_VALUE]
            .copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            find_symbol(&undefined, "probe"),
            Err("no symbol `probe` in the kernel image".into())
        );
    }

    #[test]
    fn a_section_table_whose_end_overflows_u64_is_an_error_not_an_overflow() {
        let mut huge = image(false);
        huge[E_SHOFF..E_SHOFF + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(find_symbol(&huge, "probe").is_err());
    }

    #[test]
    fn a_section_count_whose_table_ends_past_the_image_is_an_error() {
        let mut many = image(false);
        many[E_SHNUM..E_SHNUM + 2].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(find_symbol(&many, "probe").is_err());
    }
}
