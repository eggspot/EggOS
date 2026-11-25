//! Dalvik instructions lowered to a few machine-independent operations (D760), which the
//! aarch64 and riscv64 back ends emit: moves, constants, integer and floating-point
//! arithmetic, comparisons, branches, calls through the entry table (to the runtime's helpers
//! and to methods) and returns, each on Dalvik registers. It gives every instruction the same
//! meaning the x86_64 back end does (D471, D472, D499, D503, D504); a back end only chooses
//! where each register lives and encodes the operations.

use crate::{CompileError, helper, method_entry};
use alloc::vec::Vec;
use eggos_android_package::dalvik::{self, Instruction};
use eggos_android_package::dex::{Catches, Code};

const PACKED_SWITCH: u16 = 0x0100;
const SPARSE_SWITCH: u16 = 0x0200;
const ARRAY_DATA: u16 = 0x0300;
const MOST_FILLED_ELEMENTS: usize = 4096;

/// How wide a value is: a 32-bit int or float in a register's low half, or a 64-bit long,
/// double or object in all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Int,
    Long,
}

/// An integer operation of two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Add,
    Subtract,
    Multiply,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
}

/// A floating-point operation of two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// A branch's condition on two ints (or one int against zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Equal,
    NotEqual,
    Less,
    GreaterOrEqual,
    Greater,
    LessOrEqual,
}

/// A narrowing of an int.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Narrow {
    Byte,
    Char,
    Short,
}

/// An operand: a Dalvik register or a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Register(u32),
    Constant(i64),
}

/// One machine-independent operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Move {
        width: Width,
        to: u32,
        from: u32,
    },
    Constant {
        width: Width,
        to: u32,
        value: i64,
    },
    Binary {
        width: Width,
        operation: Operation,
        to: u32,
        left: Operand,
        right: Operand,
    },
    Negate {
        width: Width,
        to: u32,
        from: u32,
    },
    Not {
        width: Width,
        to: u32,
        from: u32,
    },
    FlipSign {
        width: Width,
        to: u32,
        from: u32,
    },
    IntToLong {
        to: u32,
        from: u32,
    },
    LongToInt {
        to: u32,
        from: u32,
    },
    Narrow {
        narrow: Narrow,
        to: u32,
        from: u32,
    },
    CompareLong {
        to: u32,
        left: u32,
        right: u32,
    },
    Float {
        width: Width,
        operation: FloatOperation,
        to: u32,
        left: u32,
        right: u32,
    },
    CompareFloat {
        width: Width,
        below_on_nan: bool,
        to: u32,
        left: u32,
        right: u32,
    },
    IntegerToFloat {
        from_width: Width,
        to_width: Width,
        to: u32,
        from: u32,
    },
    FloatToFloat {
        to_width: Width,
        to: u32,
        from: u32,
    },
    Call {
        entry: usize,
        arguments: Vec<Operand>,
        result: Option<(Width, u32)>,
    },
    Return {
        value: Option<(Width, u32)>,
    },
    Jump {
        target: usize,
    },
    Branch {
        condition: Condition,
        left: u32,
        right: Option<u32>,
        target: usize,
    },
    BranchOnKey {
        register: u32,
        key: i32,
        target: usize,
    },
}

/// One lowered instruction: the code unit it starts at, its operations, and the `try`
/// handlers that catch what its calls throw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowered {
    pub at: usize,
    pub ops: Vec<Op>,
    pub catches: Option<Catches>,
}

impl Lowered {
    /// Whether it calls out, so the pending exception is looked at after it.
    pub fn calls(&self) -> bool {
        self.ops.iter().any(|op| matches!(op, Op::Call { .. }))
    }
}

fn branch_target(at: usize, offset: i64) -> Option<usize> {
    usize::try_from(i64::try_from(at).ok()?.checked_add(offset)?).ok()
}

fn payload(code: &Code<'_>, at: usize, offset: i64, kind: u16) -> Option<usize> {
    let start = branch_target(at, offset)?;
    (code.unit(start)? == kind).then_some(start)
}

fn payload_word(code: &Code<'_>, at: usize) -> Option<u32> {
    Some(u32::from(code.unit(at)?) | u32::from(code.unit(at + 1)?) << 16)
}

