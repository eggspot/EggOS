//! What the layer needs to run an x64 program on an ARM64 computer through ahead-of-time
//! translation (D191, slice 39d2), as pure logic with host tests: the translator's input from
//! the laid-out, unrelocated image (sections, and the entry point, exports, TLS callbacks,
//! `.pdata` function starts and relocation targets in code as seeds), the import address
//! table's slots and each import's argument kinds for its bridge, and the Windows status and
//! report of a trap or fault in translated code. The cache, the trap causes and the mapping
//! are shared with the macOS layer in `eggos-translation-loader`; `translation.rs` maps and
//! runs the result.

use crate::fault::{self, Image, Line};
use crate::pe::{Pe, directory, read_u16, read_u32, read_u64, section_flags};
use alloc::vec::Vec;
use eggos_translate::image::bridge::{ArgumentKind, DEFAULT_ARGUMENTS};
use eggos_translate::image::{Section, Seed, SeedOrigin, pdata_function_starts};
pub use eggos_translation_loader::trap::TrapCause;

const RELOCATION_DIR64: u16 = 10;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
const IMPORT_BY_ORDINAL: u64 = 1 << 63;
const TLS_CALLBACKS_FIELD: usize = 24;
const EXPORT_FUNCTION_COUNT_FIELD: usize = 20;
const EXPORT_FUNCTIONS_FIELD: usize = 28;
const MAX_TLS_CALLBACKS: usize = 1024;
const MAX_EXPORTS: usize = 1 << 16;
const INTEGERS: [ArgumentKind; DEFAULT_ARGUMENTS] = [ArgumentKind::Integer; DEFAULT_ARGUMENTS];
const ONE_FLOATING_POINT: [ArgumentKind; 1] = [ArgumentKind::FloatingPoint];

/// The sections of `image`, laid out by `image::copy_sections` and not relocated, as the
/// translator reads them.
pub fn sections<'a>(pe: &Pe<'_>, image: &'a [u8]) -> Vec<Section<'a>> {
    pe.sections()
        .filter_map(|section| {
            let start = section.virtual_address as usize;
            let end = start
                .checked_add(section.memory_size() as usize)?
                .min(image.len());
            Some(Section {
                rva: section.virtual_address,
                bytes: image.get(start..end)?,
                executable: section.flags & section_flags::MEMORY_EXECUTE != 0,
            })
        })
        .collect()
}

fn executable(pe: &Pe<'_>, rva: u64) -> bool {
    pe.sections().any(|section| {
        section.flags & section_flags::MEMORY_EXECUTE != 0
            && (u64::from(section.virtual_address)
                ..u64::from(section.virtual_address) + u64::from(section.memory_size()))
                .contains(&rva)
    })
}

fn seed(rva: u32, origin: SeedOrigin) -> Seed {
    Seed { rva, origin }
}

/// The program's known entry points: its entry point, exports, TLS callbacks, the function
/// starts of its exception table and the relocation targets that fall in code. Tables that
/// are damaged give what could be read.
pub fn seeds(pe: &Pe<'_>, image: &[u8]) -> Vec<Seed> {
    let mut seeds = Vec::new();
    seeds.push(seed(pe.entry, SeedOrigin::EntryPoint));
    export_seeds(pe, image, &mut seeds);
    tls_callback_seeds(pe, image, &mut seeds);
    let exceptions = pe.directory(directory::EXCEPTION);
    let start = exceptions.address as usize;
    if let Some(table) = image.get(start..start.saturating_add(exceptions.size as usize)) {
        for rva in pdata_function_starts(table) {
            seeds.push(seed(rva, SeedOrigin::ExceptionTable));
        }
    }
    relocation_seeds(pe, image, &mut seeds);
    seeds
}

fn export_seeds(pe: &Pe<'_>, image: &[u8], seeds: &mut Vec<Seed>) {
    let exports = pe.directory(directory::EXPORT);
    if exports.size == 0 {
        return;
    }
    let at = exports.address as usize;
    let (Ok(count), Ok(functions)) = (
        read_u32(image, at + EXPORT_FUNCTION_COUNT_FIELD),
        read_u32(image, at + EXPORT_FUNCTIONS_FIELD),
    ) else {
        return;
    };
    let forwarders =
        u64::from(exports.address)..u64::from(exports.address) + u64::from(exports.size);
    for index in 0..(count as usize).min(MAX_EXPORTS) {
        let Ok(rva) = read_u32(image, functions as usize + index * 4) else {
            return;
        };
        if rva != 0 && !forwarders.contains(&u64::from(rva)) && executable(pe, u64::from(rva)) {
            seeds.push(seed(rva, SeedOrigin::Export));
        }
    }
}

