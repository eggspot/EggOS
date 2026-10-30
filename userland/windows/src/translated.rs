//! What the layer needs to run an x64 program on an ARM64 computer through ahead-of-time
//! translation (D191, slice 39d2), as pure logic with host tests: the translator's input from
//! the laid-out, unrelocated image (sections, and the entry point, exports, TLS callbacks,
//! `.pdata` function starts and relocation targets in code as seeds), the import address
//! table's slots and each import's argument kinds for its bridge, the code generation mode
//! from the CPU's capabilities, what a trap or fault in translated code means, and the cache
//! directory's reading, atomic writing and eviction over any [`CacheFiles`].
//! `translation.rs` maps and runs the result.

use crate::fault::{self, Image, Line};
use crate::pe::{Pe, directory, read_u16, read_u32, read_u64, section_flags};
use alloc::string::String;
use alloc::vec::Vec;
use eggos_translate::cache::{self, CacheFile, policy};
use eggos_translate::codegen::{CodegenMode, trap};
use eggos_translate::image::bridge::{ArgumentKind, DEFAULT_ARGUMENTS};
use eggos_translate::image::{BackMapEntry, Section, Seed, SeedOrigin, pdata_function_starts};

const RELOCATION_DIR64: u16 = 10;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
const IMPORT_BY_ORDINAL: u64 = 1 << 63;
const TLS_CALLBACKS_FIELD: usize = 24;
const EXPORT_FUNCTION_COUNT_FIELD: usize = 20;
const EXPORT_FUNCTIONS_FIELD: usize = 28;
const MAX_TLS_CALLBACKS: usize = 1024;
const MAX_EXPORTS: usize = 1 << 16;
const HWCAP_ATOMICS: u64 = 1 << 8;
const HWCAP_LRCPC: u64 = 1 << 15;
const HWCAP_FLAGM: u64 = 1 << 35;
const INTEGERS: [ArgumentKind; DEFAULT_ARGUMENTS] = [ArgumentKind::Integer; DEFAULT_ARGUMENTS];
const ONE_FLOATING_POINT: [ArgumentKind; 1] = [ArgumentKind::FloatingPoint];
const BRK_MASK: u32 = 0xFFE0_001F;
const BRK: u32 = 0xD420_0000;

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
        match (self.library.is_empty(), self.function.as_slice()) {
            (false, b"sqrtf") => &ONE_FLOATING_POINT,
            _ => &INTEGERS,
        }
    }
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

/// The code generation mode for a CPU whose `AT_HWCAP` is `capabilities`.
pub fn codegen_mode(capabilities: u64) -> CodegenMode {
    CodegenMode {
        hardware_tso: false,
        lse: capabilities & HWCAP_ATOMICS != 0,
        lrcpc: capabilities & HWCAP_LRCPC != 0,
        flagm: capabilities & HWCAP_FLAGM != 0,
    }
}

/// Why translated code trapped, from its `brk` immediate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCause {
    Breakpoint,
    InvalidOpcode,
    Refused,
    DivideError,
    FastFail,
    GeneralProtection,
    Undiscovered,
}

impl TrapCause {
    /// The cause of the `brk` that `instruction` is, if it is one of translated code's.
    pub fn of(instruction: u32) -> Option<TrapCause> {
        if instruction & BRK_MASK != BRK {
            return None;
        }
        Some(match (instruction >> 5) as u16 {
            trap::BREAKPOINT => TrapCause::Breakpoint,
            trap::INVALID_OPCODE => TrapCause::InvalidOpcode,
            trap::REFUSED => TrapCause::Refused,
            trap::DIVIDE_ERROR => TrapCause::DivideError,
            trap::FAST_FAIL => TrapCause::FastFail,
            trap::GENERAL_PROTECTION => TrapCause::GeneralProtection,
            trap::UNDISCOVERED => TrapCause::Undiscovered,
            _ => return None,
        })
    }

    /// Whether the guest address is in `x16` (the stub loaded it) rather than found through
    /// the back-map.
    pub fn address_in_x16(self) -> bool {
        matches!(self, TrapCause::Refused | TrapCause::Undiscovered)
    }

