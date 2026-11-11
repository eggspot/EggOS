//! POWER interrupt entry (Power ISA 3.0B, Book III, chapter 7; D304).
//!
//! With `LPCR.AIL` 3, every interrupt taken with translation on goes to `0xc000...4000` plus
//! its vector, where the boot code copied [`VECTORS_PHYSICAL`]'s block: each vector keeps
//! `r13` in `HSPRG1`, takes this CPU's scratch area from `HSPRG0`, saves `r12` there and
//! jumps to [`ppc64_common_entry`] with the vector number in `r12` (bit 0 set for the
//! interrupts that use `HSRR0`/`HSRR1`). The common entry saves the program's registers into
//! its `UserContext` when the interrupt came from problem state and returns from
//! [`run_user`] (D47); from the kernel it saves every register on the current stack below the
//! red zone, calls [`ppc64_exception`] and returns with `hrfid`.

use super::{msr, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Access, PageFault};
use crate::user::{self, TestProgram, UserContext, UserExit};

/// Where the boot code copies the vector block: AIL 3 adds `0xc000...4000` to each vector.
pub(super) const VECTORS_PHYSICAL: u64 = 0x4000;
/// The vector block's size: the vectors to `0xf80`, then the trampoline.
pub(super) const VECTORS_SIZE: u64 = 0x1100;
/// The most CPUs, by PIR: each has a scratch area of 128 bytes.
pub(super) const MAX_CPUS: usize = 256;
pub(super) const SCRATCH_SHIFT: u32 = 7;

/// Each CPU's exception scratch area, by PIR, in `HSPRG0`: the interrupted `r12`, `ctr`, the
/// common entry's address, the vector, the kernel's TOC and `r13`, the running program's
/// context, the program's FP facility bits, `cr`, `r11`, the interrupted `nip` and `msr`.
#[repr(C, align(128))]
pub(super) struct Scratch([u64; 16]);

pub(super) static mut SCRATCH: [Scratch; MAX_CPUS] = [const { Scratch([0; 16]) }; MAX_CPUS];

const SCRATCH_KERNEL_R13: usize = 5;
const SCRATCH_USER_FP: usize = 7;

