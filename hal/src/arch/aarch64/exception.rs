//! AArch64 exception vectors (Arm ARM, D1.10 "Exception entry").
//!
//! `VBAR_EL1` points at a 2 KiB-aligned table of 16 entries, 128 bytes each: four groups
//! (current EL with SP_EL0, current EL with SP_ELx, lower EL in AArch64, lower EL in AArch32)
//! of four types (synchronous, IRQ, FIQ, SError). Every entry saves the general-purpose
//! registers, `ELR_EL1` and `SPSR_EL1` on the current stack, then calls [`aarch64_exception`]
//! with the frame and the entry's index, and restores everything and `eret`s if it returns.
//! The kernel uses no FP/SIMD registers (D10), so they are not saved.

use crate::exception::{Exception, ExceptionKind, deliver};

core::arch::global_asm!(
    r#"
    .macro EXCEPTION_ENTRY index
    .balign 0x80
    sub     sp, sp, #{frame_size}
    stp     x0, x1, [sp, #0]
    mov     x1, #\index
    b       exception_common
    .endm

    .section .text.vectors, "ax"
    .balign 2048
    .global exception_vectors
exception_vectors:
    EXCEPTION_ENTRY 0
    EXCEPTION_ENTRY 1
    EXCEPTION_ENTRY 2
    EXCEPTION_ENTRY 3
    EXCEPTION_ENTRY 4
    EXCEPTION_ENTRY 5
    EXCEPTION_ENTRY 6
    EXCEPTION_ENTRY 7
    EXCEPTION_ENTRY 8
    EXCEPTION_ENTRY 9
    EXCEPTION_ENTRY 10
    EXCEPTION_ENTRY 11
    EXCEPTION_ENTRY 12
    EXCEPTION_ENTRY 13
    EXCEPTION_ENTRY 14
    EXCEPTION_ENTRY 15

exception_common:
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    mrs     x2, elr_el1
    stp     x30, x2, [sp, #240]
    mrs     x2, spsr_el1
    str     x2, [sp, #256]

    mov     x0, sp
    bl      {handler}

    ldr     x2, [sp, #256]
    msr     spsr_el1, x2
    ldp     x30, x2, [sp, #240]
    msr     elr_el1, x2
    ldp     x28, x29, [sp, #224]
    ldp     x26, x27, [sp, #208]
    ldp     x24, x25, [sp, #192]
    ldp     x22, x23, [sp, #176]
    ldp     x20, x21, [sp, #160]
    ldp     x18, x19, [sp, #144]
    ldp     x16, x17, [sp, #128]
    ldp     x14, x15, [sp, #112]
    ldp     x12, x13, [sp, #96]
    ldp     x10, x11, [sp, #80]
    ldp     x8, x9, [sp, #64]
    ldp     x6, x7, [sp, #48]
    ldp     x4, x5, [sp, #32]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp, #0]
    add     sp, sp, #{frame_size}
    eret
"#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym aarch64_exception,
);

/// The registers saved by the entry code, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// x0 to x30.
    x: [u64; 31],
    elr: u64,
    spsr: u64,
    /// Keeps the frame a multiple of 16 bytes, as the stack pointer must stay aligned.
    _pad: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);

/// Exception classes (`ESR_EL1.EC`, Arm ARM D24.2.40).
const EC_UNKNOWN: u64 = 0x00;
const EC_ILLEGAL_STATE: u64 = 0x0e;
const EC_SVC64: u64 = 0x15;
const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
const EC_INSTRUCTION_ABORT: u64 = 0x21;
const EC_PC_ALIGNMENT: u64 = 0x22;
const EC_DATA_ABORT_LOWER: u64 = 0x24;
const EC_DATA_ABORT: u64 = 0x25;
const EC_SP_ALIGNMENT: u64 = 0x26;
const EC_BRK64: u64 = 0x3c;

/// `ESR_EL1.ISS.FnV` for aborts: set when `FAR_EL1` does not hold the faulting address.
const ISS_FAR_NOT_VALID: u64 = 1 << 10;

/// Entry types within each group of four vector entries.
const TYPE_SYNCHRONOUS: u64 = 0;
const TYPE_IRQ: u64 = 1;
const TYPE_FIQ: u64 = 2;

/// Size of every A64 instruction, `brk` included.
const INSTRUCTION_SIZE: u64 = 4;

/// Called by the entry code with the saved registers and the vector entry's index (0-15).
extern "C" fn aarch64_exception(frame: &mut TrapFrame, index: u64) {
    let esr: u64;
    let far: u64;
    // SAFETY: reading the syndrome register at EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack)) };
    // SAFETY: reading the fault address register at EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack)) };

    let exception = decode(index % 4, esr, far, frame.elr);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.elr += INSTRUCTION_SIZE;
    }
}

fn decode(entry_type: u64, esr: u64, far: u64, pc: u64) -> Exception {
    let class = esr >> 26 & 0x3f;
    let (kind, name, far_valid) = match entry_type {
        TYPE_SYNCHRONOUS => {
            let (kind, name) = synchronous_kind(class);
            let far_valid = match class {
                EC_PC_ALIGNMENT => true,
                EC_INSTRUCTION_ABORT_LOWER
                | EC_INSTRUCTION_ABORT
                | EC_DATA_ABORT_LOWER
                | EC_DATA_ABORT => esr & ISS_FAR_NOT_VALID == 0,
                _ => false,
            };
            (kind, name, far_valid)
        }
        TYPE_IRQ => (ExceptionKind::Interrupt, "IRQ", false),
        TYPE_FIQ => (ExceptionKind::Interrupt, "FIQ", false),
        _ => (ExceptionKind::Other, "SError (asynchronous abort)", false),
    };
    Exception {
        kind,
        name,
        code: esr,
        pc,
        address: far_valid.then_some(far),
    }
}

fn synchronous_kind(class: u64) -> (ExceptionKind, &'static str) {
    match class {
        EC_UNKNOWN => (ExceptionKind::IllegalInstruction, "undefined instruction"),
        EC_ILLEGAL_STATE => (ExceptionKind::IllegalInstruction, "illegal execution state"),
        EC_SVC64 => (ExceptionKind::Other, "supervisor call (svc)"),
        EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => {
            (ExceptionKind::InstructionFault, "instruction abort")
        }
        EC_PC_ALIGNMENT => (ExceptionKind::Alignment, "PC alignment fault"),
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT => (ExceptionKind::DataFault, "data abort"),
        EC_SP_ALIGNMENT => (ExceptionKind::Alignment, "SP alignment fault"),
        EC_BRK64 => (ExceptionKind::Breakpoint, "breakpoint (brk)"),
        _ => (ExceptionKind::Other, "synchronous exception"),
    }
}