    /// The Windows status the program ends with.
    pub fn status(self) -> u32 {
        match self {
            TrapCause::Breakpoint => fault::status::BREAKPOINT,
            TrapCause::DivideError => fault::status::INTEGER_DIVIDE_BY_ZERO,
            TrapCause::FastFail => fault::status::STACK_BUFFER_OVERRUN,
            TrapCause::GeneralProtection => fault::status::ACCESS_VIOLATION,
            TrapCause::InvalidOpcode | TrapCause::Refused | TrapCause::Undiscovered => {
                fault::status::ILLEGAL_INSTRUCTION
            }
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

/// The guest relative address of the instruction whose translation holds code offset
/// `host`: the last back-map entry at or before it.
pub fn guest_rva(back_map: &[BackMapEntry], host: u32) -> Option<u32> {
    let after = back_map.partition_point(|entry| entry.host <= host);
    after.checked_sub(1).map(|index| back_map[index].rva)
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
        Some(cause) => fault::report_exception(program, image, cause.status(), guest),
        None => fault::report_exception(program, image, fault::status::ACCESS_VIOLATION, guest),
    }
}

/// A cache directory: the `.tx` files of `policy::DIRECTORY` on the disk, or a fake one in
/// tests. Names are file names in the directory.
pub trait CacheFiles {
    /// The whole file, or `None` if it cannot be read.
    fn read(&mut self, name: &str) -> Option<Vec<u8>>;
    /// Creates a new file holding `bytes`; false if it exists or cannot be written.
    fn create(&mut self, name: &str, bytes: &[u8]) -> bool;
    /// Moves `from` to `to`, replacing it, in one step.
    fn rename(&mut self, from: &str, to: &str) -> bool;
    /// Removes the file.
    fn remove(&mut self, name: &str);
    /// The files in the directory.
    fn list(&mut self) -> Vec<policy::CachedFile>;
    /// Marks the file as launched now.
    fn touch(&mut self, name: &str);
    /// The cache's cap in bytes.
    fn cap(&mut self) -> u64;
    /// The time in seconds.
    fn now(&mut self) -> u64;
}

/// The cached translation for `key`, checked, and marked as launched now; `None` when there
/// is none or it is damaged, stale or for another key (a damaged one is removed).
pub fn load(files: &mut impl CacheFiles, key: &[u8; 32]) -> Option<CacheFile> {
    let name = policy::file_name(key);
    let bytes = files.read(&name)?;
    match cache::parse(&bytes) {
        Ok(file) if cache::cache_key(&file.input_digest, file.mode) == *key => {
            files.touch(&name);
            Some(file)
        }
        _ => {
            files.remove(&name);
            None
        }
    }
}

/// Stores `bytes`, the serialized translation for `key`, written by `writer` (the process
/// id): first the evictions the cap asks for, then a temporary file renamed over the final
/// name, so a reader never sees a partial file. False when it could not be stored; the
/// program still runs from the translation in memory.
pub fn store(files: &mut impl CacheFiles, key: &[u8; 32], bytes: &[u8], writer: u64) -> bool {
    let cap = files.cap();
    if bytes.len() as u64 > cap {
        return false;
    }
    let now = files.now();
    let listed = files.list();
    for name in policy::evictions(&listed, cap, bytes.len() as u64, now) {
        files.remove(&name);
    }
    let temporary: String = policy::temporary_name(key, writer);
    if !files.create(&temporary, bytes) {
        files.remove(&temporary);
        return false;
    }
    if !files.rename(&temporary, &policy::file_name(key)) {
        files.remove(&temporary);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::Pe;
    use crate::pe::tests::{Builder, DATA, RDATA, TEST_IMAGE_BASE, TEXT};
    use eggos_translate::cache::{CacheFile, input_digest, serialize};
    use eggos_translate::image::bridge::add_import_stubs;
    use eggos_translate::image::{ImageInput, translate_image};
    use std::collections::BTreeMap;

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
    fn the_mode_follows_the_cpu_s_capabilities() {
        assert_eq!(codegen_mode(0), CodegenMode::default());
        let all = codegen_mode(HWCAP_ATOMICS | HWCAP_LRCPC | HWCAP_FLAGM);
        assert!(all.lse && all.lrcpc && all.flagm && !all.hardware_tso);
    }

    #[test]
    fn translated_code_s_traps_are_read_from_their_brk() {
        let brk = |immediate: u16| BRK | (u32::from(immediate) << 5);
        assert_eq!(TrapCause::of(brk(trap::REFUSED)), Some(TrapCause::Refused));
        assert_eq!(
            TrapCause::of(brk(trap::UNDISCOVERED)),
            Some(TrapCause::Undiscovered)
        );
        assert_eq!(
            TrapCause::of(brk(trap::DIVIDE_ERROR)),
            Some(TrapCause::DivideError)
        );
        assert_eq!(TrapCause::of(brk(0xF000)), None);
        assert_eq!(TrapCause::of(0xD503_201F), None);
        assert!(TrapCause::Refused.address_in_x16());
        assert!(!TrapCause::DivideError.address_in_x16());
        assert_eq!(TrapCause::DivideError.status(), 0xc000_0094);
        assert_eq!(TrapCause::Refused.status(), 0xc000_001d);
        assert_eq!(fault_status(fault::SIGSEGV), 0xc000_0005);
    }

    #[test]
    fn the_back_map_gives_the_guest_instruction_of_a_host_offset() {
        let map = [
            BackMapEntry {
                host: 64,
                rva: 0x1000,
            },
            BackMapEntry {
                host: 72,
                rva: 0x1003,
            },
            BackMapEntry {
                host: 96,
                rva: 0x1007,
            },
        ];
        assert_eq!(guest_rva(&map, 10), None);
        assert_eq!(guest_rva(&map, 64), Some(0x1000));
        assert_eq!(guest_rva(&map, 80), Some(0x1003));
        assert_eq!(guest_rva(&map, 400), Some(0x1007));
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

    #[derive(Default)]
    struct FakeCache {
        files: BTreeMap<String, (Vec<u8>, u64)>,
        cap: u64,
        now: u64,
        fail_rename: bool,
    }

    impl CacheFiles for FakeCache {
        fn read(&mut self, name: &str) -> Option<Vec<u8>> {
            self.files.get(name).map(|(bytes, _)| bytes.clone())
        }
        fn create(&mut self, name: &str, bytes: &[u8]) -> bool {
            if self.files.contains_key(name) {
                return false;
            }
            self.files.insert(name.into(), (bytes.to_vec(), self.now));
            true
        }
        fn rename(&mut self, from: &str, to: &str) -> bool {
            if self.fail_rename {
                return false;
            }
            match self.files.remove(from) {
                Some(file) => {
                    self.files.insert(to.into(), file);
                    true
                }
                None => false,
            }
        }
        fn remove(&mut self, name: &str) {
            self.files.remove(name);
        }
        fn list(&mut self) -> Vec<policy::CachedFile> {
            self.files
                .iter()
                .map(|(name, (bytes, last_launch))| policy::CachedFile {
                    name: name.clone(),
                    bytes: bytes.len() as u64,
                    last_launch: *last_launch,
                    in_use: false,
                })
                .collect()
        }
        fn touch(&mut self, name: &str) {
            let now = self.now;
            if let Some(file) = self.files.get_mut(name) {
                file.1 = now;
            }
        }
        fn cap(&mut self) -> u64 {
            self.cap
        }
        fn now(&mut self) -> u64 {
            self.now
        }
    }

    fn cache_file(program: &[u8]) -> (CacheFile, [u8; 32]) {
        let code = [0xc3u8];
        let sections = [Section {
            rva: TEXT,
            bytes: &code,
            executable: true,
        }];
        let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
        let input = ImageInput {
            image_base: TEST_IMAGE_BASE,
            sections: &sections,
            seeds: &seeds,
        };
        let mode = CodegenMode::default();
        let digest = input_digest(program);
        let file = CacheFile {
            input_digest: digest,
            mode,
            image_base: TEST_IMAGE_BASE,
            image: translate_image(&input, mode).unwrap(),
        };
        (file, cache::cache_key(&digest, mode))
    }

    #[test]
    fn a_stored_translation_is_renamed_into_place_and_loads_back_touched() {
        let (file, key) = cache_file(b"program one");
        let bytes = serialize(&file);
        let mut cache = FakeCache {
            cap: 1 << 30,
            now: 100,
            ..FakeCache::default()
        };
        assert!(store(&mut cache, &key, &bytes, 42));
        assert_eq!(cache.files.len(), 1);
        let name = policy::file_name(&key);
        assert!(cache.files.contains_key(&name));
        cache.now = 200;
        let loaded = load(&mut cache, &key).unwrap();
        assert_eq!(loaded.image.code, file.image.code);
        assert_eq!(cache.files[&name].1, 200);
    }

    #[test]
    fn a_failed_rename_leaves_no_temporary_and_a_damaged_file_is_removed() {
        let (file, key) = cache_file(b"program two");
        let bytes = serialize(&file);
        let mut cache = FakeCache {
            cap: 1 << 30,
            fail_rename: true,
            ..FakeCache::default()
        };
        assert!(!store(&mut cache, &key, &bytes, 7));
        assert!(cache.files.is_empty());
        let mut damaged = bytes.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0xff;
        cache.files.insert(policy::file_name(&key), (damaged, 0));
        assert!(load(&mut cache, &key).is_none());
        assert!(cache.files.is_empty());
    }

    #[test]
    fn storing_past_the_cap_evicts_the_least_recently_launched_file() {
        let (first, first_key) = cache_file(b"first");
        let (second, second_key) = cache_file(b"second");
        let first = serialize(&first);
        let second = serialize(&second);
        let mut cache = FakeCache {
            cap: first.len() as u64 + second.len() as u64 / 2,
            now: 10,
            ..FakeCache::default()
        };
        assert!(store(&mut cache, &first_key, &first, 1));
        cache.now = 20;
        assert!(store(&mut cache, &second_key, &second, 1));
        assert!(!cache.files.contains_key(&policy::file_name(&first_key)));
        assert!(cache.files.contains_key(&policy::file_name(&second_key)));
        let mut tiny = FakeCache::default();
        assert!(!store(&mut tiny, &first_key, &first, 1));
    }
}
