//! ELF, the executable format of Linux (and later Android) programs: the parts a loader needs,
//! read from an untrusted image (M6, D51).
//!
//! [`Elf::parse`] checks the file header against the machine the kernel runs on and that the
//! program headers lie within the image; [`Elf::program_headers`] then reads them one at a
//! time. Nothing is copied and nothing is trusted: every offset and size is checked before
//! use, and arithmetic that could overflow is checked too. What the program headers mean (how
//! segments become mappings) is the loader's business ([`crate::linux::exec`]).
//!
//! Only 64-bit little-endian images so far, the byte order of every architecture brought up;
//! big-endian ones (ppc64, s390x, mips64) come with those architectures (M9). Formats are from
//! the System V ABI's "Object Files" chapter (the ELF-64 specification).

/// The `e_machine` of each architecture, and the kernel's own.
pub mod machine {
    pub const X86_64: u16 = 62;
    pub const AARCH64: u16 = 183;
    pub const RISCV: u16 = 243;

    /// The machine the kernel runs on: the only one whose programs it runs natively (D17).
    #[cfg(target_arch = "x86_64")]
    pub const NATIVE: u16 = X86_64;
    #[cfg(target_arch = "aarch64")]
    pub const NATIVE: u16 = AARCH64;
    #[cfg(target_arch = "riscv64")]
    pub const NATIVE: u16 = RISCV;
}

/// Program header types (`p_type`) the loader looks at.
pub mod segment {
    /// A segment to map.
    pub const LOAD: u32 = 1;
    /// The path of the program's interpreter (dynamic loader).
    pub const INTERP: u32 = 3;
    /// Where the program headers themselves are in memory.
    pub const PHDR: u32 = 6;
    /// `PT_GNU_STACK`: the stack's permissions, in its flags.
    pub const GNU_STACK: u32 = 0x6474_e551;

    /// `p_flags`: executable, writable, readable.
    pub const EXECUTE: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const READ: u32 = 4;
}

/// The size of the file header (`Elf64_Ehdr`).
pub const HEADER_SIZE: usize = 64;
/// The size of a program header (`Elf64_Phdr`), the only `e_phentsize` accepted.
pub const PROGRAM_HEADER_SIZE: usize = 56;
/// The most program headers accepted: a page of them, as Linux allows, so a hostile count
/// cannot make the loader walk far (this also refuses `PN_XNUM`'s extended count).
pub const MAX_PROGRAM_HEADERS: u16 = (4096 / PROGRAM_HEADER_SIZE) as u16;

/// What kind of file an image is (`e_type`), of the kinds a loader runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `ET_EXEC`: linked to run at the addresses its program headers name.
    Executable,
    /// `ET_DYN`: position-independent, loaded at an address the loader picks (a PIE, a
    /// static PIE, or a dynamic loader).
    PositionIndependent,
}

/// Why an image is not one the kernel can run: Linux's `ENOEXEC`. The text says what is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidElf(pub &'static str);

/// A parsed ELF image: its file header, checked, and the image it came from.
#[derive(Debug, Clone, Copy)]
pub struct Elf<'a> {
    image: &'a [u8],
    kind: Kind,
    entry: u64,
    program_header_offset: usize,
    program_header_count: u16,
}

/// One program header (`Elf64_Phdr`), as read: not yet checked against the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramHeader {
    pub kind: u32,
    pub flags: u32,
    /// Where the segment's bytes start in the image.
    pub offset: u64,
    /// Where it starts in memory, before any load bias.
    pub virt: u64,
    /// Its bytes in the image; the rest of its memory, up to `memory_size`, is zero.
    pub file_size: u64,
    pub memory_size: u64,
    pub align: u64,
}

