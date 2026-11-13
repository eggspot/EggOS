//! ELF, the executable format of Linux (and later Android) programs: the parts a loader needs,
//! read from an untrusted image (M6, D51).
//!
//! [`Elf::parse`] checks the file header against the machine the kernel runs on and that the
//! program headers lie within the image; [`Elf::program_headers`] then reads them one at a
//! time. Nothing is copied and nothing is trusted: every offset and size is checked before
//! use, and arithmetic that could overflow is checked too. What the program headers mean (how
//! segments become mappings) is the loader's business ([`crate::linux::exec`]).
//!
//! Only images of the kernel's own class and byte order: the programs it runs natively (D17)
//! share them, so every field reads in native order, little-endian today and big-endian on
//! the big-endian ports (D318), and addresses are 64-bit, or 32-bit on the 32-bit ports
//! (D488). Formats are from the System V ABI's "Object Files" chapter (the ELF-32 and ELF-64
//! specifications).

/// `EI_DATA` of the kernel's own byte order: `ELFDATA2LSB` (1) or `ELFDATA2MSB` (2).
const NATIVE_DATA: u8 = if cfg!(target_endian = "big") { 2 } else { 1 };

/// Whether the kernel's programs are ELF-64 images; ELF-32 ones on the 32-bit ports (D488).
pub const CLASS_64: bool = crate::linux::abi::WORD_BYTES == 8;

/// `EI_CLASS` of the kernel's programs: `ELFCLASS64` (2) or `ELFCLASS32` (1).
const NATIVE_CLASS: u8 = if CLASS_64 { 2 } else { 1 };

/// Bytes of an address, an offset or a size in the kernel's class of image.
const ADDRESS: usize = if CLASS_64 { 8 } else { 4 };

mod field {
    use super::{ADDRESS, CLASS_64};

    pub const ENTRY: usize = 24;
    pub const PROGRAM_HEADER_OFFSET: usize = ENTRY + ADDRESS;
    pub const SECTION_HEADER_OFFSET: usize = PROGRAM_HEADER_OFFSET + ADDRESS;
    pub const FLAGS: usize = SECTION_HEADER_OFFSET + ADDRESS;
    pub const HEADER_SIZE: usize = FLAGS + 4;
    pub const PROGRAM_HEADER_SIZE: usize = HEADER_SIZE + 2;
    pub const PROGRAM_HEADER_COUNT: usize = PROGRAM_HEADER_SIZE + 2;

    pub const KIND: usize = 0;
    pub const SEGMENT_FLAGS: usize = if CLASS_64 { 4 } else { 24 };
    pub const OFFSET: usize = if CLASS_64 { 8 } else { 4 };
    pub const VIRT: usize = OFFSET + ADDRESS;
    pub const PHYS: usize = VIRT + ADDRESS;
    pub const FILE_SIZE: usize = PHYS + ADDRESS;
    pub const MEMORY_SIZE: usize = FILE_SIZE + ADDRESS;
    pub const ALIGN: usize = if CLASS_64 { MEMORY_SIZE + ADDRESS } else { 28 };
}

/// The `e_machine` of each architecture, and the kernel's own.
pub mod machine {
    pub const X86_64: u16 = 62;
    pub const AARCH64: u16 = 183;
    pub const RISCV: u16 = 243;
    /// LoongArch (D290).
    pub const LOONGARCH: u16 = 258;
    /// 64-bit PowerPC (D304).
    pub const PPC64: u16 = 21;
    /// MIPS (D313).
    pub const MIPS: u16 = 8;
    /// s390x (D323).
    pub const S390: u16 = 22;
    /// SPARC V9, 64-bit (D327).
    pub const SPARCV9: u16 = 43;
    /// 32-bit Arm (D488).
    pub const ARM: u16 = 40;

