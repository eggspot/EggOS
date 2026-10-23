//! RISC-V supervisor-mode trap entry (RISC-V privileged spec, 12.1 "Supervisor CSRs").
//!
//! `stvec` points at [`exception_entry`] in direct mode, so every exception and interrupt taken
//! to S-mode from the kernel arrives there; while a program runs in U-mode it points at the user
//! trap entry instead, which saves the program's registers into its `UserContext` and returns
//! from [`run_user`] (D47). `exception_entry` saves the general-purpose registers, `sepc` and `sstatus` on the
//! current stack, calls [`riscv64_exception`] with the frame, and restores everything and
//! `sret`s if it returns. The boot code turns the FPU and vector unit off (`sstatus.FS` and `VS`
//! = Off), so kernel code or a program that used their registers would trap as an illegal
//! instruction instead of silently corrupting or leaking state that this entry does not save
//! (D49).

use super::timer;
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::user::{self, TestProgram, UserContext, UserExit};

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

    // `stvec` while a program runs in U-mode (D47): every trap from it lands here, so traps
    // from the kernel pay nothing for user mode. `sscratch` holds the program's `UserContext`.
    // Saves the program's registers there, puts `stvec` back and returns from
    // `riscv64_enter_user` on the kernel stack it left, with the kernel's `tp`. `gp` stays the
    // program's: the kernel never uses it (the image defines no `__global_pointer$`, so the
    // linker relaxes nothing against it).
    .balign 4
riscv64_user_trap_entry:
    csrrw   x31, sscratch, x31
    sd      x1, 8(x31)
    sd      x2, 16(x31)
    sd      x3, 24(x31)
    sd      x4, 32(x31)
    sd      x5, 40(x31)
    sd      x6, 48(x31)
    sd      x7, 56(x31)
    sd      x8, 64(x31)
    sd      x9, 72(x31)
    sd      x10, 80(x31)
    sd      x11, 88(x31)
    sd      x12, 96(x31)
    sd      x13, 104(x31)
    sd      x14, 112(x31)
    sd      x15, 120(x31)
    sd      x16, 128(x31)
    sd      x17, 136(x31)
    sd      x18, 144(x31)
    sd      x19, 152(x31)
    sd      x20, 160(x31)
    sd      x21, 168(x31)
    sd      x22, 176(x31)
    sd      x23, 184(x31)
    sd      x24, 192(x31)
    sd      x25, 200(x31)
    sd      x26, 208(x31)
    sd      x27, 216(x31)
    sd      x28, 224(x31)
    sd      x29, 232(x31)
    sd      x30, 240(x31)
    csrr    t0, sscratch
    sd      t0, 248(x31)
    csrr    t0, sepc
    sd      t0, 256(x31)
    la      t0, exception_entry
    csrw    stvec, t0
    ld      sp, 264(x31)
    ld      ra, 0(sp)
    ld      s0, 8(sp)
    ld      s1, 16(sp)
    ld      s2, 24(sp)
    ld      s3, 32(sp)
    ld      s4, 40(sp)
    ld      s5, 48(sp)
    ld      s6, 56(sp)
    ld      s7, 64(sp)
    ld      s8, 72(sp)
    ld      s9, 80(sp)
    ld      s10, 88(sp)
    ld      s11, 96(sp)
    ld      tp, 104(sp)
    addi    sp, sp, 112
    ret

    // `riscv64_enter_user(context)`: keeps the kernel's callee-saved registers and `tp` on its
    // stack and the stack pointer in the context, points `stvec` at the user trap entry and
    // `sscratch` at the context, and `sret`s to U-mode with the program's registers.
    .global riscv64_enter_user
