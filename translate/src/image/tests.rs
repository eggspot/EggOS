use super::link::{Limits, translate_image_with, veneer_target};
use super::*;
use crate::codegen::aarch64::encode::{self, Bitfield};
use crate::codegen::aarch64::{Reg, Size};
use crate::codegen::flags::Flags;
use crate::codegen::{BlockEnd, CodegenMode, trap};

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const RODATA: u32 = 0x3000;

const MODE: CodegenMode = CodegenMode {
    hardware_tso: false,
    lse: true,
    lrcpc: true,
    flagm: true,
};

const LOOP: [u8; 10] = [0x31, 0xC0, 0xFF, 0xC0, 0x83, 0xF8, 0x0A, 0x75, 0xF9, 0xC3];

const SWITCH: [u8; 42] = [
    0x83, 0xFF, 0x02, 0x77, 0x22, 0x48, 0x8D, 0x05, 0xF4, 0x1F, 0x00, 0x00, 0x48, 0x63, 0x0C, 0xB8,
    0x48, 0x01, 0xC1, 0xFF, 0xE1, 0xB8, 0x01, 0x00, 0x00, 0x00, 0xC3, 0xB8, 0x02, 0x00, 0x00, 0x00,
    0xC3, 0xB8, 0x03, 0x00, 0x00, 0x00, 0xC3, 0x31, 0xC0, 0xC3,
];

const SWITCH_TABLE: [u8; 12] = [
    0x15, 0xE0, 0xFF, 0xFF, 0x1B, 0xE0, 0xFF, 0xFF, 0x21, 0xE0, 0xFF, 0xFF,
];

const CALLS: [u8; 35] = [
    0xE8, 0x0B, 0x00, 0x00, 0x00, 0xE8, 0x16, 0x00, 0x00, 0x00, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    0xE8, 0x0B, 0x00, 0x00, 0x00, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    0x31, 0xC0, 0xC3,
];

const DATA_IN_CODE: [u8; 22] = [
    0xC3, 0x06, 0x07, 0x0E, 0x17, 0x1E, 0x1F, 0x27, 0x2F, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    0x55, 0x48, 0x89, 0xE5, 0x5D, 0xC3,
];

fn at(rva: u32) -> u64 {
    BASE + u64::from(rva)
}

fn seed(rva: u32, origin: SeedOrigin) -> Seed {
    Seed { rva, origin }
}

fn text(bytes: &[u8]) -> Section<'_> {
    Section {
        rva: TEXT,
        bytes,
        executable: true,
    }
}

fn block_starts(discovery: &Discovery) -> Vec<u64> {
    discovery.blocks.iter().map(|block| block.start).collect()
}

fn words(code: &[u8]) -> Vec<u32> {
    code.chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect()
}

fn branch_target(at: usize, word: u32) -> Option<usize> {
    if word & 0xFC00_0000 != 0x1400_0000 {
        return None;
    }
    let offset = ((word << 6) as i32) >> 6;
    usize::try_from(at as i64 + i64::from(offset)).ok()
}

fn host_of(image: &TranslatedImage, rva: u32) -> u32 {
    image
        .back_map
        .iter()
        .find(|entry| entry.rva == rva)
        .map(|entry| entry.host)
        .unwrap()
}

#[test]
fn a_loop_splits_at_its_head_and_needs_no_flags_across_blocks() {
    let sections = [text(&LOOP)];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let discovery = discover(&input);
    assert_eq!(
        block_starts(&discovery),
        [at(0x1000), at(0x1002), at(0x1009)]
    );
    assert_eq!(discovery.blocks[0].instructions.len(), 1);
    assert_eq!(discovery.blocks[0].end, BlockEnd::FallThrough(at(0x1002)));
    assert_eq!(discovery.blocks[1].end, BlockEnd::Terminator);
    assert_eq!(discovery.entries, [at(0x1000)]);
    assert_eq!(live_outs(&discovery), [Flags::NONE; 3]);
}

