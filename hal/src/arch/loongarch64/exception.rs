//! LoongArch64 exception entry (LoongArch Reference Manual, volume 1, 6.2; D290).
//!
//! `EENTRY` points at [`loongarch64_kernel_entry`] (`ECFG.VS` 0: every exception and interrupt
//! arrives there), which saves the general-purpose registers, `ERA` and `PRMD` on the current
//! stack, calls [`loongarch64_exception`] with the frame and restores everything; while a
//! program runs it points at the user entry instead, which saves the program's registers into its
//! `UserContext` and returns from [`run_user`] (D47), so exceptions from the kernel pay nothing
//! for user mode. TLB refills have their own entry, `TLBRENTRY`, run in direct address mode: the
//! handler walks the tables with `lddir` and `ldpte`. The FP unit is off for the kernel, which is
//! soft-float, and on for a program only once it used it (`fp`, D54).

use super::{csr, csr_read, interrupts, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::user::{self, TestProgram, UserContext, UserExit};

core::arch::global_asm!(
    r#"
    .pushsection .text.tlb_refill, "ax"
    .balign 4096
    .global loongarch64_tlb_refill
loongarch64_tlb_refill:
    csrwr       $t0, {tlbrsave}
    csrwr       $t1, {save7}
    csrrd       $t0, {pgd}
    lddir       $t0, $t0, 3
    lddir       $t0, $t0, 2
    lddir       $t0, $t0, 1
    ldpte       $t0, 0
    ldpte       $t0, 1
    tlbfill
    li.w        $t0, {page_shift}
    li.w        $t1, 0x3f
    csrxchg     $t0, $t1, {tlbrehi}
    csrrd       $t1, {save7}
    csrrd       $t0, {tlbrsave}
    ertn
    .popsection

    .pushsection .text.trap, "ax"
    .balign 4096
    .global loongarch64_kernel_entry
loongarch64_kernel_entry:
    addi.d      $sp, $sp, -{frame_size}
    st.d        $r1, $sp, 8
    st.d        $r2, $sp, 16
    .irp        n, 4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    st.d        $r\n, $sp, \n * 8
    .endr
    addi.d      $t0, $sp, {frame_size}
    st.d        $t0, $sp, 24
    csrrd       $t0, {era}
    st.d        $t0, $sp, 256
    csrrd       $t0, {prmd}
    st.d        $t0, $sp, 264
    move        $a0, $sp
    bl          {handler}
    ld.d        $t0, $sp, 264
    csrwr       $t0, {prmd}
    ld.d        $t0, $sp, 256
    csrwr       $t0, {era}
    ld.d        $r1, $sp, 8
    ld.d        $r2, $sp, 16
    .irp        n, 4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld.d        $r\n, $sp, \n * 8
    .endr
    addi.d      $sp, $sp, {frame_size}
    ertn

    .balign 4096
loongarch64_user_entry:
    csrwr       $t0, {save0}
    st.d        $r1, $t0, 8
    st.d        $r2, $t0, 16
    st.d        $r3, $t0, 24
    .irp        n, 4,5,6,7,8,9,10,11,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    st.d        $r\n, $t0, \n * 8
    .endr
    csrrd       $t1, {save0}
    st.d        $t1, $t0, 96
    csrrd       $t1, {era}
    st.d        $t1, $t0, 256
    la.pcrel    $t1, loongarch64_kernel_entry
    csrwr       $t1, {eentry}
    ld.d        $sp, $t0, 264
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
    ld.d        $r21, $sp, 88
    addi.d      $sp, $sp, 96
    jirl        $zero, $ra, 0

    .global loongarch64_enter_user
loongarch64_enter_user:
    addi.d      $sp, $sp, -96
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
    st.d        $r21, $sp, 88
    st.d        $sp, $a0, 264
    ld.d        $t0, $a0, 256
    csrwr       $t0, {era}
    li.w        $t0, {prmd_user}
    csrwr       $t0, {prmd}
    move        $t0, $a0
    csrwr       $t0, {save0}
    la.pcrel    $t0, loongarch64_user_entry
    csrwr       $t0, {eentry}
    ld.d        $r1, $a0, 8
    ld.d        $r2, $a0, 16
    ld.d        $r3, $a0, 24
    .irp        n, 5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld.d        $r\n, $a0, \n * 8
    .endr
    ld.d        $a0, $a0, 32
    ertn
    .popsection

    .pushsection .rodata.user_test, "a"
    .balign 4
    .global loongarch64_user_test_start
loongarch64_user_test_start:
    .global loongarch64_user_test_checks
loongarch64_user_test_checks:
    li.w        $a0, 1
    li.w        $a1, 2
    li.w        $a2, 3
    li.w        $a3, 4
    li.w        $a4, 5
    li.w        $a5, 6
    li.w        $r1, 0x101
    li.w        $r2, 0x102
    li.w        $r10, 0x10a
    .irp        n, 12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    li.w        $r\n, 0x100 + \n
    .endr
    li.w        $a7, {syscall_first}
    syscall     0
    addi.d      $a1, $a1, -2
    addi.d      $a2, $a2, -3
    or          $a1, $a1, $a2
    addi.d      $a3, $a3, -4
    or          $a1, $a1, $a3
    addi.d      $a4, $a4, -5
    or          $a1, $a1, $a4
    addi.d      $a5, $a5, -6
    or          $a1, $a1, $a5
    addi.d      $a7, $a7, -{syscall_first}
    or          $a1, $a1, $a7
    addi.d      $r1, $r1, -0x101
    or          $a1, $a1, $r1
    addi.d      $r2, $r2, -0x102
    or          $a1, $a1, $r2
    addi.d      $r10, $r10, -0x10a
    or          $a1, $a1, $r10
    .irp        n, 12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi.d      $r\n, $r\n, -(0x100 + \n)
    or          $a1, $a1, $r\n
    .endr
    li.w        $a7, {syscall_report}
    syscall     0
    ld.d        $a1, $a0, 0
1:  b           1b
    .global loongarch64_user_test_floating_point
loongarch64_user_test_floating_point:
    movgr2fr.d  $f0, $zero
    .global loongarch64_user_test_null_syscalls
loongarch64_user_test_null_syscalls:
    li.w        $a7, {syscall_null}
2:  syscall     0
    b           2b
    .global loongarch64_user_test_spin
loongarch64_user_test_spin:
3:  b           3b
    .global loongarch64_user_test_linux_hello
loongarch64_user_test_linux_hello:
    li.w        $a0, 1
    la.pcrel    $a1, 9f
    li.w        $a2, {hello_length}
    li.w        $a7, 64
    syscall     0
    addi.d      $s1, $a0, -{hello_length}
    li.w        $a0, 1
    li.w        $a1, 0
    li.w        $a2, 1
    syscall     0
    addi.d      $a0, $a0, 14
    or          $s1, $s1, $a0
    li.w        $a7, {unknown_syscall}
    syscall     0
    addi.d      $a0, $a0, 38
    or          $s1, $s1, $a0
    sltu        $a0, $zero, $s1
    li.w        $a7, 94
    syscall     0
4:  b           4b
9:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global loongarch64_user_test_linux_start
loongarch64_user_test_linux_start:
    andi        $t0, $sp, 15
    li.w        $a0, 255
    bnez        $t0, 9f
    li.w        $a7, 172
    syscall     0
    move        $s4, $a0
    li.w        $a7, 178
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    bne         $t0, $s4, 9f
    addi.d      $a0, $sp, -16
    li.w        $a1, 16
    li.w        $a2, 0
    li.w        $a7, 278
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    li.w        $t1, 16
    bne         $t0, $t1, 9f
    ld.d        $t0, $sp, -16
    ld.d        $t1, $sp, -8
    or          $t0, $t0, $t1
    beqz        $t0, 9f
    li.w        $a0, 0
    li.w        $a1, 3
    li.w        $a2, 0
    addi.d      $a3, $sp, -32
    li.w        $a7, 261
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    bnez        $t0, 9f
    ld.d        $t0, $sp, -32
    li.w        $t1, 0x800000
    bne         $t0, $t1, 9f
    ld.d        $t0, $sp, -24
    li.d        $t1, -1
    bne         $t0, $t1, 9f
    li.w        $t0, 1
    st.d        $t0, $sp, -64
    st.d        $zero, $sp, -56
    st.d        $zero, $sp, -48
    li.w        $a0, 13
    addi.d      $a1, $sp, -64
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 134
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    bnez        $t0, 9f
    li.w        $a0, 13
    li.w        $a1, 0
    addi.d      $a2, $sp, -96
    li.w        $a3, 8
    li.w        $a7, 134
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    bnez        $t0, 9f
    ld.d        $t0, $sp, -96
    li.w        $t1, 1
    bne         $t0, $t1, 9f
    li.w        $a0, 0
    addi.d      $a1, $sp, -128
    li.w        $a7, 132
    syscall     0
    move        $t0, $a0
    li.w        $a0, 255
    bnez        $t0, 9f
    ld.w        $t0, $sp, -120
    li.w        $t1, 2
    bne         $t0, $t1, 9f
    ld.d        $s1, $sp, 0
    addi.d      $s2, $sp, 8
    li.w        $s3, 2
6:  ld.d        $a1, $s2, 0
    addi.d      $s2, $s2, 8
    beqz        $a1, 8f
    li.w        $a2, 0
7:  add.d       $t0, $a1, $a2
    ld.bu       $t1, $t0, 0
    beqz        $t1, 5f
    addi.d      $a2, $a2, 1
    b           7b
5:  addi.d      $sp, $sp, -16
    st.d        $a1, $sp, 0
    st.d        $a2, $sp, 8
    li.w        $a0, 1
    move        $a1, $sp
    li.w        $a2, 1
    li.w        $a7, 66
    syscall     0
    addi.d      $sp, $sp, 16
    b           6b
8:  addi.d      $s3, $s3, -1
    bnez        $s3, 6b
    move        $a0, $s1
9:  li.w        $a7, 94
    syscall     0
4:  b           4b
    .global loongarch64_user_test_linux_start_end
loongarch64_user_test_linux_start_end:
    .global loongarch64_user_test_linux_unknown_syscalls
loongarch64_user_test_linux_unknown_syscalls:
    li.w        $a7, {unknown_syscall}
5:  syscall     0
    b           5b
    .global loongarch64_user_test_stack_growth
loongarch64_user_test_stack_growth:
    li.d        $t0, -4096
    and         $t0, $sp, $t0
    li.w        $t1, 4096
6:  st.d        $t0, $t0, 0
    sub.d       $t0, $t0, $t1
    b           6b
    .global loongarch64_user_test_linux_thread_pointer
loongarch64_user_test_linux_thread_pointer:
    move        $s0, $tp
    li.w        $a7, 172
    syscall     0
    slli.d      $s1, $a0, 12
    move        $tp, $s1
    li.w        $s2, {thread_pointer_rounds}
7:  li.w        $a7, 124
    syscall     0
    or          $s0, $s0, $a0
    xor         $t0, $tp, $s1
    or          $s0, $s0, $t0
    addi.d      $s2, $s2, -1
    bnez        $s2, 7b
    sltu        $a0, $zero, $s0
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_linux_poll
loongarch64_user_test_linux_poll:
    li.w        $s1, 1
    st.d        $zero, $sp, -32
    li.w        $t0, 1
    st.w        $t0, $sp, -24
    li.w        $t0, 4
    st.h        $t0, $sp, -20
    st.h        $zero, $sp, -18
    li.w        $t0, 2
    st.d        $t0, $sp, -16
    st.d        $zero, $sp, -48
    st.d        $zero, $sp, -40
    addi.d      $a0, $sp, -32
    li.w        $a1, 3
    addi.d      $a2, $sp, -48
    li.w        $a3, 0
    li.w        $a4, 0
    li.w        $a7, 73
    syscall     0
    li.w        $t0, 1
    bne         $a0, $t0, 9f
    ld.hu       $t0, $sp, -26
    bnez        $t0, 9f
    ld.hu       $t0, $sp, -18
    li.w        $t1, 4
    bne         $t0, $t1, 9f
    ld.hu       $t0, $sp, -10
    bnez        $t0, 9f
    li.w        $a0, 1
    li.w        $a1, 0x5413
    addi.d      $a2, $sp, -64
    li.w        $a7, 29
    syscall     0
    bnez        $a0, 9f
    li.w        $t0, {poll_wait}
    st.d        $zero, $sp, -48
    st.d        $t0, $sp, -40
    li.w        $a0, 0
    li.w        $a1, 0
    addi.d      $a2, $sp, -48
    li.w        $a3, 0
    li.w        $a4, 0
    li.w        $a7, 73
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, -48
    ld.d        $t1, $sp, -40
    or          $t0, $t0, $t1
    bnez        $t0, 9f
    li.w        $s1, 0
9:  move        $a0, $s1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_linux_signals
loongarch64_user_test_linux_signals:
    li.w        $a0, 0
    li.w        $a1, 17
    li.w        $a7, 129
    syscall     0
    bnez        $a0, 9f
    li.w        $t0, 1 << 9
    st.d        $t0, $sp, -16
    li.w        $a0, 0
    addi.d      $a1, $sp, -16
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
    bnez        $a0, 9f
    li.w        $a7, 172
    syscall     0
    move        $a1, $a0
    li.w        $a2, 10
    li.w        $a7, 131
    syscall     0
    bnez        $a0, 9f
    addi.d      $a0, $sp, -32
    li.w        $a1, 8
    li.w        $a7, 136
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, -32
    li.w        $t1, 1 << 9
    bne         $t0, $t1, 9f
    li.w        $a0, 1
    addi.d      $a1, $sp, -16
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_linux_signal_handler
loongarch64_user_test_linux_signal_handler:
    li.w        $t0, 0x2800
    sub.d       $t1, $sp, $t0
    st.d        $t1, $sp, -32
    st.d        $zero, $sp, -24
    li.w        $t0, 0x2000
    st.d        $t0, $sp, -16
    addi.d      $a0, $sp, -32
    li.w        $a1, 0
    li.w        $a7, 132
    syscall     0
    bnez        $a0, 9f
    la.pcrel    $t0, 5f
    st.d        $t0, $sp, -64
    li.w        $t0, 0x08000004
    st.d        $t0, $sp, -56
    st.d        $zero, $sp, -48
    li.w        $a0, 10
    addi.d      $a1, $sp, -64
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 134
    syscall     0
    bnez        $a0, 9f
    li.w        $a7, 172
    syscall     0
    move        $a1, $a0
    li.w        $a2, 10
    li.w        $s1, 0x5a5
    li.w        $s2, 1
    li.d        $t0, {signal_fp_kept}
    movgr2fr.d  $fs0, $t0
    li.w        $t0, {signal_rounding}
    movgr2fcsr  $fcsr0, $t0
    li.w        $a7, 131
    syscall     0
    bnez        $a0, 9f
    li.w        $t0, 0x5a5
    bne         $s1, $t0, 9f
    li.w        $t0, 2
    bne         $s2, $t0, 9f
    movfr2gr.d  $t0, $fs0
    li.d        $t1, {signal_fp_kept}
    bne         $t0, $t1, 9f
    movfr2gr.d  $t0, $fs1
    li.d        $t1, {signal_fp_changed}
    bne         $t0, $t1, 9f
    movfcsr2gr  $t0, $fcsr0
    andi        $t0, $t0, 0x300
    li.w        $t1, {signal_rounding}
    bne         $t0, $t1, 9f
    li.w        $a0, 0
    li.w        $a1, 0
    addi.d      $a2, $sp, -72
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, -72
    bnez        $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
5:  li.w        $t0, 10
    bne         $a0, $t0, 7f
    ld.w        $t1, $a1, 0
    bne         $t1, $t0, 7f
    ld.w        $t1, $a1, 8
    li.w        $t0, -6
    bne         $t1, $t0, 7f
    ld.d        $t0, $a2, 16
    bltu        $sp, $t0, 7f
    ld.d        $t1, $a2, 32
    add.d       $t0, $t0, $t1
    bgeu        $sp, $t0, 7f
    li.d        $t1, {signal_fp_kept}
    movfr2gr.d  $t0, $fs0
    bne         $t0, $t1, 7f
    ld.d        $t0, $a2, {saved_fs0}
    bne         $t0, $t1, 7f
    ld.w        $t0, $a2, {saved_fcsr}
    li.w        $t1, {signal_rounding}
    bne         $t0, $t1, 7f
    li.d        $t0, {signal_fp_changed}
    st.d        $t0, $a2, {saved_fs1}
    movgr2fr.d  $fs0, $zero
    li.w        $t0, 2
    st.d        $t0, $a2, {saved_s2}
7:  li.w        $s1, 0
    jirl        $zero, $ra, 0
    .global loongarch64_user_test_linux_fault_handler
loongarch64_user_test_linux_fault_handler:
    la.pcrel    $t0, 5f
    st.d        $t0, $sp, -64
    li.w        $t0, 4
    st.d        $t0, $sp, -56
    st.d        $zero, $sp, -48
    li.w        $a0, 11
    addi.d      $a1, $sp, -64
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 134
    syscall     0
    bnez        $a0, 9f
    li.w        $a0, 0
    li.w        $a1, 4096
    li.w        $a2, 0
    li.w        $a3, 0x22
    li.w        $a4, -1
    li.w        $a5, 0
    li.w        $a7, 222
    syscall     0
    li.d        $t0, -4095
    bgeu        $a0, $t0, 9f
    move        $s1, $a0
    li.w        $s2, 0
    li.d        $s3, {fault_address}
    li.w        $s4, 1
    la.pcrel    $s5, 1f
    ld.d        $t0, $s3, 0
    b           9f
1:  move        $s3, $s1
    li.w        $s4, 2
    la.pcrel    $s5, 2f
    ld.d        $t0, $s3, 0
    b           9f
2:  li.w        $t0, 2
    bne         $s2, $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
5:  li.w        $t0, 11
    bne         $a0, $t0, 7f
    ld.w        $t1, $a1, 0
    bne         $t1, $t0, 7f
    ld.w        $t1, $a1, 8
    ld.d        $t2, $a2, {saved_s4}
    bne         $t1, $t2, 7f
    ld.d        $t1, $a1, 16
    ld.d        $t2, $a2, {saved_s3}
    bne         $t1, $t2, 7f
    ld.d        $t1, $a2, {saved_s2}
    addi.d      $t1, $t1, 1
    st.d        $t1, $a2, {saved_s2}
7:  ld.d        $t1, $a2, {saved_s5}
    st.d        $t1, $a2, {saved_pc}
    jirl        $zero, $ra, 0
    .global loongarch64_user_test_linux_signal_queue
loongarch64_user_test_linux_signal_queue:
    addi.d      $s4, $sp, -288
    li.d        $t0, 1 << ({queued_signal} - 1)
    st.d        $t0, $s4, 272
    st.d        $zero, $s4, 256
    st.d        $zero, $s4, 264
    st.d        $zero, $s4, 144
    li.w        $a0, 0
    addi.d      $a1, $s4, 272
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
    bnez        $a0, 9f
    li.w        $a7, 172
    syscall     0
    move        $s1, $a0
    li.w        $t0, {queued_signal}
    st.w        $t0, $s4, 128
    li.w        $t0, -1
    st.w        $t0, $s4, 136
    st.w        $s1, $s4, 144
    li.d        $t0, {first_value}
    st.d        $t0, $s4, 152
    move        $a0, $s1
    li.w        $a1, {queued_signal}
    addi.d      $a2, $s4, 128
    li.w        $a7, 138
    syscall     0
    bnez        $a0, 9f
    li.d        $t0, {second_value}
    st.d        $t0, $s4, 152
    move        $a0, $s1
    li.w        $a1, {queued_signal}
    addi.d      $a2, $s4, 128
    li.w        $a7, 138
    syscall     0
    bnez        $a0, 9f
    move        $a0, $s1
    li.w        $a1, {queued_signal}
    li.w        $a7, 129
    syscall     0
    bnez        $a0, 9f
    li.w        $s2, -1
    li.d        $s3, {first_value}
    bl          6f
    li.d        $s3, {second_value}
    bl          6f
    li.w        $s2, 0
    li.w        $s3, 0
    bl          6f
    addi.d      $a0, $s4, 272
    move        $a1, $s4
    addi.d      $a2, $s4, 256
    li.w        $a3, 8
    li.w        $a7, 137
    syscall     0
    li.w        $t0, -11
    bne         $a0, $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
6:  addi.d      $a0, $s4, 272
    move        $a1, $s4
    addi.d      $a2, $s4, 256
    li.w        $a3, 8
    li.w        $a7, 137
    syscall     0
    li.w        $t0, {queued_signal}
    bne         $a0, $t0, 9b
    ld.w        $t1, $s4, 0
    bne         $t1, $t0, 9b
    ld.w        $t1, $s4, 8
    bne         $t1, $s2, 9b
    ld.d        $t1, $s4, 16
    bne         $t1, $s1, 9b
    ld.d        $t1, $s4, 24
    bne         $t1, $s3, 9b
    jirl        $zero, $ra, 0
    .global loongarch64_user_test_linux_signal_suspend
loongarch64_user_test_linux_signal_suspend:
    addi.d      $sp, $sp, -64
    la.pcrel    $t0, 5f
    st.d        $t0, $sp, 0
    li.w        $t0, 4
    st.d        $t0, $sp, 8
    st.d        $zero, $sp, 16
    li.w        $a0, 10
    move        $a1, $sp
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 134
    syscall     0
    bnez        $a0, 9f
    li.w        $t0, 512
    st.d        $t0, $sp, 32
    st.d        $zero, $sp, 40
    li.w        $a0, 0
    addi.d      $a1, $sp, 32
    li.w        $a2, 0
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
    bnez        $a0, 9f
    li.w        $a7, 172
    syscall     0
    move        $a1, $a0
    li.w        $a2, 10
    li.w        $a7, 131
    syscall     0
    bnez        $a0, 9f
    li.w        $s2, 1
    addi.d      $a0, $sp, 40
    li.w        $a1, 8
    li.w        $a7, 133
    syscall     0
    li.w        $t0, -4
    bne         $a0, $t0, 9f
    li.w        $t0, 2
    bne         $s2, $t0, 9f
    li.w        $a0, 0
    li.w        $a1, 0
    addi.d      $a2, $sp, 48
    li.w        $a3, 8
    li.w        $a7, 135
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, 48
    li.w        $t1, 512
    bne         $t0, $t1, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
5:  li.w        $t0, 10
    bne         $a0, $t0, 7f
    ld.d        $t0, $a2, {uc_sigmask}
    li.w        $t1, 512
    bne         $t0, $t1, 7f
    li.w        $t0, 2
    st.d        $t0, $a2, {saved_s2}
7:  jirl        $zero, $ra, 0
    .global loongarch64_user_test_linux_clock
loongarch64_user_test_linux_clock:
    addi.d      $sp, $sp, -64
    li.w        $a0, 1
    move        $a1, $sp
    li.w        $a7, 113
    syscall     0
    bnez        $a0, 9f
    li.w        $t0, {clock_wait}
    st.d        $zero, $sp, 32
    st.d        $t0, $sp, 40
    addi.d      $a0, $sp, 32
    li.w        $a1, 0
    li.w        $a7, 101
    syscall     0
    bnez        $a0, 9f
    li.w        $a0, 1
    addi.d      $a1, $sp, 16
    li.w        $a7, 113
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, 0
    ld.d        $t1, $sp, 8
    ld.d        $t2, $sp, 16
    ld.d        $t3, $sp, 24
    sub.d       $t2, $t2, $t0
    li.w        $t4, 1000000000
    mul.d       $t2, $t2, $t4
    add.d       $t2, $t2, $t3
    sub.d       $t2, $t2, $t1
    ld.d        $t0, $sp, 40
    bltu        $t2, $t0, 9f
    li.w        $a0, 2
    addi.d      $a1, $sp, 16
    li.w        $a7, 113
    syscall     0
    bnez        $a0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_linux_stat
loongarch64_user_test_linux_stat:
    addi.d      $sp, $sp, -272
    st.b        $zero, $sp, 256
    li.w        $a0, 1
    move        $a1, $sp
    li.w        $a7, 80
    syscall     0
    bnez        $a0, 9f
    ld.wu       $t0, $sp, 16
    li.w        $t1, 0xf000
    and         $t0, $t0, $t1
    li.w        $t1, 0x2000
    bne         $t0, $t1, 9f
    ld.d        $t0, $sp, 32
    li.w        $t1, 0x501
    bne         $t0, $t1, 9f
    st.d        $zero, $sp, 32
    li.w        $a0, 1
    addi.d      $a1, $sp, 256
    move        $a2, $sp
    li.w        $a3, 0x1000
    li.w        $a7, 79
    syscall     0
    bnez        $a0, 9f
    ld.d        $t0, $sp, 32
    li.w        $t1, 0x501
    bne         $t0, $t1, 9f
    st.h        $zero, $sp, 28
    st.w        $zero, $sp, 128
    li.w        $a0, 1
    addi.d      $a1, $sp, 256
    li.w        $a2, 0x1000
    li.w        $a3, 0x7ff
    move        $a4, $sp
    li.w        $a7, 291
    syscall     0
    bnez        $a0, 9f
    ld.hu       $t0, $sp, 28
    li.w        $t1, 0xf000
    and         $t0, $t0, $t1
    li.w        $t1, 0x2000
    bne         $t0, $t1, 9f
    ld.wu       $t0, $sp, 128
    li.w        $t1, 5
    bne         $t0, $t1, 9f
    li.w        $a0, 1
    li.w        $a1, 0
    li.w        $a2, 1
    li.w        $a7, 62
    syscall     0
    li.w        $t0, -29
    bne         $a0, $t0, 9f
    li.w        $a0, 3
    move        $a1, $sp
    li.w        $a7, 80
    syscall     0
    li.w        $t0, -9
    bne         $a0, $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_linux_credentials
loongarch64_user_test_linux_credentials:
    li.w        $a7, 175
    syscall     0
    bnez        $a0, 9f
    li.w        $a0, 1000
    li.w        $a7, 146
    syscall     0
    bnez        $a0, 9f
    li.w        $a7, 174
    syscall     0
    li.w        $t0, 1000
    bne         $a0, $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 146
    syscall     0
    li.w        $t0, -1
    bne         $a0, $t0, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_fp_registers
loongarch64_user_test_fp_registers:
    li.w        $a7, {syscall_first}
    syscall     0
    move        $s2, $a0
    li.w        $s3, 0
    movfcsr2gr  $t0, $fcsr0
    or          $s3, $s3, $t0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    movfr2gr.d  $t0, $f\i
    or          $s3, $s3, $t0
    .endr
    andi        $t0, $s2, 3
    slli.d      $t0, $t0, 8
    movgr2fcsr  $fcsr0, $t0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi.d      $t0, $s2, \i
    movgr2fr.d  $f\i, $t0
    .endr
7:  li.w        $a7, {syscall_null}
    syscall     0
    move        $s4, $a0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi.d      $t0, $s2, \i
    movfr2gr.d  $t1, $f\i
    xor         $t1, $t1, $t0
    or          $s3, $s3, $t1
    .endr
    movfcsr2gr  $t0, $fcsr0
    andi        $t1, $s2, 3
    slli.d      $t1, $t1, 8
    xor         $t0, $t0, $t1
    or          $s3, $s3, $t0
    beqz        $s4, 7b
    move        $a0, $s3
    li.w        $a7, {syscall_report}
    syscall     0
8:  b           8b
    .global loongarch64_user_test_illegal_instruction
loongarch64_user_test_illegal_instruction:
    .word       0
    .global loongarch64_user_test_linux_move_and_discard
loongarch64_user_test_linux_move_and_discard:
    li.w        $a0, 0
    li.w        $a1, 4096
    li.w        $a2, 3
    li.w        $a3, 0x22
    li.w        $a4, -1
    li.w        $a5, 0
    li.w        $a7, 222
    syscall     0
    li.d        $t0, -4096
    bgeu        $a0, $t0, 9f
    move        $s3, $a0
    li.w        $a1, 4096
    li.w        $a2, 23
    li.w        $a7, 233
    syscall     0
    bnez        $a0, 9f
    li.w        $t1, 0x12345678
    st.d        $t1, $s3, 0
    li.w        $t2, 0x100000
    sub.d       $s4, $s3, $t2
    move        $a0, $s3
    li.w        $a1, 4096
    li.w        $a2, 4096
    li.w        $a3, 3
    move        $a4, $s4
    li.w        $a7, 216
    syscall     0
    bne         $a0, $s4, 9f
    ld.d        $t2, $s4, 0
    bne         $t2, $t1, 9f
    move        $a0, $s3
    li.w        $a1, 4096
    li.w        $a2, 3
    li.w        $a3, 0x32
    li.w        $a4, -1
    li.w        $a5, 0
    li.w        $a7, 222
    syscall     0
    bne         $a0, $s3, 9f
    ld.d        $t2, $s3, 0
    bnez        $t2, 9f
    move        $a0, $s4
    li.w        $a1, 4096
    li.w        $a2, 4
    li.w        $a7, 233
    syscall     0
    bnez        $a0, 9f
    ld.d        $t2, $s4, 0
    bnez        $t2, 9f
    li.w        $a0, 0
    li.w        $a7, 94
    syscall     0
9:  li.w        $a0, 1
    li.w        $a7, 94
    syscall     0
8:  b           8b
    .global loongarch64_user_test_end
loongarch64_user_test_end:
    .popsection
"#,
    tlbrsave = const csr::TLBRSAVE,
    save7 = const csr::SAVE7,
    pgd = const 0x1b,
    tlbrehi = const csr::TLBREHI,
    page_shift = const 12,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym loongarch64_exception,
    era = const csr::ERA,
    prmd = const csr::PRMD,
    eentry = const csr::EENTRY,
    save0 = const csr::SAVE0,
    prmd_user = const PRMD_USER,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    saved_s2 = const SIGCONTEXT + 8 + 25 * 8,
    saved_s3 = const SIGCONTEXT + 8 + 26 * 8,
    saved_s4 = const SIGCONTEXT + 8 + 27 * 8,
    saved_s5 = const SIGCONTEXT + 8 + 28 * 8,
    saved_pc = const SIGCONTEXT,
    saved_fs0 = const FP_REGISTERS + 24 * 8,
    saved_fs1 = const FP_REGISTERS + 25 * 8,
    saved_fcsr = const FP_REGISTERS + 32 * 8 + 8,
    signal_fp_kept = const TestProgram::LINUX_SIGNAL_FP_KEPT,
    signal_fp_changed = const TestProgram::LINUX_SIGNAL_FP_CHANGED,
    signal_rounding = const 2 << 8,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    uc_sigmask = const 40,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
);

/// `uc_mcontext` in a `ucontext_t`, and the FP registers in its FPU context, the first of the
/// extended contexts after the 272 bytes of `struct sigcontext` and the context's 16-byte header.
const SIGCONTEXT: usize = 176;
const FP_REGISTERS: usize = SIGCONTEXT + 272 + 16;

/// `PRMD` for a return to a program: privilege 3, interrupts enabled.
const PRMD_USER: u64 = 0x7;

/// The registers saved by the kernel entry, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// `$r0` to `$r31` by register number; `r[0]` is unused and `r[3]` is the `sp` at the time
    /// of the exception.
    r: [u64; 32],
    era: u64,
    prmd: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);