impl<'a> Elf<'a> {
    /// Checks `image`'s file header: an ELF-64 little-endian image of `machine`, an executable
    /// or position-independent file, with program headers of the standard size that lie
    /// within the image.
    pub fn parse(image: &'a [u8], machine: u16) -> Result<Self, InvalidElf> {
        if image.len() < HEADER_SIZE || image[..4] != *b"\x7fELF" {
            return Err(InvalidElf("not an ELF image"));
        }
        // `e_ident`: class, byte order, version.
        if image[4] != 2 {
            return Err(InvalidElf("not a 64-bit ELF image"));
        }
        if image[5] != 1 {
            return Err(InvalidElf("not a little-endian ELF image"));
        }
        if image[6] != 1 {
            return Err(InvalidElf("an unknown ELF version"));
        }
        let kind = match u16_at(image, 16) {
            2 => Kind::Executable,
            3 => Kind::PositionIndependent,
            _ => return Err(InvalidElf("not an executable ELF file")),
        };
        if u16_at(image, 18) != machine {
            return Err(InvalidElf("an ELF image for another machine"));
        }
        let entry = u64_at(image, 24);
        let program_header_offset = u64_at(image, 32);
        if usize::from(u16_at(image, 54)) != PROGRAM_HEADER_SIZE {
            return Err(InvalidElf("program headers of an unknown size"));
        }
        let program_header_count = u16_at(image, 56);
        if program_header_count == 0 || program_header_count > MAX_PROGRAM_HEADERS {
            return Err(InvalidElf("no program headers, or too many"));
        }
        let table_size = (usize::from(program_header_count) * PROGRAM_HEADER_SIZE) as u64;
        if program_header_offset
            .checked_add(table_size)
            .is_none_or(|end| end > image.len() as u64)
        {
            return Err(InvalidElf("the program headers are not within the image"));
        }
        Ok(Self {
            image,
            kind,
            entry,
            // Within the image, so it fits.
            program_header_offset: program_header_offset as usize,
            program_header_count,
        })
    }

    /// The whole image.
    pub fn image(&self) -> &'a [u8] {
        self.image
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The address of the first instruction, before any load bias.
    pub fn entry(&self) -> u64 {
        self.entry
    }

    /// Where the program headers start in the image (`e_phoff`).
    pub fn program_header_offset(&self) -> u64 {
        self.program_header_offset as u64
    }

    /// How many program headers there are (`e_phnum`), at least one.
    pub fn program_header_count(&self) -> u16 {
        self.program_header_count
    }

    /// The program headers, in the image's order (for `LOAD` segments, ascending addresses).
    pub fn program_headers(&self) -> impl Iterator<Item = ProgramHeader> + 'a {
        let table = &self.image[self.program_header_offset..]
            [..usize::from(self.program_header_count) * PROGRAM_HEADER_SIZE];
        table
            .as_chunks::<PROGRAM_HEADER_SIZE>()
            .0
            .iter()
            .map(|header| ProgramHeader {
                kind: u32_at(header, 0),
                flags: u32_at(header, 4),
                offset: u64_at(header, 8),
                virt: u64_at(header, 16),
                // `p_paddr` (24) means nothing to a user program.
                file_size: u64_at(header, 32),
                memory_size: u64_at(header, 40),
                align: u64_at(header, 48),
            })
    }
}

impl ProgramHeader {
    /// The segment's bytes in `image`, or `None` if they are not all within it.
    pub fn file_bytes<'a>(&self, image: &'a [u8]) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(self.file_size)?;
        image.get(usize::try_from(self.offset).ok()?..usize::try_from(end).ok()?)
    }
}

/// The little-endian `u16` at `offset` of `bytes`, which the caller has checked holds it.
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(word)
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(word)
}

/// Builds small ELF images, for the loader's tests and the boot self-test that runs the HAL's
/// test program as a Linux program: there is no file system to load one from yet (M7).
pub mod build {
    use super::{HEADER_SIZE, PROGRAM_HEADER_SIZE, ProgramHeader};
    use alloc::vec;
    use alloc::vec::Vec;

