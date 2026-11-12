//! RV32's way into and out of user mode (D480). The program's registers live in RV64's
//! [`Registers`] layout, each 32-bit register in the low half of its 64-bit slot, so the
//! kernel's user-mode code is the same for both widths. `riscv32_enter_user` saves the kernel's
//! callee-saved registers and `tp` on its stack, keeps the stack pointer in the context and the
//! context in `sscratch`, points `stvec` at the user trap entry, loads the program's registers
//! and returns to U-mode; the user trap entry saves them back, puts the kernel's stack, `stvec`
//! and registers back, and returns from `riscv32_enter_user` into [`run_user`], which says why.

use crate::user::riscv64::Registers;
use crate::user::{UserContext, UserExit};
use crate::{Exception, ExceptionKind};

const PC: usize = 256;
const KERNEL_SP: usize = 264;
const USER_A0: usize = 80;
const KERNEL_FRAME: usize = 64;
const SUPERVISOR_PREVIOUS: usize = 1 << 8;
const PREVIOUS_INTERRUPTS: usize = 1 << 5;
const INTERRUPT: u32 = 1 << 31;
const ECALL_FROM_USER: u32 = 8;
const ECALL_SIZE: u64 = 4;
const LOW_HALF: u64 = 0xffff_ffff;

core::arch::global_asm!(
    r#"
    .section .text, "ax"
    .balign 4
    .global riscv32_enter_user
riscv32_enter_user:
    addi    sp, sp, -{frame}
    sw      ra, 0(sp)
    sw      s0, 4(sp)
    sw      s1, 8(sp)
    sw      s2, 12(sp)
    sw      s3, 16(sp)
    sw      s4, 20(sp)
    sw      s5, 24(sp)
    sw      s6, 28(sp)
    sw      s7, 32(sp)
    sw      s8, 36(sp)
    sw      s9, 40(sp)
    sw      s10, 44(sp)
    sw      s11, 48(sp)
    sw      tp, 52(sp)
    sw      sp, {kernel_sp}(a0)
    csrw    sscratch, a0
    lla     t0, riscv32_user_trap
    csrw    stvec, t0
    lw      t0, {pc}(a0)
    csrw    sepc, t0
    li      t0, {previous}
    csrc    sstatus, t0
    li      t0, {previous_interrupts}
    csrs    sstatus, t0
    lw      x1, 8(a0)
    lw      x2, 16(a0)
    lw      x3, 24(a0)
    lw      x4, 32(a0)
    lw      x5, 40(a0)
    lw      x6, 48(a0)
    lw      x7, 56(a0)
    lw      x8, 64(a0)
    lw      x9, 72(a0)
    lw      x11, 88(a0)
    lw      x12, 96(a0)
    lw      x13, 104(a0)
    lw      x14, 112(a0)
    lw      x15, 120(a0)
    lw      x16, 128(a0)
    lw      x17, 136(a0)
    lw      x18, 144(a0)
    lw      x19, 152(a0)
    lw      x20, 160(a0)
    lw      x21, 168(a0)
    lw      x22, 176(a0)
    lw      x23, 184(a0)
    lw      x24, 192(a0)
    lw      x25, 200(a0)
    lw      x26, 208(a0)
    lw      x27, 216(a0)
    lw      x28, 224(a0)
    lw      x29, 232(a0)
    lw      x30, 240(a0)
    lw      x31, 248(a0)
    lw      a0, {user_a0}(a0)
    sret

    .balign 4
riscv32_user_trap:
    csrrw   a0, sscratch, a0
    sw      x1, 8(a0)
    sw      x2, 16(a0)
    sw      x3, 24(a0)
    sw      x4, 32(a0)
    sw      x5, 40(a0)
    sw      x6, 48(a0)
    sw      x7, 56(a0)
    sw      x8, 64(a0)
    sw      x9, 72(a0)
    sw      x11, 88(a0)
    sw      x12, 96(a0)
    sw      x13, 104(a0)
    sw      x14, 112(a0)
    sw      x15, 120(a0)
    sw      x16, 128(a0)
    sw      x17, 136(a0)
    sw      x18, 144(a0)
    sw      x19, 152(a0)
    sw      x20, 160(a0)
    sw      x21, 168(a0)
    sw      x22, 176(a0)
    sw      x23, 184(a0)
    sw      x24, 192(a0)
    sw      x25, 200(a0)
    sw      x26, 208(a0)
    sw      x27, 216(a0)
    sw      x28, 224(a0)
    sw      x29, 232(a0)
    sw      x30, 240(a0)
    sw      x31, 248(a0)
    csrr    t0, sscratch
    sw      t0, {user_a0}(a0)
    csrr    t0, sepc
    sw      t0, {pc}(a0)
    lw      sp, {kernel_sp}(a0)
    lla     t0, riscv32_trap_entry
    csrw    stvec, t0
    csrw    sscratch, zero
    lw      ra, 0(sp)
    lw      s0, 4(sp)
    lw      s1, 8(sp)
    lw      s2, 12(sp)
    lw      s3, 16(sp)
    lw      s4, 20(sp)
    lw      s5, 24(sp)
    lw      s6, 28(sp)
    lw      s7, 32(sp)
    lw      s8, 36(sp)
    lw      s9, 40(sp)
    lw      s10, 44(sp)
    lw      s11, 48(sp)
    lw      tp, 52(sp)
    addi    sp, sp, {frame}
    ret
"#,
    frame = const KERNEL_FRAME,
    kernel_sp = const KERNEL_SP,
    pc = const PC,
    user_a0 = const USER_A0,
    previous = const SUPERVISOR_PREVIOUS,
    previous_interrupts = const PREVIOUS_INTERRUPTS,
);

