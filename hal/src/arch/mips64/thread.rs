//! MIPS64 kernel thread switch (D313). The n64 callee-saved registers are `ra`, `sp`, `s0`-`s7`
//! and `s8`; `gp` is the CPU's block, not the thread's, so it stays; the kernel is soft-float,
//! so no FP register holds kernel state.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    // a0: where to save the running thread's context; a1: the context to resume.
eggos_switch_thread:
    daddiu      $29, $29, -{frame_size}
    sd          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    sd          $\n, (\n - 15) * 8($29)
    .endr
    sd          $30, 72($29)
    sd          $29, 0($4)
    move        $29, $5
    ld          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    ld          $\n, (\n - 15) * 8($29)
    .endr
    ld          $30, 72($29)
    daddiu      $29, $29, {frame_size}
    jr          $31

    // A new thread's first switch returns here: s0 is its entry, s1 the argument.
    .global eggos_thread_start
eggos_thread_start:
    move        $4, $17
    move        $25, $16
    jalr        $25
    .word       {reserved_instruction}
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
    reserved_instruction = const 5,
);

/// `ra`, s0-s7, `s8` and two words of padding, which keep `sp` 16-byte aligned.
const FRAME_WORDS: usize = 12;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: `ra` = the trampoline, s0 =
/// `entry`, s1 = `argument`, `s8` = 0, which ends frame-chain walks.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[0] = eggos_thread_start as *const () as usize;
    frame[1] = entry as usize;
    frame[2] = argument;
    frame
}
