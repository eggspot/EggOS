use super::{DecodeError, DecodeErrorKind, MAX_INSTRUCTION_LENGTH, Refusal, decode};
use crate::instruction::{
    Address, Condition, Gpr, Index, Instruction, Memory, Mnemonic, Operand, Register, Repeat,
    Segment, Width,
};

pub(super) const AT: u64 = 0x1000;

pub(super) fn decoded(bytes: &[u8]) -> Instruction {
    match decode(bytes, AT) {
        Ok(instruction) => {
            assert_eq!(
                usize::from(instruction.length),
                bytes.len(),
                "length of {bytes:02x?}"
            );
            instruction
        }
        Err(error) => panic!("{bytes:02x?}: {error}"),
    }
}

pub(super) fn check(bytes: &[u8], mnemonic: Mnemonic, width: Width, operands: &[Operand]) {
    let instruction = decoded(bytes);
    assert_eq!(instruction.mnemonic, mnemonic, "{bytes:02x?}");
    assert_eq!(instruction.width, width, "{bytes:02x?}");
    assert_eq!(instruction.operands(), operands, "{bytes:02x?}");
}

pub(super) fn error(bytes: &[u8]) -> DecodeErrorKind {
    match decode(bytes, AT) {
        Ok(instruction) => panic!("{bytes:02x?} decoded as {instruction:?}"),
        Err(error) => {
            assert_eq!(error.address, AT);
            error.kind
        }
    }
}

pub(super) fn refused(bytes: &[u8]) -> Refusal {
    match error(bytes) {
        DecodeErrorKind::Unsupported { refusal, length } => {
            assert_eq!(usize::from(length), bytes.len(), "length of {bytes:02x?}");
            refusal
        }
        other => panic!("{bytes:02x?}: {other:?}"),
    }
}

pub(super) fn general(gpr: Gpr, width: Width) -> Operand {
    Operand::Register(Register::General { gpr, width })
}

pub(super) fn xmm(number: u8) -> Operand {
    Operand::Register(Register::Xmm(number))
}

pub(super) fn based(
    base: Option<Gpr>,
    index: Option<(Gpr, u8)>,
    displacement: i32,
    width: Width,
) -> Operand {
    Operand::Memory(Memory {
        segment: None,
        address: Address::Based {
            base,
            index: index.map(|(register, scale)| Index { register, scale }),
            displacement,
        },
        width,
    })
}

pub(super) fn at(base: Gpr, displacement: i32, width: Width) -> Operand {
    based(Some(base), None, displacement, width)
}

fn rip(target: u64, width: Width) -> Operand {
    Operand::Memory(Memory {
        segment: None,
        address: Address::Rip { target },
        width,
    })
}

fn st(number: u8) -> Operand {
    Operand::Register(Register::St(number))
}

#[test]
fn register_to_register_move_names_both_registers() {
    let rax = general(Gpr::RAX, Width::Qword);
    let rbx = general(Gpr::RBX, Width::Qword);
    check(
        &[0x48, 0x89, 0xD8],
        Mnemonic::Mov,
        Width::Qword,
        &[rax, rbx],
    );
    check(
        &[0x48, 0x8B, 0xC3],
        Mnemonic::Mov,
        Width::Qword,
        &[rax, rbx],
    );
    let eax = general(Gpr::RAX, Width::Dword);
    let ebx = general(Gpr::RBX, Width::Dword);
    check(&[0x89, 0xD8], Mnemonic::Mov, Width::Dword, &[eax, ebx]);
}

