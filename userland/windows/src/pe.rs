//! The PE32+ executable format (Microsoft's "PE Format" specification): a checked parser of
//! the headers and section table of an x64 or ARM64 Windows console or GUI executable. Every offset
//! and size read from the file is checked against the file and the image before it is used, so
//! a malformed file is refused with a reason, never a panic.

pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;
pub const OPTIONAL_HEADER_MAGIC_PE32_PLUS: u16 = 0x20b;
pub const SUBSYSTEM_WINDOWS_GUI: u16 = 2;
pub const SUBSYSTEM_WINDOWS_CONSOLE: u16 = 3;
pub const PAGE_SIZE: u64 = 4096;
pub const MAX_IMAGE_SIZE: u64 = 1 << 30;
pub const MAX_SECTIONS: usize = 96;
pub const USER_ADDRESS_LIMIT: u64 = 1 << 47;

pub mod characteristics {
    pub const RELOCATIONS_STRIPPED: u16 = 0x0001;
    pub const EXECUTABLE_IMAGE: u16 = 0x0002;
    pub const DLL: u16 = 0x2000;
}

pub mod section_flags {
    pub const MEMORY_EXECUTE: u32 = 0x2000_0000;
    pub const MEMORY_READ: u32 = 0x4000_0000;
    pub const MEMORY_WRITE: u32 = 0x8000_0000;
}

pub mod directory {
    #[cfg(any(test, target_arch = "aarch64"))]
    pub const EXPORT: usize = 0;
    pub const IMPORT: usize = 1;
    pub const EXCEPTION: usize = 3;
    pub const BASE_RELOCATION: usize = 5;
    pub const TLS: usize = 9;
    pub const COUNT: usize = 16;
}

const DOS_HEADER_SIZE: usize = 64;
const NEW_HEADER_OFFSET_FIELD: usize = 0x3c;
const COFF_HEADER_SIZE: usize = 20;
const OPTIONAL_HEADER_FIXED_SIZE: usize = 112;
const SECTION_HEADER_SIZE: usize = 40;

/// Why a file is not a Windows program the layer can start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPe(pub &'static str);

/// A data directory: where a table is in the image, as a relative virtual address and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Directory {
    pub address: u32,
    pub size: u32,
}

/// A section's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_size: u32,
    pub raw_offset: u32,
    pub flags: u32,
}

impl Section {
    /// The bytes the section takes in the image.
    pub fn memory_size(&self) -> u32 {
        match self.virtual_size {
            0 => self.raw_size,
            size => size,
        }
    }

    /// The bytes of the file copied to the section's start; the rest of it is zero.
    pub fn file_bytes(&self) -> u32 {
        self.raw_size.min(self.memory_size())
    }
}

/// The CPU a program's machine code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    /// x64 (`IMAGE_FILE_MACHINE_AMD64`), which runs on x86_64.
    X64,
    /// ARM64 (`IMAGE_FILE_MACHINE_ARM64`), which runs on aarch64.
    Arm64,
}

impl Machine {
    fn from_field(machine: u16) -> Option<Self> {
        match machine {
            MACHINE_AMD64 => Some(Self::X64),
            MACHINE_ARM64 => Some(Self::Arm64),
            _ => None,
        }
    }

    /// Why a program for this CPU cannot run on a computer whose CPU is `computer`, if it
    /// cannot: the layer runs each program's code directly, never emulated (D17), and an x64
    /// program on an ARM64 computer runs translated ahead of time (D191).
    pub fn refusal_on(self, computer: Machine) -> Option<InvalidPe> {
        match (self, computer) {
            (Self::Arm64, Self::X64) => Some(InvalidPe(
                "an ARM64 program, and this computer runs x64 programs",
            )),
            _ => None,
        }
    }

    /// Whether a program for this CPU runs translated on a computer whose CPU is `computer`:
    /// an x64 program on ARM64 (D191).
    pub fn translated_on(self, computer: Machine) -> bool {
        matches!((self, computer), (Self::X64, Self::Arm64))
    }
}

/// The subsystem a program was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    Console,
    Gui,
}

