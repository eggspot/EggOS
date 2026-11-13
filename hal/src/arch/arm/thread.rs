//! 32-bit Arm's kernel thread switch (D35, D479): AAPCS's callee-saved registers `r4` to
//! `r11` and the return address `lr`, with one word of padding so the frame keeps `sp` 8-byte
//! aligned (16 here, as every architecture's initial frame ends 16-byte aligned).

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .arm
    .balign 4
    .global eggos_switch_thread
eggos_switch_thread:
    push    {{r3-r11, lr}}
    str     sp, [r0]
    mov     sp, r1
    pop     {{r3-r11, lr}}
    bx      lr

    .global eggos_thread_start
eggos_thread_start:
    mov     r0, r6
    blx     r5
    udf     #0
    .popsection
"#,
);

const FRAME_WORDS: usize = 10;
const ENTRY: usize = 2;
const ARGUMENT: usize = 3;
const RETURN_ADDRESS: usize = 9;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: `lr` the trampoline, `r5` the
/// entry and `r6` the argument, `r11` (the frame pointer) 0.
pub(super) fn initial_frame(
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> [usize; FRAME_WORDS] {
    let mut frame = [0; FRAME_WORDS];
    frame[RETURN_ADDRESS] = eggos_thread_start as *const () as usize;
    frame[ENTRY] = entry as usize;
    frame[ARGUMENT] = argument;
    frame
}
