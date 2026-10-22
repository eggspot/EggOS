//! RISC-V kernel thread switch (D35). The callee-saved registers are `ra`, `sp` and s0-s11;
//! the boot code turns the FPU off, so no FP register holds kernel state (see `exception`).

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    // a0: where to save the running thread's context; a1: the context to resume.
eggos_switch_thread:
    addi    sp, sp, -{frame_size}
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
    sd      sp, 0(a0)
    mv      sp, a1
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
    addi    sp, sp, {frame_size}
    ret

    // A new thread's first switch returns here: s1 is its entry, s2 the argument.
    .global eggos_thread_start
eggos_thread_start:
    mv      a0, s2
    jalr    s1
    unimp
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
);

/// `ra`, s0-s11 and one word of padding, which keeps `sp` 16-byte aligned.
const FRAME_WORDS: usize = 14;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: `ra` = the trampoline, s0 (the
/// frame pointer) = 0, which ends frame-chain walks, s1 = `entry` and s2 = `argument`.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[0] = eggos_thread_start as *const () as usize;
    frame[2] = entry as usize;
    frame[3] = argument;
    frame
}
