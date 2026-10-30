//! A checked reader of 64-bit Mach-O executables and fat (universal) files: the header, the
//! load commands the layer needs and the slice for the running CPU. Every offset, size and
//! count comes from the file and is checked against it before use.

pub const MH_MAGIC_64: u32 = 0xfeed_facf;
pub const FAT_MAGIC: u32 = 0xcafe_babe;
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;
pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
pub const CPU_SUBTYPE_ARM64E: u32 = 2;
pub const MH_EXECUTE: u32 = 2;
pub const MH_PIE: u32 = 0x20_0000;
pub const VM_PROT_READ: u32 = 1;
pub const VM_PROT_WRITE: u32 = 2;
pub const VM_PROT_EXECUTE: u32 = 4;
pub const SG_READ_ONLY: u32 = 0x10;
pub const MAX_IMAGE_SIZE: u64 = 1 << 30;
pub const MAX_FAT_ARCHITECTURES: u32 = 32;
pub const MAX_SEGMENTS: usize = 16;
pub const MAX_LIBRARIES: usize = 32;
pub const LIBSYSTEM: &[u8] = b"/usr/lib/libSystem.B.dylib";
pub const LIBOBJC: &[u8] = b"/usr/lib/libobjc.A.dylib";
pub const FOUNDATION: &[u8] =
    b"/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation";
pub const CORE_FOUNDATION: &[u8] =
    b"/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation";
pub const APPKIT: &[u8] = b"/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit";
pub const COCOA: &[u8] = b"/System/Library/Frameworks/Cocoa.framework/Versions/A/Cocoa";

const HEADER_SIZE: usize = 32;
const FAT_ARCH_SIZE: usize = 20;
const FAT_ARCH_64_SIZE: usize = 32;
const SEGMENT_COMMAND_SIZE: usize = 72;
const SEGMENT_ALIGNMENT: u64 = 4096;
const LC_REQ_DYLD: u32 = 0x8000_0000;
const SECTION_SIZE: usize = 80;
const SECTION_TYPE: u32 = 0xff;
const S_THREAD_LOCAL_REGULAR: u32 = 0x11;
const S_THREAD_LOCAL_ZEROFILL: u32 = 0x12;
const MAX_THREAD_LOCAL_SIZE: u64 = 16 << 20;
const MAX_THREAD_LOCAL_ALIGNMENT_SHIFT: u32 = 12;

mod command {
    pub const UNIXTHREAD: u32 = 0x5;
    pub const LOAD_DYLIB: u32 = 0xc;
    pub const LOAD_DYLINKER: u32 = 0xe;
    pub const SEGMENT_64: u32 = 0x19;
    pub const ENCRYPTION_INFO_64: u32 = 0x2c;
    pub const DYLD_INFO: u32 = 0x22;
    pub const DYLD_INFO_ONLY: u32 = 0x8000_0022;
    pub const LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
    pub const REEXPORT_DYLIB: u32 = 0x8000_001f;
    pub const LAZY_LOAD_DYLIB: u32 = 0x20;
    pub const LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;
    pub const MAIN: u32 = 0x8000_0028;
    pub const DYLD_EXPORTS_TRIE: u32 = 0x8000_0033;
    pub const DYLD_CHAINED_FIXUPS: u32 = 0x8000_0034;
    pub const RPATH: u32 = 0x8000_001c;
}

/// Why a file is not a Mach-O executable the layer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMachO(pub &'static str);

/// The CPU a Mach-O slice is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cpu {
    Arm64,
    Arm64e,
    X86_64,
    Other(u32),
}

impl Cpu {
    fn of(cputype: u32, cpusubtype: u32) -> Self {
        match cputype {
            CPU_TYPE_ARM64 if cpusubtype & 0xff == CPU_SUBTYPE_ARM64E => Self::Arm64e,
            CPU_TYPE_ARM64 => Self::Arm64,
            CPU_TYPE_X86_64 => Self::X86_64,
            other => Self::Other(other),
        }
    }

    /// The name `lipo` and `file` give the CPU.
    pub fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::Arm64e => "arm64e",
            Self::X86_64 => "x86_64",
            Self::Other(0x0000_0007) => "i386",
            Self::Other(0x0000_000c) => "arm",
            Self::Other(0x0000_0012) => "ppc",
            Self::Other(0x0100_0012) => "ppc64",
            Self::Other(_) => "an unknown CPU",
        }
    }
}

