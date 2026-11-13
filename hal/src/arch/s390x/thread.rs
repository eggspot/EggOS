//! s390x kernel thread switch (D321). The ELF ABI's callee-saved registers are r6 to r15; the
//! kernel is soft-float, so no FP register holds kernel state. A frame keeps the 160-byte
//! register save area the ABI gives every callee below it.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
    # r2: where to save the running thread's context; r3: the context to resume.
eggos_switch_thread:
    aghi        %r15, -{frame_size}
    stmg        %r6, %r14, 160(%r15)
    stg         %r15, 0(%r2)
    lgr         %r15, %r3
    lmg         %r6, %r14, 160(%r15)
    aghi        %r15, {frame_size}
    br          %r14

    # A new thread's first switch returns here: r6 is its entry, r7 the argument.
    .global eggos_thread_start
eggos_thread_start:
    lgr         %r2, %r7
    basr        %r14, %r6
    .word       0
    .popsection
"#,
    frame_size = const FRAME_WORDS * 8,
);

const FRAME_WORDS: usize = 20 + 9 + 1;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: r6 = `entry`, r7 = `argument`,
/// r14 = the trampoline.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[20] = entry as usize;
    frame[21] = argument;
    frame[28] = eggos_thread_start as *const () as usize;
    frame
}
