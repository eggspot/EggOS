//! LoongArch64 kernel thread switch (D290). The callee-saved registers are `ra`, `sp`, `fp` and
//! s0-s8; the kernel is soft-float, so no FP register holds kernel state.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    // a0: where to save the running thread's context; a1: the context to resume.
eggos_switch_thread:
    addi.d      $sp, $sp, -{frame_size}
    st.d        $ra, $sp, 0
    st.d        $fp, $sp, 8
    st.d        $s0, $sp, 16
    st.d        $s1, $sp, 24
    st.d        $s2, $sp, 32
    st.d        $s3, $sp, 40
    st.d        $s4, $sp, 48
    st.d        $s5, $sp, 56
    st.d        $s6, $sp, 64
    st.d        $s7, $sp, 72
    st.d        $s8, $sp, 80
    st.d        $sp, $a0, 0
    move        $sp, $a1
    ld.d        $ra, $sp, 0
    ld.d        $fp, $sp, 8
    ld.d        $s0, $sp, 16
    ld.d        $s1, $sp, 24
    ld.d        $s2, $sp, 32
    ld.d        $s3, $sp, 40
    ld.d        $s4, $sp, 48
    ld.d        $s5, $sp, 56
    ld.d        $s6, $sp, 64
    ld.d        $s7, $sp, 72
    ld.d        $s8, $sp, 80
    addi.d      $sp, $sp, {frame_size}
    jirl        $zero, $ra, 0

    // A new thread's first switch returns here: s0 is its entry, s1 the argument.
    .global eggos_thread_start
eggos_thread_start:
    move        $a0, $s1
    jirl        $ra, $s0, 0
    .word       0
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
);

/// `ra`, `fp`, s0-s8 and one word of padding, which keeps `sp` 16-byte aligned.
const FRAME_WORDS: usize = 12;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: `ra` = the trampoline, `fp` = 0,
/// which ends frame-chain walks, s0 = `entry` and s1 = `argument`.
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