riscv64_enter_user:
    addi    sp, sp, -112
    sd      ra, 0(sp)
    sd      s0, 8(sp)
    sd      s1, 16(sp)
    sd      s2, 24(sp)
    sd      s3, 32(sp)
    sd      s4, 40(sp)
    sd      s5, 48(sp)
    sd      s6, 56(sp)
    sd      s7, 64(sp)
    sd      s8, 72(sp)
    sd      s9, 80(sp)
    sd      s10, 88(sp)
    sd      s11, 96(sp)
    sd      tp, 104(sp)
    sd      sp, 264(a0)
    ld      t0, 256(a0)
    csrw    sepc, t0
    // U-mode next (SPP clear), with interrupts enabled there (SPIE).
    li      t0, {sstatus_spp}
    csrc    sstatus, t0
    li      t0, {sstatus_spie}
    csrs    sstatus, t0
    csrw    sscratch, a0
    la      t0, riscv64_user_trap_entry
    csrw    stvec, t0
    ld      x1, 8(x10)
    ld      x2, 16(x10)
    ld      x3, 24(x10)
    ld      x4, 32(x10)
    ld      x5, 40(x10)
    ld      x6, 48(x10)
    ld      x7, 56(x10)
    ld      x8, 64(x10)
    ld      x9, 72(x10)
    ld      x11, 88(x10)
    ld      x12, 96(x10)
    ld      x13, 104(x10)
    ld      x14, 112(x10)
    ld      x15, 120(x10)
    ld      x16, 128(x10)
    ld      x17, 136(x10)
    ld      x18, 144(x10)
    ld      x19, 152(x10)
    ld      x20, 160(x10)
    ld      x21, 168(x10)
    ld      x22, 176(x10)
    ld      x23, 184(x10)
    ld      x24, 192(x10)
    ld      x25, 200(x10)
    ld      x26, 208(x10)
    ld      x27, 216(x10)
    ld      x28, 224(x10)
    ld      x29, 232(x10)
    ld      x30, 240(x10)
    ld      x31, 248(x10)
    ld      x10, 80(x10)
    sret
    .popsection

    // The user-mode test program (D47, `TestProgram`): position-independent, no stack, no
    // data. Copied to a user page, never run where it is linked.
    .pushsection .rodata.user_test, "a"
    .balign 4
    .global riscv64_user_test_start
riscv64_user_test_start:
    .global riscv64_user_test_checks
riscv64_user_test_checks:
    li      a0, 1
    li      a1, 2
    li      a2, 3
    li      a3, 4
    li      a4, 5
    li      a5, 6
    li      x1, 0x101
    li      x3, 0x103
    li      x4, 0x104
    li      x5, 0x105
    li      x6, 0x106
    li      x7, 0x107
    li      x8, 0x108
    li      x9, 0x109
    li      x16, 0x110
    li      x18, 0x112
    li      x19, 0x113
    li      x20, 0x114
    li      x21, 0x115
    li      x22, 0x116
    li      x23, 0x117
    li      x24, 0x118
    li      x25, 0x119
    li      x26, 0x11a
    li      x27, 0x11b
    li      x28, 0x11c
    li      x29, 0x11d
    li      x30, 0x11e
    li      x31, 0x11f
    li      a7, {syscall_first}
    ecall
    // Every register but a0 must be as it was: a1 collects the differences.
    addi    a1, a1, -2
    addi    a2, a2, -3
    or      a1, a1, a2
    addi    a3, a3, -4
    or      a1, a1, a3
    addi    a4, a4, -5
    or      a1, a1, a4
    addi    a5, a5, -6
    or      a1, a1, a5
    addi    a7, a7, -{syscall_first}
    or      a1, a1, a7
    addi    x1, x1, -0x101
    or      a1, a1, x1
    addi    x3, x3, -0x103
    or      a1, a1, x3
    addi    x4, x4, -0x104
    or      a1, a1, x4
    addi    x5, x5, -0x105
    or      a1, a1, x5
    addi    x6, x6, -0x106
    or      a1, a1, x6
    addi    x7, x7, -0x107
    or      a1, a1, x7
    addi    x8, x8, -0x108
    or      a1, a1, x8
    addi    x9, x9, -0x109
    or      a1, a1, x9
    addi    x16, x16, -0x110
    or      a1, a1, x16
    addi    x18, x18, -0x112
    or      a1, a1, x18
    addi    x19, x19, -0x113
    or      a1, a1, x19
    addi    x20, x20, -0x114
    or      a1, a1, x20
    addi    x21, x21, -0x115
    or      a1, a1, x21
    addi    x22, x22, -0x116
    or      a1, a1, x22
    addi    x23, x23, -0x117
    or      a1, a1, x23
    addi    x24, x24, -0x118
    or      a1, a1, x24
    addi    x25, x25, -0x119
    or      a1, a1, x25
    addi    x26, x26, -0x11a
    or      a1, a1, x26
    addi    x27, x27, -0x11b
    or      a1, a1, x27
    addi    x28, x28, -0x11c
    or      a1, a1, x28
    addi    x29, x29, -0x11d
    or      a1, a1, x29
    addi    x30, x30, -0x11e
    or      a1, a1, x30
    addi    x31, x31, -0x11f
    or      a1, a1, x31
    li      a7, {syscall_report}
    ecall
    ld      a1, 0(a0)