/// The CPU this layer runs programs for, if any.
pub const fn host_cpu() -> Option<Cpu> {
    if cfg!(target_arch = "aarch64") {
        Some(Cpu::Arm64)
    } else if cfg!(target_arch = "x86_64") {
        Some(Cpu::X86_64)
    } else {
        None
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn be32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    Some(u32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn be64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    Some(u64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

pub(crate) fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    u32_at(bytes, offset)
}

pub(crate) fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    u64_at(bytes, offset)
}

/// Whether `file` is a fat file rather than, say, a Java class file, which starts with the same
/// magic but whose next word is a class-file version of at least 45.
pub fn is_fat(file: &[u8]) -> bool {
    matches!(be32_at(file, 0), Some(FAT_MAGIC | FAT_MAGIC_64))
        && be32_at(file, 4).is_some_and(|count| (1..=MAX_FAT_ARCHITECTURES).contains(&count))
}

/// One slice of a fat file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub cpu: Cpu,
    pub offset: u64,
    pub size: u64,
}

/// The slices a fat file lists, each checked to lie inside the file.
pub fn fat_slices(
    file: &[u8],
) -> Result<impl Iterator<Item = Result<Slice, InvalidMachO>> + '_, InvalidMachO> {
    let magic = be32_at(file, 0).ok_or(InvalidMachO("truncated fat header"))?;
    if !is_fat(file) {
        return Err(InvalidMachO("not a fat file"));
    }
    let count = be32_at(file, 4).ok_or(InvalidMachO("truncated fat header"))? as usize;
    let entry_size = if magic == FAT_MAGIC_64 {
        FAT_ARCH_64_SIZE
    } else {
        FAT_ARCH_SIZE
    };
    let table_end = count
        .checked_mul(entry_size)
        .and_then(|size| size.checked_add(8))
        .ok_or(InvalidMachO("fat table too large"))?;
    if table_end > file.len() {
        return Err(InvalidMachO("truncated fat table"));
    }
    Ok((0..count).map(move |index| {
        let at = 8 + index * entry_size;
        let read = |offset: usize| be32_at(file, at + offset).unwrap_or(0);
        let cpu = Cpu::of(read(0), read(4));
        let (offset, size) = if magic == FAT_MAGIC_64 {
            (
                be64_at(file, at + 8).unwrap_or(0),
                be64_at(file, at + 16).unwrap_or(0),
            )
        } else {
            (u64::from(read(8)), u64::from(read(12)))
        };
        let end = offset
            .checked_add(size)
            .ok_or(InvalidMachO("fat slice out of range"))?;
        if offset < table_end as u64 || end > file.len() as u64 || size == 0 {
            return Err(InvalidMachO("fat slice outside the file"));
        }
        Ok(Slice { cpu, offset, size })
    }))
}

/// Why no slice of a file runs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSlice {
    Invalid(InvalidMachO),
    /// The file has code only for other CPUs: up to four of them, in the file's order.
    OtherCpus {
        found: [Option<Cpu>; 4],
    },
}

/// The bytes of the 64-bit Mach-O executable in `file` for `cpu`: the file itself for a thin
/// file, or its slice for `cpu` from a fat file.
pub fn select_slice(file: &[u8], cpu: Cpu) -> Result<&[u8], NoSlice> {
    if !is_fat(file) {
        return match u32_at(file, 0) {
            Some(MH_MAGIC_64) => {
                let found = Cpu::of(u32_at(file, 4).unwrap_or(0), u32_at(file, 8).unwrap_or(0));
                if found == cpu {
                    Ok(file)
                } else {
                    Err(NoSlice::OtherCpus {
                        found: [Some(found), None, None, None],
                    })
                }
            }
            _ => Err(NoSlice::Invalid(InvalidMachO(
                "not a 64-bit Mach-O or fat file",
            ))),
        };
    }
    let mut found = [None; 4];
    for (index, slice) in fat_slices(file).map_err(NoSlice::Invalid)?.enumerate() {
        let slice = slice.map_err(NoSlice::Invalid)?;
        if slice.cpu == cpu {
            let start = slice.offset as usize;
            return Ok(&file[start..start + slice.size as usize]);
        }
        if let Some(slot) = found.get_mut(index) {
            *slot = Some(slice.cpu);
        }
    }
    Err(NoSlice::OtherCpus { found })
}

