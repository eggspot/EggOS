//! The register layouts `ptrace` reads and writes (D561): x86_64's `user_regs_struct`,
//! AArch64's `user_pt_regs` and RISC-V's `user_regs_struct`, the `NT_PRSTATUS` register set.

use eggos_hal::user::{UserContext, UserRegisters};

#[cfg(target_arch = "x86_64")]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 27 * 8;
    pub const SINGLE_STEPS: bool = true;
    pub const STEPS_IN_SOFTWARE: bool = false;
    const TRAP_FLAG: u64 = 1 << 8;

    pub fn next_instructions(
        _registers: &UserRegisters,
        _read: &mut dyn FnMut(u64) -> Option<u32>,
    ) -> [Option<u64>; 2] {
        [None, None]
    }

    pub fn get(registers: &UserRegisters) -> [u64; 27] {
        let r = registers;
        [
            r.r15, r.r14, r.r13, r.r12, r.rbp, r.rbx, r.r11, r.r10, r.r9, r.r8, r.rax, r.rcx,
            r.rdx, r.rsi, r.rdi, r.rax, r.rip, u64::from(r.cs), r.rflags, r.rsp, u64::from(r.ss),
            0, 0, 0, 0, 0, 0,
        ]
    }

    pub fn set(registers: &mut UserRegisters, words: &[u64]) {
        let r = registers;
        [
            r.r15, r.r14, r.r13, r.r12, r.rbp, r.rbx, r.r11, r.r10, r.r9, r.r8, r.rax, r.rcx,
            r.rdx, r.rsi, r.rdi,
        ] = core::array::from_fn(|index| words[index]);
        r.rip = words[16];
        r.rflags = words[18];
        r.rsp = words[19];
    }

    pub fn step(registers: &mut UserRegisters, on: bool) {
        match on {
            true => registers.rflags |= TRAP_FLAG,
            false => registers.rflags &= !TRAP_FLAG,
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 34 * 8;
    pub const SINGLE_STEPS: bool = true;
    pub const STEPS_IN_SOFTWARE: bool = false;
    const SOFTWARE_STEP: u64 = 1 << 21;

    pub fn next_instructions(
        _registers: &UserRegisters,
        _read: &mut dyn FnMut(u64) -> Option<u32>,
    ) -> [Option<u64>; 2] {
        [None, None]
    }

    pub fn get(registers: &UserRegisters) -> [u64; 34] {
        core::array::from_fn(|index| match index {
            0..31 => registers.x[index],
            31 => registers.sp,
            32 => registers.pc,
            _ => registers.pstate,
        })
    }

    pub fn set(registers: &mut UserRegisters, words: &[u64]) {
        registers.x.copy_from_slice(&words[..31]);
        registers.sp = words[31];
        registers.pc = words[32];
    }

    pub fn step(registers: &mut UserRegisters, on: bool) {
        match on {
            true => registers.pstate |= SOFTWARE_STEP,
            false => registers.pstate &= !SOFTWARE_STEP,
        }
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 32 * super::super::WORD_BYTES;
    pub const SINGLE_STEPS: bool = true;
    pub const STEPS_IN_SOFTWARE: bool = true;
    const OPCODE_JAL: u32 = 0x6f;
    const OPCODE_JALR: u32 = 0x67;
    const OPCODE_BRANCH: u32 = 0x63;

    fn sign_extend(value: u32, bits: u32) -> i64 {
        let shift = 32 - bits;
        i64::from(((value << shift) as i32) >> shift)
    }

    fn register(registers: &UserRegisters, index: u32) -> u64 {
        match index {
            0 => 0,
            index => registers.x[index as usize],
        }
    }

    /// Where the instruction at `registers.pc` may go next (D598): the next instruction, and a
    /// jump's or a taken branch's target, read through `read` (a 32-bit fetch at an address).
    pub fn next_instructions(
        registers: &UserRegisters,
        read: &mut dyn FnMut(u64) -> Option<u32>,
    ) -> [Option<u64>; 2] {
        let pc = registers.pc;
        let Some(word) = read(pc) else {
            return [None, None];
        };
        if word & 0b11 != 0b11 {
            return compressed_next(registers, pc, word & 0xffff);
        }
        let following = pc.wrapping_add(4);
        let rs1 = word >> 15 & 0x1f;
        match word & 0x7f {
            OPCODE_JAL => {
                let offset = (word >> 31 & 1) << 20
                    | (word >> 12 & 0xff) << 12
                    | (word >> 20 & 1) << 11
                    | (word >> 21 & 0x3ff) << 1;
                [Some(pc.wrapping_add(sign_extend(offset, 21) as u64)), None]
            }
            OPCODE_JALR => {
                let target = register(registers, rs1).wrapping_add(sign_extend(word >> 20, 12) as u64);
                [Some(target & !1), None]
            }
            OPCODE_BRANCH => {
                let offset = (word >> 31 & 1) << 12
                    | (word >> 7 & 1) << 11
                    | (word >> 25 & 0x3f) << 5
                    | (word >> 8 & 0xf) << 1;
                let target = pc.wrapping_add(sign_extend(offset, 13) as u64);
                [Some(following), Some(target)]
            }
            _ => [Some(following), None],
        }
    }

    fn compressed_next(registers: &UserRegisters, pc: u64, half: u32) -> [Option<u64>; 2] {
        let following = pc.wrapping_add(2);
        let quadrant = half & 0b11;
        let function = half >> 13;
        let jump_offset = || {
            let offset = (half >> 12 & 1) << 11
                | (half >> 11 & 1) << 4
                | (half >> 9 & 3) << 8
                | (half >> 8 & 1) << 10
                | (half >> 7 & 1) << 6
                | (half >> 6 & 1) << 7
                | (half >> 3 & 7) << 1
                | (half >> 2 & 1) << 5;
            pc.wrapping_add(sign_extend(offset, 12) as u64)
        };
        match (quadrant, function) {
            (0b01, 0b101) => [Some(jump_offset()), None],
            (0b01, 0b110 | 0b111) => {
                let offset = (half >> 12 & 1) << 8
                    | (half >> 10 & 3) << 3
                    | (half >> 5 & 3) << 6
                    | (half >> 3 & 3) << 1
                    | (half >> 2 & 1) << 5;
                let target = pc.wrapping_add(sign_extend(offset, 9) as u64);
                [Some(following), Some(target)]
            }
            (0b10, 0b100) if half >> 2 & 0x1f == 0 && half >> 7 & 0x1f != 0 => {
                [Some(register(registers, half >> 7 & 0x1f) & !1), None]
            }
            _ => [Some(following), None],
        }
    }

    pub fn get(registers: &UserRegisters) -> [u64; 32] {
        core::array::from_fn(|index| match index {
            0 => registers.pc,
            _ => registers.x[index],
        })
    }

    pub fn set(registers: &mut UserRegisters, words: &[u64]) {
        registers.pc = words[0];
        registers.x[1..].copy_from_slice(&words[1..32]);
    }

    pub fn step(_registers: &mut UserRegisters, _on: bool) {}
}

#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "riscv32"
)))]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 0;
    pub const SINGLE_STEPS: bool = false;
    pub const STEPS_IN_SOFTWARE: bool = false;

    pub fn next_instructions(
        _registers: &UserRegisters,
        _read: &mut dyn FnMut(u64) -> Option<u32>,
    ) -> [Option<u64>; 2] {
        [None, None]
    }

    pub fn get(_registers: &UserRegisters) -> [u64; 0] {
        []
    }

    pub fn set(_registers: &mut UserRegisters, _words: &[u64]) {}

    pub fn step(_registers: &mut UserRegisters, _on: bool) {}
}

