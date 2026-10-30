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
    let error = translate_block(&straight(&[0x0F, 0xA2]), ORDERED, Flags::NONE).unwrap_err();
    assert_eq!(error.kind, LowerErrorKind::Unhandled(Mnemonic::Cpuid));
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
    assert_eq!(code.code, [encode::brk(trap::REFUSED)]);
    assert_eq!(code.starts.last().map(|start| start.guest), Some(BASE + 1));
}
