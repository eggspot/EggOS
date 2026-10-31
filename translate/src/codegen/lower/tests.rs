use alloc::vec;
use alloc::vec::Vec;

use crate::codegen::aarch64::encode;
use crate::codegen::{
    Block, BlockEnd, CodegenMode, Exit, ExitKind, Flags, LowerErrorKind, form_block,
    translate_block, trap,
};
use crate::decode;
use crate::instruction::{Instruction, Mnemonic};

const BASE: u64 = 0x1000;
const UNLINKED: u32 = 0xD43C_01E0;

fn decode_all(bytes: &[u8]) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let instruction = decode(&bytes[offset..], BASE + offset as u64).unwrap();
        offset += usize::from(instruction.length);
        instructions.push(instruction);
    }
    instructions
}

fn straight(bytes: &[u8]) -> Block {
    Block {
        start: BASE,
        instructions: decode_all(bytes),
        end: BlockEnd::FallThrough(BASE + bytes.len() as u64),
    }
}

fn terminated(bytes: &[u8]) -> Block {
    Block {
        start: BASE,
        instructions: decode_all(bytes),
        end: BlockEnd::Terminator,
    }
}

const ORDERED: CodegenMode = CodegenMode {
    hardware_tso: false,
    lse: true,
    lrcpc: true,
    flagm: true,
};

const TSO: CodegenMode = CodegenMode {
    hardware_tso: true,
    lse: true,
    lrcpc: true,
    flagm: true,
};

#[test]
fn the_unlinked_placeholder_is_a_brk() {
    assert_eq!(encode::brk(trap::UNLINKED), UNLINKED);
}