#[test]
fn a_switch_jump_table_is_bounded_by_its_compare() {
    let sections = [
        text(&SWITCH),
        Section {
            rva: RODATA,
            bytes: &SWITCH_TABLE,
            executable: false,
        },
    ];
    let seeds = [seed(TEXT, SeedOrigin::Export)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let discovery = discover(&input);
    assert_eq!(
        discovery.jump_tables,
        [JumpTable {
            jump: at(0x1013),
            table: at(RODATA),
            entry_bytes: 4,
            targets: vec![at(0x1015), at(0x101B), at(0x1021)],
        }]
    );
    assert_eq!(discovery.unresolved_jumps, 0);
    for target in [0x1015, 0x101B, 0x1021] {
        assert!(discovery.entries.contains(&at(target)));
        assert!(discovery.block_index(at(target)).is_some());
    }
    assert!(discovery.block_index(at(0x1027)).is_some());
    assert!(discovery.jump_table(at(0x1013)).is_some());
}

#[test]
fn a_switch_without_a_bound_stays_indirect() {
    let mut unbounded = SWITCH;
    unbounded[3] = 0x90;
    unbounded[4] = 0x90;
    let sections = [
        text(&unbounded),
        Section {
            rva: RODATA,
            bytes: &SWITCH_TABLE,
            executable: false,
        },
    ];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let discovery = discover(&input);
    assert!(discovery.jump_tables.is_empty());
    assert_eq!(discovery.unresolved_jumps, 1);
    let jump = discovery
        .blocks
        .iter()
        .position(|block| block.instructions.last().map(|i| i.address) == Some(at(0x1013)))
        .unwrap();
    assert_eq!(live_outs(&discovery)[jump], Flags::ALL);
}

#[test]
fn calls_make_their_targets_and_return_sites_entries() {
    let sections = [text(&CALLS)];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let discovery = discover(&input);
    let expected = [
        at(0x1000),
        at(0x1005),
        at(0x100A),
        at(0x1010),
        at(0x1015),
        at(0x1020),
    ];
    assert_eq!(block_starts(&discovery), expected);
    assert_eq!(discovery.entries, expected);
    assert_eq!(discovery.swept_entries, 0);
}

#[test]
fn the_sweep_finds_an_unreferenced_function_past_data_in_code() {
    let sections = [text(&DATA_IN_CODE)];
    let seeds = [
        seed(TEXT, SeedOrigin::EntryPoint),
        seed(0x1001, SeedOrigin::Relocation),
    ];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let discovery = discover(&input);
    assert_eq!(block_starts(&discovery), [at(0x1000), at(0x1010)]);
    assert_eq!(discovery.swept_entries, 1);
    assert_eq!(discovery.entries, [at(0x1000), at(0x1010)]);
}

#[test]
fn exits_are_linked_into_branches_to_their_blocks() {
    let sections = [text(&LOOP)];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let image = translate_image(&input, MODE).unwrap();
    let code = words(&image.code);
    assert!(!code.contains(&encode::brk(trap::UNLINKED)));
    let head = host_of(&image, 0x1000) as usize / 4;
    let body = host_of(&image, 0x1002) as usize / 4;
    let tail = host_of(&image, 0x1009) as usize / 4;
    let targets = |range: core::ops::Range<usize>| -> Vec<usize> {
        range
            .filter_map(|index| branch_target(index, code[index]))
            .collect()
    };
    assert!(targets(head..body).contains(&body));
    let from_body = targets(body..tail);
    assert!(from_body.contains(&body));
    assert!(from_body.contains(&tail));
    assert_eq!(image.dispatch, 0);
    assert!(image.enter > 0);
    assert_eq!(image.entries.lookup(0x1000), Some(host_of(&image, 0x1000)));
    assert_eq!(image.entries.lookup(0x1002), None);
    assert!(image.entries.lookup(RETURN_TO_NATIVE_RVA).is_some());
    assert_eq!(image.stats.blocks, 3);
    assert_eq!(image.stats.instructions, 5);
    assert_eq!(image.stats.guest_bytes, 10);
    assert_eq!(image.stats.veneers, 0);
    assert!(image.refused_sites.is_empty());
    assert_eq!(image.code.len() % 8, 0);
}

#[test]
fn far_branches_go_through_veneers_to_their_targets() {
    let sections = [text(&CALLS)];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let limits = Limits {
        segment_words: 8,
        direct_reach_words: 4,
    };
    let image = translate_image_with(&input, MODE, limits).unwrap();
    assert!(image.stats.veneers > 0);
    let code = words(&image.code);
    let starts: Vec<usize> = image
        .back_map
        .iter()
        .map(|entry| entry.host as usize / 4)
        .chain([image.dispatch as usize / 4])
        .collect();
    let mut found = 0;
    for (index, &word) in code.iter().enumerate() {
        if word & 0x9F00_001F == 0x9000_0011 {
            let target = veneer_target(index as u32, &code[index..]).unwrap() as usize;
            assert!(starts.contains(&target));
            assert_eq!(code[index + 2], encode::br(Reg(17)));
            found += 1;
        }
    }
    assert_eq!(found, image.stats.veneers);
    assert!(!code.contains(&encode::brk(trap::UNLINKED)));
}

#[test]
fn refused_bytes_become_a_listed_site() {
    let bytes = [0xC5, 0xF8, 0x77, 0xC3];
    let sections = [text(&bytes)];
    let seeds = [seed(TEXT, SeedOrigin::EntryPoint)];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let image = translate_image(&input, MODE).unwrap();
    assert_eq!(
        image.refused_sites,
        [RefusedSite {
            rva: TEXT,
            length: 3,
            reason: SiteReason::Refused(crate::Refusal::Vex),
        }]
    );
    let code = words(&image.code);
    assert!(code.contains(&encode::brk(trap::REFUSED)));
    assert!(!image.guest_address_literals.is_empty());
}

#[test]
fn the_entries_table_finds_every_key_and_rejects_bad_slots() {
    let bits = EntriesTable::bits_for(1000);
    let mut table = EntriesTable::with_bits(bits);
    for key in 0..1000u32 {
        assert!(table.insert(0x1000 + key * 16, key * 4));
    }
    assert_eq!(table.len(), 1000);
    for key in 0..1000u32 {
        assert_eq!(table.lookup(0x1000 + key * 16), Some(key * 4));
    }
    assert_eq!(table.lookup(0x1001), None);
    let copy = EntriesTable::from_slots(bits, table.slots().to_vec()).unwrap();
    assert_eq!(copy, table);
    let mut duplicated = table.slots().to_vec();
    let full = duplicated
        .iter()
        .position(|&slot| slot as u32 != EMPTY_KEY)
        .unwrap();
    let empty = duplicated
        .iter()
        .position(|&slot| slot as u32 == EMPTY_KEY)
        .unwrap();
    duplicated[empty] = duplicated[full];
    assert_eq!(EntriesTable::from_slots(bits, duplicated), None);
    let bytes = table.runtime_bytes(0x1111, 0x2222);
    assert_eq!(bytes.len(), 16 + 8 * table.slots().len());
    assert_eq!(bytes[..8], 0x1111u64.to_le_bytes());
    assert_eq!(bytes[8..16], 0x2222u64.to_le_bytes());
}

#[test]
fn a_full_table_refuses_more_entries() {
    let mut table = EntriesTable::with_bits(MIN_BITS);
    let capacity = (1usize << MIN_BITS) - 1;
    for key in 0..capacity as u32 {
        assert!(table.insert(key + 1, key));
    }
    assert!(!table.insert(0x9999, 0));
    assert!(table.insert(1, 7));
    assert_eq!(table.lookup(1), Some(7));
}

#[test]
fn the_dispatch_hashes_like_the_table() {
    let routines = runtime::routines(6).unwrap();
    assert_eq!(routines.dispatch, 0);
    assert_eq!(routines.words.len() % 2, 0);
    let shift = encode::bitfield(Bitfield::Ubfm, Size::W, Reg(22), Reg(22), 26, 31);
    assert!(routines.words[..routines.enter as usize].contains(&shift));
    assert!(routines.words[..routines.enter as usize].contains(&encode::brk(trap::UNDISCOVERED)));
    assert!(routines.return_to_native > routines.enter);
    assert!(routines.words.contains(&encode::ret(Reg(30))));
    let table = EntriesTable::with_bits(6);
    let rva = 0x1234u32;
    assert_eq!(
        table.home(rva),
        (rva.wrapping_mul(HASH_MULTIPLIER) >> 26) as usize
    );
}

#[test]
fn exception_table_and_function_start_seeds_are_read() {
    let mut pdata = Vec::new();
    for (begin, end) in [(0x1000u32, 0x1010u32), (0x1020, 0x1020), (0x1030, 0x1080)] {
        pdata.extend_from_slice(&begin.to_le_bytes());
        pdata.extend_from_slice(&end.to_le_bytes());
        pdata.extend_from_slice(&0x5000u32.to_le_bytes());
    }
    pdata.extend_from_slice(&[1, 2, 3]);
    assert_eq!(pdata_function_starts(&pdata), [0x1000, 0x1030]);
    assert_eq!(
        function_starts(&[0x80, 0x20, 0x10, 0x00, 0x44], 0),
        Some(vec![0x1000, 0x1010])
    );
    assert_eq!(function_starts(&[0x80], 0), None);
    assert_eq!(function_starts(&[], 0x2000), Some(vec![]));
}