fn int_operation(index: u8) -> Option<Operation> {
    Some(match index {
        0 => Operation::Add,
        1 => Operation::Subtract,
        2 => Operation::Multiply,
        5 => Operation::And,
        6 => Operation::Or,
        7 => Operation::Xor,
        8 => Operation::ShiftLeft,
        9 => Operation::ShiftRight,
        10 => Operation::UnsignedShiftRight,
        _ => return None,
    })
}

fn division_helper(width: Width, index: u8) -> Option<usize> {
    Some(match (width, index) {
        (Width::Int, 3) => helper::DIVIDE_INT,
        (Width::Int, 4) => helper::REMAINDER_INT,
        (Width::Long, 3) => helper::DIVIDE_LONG,
        (Width::Long, 4) => helper::REMAINDER_LONG,
        _ => return None,
    })
}

fn float_operation(index: u8) -> Option<FloatOperation> {
    Some(match index {
        0 => FloatOperation::Add,
        1 => FloatOperation::Subtract,
        2 => FloatOperation::Multiply,
        3 => FloatOperation::Divide,
        _ => return None,
    })
}

fn field_width(opcode: u8) -> Width {
    match opcode {
        0x53 | 0x54 | 0x5a | 0x5b | 0x61 | 0x62 | 0x45 | 0x46 => Width::Long,
        _ => Width::Int,
    }
}

fn is_virtual(opcode: u8) -> bool {
    matches!(opcode, 0x6e | 0x72 | 0x74 | 0x78)
}

const CONDITIONS: [Condition; 6] = [
    Condition::Equal,
    Condition::NotEqual,
    Condition::Less,
    Condition::GreaterOrEqual,
    Condition::Greater,
    Condition::LessOrEqual,
];

fn call(entry: usize, arguments: &[Operand], result: Option<(Width, u32)>) -> Op {
    Op::Call {
        entry,
        arguments: arguments.to_vec(),
        result,
    }
}

fn switch(
    code: &Code<'_>,
    instruction: &Instruction,
    at: usize,
    ops: &mut Vec<Op>,
) -> Option<()> {
    let sparse = instruction.opcode == 0x2c;
    let kind = if sparse { SPARSE_SWITCH } else { PACKED_SWITCH };
    let start = payload(code, at, instruction.literal, kind)?;
    let size = usize::from(code.unit(start + 1)?);
    let first_key = payload_word(code, start + 2)? as i32;
    let targets = match sparse {
        true => start + 2 + size * 2,
        false => start + 4,
    };
    for case in 0..size {
        let key = match sparse {
            true => payload_word(code, start + 2 + case * 2)? as i32,
            false => first_key.wrapping_add(case as i32),
        };
        let offset = payload_word(code, targets + case * 2)? as i32;
        let target = branch_target(at, i64::from(offset))?;
        ops.push(Op::BranchOnKey {
            register: instruction.a,
            key,
            target,
        });
    }
    Some(())
}

fn fill_array(
    code: &Code<'_>,
    instruction: &Instruction,
    at: usize,
    ops: &mut Vec<Op>,
) -> Option<()> {
    let start = payload(code, at, instruction.literal, ARRAY_DATA)?;
    let width = usize::from(code.unit(start + 1)?);
    let count = payload_word(code, start + 2)? as usize;
    if !matches!(width, 1 | 2 | 4 | 8) || count > MOST_FILLED_ELEMENTS {
        return None;
    }
    let mut bytes = Vec::new();
    for unit in 0..(width * count).div_ceil(2) {
        bytes.extend_from_slice(&code.unit(start + 4 + unit)?.to_le_bytes());
    }
    for (index, element) in bytes.chunks(width).take(count).enumerate() {
        let mut value = [0u8; 8];
        value[..width].copy_from_slice(element);
        let shift = 64 - 8 * width as u32;
        let value = i64::from_le_bytes(value) << shift >> shift;
        ops.push(call(
            helper::ARRAY_PUT,
            &[
                Operand::Register(instruction.a),
                Operand::Constant(index as i64),
                Operand::Constant(value),
            ],
            None,
        ));
    }
    Some(())
}

