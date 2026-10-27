use crate::fixups::{ChainedFixups, Source};
use crate::macho::{
    self, CPU_TYPE_ARM64, CPU_TYPE_X86_64, Cpu, InvalidMachO, MH_MAGIC_64, MachO, NoSlice,
};

const TEXT: u64 = 0x1_0000_0000;
const LOAD_ADDRESS: u64 = 0x7000_0000_0000;
const FIXUPS_AT: usize = 0x8000;

fn put32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn segment(
    name: &[u8],
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    prot: u32,
    flags: u32,
) -> Vec<u8> {
    let mut command = Vec::new();
    put32(&mut command, 0x19);
    put32(&mut command, 72);
    let mut padded = [0u8; 16];
    padded[..name.len()].copy_from_slice(name);
    command.extend_from_slice(&padded);
    for value in [vmaddr, vmsize, fileoff, filesize] {
        put64(&mut command, value);
    }
    for value in [prot, prot, 0, flags] {
        put32(&mut command, value);
    }
    command
}

fn string_command(kind: u32, offset: u32, text: &[u8]) -> Vec<u8> {
    let mut command = Vec::new();
    put32(&mut command, kind);
    let size = (offset as usize + text.len() + 1).next_multiple_of(8);
    put32(&mut command, size as u32);
    put32(&mut command, offset);
    command.resize(offset as usize, 0);
    command.extend_from_slice(text);
    command.resize(size, 0);
    command
}

fn chained_fixups(pointer_format: u16) -> Vec<u8> {
    let mut data = Vec::new();
    for value in [0, 32, 80, 88, 2, 1, 0] {
        put32(&mut data, value);
    }
    data.resize(32, 0);
    put32(&mut data, 4);
    for offset in [0, 0, 24, 0] {
        put32(&mut data, offset);
    }
    data.resize(56, 0);
    put32(&mut data, 24);
    data.extend_from_slice(&0x4000u16.to_le_bytes());
    data.extend_from_slice(&pointer_format.to_le_bytes());
    put64(&mut data, 0x4000);
    put32(&mut data, 0);
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    assert_eq!(data.len(), 80);
    put32(&mut data, 1);
    put32(&mut data, 1 | 0x100 | (6 << 9));
    data.extend_from_slice(b"_puts\0_weak\0");
    data
}

fn executable(cputype: u32, pointer_format: u16) -> Vec<u8> {
    let fixups = chained_fixups(pointer_format);
    let mut commands = Vec::new();
    commands.extend(segment(b"__PAGEZERO", 0, TEXT, 0, 0, 0, 0));
    commands.extend(segment(b"__TEXT", TEXT, 0x4000, 0, 0x4000, 5, 0));
    commands.extend(segment(
        b"__DATA_CONST",
        TEXT + 0x4000,
        0x4000,
        0x4000,
        0x4000,
        3,
        0x10,
    ));
    commands.extend(segment(
        b"__LINKEDIT",
        TEXT + 0x8000,
        0x4000,
        0x8000,
        0x200,
        1,
        0,
    ));
    let mut fixups_command = Vec::new();
    for value in [0x8000_0034, 16, FIXUPS_AT as u32, fixups.len() as u32] {
        put32(&mut fixups_command, value);
    }
    commands.extend(fixups_command);
    commands.extend(string_command(0xe, 12, b"/usr/lib/dyld"));
    let mut main = Vec::new();
    put32(&mut main, 0x8000_0028);
    put32(&mut main, 24);
    put64(&mut main, 0x1000);
    put64(&mut main, 0);
    commands.extend(main);
    commands.extend(string_command(0xc, 24, macho::LIBSYSTEM));
    let mut file = Vec::new();
    for value in [
        MH_MAGIC_64,
        cputype,
        0,
        2,
        8,
        commands.len() as u32,
        0x20_0085,
        0,
    ] {
        put32(&mut file, value);
    }
    file.extend(commands);
    file.resize(0x4000, 0);
    for raw in [
        0x8010_0000_0000_0000u64,
        0x8010_0000_0000_0001 | (3 << 24),
        if pointer_format == 2 {
            TEXT + 0x1234
        } else {
            0x1234
        } | (0x5a << 36),
    ] {
        put64(&mut file, raw);
    }
    file.resize(FIXUPS_AT, 0);
    file.extend(fixups);
    file.resize(FIXUPS_AT + 0x200, 0);
    file
}