fn tls_callback_seeds(pe: &Pe<'_>, image: &[u8], seeds: &mut Vec<Seed>) {
    let tls = pe.directory(directory::TLS);
    if tls.size == 0 {
        return;
    }
    let Ok(callbacks) = read_u64(image, tls.address as usize + TLS_CALLBACKS_FIELD) else {
        return;
    };
    let Some(mut at) = callbacks
        .checked_sub(pe.image_base)
        .and_then(|rva| usize::try_from(rva).ok())
    else {
        return;
    };
    for _ in 0..MAX_TLS_CALLBACKS {
        let Ok(callback) = read_u64(image, at) else {
            return;
        };
        let Some(rva) = callback.checked_sub(pe.image_base) else {
            return;
        };
        if callback == 0 {
            return;
        }
        if let Ok(rva) = u32::try_from(rva) {
            seeds.push(seed(rva, SeedOrigin::TlsCallback));
        }
        at += 8;
    }
}

fn relocation_seeds(pe: &Pe<'_>, image: &[u8], seeds: &mut Vec<Seed>) {
    let table = pe.directory(directory::BASE_RELOCATION);
    let mut block = table.address as usize;
    let end = block.saturating_add(table.size as usize);
    while block + 8 <= end {
        let (Ok(page), Ok(size)) = (read_u32(image, block), read_u32(image, block + 4)) else {
            return;
        };
        let size = size as usize;
        if size < 8 || block + size > end {
            return;
        }
        for entry in (block + 8..block + size).step_by(2) {
            let Ok(entry) = read_u16(image, entry) else {
                return;
            };
            if entry >> 12 != RELOCATION_DIR64 {
                continue;
            }
            let target = page as usize + usize::from(entry & 0xfff);
            let Ok(value) = read_u64(image, target) else {
                continue;
            };
            let rva = value.wrapping_sub(pe.image_base);
            if let Ok(rva) = u32::try_from(rva)
                && executable(pe, u64::from(rva))
            {
                seeds.push(seed(rva, SeedOrigin::Relocation));
            }
        }
        block += size;
    }
}

/// One slot of the import address table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSlot {
    /// The slot's relative address.
    pub rva: u32,
    /// The library's name as the import names it.
    pub library: Vec<u8>,
    /// The function's name; empty for an import by ordinal.
    pub function: Vec<u8>,
}

impl ImportSlot {
    /// How the function takes its arguments, for its bridge: every import is taken as
    /// [`DEFAULT_ARGUMENTS`] integers (variadic ones too) except the few the layer exports
    /// with floating-point arguments.
    pub fn arguments(&self) -> &'static [ArgumentKind] {
        if self.library.is_empty() {
            &INTEGERS
        } else {
            function_arguments(&self.function)
        }
    }
}

/// How the layer's export `function` takes its arguments, for its bridge: [`DEFAULT_ARGUMENTS`]
/// integers unless it is one of the few with floating-point arguments; an empty name (an
/// ordinal) is taken as integers.
pub fn function_arguments(function: &[u8]) -> &'static [ArgumentKind] {
    match function {
        b"sqrtf" => &ONE_FLOATING_POINT,
        _ => &INTEGERS,
    }
}

/// How many functions found while the program runs (`GetProcAddress`) can be called from
/// translated code: each gets one of the procedure stubs the translation carries.
pub const PROCEDURE_STUBS: usize = 64;
const PROCEDURE_STUB_SPACING: u32 = 16;

/// The relative addresses of the procedure stubs, the guest addresses `GetProcAddress` hands
/// out: 16 bytes apart from the first page past the image (`mapped_size`, a multiple of the
/// page size), where no guest code is. `None` when they would not fit in 32 bits.
pub fn procedure_stub_rvas(mapped_size: u64) -> Option<Vec<u32>> {
    let first = u32::try_from(mapped_size).ok()?;
    (0..PROCEDURE_STUBS as u32)
        .map(|index| first.checked_add(index * PROCEDURE_STUB_SPACING))
        .collect()
}