/// A checked PE32+ executable for x64 or ARM64 with the console or GUI subsystem.
#[derive(Debug, Clone, Copy)]
pub struct Pe<'a> {
    pub file: &'a [u8],
    pub machine: Machine,
    pub subsystem: Subsystem,
    pub characteristics: u16,
    pub entry: u32,
    pub image_base: u64,
    pub section_alignment: u32,
    pub image_size: u32,
    pub headers_size: u32,
    directories: [Directory; directory::COUNT],
    section_table: usize,
    section_count: usize,
}

impl<'a> Pe<'a> {
    /// Checks `file`'s headers and section table.
    pub fn parse(file: &'a [u8]) -> Result<Self, InvalidPe> {
        if file.len() < DOS_HEADER_SIZE || !file.starts_with(b"MZ") {
            return Err(InvalidPe("not a Windows program (no MZ header)"));
        }
        let new_header = read_u32(file, NEW_HEADER_OFFSET_FIELD)? as usize;
        if file.get(new_header..new_header.saturating_add(4)) != Some(b"PE\0\0") {
            return Err(InvalidPe("no PE signature: an MS-DOS program"));
        }
        let coff = new_header + 4;
        let machine = read_u16(file, coff)?;
        let section_count = usize::from(read_u16(file, coff + 2)?);
        let optional_size = usize::from(read_u16(file, coff + 16)?);
        let characteristics = read_u16(file, coff + 18)?;
        let machine =
            Machine::from_field(machine).ok_or(InvalidPe("not an x64 or ARM64 program"))?;
        if characteristics & characteristics::EXECUTABLE_IMAGE == 0 {
            return Err(InvalidPe("not an executable image"));
        }
        if characteristics & characteristics::DLL != 0 {
            return Err(InvalidPe("a DLL, not a program"));
        }
        if section_count == 0 || section_count > MAX_SECTIONS {
            return Err(InvalidPe("no sections or too many sections"));
        }
        let optional = coff + COFF_HEADER_SIZE;
        let optional_end = optional
            .checked_add(optional_size)
            .filter(|end| *end <= file.len())
            .ok_or(InvalidPe("the optional header is past the end of the file"))?;
        if optional_size < OPTIONAL_HEADER_FIXED_SIZE {
            return Err(InvalidPe("the optional header is too short"));
        }
        if read_u16(file, optional)? != OPTIONAL_HEADER_MAGIC_PE32_PLUS {
            return Err(InvalidPe("not a PE32+ (64-bit) program"));
        }
        let entry = read_u32(file, optional + 16)?;
        let image_base = read_u64(file, optional + 24)?;
        let section_alignment = read_u32(file, optional + 32)?;
        let file_alignment = read_u32(file, optional + 36)?;
        let image_size = read_u32(file, optional + 56)?;
        let headers_size = read_u32(file, optional + 60)?;
        let subsystem = read_u16(file, optional + 68)?;
        let directory_count = read_u32(file, optional + 108)? as usize;
        let subsystem = match subsystem {
            SUBSYSTEM_WINDOWS_CONSOLE => Subsystem::Console,
            SUBSYSTEM_WINDOWS_GUI => Subsystem::Gui,
            _ => {
                return Err(InvalidPe(
                    "not a console or GUI program (drivers and other subsystems do not run)",
                ));
            }
        };
        if !section_alignment.is_power_of_two() || u64::from(section_alignment) < PAGE_SIZE {
            return Err(InvalidPe(
                "section alignment is not a power of two of at least a page",
            ));
        }
        if !file_alignment.is_power_of_two() || file_alignment > section_alignment {
            return Err(InvalidPe(
                "file alignment is not a power of two up to the sections'",
            ));
        }
        if image_base % 0x1_0000 != 0 {
            return Err(InvalidPe("the image base is not 64 KiB aligned"));
        }
        if image_base.saturating_add(u64::from(image_size)) > USER_ADDRESS_LIMIT {
            return Err(InvalidPe("the image base is outside user space"));
        }
        if image_size == 0 || u64::from(image_size) > MAX_IMAGE_SIZE {
            return Err(InvalidPe("the image size is zero or larger than 1 GiB"));
        }
        if headers_size == 0 || headers_size > image_size || headers_size as usize > file.len() {
            return Err(InvalidPe(
                "the headers' size is outside the image or the file",
            ));
        }
        if directory_count > (optional_size - OPTIONAL_HEADER_FIXED_SIZE) / 8 {
            return Err(InvalidPe(
                "more data directories than the optional header holds",
            ));
        }
        let mut directories = [Directory::default(); directory::COUNT];
        for (index, entry) in directories
            .iter_mut()
            .enumerate()
            .take(directory_count.min(directory::COUNT))
        {
            let at = optional + OPTIONAL_HEADER_FIXED_SIZE + index * 8;
            *entry = Directory {
                address: read_u32(file, at)?,
                size: read_u32(file, at + 4)?,
            };
        }
        let section_table = optional_end;
        if section_table + section_count * SECTION_HEADER_SIZE > file.len() {
            return Err(InvalidPe("the section table is past the end of the file"));
        }
        let pe = Self {
            file,
            machine,
            subsystem,
            characteristics,
            entry,
            image_base,
            section_alignment,
            image_size,
            headers_size,
            directories,
            section_table,
            section_count,
        };
        pe.check_sections()?;
        pe.check_directories()?;
        Ok(pe)
    }