/// `ESTAT.Ecode` of the exceptions the HAL tells apart.
pub(super) const ECODE_INTERRUPT: u64 = 0x0;
pub(super) const ECODE_PAGE_INVALID_LOAD: u64 = 0x1;
pub(super) const ECODE_PAGE_INVALID_STORE: u64 = 0x2;
pub(super) const ECODE_PAGE_INVALID_FETCH: u64 = 0x3;
pub(super) const ECODE_PAGE_MODIFY: u64 = 0x4;
pub(super) const ECODE_PAGE_NOT_READABLE: u64 = 0x5;
pub(super) const ECODE_PAGE_NOT_EXECUTABLE: u64 = 0x6;
pub(super) const ECODE_PAGE_PRIVILEGE: u64 = 0x7;
const ECODE_ADDRESS_ERROR: u64 = 0x8;
const ECODE_ALIGNMENT: u64 = 0x9;
const ECODE_SYSCALL: u64 = 0xb;
const ECODE_BREAK: u64 = 0xc;
const ECODE_NO_INSTRUCTION: u64 = 0xd;
const ECODE_PRIVILEGED_INSTRUCTION: u64 = 0xe;
const ECODE_FP_DISABLED: u64 = 0xf;
const ECODE_LSX_DISABLED: u64 = 0x10;
const ECODE_LASX_DISABLED: u64 = 0x11;
const ECODE_FP_EXCEPTION: u64 = 0x12;
const ECODE_WATCHPOINT: u64 = 0x13;
/// The size of every LoongArch instruction.
const INSTRUCTION_SIZE: u64 = 4;