core::arch::global_asm!(
    r#"
    .macro VECTOR offset, flag
    .org        ppc64_vectors_start + \offset
    mtspr       305, %r13
    mfspr       %r13, 304
    std         %r12, 0(%r13)
    li          %r12, \offset + \flag
    b           ppc64_vector_trampoline
    .endm

    .pushsection .text.vectors, "ax"
    .balign 4096
    .global ppc64_vectors_start
ppc64_vectors_start:
    VECTOR      0x300, 0
    VECTOR      0x380, 0
    VECTOR      0x400, 0
    VECTOR      0x480, 0
    VECTOR      0x500, 1
    VECTOR      0x600, 0
    VECTOR      0x700, 0
    VECTOR      0x800, 0
    VECTOR      0x900, 0
    VECTOR      0x980, 1
    VECTOR      0xa00, 0
    VECTOR      0xc00, 0
    VECTOR      0xd00, 0
    VECTOR      0xe00, 1
    VECTOR      0xe20, 1
    VECTOR      0xe40, 1
    VECTOR      0xe60, 1
    VECTOR      0xe80, 1
    VECTOR      0xea0, 1
    VECTOR      0xf00, 0
    VECTOR      0xf20, 0
    VECTOR      0xf40, 0
    VECTOR      0xf60, 0
    VECTOR      0xf80, 1
    .org        ppc64_vectors_start + 0x1000
ppc64_vector_trampoline:
    std         %r12, 24(%r13)
    mfctr       %r12
    std         %r12, 8(%r13)
    ld          %r12, 16(%r13)
    mtctr       %r12
    ld          %r12, 24(%r13)
    bctr
    .org        ppc64_vectors_start + {vectors_size}
    .popsection

    .pushsection .text.trap, "ax"
    .balign 128
    .global ppc64_common_entry
ppc64_common_entry:
    std         %r12, 24(%r13)
    std         %r11, 72(%r13)
    mfcr        %r11
    std         %r11, 64(%r13)
    andi.       %r11, %r12, 1
    beq         1f
    mfspr       %r11, 314
    mfspr       %r12, 315
    b           2f
1:  mfspr       %r11, 26
    mfspr       %r12, 27
2:  std         %r11, 80(%r13)
    std         %r12, 88(%r13)
    andi.       %r12, %r12, {problem_state}
    bne         ppc64_user_exit

    mr          %r12, %r1
    addi        %r1, %r1, -(288 + {frame_size})
    std         %r12, 0(%r1)
    std         %r12, (32 + 8)(%r1)
    std         %r0, 32(%r1)
    std         %r2, (32 + 16)(%r1)
    .irp        n, 3,4,5,6,7,8,9,10,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    std         %r\n, (32 + \n * 8)(%r1)
    .endr
    ld          %r11, 72(%r13)
    std         %r11, (32 + 88)(%r1)
    ld          %r11, 0(%r13)
    std         %r11, (32 + 96)(%r1)
    mfspr       %r11, 305
    std         %r11, (32 + 104)(%r1)
    ld          %r11, 80(%r13)
    std         %r11, (32 + 256)(%r1)
    ld          %r11, 88(%r13)
    std         %r11, (32 + 264)(%r1)
    ld          %r11, 8(%r13)
    std         %r11, (32 + 272)(%r1)
    mflr        %r11
    std         %r11, (32 + 280)(%r1)
    mfxer       %r11
    std         %r11, (32 + 288)(%r1)
    ld          %r11, 64(%r13)
    std         %r11, (32 + 296)(%r1)
    ld          %r11, 24(%r13)
    std         %r11, (32 + 304)(%r1)
    mfspr       %r11, 19
    std         %r11, (32 + 312)(%r1)
    mfspr       %r11, 18
    std         %r11, (32 + 320)(%r1)
    li          %r11, {recoverable}
    mtmsrd      %r11, 1
    ld          %r13, 40(%r13)
    addi        %r3, %r1, 32
    bl          {handler}
    nop
    li          %r12, 0
    mtmsrd      %r12, 1
    ld          %r12, (32 + 256)(%r1)
    mtspr       314, %r12
    ld          %r12, (32 + 264)(%r1)
    mtspr       315, %r12
    ld          %r0, (32 + 272)(%r1)
    mtctr       %r0
    ld          %r0, (32 + 280)(%r1)
    mtlr        %r0
    ld          %r0, (32 + 288)(%r1)
    mtxer       %r0
    ld          %r0, (32 + 296)(%r1)
    mtcr        %r0
    ld          %r0, 32(%r1)
    ld          %r2, (32 + 16)(%r1)
    .irp        n, 3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld          %r\n, (32 + \n * 8)(%r1)
    .endr
    ld          %r1, (32 + 8)(%r1)
    hrfid

    // From problem state: the program's registers into its context, then back to
    // `ppc64_enter_user`'s caller on the kernel stack it left.
ppc64_user_exit:
    ld          %r12, 48(%r13)
    std         %r0, 0(%r12)
    std         %r1, 8(%r12)
    std         %r2, 16(%r12)
    .irp        n, 3,4,5,6,7,8,9,10,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    std         %r\n, (\n * 8)(%r12)
    .endr
    ld          %r11, 72(%r13)
    std         %r11, 88(%r12)
    ld          %r11, 0(%r13)
    std         %r11, 96(%r12)
    mfspr       %r11, 305
    std         %r11, 104(%r12)
    ld          %r11, 80(%r13)
    std         %r11, 256(%r12)
    ld          %r11, 88(%r13)
    std         %r11, 264(%r12)
    ld          %r11, 8(%r13)
    std         %r11, 272(%r12)
    mflr        %r11
    std         %r11, 280(%r12)
    mfxer       %r11
    std         %r11, 288(%r12)
    ld          %r11, 64(%r13)
    std         %r11, 296(%r12)
    ld          %r11, 24(%r13)
    std         %r11, 312(%r12)
    mfspr       %r11, 19
    std         %r11, 320(%r12)
    mfspr       %r11, 18
    std         %r11, 328(%r12)
    li          %r11, 0
    std         %r11, 48(%r13)
    li          %r11, {recoverable}
    mtmsrd      %r11, 1
    ld          %r1, 304(%r12)
    ld          %r2, 32(%r13)
    ld          %r13, 40(%r13)
    .irp        n, 14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld          %r\n, (32 + (\n - 14) * 8)(%r1)
    .endr
    ld          %r0, 176(%r1)
    mtcr        %r0
    addi        %r1, %r1, 192
    ld          %r0, 16(%r1)
    mtlr        %r0
    blr

    // `ppc64_enter_user(context)`: keeps the kernel's callee-saved registers on its stack and
    // the stack pointer in the context, names the context in the scratch area, and returns
    // to problem state with the program's registers.
    .global ppc64_enter_user
ppc64_enter_user:
    mflr        %r0
    std         %r0, 16(%r1)
    stdu        %r1, -192(%r1)
    .irp        n, 14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    std         %r\n, (32 + (\n - 14) * 8)(%r1)
    .endr
    mfcr        %r0
    std         %r0, 176(%r1)
    std         %r2, 184(%r1)
    std         %r1, 304(%r3)
    mfspr       %r4, 304
    std         %r3, 48(%r4)
    ld          %r5, 56(%r4)
    lis         %r6, ({msr_user} >> 48) & 0xffff
    ori         %r6, %r6, ({msr_user} >> 32) & 0xffff
    sldi        %r6, %r6, 32
    oris        %r6, %r6, ({msr_user} >> 16) & 0xffff
    ori         %r6, %r6, {msr_user} & 0xffff
    or          %r5, %r5, %r6
    li          %r6, 0
    mtmsrd      %r6, 1
    mtspr       27, %r5
    ld          %r5, 256(%r3)
    mtspr       26, %r5
    ld          %r5, 272(%r3)
    mtctr       %r5
    ld          %r5, 280(%r3)
    mtlr        %r5
    ld          %r5, 288(%r3)
    mtxer       %r5
    ld          %r5, 296(%r3)
    mtcr        %r5
    ld          %r0, 0(%r3)
    ld          %r1, 8(%r3)
    ld          %r2, 16(%r3)
    .irp        n, 4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld          %r\n, (\n * 8)(%r3)
    .endr
    ld          %r3, 24(%r3)
    rfid
    .popsection

    // The user-mode test programs (D47, `TestProgram`): position-independent, no data. Copied
    // to a user page, never run where they are linked. Linux's PowerPC numbers; a call failed
    // when CR0.SO is set, with the error number positive in r3.
    .macro SYS number
    li          %r0, \number
    sc
    .endm
    .macro OK
    bso         9f
    cmpdi       %r3, 0
    bne         9f
    .endm
    .macro FAILED_WITH errno
    bns         9f
    cmpdi       %r3, \errno
    bne         9f
    .endm
    .macro SOBIT reg
    mfcr        \reg
    rlwinm      \reg, \reg, 4, 31, 31
    .endm
    .macro LI64 reg, value
    lis         \reg, ((\value) >> 48) & 0xffff
    ori         \reg, \reg, ((\value) >> 32) & 0xffff
    sldi        \reg, \reg, 32
    oris        \reg, \reg, ((\value) >> 16) & 0xffff
    ori         \reg, \reg, (\value) & 0xffff
    .endm
    .macro EXIT_OK_OR_FAIL
    li          %r3, 0
    SYS         234
9:  li          %r3, 1
    SYS         234
8:  b           8b
    .endm

    .pushsection .rodata.user_test, "a"
    .machine power9
    .balign 4
    .global ppc64_user_test_start
ppc64_user_test_start:
    .global ppc64_user_test_checks
ppc64_user_test_checks:
    li          %r3, 1
    li          %r4, 2
    li          %r5, 3
    li          %r6, 4
    li          %r7, 5
    li          %r8, 6
    li          %r2, 0x102
    .irp        n, 9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    li          %r\n, 0x100 + \n
    .endr
    li          %r0, {syscall_first}
    sc
    addi        %r4, %r4, -2
    addi        %r5, %r5, -3
    or          %r4, %r4, %r5
    addi        %r6, %r6, -4
    or          %r4, %r4, %r6
    addi        %r7, %r7, -5
    or          %r4, %r4, %r7
    addi        %r8, %r8, -6
    or          %r4, %r4, %r8
    addi        %r0, %r0, -{syscall_first}
    or          %r4, %r4, %r0
    addi        %r2, %r2, -0x102
    or          %r4, %r4, %r2
    .irp        n, 9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi        %r\n, %r\n, -(0x100 + \n)
    or          %r4, %r4, %r\n
    .endr
    li          %r0, {syscall_report}
    sc
    ld          %r4, 0(%r3)
1:  b           1b

    .global ppc64_user_test_floating_point
ppc64_user_test_floating_point:
    fmr         0, 1
    .global ppc64_user_test_null_syscalls
ppc64_user_test_null_syscalls:
    li          %r0, {syscall_null}
2:  sc
    b           2b
    .global ppc64_user_test_spin
ppc64_user_test_spin:
3:  b           3b

    .global ppc64_user_test_linux_hello
ppc64_user_test_linux_hello:
    li          %r14, 0
    li          %r3, 1
    bcl         20, 31, 1f
1:  mflr        %r4
    addi        %r4, %r4, (7f - 1b)
    li          %r5, {hello_length}
    SYS         4
    SOBIT       %r15
    or          %r14, %r14, %r15
    addi        %r15, %r3, -{hello_length}
    or          %r14, %r14, %r15
    li          %r3, 1
    li          %r4, 0
    li          %r5, 1
    SYS         4
    SOBIT       %r15
    xori        %r15, %r15, 1
    or          %r14, %r14, %r15
    addi        %r15, %r3, -14
    or          %r14, %r14, %r15
    li          %r0, {unknown_syscall}
    sc
    SOBIT       %r15
    xori        %r15, %r15, 1
    or          %r14, %r14, %r15
    addi        %r15, %r3, -38
    or          %r14, %r14, %r15
    li          %r3, 0
    cmpdi       %r14, 0
    beq         2f
    li          %r3, 1
2:  SYS         234
4:  b           4b
7:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4

    .global ppc64_user_test_linux_start
ppc64_user_test_linux_start:
    andi.       %r9, %r1, 15
    bne         9f
    SYS         20
    mr          %r20, %r3
    SYS         207
    cmpd        %r3, %r20
    bne         9f
    addi        %r3, %r1, -16
    li          %r4, 16
    li          %r5, 0
    SYS         359
    bso         9f
    cmpdi       %r3, 16
    bne         9f
    ld          %r9, -16(%r1)
    ld          %r10, -8(%r1)
    or          %r9, %r9, %r10
    cmpdi       %r9, 0
    beq         9f
    li          %r3, 0
    li          %r4, 3
    li          %r5, 0
    addi        %r6, %r1, -32
    SYS         325
    OK
    ld          %r9, -32(%r1)
    lis         %r10, 0x80
    cmpd        %r9, %r10
    bne         9f
    ld          %r9, -24(%r1)
    cmpdi       %r9, -1
    bne         9f
    li          %r9, 1
    std         %r9, -64(%r1)
    li          %r9, 0
    std         %r9, -56(%r1)
    std         %r9, -48(%r1)
    std         %r9, -40(%r1)
    li          %r3, 13
    addi        %r4, %r1, -64
    li          %r5, 0
    li          %r6, 8
    SYS         173
    OK
    li          %r3, 13
    li          %r4, 0
    addi        %r5, %r1, -112
    li          %r6, 8
    SYS         173
    OK
    ld          %r9, -112(%r1)
    cmpdi       %r9, 1
    bne         9f
    li          %r3, 0
    addi        %r4, %r1, -136
    SYS         185
    OK
    lwz         %r9, -128(%r1)
    cmpwi       %r9, 2
    bne         9f
    ld          %r20, 0(%r1)
    addi        %r21, %r1, 8
    li          %r22, 2
6:  ld          %r4, 0(%r21)
    addi        %r21, %r21, 8
    cmpdi       %r4, 0
    beq         8f
    li          %r5, 0
7:  lbzx        %r9, %r4, %r5
    cmpdi       %r9, 0
    beq         5f
    addi        %r5, %r5, 1
    b           7b
5:  addi        %r1, %r1, -16
    std         %r4, 0(%r1)
    std         %r5, 8(%r1)
    li          %r3, 1
    mr          %r4, %r1
    li          %r5, 1
    SYS         146
    addi        %r1, %r1, 16
    b           6b
8:  addi        %r22, %r22, -1
    cmpdi       %r22, 0
    bne         6b
    mr          %r3, %r20
    SYS         234
9:  li          %r3, 255
    SYS         234
4:  b           4b
    .global ppc64_user_test_linux_start_end
ppc64_user_test_linux_start_end:

    .global ppc64_user_test_linux_unknown_syscalls
ppc64_user_test_linux_unknown_syscalls:
    li          %r0, {unknown_syscall}
5:  sc
    b           5b

    .global ppc64_user_test_stack_growth
ppc64_user_test_stack_growth:
    clrrdi      %r9, %r1, 12
    li          %r10, 4096
6:  std         %r9, 0(%r9)
    sub         %r9, %r9, %r10
    b           6b

    .global ppc64_user_test_linux_thread_pointer
ppc64_user_test_linux_thread_pointer:
    mr          %r20, %r13
    SYS         20
    sldi        %r21, %r3, 12
    mr          %r13, %r21
    li          %r22, {thread_pointer_rounds}
7:  SYS         158
    SOBIT       %r9
    or          %r20, %r20, %r9
    or          %r20, %r20, %r3
    xor         %r9, %r13, %r21
    or          %r20, %r20, %r9
    addi        %r22, %r22, -1
    cmpdi       %r22, 0
    bne         7b
    li          %r3, 0
    cmpdi       %r20, 0
    beq         5f
    li          %r3, 1
5:  SYS         234
8:  b           8b

    .global ppc64_user_test_linux_poll
ppc64_user_test_linux_poll:
    li          %r20, 1
    li          %r9, 0
    std         %r9, -32(%r1)
    li          %r9, 1
    stw         %r9, -24(%r1)
    li          %r9, 4
    sth         %r9, -20(%r1)
    li          %r9, 0
    sth         %r9, -18(%r1)
    li          %r9, 2
    std         %r9, -16(%r1)
    li          %r9, 0
    std         %r9, -48(%r1)
    std         %r9, -40(%r1)
    addi        %r3, %r1, -32
    li          %r4, 3
    addi        %r5, %r1, -48
    li          %r6, 0
    li          %r7, 0
    SYS         281
    bso         9f
    cmpdi       %r3, 1
    bne         9f
    lhz         %r9, -26(%r1)
    cmpdi       %r9, 0
    bne         9f
    lhz         %r9, -18(%r1)
    cmpdi       %r9, 4
    bne         9f
    lhz         %r9, -10(%r1)
    cmpdi       %r9, 0
    bne         9f
    li          %r3, 1
    LI64        %r4, 0x40087468
    addi        %r5, %r1, -64
    SYS         54
    OK
    LI64        %r9, {poll_wait}
    li          %r10, 0
    std         %r10, -48(%r1)
    std         %r9, -40(%r1)
    li          %r3, 0
    li          %r4, 0
    addi        %r5, %r1, -48
    li          %r6, 0
    li          %r7, 0
    SYS         281
    OK
    ld          %r9, -48(%r1)
    ld          %r10, -40(%r1)
    or          %r9, %r9, %r10
    cmpdi       %r9, 0
    bne         9f
    li          %r20, 0
9:  mr          %r3, %r20
    SYS         234
8:  b           8b

    .global ppc64_user_test_linux_signals
ppc64_user_test_linux_signals:
    li          %r3, 0
    li          %r4, 17
    SYS         37
    OK
    li          %r9, 512
    std         %r9, -16(%r1)
    li          %r3, 0
    addi        %r4, %r1, -16
    li          %r5, 0
    li          %r6, 8
    SYS         174
    OK
    SYS         20
    mr          %r4, %r3
    li          %r5, 10
    SYS         250
    OK
    addi        %r3, %r1, -32
    li          %r4, 8
    SYS         175
    OK
    ld          %r9, -32(%r1)
    cmpdi       %r9, 512
    bne         9f
    li          %r3, 1
    addi        %r4, %r1, -16
    li          %r5, 0
    li          %r6, 8
    SYS         174
9:  li          %r3, 1
    SYS         234
8:  b           8b

    .global ppc64_user_test_linux_signal_handler
ppc64_user_test_linux_signal_handler:
    addi        %r9, %r1, -0x2800
    std         %r9, -32(%r1)
    li          %r9, 0
    std         %r9, -24(%r1)
    li          %r9, 0x2000
    std         %r9, -16(%r1)
    addi        %r3, %r1, -32
    li          %r4, 0
    SYS         185
    OK
    bcl         20, 31, 1f
1:  mflr        %r9
    addi        %r9, %r9, (5f - 1b)
    std         %r9, -64(%r1)
    LI64        %r9, 0x08000004
    std         %r9, -56(%r1)
    li          %r9, 0
    std         %r9, -48(%r1)
    std         %r9, -40(%r1)
    li          %r3, 10
    addi        %r4, %r1, -64
    li          %r5, 0
    li          %r6, 8
    SYS         173
    OK
    SYS         20
    mr          %r4, %r3
    li          %r5, 10
    li          %r15, 0x5a5
    li          %r16, 1
    LI64        %r9, {signal_fp_kept}
    mtvsrd      14, %r9
    li          %r9, {signal_rounding}
    mtvsrd      0, %r9
    mtfsf       0xff, 0
    SYS         250
    OK
    cmpdi       %r15, 0x5a5
    bne         9f
    cmpdi       %r16, 2
    bne         9f
    mfvsrd      %r9, 14
    LI64        %r10, {signal_fp_kept}
    cmpd        %r9, %r10
    bne         9f
    mfvsrd      %r9, 15
    LI64        %r10, {signal_fp_changed}
    cmpd        %r9, %r10
    bne         9f
    mffs        0
    mfvsrd      %r9, 0
    andi.       %r9, %r9, 3
    cmpdi       %r9, {signal_rounding}
    bne         9f
    li          %r3, 0
    li          %r4, 0
    addi        %r5, %r1, -72
    li          %r6, 8
    SYS         174
    OK
    ld          %r9, -72(%r1)
    cmpdi       %r9, 0
    bne         9f
    EXIT_OK_OR_FAIL
5:  cmpdi       %r3, 10
    bne         7f
    lwz         %r9, 0(%r4)
    cmpwi       %r9, 10
    bne         7f
    lwa         %r9, 8(%r4)
    cmpdi       %r9, -6
    bne         7f
    ld          %r9, 16(%r5)
    cmpld       %r1, %r9
    blt         7f
    ld          %r10, 32(%r5)
    add         %r9, %r9, %r10
    cmpld       %r1, %r9
    bge         7f
    LI64        %r10, {signal_fp_kept}
    mfvsrd      %r9, 14
    cmpd        %r9, %r10
    bne         7f
    ld          %r9, {saved_f14}(%r5)
    cmpd        %r9, %r10
    bne         7f
    ld          %r9, {saved_fpscr}(%r5)
    andi.       %r9, %r9, 3
    cmpdi       %r9, {signal_rounding}
    bne         7f
    LI64        %r9, {signal_fp_changed}
    std         %r9, {saved_f15}(%r5)
    li          %r9, 0
    mtvsrd      14, %r9
    li          %r9, 2
    std         %r9, {saved_r16}(%r5)
7:  li          %r15, 0
    blr

    .global ppc64_user_test_linux_fault_handler
ppc64_user_test_linux_fault_handler:
    bcl         20, 31, 1f
1:  mflr        %r9
    addi        %r9, %r9, (5f - 1b)
    std         %r9, -64(%r1)
    li          %r9, 4
    std         %r9, -56(%r1)
    li          %r9, 0
    std         %r9, -48(%r1)
    std         %r9, -40(%r1)
    li          %r3, 11
    addi        %r4, %r1, -64
    li          %r5, 0
    li          %r6, 8
    SYS         173
    OK
    li          %r3, 0
    li          %r4, 4096
    li          %r5, 0
    li          %r6, 0x22
    li          %r7, -1
    li          %r8, 0
    SYS         90
    bso         9f
    mr          %r20, %r3
    li          %r16, 0
    LI64        %r17, {fault_address}
    li          %r18, 1
    bcl         20, 31, 2f
2:  mflr        %r19
    addi        %r19, %r19, (3f - 2b)
    ld          %r9, 0(%r17)
    b           9f
3:  mr          %r17, %r20
    li          %r18, 2
    bcl         20, 31, 4f
4:  mflr        %r19
    addi        %r19, %r19, (6f - 4b)
    ld          %r9, 0(%r17)
    b           9f
6:  cmpdi       %r16, 2
    bne         9f
    EXIT_OK_OR_FAIL
5:  cmpdi       %r3, 11
    bne         7f
    lwz         %r9, 0(%r4)
    cmpwi       %r9, 11
    bne         7f
    lwa         %r9, 8(%r4)
    ld          %r10, {saved_r18}(%r5)
    cmpd        %r9, %r10
    bne         7f
    ld          %r9, 16(%r4)
    ld          %r10, {saved_r17}(%r5)
    cmpd        %r9, %r10
    bne         7f
    ld          %r9, {saved_r16}(%r5)
    addi        %r9, %r9, 1
    std         %r9, {saved_r16}(%r5)
7:  ld          %r9, {saved_r19}(%r5)
    std         %r9, {saved_nip}(%r5)
    blr

    .global ppc64_user_test_linux_signal_queue
ppc64_user_test_linux_signal_queue:
    addi        %r24, %r1, -288
    LI64        %r9, 1 << ({queued_signal} - 1)
    std         %r9, 272(%r24)
    li          %r9, 0
    std         %r9, 256(%r24)
    std         %r9, 264(%r24)
    std         %r9, 144(%r24)
    li          %r3, 0
    addi        %r4, %r24, 272
    li          %r5, 0
    li          %r6, 8
    SYS         174
    OK
    SYS         20
    mr          %r20, %r3
    li          %r9, {queued_signal}
    stw         %r9, 128(%r24)
    li          %r9, -1
    stw         %r9, 136(%r24)
    stw         %r20, 144(%r24)
    LI64        %r9, {first_value}
    std         %r9, 152(%r24)
    mr          %r3, %r20
    li          %r4, {queued_signal}
    addi        %r5, %r24, 128
    SYS         177
    OK
    LI64        %r9, {second_value}
    std         %r9, 152(%r24)
    mr          %r3, %r20
    li          %r4, {queued_signal}
    addi        %r5, %r24, 128
    SYS         177
    OK
    mr          %r3, %r20
    li          %r4, {queued_signal}
    SYS         37
    OK
    li          %r21, -1
    LI64        %r22, {first_value}
    bl          6f
    LI64        %r22, {second_value}
    bl          6f
    li          %r21, 0
    li          %r22, 0
    bl          6f
    addi        %r3, %r24, 272
    mr          %r4, %r24
    addi        %r5, %r24, 256
    li          %r6, 8
    SYS         176
    FAILED_WITH 11
    EXIT_OK_OR_FAIL
6:  addi        %r3, %r24, 272
    mr          %r4, %r24
    addi        %r5, %r24, 256
    li          %r6, 8
    SYS         176
    bso         9b
    cmpdi       %r3, {queued_signal}
    bne         9b
    lwz         %r9, 0(%r24)
    cmpwi       %r9, {queued_signal}
    bne         9b
    lwa         %r9, 8(%r24)
    cmpd        %r9, %r21
    bne         9b
    ld          %r9, 16(%r24)
    cmpd        %r9, %r20
    bne         9b
    ld          %r9, 24(%r24)
    cmpd        %r9, %r22
    bne         9b
    blr

    .global ppc64_user_test_linux_signal_suspend
ppc64_user_test_linux_signal_suspend:
    addi        %r1, %r1, -64
    bcl         20, 31, 1f
1:  mflr        %r9
    addi        %r9, %r9, (5f - 1b)
    std         %r9, 0(%r1)
    li          %r9, 4
    std         %r9, 8(%r1)
    li          %r9, 0
    std         %r9, 16(%r1)
    std         %r9, 24(%r1)
    li          %r3, 10
    mr          %r4, %r1
    li          %r5, 0
    li          %r6, 8
    SYS         173
    OK
    li          %r9, 512
    std         %r9, 32(%r1)
    li          %r9, 0
    std         %r9, 40(%r1)
    li          %r3, 0
    addi        %r4, %r1, 32
    li          %r5, 0
    li          %r6, 8
    SYS         174
    OK
    SYS         20
    mr          %r4, %r3
    li          %r5, 10
    SYS         250
    OK
    li          %r16, 1
    addi        %r3, %r1, 40
    li          %r4, 8
    SYS         178
    FAILED_WITH 4
    cmpdi       %r16, 2
    bne         9f
    li          %r3, 0
    li          %r4, 0
    addi        %r5, %r1, 48
    li          %r6, 8
    SYS         174
    OK
    ld          %r9, 48(%r1)
    cmpdi       %r9, 512
    bne         9f
    EXIT_OK_OR_FAIL
5:  cmpdi       %r3, 10
    bne         7f
    ld          %r9, 40(%r5)
    cmpdi       %r9, 512
    bne         7f
    li          %r9, 2
    std         %r9, {saved_r16}(%r5)
7:  blr

    .global ppc64_user_test_linux_clock
ppc64_user_test_linux_clock:
    addi        %r1, %r1, -64
    li          %r3, 1
    mr          %r4, %r1
    SYS         246
    OK
    LI64        %r9, {clock_wait}
    li          %r10, 0
    std         %r10, 32(%r1)
    std         %r9, 40(%r1)
    addi        %r3, %r1, 32
    li          %r4, 0
    SYS         162
    OK
    li          %r3, 1
    addi        %r4, %r1, 16
    SYS         246
    OK
    ld          %r9, 0(%r1)
    ld          %r10, 8(%r1)
    ld          %r11, 16(%r1)
    ld          %r12, 24(%r1)
    sub         %r11, %r11, %r9
    LI64        %r9, 1000000000
    mulld       %r11, %r11, %r9
    add         %r11, %r11, %r12
    sub         %r11, %r11, %r10
    ld          %r9, 40(%r1)
    cmpld       %r11, %r9
    blt         9f
    li          %r3, 2
    addi        %r4, %r1, 16
    SYS         246
    OK
    EXIT_OK_OR_FAIL

    .global ppc64_user_test_linux_stat
ppc64_user_test_linux_stat:
    addi        %r1, %r1, -272
    li          %r9, 0
    stb         %r9, 256(%r1)
    li          %r3, 1
    mr          %r4, %r1
    SYS         108
    OK
    lwz         %r9, 24(%r1)
    andi.       %r9, %r9, 0xf000
    cmpdi       %r9, 0x2000
    bne         9f
    ld          %r9, 40(%r1)
    cmpdi       %r9, 0x501
    bne         9f
    li          %r9, 0
    std         %r9, 40(%r1)
    li          %r3, 1
    addi        %r4, %r1, 256
    mr          %r5, %r1
    li          %r6, 0x1000
    SYS         291
    OK
    ld          %r9, 40(%r1)
    cmpdi       %r9, 0x501
    bne         9f
    li          %r9, 0
    sth         %r9, 28(%r1)
    stw         %r9, 128(%r1)
    li          %r3, 1
    addi        %r4, %r1, 256
    li          %r5, 0x1000
    li          %r6, 0x7ff
    mr          %r7, %r1
    SYS         383
    OK
    lhz         %r9, 28(%r1)
    andi.       %r9, %r9, 0xf000
    cmpdi       %r9, 0x2000
    bne         9f
    lwz         %r9, 128(%r1)
    cmpdi       %r9, 5
    bne         9f
    li          %r3, 1
    li          %r4, 0
    li          %r5, 1
    SYS         19
    FAILED_WITH 29
    li          %r3, 3
    mr          %r4, %r1
    SYS         108
    FAILED_WITH 9
    EXIT_OK_OR_FAIL

    .global ppc64_user_test_linux_credentials
ppc64_user_test_linux_credentials:
    SYS         49
    OK
    li          %r3, 1000
    SYS         23
    OK
    SYS         24
    cmpdi       %r3, 1000
    bne         9f
    li          %r3, 0
    SYS         23
    FAILED_WITH 1
    EXIT_OK_OR_FAIL

    .global ppc64_user_test_fp_registers
ppc64_user_test_fp_registers:
    li          %r0, {syscall_first}
    sc
    mr          %r16, %r3
    li          %r17, 0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    mfvsrd      %r9, \i
    or          %r17, %r17, %r9
    .endr
    mffs        0
    mfvsrd      %r9, 0
    or          %r17, %r17, %r9
    andi.       %r9, %r16, 3
    mtvsrd      0, %r9
    mtfsf       0xff, 0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi        %r9, %r16, \i
    mtvsrd      \i, %r9
    .endr
7:  li          %r0, {syscall_null}
    sc
    mr          %r18, %r3
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    addi        %r9, %r16, \i
    mfvsrd      %r10, \i
    xor         %r10, %r10, %r9
    or          %r17, %r17, %r10
    .endr
    mffs        0
    mfvsrd      %r9, 0
    andi.       %r10, %r16, 3
    xor         %r9, %r9, %r10
    or          %r17, %r17, %r9
    mtvsrd      0, %r16
    cmpdi       %r18, 0
    beq         7b
    mr          %r3, %r17
    li          %r0, {syscall_report}
    sc
8:  b           8b

    .global ppc64_user_test_illegal_instruction
ppc64_user_test_illegal_instruction:
    .long       0

    .global ppc64_user_test_linux_move_and_discard
ppc64_user_test_linux_move_and_discard:
    li          %r3, 0
    li          %r4, 4096
    li          %r5, 3
    li          %r6, 0x22
    li          %r7, -1
    li          %r8, 0
    SYS         90
    bso         9f
    mr          %r19, %r3
    li          %r4, 4096
    li          %r5, 23
    SYS         205
    OK
    lis         %r11, 0x1234
    ori         %r11, %r11, 0x5678
    std         %r11, 0(%r19)
    lis         %r10, 0x10
    sub         %r20, %r19, %r10
    mr          %r3, %r19
    li          %r4, 4096
    li          %r5, 4096
    li          %r6, 3
    mr          %r7, %r20
    SYS         163
    bso         9f
    cmpd        %r3, %r20
    bne         9f
    ld          %r10, 0(%r20)
    cmpd        %r10, %r11
    bne         9f
    mr          %r3, %r19
    li          %r4, 4096
    li          %r5, 3
    li          %r6, 0x32
    li          %r7, -1
    li          %r8, 0
    SYS         90
    bso         9f
    cmpd        %r3, %r19
    bne         9f
    ld          %r10, 0(%r19)
    cmpdi       %r10, 0
    bne         9f
    mr          %r3, %r20
    li          %r4, 4096
    li          %r5, 4
    SYS         205
    OK
    ld          %r10, 0(%r20)
    cmpdi       %r10, 0
    bne         9f
    EXIT_OK_OR_FAIL
    .global ppc64_user_test_end
ppc64_user_test_end:
    .popsection
"#,
    vectors_size = const VECTORS_SIZE,
    problem_state = const msr::PROBLEM_STATE,
    frame_size = const core::mem::size_of::<TrapFrame>() + 32,
    recoverable = const msr::RECOVERABLE,
    handler = sym ppc64_exception,
    msr_user = const msr::USER,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    signal_fp_kept = const TestProgram::LINUX_SIGNAL_FP_KEPT,
    signal_fp_changed = const TestProgram::LINUX_SIGNAL_FP_CHANGED,
    signal_rounding = const 2,
    saved_f14 = const FP_REGS + 14 * 8,
    saved_f15 = const FP_REGS + 15 * 8,
    saved_fpscr = const FP_REGS + 32 * 8,
    saved_r16 = const GP_REGS + 16 * 8,
    saved_r17 = const GP_REGS + 17 * 8,
    saved_r18 = const GP_REGS + 18 * 8,
    saved_r19 = const GP_REGS + 19 * 8,
    saved_nip = const GP_REGS + 32 * 8,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
);

