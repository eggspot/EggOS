//! AArch64 kernel thread switch (D35). The AAPCS64 callee-saved registers are x19-x29, the
//! link register x30 and `sp`; the kernel is built soft-float (D10), so no FP/SIMD register
//! holds kernel state.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    // x0: where to save the running thread's context; x1: the context to resume.
eggos_switch_thread:
    sub     sp, sp, #{frame_size}
    stp     x19, x20, [sp, #0]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    mov     x9, sp
    str     x9, [x0]
    mov     sp, x1
    ldp     x19, x20, [sp, #0]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    add     sp, sp, #{frame_size}
    ret

    // A new thread's first switch returns here: x19 is its entry, x20 the argument.
    .global eggos_thread_start
eggos_thread_start:
    mov     x0, x20
    blr     x19
    udf     #0
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
);

/// x19-x30, as `eggos_switch_thread` stores them.
const FRAME_WORDS: usize = 12;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: x19 = `entry`, x20 =
/// `argument`, x29 (the frame pointer) = 0, which ends frame-chain walks, and x30 = the
/// trampoline.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[0] = entry as usize;
    frame[1] = argument;
    frame[11] = eggos_thread_start as *const () as usize;
    frame
}
