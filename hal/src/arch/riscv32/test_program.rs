//! RV32's user test program (D47, D481): the register check, floating point, null system
//! calls, the spin, the illegal instruction and Linux's hello, as RV64's run them. The Linux
//! programs RV32 does not have yet (start-up, signals, polling, clocks and the rest) exit with
//! status 1 at once, so their self-tests fail plainly until they are written.

use crate::user::TestProgram;

core::arch::global_asm!(
    r#"
    .pushsection .rodata.user_test, "a"
    .balign 4
    .global riscv32_user_test_start
riscv32_user_test_start:
riscv32_user_test_checks:
    li      s0, 0x5a000000
    li      s1, 0x5a000001
    li      s2, 0x5a000002
    li      s3, 0x5a000003
    li      s4, 0x5a000004
    li      s5, 0x5a000005
    li      s6, 0x5a000006
    li      s7, 0x5a000007
    li      s8, 0x5a000008
    li      s9, 0x5a000009
    li      s10, 0x5a00000a
    li      s11, 0x5a00000b
    li      a0, 1
    li      a1, 2
    li      a2, 3
    li      a3, 4
    li      a4, 5
    li      a5, 6
    li      a7, 0
    ecall
    li      a1, 0
    li      t0, 0x5a000000
    bne     s0, t0, 1f
    li      t0, 0x5a000005
    bne     s5, t0, 1f
    li      t0, 0x5a00000b
    bne     s11, t0, 1f
    j       2f
1:  li      a1, 1
2:  li      a7, 1
    ecall
    lw      t0, 0(a0)

riscv32_user_test_floating_point:
    fadd.d  f0, f0, f0
riscv32_user_test_null_syscalls:
    li      a7, 2
3:  ecall
    j       3b

riscv32_user_test_spin:
4:  j       4b

riscv32_user_test_illegal_instruction:
    unimp

riscv32_user_test_linux_hello:
    li      a0, 1
    lla     a1, 5f
    li      a2, 6f - 5f
    li      a7, 64
    ecall
    li      a0, 0
    li      a7, 94
    ecall
5:  .ascii  "EGGOS-USER: hello from user mode\n"
6:
    .balign 4
riscv32_user_test_not_yet:
    li      a0, 1
    li      a7, 94
    ecall
    .global riscv32_user_test_end
riscv32_user_test_end:
    .popsection
"#
);

unsafe extern "C" {
    static riscv32_user_test_start: u8;
    static riscv32_user_test_checks: u8;
    static riscv32_user_test_floating_point: u8;
    static riscv32_user_test_null_syscalls: u8;
    static riscv32_user_test_spin: u8;
    static riscv32_user_test_illegal_instruction: u8;
    static riscv32_user_test_linux_hello: u8;
    static riscv32_user_test_not_yet: u8;
    static riscv32_user_test_end: u8;
}

/// [`crate::Platform::user_test_program`] on RV32.
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const riscv32_user_test_start) as usize;
    let end = (&raw const riscv32_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    let not_yet = at(&raw const riscv32_user_test_not_yet);
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written while the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const riscv32_user_test_checks),
        floating_point: at(&raw const riscv32_user_test_floating_point),
        null_syscalls: at(&raw const riscv32_user_test_null_syscalls),
        spin: at(&raw const riscv32_user_test_spin),
        fp_registers: not_yet,
        illegal_instruction: at(&raw const riscv32_user_test_illegal_instruction),
        linux_hello: at(&raw const riscv32_user_test_linux_hello),
        linux_start: not_yet,
        linux_start_end: not_yet,
        linux_unknown_syscalls: not_yet,
        stack_growth: not_yet,
        linux_thread_pointer: not_yet,
        linux_poll: not_yet,
        linux_signals: not_yet,
        linux_signal_handler: not_yet,
        linux_fault_handler: not_yet,
        linux_signal_queue: not_yet,
        linux_signal_suspend: not_yet,
        linux_clock: not_yet,
        linux_stat: not_yet,
        linux_credentials: not_yet,
        linux_move_and_discard: not_yet,
    }
}