/// `gp_regs` and `fp_regs` in a 64-bit `ucontext_t`: `uc_mcontext` at 168, `gp_regs` 64 into
/// it, `fp_regs` after its 48 words.
const GP_REGS: usize = 168 + 64;
const FP_REGS: usize = GP_REGS + 48 * 8;

/// The registers the kernel entry saves, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    gpr: [u64; 32],
    nip: u64,
    msr: u64,
    ctr: u64,
    lr: u64,
    xer: u64,
    cr: u64,
    /// The vector, bit 0 set when it uses `HSRR0`/`HSRR1`.
    vector: u64,
    dar: u64,
    dsisr: u64,
    padding: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 336);

/// The vectors the HAL tells apart.
const DATA_STORAGE: u64 = 0x300;
const DATA_SEGMENT: u64 = 0x380;
const INSTRUCTION_STORAGE: u64 = 0x400;
const INSTRUCTION_SEGMENT: u64 = 0x480;
const EXTERNAL: u64 = 0x500;
const ALIGNMENT: u64 = 0x600;
const PROGRAM: u64 = 0x700;
const FP_UNAVAILABLE: u64 = 0x800;
const DECREMENTER: u64 = 0x900;
const HYPERVISOR_DECREMENTER: u64 = 0x980;
const DOORBELL: u64 = 0xa00;
const SYSTEM_CALL: u64 = 0xc00;
const TRACE: u64 = 0xd00;
const HYPERVISOR_DATA_STORAGE: u64 = 0xe00;
const HYPERVISOR_INSTRUCTION_STORAGE: u64 = 0xe20;
const EMULATION_ASSISTANCE: u64 = 0xe40;
const HYPERVISOR_DOORBELL: u64 = 0xe80;
const HYPERVISOR_VIRTUALIZATION: u64 = 0xea0;
const VECTOR_UNAVAILABLE: u64 = 0xf20;
const VSX_UNAVAILABLE: u64 = 0xf40;
const FACILITY_UNAVAILABLE: u64 = 0xf60;
const HYPERVISOR_FACILITY_UNAVAILABLE: u64 = 0xf80;