fn convert(opcode: u8, to: u32, from: u32) -> Option<Op> {
    let helper_call = |entry: usize, result: Width| {
        call(entry, &[Operand::Register(from)], Some((result, to)))
    };
    Some(match opcode {
        0x82 => Op::IntegerToFloat {
            from_width: Width::Int,
            to_width: Width::Int,
            to,
            from,
        },
        0x83 => Op::IntegerToFloat {
            from_width: Width::Int,
            to_width: Width::Long,
            to,
            from,
        },
        0x85 => Op::IntegerToFloat {
            from_width: Width::Long,
            to_width: Width::Int,
            to,
            from,
        },
        0x86 => Op::IntegerToFloat {
            from_width: Width::Long,
            to_width: Width::Long,
            to,
            from,
        },
        0x89 => Op::FloatToFloat {
            to_width: Width::Long,
            to,
            from,
        },
        0x8c => Op::FloatToFloat {
            to_width: Width::Int,
            to,
            from,
        },
        0x87 => helper_call(helper::FLOAT_TO_INT, Width::Int),
        0x88 => helper_call(helper::FLOAT_TO_LONG, Width::Long),
        0x8a => helper_call(helper::DOUBLE_TO_INT, Width::Int),
        0x8b => helper_call(helper::DOUBLE_TO_LONG, Width::Long),
        0x8d => Op::Narrow {
            narrow: Narrow::Byte,
            to,
            from,
        },
        0x8e => Op::Narrow {
            narrow: Narrow::Char,
            to,
            from,
        },
        0x8f => Op::Narrow {
            narrow: Narrow::Short,
            to,
            from,
        },
        _ => return None,
    })
}

fn integer(
    ops: &mut Vec<Op>,
    width: Width,
    index: u8,
    to: u32,
    left: u32,
    right: Operand,
) -> Option<()> {
    if let Some(operation) = int_operation(index) {
        ops.push(Op::Binary {
            width,
            operation,
            to,
            left: Operand::Register(left),
            right,
        });
        return Some(());
    }
    let entry = division_helper(width, index)?;
    ops.push(call(entry, &[Operand::Register(left), right], Some((width, to))));
    Some(())
}

fn floating(
    ops: &mut Vec<Op>,
    width: Width,
    index: u8,
    to: u32,
    left: u32,
    right: u32,
) -> Option<()> {
    if index == 4 {
        let entry = match width {
            Width::Int => helper::REMAINDER_FLOAT,
            Width::Long => helper::REMAINDER_DOUBLE,
        };
        ops.push(call(
            entry,
            &[Operand::Register(left), Operand::Register(right)],
            Some((width, to)),
        ));
        return Some(());
    }
    ops.push(Op::Float {
        width,
        operation: float_operation(index)?,
        to,
        left,
        right,
    });
    Some(())
}