unsafe extern "C" {
    fn riscv32_enter_user(registers: *mut Registers);
}

const _: () = assert!(core::mem::offset_of!(Registers, pc) == PC);
const _: () = assert!(core::mem::offset_of!(Registers, kernel_sp) == KERNEL_SP);

/// [`crate::Platform::run_user`] on RV32: the caller guarantees interrupts are masked and the
/// active tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked, so nothing traps between the switch of `stvec` and the
    // `sret`; the trap that ends the program saves its registers into the context, which
    // `&mut` keeps valid and unaliased, and returns here with the kernel's registers restored.
    unsafe { riscv32_enter_user(registers) };
    for register in &mut registers.x {
        *register &= LOW_HALF;
    }
    registers.pc &= LOW_HALF;
    let scause: u32;
    let stval: u32;
    // SAFETY: reads the trap CSRs, which nothing has changed since the trap from U-mode.
    unsafe {
        core::arch::asm!("csrr {}, scause", "csrr {}, stval", out(reg) scause, out(reg) stval)
    };
    if scause == ECALL_FROM_USER {
        registers.pc += ECALL_SIZE;
        return UserExit::Syscall;
    }
    if scause & INTERRUPT != 0 {
        super::interrupt(scause);
        return UserExit::Interrupt;
    }
    registers.scause = u64::from(scause);
    registers.stval = u64::from(stval);
    UserExit::Exception(decode(scause, stval, registers.pc))
}

impl Registers {
    /// [`UserContext::last_exception`] on RV32: from the `scause` and `stval` `run_user` kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.scause as u32, self.stval as u32, self.pc)
    }
}

fn decode(scause: u32, stval: u32, pc: u64) -> Exception {
    let code = scause & !INTERRUPT;
    let (kind, name, has_address) = match (scause & INTERRUPT != 0, code) {
        (true, _) => (ExceptionKind::Interrupt, "interrupt", false),
        (false, 0 | 4 | 6) => (ExceptionKind::Alignment, "misaligned address", true),
        (false, 1) => (ExceptionKind::InstructionFault, "instruction access fault", true),
        (false, 2) => (ExceptionKind::IllegalInstruction, "illegal instruction", false),
        (false, 3) => (ExceptionKind::Breakpoint, "breakpoint (ebreak)", false),
        (false, 5) => (ExceptionKind::DataFault, "load access fault", true),
        (false, 7) => (ExceptionKind::DataFault, "store access fault", true),
        (false, 12) => (ExceptionKind::InstructionFault, "instruction page fault", true),
        (false, 13) => (ExceptionKind::DataFault, "load page fault", true),
        (false, 15) => (ExceptionKind::DataFault, "store page fault", true),
        _ => (ExceptionKind::Other, "exception", false),
    };
    Exception {
        kind,
        name,
        code: u64::from(scause),
        pc,
        address: has_address.then_some(u64::from(stval)),
    }
}