/// A segment (`LC_SEGMENT_64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Segment {
    pub name: [u8; 16],
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub initprot: u32,
    pub flags: u32,
}

impl Segment {
    #[cfg(test)]
    pub fn name(&self) -> &[u8] {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        &self.name[..end]
    }

    fn is_page_zero(&self) -> bool {
        self.vmaddr == 0 && self.filesize == 0 && self.initprot == 0
    }

    pub fn end(&self) -> u64 {
        self.vmaddr + self.vmsize
    }
}

/// A library the executable links (`LC_LOAD_DYLIB` and its kin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Library<'a> {
    pub path: &'a [u8],
    pub weak: bool,
}

/// A range of the slice (`linkedit_data_command`'s `dataoff` and `datasize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub offset: usize,
    pub size: usize,
}

/// The template of the program's thread-local variables: its `__thread_data` and
/// `__thread_bss` sections (`S_THREAD_LOCAL_REGULAR` and `S_THREAD_LOCAL_ZEROFILL`), from the
/// lowest address to the end of the highest, which each thread gets a copy of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadLocalTemplate {
    pub address: u64,
    pub size: u64,
    pub alignment: u64,
}

/// A section's place in the image, at its unslid address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRange {
    pub address: u64,
    pub size: u64,
}

/// The Objective-C metadata sections the runtime reads, found by name in any segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ObjcSections {
    pub image_info: Option<SectionRange>,
    pub class_list: Option<SectionRange>,
    pub non_lazy_class_list: Option<SectionRange>,
    pub category_list: Option<SectionRange>,
    pub non_lazy_category_list: Option<SectionRange>,
    pub selector_references: Option<SectionRange>,
}

impl ObjcSections {
    /// Whether the image carries Objective-C metadata at all.
    pub fn present(&self) -> bool {
        self.image_info.is_some()
            || self.class_list.is_some()
            || self.category_list.is_some()
            || self.selector_references.is_some()
    }

    fn slot(&mut self, name: &[u8]) -> Option<&mut Option<SectionRange>> {
        Some(match name {
            b"__objc_imageinfo" => &mut self.image_info,
            b"__objc_classlist" => &mut self.class_list,
            b"__objc_nlclslist" => &mut self.non_lazy_class_list,
            b"__objc_catlist" => &mut self.category_list,
            b"__objc_nlcatlist" => &mut self.non_lazy_category_list,
            b"__objc_selrefs" => &mut self.selector_references,
            _ => return None,
        })
    }

    fn all(&self) -> [Option<SectionRange>; 6] {
        [
            self.image_info,
            self.class_list,
            self.non_lazy_class_list,
            self.category_list,
            self.non_lazy_category_list,
            self.selector_references,
        ]
    }
}

/// A parsed 64-bit Mach-O executable.
#[derive(Debug, Clone, Copy)]
pub struct MachO<'a> {
    pub bytes: &'a [u8],
    pub cpu: Cpu,
    pub pie: bool,
    segments: [Segment; MAX_SEGMENTS],
    segment_count: usize,
    libraries: [Option<Library<'a>>; MAX_LIBRARIES],
    library_count: usize,
    pub entry: u64,
    pub chained_fixups: Option<Range>,
    pub dyld_info: bool,
    pub thread_locals: Option<ThreadLocalTemplate>,
    pub objc: ObjcSections,
    pub unwind_info: Option<SectionRange>,
}

