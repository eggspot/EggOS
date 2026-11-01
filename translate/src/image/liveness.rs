use alloc::vec;
use alloc::vec::Vec;

use super::Discovery;
use super::discover::direct_target;
use crate::codegen::flags::{self, Flags};
use crate::codegen::{Block, BlockEnd};
use crate::instruction::{Address, Gpr, Instruction, Memory, Mnemonic, Operand, Register};

struct Edges {
    successors: Vec<u64>,
    unknown: Flags,
    returns: bool,
    return_site: Option<u64>,
}

/// For each block of `discovery` (in order), the flags a successor may read when it exits,
/// to pass to [`crate::codegen::translate_block`] as `live_out`: the union of the successors'
/// live-in flags, found by iterating to a fixed point over the block graph.
///
/// Where the successor is unknown: an indirect `jmp` that is not a jump table and a trap or
/// refused site keep every flag live; a tail call (an epilogue, then an indirect `jmp`) and an
/// indirect call keep none, since neither x86-64 calling convention passes flags; a `ret` keeps
/// none unless some return site reads a flag its callee left, and then every `ret` keeps all
/// (decided by research, D191).
pub fn live_outs(discovery: &Discovery) -> Vec<Flags> {
    let edges: Vec<Edges> = discovery
        .blocks
        .iter()
        .map(|block| edges(block, discovery))
        .collect();
    let (live_in, live_out) = solve(discovery, &edges, Flags::NONE);
    let return_site_reads = edges
        .iter()
        .filter_map(|edge| edge.return_site)
        .filter_map(|site| discovery.block_index(site))
        .any(|index| !live_in[index].is_empty());
    if return_site_reads {
        solve(discovery, &edges, Flags::ALL).1
    } else {
        live_out
    }
}

fn edges(block: &Block, discovery: &Discovery) -> Edges {
    let mut edges = Edges {
        successors: Vec::new(),
        unknown: Flags::NONE,
        returns: false,
        return_site: None,
    };
    match block.end {
        BlockEnd::FallThrough(next) => edges.successors.push(next),
        BlockEnd::Refused { .. } => edges.unknown = Flags::ALL,
        BlockEnd::Terminator => match block.instructions.split_last() {
            None => edges.unknown = Flags::ALL,
            Some((last, before)) => {
                let next = last.end();
                let target = direct_target(last);
                match last.mnemonic {
                    Mnemonic::Jmp => match (target, discovery.jump_table(last.address)) {
                        (Some(target), _) => edges.successors.push(target),
                        (None, Some(table)) => edges.successors.extend_from_slice(&table.targets),
                        (None, None) if is_tail_call(last, before) => {}
                        (None, None) => edges.unknown = Flags::ALL,
                    },
                    Mnemonic::Jcc(_)
                    | Mnemonic::Loop
                    | Mnemonic::Loope
                    | Mnemonic::Loopne
                    | Mnemonic::Jrcxz => {
                        edges.successors.extend(target);
                        edges.successors.push(next);
                    }
                    Mnemonic::Call => {
                        edges.successors.extend(target);
                        edges.successors.push(next);
                        edges.return_site = Some(next);
                    }
                    Mnemonic::Ret => edges.returns = true,
                    _ => edges.unknown = Flags::ALL,
                }
            }
        },
    }
    edges
}

fn is_tail_call(jump: &Instruction, before: &[Instruction]) -> bool {
    if before.is_empty() {
        return matches!(
            jump.operands().first(),
            Some(Operand::Memory(Memory {
                address: Address::Rip { .. },
                ..
            }))
        );
    }
    before.iter().all(is_epilogue)
}

fn is_epilogue(instruction: &Instruction) -> bool {
    let operands = instruction.operands();
    let writes_stack = matches!(
        operands.first(),
        Some(Operand::Register(Register::General { gpr: Gpr::RSP, .. }))
    );
    match instruction.mnemonic {
        Mnemonic::Pop | Mnemonic::Leave => true,
        Mnemonic::Add | Mnemonic::Lea => writes_stack,
        Mnemonic::Mov => {
            writes_stack
                && matches!(
                    operands.get(1),
                    Some(Operand::Register(Register::General { gpr: Gpr::RBP, .. }))
                )
        }
        _ => false,
    }
}

fn live_in(instructions: &[Instruction], live_out: Flags) -> Flags {
    instructions
        .iter()
        .rev()
        .fold(live_out, |live, instruction| {
            let effect = flags::effect(instruction);
            live.without(effect.kills).union(effect.reads)
        })
}

fn solve(discovery: &Discovery, edges: &[Edges], at_return: Flags) -> (Vec<Flags>, Vec<Flags>) {
    let count = discovery.blocks.len();
    let mut live_ins = vec![Flags::NONE; count];
    let mut live_outs = vec![Flags::NONE; count];
    let mut changed = true;
    while changed {
        changed = false;
        for index in (0..count).rev() {
            let edge = &edges[index];
            let mut out = edge.unknown;
            if edge.returns {
                out = out.union(at_return);
            }
            for &successor in &edge.successors {
                let successor_in = discovery
                    .block_index(successor)
                    .map_or(Flags::ALL, |successor| live_ins[successor]);
                out = out.union(successor_in);
            }
            live_outs[index] = out;
            let input = live_in(&discovery.blocks[index].instructions, out);
            if input != live_ins[index] {
                live_ins[index] = input;
                changed = true;
            }
        }
    }
    (live_ins, live_outs)
}
