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

fn with_sections(mut command: Vec<u8>, sections: &[(&[u8], u64, u64)]) -> Vec<u8> {
    let size = 72 + 80 * sections.len() as u32;
    command[4..8].copy_from_slice(&size.to_le_bytes());
    command[64..68].copy_from_slice(&(sections.len() as u32).to_le_bytes());
    for &(name, address, length) in sections {
        let mut padded = [0u8; 32];
        padded[..name.len()].copy_from_slice(name);
        command.extend_from_slice(&padded);
        put64(&mut command, address);
        put64(&mut command, length);
        for value in [0u32, 3, 0, 0, 0, 0, 0, 0] {
            put32(&mut command, value);
        }
    }
    command
}

fn executable(cputype: u32, pointer_format: u16) -> Vec<u8> {
    executable_with(cputype, pointer_format, &[])
}

fn executable_with(
    cputype: u32,
    pointer_format: u16,
    data_sections: &[(&[u8], u64, u64)],
) -> Vec<u8> {
    let fixups = chained_fixups(pointer_format);
    let mut commands = Vec::new();
    commands.extend(segment(b"__PAGEZERO", 0, TEXT, 0, 0, 0, 0));
    commands.extend(segment(b"__TEXT", TEXT, 0x4000, 0, 0x4000, 5, 0));
    commands.extend(with_sections(
        segment(
            b"__DATA_CONST",
            TEXT + 0x4000,
            0x4000,
            0x4000,
            0x4000,
            3,
            0x10,
        ),
        data_sections,
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
fn objective_c_sections_are_found_by_name_in_any_segment() {
    let file = executable_with(
        CPU_TYPE_ARM64,
        6,
        &[
            (b"__objc_classlist", TEXT + 0x4100, 16),
            (b"__objc_selrefs", TEXT + 0x4200, 24),
            (b"__objc_imageinfo", TEXT + 0x4300, 8),
            (b"__got", TEXT + 0x4400, 8),
        ],
    );
    let program = MachO::parse(&file).unwrap();
    let objc = program.objc;
    assert!(objc.present());
    assert_eq!(
        objc.class_list,
        Some(macho::SectionRange {
            address: TEXT + 0x4100,
            size: 16
        })
    );
    assert_eq!(objc.selector_references.map(|range| range.size), Some(24));
    assert_eq!(
        objc.image_info.map(|range| range.address),
        Some(TEXT + 0x4300)
    );
    assert_eq!(objc.category_list, None);
    assert!(
        !MachO::parse(&executable(CPU_TYPE_ARM64, 6))
            .unwrap()
            .objc
            .present()
    );
}

#[test]
fn an_objective_c_section_outside_its_segment_or_misaligned_is_refused() {
    for section in [
        (&b"__objc_classlist"[..], TEXT + 0x7ff8, 16),
        (&b"__objc_selrefs"[..], TEXT + 0x4104, 8),
        (&b"__objc_catlist"[..], TEXT + 0x4100, 12),
    ] {
        let file = executable_with(CPU_TYPE_ARM64, 6, &[section]);
        assert!(MachO::parse(&file).is_err(), "{:?}", section.0);
    }
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

#[test]
fn calendar_breaks_down_and_rebuilds_utc_times() {
    use crate::calendar::{BrokenDown, from_seconds, to_seconds};
    let epoch = from_seconds(0).unwrap();
    assert_eq!(
        epoch,
        BrokenDown {
            day: 1,
            year: 70,
            weekday: 4,
            ..BrokenDown::default()
        }
    );
    let later = from_seconds(1_700_000_000).unwrap();
    assert_eq!(
        (
            later.year,
            later.month,
            later.day,
            later.hour,
            later.minute,
            later.second
        ),
        (123, 10, 14, 22, 13, 20)
    );
    assert_eq!((later.weekday, later.year_day), (2, 317));
    let leap_day = from_seconds(951_782_400).unwrap();
    assert_eq!((leap_day.year, leap_day.month, leap_day.day), (100, 1, 29));
    let before = from_seconds(-1).unwrap();
    assert_eq!(
        (before.year, before.month, before.day, before.hour),
        (69, 11, 31, 23)
    );
    for seconds in [
        0,
        1_700_000_000,
        951_782_400,
        -1,
        -86_400 * 365 * 400,
        4_102_444_800,
    ] {
        assert_eq!(to_seconds(&from_seconds(seconds).unwrap()), seconds);
    }
    let overflowing = BrokenDown { month: 13, ..later };
    assert_eq!(to_seconds(&overflowing), 1_700_000_000 + 92 * 86_400);
    assert_eq!(from_seconds(i64::MAX), None);
}

#[test]
fn strftime_formats_the_c_locale_and_refuses_what_does_not_fit() {
    use crate::calendar::{format, from_seconds};
    let time = from_seconds(1_700_000_000).unwrap();
    let mut buffer = [0u8; 128];
    let mut expand = |spec: &[u8]| {
        let length = format(spec, &time, &mut buffer).unwrap();
        String::from_utf8(buffer[..length].to_vec()).unwrap()
    };
    assert_eq!(expand(b"%Y-%m-%d %H:%M:%S"), "2023-11-14 22:13:20");
    assert_eq!(
        expand(b"%a %A %b %B %h %j %u %w"),
        "Tue Tuesday Nov November Nov 318 2 2"
    );
    assert_eq!(
        expand(b"%c|%x|%X|%D|%F|%T|%R"),
        "Tue Nov 14 22:13:20 2023|11/14/23|22:13:20|11/14/23|2023-11-14|22:13:20|22:13"
    );
    assert_eq!(
        expand(b"%I %l %p %r %e %C %y %s"),
        "10 10 PM 10:13:20 PM 14 20 23 1700000000"
    );
    assert_eq!(expand(b"%Z %z %% %n%t %Ey %q"), "UTC +0000 % \n\t 23 %q");
    let mut small = [0u8; 4];
    assert_eq!(format(b"%Y", &time, &mut small), None);
    assert_eq!(format(b"abc", &time, &mut small), Some(3));
}

#[test]
fn strftime_in_en_us_writes_four_digit_years_for_x() {
    use crate::calendar::{TimeFormats, format_in, from_seconds};
    let time = from_seconds(1_772_719_629).unwrap();
    let mut buffer = [0u8; 64];
    let length = format_in(TimeFormats::UNITED_STATES, b"%x|%c|%D", &time, &mut buffer).unwrap();
    assert_eq!(
        &buffer[..length],
        b"03/05/2026|Thu Mar  5 14:07:09 2026|03/05/26"
    );
}

type Fields = (i32, i32, i32, i32, i32, i32, i32, i32);

fn strptime_fields(
    text: &str,
    format: &str,
    formats: crate::calendar::TimeFormats,
) -> Option<(usize, Fields)> {
    use crate::calendar::{BrokenDown, parse};
    let mut time = BrokenDown {
        weekday: -9,
        year_day: -9,
        ..BrokenDown::default()
    };
    let parsed = parse(text.as_bytes(), format.as_bytes(), formats, &mut time)?;
    let fields = (
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
        time.weekday,
        time.year_day,
    );
    Some((parsed.used, fields))
}

#[test]
fn strptime_reads_what_a_mac_reads() {
    use crate::calendar::TimeFormats;
    let c = TimeFormats::C;
    let read = |text, format| strptime_fields(text, format, c);
    let full = (126, 2, 5, 14, 7, 9, 4, 63);
    assert_eq!(
        read("2026-03-05 14:07:09", "%Y-%m-%d %H:%M:%S"),
        Some((19, full))
    );
    assert_eq!(read("Thu Mar  5 14:07:09 2026", "%c"), Some((24, full)));
    assert_eq!(read("1772719629", "%s"), Some((10, full)));
    let date = (126, 2, 5, 0, 0, 0, 4, 63);
    assert_eq!(read("2026-3-5x", "%Y-%m-%d"), Some((8, date)));
    assert_eq!(read("03/05/26", "%x"), Some((8, date)));
    assert_eq!(read("2026 64", "%Y %j"), Some((7, date)));
    assert_eq!(read("2026-03-05T", "%FT"), Some((11, date)));
    assert_eq!(
        strptime_fields("03/05/2026", "%x", TimeFormats::UNITED_STATES),
        Some((10, date))
    );
    assert_eq!(
        read("thursday, MARCH 5", "%A, %B %e"),
        Some((17, (0, 2, 5, 0, 0, 0, 4, -9)))
    );
    assert_eq!(read("Thurs", "%a"), Some((3, (0, 0, 0, 0, 0, 0, 4, -9))));
    assert_eq!(
        read("Thursdayx", "%A"),
        Some((8, (0, 0, 0, 0, 0, 0, 4, -9)))
    );
    assert_eq!(
        read("02:07:09 PM", "%r"),
        Some((11, (0, 0, 0, 14, 7, 9, -9, -9)))
    );
    assert_eq!(
        read("12 am", "%I %p"),
        Some((5, (0, 0, 0, 0, 0, 0, -9, -9)))
    );
    assert_eq!(read("PM 3", "%p %I"), Some((4, (0, 0, 0, 3, 0, 0, -9, -9))));
    assert_eq!(read("69", "%y").map(|(_, time)| time.0), Some(69));
    assert_eq!(read("68", "%y").map(|(_, time)| time.0), Some(168));
    assert_eq!(read("20 26", "%C %y").map(|(_, time)| time.0), Some(126));
    assert_eq!(
        read("12345", "%Y").map(|(used, time)| (used, time.0)),
        Some((4, -666))
    );
    assert_eq!(
        read(" 5", "%e").map(|(used, time)| (used, time.2)),
        Some((2, 5))
    );
    assert_eq!(read("60", "%S").map(|(_, time)| time.5), Some(60));
    for (text, format) in [
        ("  5", "%d"),
        ("13 PM", "%H %p"),
        ("13 PM", "%I %p"),
        ("24", "%H"),
        ("61", "%S"),
        ("32", "%d"),
        ("13", "%m"),
        ("367", "%j"),
        ("5x", "%d %%"),
        ("X5", "x%d"),
        ("abc", "%d"),
    ] {
        assert_eq!(read(text, format), None, "{text} {format}");
    }
}

#[test]
fn strptime_reads_zones_and_leaves_time_alone_when_it_fails() {
    use crate::calendar::{BrokenDown, TimeFormats, parse};
    let mut time = BrokenDown::default();
    let parsed = parse(b"+0700 UTC", b"%z %Z", TimeFormats::C, &mut time).unwrap();
    assert_eq!(
        (parsed.used, parsed.offset, parsed.standard_time),
        (9, Some(25_200), true)
    );
    let parsed = parse(b"-05:30", b"%z", TimeFormats::C, &mut time).unwrap();
    assert_eq!(parsed.offset, Some(-19_800));
    let before = BrokenDown {
        hour: 7,
        ..BrokenDown::default()
    };
    let mut after = before;
    assert_eq!(parse(b"11:xx", b"%H:%M", TimeFormats::C, &mut after), None);
    assert_eq!(after, before);
}

#[test]
fn getopt_parses_short_and_long_options_like_bsd() {
    use crate::getopt::{Cursor, Found, Takes, next, quiet};
    let words: &[&[u8]] = &[
        b"tool",
        b"-vn3",
        b"-n",
        b"4",
        b"-x",
        b"-o",
        b"-ofile",
        b"--list",
        b"/bin",
        b"--name=a",
        b"--ver",
        b"--na",
        b"--bogus",
        b"--verbose=1",
        b"--",
        b"-v",
    ];
    let longs: [(&[u8], Takes); 4] = [
        (b"list", Takes::Required),
        (b"name", Takes::Required),
        (b"names", Takes::Optional),
        (b"verbose", Takes::Nothing),
    ];
    let word = |index: usize| words.get(index).copied();
    let mut cursor = Cursor {
        index: 1,
        offset: 0,
    };
    let mut found = Vec::new();
    loop {
        let next = next(word, b"vn:o::", longs.iter().copied(), true, &mut cursor);
        found.push(next);
        if next == Found::Done {
            break;
        }
    }
    assert_eq!(
        found,
        [
            Found::Short(b'v', None),
            Found::Short(b'n', Some((1, 3))),
            Found::Short(b'n', Some((3, 0))),
            Found::UnknownShort(b'x'),
            Found::Short(b'o', None),
            Found::Short(b'o', Some((6, 2))),
            Found::Long(0, Some((8, 0))),
            Found::Long(1, Some((9, 7))),
            Found::Long(3, None),
            Found::AmbiguousLong(11),
            Found::UnknownLong(12),
            Found::UnexpectedLong(3),
            Found::Done,
        ]
    );
    assert_eq!(cursor.index, 15);
    let mut at_operand = Cursor {
        index: 1,
        offset: 0,
    };
    let operands: &[&[u8]] = &[b"tool", b"file", b"-v"];
    let operand = |index: usize| operands.get(index).copied();
    assert_eq!(
        next(operand, b"v", [].into_iter(), false, &mut at_operand),
        Found::Done
    );
    assert_eq!(at_operand.index, 1);
    let missing: &[&[u8]] = &[b"tool", b"-n"];
    let mut at_missing = Cursor {
        index: 1,
        offset: 0,
    };
    let missing_word = |index: usize| missing.get(index).copied();
    assert_eq!(
        next(missing_word, b":n:", [].into_iter(), false, &mut at_missing),
        Found::MissingShort(b'n')
    );
    assert!(quiet(b":n:") && quiet(b"+:n") && !quiet(b"n:"));
    let long_missing: &[&[u8]] = &[b"tool", b"--list"];
    let mut at_long = Cursor {
        index: 1,
        offset: 0,
    };
    let long_word = |index: usize| long_missing.get(index).copied();
    assert_eq!(
        next(long_word, b"", longs.iter().copied(), true, &mut at_long),
        Found::MissingLong(0)
    );
}

#[test]
fn darwin_mmap_and_status_flags_translate_to_linux() {
    use crate::darwin::{darwin_status_flags, linux_map_flags, linux_status_flags};
    assert_eq!(linux_map_flags(0x1002), Some(0x22));
    assert_eq!(linux_map_flags(0x1012 | 0x40 | 0x800), Some(0x4032));
    assert_eq!(linux_map_flags(0x0001), Some(0x01));
    assert_eq!(linux_map_flags(0x1000), None);
    assert_eq!(linux_map_flags(0x1003), None);
    assert_eq!(linux_map_flags(0x2_0002), None);
    assert_eq!(
        darwin_status_flags(0o2 | 0o4000 | 0o2000 | 0o100000),
        2 | 4 | 8
    );
    assert_eq!(linux_status_flags(4 | 8 | 1), 0o6000);
}

#[test]
fn strftime_prints_a_question_mark_for_out_of_range_names_and_never_panics() {
    use crate::calendar::{BrokenDown, format};
    let mut buffer = [0u8; 64];
    for (weekday, month) in [(9, 0), (-1, 12), (7, -1), (i32::MAX, i32::MIN)] {
        let time = BrokenDown {
            day: 1,
            year: 70,
            weekday,
            month,
            ..BrokenDown::default()
        };
        let length = format(b"%a %A %b %B %h", &time, &mut buffer).unwrap();
        let text = core::str::from_utf8(&buffer[..length]).unwrap();
        assert!(text.contains('?'), "{text}");
    }
    let invalid = BrokenDown {
        weekday: 9,
        month: 13,
        ..BrokenDown::default()
    };
    let length = format(b"%a %b", &invalid, &mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"? ?");
}

#[test]
fn darwin_and_linux_signal_numbers_map_both_ways() {
    use crate::darwin::linux_signal;
    use crate::darwin_signals::{LAST_SIGNAL, darwin_signal};
    for darwin in 1..=LAST_SIGNAL {
        if let Some(linux) = linux_signal(darwin) {
            assert_eq!(darwin_signal(linux), Some(darwin), "{darwin}");
        }
    }
    assert_eq!((linux_signal(7), linux_signal(29)), (None, None));
    assert_eq!((darwin_signal(16), darwin_signal(30)), (None, None));
    assert_eq!(darwin_signal(34), None);
    assert_eq!((darwin_signal(10), darwin_signal(17)), (Some(30), Some(20)));
    assert_eq!((darwin_signal(7), darwin_signal(31)), (Some(10), Some(12)));
}

#[test]
fn signal_sets_masks_and_action_flags_translate_to_linux() {
    use crate::darwin_signals::{self, action, linux_action};
    let bit = |signal: u32| 1u32 << (signal - 1);
    let darwin_set = bit(30) | bit(20) | bit(2) | bit(7);
    let linux = darwin_signals::linux_signal_set(darwin_set);
    assert_eq!(linux, (1 << 9) | (1 << 16) | (1 << 1));
    assert_eq!(
        darwin_signals::darwin_signal_set(linux),
        darwin_set & !bit(7)
    );
    assert_eq!(darwin_signals::darwin_signal_set(1 << 33), 0);
    let flags = action::SIGINFO | action::RESTART | action::ONSTACK | 0x100;
    assert_eq!(
        darwin_signals::linux_action_flags(flags),
        linux_action::SIGINFO | linux_action::RESTART | linux_action::ONSTACK
    );
    assert_eq!(
        darwin_signals::linux_action_flags(action::RESETHAND | action::NODEFER),
        linux_action::RESETHAND | linux_action::NODEFER
    );
    let hows = [0, 1, 2, 3, 4].map(darwin_signals::linux_mask_how);
    assert_eq!(hows, [None, Some(0), Some(1), Some(2), None]);
}

#[test]
fn wait_statuses_and_options_use_darwin_signal_numbers() {
    use crate::darwin_signals::{darwin_wait_status, linux_wait_options};
    assert_eq!(darwin_wait_status(3 << 8), 3 << 8);
    assert_eq!(darwin_wait_status(10), 30);
    assert_eq!(darwin_wait_status(0x80 | 7), 0x80 | 10);
    assert_eq!(darwin_wait_status((19 << 8) | 0x7f), (17 << 8) | 0x7f);
    assert_eq!(darwin_wait_status(0xffff), (19 << 8) | 0x7f);
    assert_eq!(linux_wait_options(1 | 2 | 0x10), Some(1 | 2 | 8));
    assert_eq!(linux_wait_options(0), Some(0));
    assert_eq!(linux_wait_options(0x20), None);
}

fn linux_signal_info(signal: i32, code: i32, first: u32, second: u32, value: u64) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&signal.to_le_bytes());
    bytes[8..12].copy_from_slice(&code.to_le_bytes());
    bytes[16..20].copy_from_slice(&first.to_le_bytes());
    bytes[20..24].copy_from_slice(&second.to_le_bytes());
    bytes[24..].copy_from_slice(&value.to_le_bytes());
    bytes
}

#[test]
fn signal_info_says_who_sent_it_what_faulted_or_how_a_child_ended() {
    use crate::darwin_signals::{code, darwin_signal_info};
    let sent = darwin_signal_info(&linux_signal_info(10, 0, 42, 7, 0));
    assert_eq!(
        (sent.signal, sent.code, sent.pid, sent.uid),
        (30, code::USER, 42, 7)
    );
    let queued = darwin_signal_info(&linux_signal_info(12, -1, 42, 7, 99));
    assert_eq!(
        (queued.signal, queued.code, queued.value),
        (31, code::QUEUE, 99)
    );
    let thread = darwin_signal_info(&linux_signal_info(10, -6, 42, 7, 0));
    assert_eq!(thread.code, code::USER);
    let fault = darwin_signal_info(&linux_signal_info(11, 1, 0x9abc_def0, 0x1234, 0));
    assert_eq!((fault.signal, fault.code), (11, 1));
    assert_eq!((fault.address, fault.pid), (0x1234_9abc_def0, 0));
    let divide = darwin_signal_info(&linux_signal_info(8, 1, 0x1000, 0, 0));
    assert_eq!((divide.code, divide.address), (7, 0x1000));
    let illegal = darwin_signal_info(&linux_signal_info(4, 2, 0x2000, 0, 0));
    assert_eq!(illegal.code, 4);
    let killed = darwin_signal_info(&linux_signal_info(17, 2, 5, 0, 10));
    assert_eq!((killed.signal, killed.code), (20, 2));
    assert_eq!((killed.pid, killed.status), (5, 30));
    let exited = darwin_signal_info(&linux_signal_info(17, 1, 5, 0, 3));
    assert_eq!(exited.status, 3);
    assert_eq!(size_of::<crate::darwin_signals::SignalInfo>(), 104);
}

#[test]
fn posix_spawn_takes_its_attributes_then_its_file_actions_in_order() {
    use crate::spawn_actions::{self, Attributes, FileAction, Step, flag};
    let attributes = Attributes {
        flags: flag::SETSID | flag::SETSIGMASK | flag::CLOEXEC_DEFAULT | flag::SETPGROUP,
        process_group: 0,
        default_signals: 0,
        mask: 4,
    };
    let actions = [
        FileAction::Open {
            fd: 1,
            path: "out",
            flags: 0o1101,
            mode: 0o644,
        },
        FileAction::Duplicate { from: 1, to: 2 },
        FileAction::Duplicate { from: 3, to: 3 },
        FileAction::Inherit { fd: 4 },
        FileAction::Close { fd: 5 },
        FileAction::ChangeDirectory { path: "/tmp" },
        FileAction::ChangeDirectoryTo { fd: 6 },
    ];
    let steps: Vec<Step<&str>> = spawn_actions::steps(attributes, &actions).collect();
    assert_eq!(
        steps,
        [
            Step::CloseAllOnExec,
            Step::NewSession,
            Step::SetProcessGroup(0),
            Step::SetMask(4),
            Step::Open {
                fd: 1,
                path: "out",
                flags: 0o1101,
                mode: 0o644,
            },
            Step::Duplicate { from: 1, to: 2 },
            Step::KeepAcrossExec(3),
            Step::KeepAcrossExec(4),
            Step::Close(5),
            Step::ChangeDirectory("/tmp"),
            Step::ChangeDirectoryTo(6),
        ]
    );
    let resets = Attributes {
        flags: flag::RESETIDS | flag::SETSIGDEF,
        default_signals: 6,
        ..Attributes::default()
    };
    let none: [FileAction<&str>; 0] = [];
    let reset_steps: Vec<Step<&str>> = spawn_actions::steps(resets, &none).collect();
    assert_eq!(reset_steps, [Step::ResetIds, Step::DefaultSignals(6)]);
    assert_eq!(
        spawn_actions::steps(Attributes::default(), &none).count(),
        0
    );
}

#[test]
fn posix_spawn_refuses_only_a_suspended_start_and_checks_descriptors() {
    use crate::spawn_actions::{self, Attributes, OPEN_MAX, flag};
    let with = |flags| Attributes {
        flags,
        ..Attributes::default()
    };
    assert_eq!(Attributes::default().check(), Ok(()));
    assert_eq!(with(flag::START_SUSPENDED).check(), Err(45));
    assert_eq!(with(0x0100).check(), Ok(()));
    assert!(with(flag::SETEXEC).replaces_caller());
    assert!(!with(flag::SETSID).replaces_caller());
    assert!(spawn_actions::valid_descriptor(0));
    assert!(spawn_actions::valid_descriptor(OPEN_MAX - 1));
    assert!(!spawn_actions::valid_descriptor(-1));
    assert!(!spawn_actions::valid_descriptor(OPEN_MAX));
}

#[test]
fn execvp_tries_each_directory_of_the_search_path() {
    use crate::spawn_actions::{PATH_MAX, candidate, is_path};
    let mut buffer = [0; PATH_MAX];
    assert_eq!(
        candidate(b"/bin", b"eggsh", &mut buffer),
        Ok(&b"/bin/eggsh\0"[..])
    );
    assert_eq!(candidate(b"", b"tool", &mut buffer), Ok(&b"./tool\0"[..]));
    let long = [b'a'; PATH_MAX];
    assert_eq!(
        candidate(&long[..PATH_MAX - 3], b"x", &mut buffer).map(<[u8]>::len),
        Ok(PATH_MAX)
    );
    assert_eq!(candidate(&long[..PATH_MAX - 2], b"x", &mut buffer), Err(63));
    assert!(is_path(b"./tool") && is_path(b"/bin/eggsh"));
    assert!(!is_path(b"eggsh"));
}
