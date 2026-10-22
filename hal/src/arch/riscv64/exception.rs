//! RISC-V supervisor-mode trap entry (RISC-V privileged spec, 12.1 "Supervisor CSRs").
//!
//! `stvec` points at [`exception_entry`] in direct mode, so every exception and interrupt taken
//! to S-mode arrives there. It saves the general-purpose registers, `sepc` and `sstatus` on the
//! current stack, calls [`riscv64_exception`] with the frame, and restores everything and
//! `sret`s if it returns. The boot code turns the FPU off (`sstatus.FS` = Off), so kernel code
//! that used FP registers would trap as an illegal instruction instead of silently corrupting
//! state that this entry does not save.

use super::timer;
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};

core::arch::global_asm!(
    r#"
    .pushsection .text.trap, "ax"
    .balign 4
    .global exception_entry
exception_entry:
    addi    sp, sp, -{frame_size}
    sd      x1, 8(sp)
    sd      x3, 24(sp)
    sd      x4, 32(sp)
    sd      x5, 40(sp)
    sd      x6, 48(sp)
    sd      x7, 56(sp)
    sd      x8, 64(sp)
    sd      x9, 72(sp)
    sd      x10, 80(sp)
    sd      x11, 88(sp)
    sd      x12, 96(sp)
    sd      x13, 104(sp)
    sd      x14, 112(sp)
    sd      x15, 120(sp)
    sd      x16, 128(sp)
    sd      x17, 136(sp)
    sd      x18, 144(sp)
    sd      x19, 152(sp)
    sd      x20, 160(sp)
    sd      x21, 168(sp)
    sd      x22, 176(sp)
    sd      x23, 184(sp)
    sd      x24, 192(sp)
    sd      x25, 200(sp)
    sd      x26, 208(sp)
    sd      x27, 216(sp)
    sd      x28, 224(sp)
    sd      x29, 232(sp)
    sd      x30, 240(sp)
    sd      x31, 248(sp)
    addi    t0, sp, {frame_size}
    sd      t0, 16(sp)
    csrr    t0, sepc
    sd      t0, 256(sp)
    csrr    t0, sstatus
    sd      t0, 264(sp)

    mv      a0, sp
    call    {handler}

    ld      t0, 264(sp)
    csrw    sstatus, t0
    ld      t0, 256(sp)
    csrw    sepc, t0
    ld      x1, 8(sp)
    ld      x3, 24(sp)
    ld      x4, 32(sp)
    ld      x5, 40(sp)
    ld      x6, 48(sp)
    ld      x7, 56(sp)
    ld      x8, 64(sp)
    ld      x9, 72(sp)
    ld      x10, 80(sp)
    ld      x11, 88(sp)
    ld      x12, 96(sp)
    ld      x13, 104(sp)
    ld      x14, 112(sp)
    ld      x15, 120(sp)
    ld      x16, 128(sp)
    ld      x17, 136(sp)
    ld      x18, 144(sp)
    ld      x19, 152(sp)
    ld      x20, 160(sp)
    ld      x21, 168(sp)
    ld      x22, 176(sp)
    ld      x23, 184(sp)
    ld      x24, 192(sp)
    ld      x25, 200(sp)
    ld      x26, 208(sp)
    ld      x27, 216(sp)
    ld      x28, 224(sp)
    ld      x29, 232(sp)
    ld      x30, 240(sp)
    ld      x31, 248(sp)
    addi    sp, sp, {frame_size}
    sret
    .popsection
"#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym riscv64_exception,
);

/// The registers saved by the entry code, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// x0 to x31, indexed by register number; `x[0]` is unused and `x[2]` is the `sp` at the
    /// time of the trap.
    x: [u64; 32],
    sepc: u64,
    sstatus: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);