/// `ESTAT`'s exception code.
pub(super) fn ecode(estat: u64) -> u64 {
    (estat >> 16) & 0x3f
}

fn subcode(estat: u64) -> u64 {
    (estat >> 22) & 0x1ff
}

/// Called by the kernel entry with the saved registers.
extern "C" fn loongarch64_exception(frame: &mut TrapFrame) {
    let estat = csr_read!(csr::ESTAT);
    let badv = csr_read!(csr::BADV);
    if ecode(estat) == ECODE_INTERRUPT && interrupt(estat, frame.era) {
        return;
    }
    let exception = decode(estat, badv, frame.era);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.era += INSTRUCTION_SIZE;
    }
}

/// Handles the pending interrupts `estat` names that the kernel takes; returns whether there
/// was one (if not, the interrupt is reported).
#[inline(always)]
fn interrupt(estat: u64, pc: u64) -> bool {
    let devices = estat & interrupts::PIN_INTERRUPT != 0;
    if devices {
        interrupts::handle(pc);
    }
    let (timer_due, wakeup) = timer::pending(estat);
    if timer_due {
        timer::end_timer_interrupt();
        crate::interrupt::deliver(Interrupt::Timer);
    }
    if wakeup {
        timer::end_wakeup();
        crate::interrupt::deliver(Interrupt::Wakeup);
    }
    timer_due || wakeup || devices
}