/// `DSISR`: the access was a store; no translation was found.
const DSISR_STORE: u64 = 0x0200_0000;
const DSISR_NO_TRANSLATION: u64 = 0x4000_0000;
/// `SRR1` after an instruction storage interrupt: the cause bits; no translation.
const SRR1_ISI_CAUSE: u64 = 0x5800_0000;
const SRR1_ISI_NO_TRANSLATION: u64 = 0x4000_0000;
/// `SRR1` after a program interrupt: FP enabled exception, illegal, privileged, trap.
const SRR1_PROGRAM_CAUSE: u64 = 0x1f_0000;
const SRR1_FP_ENABLED: u64 = 0x10_0000;
const SRR1_TRAP: u64 = 0x2_0000;
/// The size of every POWER instruction.
const INSTRUCTION_SIZE: u64 = 4;

fn scratch() -> *mut u64 {
    let area: *mut u64;
    // SAFETY: reading `HSPRG0` has no side effects.
    unsafe { core::arch::asm!("mfspr {}, 304", out(reg) area, options(nomem, nostack)) };
    area
}

/// Records `block` as the kernel's `r13`, which the way out of user mode loads again.
pub(super) fn set_kernel_r13(block: usize) {
    // SAFETY: `HSPRG0` holds this CPU's scratch area since boot; only this CPU writes it.
    unsafe { scratch().add(SCRATCH_KERNEL_R13).write_volatile(block as u64) };
}