    /// The machine the kernel runs on: the only one whose programs it runs natively (D17).
    pub const NATIVE: u16 = crate::linux::abi::ELF_MACHINE;
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

/// The size of the file header (`Elf64_Ehdr`, or `Elf32_Ehdr` on the 32-bit ports).
pub const HEADER_SIZE: usize = if CLASS_64 { 64 } else { 52 };
/// The size of a program header (`Elf64_Phdr`, or `Elf32_Phdr`), the only `e_phentsize`
/// accepted.
pub const PROGRAM_HEADER_SIZE: usize = if CLASS_64 { 56 } else { 32 };
/// The most program headers accepted: a page of them, as Linux allows, so a hostile count
/// cannot make the loader walk far (this also refuses `PN_XNUM`'s extended count).
pub const MAX_PROGRAM_HEADERS: u16 = (4096 / PROGRAM_HEADER_SIZE) as u16;
/// The longest interpreter path accepted, its NUL included: Linux's `PATH_MAX`.
pub const MAX_INTERPRETER_PATH: u64 = 4096;

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

/// One program header (`Elf64_Phdr` or `Elf32_Phdr`), as read: not yet checked against the
/// image.
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
    /// Checks `image`'s file header: an image of the kernel's class and byte order for
    /// `machine`, an executable or position-independent file, with program headers of the
    /// standard size that lie
    /// within the image.
    pub fn parse(image: &'a [u8], machine: u16) -> Result<Self, InvalidElf> {
        if image.len() < HEADER_SIZE || image[..4] != *b"\x7fELF" {
            return Err(InvalidElf("not an ELF image"));
        }
        if image[4] != NATIVE_CLASS {
            return Err(InvalidElf(match CLASS_64 {
                true => "not a 64-bit ELF image",
                false => "not a 32-bit ELF image",
            }));
        }
        if image[5] != NATIVE_DATA {
            return Err(InvalidElf("an ELF image in the other byte order"));
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
        let entry = address_at(image, field::ENTRY);
        let program_header_offset = address_at(image, field::PROGRAM_HEADER_OFFSET);
        if usize::from(u16_at(image, field::PROGRAM_HEADER_SIZE)) != PROGRAM_HEADER_SIZE {
            return Err(InvalidElf("program headers of an unknown size"));
        }
        let program_header_count = u16_at(image, field::PROGRAM_HEADER_COUNT);
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

    /// The path of the program's interpreter, its dynamic loader (`PT_INTERP`, D167), without
    /// its NUL, if it names one: the first such header's bytes, which must lie within the
    /// image, be from 2 to [`MAX_INTERPRETER_PATH`] bytes long and end with a NUL, as Linux
    /// requires. The path ends at its first NUL.
    pub fn interpreter(&self) -> Result<Option<&'a [u8]>, InvalidElf> {
        let Some(header) = self
            .program_headers()
            .find(|header| header.kind == segment::INTERP)
        else {
            return Ok(None);
        };
        if !(2..=MAX_INTERPRETER_PATH).contains(&header.file_size) {
            return Err(InvalidElf("an interpreter path of a length Linux refuses"));
        }
        let bytes = header
            .file_bytes(self.image)
            .ok_or(InvalidElf("the interpreter path is not within the image"))?;
        if bytes.last() != Some(&0) {
            return Err(InvalidElf("an interpreter path without its NUL"));
        }
        let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(0);
        Ok(Some(&bytes[..end]))
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
                kind: u32_at(header, field::KIND),
                flags: u32_at(header, field::SEGMENT_FLAGS),
                offset: address_at(header, field::OFFSET),
                virt: address_at(header, field::VIRT),
                file_size: address_at(header, field::FILE_SIZE),
                memory_size: address_at(header, field::MEMORY_SIZE),
                align: address_at(header, field::ALIGN),
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

/// The native-order `u16` at `offset` of `bytes`, which the caller has checked holds it.
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(word)
}

fn address_at(bytes: &[u8], offset: usize) -> u64 {
    match CLASS_64 {
        true => u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap()),
        false => u64::from(u32_at(bytes, offset)),
    }
}

fn put_address(bytes: &mut [u8], offset: usize, value: u64) {
    match CLASS_64 {
        true => bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes()),
        false => bytes[offset..offset + 4].copy_from_slice(&(value as u32).to_ne_bytes()),
    }
}

