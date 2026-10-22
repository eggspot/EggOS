//! x86_64 kernel thread switch (D35). The System V callee-saved registers are rbx, rbp,
//! r12-r15 and rsp; the kernel target has no SSE (`x86_64-unknown-none`), so no vector
//! register holds kernel state, and the direction flag is clear at every call by the ABI.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 16
    .global eggos_switch_thread
    // rdi: where to save the running thread's context; rsi: the context to resume.
eggos_switch_thread:
    pushq   %rbp
    pushq   %rbx
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    movq    %rsp, (%rdi)
    movq    %rsi, %rsp
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %rbx
    popq    %rbp
    ret

    // A new thread's first switch returns here: r12 is its entry, rbx the argument.
    .global eggos_thread_start
eggos_thread_start:
    movq    %rbx, %rdi
    callq   *%r12
    ud2
    .popsection
"#,
    options(att_syntax)
);

/// r15, r14, r13, r12, rbx, rbp (lowest address first) and the return address.
const FRAME_WORDS: usize = 7;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    /// Only its address is used: a new thread's first switch returns into it.
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: r12 = `entry`, rbx =
/// `argument`, rbp = 0, which ends frame-chain walks, and the trampoline as return address.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[3] = entry as usize;
    frame[4] = argument;
    frame[6] = eggos_thread_start as *const () as usize;
    frame
}