1:  j       1b
    .global riscv64_user_test_floating_point
riscv64_user_test_floating_point:
    fmv.d.x ft0, zero
    .global riscv64_user_test_null_syscalls
riscv64_user_test_null_syscalls:
    li      a7, {syscall_null}
2:  ecall
    j       2b
    .global riscv64_user_test_spin
riscv64_user_test_spin:
3:  j       3b
    .global riscv64_user_test_linux_hello
riscv64_user_test_linux_hello:
    // Linux's generic numbers: write 64, exit_group 94. s1 collects the differences.
    li      a0, 1
    lla     a1, 9f
    li      a2, {hello_length}
    li      a7, 64
    ecall
    addi    s1, a0, -{hello_length}
    li      a0, 1
    li      a1, 0
    li      a2, 1
    ecall
    addi    a0, a0, 14              // -EFAULT
    or      s1, s1, a0
    li      a7, {unknown_syscall}
    ecall
    addi    a0, a0, 38              // -ENOSYS
    or      s1, s1, a0
    snez    a0, s1
    li      a7, 94
    ecall
4:  j       4b
9:  .ascii  "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global riscv64_user_test_linux_start
riscv64_user_test_linux_start:
    // Linux's generic numbers: writev 66, exit_group 94, sigaltstack 132, rt_sigaction 134,
    // getpid 172, gettid 178, prlimit64 261, getrandom 278. s1 is argc, s2 walks argv and
    // envp, s3 counts the two lists, s4 holds the process id. Each string is written by writev
    // from one iovec on the stack. Ends with exit_group(255) if the stack is misaligned, the
    // thread's id is not the process's, getrandom does not fill the 16 bytes below the stack
    // pointer with something not zero, prlimit64 does not report the stack's limits as 8 MiB
    // soft and unlimited hard, SIGPIPE's action does not read back as SIG_IGN once set, or
    // sigaltstack does not report no alternate stack.
    andi    t0, sp, 15
    li      a0, 255
    bnez    t0, 9f
    li      a7, 172
    ecall
    mv      s4, a0
    li      a7, 178
    ecall
    mv      t0, a0
    li      a0, 255
    bne     t0, s4, 9f
    addi    a0, sp, -16
    li      a1, 16
    li      a2, 0
    li      a7, 278
    ecall
    mv      t0, a0
    li      a0, 255
    li      t1, 16
    bne     t0, t1, 9f
    ld      t0, -16(sp)
    ld      t1, -8(sp)
    or      t0, t0, t1
    beqz    t0, 9f
    li      a0, 0
    li      a1, 3                   // RLIMIT_STACK
    li      a2, 0
    addi    a3, sp, -32
    li      a7, 261
    ecall
    mv      t0, a0
    li      a0, 255
    bnez    t0, 9f
    ld      t0, -32(sp)
    li      t1, 0x800000
    bne     t0, t1, 9f
    ld      t0, -24(sp)
    li      t1, -1
    bne     t0, t1, 9f
    li      t0, 1                   // SIG_IGN
    sd      t0, -64(sp)
    sd      zero, -56(sp)
    sd      zero, -48(sp)
    li      a0, 13                  // SIGPIPE
    addi    a1, sp, -64
    li      a2, 0
    li      a3, 8
    li      a7, 134
    ecall
    mv      t0, a0
    li      a0, 255
    bnez    t0, 9f
    li      a0, 13
    li      a1, 0
    addi    a2, sp, -96
    li      a3, 8
    li      a7, 134
    ecall
    mv      t0, a0
    li      a0, 255
    bnez    t0, 9f
    ld      t0, -96(sp)
    li      t1, 1
    bne     t0, t1, 9f
    li      a0, 0
    addi    a1, sp, -128
    li      a7, 132
    ecall
    mv      t0, a0
    li      a0, 255
    bnez    t0, 9f
    lw      t0, -120(sp)
    li      t1, 2                   // SS_DISABLE
    bne     t0, t1, 9f
    ld      s1, 0(sp)
    addi    s2, sp, 8
    li      s3, 2