    /// An image of `kind` (`e_type`) for `machine` that starts at `entry`, with `headers` as
    /// its program headers right after the file header, and `contents` at their offsets (the
    /// image grows to hold them; gaps are zero).
    pub fn image(
        kind: u16,
        machine: u16,
        entry: u64,
        headers: &[ProgramHeader],
        contents: &[(usize, &[u8])],
    ) -> Vec<u8> {
        let table_end = HEADER_SIZE + headers.len() * PROGRAM_HEADER_SIZE;
        let end = contents
            .iter()
            .map(|(offset, bytes)| offset + bytes.len())
            .fold(table_end, usize::max);
        let mut image = vec![0u8; end];
        image[..4].copy_from_slice(b"\x7fELF");
        // 64-bit, little-endian, version 1.
        image[4..7].copy_from_slice(&[2, 1, 1]);
        image[16..18].copy_from_slice(&kind.to_le_bytes());
        image[18..20].copy_from_slice(&machine.to_le_bytes());
        image[20..24].copy_from_slice(&1u32.to_le_bytes());
        image[24..32].copy_from_slice(&entry.to_le_bytes());
        image[32..40].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        image[52..54].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
        image[54..56].copy_from_slice(&(PROGRAM_HEADER_SIZE as u16).to_le_bytes());
        image[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (index, header) in headers.iter().enumerate() {
            let at = HEADER_SIZE + index * PROGRAM_HEADER_SIZE;
            let entry = &mut image[at..at + PROGRAM_HEADER_SIZE];
            entry[0..4].copy_from_slice(&header.kind.to_le_bytes());
            entry[4..8].copy_from_slice(&header.flags.to_le_bytes());
            entry[8..16].copy_from_slice(&header.offset.to_le_bytes());
            entry[16..24].copy_from_slice(&header.virt.to_le_bytes());
            entry[24..32].copy_from_slice(&header.virt.to_le_bytes());
            entry[32..40].copy_from_slice(&header.file_size.to_le_bytes());
            entry[40..48].copy_from_slice(&header.memory_size.to_le_bytes());
            entry[48..56].copy_from_slice(&header.align.to_le_bytes());
        }
        for (offset, bytes) in contents {
            image[*offset..offset + bytes.len()].copy_from_slice(bytes);
        }
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn load(offset: u64, virt: u64, file_size: u64, memory_size: u64) -> ProgramHeader {
        ProgramHeader {
            kind: segment::LOAD,
            flags: segment::READ,
            offset,
            virt,
            file_size,
            memory_size,
            align: 0x1000,
        }
    }

    fn sample() -> Vec<u8> {
        build::image(
            2,
            machine::NATIVE,
            0x40_1000,
            &[
                load(0, 0x40_0000, 0x1010, 0x1010),
                load(0x1010, 0x60_1010, 8, 0x100),
            ],
            &[(0x1000, &[0xaa; 16]), (0x1010, b"datadata")],
        )
    }

    #[test]
    fn a_built_image_parses_back() {
        let image = sample();
        let elf = Elf::parse(&image, machine::NATIVE).unwrap();
        assert_eq!(elf.kind(), Kind::Executable);
        assert_eq!(elf.entry(), 0x40_1000);
        assert_eq!(elf.program_header_offset(), HEADER_SIZE as u64);
        assert_eq!(elf.program_header_count(), 2);
        let headers: Vec<_> = elf.program_headers().collect();
        assert_eq!(
            headers,
            [
                load(0, 0x40_0000, 0x1010, 0x1010),
                load(0x1010, 0x60_1010, 8, 0x100)
            ]
        );
        assert_eq!(headers[1].file_bytes(&image), Some(&b"datadata"[..]));
        assert_eq!(headers[0].file_bytes(&image).unwrap().len(), 0x1010);
        // Past the end, overflowing: not within the image.
        assert_eq!(load(0x1010, 0, 9, 9).file_bytes(&image), None);
        assert_eq!(load(u64::MAX, 0, 2, 2).file_bytes(&image), None);
    }

    #[test]
    fn bad_headers_are_refused() {
        let good = sample();
        let with = |offset: usize, bytes: &[u8]| {
            let mut image = good.clone();
            image[offset..offset + bytes.len()].copy_from_slice(bytes);
            Elf::parse(&image, machine::NATIVE).map(|_| ())
        };
        assert_eq!(with(0, b"\x7fELG"), Err(InvalidElf("not an ELF image")));
        assert_eq!(with(4, &[1]), Err(InvalidElf("not a 64-bit ELF image")));
        assert_eq!(
            with(5, &[2]),
            Err(InvalidElf("not a little-endian ELF image"))
        );
        assert_eq!(with(6, &[0]), Err(InvalidElf("an unknown ELF version")));
        // A relocatable object and a core file are not programs.
        for kind in [1u16, 4] {
            assert_eq!(
                with(16, &kind.to_le_bytes()),
                Err(InvalidElf("not an executable ELF file"))
            );
        }
        assert!(with(16, &3u16.to_le_bytes()).is_ok());
        let other = if machine::NATIVE == machine::X86_64 {
            machine::AARCH64
        } else {
            machine::X86_64
        };
        assert_eq!(
            with(18, &other.to_le_bytes()),
            Err(InvalidElf("an ELF image for another machine"))
        );
        assert_eq!(
            with(54, &32u16.to_le_bytes()),
            Err(InvalidElf("program headers of an unknown size"))
        );
        for count in [0, MAX_PROGRAM_HEADERS + 1] {
            assert_eq!(
                with(56, &count.to_le_bytes()),
                Err(InvalidElf("no program headers, or too many"))
            );
        }
        // A table that runs past the end, or whose offset wraps.
        assert_eq!(
            with(56, &MAX_PROGRAM_HEADERS.to_le_bytes()),
            Err(InvalidElf("the program headers are not within the image"))
        );
        assert_eq!(
            with(32, &u64::MAX.to_le_bytes()),
            Err(InvalidElf("the program headers are not within the image"))
        );
        // Too short for a header at all.
        assert_eq!(
            Elf::parse(&good[..HEADER_SIZE - 1], machine::NATIVE).map(|_| ()),
            Err(InvalidElf("not an ELF image"))
        );
    }
}