fn lower_one(
    code: &Code<'_>,
    instruction: &Instruction,
    at: usize,
) -> Result<Vec<Op>, CompileError> {
    let opcode = instruction.opcode;
    let unsupported = CompileError::Unsupported { opcode, at };
    let bad_branch = CompileError::BadBranch { at };
    let (a, b, c) = (instruction.a, instruction.b, instruction.c);
    let index = i64::from(instruction.index);
    let register = Operand::Register;
    let constant = Operand::Constant;
    let mut ops = Vec::new();
    match opcode {
        0x00 | 0x1d | 0x1e => {}
        0x01..=0x03 => ops.push(Op::Move {
            width: Width::Int,
            to: a,
            from: b,
        }),
        0x04..=0x09 => ops.push(Op::Move {
            width: Width::Long,
            to: a,
            from: b,
        }),
        0x0a | 0x0b | 0x0c => {}
        0x0d => ops.push(call(helper::TAKE_EXCEPTION, &[], Some((Width::Long, a)))),
        0x0e => ops.push(Op::Return { value: None }),
        0x0f => ops.push(Op::Return {
            value: Some((Width::Int, a)),
        }),
        0x10 | 0x11 => ops.push(Op::Return {
            value: Some((Width::Long, a)),
        }),
        0x12..=0x15 => ops.push(Op::Constant {
            width: Width::Int,
            to: a,
            value: instruction.literal,
        }),
        0x16..=0x19 => ops.push(Op::Constant {
            width: Width::Long,
            to: a,
            value: instruction.literal,
        }),
        0x1a | 0x1b => ops.push(call(
            helper::STRING,
            &[constant(index)],
            Some((Width::Long, a)),
        )),
        0x22 => ops.push(call(
            helper::NEW_INSTANCE,
            &[constant(index)],
            Some((Width::Long, a)),
        )),
        0x1f => ops.push(call(
            helper::CHECK_CAST,
            &[register(a), constant(index)],
            None,
        )),
        0x20 => ops.push(call(
            helper::INSTANCE_OF,
            &[register(b), constant(index)],
            Some((Width::Int, a)),
        )),
        0x21 => ops.push(call(
            helper::ARRAY_LENGTH,
            &[register(b)],
            Some((Width::Int, a)),
        )),
        0x23 => ops.push(call(
            helper::NEW_ARRAY,
            &[register(b), constant(index)],
            Some((Width::Long, a)),
        )),
        0x26 => fill_array(code, instruction, at, &mut ops).ok_or(unsupported)?,
        0x27 => ops.push(call(helper::THROW, &[register(a)], None)),
        0x28..=0x2a => ops.push(Op::Jump {
            target: branch_target(at, instruction.literal).ok_or(bad_branch)?,
        }),
        0x2b | 0x2c => switch(code, instruction, at, &mut ops).ok_or(bad_branch)?,
        0x2d..=0x30 => ops.push(Op::CompareFloat {
            width: if opcode >= 0x2f { Width::Long } else { Width::Int },
            below_on_nan: matches!(opcode, 0x2d | 0x2f),
            to: a,
            left: b,
            right: c,
        }),
        0x31 => ops.push(Op::CompareLong {
            to: a,
            left: b,
            right: c,
        }),
        0x32..=0x37 => ops.push(Op::Branch {
            condition: CONDITIONS[usize::from(opcode - 0x32)],
            left: a,
            right: Some(b),
            target: branch_target(at, instruction.literal).ok_or(bad_branch)?,
        }),
        0x38..=0x3d => ops.push(Op::Branch {
            condition: CONDITIONS[usize::from(opcode - 0x38)],
            left: a,
            right: None,
            target: branch_target(at, instruction.literal).ok_or(bad_branch)?,
        }),
        0x44..=0x4a => ops.push(call(
            helper::ARRAY_GET,
            &[register(b), register(c)],
            Some((field_width(opcode), a)),
        )),
        0x4b..=0x51 => ops.push(call(
            helper::ARRAY_PUT,
            &[register(b), register(c), register(a)],
            None,
        )),
        0x52..=0x58 => ops.push(call(
            helper::FIELD_GET,
            &[register(b), constant(index)],
            Some((field_width(opcode), a)),
        )),
        0x59..=0x5f => ops.push(call(
            helper::FIELD_PUT,
            &[register(b), constant(index), register(a)],
            None,
        )),
        0x60..=0x66 => ops.push(call(
            helper::STATIC_GET,
            &[constant(index)],
            Some((field_width(opcode), a)),
        )),
        0x67..=0x6d => ops.push(call(
            helper::STATIC_PUT,
            &[constant(index), register(a)],
            None,
        )),
        0x6e..=0x72 | 0x74..=0x78 => {
            let mut arguments = Vec::new();
            if is_virtual(opcode) {
                arguments.push(constant(index));
            }
            let count = instruction.argument_count;
            match opcode {
                0x6e..=0x72 => arguments.extend(
                    instruction.arguments[..count]
                        .iter()
                        .map(|&each| register(u32::from(each))),
                ),
                _ => arguments.extend((0..count as u32).map(|word| register(b + word))),
            }
            let entry = match is_virtual(opcode) {
                true => helper::INVOKE_VIRTUAL,
                false => method_entry(instruction.index),
            };
            ops.push(Op::Call {
                entry,
                arguments,
                result: None,
            });
        }
        0x7b => ops.push(Op::Negate {
            width: Width::Int,
            to: a,
            from: b,
        }),
        0x7c => ops.push(Op::Not {
            width: Width::Int,
            to: a,
            from: b,
        }),
        0x7d => ops.push(Op::Negate {
            width: Width::Long,
            to: a,
            from: b,
        }),
        0x7e => ops.push(Op::Not {
            width: Width::Long,
            to: a,
            from: b,
        }),
        0x7f => ops.push(Op::FlipSign {
            width: Width::Int,
            to: a,
            from: b,
        }),
        0x80 => ops.push(Op::FlipSign {
            width: Width::Long,
            to: a,
            from: b,
        }),
        0x81 => ops.push(Op::IntToLong { to: a, from: b }),
        0x84 => ops.push(Op::LongToInt { to: a, from: b }),
        0x82 | 0x83 | 0x85..=0x8f => ops.push(convert(opcode, a, b).ok_or(unsupported)?),
        0x90..=0xa5 => {
            let (width, which) = match opcode {
                0x90..=0x9a => (Width::Int, opcode - 0x90),
                _ => (Width::Long, opcode - 0x9b),
            };
            integer(&mut ops, width, which, a, b, register(c)).ok_or(unsupported)?;
        }
        0xb0..=0xc5 => {
            let (width, which) = match opcode {
                0xb0..=0xba => (Width::Int, opcode - 0xb0),
                _ => (Width::Long, opcode - 0xbb),
            };
            integer(&mut ops, width, which, a, a, register(b)).ok_or(unsupported)?;
        }
        0xd0..=0xe2 => {
            let which = match opcode {
                0xd0..=0xd7 => opcode - 0xd0,
                _ => opcode - 0xd8,
            };
            if which == 1 {
                ops.push(Op::Binary {
                    width: Width::Int,
                    operation: Operation::Subtract,
                    to: a,
                    left: constant(instruction.literal),
                    right: register(b),
                });
            } else {
                integer(&mut ops, Width::Int, which, a, b, constant(instruction.literal))
                    .ok_or(unsupported)?;
            }
        }
        0xa6..=0xaf => {
            let (width, which) = match opcode {
                0xa6..=0xaa => (Width::Int, opcode - 0xa6),
                _ => (Width::Long, opcode - 0xab),
            };
            floating(&mut ops, width, which, a, b, c).ok_or(unsupported)?;
        }
        0xc6..=0xcf => {
            let (width, which) = match opcode {
                0xc6..=0xca => (Width::Int, opcode - 0xc6),
                _ => (Width::Long, opcode - 0xcb),
            };
            floating(&mut ops, width, which, a, a, b).ok_or(unsupported)?;
        }
        _ => return Err(unsupported),
    }
    Ok(ops)
}

