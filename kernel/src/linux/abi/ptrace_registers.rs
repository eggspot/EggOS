//! The register layouts `ptrace` reads and writes (D561): x86_64's `user_regs_struct`,
//! AArch64's `user_pt_regs` and RISC-V's `user_regs_struct`, the `NT_PRSTATUS` register set.

use eggos_hal::user::{UserContext, UserRegisters};

#[cfg(target_arch = "x86_64")]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 27 * 8;
    pub const SINGLE_STEPS: bool = true;
    const TRAP_FLAG: u64 = 1 << 8;

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
    pub const SINGLE_STEPS: bool = false;

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

    pub fn step(_registers: &mut UserRegisters, _on: bool) {}
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
mod layout {
    use super::UserRegisters;
    pub const BYTES: usize = 32 * super::super::WORD_BYTES;
    pub const SINGLE_STEPS: bool = false;

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