#[test]
fn a_32_bit_add_is_one_instruction_writing_the_whole_register() {
    let block = translate_block(&straight(&[0x01, 0xD8]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(block.code, [0x0B03_0000, UNLINKED]);
    assert_eq!(
        block.exits,
        [Exit {
            word: 1,
            kind: ExitKind::Jump(BASE + 2)
        }]
    );
    assert_eq!(block.starts.len(), 1);
    assert_eq!(block.guest_end, BASE + 2);
}

#[test]
fn a_compare_feeding_je_sets_only_nzcv() {
    let block = terminated(&[0x39, 0xD8, 0x74, 0x05]);
    let code = translate_block(&block, ORDERED, Flags::NONE).unwrap();
    assert_eq!(code.code, [0x6B03_001F, 0x5400_0040, UNLINKED, UNLINKED]);
    assert_eq!(
        code.exits,
        [
            Exit {
                word: 2,
                kind: ExitKind::Jump(BASE + 4)
            },
            Exit {
                word: 3,
                kind: ExitKind::Jump(BASE + 9)
            },
        ]
    );
}

#[test]
fn above_after_compare_uses_the_inverted_carry_and_fixes_it_at_each_exit() {
    let block = terminated(&[0x39, 0xD8, 0x77, 0x05]);
    let live_out = Flags::CF.union(Flags::ZF);
    let code = translate_block(&block, ORDERED, live_out).unwrap();
    assert_eq!(
        code.code,
        [
            0x6B03_001F,
            0x5400_0068,
            encode::cfinv(),
            UNLINKED,
            encode::cfinv(),
            UNLINKED
        ]
    );
}

#[test]
fn a_byte_add_shifts_into_the_top_byte_for_its_flags_and_merges_the_result() {
    let block = terminated(&[0x00, 0xD8, 0x74, 0x00]);
    let code = translate_block(&block, ORDERED, Flags::NONE).unwrap();
    assert_eq!(
        &code.code[..7],
        [
            0x5300_1C16,
            0x5300_1C77,
            0x5308_1ED8,
            0x2B17_6318,
            0x5318_7F18,
            0xB340_1F00,
            0x5400_0040,
        ]
    );
}

#[test]
fn a_load_without_hardware_tso_is_an_acquire() {
    let block = translate_block(&straight(&[0x8B, 0x43, 0x08]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(block.code, [0x9100_2070, 0xB8BF_C200, UNLINKED]);
}

#[test]
fn push_and_pop_with_hardware_tso_are_plain_accesses() {
    let block = translate_block(&straight(&[0x53, 0x58]), TSO, Flags::NONE).unwrap();
    assert_eq!(
        block.code,
        [
            0xD100_2084,
            0xF900_0083,
            0xF940_0096,
            0x9100_2084,
            0xAA16_03E0,
            UNLINKED
        ]
    );
}

#[test]
fn a_call_pushes_a_relocated_return_address_and_exits_twice() {
    let block = terminated(&[0xE8, 0x10, 0x00, 0x00, 0x00]);
    let code = translate_block(&block, ORDERED, Flags::NONE).unwrap();
    let kinds: Vec<ExitKind> = code.exits.iter().map(|exit| exit.kind).collect();
    assert_eq!(
        kinds,
        [ExitKind::Jump(BASE + 0x15), ExitKind::Jump(BASE + 5)]
    );
    assert_eq!(code.guest_address_literals.len(), 1);
    let literal = code.guest_address_literals[0] as usize;
    assert_eq!(literal % 2, 0);
    let value = u64::from(code.code[literal]) | (u64::from(code.code[literal + 1]) << 32);
    assert_eq!(value, BASE + 5);
}

#[test]
fn a_return_compares_the_shadow_stack_and_falls_back_to_the_dispatch() {
    let code = translate_block(&terminated(&[0xC3]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(code.exits.len(), 1);
    assert_eq!(code.exits[0].kind, ExitKind::Indirect);
    assert!(
        code.code
            .contains(&encode::br(crate::codegen::registers::TEMPORARIES[0]))
    );
}

#[test]
fn division_checks_for_a_zero_divisor() {
    let code = translate_block(&straight(&[0xF7, 0xF3]), ORDERED, Flags::NONE).unwrap();
    assert!(code.code.contains(&encode::brk(trap::DIVIDE_ERROR)));
}

#[test]
fn every_integer_form_lowers() {
    let sequences: [&[u8]; 24] = [
        &[0x48, 0x0F, 0xAF, 0xC3],
        &[0x6B, 0xC3, 0x07],
        &[0x48, 0xF7, 0xE3],
        &[0x48, 0xF7, 0xFB],
        &[0x48, 0x99],
        &[0x98],
        &[0xD3, 0xE0],
        &[0x48, 0xC1, 0xF8, 0x03],
        &[0xD0, 0xC0],
        &[0x0F, 0xA4, 0xD8, 0x04],
        &[0x0F, 0xA3, 0xD8],
        &[0x0F, 0xBA, 0x28, 0x05],
        &[0x0F, 0xBC, 0xC3],
        &[0xF3, 0x0F, 0xBD, 0xC3],
        &[0xF3, 0x0F, 0xB8, 0xC3],
        &[0x0F, 0xC8],
        &[0x0F, 0x4A, 0xC3],
        &[0x0F, 0x9A, 0xC0],
        &[0x87, 0x18],
        &[0xF0, 0x0F, 0xC1, 0x18],
        &[0xF0, 0x0F, 0xB1, 0x18],
        &[0xF0, 0xFF, 0x00],
        &[0x48, 0x8D, 0x44, 0x98, 0x10],
        &[0x65, 0x48, 0x8B, 0x04, 0x25, 0x30, 0x00, 0x00, 0x00],
    ];
    for mode in [ORDERED, TSO, CodegenMode::default()] {
        for bytes in sequences {
            let block = straight(bytes);
            assert!(
                translate_block(&block, mode, Flags::ALL).is_ok(),
                "{bytes:02x?} in {mode:?}"
            );
        }
    }
}

#[test]
fn an_instruction_left_to_a_later_slice_is_reported() {
    let error = translate_block(&straight(&[0x0F, 0x05]), ORDERED, Flags::NONE).unwrap_err();
    assert_eq!(error.kind, LowerErrorKind::Unhandled(Mnemonic::Syscall));
    assert_eq!(error.address, BASE);
}

#[test]
fn a_block_stops_at_its_branch() {
    let block = form_block(&[0x01, 0xD8, 0xC3, 0x90], BASE, BASE);
    assert_eq!(block.instructions.len(), 2);
    assert_eq!(block.end, BlockEnd::Terminator);
}

#[test]
fn a_long_straight_run_falls_through() {
    let nops = vec![0x90u8; 300];
    let block = form_block(&nops, BASE, BASE);
    assert_eq!(
        block.instructions.len(),
        crate::codegen::MAX_BLOCK_INSTRUCTIONS
    );
    assert_eq!(block.end, BlockEnd::FallThrough(BASE + 256));
}

#[test]
fn a_refused_instruction_becomes_a_trap_at_its_address() {
    let block = form_block(&[0x90, 0xC5, 0xF8, 0x77], BASE, BASE);
    assert!(matches!(block.end, BlockEnd::Refused { address, .. } if address == BASE + 1));
    let code = translate_block(&block, ORDERED, Flags::NONE).unwrap();
    assert_eq!(
        code.code,
        [
            encode::load_literal(crate::codegen::aarch64::Reg(16), 2),
            encode::brk(trap::REFUSED),
            (BASE + 1) as u32,
            0
        ]
    );
    assert_eq!(code.guest_address_literals, [2]);
    assert_eq!(code.starts.last().map(|start| start.guest), Some(BASE + 1));
}

#[test]
fn a_packed_single_add_is_one_vector_instruction() {
    use crate::codegen::aarch64::simd::{self, Float, FloatThreeSame};
    let block = translate_block(&straight(&[0x0F, 0x58, 0xC1]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(
        block.code,
        [
            simd::float_three_same(FloatThreeSame::Fadd, Float::Single, 0, 0, 1),
            UNLINKED
        ]
    );
}

#[test]
fn a_scalar_add_keeps_the_upper_lanes() {
    use crate::codegen::aarch64::simd::{self, Element, Float, Scalar};
    let block =
        translate_block(&straight(&[0xF3, 0x0F, 0x58, 0xC1]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(
        block.code,
        [
            simd::scalar(Scalar::Fadd, Float::Single, 17, 0, 1),
            simd::insert_element(Element::S, 0, 0, 17, 0),
            UNLINKED
        ]
    );
}

#[test]
fn an_unordered_compare_sets_zero_and_carry() {
    use crate::codegen::aarch64::encode::Logic;
    use crate::codegen::aarch64::simd::{self, Float};
    use crate::codegen::aarch64::{Cond, Reg, Size};
    let block = translate_block(&straight(&[0x66, 0x0F, 0x2E, 0xC1]), ORDERED, Flags::ZF).unwrap();
    let scratch = Reg(17);
    assert_eq!(
        block.code,
        [
            simd::fcmp(Float::Double, 0, 1),
            encode::b_cond(Cond::Vc, 4),
            encode::logical_immediate(Logic::Orr, Size::W, scratch, Reg::ZR, 0x4000_0000).unwrap(),
            encode::msr_nzcv(scratch),
            encode::b(1),
            UNLINKED
        ]
    );
}

#[test]
fn cpuid_reports_sse2_without_avx() {
    let leaf = super::system::CPUID_LEAVES
        .iter()
        .find(|(leaf, _)| *leaf == 1)
        .map(|(_, values)| *values)
        .unwrap();
    assert_ne!(leaf[3] & (1 << 26), 0);
    assert_ne!(leaf[2] & (1 << 23), 0);
    assert_eq!(leaf[2] & (1 << 28), 0);
    assert_eq!(leaf[2] & (1 << 27), 0);
    let brand = super::system::CPUID_LEAVES[5].1[0];
    assert_eq!(brand.to_le_bytes(), *b"EggO");
    let block = translate_block(&straight(&[0x0F, 0xA2]), ORDERED, Flags::NONE).unwrap();
    assert!(block.code.len() > 4 * super::system::CPUID_LEAVES.len());
}

#[test]
fn every_sse_and_system_form_lowers() {
    let sequences: [&[u8]; 76] = [
        &[0x0F, 0x28, 0xC1],
        &[0x0F, 0x10, 0x00],
        &[0x0F, 0x29, 0x00],
        &[0xF3, 0x0F, 0x10, 0x00],
        &[0xF2, 0x0F, 0x11, 0x00],
        &[0x66, 0x0F, 0x6E, 0xC0],
        &[0x66, 0x48, 0x0F, 0x7E, 0xC0],
        &[0xF3, 0x0F, 0x7E, 0xC1],
        &[0x66, 0x0F, 0xD6, 0x00],
        &[0x0F, 0x16, 0x00],
        &[0x0F, 0x17, 0x00],
        &[0x0F, 0x12, 0xC1],
        &[0x0F, 0x50, 0xC1],
        &[0x66, 0x0F, 0xD7, 0xC1],
        &[0x0F, 0x5D, 0xC1],
        &[0xF2, 0x0F, 0x5F, 0xC1],
        &[0xF3, 0x0F, 0x51, 0xC1],
        &[0x0F, 0x53, 0xC1],
        &[0x0F, 0xC2, 0xC1, 0x01],
        &[0xF2, 0x0F, 0xC2, 0xC1, 0x07],
        &[0x66, 0x0F, 0x2F, 0xC1],
        &[0x0F, 0x2E, 0x00],
        &[0x66, 0x0F, 0x63, 0xC1],
        &[0x66, 0x0F, 0x67, 0xC1],
        &[0x66, 0x0F, 0xE5, 0xC1],
        &[0x66, 0x0F, 0xF4, 0xC1],
        &[0x66, 0x0F, 0xF5, 0xC1],
        &[0x66, 0x0F, 0xF6, 0xC1],
        &[0x66, 0x0F, 0x73, 0xD8, 0x04],
        &[0x66, 0x0F, 0x73, 0xF8, 0x14],
        &[0x66, 0x0F, 0x71, 0xF0, 0x03],
        &[0x66, 0x0F, 0x72, 0xE0, 0x40],
        &[0x66, 0x0F, 0xE2, 0xC1],
        &[0x66, 0x0F, 0xD3, 0x00],
        &[0x66, 0x0F, 0x70, 0xC1, 0x1B],
        &[0x66, 0x0F, 0x70, 0xC0, 0x00],
        &[0xF2, 0x0F, 0x70, 0xC1, 0x1B],
        &[0xF3, 0x0F, 0x70, 0xC0, 0x1B],
        &[0x0F, 0xC6, 0xC1, 0x44],
        &[0x66, 0x0F, 0xC6, 0xC0, 0x01],
        &[0x66, 0x0F, 0xC5, 0xC1, 0x02],
        &[0x66, 0x0F, 0xC4, 0xC0, 0x03],
        &[0x66, 0x0F, 0x38, 0x17, 0xC1],
        &[0x66, 0x0F, 0x38, 0x17, 0x00],
        &[0x66, 0x0F, 0x38, 0x32, 0xC1],
        &[0x66, 0x41, 0x0F, 0x38, 0x32, 0x04, 0x24],
        &[0x66, 0x0F, 0x3A, 0x14, 0xC8, 0x03],
        &[0x66, 0x0F, 0x3A, 0x14, 0x08, 0x0F],
        &[0x66, 0x0F, 0x3A, 0x16, 0xC8, 0x02],
        &[0x66, 0x48, 0x0F, 0x3A, 0x16, 0x08, 0x01],
        &[0x66, 0x0F, 0x3A, 0x20, 0xC1, 0x05],
        &[0x66, 0x0F, 0x3A, 0x20, 0x01, 0x0F],
        &[0xF2, 0x48, 0x0F, 0x2A, 0xC0],
        &[0xF3, 0x0F, 0x2D, 0xC0],
        &[0xF2, 0x48, 0x0F, 0x2C, 0xC0],
        &[0x0F, 0x5B, 0xC1],
        &[0x66, 0x0F, 0xE6, 0xC1],
        &[0xF3, 0x0F, 0x5B, 0xC1],
        &[0xF3, 0x0F, 0xE6, 0xC1],
        &[0x0F, 0x5A, 0xC1],
        &[0x66, 0x0F, 0x60, 0xC1],
        &[0x66, 0x0F, 0x74, 0xC1],
        &[0x66, 0x0F, 0xFE, 0x00],
        &[0x0F, 0xAE, 0x10],
        &[0x0F, 0xAE, 0x18],
        &[0x9F],
        &[0x9E],
        &[0x9C],
        &[0x9D],
        &[0xFC],
        &[0xFD],
        &[0x0F, 0x31],
        &[0x0F, 0x01, 0xD0],
        &[0x0F, 0xC7, 0x08],
        &[0x48, 0x0F, 0xC7, 0x08],
        &[0x0F, 0xAE, 0x38],
    ];
    for mode in [ORDERED, TSO, CodegenMode::default()] {
        for bytes in sequences {
            let block = straight(bytes);
            assert!(
                translate_block(&block, mode, Flags::ALL).is_ok(),
                "{bytes:02x?} in {mode:?}"
            );
        }
    }
}

#[test]
fn rep_movsb_is_two_post_index_loops_chosen_by_the_direction_byte() {
    let block = translate_block(&straight(&[0xF3, 0xA4]), TSO, Flags::NONE).unwrap();
    assert_eq!(
        block.code,
        [
            0xB400_0181,
            0x3940_4ABA,
            0x3700_00DA,
            0x3840_14D6,
            0x3800_14F6,
            0xD100_0421,
            0xB5FF_FFA1,
            0x1400_0005,
            0x385F_F4D6,
            0x381F_F4F6,
            0xD100_0421,
            0xB5FF_FFA1,
            UNLINKED,
        ]
    );
}

#[test]
fn fchs_negates_st0_in_the_state_block() {
    let block = translate_block(&straight(&[0xD9, 0xE0]), ORDERED, Flags::NONE).unwrap();
    assert_eq!(
        block.code,
        [
            0x3940_72B9,
            0x8B19_0EBA,
            0xFD40_1351,
            0x1E61_4231,
            0xFD00_1351,
            UNLINKED,
        ]
    );
}

#[test]
fn every_x87_form_lowers() {
    let sequences: [&[u8]; 42] = [
        &[0xDD, 0x00],
        &[0xD9, 0x00],
        &[0xDB, 0x2C, 0x24],
        &[0xD9, 0xC1],
        &[0xDD, 0x18],
        &[0xD9, 0x10],
        &[0xDB, 0x3C, 0x24],
        &[0xDD, 0xD9],
        &[0xDF, 0x00],
        &[0xDB, 0x00],
        &[0xDF, 0x2C, 0x24],
        &[0xDB, 0x10],
        &[0xDF, 0x18],
        &[0xDF, 0x3C, 0x24],
        &[0xD8, 0xC1],
        &[0xDC, 0x08],
        &[0xDE, 0xC1],
        &[0xDC, 0xE9],
        &[0xDE, 0xF9],
        &[0xD8, 0x70, 0x08],
        &[0xD8, 0x38],
        &[0xD9, 0xE0],
        &[0xD9, 0xE1],
        &[0xD9, 0xFA],
        &[0xD9, 0xFC],
        &[0xD9, 0xE8],
        &[0xD9, 0xEE],
        &[0xD9, 0xEB],
        &[0xD9, 0xC9],
        &[0xDB, 0xF1],
        &[0xDF, 0xE9],
        &[0xD8, 0xD1],
        &[0xDD, 0xE9],
        &[0xDC, 0x18],
        &[0xDE, 0xD9],
        &[0xDA, 0xE9],
        &[0xD9, 0x28],
        &[0xD9, 0x38],
        &[0xDD, 0x38],
        &[0xDF, 0xE0],
        &[0xDB, 0xE3],
        &[0x9B, 0xDB, 0xE2],
    ];
    for mode in [ORDERED, TSO, CodegenMode::default()] {
        for bytes in sequences {
            let block = straight(bytes);
            assert!(
                translate_block(&block, mode, Flags::ALL).is_ok(),
                "{bytes:02x?} in {mode:?}"
            );
        }
    }
}

#[test]
fn every_string_and_legacy_integer_form_lowers() {
    let sequences: [&[u8]; 17] = [
        &[0xA4],
        &[0xF3, 0xA4],
        &[0xF3, 0x48, 0xA5],
        &[0xF3, 0xAA],
        &[0x66, 0xAB],
        &[0xAC],
        &[0xF3, 0xA6],
        &[0xF2, 0xAE],
        &[0x48, 0xA7],
        &[0xF2, 0x48, 0xAF],
        &[0xD7],
        &[0xC8, 0x20, 0x00, 0x00],
        &[0xC8, 0x10, 0x00, 0x02],
        &[0xD0, 0xD0],
        &[0x48, 0xD3, 0xD8],
        &[0xC1, 0xD0, 0x05],
        &[0xD2, 0x18],
    ];
    for mode in [ORDERED, TSO, CodegenMode::default()] {
        for bytes in sequences {
            let block = straight(bytes);
            assert!(
                translate_block(&block, mode, Flags::ALL).is_ok(),
                "{bytes:02x?} in {mode:?}"
            );
        }
    }
}