    /// The image's size rounded up to whole pages: what is mapped.
    pub fn mapped_size(&self) -> u64 {
        u64::from(self.image_size).next_multiple_of(PAGE_SIZE)
    }

    /// Data directory `index` ([`directory`]); empty if the file has none.
    pub fn directory(&self, index: usize) -> Directory {
        self.directories.get(index).copied().unwrap_or_default()
    }

    /// The section headers, in file order.
    pub fn sections(&self) -> impl Iterator<Item = Section> + '_ {
        (0..self.section_count).map(move |index| {
            let at = self.section_table + index * SECTION_HEADER_SIZE;
            let field = |offset| read_u32(self.file, at + offset).unwrap_or(0);
            let mut name = [0; 8];
            name.copy_from_slice(&self.file[at..at + 8]);
            Section {
                name,
                virtual_size: field(8),
                virtual_address: field(12),
                raw_size: field(16),
                raw_offset: field(20),
                flags: field(36),
            }
        })
    }

    /// Whether the image can run at another base than its preferred one.
    pub fn relocatable(&self) -> bool {
        self.characteristics & characteristics::RELOCATIONS_STRIPPED == 0
            && self.directory(directory::BASE_RELOCATION).size != 0
    }

    fn check_sections(&self) -> Result<(), InvalidPe> {
        let mut previous_end = u64::from(self.headers_size);
        let mut entry_executable = false;
        for section in self.sections() {
            let start = u64::from(section.virtual_address);
            let end = start + u64::from(section.memory_size());
            if start % u64::from(self.section_alignment) != 0 {
                return Err(InvalidPe(
                    "a section is not aligned to the section alignment",
                ));
            }
            if start < previous_end {
                return Err(InvalidPe("sections overlap or are out of order"));
            }
            if end > u64::from(self.image_size) {
                return Err(InvalidPe("a section ends past the image"));
            }
            let raw_end = u64::from(section.raw_offset) + u64::from(section.file_bytes());
            if section.file_bytes() != 0 && raw_end > self.file.len() as u64 {
                return Err(InvalidPe("a section's data is past the end of the file"));
            }
            let flags = section.flags;
            if flags & section_flags::MEMORY_WRITE != 0
                && flags & section_flags::MEMORY_EXECUTE != 0
            {
                return Err(InvalidPe("a section is both writable and executable"));
            }
            let entry = u64::from(self.entry);
            if flags & section_flags::MEMORY_EXECUTE != 0 && (start..end).contains(&entry) {
                entry_executable = true;
            }
            previous_end = end.next_multiple_of(u64::from(self.section_alignment));
        }
        match entry_executable {
            true => Ok(()),
            false => Err(InvalidPe("the entry point is not in an executable section")),
        }
    }

    fn check_directories(&self) -> Result<(), InvalidPe> {
        for index in [
            directory::IMPORT,
            directory::EXCEPTION,
            directory::BASE_RELOCATION,
            directory::TLS,
        ] {
            let Directory { address, size } = self.directory(index);
            if size != 0 && u64::from(address) + u64::from(size) > u64::from(self.image_size) {
                return Err(InvalidPe("a data directory is outside the image"));
            }
        }
        Ok(())
    }
}

