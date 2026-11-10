//! Whether a Linux program draws windows (D223): it names a windowing system's or a GUI
//! toolkit's library among the shared libraries it needs (its `DT_NEEDED` entries), read from
//! its dynamic section and string table as the System V ELF specification lays them out. The
//! installer makes such a program a windowed app rather than a console app.

use crate::program::{elf, u16_le, u32_le, u64_le};

/// The start of the file name of each library whose programs draw windows, up to its `.so`:
/// the X and Wayland client libraries, SDL, GTK, Qt's GUI module and GLFW.
pub const GUI_LIBRARIES: [&[u8]; 12] = [
    b"libX11.so",
    b"libxcb.so",
    b"libwayland-client.so",
    b"libSDL-1.2.so",
    b"libSDL2-2.0.so",
    b"libSDL3.so",
    b"libgtk-x11-2.0.so",
    b"libgtk-3.so",
    b"libgtk-4.so",
    b"libQt5Gui.so",
    b"libQt6Gui.so",
    b"libglfw.so",
];

/// A range of a program's bytes to read: where it starts and the buffer it fills.
pub type ByteRange<'b> = (u64, &'b mut [u8]);

/// A program's bytes, read a few ranges at a time: the program a file or an archive's entry
/// holds.
pub trait ProgramBytes {
    /// How many bytes the program has.
    fn size(&self) -> u64;

