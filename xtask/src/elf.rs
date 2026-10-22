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

    /// Offsets of the `count` table entries of `entry_size` bytes starting at `table`.
    fn entries(table: u64, count: u64, entry_size: usize) -> impl Iterator<Item = u64> + use<> {
        (0..count).map(move |index| table + index * entry_size as u64)
    }

    /// The value (address) and size of the symbol called `name`, from the `.symtab` section.
    fn symbol(&self, name: &str) -> Result<Option<(u64, u64)>, String> {
        let section_table = self.u64(0x28)?;
        let section_count = self.u16(0x3c)?.into();
        for section in Self::entries(section_table, section_count, SECTION_HEADER_SIZE) {
            if self.u32(section + 4)? != SECTION_TYPE_SYMTAB {
                continue;
            }
            let symbols = self.u64(section + 24)?;
            let symbols_size = self.u64(section + 32)?;
            // `sh_link` names the string table holding the symbol names.
            let strings_section =
                section_table + u64::from(self.u32(section + 40)?) * SECTION_HEADER_SIZE as u64;
            let strings = self.u64(strings_section + 24)?;
            let count = symbols_size / SYMBOL_SIZE as u64;
            for symbol in Self::entries(symbols, count, SYMBOL_SIZE) {
                let name_offset = strings + u64::from(self.u32(symbol)?);
                if self.c_string_is(name_offset, name)? {
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
        for segment in Self::entries(segment_table, segment_count, PROGRAM_HEADER_SIZE) {
            if self.u32(segment)? != SEGMENT_TYPE_LOAD {
                continue;
            }
            let virtual_start = self.u64(segment + 16)?;
            let memory_size = self.u64(segment + 40)?;
            if (virtual_start..virtual_start.saturating_add(memory_size)).contains(&address) {
                return Ok(Some(self.u64(segment + 24)? + (address - virtual_start)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Header: program headers at 0x40 (one), section headers at 0x100 (three).
        put(0x20, 0x40, 8);
        put(0x28, 0x100, 8);
        put(0x38, 1, 2);
        put(0x3c, 3, 2);
        // The loadable segment.
        put(0x40, u64::from(SEGMENT_TYPE_LOAD), 4);
        put(0x40 + 16, 0xffff_0000_0000_1000, 8);
        put(0x40 + 24, 0x4000_1000, 8);
        put(0x40 + 40, 0x2000, 8);
        // Section 1: `.symtab` at 0x80, three symbols (the null one first), strings in
        // section 2.
        put(0x140 + 4, u64::from(SECTION_TYPE_SYMTAB), 4);
        put(0x140 + 24, 0x80, 8);
        put(0x140 + 32, 3 * SYMBOL_SIZE as u64, 8);
        put(0x140 + 40, 2, 4);
        // Section 2: the string table at 0x1c0.
        put(0x180 + 24, 0x1c0, 8);
        // Symbol 1: `other`; symbol 2: `probe`.
        put(0x80 + 24, 1, 4);
        put(0x80 + 24 + 8, 0xffff_0000_0000_1000, 8);
        put(0x80 + 48, 7, 4);
        put(0x80 + 48 + 8, 0xffff_0000_0000_1010, 8);
        put(0x80 + 48 + 16, 32, 8);
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
        out[0x1c0..0x1cd].copy_from_slice(b"\0other\0probe\0");
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
        // Move `probe` past the end of the segment.
        outside[0x80 + 48 + 8..0x80 + 48 + 16]
            .copy_from_slice(&0xffff_0000_0000_3000u64.to_le_bytes());
        assert!(find_symbol(&outside, "probe").is_err());
    }
}
