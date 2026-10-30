//! EggOS's own `libEGL.so.1`, `libGL.so.1` and `libGLESv2.so.2` for Linux apps whose toolkit is
//! linked against them (D179): EggOS has no GPU stack yet, so every call answers that no display,
//! configuration, context or surface exists, as a system without a GPU driver answers, and the
//! toolkit draws with its own raster engine into `wl_shm` buffers. musl's loader resolves every
//! symbol a library names when it loads it, so the toolkit cannot start unless these names
//! exist; Alpine's own builds of them bring Mesa, LLVM and about 235 MB that EggOS cannot run.
//!
//! Each library is written here, byte by byte, as an ELF-64 shared object (System V gABI): one
//! read-and-execute segment with the symbol hash table, the dynamic symbols, their names and
//! one short function per symbol that returns its fixed answer, and one read-and-write segment
//! with the dynamic section naming the library (`DT_SONAME`), with section headers so that
//! `readelf` and `objdump` read it too.

/// One exported function: its name and the integer it returns (in `eax`/`rax` on x86_64, in
/// `w0`/`x0` on AArch64), 0 meaning `EGL_FALSE`, `EGL_NO_DISPLAY`, a null pointer or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub name: &'static str,
    pub returns: u16,
}

/// One library: the name it is loaded by and the functions it exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Library {
    pub soname: &'static str,
    pub functions: &'static [Function],
}

const fn returning_zero(name: &'static str) -> Function {
    Function { name, returns: 0 }
}

const fn returning(name: &'static str, returns: u16) -> Function {
    Function { name, returns }
}

/// `EGL_NOT_INITIALIZED`: what `eglGetError` answers, as no display was ever initialised.
pub const EGL_NOT_INITIALIZED: u16 = 0x3001;
/// `EGL_TRUE`: what `eglReleaseThread` answers, as a thread with nothing current releases
/// nothing and succeeds.
pub const EGL_TRUE: u16 = 1;
/// `GLX_NO_EXTENSION`: what GLX's queries answer for a display without GLX.
pub const GLX_NO_EXTENSION: u16 = 3;

/// EGL 1.5's entry points, sorted by name.
pub const EGL: Library = Library {
    soname: "libEGL.so.1",
    functions: &[
        returning_zero("eglBindAPI"),
        returning_zero("eglBindTexImage"),
        returning_zero("eglChooseConfig"),
        returning_zero("eglCopyBuffers"),
        returning_zero("eglCreateContext"),
        returning_zero("eglCreatePbufferFromClientBuffer"),
        returning_zero("eglCreatePbufferSurface"),
        returning_zero("eglCreatePixmapSurface"),
        returning_zero("eglCreatePlatformPixmapSurface"),
        returning_zero("eglCreatePlatformWindowSurface"),
        returning_zero("eglCreateWindowSurface"),
        returning_zero("eglDestroyContext"),
        returning_zero("eglDestroySurface"),
        returning_zero("eglGetConfigAttrib"),
        returning_zero("eglGetConfigs"),
        returning_zero("eglGetCurrentContext"),
        returning_zero("eglGetCurrentDisplay"),
        returning_zero("eglGetCurrentSurface"),
        returning_zero("eglGetDisplay"),
        returning("eglGetError", EGL_NOT_INITIALIZED),
        returning_zero("eglGetPlatformDisplay"),
        returning_zero("eglGetProcAddress"),
        returning_zero("eglInitialize"),
        returning_zero("eglMakeCurrent"),
        returning_zero("eglQueryAPI"),
        returning_zero("eglQueryContext"),
        returning_zero("eglQueryString"),
        returning_zero("eglQuerySurface"),
        returning("eglReleaseThread", EGL_TRUE),
        returning_zero("eglReleaseTexImage"),
        returning_zero("eglSurfaceAttrib"),
        returning_zero("eglSwapBuffers"),
        returning_zero("eglSwapInterval"),
        returning_zero("eglTerminate"),
        returning_zero("eglWaitClient"),
        returning_zero("eglWaitGL"),
        returning_zero("eglWaitNative"),
    ],
};