/// Whether programs on this CPU run with FP, vector and VSX on.
pub(super) fn set_user_fp(on: bool) {
    let bits = if on { msr::USER_FP } else { 0 };
    // SAFETY: as in `set_kernel_r13`.
    unsafe { scratch().add(SCRATCH_USER_FP).write_volatile(bits) };
}

/// Called by the kernel entry with the saved registers.
extern "C" fn ppc64_exception(frame: &mut TrapFrame) {
    let vector = frame.vector & !1;
    if interrupt(vector) {
        return;
    }
    let exception = decode(vector, frame.msr, frame.dar, frame.dsisr, frame.nip);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.nip += INSTRUCTION_SIZE;
    }
}

/// Handles the interrupt `vector` names, if it is one the kernel takes; whether it was.
#[inline(always)]
fn interrupt(vector: u64) -> bool {
    match vector {
        DECREMENTER => {
            timer::end_timer_interrupt();
            crate::interrupt::deliver(Interrupt::Timer);
            true
        }
        HYPERVISOR_DOORBELL | DOORBELL => {
            timer::end_wakeup();
            crate::interrupt::deliver(Interrupt::Wakeup);
            true
        }
        EXTERNAL | HYPERVISOR_VIRTUALIZATION => {
            super::xive::handle();
            true
        }
        HYPERVISOR_DECREMENTER => true,
        _ => false,
    }
}