pub fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, InvalidPe> {
    bytes
        .get(offset..offset.wrapping_add(2))
        .and_then(|field| field.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(InvalidPe("a header is past the end of the file"))
}

pub fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, InvalidPe> {
    bytes
        .get(offset..offset.wrapping_add(4))
        .and_then(|field| field.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(InvalidPe("a header is past the end of the file"))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, InvalidPe> {
    bytes
        .get(offset..offset.wrapping_add(8))
        .and_then(|field| field.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(InvalidPe("a header is past the end of the file"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const TEST_IMAGE_BASE: u64 = 0x1_4000_0000;
    pub const TEXT: u32 = 0x1000;
    pub const RDATA: u32 = 0x2000;
    pub const DATA: u32 = 0x3000;
    pub const IMAGE_SIZE: u32 = 0x4000;
    const PE_OFFSET: usize = 0x80;
    pub const OPTIONAL: usize = PE_OFFSET + 24;
    pub const SECTION_TABLE: usize = OPTIONAL + 240;
    const FILE_ALIGNMENT: usize = 0x200;

    pub fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
        bytes[at..at + value.len()].copy_from_slice(value);
    }

    pub struct Builder {
        pub file: Vec<u8>,
    }

    impl Builder {
        pub fn new() -> Self {
            let mut file = vec![0u8; 0x400 + 3 * FILE_ALIGNMENT];
            put(&mut file, 0, b"MZ");
            put(&mut file, 0x3c, &(PE_OFFSET as u32).to_le_bytes());
            put(&mut file, PE_OFFSET, b"PE\0\0");
            let coff = PE_OFFSET + 4;
            put(&mut file, coff, &MACHINE_AMD64.to_le_bytes());
            put(&mut file, coff + 2, &3u16.to_le_bytes());
            put(&mut file, coff + 16, &240u16.to_le_bytes());
            put(&mut file, coff + 18, &0x22u16.to_le_bytes());
            put(
                &mut file,
                OPTIONAL,
                &OPTIONAL_HEADER_MAGIC_PE32_PLUS.to_le_bytes(),
            );
            put(&mut file, OPTIONAL + 16, &TEXT.to_le_bytes());
            put(&mut file, OPTIONAL + 24, &TEST_IMAGE_BASE.to_le_bytes());
            put(&mut file, OPTIONAL + 32, &0x1000u32.to_le_bytes());
            put(
                &mut file,
                OPTIONAL + 36,
                &(FILE_ALIGNMENT as u32).to_le_bytes(),
            );
            put(&mut file, OPTIONAL + 56, &IMAGE_SIZE.to_le_bytes());
            put(&mut file, OPTIONAL + 60, &0x400u32.to_le_bytes());
            put(
                &mut file,
                OPTIONAL + 68,
                &SUBSYSTEM_WINDOWS_CONSOLE.to_le_bytes(),
            );
            put(&mut file, OPTIONAL + 72, &0x10_0000u64.to_le_bytes());
            put(&mut file, OPTIONAL + 108, &16u32.to_le_bytes());
            let sections = [
                (b".text\0\0\0", TEXT, 0x60000020u32),
                (b".rdata\0\0", RDATA, 0x40000040),
                (b".data\0\0\0", DATA, 0xc0000040),
            ];
            for (index, (name, address, flags)) in sections.into_iter().enumerate() {
                let at = SECTION_TABLE + index * 40;
                put(&mut file, at, name);
                put(&mut file, at + 8, &0x100u32.to_le_bytes());
                put(&mut file, at + 12, &address.to_le_bytes());
                put(&mut file, at + 16, &(FILE_ALIGNMENT as u32).to_le_bytes());
                let raw = 0x400 + index * FILE_ALIGNMENT;
                put(&mut file, at + 20, &(raw as u32).to_le_bytes());
                put(&mut file, at + 36, &flags.to_le_bytes());
                file[raw] = 0xc3 - index as u8;
            }
            Self { file }
        }

        pub fn directory(mut self, index: usize, address: u32, size: u32) -> Self {
            let at = OPTIONAL + 112 + index * 8;
            put(&mut self.file, at, &address.to_le_bytes());
            put(&mut self.file, at + 4, &size.to_le_bytes());
            self
        }

        pub fn section_data(mut self, section: usize, offset: usize, bytes: &[u8]) -> Self {
            let at = 0x400 + section * FILE_ALIGNMENT + offset;
            put(&mut self.file, at, bytes);
            self
        }

        pub fn with(mut self, change: impl FnOnce(&mut Vec<u8>)) -> Self {
            change(&mut self.file);
            self
        }
    }

    #[test]
    fn a_console_executable_s_headers_and_sections_are_read() {
        let builder = Builder::new();
        let pe = Pe::parse(&builder.file).unwrap();
        assert_eq!(pe.image_base, TEST_IMAGE_BASE);
        assert_eq!(pe.entry, TEXT);
        assert_eq!(pe.subsystem, Subsystem::Console);
        assert_eq!(pe.mapped_size(), 0x4000);
        let sections: Vec<Section> = pe.sections().collect();
        assert_eq!(sections.len(), 3);
        assert_eq!(&sections[1].name, b".rdata\0\0");
        assert_eq!(sections[2].virtual_address, DATA);
        assert_eq!(sections[2].file_bytes(), 0x100);
        assert!(!pe.relocatable());
        let relocatable = Builder::new().directory(directory::BASE_RELOCATION, RDATA, 12);
        assert!(Pe::parse(&relocatable.file).unwrap().relocatable());
        let gui =
            Builder::new().with(|f| put(f, OPTIONAL + 68, &SUBSYSTEM_WINDOWS_GUI.to_le_bytes()));
        assert_eq!(Pe::parse(&gui.file).unwrap().subsystem, Subsystem::Gui);
    }

    #[test]
    fn x64_programs_run_on_both_cpus_and_arm64_programs_only_on_arm64() {
        let x64 = Builder::new();
        let arm64 = Builder::new().with(|f| put(f, PE_OFFSET + 4, &MACHINE_ARM64.to_le_bytes()));
        let x64 = Pe::parse(&x64.file).unwrap().machine;
        let arm64 = Pe::parse(&arm64.file).unwrap().machine;
        assert_eq!((x64, arm64), (Machine::X64, Machine::Arm64));
        assert_eq!(x64.refusal_on(Machine::X64), None);
        assert_eq!(arm64.refusal_on(Machine::Arm64), None);
        assert_eq!(x64.refusal_on(Machine::Arm64), None);
        assert!(x64.translated_on(Machine::Arm64));
        assert!(!x64.translated_on(Machine::X64));
        assert!(!arm64.translated_on(Machine::Arm64));
        let arm64_on_x64 = arm64.refusal_on(Machine::X64).unwrap().0;
        assert_eq!(
            arm64_on_x64,
            "an ARM64 program, and this computer runs x64 programs"
        );
    }

    fn refused(change: impl FnOnce(&mut Vec<u8>)) -> &'static str {
        let builder = Builder::new().with(change);
        Pe::parse(&builder.file).unwrap_err().0
    }

    fn put_u32(file: &mut [u8], at: usize, value: u32) {
        put(file, at, &value.to_le_bytes());
    }

    #[test]
    fn malformed_headers_are_refused_with_a_reason() {
        assert!(refused(|f| f.truncate(10)).contains("MZ"));
        assert!(refused(|f| put_u32(f, 0x3c, u32::MAX)).contains("PE signature"));
        assert!(refused(|f| put_u32(f, 0x3c, 0x3fe)).contains("PE signature"));
        for machine in [0x14cu16, 0x1c4, 0xa641, 0x5064, 0] {
            let reason = refused(|f| put(f, PE_OFFSET + 4, &machine.to_le_bytes()));
            assert_eq!(reason, "not an x64 or ARM64 program", "{machine:#x}");
        }
        assert!(refused(|f| put(f, PE_OFFSET + 22, &0x2022u16.to_le_bytes())).contains("DLL"));
        assert!(refused(|f| put(f, PE_OFFSET + 22, &0u16.to_le_bytes())).contains("executable"));
        assert!(refused(|f| put(f, PE_OFFSET + 6, &0u16.to_le_bytes())).contains("sections"));
        assert!(refused(|f| put(f, PE_OFFSET + 6, &500u16.to_le_bytes())).contains("sections"));
        assert!(refused(|f| put(f, PE_OFFSET + 20, &0xffffu16.to_le_bytes())).contains("optional"));
        assert!(refused(|f| put(f, PE_OFFSET + 20, &64u16.to_le_bytes())).contains("too short"));
        assert!(refused(|f| put(f, OPTIONAL, &0x10bu16.to_le_bytes())).contains("PE32+"));
        assert!(refused(|f| put(f, OPTIONAL + 68, &1u16.to_le_bytes())).contains("GUI"));
        assert!(refused(|f| put(f, OPTIONAL + 68, &10u16.to_le_bytes())).contains("console"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 32, 0x200)).contains("section alignment"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 32, 0x3000)).contains("section alignment"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 36, 0x2000)).contains("file alignment"));
        assert!(refused(|f| put(f, OPTIONAL + 24, &0x1234u64.to_le_bytes())).contains("base"));
        assert!(
            refused(|f| put(f, OPTIONAL + 24, &(u64::MAX - 0xffff).to_le_bytes())).contains("base")
        );
        assert!(refused(|f| put_u32(f, OPTIONAL + 56, 0)).contains("image size"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 56, u32::MAX)).contains("image size"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 60, 0x8000)).contains("headers"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 108, 100)).contains("data directories"));
    }

    #[test]
    fn malformed_sections_and_directories_are_refused() {
        let section = |index: usize, field: usize| SECTION_TABLE + index * 40 + field;
        assert!(refused(|f| put(f, PE_OFFSET + 6, &60u16.to_le_bytes())).contains("section table"));
        assert!(refused(|f| f.truncate(SECTION_TABLE + 50)).contains("headers"));
        assert!(refused(|f| put_u32(f, section(1, 12), 0x2100)).contains("aligned"));
        assert!(refused(|f| put_u32(f, section(1, 12), 0x1000)).contains("overlap"));
        assert!(refused(|f| put_u32(f, section(0, 12), 0)).contains("overlap"));
        assert!(refused(|f| put_u32(f, section(2, 8), 0x1001)).contains("past the image"));
        assert!(refused(|f| put_u32(f, section(2, 8), u32::MAX)).contains("past the image"));
        assert!(refused(|f| put_u32(f, section(2, 20), u32::MAX)).contains("end of the file"));
        assert!(refused(|f| put_u32(f, section(2, 36), 0xe0000040)).contains("writable and"));
        assert!(refused(|f| put_u32(f, section(0, 36), 0x40000040)).contains("entry point"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 16, RDATA)).contains("entry point"));
        assert!(refused(|f| put_u32(f, OPTIONAL + 16, u32::MAX)).contains("entry point"));
        let import = OPTIONAL + 112 + directory::IMPORT * 8;
        assert!(
            refused(|f| {
                put_u32(f, import, 0x3f00);
                put_u32(f, import + 4, 0x200);
            })
            .contains("data directory")
        );
        assert!(
            refused(|f| {
                put_u32(f, import, u32::MAX);
                put_u32(f, import + 4, u32::MAX);
            })
            .contains("data directory")
        );
    }

    #[test]
    fn every_truncation_and_single_byte_change_is_refused_or_parsed_without_panicking() {
        let good = Builder::new().file;
        for length in 0..good.len() {
            let _ = Pe::parse(&good[..length]);
        }
        for at in 0..0x400 {
            for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut file = good.clone();
                file[at] = value;
                if let Ok(pe) = Pe::parse(&file) {
                    for section in pe.sections() {
                        assert!(
                            u64::from(section.virtual_address) + u64::from(section.memory_size())
                                <= pe.mapped_size()
                        );
                    }
                }
            }
        }
    }
}