fn laid_out(program: &MachO<'_>) -> Vec<u8> {
    let mut image = vec![0u8; program.mapped_size() as usize];
    for segment in program.segments() {
        let at = (segment.vmaddr - program.base_address()) as usize;
        let (from, length) = (segment.fileoff as usize, segment.filesize as usize);
        image[at..at + length].copy_from_slice(&program.bytes[from..from + length]);
    }
    image
}

fn word(image: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(image[at..at + 8].try_into().unwrap())
}

#[test]
fn an_executable_parses_with_its_segments_entry_and_library() {
    let file = executable(CPU_TYPE_ARM64, 2);
    let program = MachO::parse(&file).unwrap();
    assert_eq!(program.cpu, Cpu::Arm64);
    assert!(program.pie);
    assert_eq!(program.entry, 0x1000);
    assert_eq!(program.base_address(), TEXT);
    assert_eq!(program.mapped_size(), 0xc000);
    let names: Vec<&[u8]> = program.segments().iter().map(|s| s.name()).collect();
    assert_eq!(names, [&b"__TEXT"[..], b"__DATA_CONST", b"__LINKEDIT"]);
    assert_eq!(program.library(1).unwrap().path, macho::LIBSYSTEM);
    assert_eq!(program.library(2), None);
    assert_eq!(program.library(0), None);
    assert!(!program.dyld_info);
}

#[test]
fn chained_fixups_bind_imports_and_rebase_by_the_slide() {
    for format in [2, 6] {
        let file = executable(CPU_TYPE_X86_64, format);
        let program = MachO::parse(&file).unwrap();
        let fixups = ChainedFixups::parse(&program, program.chained_fixups.unwrap()).unwrap();
        assert_eq!(fixups.import_count(), 2);
        let puts = fixups.import(0).unwrap();
        assert_eq!(
            (puts.name, puts.weak, puts.source),
            (&b"_puts"[..], false, Source::Library(1))
        );
        let weak = fixups.import(1).unwrap();
        assert_eq!((weak.name, weak.weak), (&b"_weak"[..], true));
        assert!(fixups.import(2).is_err());
        let mut image = laid_out(&program);
        fixups
            .apply(&program, &mut image, LOAD_ADDRESS, &[0x1111, 0x2222])
            .unwrap();
        assert_eq!(word(&image, 0x4000), 0x1111);
        assert_eq!(word(&image, 0x4008), 0x2222 + 3);
        assert_eq!(word(&image, 0x4010), (LOAD_ADDRESS + 0x1234) | (0x5a << 56));
    }
}

#[test]
fn a_bind_to_a_missing_import_or_a_chain_out_of_its_page_is_refused() {
    let file = executable(CPU_TYPE_ARM64, 2);
    let program = MachO::parse(&file).unwrap();
    let fixups = ChainedFixups::parse(&program, program.chained_fixups.unwrap()).unwrap();
    let mut image = laid_out(&program);
    assert!(
        fixups
            .apply(&program, &mut image, LOAD_ADDRESS, &[1])
            .is_err()
    );
    let mut runaway = file.clone();
    runaway[0x4010..0x4018].copy_from_slice(&(0xfffu64 << 51).to_le_bytes());
    let program = MachO::parse(&runaway).unwrap();
    let mut image = laid_out(&program);
    assert_eq!(
        fixups.apply(&program, &mut image, LOAD_ADDRESS, &[1, 2]),
        Err(InvalidMachO(
            "a chained pointer is outside its page or the image"
        ))
    );
}

fn fat(slices: &[(u32, &[u8])]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend_from_slice(&macho::FAT_MAGIC.to_be_bytes());
    file.extend_from_slice(&(slices.len() as u32).to_be_bytes());
    let mut offset = 0x4000u32;
    for (cputype, bytes) in slices {
        for value in [*cputype, 0, offset, bytes.len() as u32, 14] {
            file.extend_from_slice(&value.to_be_bytes());
        }
        offset = (offset + bytes.len() as u32).next_multiple_of(0x4000);
    }
    for (_, bytes) in slices {
        file.resize(file.len().next_multiple_of(0x4000), 0);
        file.extend_from_slice(bytes);
    }
    file
}