    /// Fills each range's buffer from its offset, in one pass over the program; whether all
    /// were filled.
    fn read_ranges(&mut self, ranges: &mut [ByteRange<'_>]) -> bool;
}

impl<R: crate::reader::Reader> ProgramBytes for R {
    fn size(&self) -> u64 {
        crate::reader::Reader::size(self)
    }

    fn read_ranges(&mut self, ranges: &mut [ByteRange<'_>]) -> bool {
        ranges
            .iter_mut()
            .all(|(offset, buffer)| self.read_exact_at(*offset, buffer))
    }
}

const LOAD: u32 = 1;
const DYNAMIC: u32 = 2;
const PROGRAM_HEADER_BYTES: usize = 56;
const MAX_LOADS: usize = 16;
const DYNAMIC_ENTRY_BYTES: usize = 16;
const MAX_DYNAMIC_BYTES: usize = 4096;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;
const MAX_NEEDED: usize = 64;
const NAME_BYTES: usize = 64;

#[derive(Clone, Copy, Default)]
struct Load {
    offset: u64,
    address: u64,
    file_size: u64,
}

struct Segments {
    loads: [Load; MAX_LOADS],
    load_count: usize,
    dynamic: Option<(u64, u64)>,
}

fn segments(head: &[u8]) -> Option<Segments> {
    if !head.starts_with(elf::MAGIC)
        || head.get(elf::CLASS) != Some(&elf::CLASS_64)
        || head.get(elf::DATA) != Some(&elf::LITTLE_ENDIAN)
    {
        return None;
    }
    let first = usize::try_from(u64_le(head, elf::PROGRAM_HEADERS)?).ok()?;
    let size = usize::from(u16_le(head, elf::PROGRAM_HEADER_SIZE)?);
    let count = usize::from(u16_le(head, elf::PROGRAM_HEADER_COUNT)?);
    if size < PROGRAM_HEADER_BYTES {
        return None;
    }
    let mut found = Segments {
        loads: [Load::default(); MAX_LOADS],
        load_count: 0,
        dynamic: None,
    };
    for index in 0..count {
        let at = first.checked_add(index.checked_mul(size)?)?;
        let offset = u64_le(head, at.checked_add(8)?)?;
        let file_size = u64_le(head, at.checked_add(32)?)?;
        match u32_le(head, at)? {
            LOAD if found.load_count < MAX_LOADS => {
                found.loads[found.load_count] = Load {
                    offset,
                    address: u64_le(head, at.checked_add(16)?)?,
                    file_size,
                };
                found.load_count += 1;
            }
            DYNAMIC => found.dynamic = Some((offset, file_size)),
            _ => {}
        }
    }
    Some(found)
}

impl Segments {
    fn file_offset(&self, address: u64) -> Option<u64> {
        self.loads[..self.load_count].iter().find_map(|load| {
            let within = address.checked_sub(load.address)?;
            match within < load.file_size {
                true => load.offset.checked_add(within),
                false => None,
            }
        })
    }
}

struct Needed {
    names: [u64; MAX_NEEDED],
    count: usize,
    strings: Option<u64>,
    strings_size: u64,
}

fn needed(dynamic: &[u8]) -> Needed {
    let mut found = Needed {
        names: [0; MAX_NEEDED],
        count: 0,
        strings: None,
        strings_size: 0,
    };
    for entry in dynamic.as_chunks::<DYNAMIC_ENTRY_BYTES>().0 {
        let (Some(tag), Some(value)) = (u64_le(entry, 0), u64_le(entry, 8)) else {
            break;
        };
        match tag {
            DT_NULL => break,
            DT_NEEDED if found.count < MAX_NEEDED => {
                found.names[found.count] = value;
                found.count += 1;
            }
            DT_STRTAB => found.strings = Some(value),
            DT_STRSZ => found.strings_size = value,
            _ => {}
        }
    }
    found
}

/// Whether `name`, a needed library's file name, is one of [`GUI_LIBRARIES`] (any version).
pub fn is_gui_library(name: &[u8]) -> bool {
    GUI_LIBRARIES.iter().any(|library| {
        name.strip_prefix(*library)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"."))
    })
}

/// Whether the 64-bit little-endian ELF program whose first bytes are `head` (its header and
/// program headers) and whose bytes `program` reads needs one of [`GUI_LIBRARIES`]. A
/// statically linked program, or one whose dynamic section cannot be read, needs none.
pub fn links_gui_library(head: &[u8], program: &mut impl ProgramBytes) -> bool {
    let Some(segments) = segments(head) else {
        return false;
    };
    let Some((dynamic_offset, dynamic_size)) = segments.dynamic else {
        return false;
    };
    let size = program.size();
    let dynamic_length = dynamic_size
        .min(MAX_DYNAMIC_BYTES as u64)
        .min(size.saturating_sub(dynamic_offset)) as usize;
    let mut dynamic = [0u8; MAX_DYNAMIC_BYTES];
    if !program.read_ranges(&mut [(dynamic_offset, &mut dynamic[..dynamic_length])]) {
        return false;
    }
    let needed = needed(&dynamic[..dynamic_length]);
    let Some(strings) = needed
        .strings
        .and_then(|address| segments.file_offset(address))
    else {
        return false;
    };
    let mut names = [[0u8; NAME_BYTES]; MAX_NEEDED];
    let mut ranges: [ByteRange<'_>; MAX_NEEDED] = core::array::from_fn(|_| (0, &mut [][..]));
    let mut range_count = 0;
    for (name, buffer) in needed.names[..needed.count].iter().zip(names.iter_mut()) {
        let Some(start) = strings.checked_add(*name) else {
            continue;
        };
        if *name >= needed.strings_size || start >= size {
            continue;
        }
        let length = (NAME_BYTES as u64)
            .min(needed.strings_size - *name)
            .min(size - start) as usize;
        ranges[range_count] = (start, &mut buffer[..length]);
        range_count += 1;
    }
    if !program.read_ranges(&mut ranges[..range_count]) {
        return false;
    }
    ranges[..range_count].iter().any(|(_, bytes)| {
        let name = bytes.split(|&byte| byte == 0).next().unwrap_or(bytes);
        is_gui_library(name)
    })
}

#[cfg(test)]
pub(crate) mod samples {
    use super::{DT_NEEDED, DT_STRSZ, DT_STRTAB, DYNAMIC, LOAD};
    use crate::program::elf::INTERPRETER as INTERPRETER_KIND;

    const ADDRESS: u64 = 0x40_0000;
    const INTERPRETER_AT: usize = 0x200;
    const INTERPRETER: &[u8] = b"/lib/ld-musl-x86_64.so.1\0";
    const STRINGS_AT: u64 = 0x1000;
    const DYNAMIC_AT: u64 = 0x3000;

    /// A dynamically linked x86_64 ELF program, loaded by `/lib/ld-musl-x86_64.so.1`, needing
    /// `libraries`: its string table at 4 KiB (past the first bytes the installer reads) and its
    /// dynamic section at 12 KiB.
    pub fn program_needing(libraries: &[&str]) -> Vec<u8> {
        let mut program = vec![0u8; DYNAMIC_AT as usize];
        program[..4].copy_from_slice(b"\x7fELF");
        program[4] = 2;
        program[5] = 1;
        program[6] = 1;
        program[16..18].copy_from_slice(&3u16.to_le_bytes());
        program[18..20].copy_from_slice(&62u16.to_le_bytes());
        program[32..40].copy_from_slice(&64u64.to_le_bytes());
        program[54..56].copy_from_slice(&56u16.to_le_bytes());
        program[56..58].copy_from_slice(&3u16.to_le_bytes());
        program[INTERPRETER_AT..INTERPRETER_AT + INTERPRETER.len()].copy_from_slice(INTERPRETER);
        let mut strings = vec![0u8];
        let mut offsets = Vec::new();
        for library in libraries {
            offsets.push(strings.len() as u64);
            strings.extend_from_slice(library.as_bytes());
            strings.push(0);
        }
        let mut dynamic = Vec::new();
        for offset in &offsets {
            dynamic.extend_from_slice(&DT_NEEDED.to_le_bytes());
            dynamic.extend_from_slice(&offset.to_le_bytes());
        }
        for (tag, value) in [
            (DT_STRTAB, ADDRESS + STRINGS_AT),
            (DT_STRSZ, strings.len() as u64),
            (0, 0),
        ] {
            dynamic.extend_from_slice(&tag.to_le_bytes());
            dynamic.extend_from_slice(&value.to_le_bytes());
        }
        let strings_at = STRINGS_AT as usize;
        program[strings_at..strings_at + strings.len()].copy_from_slice(&strings);
        program.extend_from_slice(&dynamic);
        let end = program.len() as u64;
        let headers = [
            (LOAD, 0, ADDRESS, end),
            (
                DYNAMIC,
                DYNAMIC_AT,
                ADDRESS + DYNAMIC_AT,
                dynamic.len() as u64,
            ),
            (
                INTERPRETER_KIND,
                INTERPRETER_AT as u64,
                ADDRESS + INTERPRETER_AT as u64,
                INTERPRETER.len() as u64,
            ),
        ];
        for (index, (kind, offset, address, size)) in headers.into_iter().enumerate() {
            let at = 64 + index * 56;
            program[at..at + 4].copy_from_slice(&kind.to_le_bytes());
            program[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            program[at + 16..at + 24].copy_from_slice(&address.to_le_bytes());
            program[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
            program[at + 40..at + 48].copy_from_slice(&size.to_le_bytes());
        }
        program
    }
}

#[cfg(test)]
mod tests {
    use super::samples::program_needing;
    use super::*;
    use crate::program::HEAD_BYTES;
    use crate::reader::Bytes;

    fn links(program: &[u8]) -> bool {
        let head = &program[..program.len().min(HEAD_BYTES)];
        links_gui_library(head, &mut Bytes(program))
    }

    #[test]
    fn a_program_needing_a_windowing_or_toolkit_library_draws_windows() {
        for library in [
            "libSDL2-2.0.so.0",
            "libgtk-3.so.0",
            "libQt5Gui.so.5",
            "libQt6Gui.so.6",
            "libX11.so.6",
            "libwayland-client.so.0",
        ] {
            let program = program_needing(&["libc.so.6", "libm.so.6", library]);
            assert!(links(&program), "{library}");
        }
    }

    #[test]
    fn a_program_needing_other_libraries_or_none_is_a_console_program() {
        assert!(!links(&program_needing(&["libc.so.6", "libncursesw.so.6"])));
        assert!(!links(&program_needing(&[])));
        let mut static_program = program_needing(&["libgtk-3.so.0"]);
        static_program[56..58].copy_from_slice(&1u16.to_le_bytes());
        assert!(!links(&static_program));
        assert!(!links(b"\x7fELF"));
    }

    #[test]
    fn a_library_name_matches_by_its_whole_name_before_the_version() {
        assert!(is_gui_library(b"libgtk-4.so"));
        assert!(is_gui_library(b"libgtk-4.so.1"));
        assert!(!is_gui_library(b"libgtk-4.so-helper"));
        assert!(!is_gui_library(b"libX11-xcb.so.1"));
        assert!(!is_gui_library(b"libQt5Core.so.5"));
    }

    #[test]
    fn a_dynamic_section_cut_short_by_the_end_of_the_file_is_read_as_far_as_it_goes() {
        let mut program = program_needing(&["libSDL2-2.0.so.0"]);
        let cut = program.len() - 8;
        program.truncate(cut);
        assert!(links(&program));
    }
}
