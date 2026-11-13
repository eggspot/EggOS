//! RV32's kernel thread switch (D35, D479): RV64's (`ra`, `sp` and `s0` to `s11` callee-saved)
//! with 32-bit words, its frame padded to 16 bytes as RV32's calling convention keeps `sp`.

core::arch::global_asm!(
    r#"
    .pushsection .text.thread, "ax"
    .balign 4
    .global eggos_switch_thread
eggos_switch_thread:
    addi    sp, sp, -{frame_size}
    sw      ra, 0(sp)
    sw      s0, 4(sp)
    sw      s1, 8(sp)
    sw      s2, 12(sp)
    sw      s3, 16(sp)
    sw      s4, 20(sp)
    sw      s5, 24(sp)
    sw      s6, 28(sp)
    sw      s7, 32(sp)
    sw      s8, 36(sp)
    sw      s9, 40(sp)
    sw      s10, 44(sp)
    sw      s11, 48(sp)
    sw      sp, 0(a0)
    mv      sp, a1
    lw      ra, 0(sp)
    lw      s0, 4(sp)
    lw      s1, 8(sp)
    lw      s2, 12(sp)
    lw      s3, 16(sp)
    lw      s4, 20(sp)
    lw      s5, 24(sp)
    lw      s6, 28(sp)
    lw      s7, 32(sp)
    lw      s8, 36(sp)
    lw      s9, 40(sp)
    lw      s10, 44(sp)
    lw      s11, 48(sp)
    addi    sp, sp, {frame_size}
    ret

    .global eggos_thread_start
eggos_thread_start:
    mv      a0, s2
    jalr    s1
    unimp
    .popsection
"#,
    frame_size = const FRAME_WORDS * 4,
);

const FRAME_WORDS: usize = 16;

unsafe extern "C" {
    pub(super) fn eggos_switch_thread(save: *mut usize, resume: usize);
    fn eggos_thread_start();
}

/// The initial frame of a thread that calls `entry(argument)`: `ra` the trampoline, `s0` 0,
/// which ends frame-chain walks, `s1` the entry and `s2` the argument.
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
