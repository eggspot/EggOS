//! Which x86 flags each instruction reads, defines and destroys, and the backward liveness pass
//! over a block that lets the code generator skip every flag result nobody reads (D191).

use alloc::vec::Vec;

use crate::instruction::{Condition, Instruction, Mnemonic, Operand, Width};

/// A set of x86 status flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(u8);

impl Flags {
    /// No flag.
    pub const NONE: Flags = Flags(0);
    /// The carry flag.
    pub const CF: Flags = Flags(1);
    /// The parity flag (of the result's low byte).
    pub const PF: Flags = Flags(2);
    /// The adjust flag (the carry out of bit 3).
    pub const AF: Flags = Flags(4);
    /// The zero flag.
    pub const ZF: Flags = Flags(8);
    /// The sign flag.
    pub const SF: Flags = Flags(16);
    /// The overflow flag.
    pub const OF: Flags = Flags(32);
    /// All six status flags.
    pub const ALL: Flags = Flags(63);
    /// The four flags kept in NZCV: CF, ZF, SF and OF.
    pub const NZCV: Flags = Flags(1 | 8 | 16 | 32);

    /// Both sets.
    pub const fn union(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }

    /// The flags in both sets.
    pub const fn intersection(self, other: Flags) -> Flags {
        Flags(self.0 & other.0)
    }

    /// The flags of `self` not in `other`.
    pub const fn without(self, other: Flags) -> Flags {
        Flags(self.0 & !other.0)
    }

    /// Whether any flag of `other` is in `self`.
    pub const fn any(self, other: Flags) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What one instruction does to the flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagEffect {
    /// The flags it reads.
    pub reads: Flags,
    /// The flags it sets to a defined value (on every path through it).
    pub defines: Flags,
    /// The flags whose earlier value is gone afterwards: `defines` plus those left undefined.
    /// An instruction that may leave the flags unchanged at run time (a shift by `cl`) kills
    /// nothing.
    pub kills: Flags,
}

impl FlagEffect {
    const NONE: FlagEffect = FlagEffect {
        reads: Flags::NONE,
        defines: Flags::NONE,
        kills: Flags::NONE,
    };

    const fn writes(defines: Flags, kills: Flags) -> FlagEffect {
        FlagEffect {
            reads: Flags::NONE,
            defines,
            kills,
        }
    }

