//! SPARC64 kernel thread switch (D326). Register windows hold the callers' state: the switch
//! flushes every window of the running thread to its stack (`flushw`), saves its `%sp` and
//! `%i7`, and resumes the other thread from its saved `%sp` and `%i7` (its `%fp` is the frame
//! above) by returning, whose `restore` fills its caller's window from its stack. Stacks carry
//! the ABI's 2047 bias: a saved context is a biased `%sp`.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
eggos_switch_thread:
    save        %sp, -{frame}, %sp
    flushw
    stx         %i7, [%sp + {bias} + {frame_i7}]
    stx         %sp, [%i0]
    mov         %i1, %sp
    add         %sp, {frame}, %fp
    ldx         [%sp + {bias} + {frame_i7}], %i7
    ret
     restore

    .global eggos_thread_start
eggos_thread_start:
    mov         %l1, %o0
    call        %l0
     nop
    illtrap     0
    .popsection
"#,
    frame = const SWITCH_FRAME_BYTES,
    bias = const STACK_BIAS,
    frame_i7 = const I7_OFFSET,
);

/// The ABI's stack bias: `%sp` is 2047 below the frame's real address.
pub(super) const STACK_BIAS: usize = 2047;
const SWITCH_FRAME_BYTES: usize = 192;
const I7_OFFSET: usize = 176;
const SAVE_AREA_WORDS: usize = 16;
const FRAME_WORDS: usize = SWITCH_FRAME_BYTES / 8 + SAVE_AREA_WORDS;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    fn eggos_thread_start();
}

/// The initial frames of a thread that calls `entry(argument)`, lowest first: the switch's
/// frame, whose `%i7` slot holds the trampoline less 8 (`ret` adds 8), then the window save area
/// the switch's `restore` fills the trampoline's window from: `%l0` the entry, `%l1` the
/// argument, `%i6` 0, which ends frame-chain walks.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let above = SWITCH_FRAME_BYTES / 8;
    let mut frame = [0; FRAME_WORDS];
    frame[I7_OFFSET / 8] = eggos_thread_start as *const () as usize - 8;
    frame[above] = entry as usize;
    frame[above + 1] = argument;
    frame
}
