//! POWER kernel thread switch (D304). The callee-saved registers are `r14` to `r31`, `r1`,
//! `r2` (the TOC, the same everywhere in the kernel), the link register and CR fields 2 to 4;
//! the kernel uses no FP or vector register.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    // r3: where to save the running thread's context; r4: the context to resume.
eggos_switch_thread:
    mflr        %r0
    stdu        %r1, -{frame_size}(%r1)
    std         %r0, 32(%r1)
    mfcr        %r0
    std         %r0, 40(%r1)
    .irp        n, 14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    std         %r\n, (48 + (\n - 14) * 8)(%r1)
    .endr
    std         %r1, 0(%r3)
    mr          %r1, %r4
    ld          %r0, 40(%r1)
    mtcr        %r0
    .irp        n, 14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld          %r\n, (48 + (\n - 14) * 8)(%r1)
    .endr
    ld          %r0, 32(%r1)
    mtlr        %r0
    addi        %r1, %r1, {frame_size}
    blr

    // A new thread's first switch returns here: r14 is its entry, r15 the argument.
    .global eggos_thread_start
eggos_thread_start:
    mr          %r3, %r15
    mr          %r12, %r14
    mtctr       %r14
    bctrl
    .long       0
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
);

/// 32 bytes of ABI header, the link register, CR, `r14` to `r31`, padding to 16 bytes.
const FRAME_WORDS: usize = 4 + 2 + 18;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: the link register the
/// trampoline, CR 0, `r14` `entry` and `r15` `argument`, the rest zero.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[4] = eggos_thread_start as *const () as usize;
    frame[6] = entry as usize;
    frame[7] = argument;
    frame
}