unsafe extern "C" {
    /// Enters problem state with `context`'s registers; returns once an interrupt from it saved
    /// them back.
    fn ppc64_enter_user(context: *mut user::powerpc64::Registers);
    static ppc64_user_test_start: u8;
    static ppc64_user_test_checks: u8;
    static ppc64_user_test_floating_point: u8;
    static ppc64_user_test_null_syscalls: u8;
    static ppc64_user_test_spin: u8;
    static ppc64_user_test_fp_registers: u8;
    static ppc64_user_test_illegal_instruction: u8;
    static ppc64_user_test_linux_hello: u8;
    static ppc64_user_test_linux_start: u8;
    static ppc64_user_test_linux_start_end: u8;
    static ppc64_user_test_linux_unknown_syscalls: u8;
    static ppc64_user_test_stack_growth: u8;
    static ppc64_user_test_linux_thread_pointer: u8;
    static ppc64_user_test_linux_poll: u8;
    static ppc64_user_test_linux_signals: u8;
    static ppc64_user_test_linux_signal_handler: u8;
    static ppc64_user_test_linux_fault_handler: u8;
    static ppc64_user_test_linux_signal_queue: u8;
    static ppc64_user_test_linux_signal_suspend: u8;
    static ppc64_user_test_linux_clock: u8;
    static ppc64_user_test_linux_stat: u8;
    static ppc64_user_test_linux_credentials: u8;
    static ppc64_user_test_linux_move_and_discard: u8;
    static ppc64_user_test_end: u8;
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel quadrant is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked (the caller's duty), so nothing arrives between the
    // scratch area naming the context and the `rfid`; the interrupt that ends the program
    // saves its registers into the context, which `&mut` keeps valid and unaliased, and
    // returns here on the kernel stack with the callee-saved registers, `r2` and `r13`
    // restored. The program runs in problem state, where only its quadrant is reachable.
    unsafe { ppc64_enter_user(registers) };
    let vector = registers.trap & !1;
    if vector == SYSTEM_CALL {
        return UserExit::Syscall;
    }
    if interrupt(vector) {
        return UserExit::Interrupt;
    }
    UserExit::Exception(decode(vector, registers.msr, registers.dar, registers.dsisr, registers.nip))
}

impl crate::user::powerpc64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the vector,
    /// `SRR1`, `DAR` and `DSISR` it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.trap & !1, self.msr, self.dar, self.dsisr, self.nip)
    }
}