#[test]
fn rip_relative_operands_resolve_against_the_next_instruction() {
    let bytes = [0x8B, 0x05, 0x10, 0x00, 0x00, 0x00];
    let eax = general(Gpr::RAX, Width::Dword);
    check(
        &bytes,
        Mnemonic::Mov,
        Width::Dword,
        &[eax, rip(AT + 6 + 0x10, Width::Dword)],
    );
    let jump = [0xFF, 0x25, 0xF0, 0xFF, 0xFF, 0xFF];
    check(
        &jump,
        Mnemonic::Jmp,
        Width::Qword,
        &[rip(AT + 6 - 0x10, Width::Qword)],
    );
    let store = [0xC7, 0x05, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
    check(
        &store,
        Mnemonic::Mov,
        Width::Dword,
        &[rip(AT + 10 + 8, Width::Dword), Operand::Immediate(1)],
    );
}

#[test]
fn rip_relative_holds_with_rex_b_and_r13_needs_a_displacement() {
    let rip_form = [0x41, 0x8B, 0x05, 0x00, 0x00, 0x00, 0x00];
    let eax = general(Gpr::RAX, Width::Dword);
    check(
        &rip_form,
        Mnemonic::Mov,
        Width::Dword,
        &[eax, rip(AT + 7, Width::Dword)],
    );
    let r12 = general(Gpr::R12, Width::Qword);
    check(
        &[0x4D, 0x8B, 0x65, 0x00],
        Mnemonic::Mov,
        Width::Qword,
        &[r12, at(Gpr::R13, 0, Width::Qword)],
    );
}

#[test]
fn scaled_index_addresses_decode_base_index_scale_and_displacement() {
    let rcx = general(Gpr::RCX, Width::Qword);
    check(
        &[0x48, 0x8D, 0x4C, 0x98, 0x7F],
        Mnemonic::Lea,
        Width::Qword,
        &[
            rcx,
            based(Some(Gpr::RAX), Some((Gpr::RBX, 4)), 0x7F, Width::Qword),
        ],
    );
    let eax = general(Gpr::RAX, Width::Dword);
    check(
        &[0x8B, 0x44, 0x45, 0x00],
        Mnemonic::Mov,
        Width::Dword,
        &[
            eax,
            based(Some(Gpr::RBP), Some((Gpr::RAX, 2)), 0, Width::Dword),
        ],
    );
    let rax = general(Gpr::RAX, Width::Qword);
    check(
        &[0x49, 0x8B, 0x04, 0x24],
        Mnemonic::Mov,
        Width::Qword,
        &[rax, at(Gpr::R12, 0, Width::Qword)],
    );
    check(
        &[0x4A, 0x8B, 0x04, 0x20],
        Mnemonic::Mov,
        Width::Qword,
        &[
            rax,
            based(Some(Gpr::RAX), Some((Gpr::R12, 1)), 0, Width::Qword),
        ],
    );
    check(
        &[0x48, 0x8B, 0x84, 0x88, 0x00, 0x01, 0x00, 0x00],
        Mnemonic::Mov,
        Width::Qword,
        &[
            rax,
            based(Some(Gpr::RAX), Some((Gpr::RCX, 4)), 0x100, Width::Qword),
        ],
    );
}

#[test]
fn a_scaled_index_without_base_takes_a_32_bit_displacement() {
    let rax = general(Gpr::RAX, Width::Qword);
    check(
        &[0x48, 0x8B, 0x04, 0xC5, 0x10, 0x00, 0x00, 0x00],
        Mnemonic::Mov,
        Width::Qword,
        &[rax, based(None, Some((Gpr::RAX, 8)), 0x10, Width::Qword)],
    );
}

#[test]
fn fs_and_gs_overrides_stay_on_absolute_addresses() {
    let bytes = [0x65, 0x48, 0x8B, 0x04, 0x25, 0x30, 0x00, 0x00, 0x00];
    let instruction = decoded(&bytes);
    let teb = Operand::Memory(Memory {
        segment: Some(Segment::Gs),
        address: Address::Based {
            base: None,
            index: None,
            displacement: 0x30,
        },
        width: Width::Qword,
    });
    assert_eq!(
        instruction.operands(),
        &[general(Gpr::RAX, Width::Qword), teb]
    );
    let fs = decoded(&[0x64, 0x48, 0x8B, 0x04, 0x25, 0x30, 0x00, 0x00, 0x00]);
    let Operand::Memory(memory) = fs.operands()[1] else {
        panic!("{fs:?}");
    };
    assert_eq!(memory.segment, Some(Segment::Fs));
    let ignored = decoded(&[0x2E, 0x8B, 0x00]);
    let Operand::Memory(memory) = ignored.operands()[1] else {
        panic!("{ignored:?}");
    };
    assert_eq!(memory.segment, None);
}

#[test]
fn immediates_sign_extend_except_unsigned_fields() {
    let qword_at_rsp = at(Gpr::RSP, 8, Width::Qword);
    check(
        &[0x48, 0x83, 0x44, 0x24, 0x08, 0x01],
        Mnemonic::Add,
        Width::Qword,
        &[qword_at_rsp, Operand::Immediate(1)],
    );
    let rax = general(Gpr::RAX, Width::Qword);
    check(
        &[0x48, 0x83, 0xE0, 0xF0],
        Mnemonic::And,
        Width::Qword,
        &[rax, Operand::Immediate(-16)],
    );
    check(
        &[0x48, 0x81, 0xE8, 0x00, 0x00, 0x00, 0x80],
        Mnemonic::Sub,
        Width::Qword,
        &[
            general(Gpr::RAX, Width::Qword),
            Operand::Immediate(-0x8000_0000),
        ],
    );
    let eax = general(Gpr::RAX, Width::Dword);
    check(
        &[0xC1, 0xE0, 0xFF],
        Mnemonic::Shl,
        Width::Dword,
        &[eax, Operand::Immediate(0xFF)],
    );
    check(
        &[0xC2, 0x00, 0x80],
        Mnemonic::Ret,
        Width::Qword,
        &[Operand::Immediate(0x8000)],
    );
    check(
        &[0x69, 0xC1, 0x78, 0x56, 0x34, 0x12],
        Mnemonic::Imul,
        Width::Dword,
        &[
            eax,
            general(Gpr::RCX, Width::Dword),
            Operand::Immediate(0x1234_5678),
        ],
    );
    check(
        &[0xF6, 0x01, 0x80],
        Mnemonic::Test,
        Width::Byte,
        &[at(Gpr::RCX, 0, Width::Byte), Operand::Immediate(-0x80)],
    );
}

#[test]
fn a_64_bit_immediate_only_with_rex_w_on_b8() {
    let bytes = [0x48, 0xB8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
    check(
        &bytes,
        Mnemonic::Mov,
        Width::Qword,
        &[
            general(Gpr::RAX, Width::Qword),
            Operand::Immediate(0x1122_3344_5566_7788),
        ],
    );
    check(
        &[0x41, 0xB9, 0xFF, 0xFF, 0xFF, 0xFF],
        Mnemonic::Mov,
        Width::Dword,
        &[general(Gpr::R9, Width::Dword), Operand::Immediate(-1)],
    );
    check(
        &[0x66, 0xB8, 0x34, 0x12],
        Mnemonic::Mov,
        Width::Word,
        &[general(Gpr::RAX, Width::Word), Operand::Immediate(0x1234)],
    );
}

#[test]
fn moffs_moves_take_an_eight_byte_address() {
    let bytes = [0x48, 0xA1, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
    let source = Operand::Memory(Memory {
        segment: None,
        address: Address::Absolute(0x1122_3344_5566_7788),
        width: Width::Qword,
    });
    check(
        &bytes,
        Mnemonic::Mov,
        Width::Qword,
        &[general(Gpr::RAX, Width::Qword), source],
    );
}

#[test]
fn operand_size_prefix_gives_16_bits_and_rex_w_wins() {
    check(
        &[0x66, 0xC7, 0x00, 0x34, 0x12],
        Mnemonic::Mov,
        Width::Word,
        &[at(Gpr::RAX, 0, Width::Word), Operand::Immediate(0x1234)],
    );
    let rax = general(Gpr::RAX, Width::Qword);
    let rbx = general(Gpr::RBX, Width::Qword);
    check(
        &[0x66, 0x48, 0x89, 0xD8],
        Mnemonic::Mov,
        Width::Qword,
        &[rax, rbx],
    );
}

#[test]
fn a_rex_prefix_before_a_legacy_prefix_is_ignored() {
    let ax = general(Gpr::RAX, Width::Word);
    let bx = general(Gpr::RBX, Width::Word);
    check(
        &[0x48, 0x66, 0x89, 0xD8],
        Mnemonic::Mov,
        Width::Word,
        &[ax, bx],
    );
}

#[test]
fn byte_registers_four_to_seven_are_high_bytes_only_without_rex() {
    let sil = general(Gpr::RSI, Width::Byte);
    check(
        &[0x40, 0xB6, 0x01],
        Mnemonic::Mov,
        Width::Byte,
        &[sil, Operand::Immediate(1)],
    );
    let dh = Operand::Register(Register::HighByte(Gpr::RDX));
    check(
        &[0xB6, 0x01],
        Mnemonic::Mov,
        Width::Byte,
        &[dh, Operand::Immediate(1)],
    );
    let eax = general(Gpr::RAX, Width::Dword);
    let ah = Operand::Register(Register::HighByte(Gpr::RAX));
    check(
        &[0x0F, 0xBE, 0xC4],
        Mnemonic::Movsx,
        Width::Dword,
        &[eax, ah],
    );
    let rax = general(Gpr::RAX, Width::Qword);
    let spl = general(Gpr::RSP, Width::Byte);
    check(
        &[0x48, 0x0F, 0xBE, 0xC4],
        Mnemonic::Movsx,
        Width::Qword,
        &[rax, spl],
    );
}

#[test]
fn branches_resolve_their_targets() {
    check(
        &[0xE8, 0x00, 0x01, 0x00, 0x00],
        Mnemonic::Call,
        Width::Qword,
        &[Operand::Target(AT + 5 + 0x100)],
    );
    check(
        &[0x75, 0xFE],
        Mnemonic::Jcc(Condition::NotEqual),
        Width::Qword,
        &[Operand::Target(AT)],
    );
    check(
        &[0x0F, 0x84, 0x10, 0x00, 0x00, 0x00],
        Mnemonic::Jcc(Condition::Equal),
        Width::Qword,
        &[Operand::Target(AT + 6 + 0x10)],
    );
    check(
        &[0xEB, 0x80],
        Mnemonic::Jmp,
        Width::Qword,
        &[Operand::Target(AT + 2 - 0x80)],
    );
    check(
        &[0xE3, 0x00],
        Mnemonic::Jrcxz,
        Width::Qword,
        &[Operand::Target(AT + 2)],
    );
    check(
        &[0xE2, 0xFE],
        Mnemonic::Loop,
        Width::Qword,
        &[Operand::Target(AT)],
    );
    check(
        &[0xFF, 0xD0],
        Mnemonic::Call,
        Width::Qword,
        &[general(Gpr::RAX, Width::Qword)],
    );
    check(
        &[0x41, 0xFF, 0xE3],
        Mnemonic::Jmp,
        Width::Qword,
        &[general(Gpr::R11, Width::Qword)],
    );
    check(&[0xC3], Mnemonic::Ret, Width::Qword, &[]);
    assert_eq!(decoded(&[0xF3, 0xC3]).mnemonic, Mnemonic::Ret);
}

#[test]
fn all_sixteen_conditions_follow_the_low_opcode_bits() {
    let expected = [
        Condition::Overflow,
        Condition::NoOverflow,
        Condition::Below,
        Condition::AboveOrEqual,
        Condition::Equal,
        Condition::NotEqual,
        Condition::BelowOrEqual,
        Condition::Above,
        Condition::Sign,
        Condition::NoSign,
        Condition::Parity,
        Condition::NoParity,
        Condition::Less,
        Condition::GreaterOrEqual,
        Condition::LessOrEqual,
        Condition::Greater,
    ];
    for (code, condition) in (0u8..).zip(expected) {
        assert_eq!(
            decoded(&[0x70 + code, 0x00]).mnemonic,
            Mnemonic::Jcc(condition)
        );
        assert_eq!(
            decoded(&[0x0F, 0x40 + code, 0xC1]).mnemonic,
            Mnemonic::Cmov(condition)
        );
        assert_eq!(
            decoded(&[0x0F, 0x90 + code, 0xC0]).mnemonic,
            Mnemonic::Set(condition)
        );
    }
}

#[test]
fn the_stack_instructions_default_to_64_bits() {
    check(
        &[0x41, 0x57],
        Mnemonic::Push,
        Width::Qword,
        &[general(Gpr::R15, Width::Qword)],
    );
    check(
        &[0x5D],
        Mnemonic::Pop,
        Width::Qword,
        &[general(Gpr::RBP, Width::Qword)],
    );
    check(
        &[0x66, 0x50],
        Mnemonic::Push,
        Width::Word,
        &[general(Gpr::RAX, Width::Word)],
    );
    check(
        &[0x6A, 0xFF],
        Mnemonic::Push,
        Width::Qword,
        &[Operand::Immediate(-1)],
    );
    check(
        &[0x68, 0x78, 0x56, 0x34, 0x12],
        Mnemonic::Push,
        Width::Qword,
        &[Operand::Immediate(0x1234_5678)],
    );
    check(
        &[0xFF, 0x30],
        Mnemonic::Push,
        Width::Qword,
        &[at(Gpr::RAX, 0, Width::Qword)],
    );
    check(
        &[0x8F, 0xC0],
        Mnemonic::Pop,
        Width::Qword,
        &[general(Gpr::RAX, Width::Qword)],
    );
    check(&[0x9C], Mnemonic::Pushf, Width::Qword, &[]);
    check(&[0xC9], Mnemonic::Leave, Width::Qword, &[]);
    check(
        &[0xC8, 0x20, 0x00, 0x00],
        Mnemonic::Enter,
        Width::Qword,
        &[Operand::Immediate(0x20), Operand::Immediate(0)],
    );
}

#[test]
fn groups_select_the_operation_by_modrm_reg() {
    let eax = general(Gpr::RAX, Width::Dword);
    let ecx = general(Gpr::RCX, Width::Dword);
    let cl = general(Gpr::RCX, Width::Byte);
    let rcx = general(Gpr::RCX, Width::Qword);
    check(&[0xF7, 0xD8], Mnemonic::Neg, Width::Dword, &[eax]);
    check(&[0xF7, 0xD0], Mnemonic::Not, Width::Dword, &[eax]);
    check(&[0xF7, 0xF1], Mnemonic::Div, Width::Dword, &[ecx]);
    check(&[0xF7, 0xF9], Mnemonic::Idiv, Width::Dword, &[ecx]);
    check(&[0xF6, 0xE1], Mnemonic::Mul, Width::Byte, &[cl]);
    check(&[0x48, 0xF7, 0xE9], Mnemonic::Imul, Width::Qword, &[rcx]);
    check(&[0xD3, 0xF8], Mnemonic::Sar, Width::Dword, &[eax, cl]);
    check(
        &[0xD1, 0xE8],
        Mnemonic::Shr,
        Width::Dword,
        &[eax, Operand::Immediate(1)],
    );
    check(
        &[0xD1, 0xC0],
        Mnemonic::Rol,
        Width::Dword,
        &[eax, Operand::Immediate(1)],
    );
    check(&[0xFF, 0xC1], Mnemonic::Inc, Width::Dword, &[ecx]);
    check(&[0xFE, 0xC9], Mnemonic::Dec, Width::Byte, &[cl]);
    check(
        &[0x83, 0xF9, 0x05],
        Mnemonic::Cmp,
        Width::Dword,
        &[ecx, Operand::Immediate(5)],
    );
    check(
        &[0x80, 0xF1, 0x01],
        Mnemonic::Xor,
        Width::Byte,
        &[cl, Operand::Immediate(1)],
    );
}

#[test]
fn the_arithmetic_rows_share_one_layout() {
    let eax = general(Gpr::RAX, Width::Dword);
    let ecx = general(Gpr::RCX, Width::Dword);
    let al = general(Gpr::RAX, Width::Byte);
    let rows = [
        (0x00, Mnemonic::Add),
        (0x08, Mnemonic::Or),
        (0x10, Mnemonic::Adc),
        (0x18, Mnemonic::Sbb),
        (0x20, Mnemonic::And),
        (0x28, Mnemonic::Sub),
        (0x30, Mnemonic::Xor),
        (0x38, Mnemonic::Cmp),
    ];
    for (row, mnemonic) in rows {
        check(&[row + 1, 0xC8], mnemonic, Width::Dword, &[eax, ecx]);
        check(&[row + 3, 0xC1], mnemonic, Width::Dword, &[eax, ecx]);
        check(
            &[row + 4, 0x7F],
            mnemonic,
            Width::Byte,
            &[al, Operand::Immediate(0x7F)],
        );
        check(
            &[row + 5, 0x01, 0x00, 0x00, 0x00],
            mnemonic,
            Width::Dword,
            &[eax, Operand::Immediate(1)],
        );
    }
}

#[test]
fn two_byte_integer_instructions_decode() {
    let eax = general(Gpr::RAX, Width::Dword);
    let ecx = general(Gpr::RCX, Width::Dword);
    let rax = general(Gpr::RAX, Width::Qword);
    check(
        &[0x0F, 0xB6, 0xC1],
        Mnemonic::Movzx,
        Width::Dword,
        &[eax, general(Gpr::RCX, Width::Byte)],
    );
    check(
        &[0x0F, 0xB7, 0xC1],
        Mnemonic::Movzx,
        Width::Dword,
        &[eax, general(Gpr::RCX, Width::Word)],
    );
    check(
        &[0x48, 0x63, 0xC1],
        Mnemonic::Movsxd,
        Width::Qword,
        &[rax, ecx],
    );
    check(
        &[0x0F, 0xAF, 0xC1],
        Mnemonic::Imul,
        Width::Dword,
        &[eax, ecx],
    );
    check(
        &[0x0F, 0xBA, 0xE0, 0x05],
        Mnemonic::Bt,
        Width::Dword,
        &[eax, Operand::Immediate(5)],
    );
    check(&[0x0F, 0xA3, 0xC8], Mnemonic::Bt, Width::Dword, &[eax, ecx]);
    check(
        &[0xF3, 0x0F, 0xB8, 0xC1],
        Mnemonic::Popcnt,
        Width::Dword,
        &[eax, ecx],
    );
    check(
        &[0xF3, 0x0F, 0xBC, 0xC1],
        Mnemonic::Tzcnt,
        Width::Dword,
        &[eax, ecx],
    );
    check(
        &[0xF3, 0x0F, 0xBD, 0xC1],
        Mnemonic::Lzcnt,
        Width::Dword,
        &[eax, ecx],
    );
    check(
        &[0x0F, 0xBC, 0xC1],
        Mnemonic::Bsf,
        Width::Dword,
        &[eax, ecx],
    );
    check(
        &[0x0F, 0xBD, 0xC1],
        Mnemonic::Bsr,
        Width::Dword,
        &[eax, ecx],
    );
    check(&[0x0F, 0xC8], Mnemonic::Bswap, Width::Dword, &[eax]);
    check(
        &[0x49, 0x0F, 0xC9],
        Mnemonic::Bswap,
        Width::Qword,
        &[general(Gpr::R9, Width::Qword)],
    );
    check(
        &[0x0F, 0xA4, 0xC8, 0x04],
        Mnemonic::Shld,
        Width::Dword,
        &[eax, ecx, Operand::Immediate(4)],
    );
    check(
        &[0x0F, 0xAD, 0xC8],
        Mnemonic::Shrd,
        Width::Dword,
        &[eax, ecx, general(Gpr::RCX, Width::Byte)],
    );
    check(&[0x99], Mnemonic::ExtendIntoRdx, Width::Dword, &[]);
    check(&[0x48, 0x99], Mnemonic::ExtendIntoRdx, Width::Qword, &[]);
    check(
        &[0x48, 0x98],
        Mnemonic::ExtendAccumulator,
        Width::Qword,
        &[],
    );
    check(&[0x0F, 0xA2], Mnemonic::Cpuid, Width::Byte, &[]);
    check(&[0x0F, 0x31], Mnemonic::Rdtsc, Width::Byte, &[]);
    check(&[0x0F, 0x01, 0xD0], Mnemonic::Xgetbv, Width::Byte, &[]);
    check(&[0x0F, 0x01, 0xF9], Mnemonic::Rdtscp, Width::Byte, &[]);
    check(&[0x0F, 0x05], Mnemonic::Syscall, Width::Byte, &[]);
    check(&[0x0F, 0x0B], Mnemonic::Undefined, Width::Byte, &[]);
    check(
        &[0xCD, 0x29],
        Mnemonic::Int,
        Width::Byte,
        &[Operand::Immediate(0x29)],
    );
    check(&[0x0F, 0xC7, 0xF0], Mnemonic::Rdrand, Width::Dword, &[eax]);
}

#[test]
fn nops_pause_endbr64_and_xchg_with_r8_are_told_apart() {
    check(&[0x90], Mnemonic::Nop, Width::Byte, &[]);
    check(&[0xF3, 0x90], Mnemonic::Pause, Width::Byte, &[]);
    check(
        &[0x49, 0x90],
        Mnemonic::Xchg,
        Width::Qword,
        &[
            general(Gpr::R8, Width::Qword),
            general(Gpr::RAX, Width::Qword),
        ],
    );
    check(
        &[0xF3, 0x0F, 0x1E, 0xFA],
        Mnemonic::Endbr64,
        Width::Byte,
        &[],
    );
    let long_nop = [0x66, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(decoded(&long_nop).mnemonic, Mnemonic::Nop);
    assert_eq!(
        decoded(&[0x0F, 0x1F, 0x44, 0x00, 0x00]).mnemonic,
        Mnemonic::Nop
    );
    assert_eq!(decoded(&[0x0F, 0x18, 0x08]).mnemonic, Mnemonic::Prefetch);
    assert_eq!(decoded(&[0x0F, 0x0D, 0x08]).mnemonic, Mnemonic::Prefetch);
}

#[test]
fn string_instructions_keep_their_repeat_prefix() {
    let movsb = decoded(&[0xF3, 0xA4]);
    assert_eq!((movsb.mnemonic, movsb.width), (Mnemonic::Movs, Width::Byte));
    assert_eq!(movsb.repeat, Some(Repeat::Rep));
    let stosq = decoded(&[0xF3, 0x48, 0xAB]);
    assert_eq!(
        (stosq.mnemonic, stosq.width),
        (Mnemonic::Stos, Width::Qword)
    );
    assert_eq!(stosq.repeat, Some(Repeat::Rep));
    let scasb = decoded(&[0xF2, 0xAE]);
    assert_eq!(
        (scasb.mnemonic, scasb.repeat),
        (Mnemonic::Scas, Some(Repeat::Repne))
    );
    let lodsd = decoded(&[0xAD]);
    assert_eq!(
        (lodsd.mnemonic, lodsd.width, lodsd.repeat),
        (Mnemonic::Lods, Width::Dword, None)
    );
    assert_eq!(decoded(&[0xA6]).mnemonic, Mnemonic::Cmps);
}

#[test]
fn lock_is_kept_on_memory_destinations_and_invalid_elsewhere() {
    let cmpxchg = decoded(&[0xF0, 0x0F, 0xB1, 0x0F]);
    assert_eq!(cmpxchg.mnemonic, Mnemonic::Cmpxchg);
    assert!(cmpxchg.lock);
    assert_eq!(
        cmpxchg.operands(),
        &[
            at(Gpr::RDI, 0, Width::Dword),
            general(Gpr::RCX, Width::Dword)
        ]
    );
    let xadd = decoded(&[0xF0, 0x0F, 0xC1, 0x07]);
    assert!(xadd.lock && xadd.mnemonic == Mnemonic::Xadd);
    let cmpxchg16b = decoded(&[0xF0, 0x48, 0x0F, 0xC7, 0x0E]);
    assert_eq!(
        (cmpxchg16b.mnemonic, cmpxchg16b.width),
        (Mnemonic::Cmpxchg16b, Width::Dqword)
    );
    assert_eq!(cmpxchg16b.operands(), &[at(Gpr::RSI, 0, Width::Dqword)]);
    assert_eq!(decoded(&[0x0F, 0xC7, 0x0E]).mnemonic, Mnemonic::Cmpxchg8b);
    assert!(decoded(&[0xF0, 0x83, 0x00, 0x01]).lock);
    assert!(!decoded(&[0x87, 0x07]).lock);
    assert_eq!(error(&[0xF0, 0x01, 0xC0]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0xF0, 0x89, 0x00]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0xF0, 0x83, 0x38, 0x01]), DecodeErrorKind::Invalid);
}

#[test]
fn opcodes_removed_from_64_bit_mode_are_invalid() {
    for opcode in [
        0x06, 0x07, 0x0E, 0x16, 0x17, 0x1E, 0x1F, 0x27, 0x2F, 0x37, 0x3F, 0x60, 0x61, 0x82, 0x9A,
        0xCE, 0xD4, 0xD5, 0xD6, 0xEA,
    ] {
        assert_eq!(
            error(&[opcode, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
            DecodeErrorKind::Invalid,
            "{opcode:#x}"
        );
    }
    assert_eq!(error(&[0x8D, 0xC0]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0xFF, 0xF8]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0xFE, 0xD0]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0xC6, 0xC8, 0x00]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0x0F, 0xBA, 0xC0, 0x00]), DecodeErrorKind::Invalid);
    assert_eq!(error(&[0x0F, 0x04]), DecodeErrorKind::Invalid);
}