impl<'a> MachO<'a> {
    /// Parses and checks the executable in `bytes` (one slice).
    pub fn parse(bytes: &'a [u8]) -> Result<Self, InvalidMachO> {
        let invalid = InvalidMachO;
        if u32_at(bytes, 0) != Some(MH_MAGIC_64) {
            return Err(invalid("not a 64-bit Mach-O file"));
        }
        let field = |offset| u32_at(bytes, offset).ok_or(invalid("truncated header"));
        let cpu = Cpu::of(field(4)?, field(8)?);
        if field(12)? != MH_EXECUTE {
            return Err(invalid(
                "not an executable (a library, bundle or object file)",
            ));
        }
        let command_count = field(16)? as usize;
        let commands_size = field(20)? as usize;
        let flags = field(24)?;
        let commands_end = HEADER_SIZE
            .checked_add(commands_size)
            .filter(|&end| end <= bytes.len())
            .ok_or(invalid("load commands outside the file"))?;
        let mut parsed = Self {
            bytes,
            cpu,
            pie: flags & MH_PIE != 0,
            segments: [Segment::default(); MAX_SEGMENTS],
            segment_count: 0,
            libraries: [None; MAX_LIBRARIES],
            library_count: 0,
            entry: 0,
            chained_fixups: None,
            dyld_info: false,
            thread_locals: None,
            objc: ObjcSections::default(),
            unwind_info: None,
        };
        let mut entry = None;
        let mut at = HEADER_SIZE;
        for _ in 0..command_count {
            let kind = u32_at(bytes, at).ok_or(invalid("truncated load command"))?;
            let size = u32_at(bytes, at + 4).ok_or(invalid("truncated load command"))? as usize;
            let end = at
                .checked_add(size)
                .filter(|&end| size >= 8 && size.is_multiple_of(8) && end <= commands_end)
                .ok_or(invalid("a load command runs past the others"))?;
            let command = &bytes[at..end];
            match kind {
                command::SEGMENT_64 => parsed.add_segment(command)?,
                command::MAIN => {
                    entry = Some(u64_at(command, 8).ok_or(invalid("truncated LC_MAIN"))?);
                }
                command::UNIXTHREAD => {
                    return Err(invalid(
                        "starts with LC_UNIXTHREAD (built for Mac OS X 10.7 or earlier), \
                         which EggOS does not run yet",
                    ));
                }
                command::LOAD_DYLIB
                | command::LOAD_WEAK_DYLIB
                | command::REEXPORT_DYLIB
                | command::LAZY_LOAD_DYLIB
                | command::LOAD_UPWARD_DYLIB => {
                    let path = string_in(command, 8).ok_or(invalid("bad library name"))?;
                    let slot = parsed
                        .libraries
                        .get_mut(parsed.library_count)
                        .ok_or(invalid("links too many libraries"))?;
                    *slot = Some(Library {
                        path,
                        weak: kind == command::LOAD_WEAK_DYLIB,
                    });
                    parsed.library_count += 1;
                }
                command::LOAD_DYLINKER => {
                    string_in(command, 8).ok_or(invalid("bad dynamic linker name"))?;
                }
                command::DYLD_CHAINED_FIXUPS => {
                    parsed.chained_fixups = Some(range_in(command, bytes.len())?);
                }
                command::DYLD_EXPORTS_TRIE => {
                    range_in(command, bytes.len())?;
                }
                command::DYLD_INFO | command::DYLD_INFO_ONLY => parsed.dyld_info = true,
                command::ENCRYPTION_INFO_64 => {
                    if u32_at(command, 16).ok_or(invalid("truncated encryption info"))? != 0 {
                        return Err(invalid("is encrypted"));
                    }
                }
                command::RPATH => {}
                other if other & LC_REQ_DYLD != 0 && !is_known_optional(other) => {
                    return Err(invalid("needs a load command EggOS does not know"));
                }
                _ => {}
            }
            at = end;
        }
        parsed.entry = entry.ok_or(invalid("has no LC_MAIN entry point"))?;
        parsed.check_layout()?;
        Ok(parsed)
    }