/// Builds small ELF images, for the loader's tests and the boot self-test that runs the HAL's
/// test program as a Linux program: there is no file system to load one from yet (M7).
pub mod build {
    use super::{
        HEADER_SIZE, NATIVE_CLASS, NATIVE_DATA, PROGRAM_HEADER_SIZE, ProgramHeader, field,
        put_address,
    };
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
        image[4..7].copy_from_slice(&[NATIVE_CLASS, NATIVE_DATA, 1]);
        let put_u16 = |image: &mut [u8], at: usize, value: u16| {
            image[at..at + 2].copy_from_slice(&value.to_ne_bytes());
        };
        put_u16(&mut image, 16, kind);
        put_u16(&mut image, 18, machine);
        image[20..24].copy_from_slice(&1u32.to_ne_bytes());
        put_address(&mut image, field::ENTRY, entry);
        put_address(&mut image, field::PROGRAM_HEADER_OFFSET, HEADER_SIZE as u64);
        put_u16(&mut image, field::HEADER_SIZE, HEADER_SIZE as u16);
        put_u16(
            &mut image,
            field::PROGRAM_HEADER_SIZE,
            PROGRAM_HEADER_SIZE as u16,
        );
        put_u16(
            &mut image,
            field::PROGRAM_HEADER_COUNT,
            headers.len() as u16,
        );
        for (index, header) in headers.iter().enumerate() {
            let at = HEADER_SIZE + index * PROGRAM_HEADER_SIZE;
            let entry = &mut image[at..at + PROGRAM_HEADER_SIZE];
            entry[field::KIND..][..4].copy_from_slice(&header.kind.to_ne_bytes());
            entry[field::SEGMENT_FLAGS..][..4].copy_from_slice(&header.flags.to_ne_bytes());
            put_address(entry, field::OFFSET, header.offset);
            put_address(entry, field::VIRT, header.virt);
            put_address(entry, field::PHYS, header.virt);
            put_address(entry, field::FILE_SIZE, header.file_size);
            put_address(entry, field::MEMORY_SIZE, header.memory_size);
            put_address(entry, field::ALIGN, header.align);
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
        assert_eq!(load(0x1010, 0, 9, 9).file_bytes(&image), None);
        assert_eq!(load(u64::MAX, 0, 2, 2).file_bytes(&image), None);
    }

    fn with_interpreter(path: &[u8]) -> Vec<u8> {
        let interp = ProgramHeader {
            kind: segment::INTERP,
            flags: segment::READ,
            offset: 0x2000,
            virt: 0x40_2000,
            file_size: path.len() as u64,
            memory_size: path.len() as u64,
            align: 1,
        };
        build::image(
            3,
            machine::NATIVE,
            0x1000,
            &[interp, load(0, 0x40_0000, 0x1010, 0x1010)],
            &[(0x2000, path)],
        )
    }

    #[test]
    fn the_interpreter_path_is_read_as_linux_reads_it() {
        let image = sample();
        let elf = Elf::parse(&image, machine::NATIVE).unwrap();
        assert_eq!(elf.interpreter(), Ok(None));
        let image = with_interpreter(b"/lib/ld-musl-x86_64.so.1\0");
        let elf = Elf::parse(&image, machine::NATIVE).unwrap();
        assert_eq!(
            elf.interpreter(),
            Ok(Some(&b"/lib/ld-musl-x86_64.so.1"[..]))
        );
        let image = with_interpreter(b"/lib/ld\0junk\0");
        let elf = Elf::parse(&image, machine::NATIVE).unwrap();
        assert_eq!(elf.interpreter(), Ok(Some(&b"/lib/ld"[..])));
        for refused in [&b"/lib/ld"[..], b"\0", &[b'a'; 4096][..]] {
            let image = with_interpreter(refused);
            let elf = Elf::parse(&image, machine::NATIVE).unwrap();
            assert!(elf.interpreter().is_err(), "{refused:?}");
        }
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
            Err(InvalidElf("an ELF image in the other byte order"))
        );
        assert_eq!(with(6, &[0]), Err(InvalidElf("an unknown ELF version")));
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
        assert_eq!(
            with(56, &MAX_PROGRAM_HEADERS.to_le_bytes()),
            Err(InvalidElf("the program headers are not within the image"))
        );
        assert_eq!(
            with(32, &u64::MAX.to_le_bytes()),
            Err(InvalidElf("the program headers are not within the image"))
        );
        assert_eq!(
            Elf::parse(&good[..HEADER_SIZE - 1], machine::NATIVE).map(|_| ()),
            Err(InvalidElf("not an ELF image"))
        );
    }
}