/// Bytes of the register set.
pub const PTRACE_REGISTERS_BYTES: usize = layout::BYTES;
/// Whether `PTRACE_SINGLESTEP` works here.
pub const SINGLE_STEPS: bool = layout::SINGLE_STEPS;
/// Whether a single step is made with breakpoints at the next instructions, as RISC-V has no
/// user-mode step of its own (D598).
pub const STEPS_IN_SOFTWARE: bool = layout::STEPS_IN_SOFTWARE;

/// Where the instruction at the stopped program counter may go next, for a step made in
/// software (D598).
pub fn next_instructions(
    registers: &UserRegisters,
    read: &mut dyn FnMut(u64) -> Option<u32>,
) -> [Option<u64>; 2] {
    layout::next_instructions(registers, read)
}

/// The breakpoint a step made in software writes at each next instruction: `c.ebreak`, which
/// fits any instruction's first two bytes.
pub const SOFTWARE_STEP_BREAKPOINT: [u8; 2] = 0x9002u16.to_le_bytes();

/// `registers` as the ABI lays them out for `ptrace`, or `None` where it is not laid out.
pub fn ptrace_registers(registers: &UserRegisters) -> Option<alloc::vec::Vec<u8>> {
    if PTRACE_REGISTERS_BYTES == 0 {
        return None;
    }
    let words = layout::get(registers);
    let width = super::WORD_BYTES;
    let mut bytes = alloc::vec::Vec::with_capacity(words.len() * width);
    for word in words {
        bytes.extend_from_slice(&word.to_ne_bytes()[..width]);
    }
    Some(bytes)
}

/// Sets `registers` from `bytes` laid out for `ptrace`; `None` where it is not laid out.
pub fn set_ptrace_registers(registers: &mut UserRegisters, bytes: &[u8]) -> Option<()> {
    if PTRACE_REGISTERS_BYTES == 0 || bytes.len() < PTRACE_REGISTERS_BYTES {
        return None;
    }
    let width = super::WORD_BYTES;
    let words: alloc::vec::Vec<u64> = bytes[..PTRACE_REGISTERS_BYTES]
        .chunks_exact(width)
        .map(super::word)
        .collect();
    layout::set(registers, &words);
    Some(())
}

/// Turns single-stepping on or off for the program `context` runs, where it can be.
pub fn set_single_step(context: &mut UserContext, on: bool) {
    let mut registers = context.registers();
    layout::step(&mut registers, on);
    context.set_registers(&registers);
}