6:  ld      a1, 0(s2)
    addi    s2, s2, 8
    beqz    a1, 8f
    li      a2, 0
7:  add     t0, a1, a2
    lbu     t1, 0(t0)
    beqz    t1, 5f
    addi    a2, a2, 1
    j       7b
5:  addi    sp, sp, -16
    sd      a1, 0(sp)
    sd      a2, 8(sp)
    li      a0, 1
    mv      a1, sp
    li      a2, 1
    li      a7, 66
    ecall
    addi    sp, sp, 16
    j       6b
8:  addi    s3, s3, -1
    bnez    s3, 6b
    mv      a0, s1
9:  li      a7, 94
    ecall
4:  j       4b
    .global riscv64_user_test_linux_unknown_syscalls
riscv64_user_test_linux_unknown_syscalls:
    li      a7, {unknown_syscall}
5:  ecall
    j       5b
    .global riscv64_user_test_stack_growth
riscv64_user_test_stack_growth:
    // From the stack pointer's page down, a page at a time: each page's address in its
    // first word, until the stack runs out.
    li      t0, -4096
    and     t0, sp, t0
    li      t1, 4096
6:  sd      t0, 0(t0)
    sub     t0, t0, t1
    j       6b
    .global riscv64_user_test_linux_thread_pointer
riscv64_user_test_linux_thread_pointer:
    // Linux's generic numbers: getpid 172, sched_yield 124, exit_group 94. The thread pointer
    // starts at zero; then getpid() << 12, set in `tp`, must outlast every sched_yield. s0
    // collects the differences.
    mv      s0, tp
    li      a7, 172
    ecall
    slli    s1, a0, 12
    mv      tp, s1
    li      s2, {thread_pointer_rounds}
7:  li      a7, 124
    ecall
    or      s0, s0, a0
    xor     t0, tp, s1
    or      s0, s0, t0
    addi    s2, s2, -1
    bnez    s2, 7b
    snez    a0, s0
    li      a7, 94
    ecall
8:  j       8b
    .global riscv64_user_test_linux_poll
riscv64_user_test_linux_poll:
    // Linux's generic numbers: ioctl 29, ppoll 73, exit_group 94. s1 is the exit status until
    // every check passed. The three `struct pollfd`s are at sp-32, the window size at sp-64,
    // the timeout at sp-48.
    li      s1, 1
    sd      zero, -32(sp)           // fd 0, no events
    li      t0, 1
    sw      t0, -24(sp)             // fd 1, POLLOUT
    li      t0, 4
    sh      t0, -20(sp)
    sh      zero, -18(sp)
    li      t0, 2
    sd      t0, -16(sp)             // fd 2, no events
    sd      zero, -48(sp)
    sd      zero, -40(sp)
    addi    a0, sp, -32
    li      a1, 3
    addi    a2, sp, -48
    li      a3, 0
    li      a4, 0
    li      a7, 73
    ecall
    li      t0, 1
    bne     a0, t0, 9f
    lhu     t0, -26(sp)
    bnez    t0, 9f
    lhu     t0, -18(sp)
    li      t1, 4
    bne     t0, t1, 9f
    lhu     t0, -10(sp)
    bnez    t0, 9f
    li      a0, 1
    li      a1, 0x5413              // TIOCGWINSZ
    addi    a2, sp, -64
    li      a7, 29
    ecall
    bnez    a0, 9f
    li      t0, {poll_wait}
    sd      zero, -48(sp)
    sd      t0, -40(sp)
    li      a0, 0
    li      a1, 0
    addi    a2, sp, -48
    li      a3, 0
    li      a4, 0
    li      a7, 73
    ecall
    bnez    a0, 9f
    ld      t0, -48(sp)
    ld      t1, -40(sp)
    or      t0, t0, t1
    bnez    t0, 9f
    li      s1, 0
9:  mv      a0, s1
    li      a7, 94
    ecall
8:  j       8b
    .global riscv64_user_test_end
riscv64_user_test_end:
    .popsection
"#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym riscv64_exception,
    sstatus_spp = const SSTATUS_SPP,
    sstatus_spie = const SSTATUS_SPIE,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
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
    if scause & SCAUSE_INTERRUPT != 0 && interrupt(scause) {
        return;
    }
    let exception = decode(scause, stval, frame.sepc);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.sepc += instruction_length(frame.sepc);
    }
}