/// OpenGL with GLX: the queries a toolkit makes before it creates a context, sorted by name.
pub const GL: Library = Library {
    soname: "libGL.so.1",
    functions: &[
        returning_zero("glFinish"),
        returning_zero("glFlush"),
        returning_zero("glGetError"),
        returning_zero("glGetIntegerv"),
        returning_zero("glGetString"),
        returning_zero("glXChooseFBConfig"),
        returning_zero("glXChooseVisual"),
        returning_zero("glXCreateContext"),
        returning_zero("glXDestroyContext"),
        returning("glXGetConfig", GLX_NO_EXTENSION),
        returning("glXGetFBConfigAttrib", GLX_NO_EXTENSION),
        returning_zero("glXGetProcAddress"),
        returning_zero("glXGetProcAddressARB"),
        returning_zero("glXGetVisualFromFBConfig"),
        returning_zero("glXMakeCurrent"),
        returning_zero("glXQueryExtension"),
        returning_zero("glXQueryVersion"),
        returning_zero("glXSwapBuffers"),
    ],
};

/// OpenGL ES 2 and 3: the queries a toolkit makes before it creates a context, sorted by name.
pub const GLES: Library = Library {
    soname: "libGLESv2.so.2",
    functions: &[
        returning_zero("glFinish"),
        returning_zero("glFlush"),
        returning_zero("glGetError"),
        returning_zero("glGetIntegerv"),
        returning_zero("glGetString"),
    ],
};

/// Every library, in the order an app's folder lists them.
pub const LIBRARIES: [Library; 3] = [EGL, GL, GLES];

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const SECTION_HEADER_SIZE: usize = 64;
const SYMBOL_SIZE: usize = 24;
const DYNAMIC_ENTRY_SIZE: usize = 16;
const FUNCTION_SIZE: usize = 16;
const PROGRAM_HEADERS: usize = 4;
const SEGMENT_ALIGN: u64 = 0x1_0000;

const TYPE_SHARED_OBJECT: u16 = 3;
const MACHINE_X86_64: u16 = 62;
const MACHINE_AARCH64: u16 = 183;

mod segment {
    pub const LOAD: u32 = 1;
    pub const DYNAMIC: u32 = 2;
    pub const GNU_STACK: u32 = 0x6474_e551;
    pub const EXECUTE: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const READ: u32 = 4;
}

mod section {
    pub const STRING_TABLE: u32 = 3;
    pub const HASH: u32 = 5;
    pub const DYNAMIC: u32 = 6;
    pub const PROGRAM_BITS: u32 = 1;
    pub const DYNAMIC_SYMBOLS: u32 = 11;
    pub const WRITE: u64 = 1;
    pub const ALLOCATED: u64 = 2;
    pub const EXECUTABLE: u64 = 4;
}

mod tag {
    pub const NULL: u64 = 0;
    pub const HASH: u64 = 4;
    pub const STRING_TABLE: u64 = 5;
    pub const SYMBOL_TABLE: u64 = 6;
    pub const STRING_TABLE_SIZE: u64 = 10;
    pub const SYMBOL_SIZE: u64 = 11;
    pub const SONAME: u64 = 14;
}

const GLOBAL_FUNCTION: u8 = 0x12;
const SECTION_NAMES: [&str; 7] = [
    "",
    ".hash",
    ".dynsym",
    ".dynstr",
    ".text",
    ".dynamic",
    ".shstrtab",
];
const TEXT_SECTION: u16 = 4;

/// The CPU a library is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    X86_64,
    Aarch64,
}

impl Machine {
    /// The machine called `arch` in xtask's architecture names.
    pub fn of(arch: &str) -> Result<Self, String> {
        match arch {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            other => Err(format!("{other}: no GL libraries are written for it")),
        }
    }