#[test]
fn short_input_is_truncated_and_more_than_fifteen_bytes_too_long() {
    assert_eq!(error(&[]), DecodeErrorKind::Truncated);
    assert_eq!(error(&[0x48]), DecodeErrorKind::Truncated);
    assert_eq!(error(&[0x8B, 0x05, 0x00]), DecodeErrorKind::Truncated);
    assert_eq!(error(&[0xE8, 0x00, 0x00]), DecodeErrorKind::Truncated);
    let mut fifteen = [0x66; MAX_INSTRUCTION_LENGTH];
    fifteen[MAX_INSTRUCTION_LENGTH - 1] = 0x90;
    assert_eq!(decoded(&fifteen).mnemonic, Mnemonic::Nop);
    let mut sixteen = [0x66; MAX_INSTRUCTION_LENGTH + 1];
    sixteen[MAX_INSTRUCTION_LENGTH] = 0x90;
    assert_eq!(error(&sixteen), DecodeErrorKind::TooLong);
}

#[test]
fn unsupported_instructions_report_their_length() {
    assert_eq!(refused(&[0xC5, 0xF8, 0x77]), Refusal::Vex);
    assert_eq!(refused(&[0xC5, 0xFD, 0x6F, 0x06]), Refusal::Vex);
    assert_eq!(refused(&[0xC4, 0xE3, 0x7D, 0x18, 0xC1, 0x01]), Refusal::Vex);
    assert_eq!(
        refused(&[0xC4, 0xE2, 0x7D, 0x58, 0x44, 0x24, 0x08]),
        Refusal::Vex
    );
    assert_eq!(
        refused(&[0x62, 0xF1, 0x7C, 0x48, 0x58, 0xC1]),
        Refusal::Evex
    );
    assert_eq!(refused(&[0x8F, 0xE8, 0x78, 0xC2, 0xC1, 0x01]), Refusal::Xop);
    assert_eq!(refused(&[0x0F, 0x38, 0x00, 0xC1]), Refusal::ThreeByteMap);
    assert_eq!(
        refused(&[0x66, 0x0F, 0x38, 0x00, 0xC1]),
        Refusal::ThreeByteMap
    );
    assert_eq!(
        refused(&[0x66, 0x0F, 0x3A, 0x0F, 0xC1, 0x08]),
        Refusal::ThreeByteMap
    );
    assert_eq!(refused(&[0x67, 0x8B, 0x00]), Refusal::AddressSize);
    assert_eq!(refused(&[0x67, 0xA4]), Refusal::AddressSize);
    assert_eq!(
        refused(&[0x66, 0xE8, 0x00, 0x00, 0x00, 0x00]),
        Refusal::Branch16
    );
    assert_eq!(refused(&[0xE4, 0x60]), Refusal::System);
    assert_eq!(refused(&[0xFA]), Refusal::System);
    assert_eq!(refused(&[0x8E, 0xD8]), Refusal::System);
    assert_eq!(refused(&[0xCA, 0x08, 0x00]), Refusal::System);
    assert_eq!(refused(&[0xFF, 0x18]), Refusal::System);
    assert_eq!(
        refused(&[0xC7, 0xF8, 0x10, 0x00, 0x00, 0x00]),
        Refusal::System
    );
    assert_eq!(refused(&[0x0F, 0xAE, 0x00]), Refusal::StateSave);
    assert_eq!(error(&[0x48, 0xC5, 0xF8, 0x77]), DecodeErrorKind::Invalid);
}