/// Handles the interrupt `scause` names, if the kernel takes it; returns whether it did (if not,
/// it is to be reported).
#[inline(always)]
fn interrupt(scause: u64) -> bool {
    match scause {
        SCAUSE_SUPERVISOR_TIMER => {
            // The interrupt stays pending while `time` >= the deadline; disarming clears it.
            timer::disarm();
            crate::interrupt::deliver(Interrupt::Timer);
            true
        }
        SCAUSE_SUPERVISOR_SOFTWARE => {
            // Cleared before the kernel looks at why it was woken, so a wake sent meanwhile is
            // taken again.
            timer::end_wakeup();
            crate::interrupt::deliver(Interrupt::Wakeup);
            true
        }
        _ => false,
    }
}

/// `sstatus.SPP`: the privilege `sret` returns to (clear: U-mode).
const SSTATUS_SPP: u64 = 1 << 8;
/// `sstatus.SPIE`: whether interrupts are enabled after `sret`.
const SSTATUS_SPIE: u64 = 1 << 5;
/// `scause` for an environment call from U-mode.
const SCAUSE_USER_ECALL: u64 = 8;
/// Size of `ecall`, which has no compressed form.
const ECALL_SIZE: u64 = 4;

unsafe extern "C" {
    /// Enters U-mode with `context`'s registers; returns once a trap from U-mode has saved
    /// them back.
    fn riscv64_enter_user(context: *mut user::riscv64::Registers);
    static riscv64_user_test_start: u8;
    static riscv64_user_test_checks: u8;
    static riscv64_user_test_floating_point: u8;
    static riscv64_user_test_null_syscalls: u8;
    static riscv64_user_test_spin: u8;
    static riscv64_user_test_linux_hello: u8;
    static riscv64_user_test_linux_start: u8;
    static riscv64_user_test_linux_unknown_syscalls: u8;
    static riscv64_user_test_stack_growth: u8;
    static riscv64_user_test_linux_thread_pointer: u8;
    static riscv64_user_test_linux_poll: u8;
    static riscv64_user_test_end: u8;
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked (the caller's duty), so nothing traps between the switch
    // of `stvec` and the `sret`; the trap that ends the program saves its registers into the
    // context, which `&mut` keeps valid and unaliased, puts `stvec` back and returns here on
    // the kernel stack with the callee-saved registers and `tp` restored. The program runs in
    // U-mode (the entry clears `SPP`), where it reaches only user mappings, and the kernel's
    // `sstatus.SUM` is clear outside `with_user_access`.
    unsafe { riscv64_enter_user(registers) };
    let scause: u64;
    let stval: u64;
    // SAFETY: as in `riscv64_exception`: no side effects. Nothing since the trap from U-mode
    // has trapped again (interrupts are masked and the kernel code above cannot fault).
    unsafe { core::arch::asm!("csrr {}, scause", out(reg) scause, options(nomem, nostack)) };
    // SAFETY: as above.
    unsafe { core::arch::asm!("csrr {}, stval", out(reg) stval, options(nomem, nostack)) };
    if scause == SCAUSE_USER_ECALL {
        registers.pc += ECALL_SIZE;
        return UserExit::Syscall;
    }
    if scause & SCAUSE_INTERRUPT != 0 && interrupt(scause) {
        return UserExit::Interrupt;
    }
    UserExit::Exception(decode(scause, stval, registers.pc))
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const riscv64_user_test_start) as usize;
    let end = (&raw const riscv64_user_test_end) as usize;
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, which is part of the
    // kernel image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: (&raw const riscv64_user_test_checks) as usize - start,
        floating_point: (&raw const riscv64_user_test_floating_point) as usize - start,
        null_syscalls: (&raw const riscv64_user_test_null_syscalls) as usize - start,
        spin: (&raw const riscv64_user_test_spin) as usize - start,
        linux_hello: (&raw const riscv64_user_test_linux_hello) as usize - start,
        linux_start: (&raw const riscv64_user_test_linux_start) as usize - start,
        linux_unknown_syscalls: (&raw const riscv64_user_test_linux_unknown_syscalls) as usize
            - start,
        stack_growth: (&raw const riscv64_user_test_stack_growth) as usize - start,
        linux_thread_pointer: (&raw const riscv64_user_test_linux_thread_pointer) as usize - start,
        linux_poll: (&raw const riscv64_user_test_linux_poll) as usize - start,
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