/// Which procedure stub a guest address relative to the image base is, if any.
pub fn procedure_stub_index(mapped_size: u64, rva: u64) -> Option<usize> {
    let offset = rva.checked_sub(mapped_size)?;
    let index = usize::try_from(offset / u64::from(PROCEDURE_STUB_SPACING)).ok()?;
    (offset.is_multiple_of(u64::from(PROCEDURE_STUB_SPACING)) && index < PROCEDURE_STUBS)
        .then_some(index)
}

fn c_string(image: &[u8], at: usize) -> Option<&[u8]> {
    let tail = image.get(at..)?;
    let length = tail.iter().position(|&byte| byte == 0)?;
    Some(&tail[..length])
}

/// Every slot of the import address tables, in table order; `None` when the import
/// directory is damaged.
pub fn import_slots(pe: &Pe<'_>, image: &[u8]) -> Option<Vec<ImportSlot>> {
    let table = pe.directory(directory::IMPORT);
    let mut slots = Vec::new();
    if table.size == 0 {
        return Some(slots);
    }
    let mut descriptor = table.address as usize;
    loop {
        let lookup = read_u32(image, descriptor).ok()? as usize;
        let name = read_u32(image, descriptor + 12).ok()? as usize;
        let address_table = read_u32(image, descriptor + 16).ok()? as usize;
        if lookup == 0 && name == 0 && address_table == 0 {
            return Some(slots);
        }
        let library = c_string(image, name)?.to_vec();
        let lookup = if lookup == 0 { address_table } else { lookup };
        for index in 0.. {
            let entry = read_u64(image, lookup + index * 8).ok()?;
            if entry == 0 {
                break;
            }
            let function = match entry & IMPORT_BY_ORDINAL {
                0 => c_string(image, (entry & 0x7fff_ffff) as usize + 2)?.to_vec(),
                _ => Vec::new(),
            };
            slots.push(ImportSlot {
                rva: u32::try_from(address_table + index * 8).ok()?,
                library: library.clone(),
                function,
            });
        }
        descriptor += IMPORT_DESCRIPTOR_SIZE;
    }
}

/// The Windows status a program whose translated code trapped with `cause` ends with.
pub fn trap_status(cause: TrapCause) -> u32 {
    match cause {
        TrapCause::Breakpoint => fault::status::BREAKPOINT,
        TrapCause::DivideError => fault::status::INTEGER_DIVIDE_BY_ZERO,
        TrapCause::FastFail => fault::status::STACK_BUFFER_OVERRUN,
        TrapCause::GeneralProtection => fault::status::ACCESS_VIOLATION,
        TrapCause::InvalidOpcode | TrapCause::Refused | TrapCause::Undiscovered => {
            fault::status::ILLEGAL_INSTRUCTION
        }
    }
}

/// The Windows status for a fault other than a trap in translated code.
pub fn fault_status(signal: i32) -> u32 {
    match signal {
        fault::SIGILL => fault::status::ILLEGAL_INSTRUCTION,
        fault::SIGFPE => fault::status::INTEGER_DIVIDE_BY_ZERO,
        fault::SIGTRAP => fault::status::BREAKPOINT,
        _ => fault::status::ACCESS_VIOLATION,
    }
}