#[test]
fn the_error_names_the_address_and_the_reason() {
    let error = DecodeError {
        address: 0x1_4000_1020,
        kind: DecodeErrorKind::Unsupported {
            refusal: Refusal::Vex,
            length: 4,
        },
    };
    assert_eq!(
        error.to_string(),
        "0x140001020: an AVX (VEX-encoded) instruction, which EggOS does not translate"
    );
}

#[test]
fn x87_memory_and_register_forms_decode() {
    check(
        &[0xDD, 0x00],
        Mnemonic::Fld,
        Width::Qword,
        &[at(Gpr::RAX, 0, Width::Qword)],
    );
    check(
        &[0xD9, 0x00],
        Mnemonic::Fld,
        Width::Dword,
        &[at(Gpr::RAX, 0, Width::Dword)],
    );
    check(
        &[0xDB, 0x2C, 0x24],
        Mnemonic::Fld,
        Width::Tbyte,
        &[at(Gpr::RSP, 0, Width::Tbyte)],
    );
    check(
        &[0xDF, 0x2C, 0x24],
        Mnemonic::Fild,
        Width::Qword,
        &[at(Gpr::RSP, 0, Width::Qword)],
    );
    check(
        &[0xD9, 0x7C, 0x24, 0x08],
        Mnemonic::Fnstcw,
        Width::Word,
        &[at(Gpr::RSP, 8, Width::Word)],
    );
    check(
        &[0xDC, 0x08],
        Mnemonic::Fmul,
        Width::Qword,
        &[at(Gpr::RAX, 0, Width::Qword)],
    );
    check(&[0xD9, 0xC9], Mnemonic::Fxch, Width::Tbyte, &[st(1)]);
    check(&[0xD8, 0xC1], Mnemonic::Fadd, Width::Tbyte, &[st(0), st(1)]);
    check(
        &[0xDE, 0xC1],
        Mnemonic::Faddp,
        Width::Tbyte,
        &[st(1), st(0)],
    );
    check(&[0xDC, 0xE9], Mnemonic::Fsub, Width::Tbyte, &[st(1), st(0)]);
    check(
        &[0xDC, 0xE1],
        Mnemonic::Fsubr,
        Width::Tbyte,
        &[st(1), st(0)],
    );
    check(
        &[0xDE, 0xF9],
        Mnemonic::Fdivp,
        Width::Tbyte,
        &[st(1), st(0)],
    );
    check(
        &[0xDF, 0xE9],
        Mnemonic::Fucomip,
        Width::Tbyte,
        &[st(0), st(1)],
    );
    check(
        &[0xDF, 0xE0],
        Mnemonic::Fnstsw,
        Width::Word,
        &[general(Gpr::RAX, Width::Word)],
    );
    check(&[0xD9, 0xE8], Mnemonic::Fld1, Width::Tbyte, &[]);
    check(&[0x9B], Mnemonic::Fwait, Width::Byte, &[]);
    check(&[0xD8, 0xD1], Mnemonic::Fcom, Width::Tbyte, &[st(0), st(1)]);
    check(
        &[0xDD, 0xE9],
        Mnemonic::Fucomp,
        Width::Tbyte,
        &[st(0), st(1)],
    );
    check(
        &[0xDC, 0x18],
        Mnemonic::Fcomp,
        Width::Qword,
        &[at(Gpr::RAX, 0, Width::Qword)],
    );
    check(&[0xDE, 0xD9], Mnemonic::Fcompp, Width::Tbyte, &[]);
    check(&[0xDA, 0xE9], Mnemonic::Fucompp, Width::Tbyte, &[]);
    check(&[0xD9, 0xFA], Mnemonic::Fsqrt, Width::Tbyte, &[]);
    check(&[0xD9, 0xFC], Mnemonic::Frndint, Width::Tbyte, &[]);
    check(&[0xD9, 0xEB], Mnemonic::Fldpi, Width::Tbyte, &[]);
    assert_eq!(refused(&[0xD9, 0xF0]), Refusal::X87);
    assert_eq!(refused(&[0xDD, 0x08]), Refusal::Sse3);
    assert_eq!(refused(&[0xD9, 0x30]), Refusal::X87);
    assert_eq!(error(&[0xD9, 0x08]), DecodeErrorKind::Invalid);
}
