//! AArch64 exception vectors (Arm ARM, D1.10 "Exception entry").
//!
//! `VBAR_EL1` points at a 2 KiB-aligned table of 16 entries, 128 bytes each: four groups
//! (current EL with SP_EL0, current EL with SP_ELx, lower EL in AArch64, lower EL in AArch32)
//! of four types (synchronous, IRQ, FIQ, SError). Every entry from EL1 saves the general-purpose
//! registers, `ELR_EL1` and `SPSR_EL1` on the current stack, then calls [`aarch64_exception`]
//! with the frame and the entry's index, and restores everything and `eret`s if it returns.
//! Every entry from EL0 (D47) saves the program's registers, `SP_EL0` and its thread pointer
//! `TPIDR_EL0` (D57) included, into its `UserContext`, where `SP_EL1` points while it runs,
//! and returns from [`run_user`] instead. The kernel uses no FP/SIMD registers (D10), so they
//! are not saved, and programs may not use them either: `CPACR_EL1` traps them at EL0 (D49).

use super::timer;
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::user::{self, TestProgram, UserContext, UserExit};

core::arch::global_asm!(
    r#"
    .macro EXCEPTION_ENTRY index
    .balign 0x80
    sub     sp, sp, #{frame_size}
    stp     x0, x1, [sp, #0]
    mov     x1, #\index
    b       exception_common
    .endm

    // From EL0 (D47): `SP_EL1` points just past the saved part of the running program's
    // `UserContext`, so the registers go straight into it.
    .macro USER_ENTRY index
    .balign 0x80
    sub     sp, sp, #{user_saved_size}
    stp     x0, x1, [sp, #0]
    mov     x1, #\index
    b       user_exception_common
    .endm

    .pushsection .text.vectors, "ax"
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
    USER_ENTRY 8
    USER_ENTRY 9
    USER_ENTRY 10
    USER_ENTRY 11
    USER_ENTRY 12
    USER_ENTRY 13
    USER_ENTRY 14
    USER_ENTRY 15

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
    dsb     nsh                     // never reached: as after the user-mode `eret`
    isb

    // The rest of the way out of EL0: save the program's registers in its `UserContext`
    // (`sp`), its thread pointer (`TPIDR_EL0`) included, record the vector entry, then return
    // from `aarch64_enter_user` on the kernel stack it left.
user_exception_common:
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
    mrs     x3, sp_el0
    stp     x2, x3, [sp, #256]
    mrs     x2, tpidr_el0
    str     x2, [sp, #272]
    ldr     x2, [sp, #{user_saved_size}]
    str     x1, [sp, #{user_saved_size} + 8]
    mov     sp, x2
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     x19, x20, [sp], #96
    ret

    // `aarch64_enter_user(context)`: keeps the kernel's callee-saved registers on its stack and
    // the stack pointer in the context, points `SP_EL1` just past the context's saved part and
    // enters EL0 with the program's registers, `TPIDR_EL0` included. Returns through
    // `user_exception_common`.
    .global aarch64_enter_user
aarch64_enter_user:
    stp     x19, x20, [sp, #-96]!
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    mov     x1, sp
    str     x1, [x0, #{user_saved_size}]
    ldp     x2, x3, [x0, #248]
    msr     elr_el1, x2
    msr     spsr_el1, x3
    ldp     x2, x3, [x0, #264]
    msr     sp_el0, x2
    msr     tpidr_el0, x3
    add     sp, x0, #{user_saved_size}
    ldp     x2, x3, [x0, #16]
    ldp     x4, x5, [x0, #32]
    ldp     x6, x7, [x0, #48]
    ldp     x8, x9, [x0, #64]
    ldp     x10, x11, [x0, #80]
    ldp     x12, x13, [x0, #96]
    ldp     x14, x15, [x0, #112]
    ldp     x16, x17, [x0, #128]
    ldp     x18, x19, [x0, #144]
    ldp     x20, x21, [x0, #160]
    ldp     x22, x23, [x0, #176]
    ldp     x24, x25, [x0, #192]
    ldp     x26, x27, [x0, #208]
    ldp     x28, x29, [x0, #224]
    ldr     x30, [x0, #240]
    ldp     x0, x1, [x0, #0]
    eret
    // Never reached: no speculation past the `eret` (straight-line speculation).
    dsb     nsh
    isb
    .popsection

    // The user-mode test program (D47, `TestProgram`): position-independent, no stack, no
    // data. Copied to a user page, never run where it is linked.
    .pushsection .rodata.user_test, "a"
    .balign 4
    .global aarch64_user_test_start
aarch64_user_test_start:
    .global aarch64_user_test_checks
aarch64_user_test_checks:
    mov     x0, #1
    mov     x1, #2
    mov     x2, #3
    mov     x3, #4
    mov     x4, #5
    mov     x5, #6
    .irp n, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
    mov     x\n, #(0x100 + \n)
    .endr
    mov     x8, #{syscall_first}
    svc     #0
    // Every register but x0 must be as it was: x1 collects the differences.
    sub     x1, x1, #2
    .irp n, 2, 3, 4, 5
    sub     x\n, x\n, #(\n + 1)
    orr     x1, x1, x\n
    .endr
    sub     x8, x8, #{syscall_first}
    orr     x1, x1, x8
    .irp n, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
    sub     x\n, x\n, #(0x100 + \n)
    orr     x1, x1, x\n
    .endr
    mov     x8, #{syscall_report}
    svc     #0
    ldr     x1, [x0]
1:  b       1b
    .global aarch64_user_test_floating_point
aarch64_user_test_floating_point:
    .inst   0x9e6703e0              // fmov d0, xzr, spelled out: the target has no FP
    .global aarch64_user_test_null_syscalls
aarch64_user_test_null_syscalls:
    mov     x8, #{syscall_null}
2:  svc     #0
    b       2b
    .global aarch64_user_test_spin
aarch64_user_test_spin:
3:  b       3b
    .global aarch64_user_test_linux_hello
aarch64_user_test_linux_hello:
    // Linux's generic numbers: write 64, exit_group 94. x19 collects the differences.
    mov     x0, #1
    adr     x1, 9f
    mov     x2, #{hello_length}
    mov     x8, #64
    svc     #0
    sub     x19, x0, #{hello_length}
    mov     x0, #1
    mov     x1, #0
    mov     x2, #1
    svc     #0
    add     x0, x0, #14             // -EFAULT
    orr     x19, x19, x0
    mov     x8, #{unknown_syscall}
    svc     #0
    add     x0, x0, #38             // -ENOSYS
    orr     x19, x19, x0
    cmp     x19, #0
    cset    x0, ne
    mov     x8, #94
    svc     #0
4:  b       4b
9:  .ascii  "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global aarch64_user_test_linux_start
aarch64_user_test_linux_start:
    // Linux's generic numbers: writev 66, exit_group 94, getpid 172, gettid 178, prlimit64
    // 261, getrandom 278. x19 is argc, x20 walks argv and envp, x21 counts the two lists, x22 holds the
    // process id. Each string is written by writev from one iovec on the stack. Ends with
    // exit_group(255) if the stack is misaligned, the thread's id is not the process's, or
    // getrandom does not fill the 16 bytes below the stack pointer with something not zero,
    // or prlimit64 does not report the stack's limits as 8 MiB soft and unlimited hard.
    mov     x9, sp
    mov     x0, #255
    tst     x9, #15
    b.ne    9f
    mov     x8, #172
    svc     #0
    mov     x22, x0
    mov     x8, #178
    svc     #0
    cmp     x0, x22
    mov     x0, #255
    b.ne    9f
    sub     x0, sp, #16
    mov     x1, #16
    mov     x2, #0
    mov     x8, #278
    svc     #0
    cmp     x0, #16
    mov     x0, #255
    b.ne    9f
    ldp     x9, x10, [sp, #-16]
    orr     x9, x9, x10
    cbz     x9, 9f
    mov     x0, #0
    mov     x1, #3                  // RLIMIT_STACK
    mov     x2, #0
    sub     x3, sp, #32
    mov     x8, #261
    svc     #0
    mov     x9, x0
    mov     x0, #255
    cbnz    x9, 9f
    ldp     x9, x10, [sp, #-32]
    mov     x11, #0x800000
    cmp     x9, x11
    b.ne    9f
    cmn     x10, #1
    b.ne    9f
    ldr     x19, [sp]
    add     x20, sp, #8
    mov     x21, #2
6:  ldr     x1, [x20], #8
    cbz     x1, 8f
    mov     x2, #0
7:  ldrb    w3, [x1, x2]
    cbz     w3, 5f
    add     x2, x2, #1
    b       7b
5:  stp     x1, x2, [sp, #-16]!
    mov     x0, #1
    mov     x1, sp
    mov     x2, #1
    mov     x8, #66
    svc     #0
    add     sp, sp, #16
    b       6b
8:  subs    x21, x21, #1
    b.ne    6b
    mov     x0, x19
9:  mov     x8, #94
    svc     #0
4:  b       4b
    .global aarch64_user_test_linux_unknown_syscalls
aarch64_user_test_linux_unknown_syscalls:
    mov     x8, #{unknown_syscall}
5:  svc     #0
    b       5b
    .global aarch64_user_test_stack_growth
aarch64_user_test_stack_growth:
    // From the stack pointer's page down, a page at a time: each page's address in its
    // first word, until the stack runs out.
    mov     x9, sp
    and     x9, x9, #~0xfff
6:  str     x9, [x9]
    sub     x9, x9, #0x1000
    b       6b
    .global aarch64_user_test_linux_thread_pointer
aarch64_user_test_linux_thread_pointer:
    // Linux's generic numbers: getpid 172, sched_yield 124, exit_group 94. The thread pointer
    // starts at zero; then getpid() << 12, set with `msr`, must outlast every sched_yield.
    // x19 collects the differences.
    mrs     x19, tpidr_el0
    mov     x8, #172
    svc     #0
    lsl     x20, x0, #12
    msr     tpidr_el0, x20
    mov     x21, #{thread_pointer_rounds}
7:  mov     x8, #124
    svc     #0
    orr     x19, x19, x0
    mrs     x0, tpidr_el0
    eor     x0, x0, x20
    orr     x19, x19, x0
    subs    x21, x21, #1
    b.ne    7b
    cmp     x19, #0
    cset    x0, ne
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_end
aarch64_user_test_end:
    .popsection
"#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    user_saved_size = const user::aarch64::SAVED_SIZE,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
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
const EC_FP_ACCESS: u64 = 0x07;
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
/// `ESR_EL1.ISS.WnR` for data aborts: the access wrote memory.
const ISS_WRITE_NOT_READ: u64 = 1 << 6;
/// `ESR_EL1.ISS.CM` for data aborts: a cache maintenance instruction faulted, which reports
/// `WnR` set though it writes nothing.
const ISS_CACHE_MAINTENANCE: u64 = 1 << 8;

/// Entry types within each group of four vector entries.
const TYPE_SYNCHRONOUS: u64 = 0;
const TYPE_IRQ: u64 = 1;
const TYPE_FIQ: u64 = 2;

/// Size of every A64 instruction, `brk` included.
const INSTRUCTION_SIZE: u64 = 4;

/// Called by the entry code with the saved registers and the vector entry's index (0-15).
extern "C" fn aarch64_exception(frame: &mut TrapFrame, index: u64) {
    if index % 4 == TYPE_IRQ {
        irq(frame.elr);
        return;
    }
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

unsafe extern "C" {
    /// Enters EL0 with `context`'s registers; returns once an exception from EL0 has saved them
    /// back, with the vector entry in `context.vector`.
    fn aarch64_enter_user(context: *mut user::aarch64::Registers);
    static aarch64_user_test_start: u8;
    static aarch64_user_test_checks: u8;
    static aarch64_user_test_floating_point: u8;
    static aarch64_user_test_null_syscalls: u8;
    static aarch64_user_test_spin: u8;
    static aarch64_user_test_linux_hello: u8;
    static aarch64_user_test_linux_start: u8;
    static aarch64_user_test_linux_unknown_syscalls: u8;
    static aarch64_user_test_stack_growth: u8;
    static aarch64_user_test_linux_thread_pointer: u8;
    static aarch64_user_test_end: u8;
}

/// `SPSR_EL1`'s condition flags (N, Z, C, V): all a program may keep of it. The rest is zero:
/// EL0 with `SP_EL0` (EL0t), AArch64, and debug, SError, IRQ and FIQ unmasked.
const SPSR_FLAGS: u64 = 0xf << 28;

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    registers.spsr &= SPSR_FLAGS;
    // SAFETY: interrupts are masked (the caller's duty), so nothing runs between the switch
    // of `SP_EL1` to the context and the `eret`; the exception that ends the program saves its
    // registers into the context, which `&mut` keeps valid and unaliased, and returns here on
    // the kernel stack with the callee-saved registers restored. `SPSR_EL1` is sanitized above,
    // so the program runs at EL0, where it reaches only user mappings and cannot change
    // `VBAR_EL1`, `SP_EL1` or the translation tables.
    unsafe { aarch64_enter_user(registers) };
    let index = registers.vector;
    if index % 4 == TYPE_IRQ {
        irq(registers.elr);
        return UserExit::Interrupt;
    }
    let esr: u64;
    let far: u64;
    // SAFETY: as in `aarch64_exception`: no side effects. Nothing since the exception from EL0
    // has taken another (interrupts are masked and the kernel code above cannot fault).
    unsafe { core::arch::asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack)) };
    // SAFETY: as above.
    unsafe { core::arch::asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack)) };
    if index % 4 == TYPE_SYNCHRONOUS && esr >> 26 & 0x3f == EC_SVC64 {
        // `ELR_EL1` already points after the `svc`.
        return UserExit::Syscall;
    }
    UserExit::Exception(decode(index % 4, esr, far, registers.elr))
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const aarch64_user_test_start) as usize;
    let end = (&raw const aarch64_user_test_end) as usize;
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, which is part of the
    // kernel image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: (&raw const aarch64_user_test_checks) as usize - start,
        floating_point: (&raw const aarch64_user_test_floating_point) as usize - start,
        null_syscalls: (&raw const aarch64_user_test_null_syscalls) as usize - start,
        spin: (&raw const aarch64_user_test_spin) as usize - start,
        linux_hello: (&raw const aarch64_user_test_linux_hello) as usize - start,
        linux_start: (&raw const aarch64_user_test_linux_start) as usize - start,
        linux_unknown_syscalls: (&raw const aarch64_user_test_linux_unknown_syscalls) as usize
            - start,
        stack_growth: (&raw const aarch64_user_test_stack_growth) as usize - start,
        linux_thread_pointer: (&raw const aarch64_user_test_linux_thread_pointer) as usize - start,
    }
}

/// Takes the pending interrupt from the GIC and handles it.
// Inlined into both callers, as when the kernel's vectors were its only one: the kernel's
// trap path keeps its length (D47).
#[inline(always)]
fn irq(pc: u64) {
    let Some(interrupt) = timer::acknowledge() else {
        return; // Spurious: the interrupt went away before it was acknowledged.
    };
    // End the interrupt before handing it on: the kernel may not return here soon (later, a
    // context switch), and `PSTATE.I` keeps this CPU from taking another one meanwhile.
    let id = interrupt.id();
    let timer_fired = id == timer::VIRTUAL_TIMER_INTERRUPT && timer::expired();
    if timer_fired {
        timer::disarm();
    }
    timer::end_of_interrupt(interrupt);
    match id {
        // A stale interrupt for a deadline since moved later is dropped: never early.
        timer::VIRTUAL_TIMER_INTERRUPT => {
            if timer_fired {
                crate::interrupt::deliver(Interrupt::Timer);
            }
        }
        timer::WAKEUP_SGI => crate::interrupt::deliver(Interrupt::Wakeup),
        _ => deliver(&Exception {
            kind: ExceptionKind::Interrupt,
            name: "unexpected IRQ",
            code: id.into(),
            pc,
            address: None,
        }),
    }
}

// Inlined into both callers, as when the kernel's vectors were its only one: the kernel's
// trap path keeps its length (D47).
#[inline(always)]
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

/// [`crate::Platform::page_fault`]: an instruction or data abort whose fault status code is a
/// translation fault (`0b0001LL`, no valid entry at level `LL`), an access flag fault
/// (`0b0010LL`) or a permission fault (`0b0011LL`); alignment faults, external aborts, TLB
/// conflicts and unsupported atomics are not page faults. A data abort writes if `WnR` is set,
/// unless a cache maintenance instruction faulted (it reports `WnR` though it writes nothing).
/// AArch64 never caches an invalid entry, but a translation fault on an entry another CPU
/// has just made valid can still race with it, so only permission faults are never stale.
pub(super) fn page_fault(esr: u64) -> Option<crate::paging::PageFault> {
    use crate::paging::{Access, PageFault};
    /// Fault status codes: translation faults are `0b0001LL`, access flag faults `0b0010LL`,
    /// permission faults `0b0011LL`.
    const TRANSLATION_FAULT: u64 = 0b00_0100;
    const PERMISSION_FAULT: u64 = 0b00_1100;
    let status = esr & 0x3f;
    if !(TRANSLATION_FAULT..=PERMISSION_FAULT | 0b11).contains(&status) {
        return None;
    }
    let access = match esr >> 26 & 0x3f {
        EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => Access::Execute,
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT
            if esr & (ISS_WRITE_NOT_READ | ISS_CACHE_MAINTENANCE) == ISS_WRITE_NOT_READ =>
        {
            Access::Write
        }
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT => Access::Read,
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale: status < PERMISSION_FAULT,
    })
}

fn synchronous_kind(class: u64) -> (ExceptionKind, &'static str) {
    match class {
        EC_UNKNOWN => (ExceptionKind::IllegalInstruction, "undefined instruction"),
        EC_FP_ACCESS => (ExceptionKind::IllegalInstruction, "SIMD/FP access trap"),
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