/// The line the layer writes when translated code of `program` (whose image is `image`)
/// trapped or faulted at guest address `guest`, ending the program.
pub fn report(program: &[u8], image: Image, cause: Option<TrapCause>, guest: u64) -> Line {
    match cause {
        Some(TrapCause::Refused) => {
            let mut line = Line::default();
            line.push(b"windows: ");
            line.push(program);
            line.push(b": the program uses an instruction EggOS does not translate yet, at ");
            line.push_hex(guest);
            fault::push_place(&mut line, program, image, guest);
            line.push(b"\n");
            line
        }
        Some(TrapCause::Undiscovered) => {
            let mut line = Line::default();
            line.push(b"windows: ");
            line.push(program);
            line.push(b": the program jumped to ");
            line.push_hex(guest);
            fault::push_place(&mut line, program, image, guest);
            line.push(b", code EggOS did not find when it translated the program\n");
            line
        }
        Some(TrapCause::FastFail) => fault::report_fail_fast(program, image, guest),
        Some(cause) => fault::report_exception(program, image, trap_status(cause), guest),
        None => fault::report_exception(program, image, fault::status::ACCESS_VIOLATION, guest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::Pe;
    use crate::pe::tests::{Builder, DATA, RDATA, TEST_IMAGE_BASE, TEXT};
    use eggos_translate::codegen::CodegenMode;
    use eggos_translate::image::bridge::add_import_stubs;
    use eggos_translate::image::{ImageInput, translate_image};

    fn laid_out(pe: &Pe<'_>) -> Vec<u8> {
        let mut image = vec![0u8; pe.mapped_size() as usize];
        crate::image::copy_sections(pe, &mut image);
        image
    }

    fn import_table(builder: Builder) -> Builder {
        let descriptor = 0x40usize;
        let lookup = 0x80u32;
        let names = 0xa0u32;
        let mut table = vec![0u8; 0xc0];
        let put32 = |table: &mut Vec<u8>, at: usize, value: u32| {
            table[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        put32(&mut table, descriptor, RDATA + lookup);
        put32(&mut table, descriptor + 12, RDATA + names);
        put32(&mut table, descriptor + 16, DATA);
        table[lookup as usize..lookup as usize + 8]
            .copy_from_slice(&u64::from(RDATA + names + 16).to_le_bytes());
        table[lookup as usize + 8..lookup as usize + 16]
            .copy_from_slice(&(IMPORT_BY_ORDINAL | 7).to_le_bytes());
        table[names as usize..names as usize + 13].copy_from_slice(b"msvcrt.dll\0\0\0");
        table[names as usize + 18..names as usize + 24].copy_from_slice(b"sqrtf\0");
        builder
            .directory(directory::IMPORT, RDATA + descriptor as u32, 40)
            .section_data(1, 0, &table)
    }

    #[test]
    fn the_translator_input_has_every_section_and_the_entry_point() {
        let builder = Builder::new();
        let pe = Pe::parse(&builder.file).unwrap();
        assert_eq!(pe.machine, crate::pe::Machine::X64);
        let image = laid_out(&pe);
        let sections = sections(&pe, &image);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].rva, TEXT);
        assert!(sections[0].executable);
        assert!(!sections[1].executable && !sections[2].executable);
        assert_eq!(sections[0].bytes[0], 0xc3);
        assert_eq!(sections[0].bytes.len(), 0x100);
        let seeds = seeds(&pe, &image);
        assert_eq!(seeds, [seed(TEXT, SeedOrigin::EntryPoint)]);
    }

    #[test]
    fn pdata_tls_callbacks_and_relocations_into_code_are_seeds() {
        let mut rdata = vec![0u8; 0x60];
        rdata[0..4].copy_from_slice(&(TEXT + 0x10).to_le_bytes());
        rdata[4..8].copy_from_slice(&(TEXT + 0x20).to_le_bytes());
        rdata[12..16].copy_from_slice(&(TEXT + 0x30).to_le_bytes());
        rdata[16..20].copy_from_slice(&(TEXT + 0x40).to_le_bytes());
        let callbacks = TEST_IMAGE_BASE + u64::from(RDATA) + 0x50;
        rdata[0x20 + 24..0x20 + 32].copy_from_slice(&callbacks.to_le_bytes());
        let callback = TEST_IMAGE_BASE + u64::from(TEXT) + 0x60;
        rdata[0x50..0x58].copy_from_slice(&callback.to_le_bytes());
        let mut data = vec![0u8; 0x30];
        let code_pointer = TEST_IMAGE_BASE + u64::from(TEXT) + 0x70;
        data[0..8].copy_from_slice(&code_pointer.to_le_bytes());
        let data_pointer = TEST_IMAGE_BASE + u64::from(DATA) + 0x10;
        data[8..16].copy_from_slice(&data_pointer.to_le_bytes());
        let mut relocations = vec![0u8; 12];
        relocations[0..4].copy_from_slice(&DATA.to_le_bytes());
        relocations[4..8].copy_from_slice(&12u32.to_le_bytes());
        relocations[8..10].copy_from_slice(&(RELOCATION_DIR64 << 12).to_le_bytes());
        relocations[10..12].copy_from_slice(&((RELOCATION_DIR64 << 12) | 8).to_le_bytes());
        let builder = Builder::new()
            .directory(directory::EXCEPTION, RDATA, 24)
            .directory(directory::TLS, RDATA + 0x20, 40)
            .directory(directory::BASE_RELOCATION, DATA + 0x20, 12)
            .section_data(1, 0, &rdata)
            .section_data(2, 0, &data)
            .section_data(2, 0x20, &relocations);
        let pe = Pe::parse(&builder.file).unwrap();
        let image = laid_out(&pe);
        let seeds = seeds(&pe, &image);
        assert!(seeds.contains(&seed(TEXT + 0x10, SeedOrigin::ExceptionTable)));
        assert!(seeds.contains(&seed(TEXT + 0x30, SeedOrigin::ExceptionTable)));
        assert!(seeds.contains(&seed(TEXT + 0x60, SeedOrigin::TlsCallback)));
        assert!(seeds.contains(&seed(TEXT + 0x70, SeedOrigin::Relocation)));
        assert!(!seeds.iter().any(|seed| seed.rva == DATA + 0x10));
    }

    #[test]
    fn import_slots_name_their_library_and_function_and_choose_the_arguments() {
        let builder = import_table(Builder::new());
        let pe = Pe::parse(&builder.file).unwrap();
        let image = laid_out(&pe);
        let slots = import_slots(&pe, &image).unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].rva, DATA);
        assert_eq!(slots[0].library, b"msvcrt.dll");
        assert_eq!(slots[0].function, b"sqrtf");
        assert_eq!(slots[0].arguments(), [ArgumentKind::FloatingPoint]);
        assert_eq!(slots[1].rva, DATA + 8);
        assert!(slots[1].function.is_empty());
        assert_eq!(slots[1].arguments().len(), DEFAULT_ARGUMENTS);
        let none = Builder::new();
        let pe = Pe::parse(&none.file).unwrap();
        assert_eq!(import_slots(&pe, &laid_out(&pe)), Some(Vec::new()));
    }

    #[test]
    fn procedure_stubs_follow_the_image_and_are_found_by_their_address() {
        let rvas = procedure_stub_rvas(0x4000).unwrap();
        assert_eq!(rvas.len(), PROCEDURE_STUBS);
        assert_eq!(rvas[0], 0x4000);
        assert_eq!(rvas[1], 0x4010);
        assert_eq!(procedure_stub_index(0x4000, 0x4010), Some(1));
        assert_eq!(procedure_stub_index(0x4000, 0x4018), None);
        assert_eq!(procedure_stub_index(0x4000, 0x3ff0), None);
        assert_eq!(procedure_stub_index(0x4000, 0x4000 + 16 * 64), None);
        assert_eq!(procedure_stub_rvas(1 << 32), None);
        assert_eq!(function_arguments(b"sqrtf"), [ArgumentKind::FloatingPoint]);
        assert_eq!(function_arguments(b"").len(), DEFAULT_ARGUMENTS);
    }

    #[test]
    fn an_image_translates_with_a_stub_for_each_import_slot() {
        let builder = import_table(Builder::new());
        let pe = Pe::parse(&builder.file).unwrap();
        let image = laid_out(&pe);
        let sections = sections(&pe, &image);
        let seeds = seeds(&pe, &image);
        let input = ImageInput {
            image_base: pe.image_base,
            sections: &sections,
            seeds: &seeds,
        };
        let mut translated = translate_image(&input, CodegenMode::default()).unwrap();
        let slots: Vec<u32> = import_slots(&pe, &image)
            .unwrap()
            .iter()
            .map(|slot| slot.rva)
            .collect();
        add_import_stubs(&mut translated, &slots).unwrap();
        assert!(translated.entries.lookup(TEXT).is_some());
        assert!(translated.entries.lookup(DATA).is_some());
        assert!(translated.entries.lookup(DATA + 8).is_some());
    }

    #[test]
    fn a_trap_s_cause_gives_the_windows_status() {
        assert_eq!(trap_status(TrapCause::DivideError), 0xc000_0094);
        assert_eq!(trap_status(TrapCause::Refused), 0xc000_001d);
        assert_eq!(fault_status(fault::SIGSEGV), 0xc000_0005);
    }

    #[test]
    fn a_refused_instruction_s_report_names_the_place() {
        let image = Image {
            base: 0x1_4000_0000,
            size: 0x4000,
        };
        let line = report(b"hello.exe", image, Some(TrapCause::Refused), 0x1_4000_1234);
        let text = std::str::from_utf8(line.as_bytes()).unwrap();
        assert!(text.starts_with("windows: hello.exe: the program uses an instruction"));
        assert!(text.contains("1234"));
        assert!(text.ends_with('\n'));
    }
}