#[test]
fn a_fat_file_gives_the_slice_for_the_cpu() {
    let arm = executable(CPU_TYPE_ARM64, 2);
    let intel = executable(CPU_TYPE_X86_64, 2);
    let file = fat(&[(CPU_TYPE_X86_64, &intel), (CPU_TYPE_ARM64, &arm)]);
    assert!(macho::is_fat(&file));
    assert_eq!(macho::select_slice(&file, Cpu::Arm64).unwrap(), &arm[..]);
    assert_eq!(macho::select_slice(&file, Cpu::X86_64).unwrap(), &intel[..]);
    assert_eq!(
        macho::select_slice(&file, Cpu::Other(0x0100_0018)),
        Err(NoSlice::OtherCpus {
            found: [Some(Cpu::X86_64), Some(Cpu::Arm64), None, None]
        })
    );
    assert_eq!(macho::select_slice(&arm, Cpu::Arm64).unwrap(), &arm[..]);
    assert!(matches!(
        macho::select_slice(&arm, Cpu::X86_64),
        Err(NoSlice::OtherCpus {
            found: [Some(Cpu::Arm64), None, None, None]
        })
    ));
    let mut outside = file.clone();
    outside[8 + 12..8 + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        macho::select_slice(&outside, Cpu::X86_64),
        Err(NoSlice::Invalid(_))
    ));
}

#[test]
fn a_java_class_file_is_not_a_fat_file() {
    let class = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x41, 0, 0];
    assert!(!macho::is_fat(&class));
    assert!(matches!(
        macho::select_slice(&class, Cpu::Arm64),
        Err(NoSlice::Invalid(_))
    ));
}

#[test]
fn bad_layouts_are_refused() {
    let file = executable(CPU_TYPE_ARM64, 2);
    let mut library = file.clone();
    library[12] = 6;
    assert!(MachO::parse(&library).is_err());
    let mut writable_code = file.clone();
    let text_prot = 32 + 72 + 60;
    writable_code[text_prot..text_prot + 4].copy_from_slice(&7u32.to_le_bytes());
    assert_eq!(
        MachO::parse(&writable_code).err(),
        Some(InvalidMachO("a segment is both writable and executable"))
    );
    let mut entry_outside = file.clone();
    let entry = file
        .windows(4)
        .position(|w| w == 0x8000_0028u32.to_le_bytes())
        .unwrap()
        + 8;
    entry_outside[entry..entry + 8].copy_from_slice(&0x5000u64.to_le_bytes());
    assert_eq!(
        MachO::parse(&entry_outside).err(),
        Some(InvalidMachO("the entry point is not in executable code"))
    );
}

fn load_without_panicking(file: &[u8]) {
    let Ok(program) = MachO::parse(file) else {
        return;
    };
    let Some(range) = program.chained_fixups else {
        return;
    };
    let Ok(fixups) = ChainedFixups::parse(&program, range) else {
        return;
    };
    for index in 0..fixups.import_count() {
        let _ = fixups.import(index);
    }
    let mut image = laid_out(&program);
    let _ = fixups.apply(&program, &mut image, LOAD_ADDRESS, &[1, 2]);
}

#[test]
fn every_truncation_and_single_byte_change_is_refused_or_loads() {
    let file = executable(CPU_TYPE_ARM64, 2);
    for length in 0..file.len() {
        load_without_panicking(&file[..length]);
    }
    let interesting = (0..0x200)
        .chain(0x4000..0x4018)
        .chain(FIXUPS_AT..FIXUPS_AT + 100);
    for at in interesting {
        for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
            let mut changed = file.clone();
            changed[at] = value;
            load_without_panicking(&changed);
            let _ = macho::select_slice(&fat(&[(CPU_TYPE_ARM64, &changed)]), Cpu::Arm64);
        }
    }
}