    fn number(self) -> u16 {
        match self {
            Self::X86_64 => MACHINE_X86_64,
            Self::Aarch64 => MACHINE_AARCH64,
        }
    }

    fn function(self, returns: u16) -> [u8; FUNCTION_SIZE] {
        let mut code = [0; FUNCTION_SIZE];
        match self {
            Self::X86_64 => {
                code.fill(0xcc);
                code[0] = 0xb8;
                code[1..5].copy_from_slice(&u32::from(returns).to_le_bytes());
                code[5] = 0xc3;
            }
            Self::Aarch64 => {
                let move_wide = 0x5280_0000u32 | (u32::from(returns) << 5);
                let ret = 0xd65f_03c0u32;
                code[0..4].copy_from_slice(&move_wide.to_le_bytes());
                code[4..8].copy_from_slice(&ret.to_le_bytes());
            }
        }
        code
    }
}

/// The System V hash of a symbol's name (`elf_hash`), which `DT_HASH`'s table is built on.
pub fn sysv_hash(name: &[u8]) -> u32 {
    let mut hash = 0u32;
    for &byte in name {
        hash = (hash << 4).wrapping_add(u32::from(byte));
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

struct Strings {
    bytes: Vec<u8>,
}

impl Strings {
    fn new() -> Self {
        Self { bytes: vec![0] }
    }

    fn add(&mut self, text: &str) -> u32 {
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
        at
    }
}

struct Layout {
    hash: usize,
    symbols: usize,
    strings: usize,
    text: usize,
    text_end: usize,
    dynamic: usize,
    dynamic_end: usize,
    section_names: usize,
    section_headers: usize,
    end: usize,
}

impl Layout {
    fn of(symbols: usize, hash_bytes: usize, string_bytes: usize, names_bytes: usize) -> Self {
        let hash = ELF_HEADER_SIZE + PROGRAM_HEADERS * PROGRAM_HEADER_SIZE;
        let symbol_table = align(hash + hash_bytes, 8);
        let strings = symbol_table + symbols * SYMBOL_SIZE;
        let text = align(strings + string_bytes, FUNCTION_SIZE);
        let text_end = text + (symbols - 1) * FUNCTION_SIZE;
        let dynamic = align(text_end, 8);
        let dynamic_end = dynamic + DYNAMIC_ENTRIES * DYNAMIC_ENTRY_SIZE;
        let section_names = dynamic_end;
        let section_headers = align(section_names + names_bytes, 8);
        let end = section_headers + SECTION_NAMES.len() * SECTION_HEADER_SIZE;
        Self {
            hash,
            symbols: symbol_table,
            strings,
            text,
            text_end,
            dynamic,
            dynamic_end,
            section_names,
            section_headers,
            end,
        }
    }

    fn data_address(&self, offset: usize) -> u64 {
        offset as u64 + SEGMENT_ALIGN
    }
}

const DYNAMIC_ENTRIES: usize = 7;

fn hash_table(names: &[&str]) -> Vec<u8> {
    let count = names.len() + 1;
    let buckets = names.len().max(1);
    let mut bucket = vec![0u32; buckets];
    let mut chain = vec![0u32; count];
    for (index, name) in names.iter().enumerate() {
        let symbol = index + 1;
        let slot = sysv_hash(name.as_bytes()) as usize % buckets;
        chain[symbol] = bucket[slot];
        bucket[slot] = symbol as u32;
    }
    let mut table = Vec::with_capacity(4 * (2 + buckets + count));
    for word in [buckets as u32, count as u32]
        .into_iter()
        .chain(bucket)
        .chain(chain)
    {
        table.extend_from_slice(&word.to_le_bytes());
    }
    table
}

struct SectionHeader {
    name: u32,
    kind: u32,
    flags: u64,
    address: u64,
    offset: usize,
    size: usize,
    link: u32,
    info: u32,
    align: u64,
    entry_size: u64,
}

fn write_section(bytes: &mut [u8], at: usize, header: &SectionHeader) {
    put_u32(bytes, at, header.name);
    put_u32(bytes, at + 4, header.kind);
    put_u64(bytes, at + 8, header.flags);
    put_u64(bytes, at + 16, header.address);
    put_u64(bytes, at + 24, header.offset as u64);
    put_u64(bytes, at + 32, header.size as u64);
    put_u32(bytes, at + 40, header.link);
    put_u32(bytes, at + 44, header.info);
    put_u64(bytes, at + 48, header.align);
    put_u64(bytes, at + 56, header.entry_size);
}

#[allow(clippy::too_many_arguments)]
fn write_segment(
    bytes: &mut [u8],
    at: usize,
    kind: u32,
    flags: u32,
    offset: usize,
    address: u64,
    size: usize,
    align: u64,
) {
    put_u32(bytes, at, kind);
    put_u32(bytes, at + 4, flags);
    put_u64(bytes, at + 8, offset as u64);
    put_u64(bytes, at + 16, address);
    put_u64(bytes, at + 24, address);
    put_u64(bytes, at + 32, size as u64);
    put_u64(bytes, at + 40, size as u64);
    put_u64(bytes, at + 48, align);
}

fn write_header(bytes: &mut [u8], machine: Machine, layout: &Layout) {
    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    put_u16(bytes, 16, TYPE_SHARED_OBJECT);
    put_u16(bytes, 18, machine.number());
    put_u32(bytes, 20, 1);
    put_u64(bytes, 32, ELF_HEADER_SIZE as u64);
    put_u64(bytes, 40, layout.section_headers as u64);
    put_u16(bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(bytes, 56, PROGRAM_HEADERS as u16);
    put_u16(bytes, 58, SECTION_HEADER_SIZE as u16);
    put_u16(bytes, 60, SECTION_NAMES.len() as u16);
    put_u16(bytes, 62, (SECTION_NAMES.len() - 1) as u16);
}

fn write_segments(bytes: &mut [u8], layout: &Layout) {
    let dynamic_size = layout.dynamic_end - layout.dynamic;
    let dynamic_address = layout.data_address(layout.dynamic);
    let mut at = ELF_HEADER_SIZE;
    for (kind, flags, offset, address, size, align) in [
        (
            segment::LOAD,
            segment::READ | segment::EXECUTE,
            0,
            0,
            layout.text_end,
            SEGMENT_ALIGN,
        ),
        (
            segment::LOAD,
            segment::READ | segment::WRITE,
            layout.dynamic,
            dynamic_address,
            dynamic_size,
            SEGMENT_ALIGN,
        ),
        (
            segment::DYNAMIC,
            segment::READ | segment::WRITE,
            layout.dynamic,
            dynamic_address,
            dynamic_size,
            8,
        ),
        (
            segment::GNU_STACK,
            segment::READ | segment::WRITE,
            0,
            0,
            0,
            16,
        ),
    ] {
        write_segment(bytes, at, kind, flags, offset, address, size, align);
        at += PROGRAM_HEADER_SIZE;
    }
}

fn write_sections(bytes: &mut [u8], layout: &Layout, names: &[u32], string_bytes: usize) {
    let headers = [
        SectionHeader {
            name: names[0],
            kind: 0,
            flags: 0,
            address: 0,
            offset: 0,
            size: 0,
            link: 0,
            info: 0,
            align: 0,
            entry_size: 0,
        },
        SectionHeader {
            name: names[1],
            kind: section::HASH,
            flags: section::ALLOCATED,
            address: layout.hash as u64,
            offset: layout.hash,
            size: layout.symbols - layout.hash,
            link: 2,
            info: 0,
            align: 8,
            entry_size: 4,
        },
        SectionHeader {
            name: names[2],
            kind: section::DYNAMIC_SYMBOLS,
            flags: section::ALLOCATED,
            address: layout.symbols as u64,
            offset: layout.symbols,
            size: layout.strings - layout.symbols,
            link: 3,
            info: 1,
            align: 8,
            entry_size: SYMBOL_SIZE as u64,
        },
        SectionHeader {
            name: names[3],
            kind: section::STRING_TABLE,
            flags: section::ALLOCATED,
            address: layout.strings as u64,
            offset: layout.strings,
            size: string_bytes,
            link: 0,
            info: 0,
            align: 1,
            entry_size: 0,
        },
        SectionHeader {
            name: names[4],
            kind: section::PROGRAM_BITS,
            flags: section::ALLOCATED | section::EXECUTABLE,
            address: layout.text as u64,
            offset: layout.text,
            size: layout.text_end - layout.text,
            link: 0,
            info: 0,
            align: FUNCTION_SIZE as u64,
            entry_size: 0,
        },
        SectionHeader {
            name: names[5],
            kind: section::DYNAMIC,
            flags: section::ALLOCATED | section::WRITE,
            address: layout.data_address(layout.dynamic),
            offset: layout.dynamic,
            size: layout.dynamic_end - layout.dynamic,
            link: 3,
            info: 0,
            align: 8,
            entry_size: DYNAMIC_ENTRY_SIZE as u64,
        },
        SectionHeader {
            name: names[6],
            kind: section::STRING_TABLE,
            flags: 0,
            address: 0,
            offset: layout.section_names,
            size: layout.section_headers - layout.section_names,
            link: 0,
            info: 0,
            align: 1,
            entry_size: 0,
        },
    ];
    for (index, header) in headers.iter().enumerate() {
        write_section(
            bytes,
            layout.section_headers + index * SECTION_HEADER_SIZE,
            header,
        );
    }
}

/// `library` written as an ELF-64 shared object for `machine`.
pub fn shared_object(machine: Machine, library: &Library) -> Vec<u8> {
    let names: Vec<&str> = library
        .functions
        .iter()
        .map(|function| function.name)
        .collect();
    let mut strings = Strings::new();
    let soname = strings.add(library.soname);
    let name_offsets: Vec<u32> = names.iter().map(|name| strings.add(name)).collect();
    let mut section_names = Strings {
        bytes: Vec::with_capacity(64),
    };
    let section_name_offsets: Vec<u32> = SECTION_NAMES
        .iter()
        .map(|name| match name.is_empty() {
            true => {
                section_names.bytes.push(0);
                0
            }
            false => section_names.add(name),
        })
        .collect();
    let hash = hash_table(&names);
    let layout = Layout::of(
        names.len() + 1,
        hash.len(),
        strings.bytes.len(),
        section_names.bytes.len(),
    );
    let mut bytes = vec![0u8; layout.end];
    write_header(&mut bytes, machine, &layout);
    write_segments(&mut bytes, &layout);
    bytes[layout.hash..layout.hash + hash.len()].copy_from_slice(&hash);
    for (index, function) in library.functions.iter().enumerate() {
        let symbol = layout.symbols + (index + 1) * SYMBOL_SIZE;
        let code = layout.text + index * FUNCTION_SIZE;
        put_u32(&mut bytes, symbol, name_offsets[index]);
        bytes[symbol + 4] = GLOBAL_FUNCTION;
        put_u16(&mut bytes, symbol + 6, TEXT_SECTION);
        put_u64(&mut bytes, symbol + 8, code as u64);
        put_u64(&mut bytes, symbol + 16, FUNCTION_SIZE as u64);
        bytes[code..code + FUNCTION_SIZE].copy_from_slice(&machine.function(function.returns));
    }
    bytes[layout.strings..layout.strings + strings.bytes.len()].copy_from_slice(&strings.bytes);
    let entries = [
        (tag::SONAME, u64::from(soname)),
        (tag::HASH, layout.hash as u64),
        (tag::STRING_TABLE, layout.strings as u64),
        (tag::SYMBOL_TABLE, layout.symbols as u64),
        (tag::STRING_TABLE_SIZE, strings.bytes.len() as u64),
        (tag::SYMBOL_SIZE, SYMBOL_SIZE as u64),
        (tag::NULL, 0),
    ];
    for (index, (tag, value)) in entries.into_iter().enumerate() {
        let at = layout.dynamic + index * DYNAMIC_ENTRY_SIZE;
        put_u64(&mut bytes, at, tag);
        put_u64(&mut bytes, at + 8, value);
    }
    bytes[layout.section_names..layout.section_names + section_names.bytes.len()]
        .copy_from_slice(&section_names.bytes);
    write_sections(
        &mut bytes,
        &layout,
        &section_name_offsets,
        strings.bytes.len(),
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    fn c_string(bytes: &[u8], at: usize) -> &str {
        let end = bytes[at..].iter().position(|&byte| byte == 0).unwrap();
        std::str::from_utf8(&bytes[at..at + end]).unwrap()
    }

    struct Loaded<'a> {
        bytes: &'a [u8],
        dynamic: Vec<(u64, u64)>,
    }

    impl<'a> Loaded<'a> {
        fn of(bytes: &'a [u8]) -> Self {
            let headers = u64_at(bytes, 32) as usize;
            let count = usize::from(u16_at(bytes, 56));
            let mut loads = Vec::new();
            let mut dynamic = None;
            for index in 0..count {
                let at = headers + index * PROGRAM_HEADER_SIZE;
                let (offset, address, size) = (
                    u64_at(bytes, at + 8),
                    u64_at(bytes, at + 16),
                    u64_at(bytes, at + 32),
                );
                match u32_at(bytes, at) {
                    segment::LOAD => {
                        assert_eq!(offset % SEGMENT_ALIGN, address % SEGMENT_ALIGN);
                        loads.push((offset, address, size));
                    }
                    segment::DYNAMIC => dynamic = Some((offset, size)),
                    _ => {}
                }
            }
            let (offset, size) = dynamic.unwrap();
            let entries = (0..size as usize / DYNAMIC_ENTRY_SIZE)
                .map(|index| {
                    let at = offset as usize + index * DYNAMIC_ENTRY_SIZE;
                    (u64_at(bytes, at), u64_at(bytes, at + 8))
                })
                .collect();
            assert_eq!(loads.len(), 2);
            Self {
                bytes,
                dynamic: entries,
            }
        }

        fn value(&self, tag: u64) -> usize {
            self.dynamic
                .iter()
                .find(|(found, _)| *found == tag)
                .map(|(_, value)| *value as usize)
                .unwrap()
        }

        fn soname(&self) -> &str {
            c_string(
                self.bytes,
                self.value(tag::STRING_TABLE) + self.value(tag::SONAME),
            )
        }

        fn lookup(&self, name: &str) -> Option<usize> {
            let hash = self.value(tag::HASH);
            let symbols = self.value(tag::SYMBOL_TABLE);
            let strings = self.value(tag::STRING_TABLE);
            let buckets = u32_at(self.bytes, hash) as usize;
            let chains = hash + 8 + buckets * 4;
            let slot = sysv_hash(name.as_bytes()) as usize % buckets;
            let mut index = u32_at(self.bytes, hash + 8 + slot * 4) as usize;
            while index != 0 {
                let symbol = symbols + index * SYMBOL_SIZE;
                let named = c_string(self.bytes, strings + u32_at(self.bytes, symbol) as usize);
                if named == name {
                    assert_eq!(self.bytes[symbol + 4], GLOBAL_FUNCTION);
                    assert_ne!(u16_at(self.bytes, symbol + 6), 0);
                    return Some(u64_at(self.bytes, symbol + 8) as usize);
                }
                index = u32_at(self.bytes, chains + index * 4) as usize;
            }
            None
        }
    }

    #[test]
    fn the_hash_is_the_system_v_one() {
        assert_eq!(sysv_hash(b""), 0);
        assert_eq!(sysv_hash(b"printf"), 0x077905a6);
        assert_eq!(sysv_hash(b"exit"), 0x0006cf04);
        assert_eq!(sysv_hash(b"eglGetDisplay"), sysv_hash(b"eglGetDisplay"));
    }

    #[test]
    fn every_library_is_sorted_and_names_each_function_once() {
        for library in LIBRARIES {
            assert!(
                library
                    .functions
                    .windows(2)
                    .all(|pair| pair[0].name < pair[1].name),
                "{}",
                library.soname
            );
        }
        assert!(EGL.functions.iter().all(|f| f.name.starts_with("egl")));
        assert!(GLES.functions.iter().all(|f| f.name.starts_with("gl")));
    }

    #[test]
    fn each_library_loads_by_its_soname_and_finds_every_function_through_its_hash() {
        for machine in [Machine::X86_64, Machine::Aarch64] {
            for library in LIBRARIES {
                let bytes = shared_object(machine, &library);
                assert_eq!(&bytes[0..4], b"\x7fELF");
                assert_eq!(u16_at(&bytes, 16), TYPE_SHARED_OBJECT);
                assert_eq!(u16_at(&bytes, 18), machine.number());
                let loaded = Loaded::of(&bytes);
                assert_eq!(loaded.soname(), library.soname);
                assert_eq!(loaded.value(tag::SYMBOL_SIZE), SYMBOL_SIZE);
                for function in library.functions {
                    let code = loaded.lookup(function.name).unwrap();
                    assert_eq!(code % FUNCTION_SIZE, 0);
                    assert_eq!(
                        bytes[code..code + FUNCTION_SIZE],
                        machine.function(function.returns)
                    );
                }
                assert_eq!(loaded.lookup("eglNoSuchCall"), None);
            }
        }
    }

    #[test]
    fn each_function_returns_its_answer_in_the_return_register() {
        let x86 = Machine::X86_64.function(EGL_NOT_INITIALIZED);
        assert_eq!(x86[..6], [0xb8, 0x01, 0x30, 0x00, 0x00, 0xc3]);
        let arm = Machine::Aarch64.function(EGL_NOT_INITIALIZED);
        assert_eq!(u32_at(&arm, 0), 0x5286_0020);
        assert_eq!(u32_at(&arm, 4), 0xd65f_03c0);
        assert_eq!(u32_at(&Machine::Aarch64.function(0), 0), 0x5280_0000);
        assert_eq!(
            Machine::of("riscv64").unwrap_err(),
            "riscv64: no GL libraries are written for it"
        );
    }

    #[test]
    fn the_section_headers_name_the_text_and_dynamic_sections() {
        let bytes = shared_object(Machine::X86_64, &EGL);
        let headers = u64_at(&bytes, 40) as usize;
        let count = usize::from(u16_at(&bytes, 60));
        let names_index = usize::from(u16_at(&bytes, 62));
        let names_at = u64_at(&bytes, headers + names_index * SECTION_HEADER_SIZE + 24) as usize;
        let named: Vec<&str> = (0..count)
            .map(|index| {
                let name = u32_at(&bytes, headers + index * SECTION_HEADER_SIZE) as usize;
                c_string(&bytes, names_at + name)
            })
            .collect();
        assert_eq!(named, SECTION_NAMES);
        let text = headers + usize::from(TEXT_SECTION) * SECTION_HEADER_SIZE;
        assert_eq!(u32_at(&bytes, text + 4), section::PROGRAM_BITS);
        assert_eq!(
            u64_at(&bytes, text + 32) as usize,
            EGL.functions.len() * FUNCTION_SIZE
        );
        assert_eq!(headers + count * SECTION_HEADER_SIZE, bytes.len());
    }
}