unsafe extern "C" {
    /// Enters privilege 3 with `context`'s registers; returns once an exception from it saved
    /// them back.
    fn loongarch64_enter_user(context: *mut user::loongarch64::Registers);
    static loongarch64_user_test_start: u8;
    static loongarch64_user_test_checks: u8;
    static loongarch64_user_test_floating_point: u8;
    static loongarch64_user_test_null_syscalls: u8;
    static loongarch64_user_test_spin: u8;
    static loongarch64_user_test_fp_registers: u8;
    static loongarch64_user_test_illegal_instruction: u8;
    static loongarch64_user_test_linux_hello: u8;
    static loongarch64_user_test_linux_start: u8;
    static loongarch64_user_test_linux_start_end: u8;
    static loongarch64_user_test_linux_unknown_syscalls: u8;
    static loongarch64_user_test_stack_growth: u8;
    static loongarch64_user_test_linux_thread_pointer: u8;
    static loongarch64_user_test_linux_poll: u8;
    static loongarch64_user_test_linux_signals: u8;
    static loongarch64_user_test_linux_signal_handler: u8;
    static loongarch64_user_test_linux_fault_handler: u8;
    static loongarch64_user_test_linux_signal_queue: u8;
    static loongarch64_user_test_linux_signal_suspend: u8;
    static loongarch64_user_test_linux_clock: u8;
    static loongarch64_user_test_linux_stat: u8;
    static loongarch64_user_test_linux_credentials: u8;
    static loongarch64_user_test_linux_move_and_discard: u8;
    static loongarch64_user_test_end: u8;
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked (the caller's duty), so nothing but TLB refills, which have
    // their own entry, arrives between the switch of `EENTRY` and the `ertn`; the exception that
    // ends the program saves its registers into the context, which `&mut` keeps valid and
    // unaliased, puts `EENTRY` back and returns here on the kernel stack with the callee-saved
    // registers and `$r21` restored. The program runs at privilege 3, where only user pages are
    // reachable.
    unsafe { loongarch64_enter_user(registers) };
    let estat = csr_read!(csr::ESTAT);
    let badv = csr_read!(csr::BADV);
    match ecode(estat) {
        ECODE_SYSCALL => {
            registers.pc += INSTRUCTION_SIZE;
            return UserExit::Syscall;
        }
        ECODE_INTERRUPT if interrupt(estat, registers.pc) => return UserExit::Interrupt,
        _ => {}
    }
    registers.estat = estat;
    registers.badv = badv;
    UserExit::Exception(decode(estat, badv, registers.pc))
}

impl crate::user::loongarch64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the `ESTAT` and
    /// `BADV` it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.estat, self.badv, self.pc)
    }
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const loongarch64_user_test_start) as usize;
    let end = (&raw const loongarch64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const loongarch64_user_test_checks),
        floating_point: at(&raw const loongarch64_user_test_floating_point),
        null_syscalls: at(&raw const loongarch64_user_test_null_syscalls),
        spin: at(&raw const loongarch64_user_test_spin),
        fp_registers: at(&raw const loongarch64_user_test_fp_registers),
        illegal_instruction: at(&raw const loongarch64_user_test_illegal_instruction),
        linux_hello: at(&raw const loongarch64_user_test_linux_hello),
        linux_start: at(&raw const loongarch64_user_test_linux_start),
        linux_start_end: at(&raw const loongarch64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const loongarch64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const loongarch64_user_test_stack_growth),
        linux_thread_pointer: at(&raw const loongarch64_user_test_linux_thread_pointer),
        linux_poll: at(&raw const loongarch64_user_test_linux_poll),
        linux_signals: at(&raw const loongarch64_user_test_linux_signals),
        linux_signal_handler: at(&raw const loongarch64_user_test_linux_signal_handler),
        linux_fault_handler: at(&raw const loongarch64_user_test_linux_fault_handler),
        linux_signal_queue: at(&raw const loongarch64_user_test_linux_signal_queue),
        linux_signal_suspend: at(&raw const loongarch64_user_test_linux_signal_suspend),
        linux_clock: at(&raw const loongarch64_user_test_linux_clock),
        linux_stat: at(&raw const loongarch64_user_test_linux_stat),
        linux_credentials: at(&raw const loongarch64_user_test_linux_credentials),
        linux_move_and_discard: at(&raw const loongarch64_user_test_linux_move_and_discard),
    }
}