/// Lowers every instruction of `code`, in order, payloads skipped.
pub fn lower(code: &Code<'_>) -> Result<Vec<Lowered>, CompileError> {
    let mut lowered = Vec::new();
    let mut at = 0;
    while at < code.length() {
        if let Some(length) = dalvik::payload_length(code, at) {
            at += length;
            continue;
        }
        let instruction = dalvik::decode(code, at).ok_or(CompileError::Truncated)?;
        if let 0x0a..=0x0c = instruction.opcode {
            let width = match instruction.opcode {
                0x0a => Width::Int,
                _ => Width::Long,
            };
            let previous = lowered
                .last_mut()
                .and_then(|last: &mut Lowered| last.ops.last_mut());
            if let Some(Op::Call { result, .. }) = previous
                && result.is_none()
            {
                *result = Some((width, instruction.a));
            }
        }
        let ops = lower_one(code, &instruction, at)?;
        let catches = match ops.iter().any(|op| matches!(op, Op::Call { .. })) {
            true => code.catches(at),
            false => None,
        };
        lowered.push(Lowered { at, ops, catches });
        at += instruction.length;
    }
    Ok(lowered)
}

/// How often each Dalvik register is read or written in `lowered`, for a back end's choice
/// of which registers live in machine registers.
pub fn uses(lowered: &[Lowered], registers: u16) -> Vec<u32> {
    let mut counts = alloc::vec![0u32; usize::from(registers)];
    let mut count = |register: u32| {
        if let Some(slot) = counts.get_mut(register as usize) {
            *slot = slot.saturating_add(1);
        }
    };
    let mut operand = |operand: &Operand, count: &mut dyn FnMut(u32)| {
        if let Operand::Register(register) = operand {
            count(*register);
        }
    };
    for instruction in lowered {
        for op in &instruction.ops {
            match op {
                Op::Move { to, from, .. }
                | Op::Negate { to, from, .. }
                | Op::Not { to, from, .. }
                | Op::FlipSign { to, from, .. }
                | Op::IntToLong { to, from }
                | Op::LongToInt { to, from }
                | Op::Narrow { to, from, .. }
                | Op::IntegerToFloat { to, from, .. }
                | Op::FloatToFloat { to, from, .. } => {
                    count(*to);
                    count(*from);
                }
                Op::Constant { to, .. } => count(*to),
                Op::Binary {
                    to, left, right, ..
                } => {
                    count(*to);
                    operand(left, &mut count);
                    operand(right, &mut count);
                }
                Op::CompareLong { to, left, right }
                | Op::Float {
                    to, left, right, ..
                }
                | Op::CompareFloat {
                    to, left, right, ..
                } => {
                    count(*to);
                    count(*left);
                    count(*right);
                }
                Op::Call {
                    arguments, result, ..
                } => {
                    for argument in arguments {
                        operand(argument, &mut count);
                    }
                    if let Some((_, to)) = result {
                        count(*to);
                    }
                }
                Op::Return { value } => {
                    if let Some((_, from)) = value {
                        count(*from);
                    }
                }
                Op::Branch { left, right, .. } => {
                    count(*left);
                    if let Some(right) = right {
                        count(*right);
                    }
                }
                Op::BranchOnKey { register, .. } => count(*register),
                Op::Jump { .. } => {}
            }
        }
    }
    counts
}