    fn add_segment(&mut self, command: &[u8]) -> Result<(), InvalidMachO> {
        let invalid = InvalidMachO;
        if command.len() < SEGMENT_COMMAND_SIZE {
            return Err(invalid("truncated segment"));
        }
        let read = |offset| u64_at(command, offset).unwrap_or(0);
        let mut name = [0u8; 16];
        name.copy_from_slice(&command[8..24]);
        let segment = Segment {
            name,
            vmaddr: read(24),
            vmsize: read(32),
            fileoff: read(40),
            filesize: read(48),
            initprot: u32_at(command, 60).unwrap_or(0),
            flags: u32_at(command, 68).unwrap_or(0),
        };
        let sections = u32_at(command, 64).unwrap_or(0) as usize;
        if sections
            .checked_mul(80)
            .and_then(|size| size.checked_add(SEGMENT_COMMAND_SIZE))
            .is_none_or(|size| size > command.len())
        {
            return Err(invalid("a segment's sections run past its command"));
        }
        for index in 0..sections {
            let at = SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
            let kind = u32_at(command, at + 64).unwrap_or(0) & SECTION_TYPE;
            if kind == S_THREAD_LOCAL_REGULAR || kind == S_THREAD_LOCAL_ZEROFILL {
                let address = u64_at(command, at + 32).unwrap_or(0);
                let size = u64_at(command, at + 40).unwrap_or(0);
                let shift = u32_at(command, at + 52).unwrap_or(0);
                self.add_thread_locals(address, size, shift)?;
            }
            let section_name = &command[at..at + 16];
            let section_name =
                &section_name[..section_name.iter().position(|&b| b == 0).unwrap_or(16)];
            let pointer_list = section_name != b"__objc_imageinfo";
            if section_name == b"__unwind_info" {
                let address = u64_at(command, at + 32).unwrap_or(0);
                let size = u64_at(command, at + 40).unwrap_or(0);
                let inside = address >= segment.vmaddr
                    && address
                        .checked_add(size)
                        .zip(segment.vmaddr.checked_add(segment.filesize))
                        .is_some_and(|(end, segment_end)| end <= segment_end);
                if inside {
                    self.unwind_info = Some(SectionRange { address, size });
                }
            }
            if let Some(slot) = self.objc.slot(section_name) {
                let address = u64_at(command, at + 32).unwrap_or(0);
                let size = u64_at(command, at + 40).unwrap_or(0);
                let inside = address >= segment.vmaddr
                    && address
                        .checked_add(size)
                        .zip(segment.vmaddr.checked_add(segment.vmsize))
                        .is_some_and(|(end, segment_end)| end <= segment_end);
                let aligned =
                    !pointer_list || (size.is_multiple_of(8) && address.is_multiple_of(8));
                if !inside || !aligned {
                    return Err(invalid(
                        "an Objective-C section is outside its segment or misaligned",
                    ));
                }
                *slot = Some(SectionRange { address, size });
            }
        }
        if segment.is_page_zero() {
            return Ok(());
        }
        let slot = self
            .segments
            .get_mut(self.segment_count)
            .ok_or(invalid("too many segments"))?;
        *slot = segment;
        self.segment_count += 1;
        Ok(())
    }

    fn add_thread_locals(
        &mut self,
        address: u64,
        size: u64,
        shift: u32,
    ) -> Result<(), InvalidMachO> {
        let invalid = InvalidMachO("a thread-local section is too large or misaligned");
        if shift > MAX_THREAD_LOCAL_ALIGNMENT_SHIFT {
            return Err(invalid);
        }
        let end = address.checked_add(size).ok_or(invalid)?;
        let (start, end, alignment) = match self.thread_locals {
            Some(template) => (
                template.address.min(address),
                (template.address + template.size).max(end),
                template.alignment.max(1 << shift),
            ),
            None => (address, end, 1 << shift),
        };
        if end - start > MAX_THREAD_LOCAL_SIZE {
            return Err(invalid);
        }
        self.thread_locals = Some(ThreadLocalTemplate {
            address: start,
            size: end - start,
            alignment,
        });
        Ok(())
    }