/// [`crate::Platform::page_fault`]: data storage faults by `DSISR`, instruction storage faults
/// by `SRR1`; "no translation" may be a translation the CPU still caches.
pub(super) fn page_fault(code: u64) -> Option<PageFault> {
    let vector = code >> 32;
    let cause = code & 0xffff_ffff;
    match vector {
        DATA_STORAGE => Some(PageFault {
            access: match cause & DSISR_STORE {
                0 => Access::Read,
                _ => Access::Write,
            },
            may_be_stale: cause & DSISR_NO_TRANSLATION != 0,
        }),
        INSTRUCTION_STORAGE => Some(PageFault {
            access: Access::Execute,
            may_be_stale: cause & SRR1_ISI_NO_TRANSLATION != 0,
        }),
        _ => None,
    }
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const ppc64_user_test_start) as usize;
    let end = (&raw const ppc64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const ppc64_user_test_checks),
        floating_point: at(&raw const ppc64_user_test_floating_point),
        null_syscalls: at(&raw const ppc64_user_test_null_syscalls),
        spin: at(&raw const ppc64_user_test_spin),
        fp_registers: at(&raw const ppc64_user_test_fp_registers),
        illegal_instruction: at(&raw const ppc64_user_test_illegal_instruction),
        linux_hello: at(&raw const ppc64_user_test_linux_hello),
        linux_start: at(&raw const ppc64_user_test_linux_start),
        linux_start_end: at(&raw const ppc64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const ppc64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const ppc64_user_test_stack_growth),
        linux_thread_pointer: at(&raw const ppc64_user_test_linux_thread_pointer),
        linux_poll: at(&raw const ppc64_user_test_linux_poll),
        linux_signals: at(&raw const ppc64_user_test_linux_signals),
        linux_signal_handler: at(&raw const ppc64_user_test_linux_signal_handler),
        linux_fault_handler: at(&raw const ppc64_user_test_linux_fault_handler),
        linux_signal_queue: at(&raw const ppc64_user_test_linux_signal_queue),
        linux_signal_suspend: at(&raw const ppc64_user_test_linux_signal_suspend),
        linux_clock: at(&raw const ppc64_user_test_linux_clock),
        linux_stat: at(&raw const ppc64_user_test_linux_stat),
        linux_credentials: at(&raw const ppc64_user_test_linux_credentials),
        linux_move_and_discard: at(&raw const ppc64_user_test_linux_move_and_discard),
    }
}