/// The `want` most used registers, most used first (the lower number first among equals).
pub fn most_used(counts: &[u32], want: usize) -> Vec<u32> {
    let mut order: Vec<u32> = (0..counts.len() as u32)
        .filter(|&register| counts[register as usize] > 0)
        .collect();
    order.sort_by(|&first, &second| {
        counts[second as usize]
            .cmp(&counts[first as usize])
            .then(first.cmp(&second))
    });
    order.truncate(want);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
    }

    fn code(bytes: &[u8]) -> Code<'_> {
        Code {
            registers: 3,
            ins: 1,
            outs: 1,
            tries: 0,
            instructions: bytes,
            try_blocks: &[],
        }
    }

    #[test]
    fn a_constant_plus_a_literal_returned_lowers_to_three_operations() {
        let bytes = units(&[0x2012, 0x00d8, 0x0300, 0x000f]);
        let lowered = lower(&code(&bytes)).unwrap();
        assert_eq!(lowered.len(), 3);
        assert_eq!(
            lowered[1].ops,
            alloc::vec![Op::Binary {
                width: Width::Int,
                operation: Operation::Add,
                to: 0,
                left: Operand::Register(0),
                right: Operand::Constant(3),
            }]
        );
        assert_eq!(
            lowered[2].ops,
            alloc::vec![Op::Return {
                value: Some((Width::Int, 0))
            }]
        );
        assert!(!lowered[1].calls());
    }

    #[test]
    fn a_virtual_invoke_passes_the_method_index_first() {
        let bytes = units(&[0x206e, 0x0004, 0x0021, 0x000e]);
        let lowered = lower(&code(&bytes)).unwrap();
        assert_eq!(
            lowered[0].ops,
            alloc::vec![Op::Call {
                entry: helper::INVOKE_VIRTUAL,
                arguments: alloc::vec![
                    Operand::Constant(4),
                    Operand::Register(1),
                    Operand::Register(2)
                ],
                result: None,
            }]
        );
        assert!(lowered[0].calls());
    }

    #[test]
    fn the_most_used_registers_come_first() {
        let counts = [3, 0, 7, 3];
        assert_eq!(most_used(&counts, 2), alloc::vec![2, 0]);
        assert_eq!(most_used(&counts, 8), alloc::vec![2, 0, 3]);
    }
}