    fn check_layout(&self) -> Result<(), InvalidMachO> {
        let invalid = InvalidMachO;
        let segments = self.segments();
        let text = segments.first().ok_or(invalid("has no segments"))?;
        if text.fileoff != 0 || text.filesize == 0 {
            return Err(invalid("the first segment does not hold the header"));
        }
        let mut previous_end = 0;
        for segment in segments {
            let end = segment
                .vmaddr
                .checked_add(segment.vmsize)
                .ok_or(invalid("a segment wraps around"))?;
            let file_end = segment
                .fileoff
                .checked_add(segment.filesize)
                .ok_or(invalid("a segment's data wraps around"))?;
            if segment.vmaddr % SEGMENT_ALIGNMENT != 0 || segment.vmaddr < previous_end {
                return Err(invalid("segments are not page-aligned and in order"));
            }
            if segment.filesize > segment.vmsize || file_end > self.bytes.len() as u64 {
                return Err(invalid("a segment's data is outside the file"));
            }
            let writable = segment.initprot & VM_PROT_WRITE != 0;
            if writable && segment.initprot & VM_PROT_EXECUTE != 0 {
                return Err(invalid("a segment is both writable and executable"));
            }
            previous_end = end;
        }
        if previous_end - text.vmaddr > MAX_IMAGE_SIZE {
            return Err(invalid("larger than 1 GiB"));
        }
        if !self.pie && text.vmaddr < SEGMENT_ALIGNMENT {
            return Err(invalid("a fixed-address program at address 0"));
        }
        let entry_in_code = segments.iter().any(|segment| {
            segment.initprot & VM_PROT_EXECUTE != 0
                && self.entry >= segment.fileoff
                && self.entry < segment.fileoff + segment.filesize
                && segment.fileoff == segment.vmaddr - text.vmaddr
        });
        if !entry_in_code {
            return Err(invalid("the entry point is not in executable code"));
        }
        if let Some(template) = self.thread_locals {
            let inside = segments.iter().any(|segment| {
                template.address >= segment.vmaddr
                    && template.address + template.size <= segment.end()
                    && segment.initprot & VM_PROT_READ != 0
            });
            if !inside {
                return Err(invalid(
                    "the thread-local template is outside every segment",
                ));
            }
        }
        let mapped = |range: SectionRange| {
            segments.iter().any(|segment| {
                range.address >= segment.vmaddr && range.address + range.size <= segment.end()
            })
        };
        if !self.objc.all().into_iter().flatten().all(mapped) {
            return Err(invalid("an Objective-C section is outside every segment"));
        }
        if let Some(range) = self.chained_fixups {
            let linkedit = segments.iter().any(|segment| {
                range.offset as u64 >= segment.fileoff
                    && (range.offset + range.size) as u64 <= segment.fileoff + segment.filesize
            });
            if !linkedit {
                return Err(invalid("the chained fixups are outside every segment"));
            }
        }
        Ok(())
    }

    /// The segments to map, `__TEXT` first, in address order (`__PAGEZERO` left out).
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.segment_count]
    }

    /// The libraries it links, in load-command order (ordinal 1 first).
    pub fn libraries(&self) -> impl Iterator<Item = Library<'a>> + '_ {
        self.libraries[..self.library_count]
            .iter()
            .flatten()
            .copied()
    }

    /// The library with the 1-based ordinal `ordinal`.
    pub fn library(&self, ordinal: usize) -> Option<Library<'a>> {
        self.libraries[..self.library_count]
            .get(ordinal.checked_sub(1)?)
            .copied()
            .flatten()
    }

    /// The lowest address the image uses: `__TEXT`'s, where the header is.
    pub fn base_address(&self) -> u64 {
        self.segments[0].vmaddr
    }

    /// How many bytes from [`Self::base_address`] the image's segments span.
    pub fn mapped_size(&self) -> u64 {
        let end = self.segments().last().map_or(0, Segment::end);
        (end - self.base_address()).next_multiple_of(SEGMENT_ALIGNMENT)
    }
}

fn is_known_optional(kind: u32) -> bool {
    matches!(
        kind,
        command::LOAD_WEAK_DYLIB
            | command::REEXPORT_DYLIB
            | command::LOAD_UPWARD_DYLIB
            | command::DYLD_INFO_ONLY
            | command::MAIN
            | command::DYLD_EXPORTS_TRIE
            | command::DYLD_CHAINED_FIXUPS
            | command::RPATH
    )
}

fn string_in(command: &[u8], offset_field: usize) -> Option<&[u8]> {
    let offset = u32_at(command, offset_field)? as usize;
    let rest = command.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

fn range_in(command: &[u8], file_size: usize) -> Result<Range, InvalidMachO> {
    let invalid = InvalidMachO("a linkedit range is outside the file");
    let offset = u32_at(command, 8).ok_or(invalid)? as usize;
    let size = u32_at(command, 12).ok_or(invalid)? as usize;
    offset
        .checked_add(size)
        .filter(|&end| end <= file_size)
        .ok_or(invalid)?;
    Ok(Range { offset, size })
}