/// `scause` bit 63: set for interrupts, clear for exceptions.
const SCAUSE_INTERRUPT: u64 = 1 << 63;
/// `scause` for the supervisor timer interrupt.
const SCAUSE_SUPERVISOR_TIMER: u64 = SCAUSE_INTERRUPT | 5;
/// `scause` for the supervisor software interrupt: the wake-up interrupt (D41).
const SCAUSE_SUPERVISOR_SOFTWARE: u64 = SCAUSE_INTERRUPT | 1;

/// Called by the entry code with the saved registers.
extern "C" fn riscv64_exception(frame: &mut TrapFrame) {
    let scause: u64;
    let stval: u64;
    // SAFETY: reading the trap cause CSR in S-mode has no side effects.
    unsafe { core::arch::asm!("csrr {}, scause", out(reg) scause, options(nomem, nostack)) };
    // SAFETY: reading the trap value CSR in S-mode has no side effects.
    unsafe { core::arch::asm!("csrr {}, stval", out(reg) stval, options(nomem, nostack)) };

    // Exceptions pay one test for the interrupts' checks.
    if scause & SCAUSE_INTERRUPT != 0 {
        match scause {
            SCAUSE_SUPERVISOR_TIMER => {
                // The interrupt stays pending while `time` >= the deadline; disarming clears
                // it.
                timer::disarm();
                crate::interrupt::deliver(Interrupt::Timer);
                return;
            }
            SCAUSE_SUPERVISOR_SOFTWARE => {
                // Cleared before the kernel looks at why it was woken, so a wake sent
                // meanwhile is taken again.
                timer::end_wakeup();
                crate::interrupt::deliver(Interrupt::Wakeup);
                return;
            }
            // Reported below.
            _ => {}
        }
    }
    let exception = decode(scause, stval, frame.sepc);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.sepc += instruction_length(frame.sepc);
    }
}

/// Length of the instruction at `pc`: 2 bytes for a compressed instruction (low two bits not
/// `11`), else 4. `ebreak` may be assembled as either.
fn instruction_length(pc: u64) -> u64 {
    // SAFETY: `pc` is the address of the instruction that just trapped (`ebreak`), so it is
    // mapped, readable and at least 2-byte aligned; reading its first parcel has no side effects.
    let parcel = unsafe { core::ptr::read_volatile(pc as usize as *const u16) };
    if parcel & 0b11 == 0b11 { 4 } else { 2 }
}

fn decode(scause: u64, stval: u64, pc: u64) -> Exception {
    let code = scause & !SCAUSE_INTERRUPT;
    let (kind, name, has_address) = if scause & SCAUSE_INTERRUPT != 0 {
        let name = match code {
            1 => "supervisor software interrupt",
            5 => "supervisor timer interrupt",
            9 => "supervisor external interrupt",
            _ => "interrupt",
        };
        (ExceptionKind::Interrupt, name, false)
    } else {
        match code {
            0 => (
                ExceptionKind::Alignment,
                "instruction address misaligned",
                true,
            ),
            1 => (
                ExceptionKind::InstructionFault,
                "instruction access fault",
                true,
            ),
            2 => (
                ExceptionKind::IllegalInstruction,
                "illegal instruction",
                false,
            ),
            3 => (ExceptionKind::Breakpoint, "breakpoint (ebreak)", false),
            4 => (ExceptionKind::Alignment, "load address misaligned", true),
            5 => (ExceptionKind::DataFault, "load access fault", true),
            6 => (ExceptionKind::Alignment, "store address misaligned", true),
            7 => (ExceptionKind::DataFault, "store access fault", true),
            8 => (ExceptionKind::Other, "environment call from U-mode", false),
            9 => (ExceptionKind::Other, "environment call from S-mode", false),
            12 => (
                ExceptionKind::InstructionFault,
                "instruction page fault",
                true,
            ),
            13 => (ExceptionKind::DataFault, "load page fault", true),
            15 => (ExceptionKind::DataFault, "store page fault", true),
            _ => (ExceptionKind::Other, "exception", false),
        }
    };
    Exception {
        kind,
        name,
        code: scause,
        pc,
        address: has_address.then_some(stval),
    }
}