fn decode(vector: u64, srr1: u64, dar: u64, dsisr: u64, pc: u64) -> Exception {
    let (kind, name, address, cause) = match vector {
        DATA_STORAGE => (ExceptionKind::DataFault, "data storage", Some(dar), dsisr),
        DATA_SEGMENT => (ExceptionKind::Segment, "data segment", Some(dar), 0),
        INSTRUCTION_STORAGE => {
            (ExceptionKind::InstructionFault, "instruction storage", Some(pc), srr1 & SRR1_ISI_CAUSE)
        }
        INSTRUCTION_SEGMENT => (ExceptionKind::Segment, "instruction segment", Some(pc), 0),
        ALIGNMENT => (ExceptionKind::Alignment, "alignment", Some(dar), dsisr),
        PROGRAM if srr1 & SRR1_TRAP != 0 => {
            (ExceptionKind::Breakpoint, "trap", None, srr1 & SRR1_PROGRAM_CAUSE)
        }
        PROGRAM if srr1 & SRR1_FP_ENABLED != 0 => (
            ExceptionKind::Arithmetic,
            "floating-point enabled exception",
            None,
            srr1 & SRR1_PROGRAM_CAUSE,
        ),
        PROGRAM => (
            ExceptionKind::IllegalInstruction,
            "illegal or privileged instruction",
            None,
            srr1 & SRR1_PROGRAM_CAUSE,
        ),
        FP_UNAVAILABLE => (ExceptionKind::IllegalInstruction, "floating point unavailable", None, 0),
        VECTOR_UNAVAILABLE => (ExceptionKind::IllegalInstruction, "vector unavailable", None, 0),
        VSX_UNAVAILABLE => (ExceptionKind::IllegalInstruction, "VSX unavailable", None, 0),
        FACILITY_UNAVAILABLE | HYPERVISOR_FACILITY_UNAVAILABLE => {
            (ExceptionKind::IllegalInstruction, "facility unavailable", None, 0)
        }
        EMULATION_ASSISTANCE => {
            (ExceptionKind::IllegalInstruction, "hypervisor emulation assistance", None, 0)
        }
        TRACE => (ExceptionKind::Debug, "trace", None, 0),
        HYPERVISOR_DATA_STORAGE => (ExceptionKind::DataFault, "hypervisor data storage", Some(dar), dsisr),
        HYPERVISOR_INSTRUCTION_STORAGE => {
            (ExceptionKind::InstructionFault, "hypervisor instruction storage", Some(pc), 0)
        }
        SYSTEM_CALL => (ExceptionKind::Other, "system call", None, 0),
        DECREMENTER | HYPERVISOR_DECREMENTER | EXTERNAL | HYPERVISOR_VIRTUALIZATION
        | HYPERVISOR_DOORBELL | DOORBELL => (ExceptionKind::Interrupt, "interrupt", None, 0),
        _ => (ExceptionKind::Other, "exception", None, 0),
    };
    Exception {
        kind,
        name,
        code: vector << 32 | cause,
        pc,
        address,
    }
}