    const fn reads(reads: Flags) -> FlagEffect {
        FlagEffect {
            reads,
            defines: Flags::NONE,
            kills: Flags::NONE,
        }
    }
}

/// The flags a condition code tests.
pub const fn condition_flags(condition: Condition) -> Flags {
    match condition {
        Condition::Overflow | Condition::NoOverflow => Flags::OF,
        Condition::Below | Condition::AboveOrEqual => Flags::CF,
        Condition::Equal | Condition::NotEqual => Flags::ZF,
        Condition::BelowOrEqual | Condition::Above => Flags::CF.union(Flags::ZF),
        Condition::Sign | Condition::NoSign => Flags::SF,
        Condition::Parity | Condition::NoParity => Flags::PF,
        Condition::Less | Condition::GreaterOrEqual => Flags::SF.union(Flags::OF),
        Condition::LessOrEqual | Condition::Greater => Flags::ZF.union(Flags::SF).union(Flags::OF),
    }
}

/// The shift or rotate count of an instruction when it is an immediate, masked as x86 masks
/// it (to 6 bits for 64-bit operands, 5 otherwise); `None` when the count is in `cl`.
pub fn immediate_count(instruction: &Instruction) -> Option<u8> {
    let mask = if instruction.width == Width::Qword {
        0x3F
    } else {
        0x1F
    };
    match instruction.operands().last() {
        Some(Operand::Immediate(count)) => Some((*count as u8) & mask),
        _ => None,
    }
}

const ARITHMETIC: Flags = Flags::ALL;
const LOGIC: Flags = Flags::ALL.without(Flags::AF);
const SHIFT: Flags = Flags::CF.union(Flags::PF).union(Flags::ZF).union(Flags::SF);
const ROTATE: Flags = Flags::CF.union(Flags::OF);

fn shift_effect(instruction: &Instruction, defined: Flags, killed: Flags) -> FlagEffect {
    match immediate_count(instruction) {
        Some(0) => FlagEffect::NONE,
        Some(1) => FlagEffect::writes(defined.union(Flags::OF), killed),
        Some(_) => FlagEffect::writes(defined, killed),
        None => FlagEffect {
            reads: Flags::NONE,
            defines: defined.union(Flags::OF),
            kills: Flags::NONE,
        },
    }
}

/// What `instruction` does to the flags.
pub fn effect(instruction: &Instruction) -> FlagEffect {
    use Mnemonic as M;
    match instruction.mnemonic {
        M::Add | M::Sub | M::Cmp | M::Neg | M::Xadd | M::Cmpxchg => {
            FlagEffect::writes(ARITHMETIC, ARITHMETIC)
        }
        M::Adc | M::Sbb => FlagEffect {
            reads: Flags::CF,
            defines: ARITHMETIC,
            kills: ARITHMETIC,
        },
        M::And | M::Or | M::Xor | M::Test => FlagEffect::writes(LOGIC, Flags::ALL),
        M::Inc | M::Dec => {
            let flags = Flags::ALL.without(Flags::CF);
            FlagEffect::writes(flags, flags)
        }
        M::Mul | M::Imul => FlagEffect::writes(Flags::CF.union(Flags::OF), Flags::ALL),
        M::Div | M::Idiv => FlagEffect::writes(Flags::NONE, Flags::ALL),
        M::Shl | M::Shr | M::Sar | M::Shld | M::Shrd => {
            shift_effect(instruction, SHIFT, Flags::ALL)
        }
        M::Rol | M::Ror => shift_effect(instruction, Flags::CF, ROTATE),
        M::Rcl | M::Rcr => {
            let rotate = shift_effect(instruction, Flags::CF, ROTATE);
            FlagEffect {
                reads: Flags::CF,
                ..rotate
            }
        }
        M::Bt | M::Bts | M::Btr | M::Btc => {
            FlagEffect::writes(Flags::CF, Flags::ALL.without(Flags::ZF))
        }
        M::Bsf | M::Bsr | M::Tzcnt => FlagEffect::writes(Flags::ZF, Flags::ALL),
        M::Lzcnt => FlagEffect::writes(Flags::CF.union(Flags::ZF), Flags::ALL),
        M::Popcnt => FlagEffect::writes(Flags::ALL, Flags::ALL),
        M::Cmpxchg8b | M::Cmpxchg16b => FlagEffect::writes(Flags::ZF, Flags::ZF),
        M::Comiss | M::Comisd | M::Ucomiss | M::Ucomisd => {
            FlagEffect::writes(Flags::ALL, Flags::ALL)
        }
        M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => FlagEffect::writes(Flags::ALL, Flags::ALL),
        M::Jcc(condition) | M::Set(condition) | M::Cmov(condition) => {
            FlagEffect::reads(condition_flags(condition))
        }
        M::Loope | M::Loopne => FlagEffect::reads(Flags::ZF),
        M::Clc | M::Stc => FlagEffect::writes(Flags::CF, Flags::CF),
        M::Cmc => FlagEffect {
            reads: Flags::CF,
            defines: Flags::CF,
            kills: Flags::CF,
        },
        M::Lahf | M::Pushf => FlagEffect::reads(Flags::ALL),
        M::Sahf => {
            let flags = Flags::ALL.without(Flags::OF);
            FlagEffect::writes(flags, flags)
        }
        M::Popf => FlagEffect::writes(Flags::ALL, Flags::ALL),
        M::Cmps | M::Scas if instruction.repeat.is_some() => FlagEffect {
            reads: Flags::NONE,
            defines: ARITHMETIC,
            kills: Flags::NONE,
        },
        M::Cmps | M::Scas => FlagEffect::writes(ARITHMETIC, ARITHMETIC),
        _ => FlagEffect::NONE,
    }
}

/// For each instruction of a block, the flags some later instruction may read before they are
/// written again: element `i` is what is live right after instruction `i`. `live_out` is what
/// is live when the block exits (every flag when nothing better is known).
pub fn live_after(instructions: &[Instruction], live_out: Flags) -> Vec<Flags> {
    let mut live = Vec::with_capacity(instructions.len());
    live.resize(instructions.len(), Flags::NONE);
    let mut current = live_out;
    for (index, instruction) in instructions.iter().enumerate().rev() {
        live[index] = current;
        let effect = effect(instruction);
        current = current.without(effect.kills).union(effect.reads);
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    fn block(bytes: &[u8]) -> Vec<Instruction> {
        let mut instructions = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let instruction = decode(&bytes[offset..], 0x1000 + offset as u64).unwrap();
            offset += usize::from(instruction.length);
            instructions.push(instruction);
        }
        instructions
    }

    #[test]
    fn a_compare_feeding_a_branch_is_the_only_live_writer() {
        let instructions = block(&[0x01, 0xD8, 0x39, 0xD8, 0x74, 0x00]);
        let live = live_after(&instructions, Flags::NONE);
        assert_eq!(live, [Flags::NONE, Flags::ZF, Flags::NONE]);
    }

    #[test]
    fn increment_keeps_the_carry_of_an_earlier_add_live() {
        let instructions = block(&[0x01, 0xD8, 0xFF, 0xC1, 0x72, 0x00]);
        let live = live_after(&instructions, Flags::NONE);
        assert_eq!(live[0], Flags::CF);
        assert_eq!(live[1], Flags::CF);
    }

    #[test]
    fn a_shift_by_cl_lets_earlier_flags_through() {
        let instructions = block(&[0x39, 0xD8, 0xD3, 0xE0, 0x0F, 0x94, 0xC0]);
        let live = live_after(&instructions, Flags::NONE);
        assert_eq!(live[0], Flags::ZF);
        assert_eq!(live[1], Flags::ZF);
    }

    #[test]
    fn a_shift_by_zero_changes_nothing() {
        let instructions = block(&[0x39, 0xD8, 0xC1, 0xE0, 0x00, 0x0F, 0x92, 0xC0]);
        let live = live_after(&instructions, Flags::NONE);
        assert_eq!(live[0], Flags::CF);
    }

    #[test]
    fn live_out_flows_back_to_the_last_writer() {
        let instructions = block(&[0x29, 0xD8, 0x89, 0xC1]);
        let live = live_after(&instructions, Flags::ALL);
        assert_eq!(live, [Flags::ALL, Flags::ALL]);
        let dead = live_after(&instructions, Flags::NONE);
        assert_eq!(dead, [Flags::NONE, Flags::NONE]);
    }

    #[test]
    fn conditions_read_their_flags() {
        assert_eq!(
            condition_flags(Condition::Above),
            Flags::CF.union(Flags::ZF)
        );
        assert_eq!(condition_flags(Condition::Less), Flags::SF.union(Flags::OF));
    }
}