fn decode(estat: u64, badv: u64, pc: u64) -> Exception {
    let (kind, name, has_address) = match ecode(estat) {
        ECODE_INTERRUPT => (ExceptionKind::Interrupt, "interrupt", false),
        ECODE_PAGE_INVALID_LOAD => (ExceptionKind::DataFault, "page invalid (load)", true),
        ECODE_PAGE_INVALID_STORE => (ExceptionKind::DataFault, "page invalid (store)", true),
        ECODE_PAGE_INVALID_FETCH => (ExceptionKind::InstructionFault, "page invalid (fetch)", true),
        ECODE_PAGE_MODIFY => (ExceptionKind::DataFault, "page modify", true),
        ECODE_PAGE_NOT_READABLE => (ExceptionKind::DataFault, "page not readable", true),
        ECODE_PAGE_NOT_EXECUTABLE => (ExceptionKind::InstructionFault, "page not executable", true),
        ECODE_PAGE_PRIVILEGE => (ExceptionKind::DataFault, "page privilege", true),
        ECODE_ADDRESS_ERROR if subcode(estat) == 0 => {
            (ExceptionKind::InstructionFault, "address error (fetch)", true)
        }
        ECODE_ADDRESS_ERROR => (ExceptionKind::DataFault, "address error (memory)", true),
        ECODE_ALIGNMENT => (ExceptionKind::Alignment, "address not aligned", true),
        ECODE_SYSCALL => (ExceptionKind::Other, "syscall", false),
        ECODE_BREAK => (ExceptionKind::Breakpoint, "breakpoint (break)", false),
        ECODE_NO_INSTRUCTION => (ExceptionKind::IllegalInstruction, "instruction not exist", false),
        ECODE_PRIVILEGED_INSTRUCTION => {
            (ExceptionKind::IllegalInstruction, "instruction privilege error", false)
        }
        ECODE_FP_DISABLED => (ExceptionKind::IllegalInstruction, "floating-point disabled", false),
        ECODE_LSX_DISABLED | ECODE_LASX_DISABLED => {
            (ExceptionKind::IllegalInstruction, "vector unit disabled", false)
        }
        ECODE_FP_EXCEPTION => (ExceptionKind::Arithmetic, "floating-point exception", false),
        ECODE_WATCHPOINT => (ExceptionKind::Debug, "watchpoint", true),
        _ => (ExceptionKind::Other, "exception", false),
    };
    Exception {
        kind,
        name,
        code: estat,
        pc,
        address: has_address.then_some(badv),
    }
}
