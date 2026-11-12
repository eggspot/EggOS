//! 32-bit Arm's user test program (D47, D482): the register check, floating point, null
//! system calls, the spin, the undefined instruction and Linux's hello in the EABI (`r7` the
//! call's number, `svc #0`). The Linux programs not written for 32-bit Arm yet exit with
//! status 1 at once, so their self-tests fail plainly.

use crate::user::TestProgram;

core::arch::global_asm!(
    r#"
    .pushsection .rodata.user_test, "a"
    .arm
    .fpu vfpv3-d32
    .balign 4
    .global arm_user_test_start
arm_user_test_start:
arm_user_test_checks:
    ldr     r8, =0x5a000008
    ldr     r9, =0x5a000009
    ldr     r10, =0x5a00000a
    ldr     r11, =0x5a00000b
    mov     r0, #1
    mov     r1, #2
    mov     r2, #3
    mov     r3, #4
    mov     r4, #5
    mov     r5, #6
    mov     r7, #0
    svc     #0
    mov     r1, #0
    ldr     r2, =0x5a000008
    cmp     r8, r2
    movne   r1, #1
    ldr     r2, =0x5a00000b
    cmp     r11, r2
    movne   r1, #1
    mov     r7, #1
    svc     #0
    ldr     r2, [r0]
    .ltorg

arm_user_test_floating_point:
    vadd.f64 d0, d0, d0
arm_user_test_null_syscalls:
    mov     r7, #2
1:  svc     #0
    b       1b

arm_user_test_spin:
2:  b       2b

arm_user_test_illegal_instruction:
    udf     #0

arm_user_test_linux_hello:
    mov     r0, #1
    adr     r1, 3f
    mov     r2, #(4f - 3f)
    mov     r7, #4
    svc     #0
    mov     r0, #0
    mov     r7, #248
    svc     #0
3:  .ascii  "EGGOS-USER: hello from user mode\n"
4:
    .balign 4
arm_user_test_not_yet:
    mov     r0, #1
    mov     r7, #248
    svc     #0
    .global arm_user_test_end
arm_user_test_end:
    .popsection
"#
);

unsafe extern "C" {
    static arm_user_test_start: u8;
    static arm_user_test_checks: u8;
    static arm_user_test_floating_point: u8;
    static arm_user_test_null_syscalls: u8;
    static arm_user_test_spin: u8;
    static arm_user_test_illegal_instruction: u8;
    static arm_user_test_linux_hello: u8;
    static arm_user_test_not_yet: u8;
    static arm_user_test_end: u8;
}

/// [`crate::Platform::user_test_program`] on 32-bit Arm.
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const arm_user_test_start) as usize;
    let end = (&raw const arm_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    let not_yet = at(&raw const arm_user_test_not_yet);
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written while the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const arm_user_test_checks),
        floating_point: at(&raw const arm_user_test_floating_point),
        null_syscalls: at(&raw const arm_user_test_null_syscalls),
        spin: at(&raw const arm_user_test_spin),
        fp_registers: not_yet,
        illegal_instruction: at(&raw const arm_user_test_illegal_instruction),
        linux_hello: at(&raw const arm_user_test_linux_hello),
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
