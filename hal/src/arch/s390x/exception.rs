//! s390x interruptions (D321). Each class's new PSW (in the lowcore) enters a stub in real
//! mode (DAT off), so it reaches the lowcore with base register 0 whatever the address space:
//! the stub saves the general registers and the old PSW there, then either loads the kernel's
//! continuation PSW (DAT on, home space), which moves them to a frame on the kernel stack and
//! calls [`s390x_exception`], or, for an interruption in problem state, copies them into the
//! program's `UserContext` and loads the PSW that returns from [`run_user`] (D47). The way back
//! to an interrupted PSW goes through real mode too, so the last `lmg` needs no base register.
//!
//! Programs run in the primary space through a per-CPU root that holds the user half's root
//! entries alone (D320); the kernel runs in the home space through the process's whole root. A
//! region-translation exception from a program whose root entry the whole root has gained
//! since the copy refreshes it and runs the program again.

use super::{DIRECT_MAP_OFFSET, KERNEL_VIRTUAL_OFFSET, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Access, Format, PageFault};
use crate::paging::s390x::{ASCE_REGION_SECOND, S390x};
use crate::user::{self, TestProgram, UserContext, UserExit};

/// The lowcore's architected fields and the HAL's own, from 0x200 on (real addresses).
pub(super) mod lowcore {
    pub const EXTERNAL_CODE: u64 = 0x86;
    pub const SVC_CODE: u64 = 0x8a;
    pub const PROGRAM_CODE: u64 = 0x8e;
    pub const TEID: u64 = 0xa8;
    pub const EXTERNAL_OLD: u64 = 0x130;
    pub const SVC_OLD: u64 = 0x140;
    pub const PROGRAM_OLD: u64 = 0x150;
    pub const MACHINE_CHECK_OLD: u64 = 0x160;
    pub const IO_OLD: u64 = 0x170;
    pub const RESTART_NEW: u64 = 0x1a0;
    pub const EXTERNAL_NEW: u64 = 0x1b0;
    pub const SVC_NEW: u64 = 0x1c0;
    pub const PROGRAM_NEW: u64 = 0x1d0;
    pub const MACHINE_CHECK_NEW: u64 = 0x1e0;
    pub const IO_NEW: u64 = 0x1f0;
    pub const SAVE: u64 = 0x200;
    pub const OLD_PSW: u64 = 0x280;
    pub const CLASS: u64 = 0x290;
    pub const CODE: u64 = 0x298;
    pub const TEID_COPY: u64 = 0x2a0;
    pub const KERNEL_ENTRY_PSW: u64 = 0x2c0;
    pub const USER_PSW: u64 = 0x2d0;
    pub const TO_USER_PSW: u64 = 0x2e0;
    pub const KERNEL_RETURN_PSW: u64 = 0x2f0;
    pub const CPU_LOCAL: u64 = 0x300;
    pub const CONTEXT: u64 = 0x308;
    pub const KERNEL_SP: u64 = 0x310;
    pub const CONTEXT_PHYSICAL: u64 = 0x318;
    pub const USER_EXIT_PSW: u64 = 0x320;
    pub const CPU_START: u64 = 0x330;
    /// The bytes of the lowcore a new CPU's copy takes from the boot CPU's: the new PSWs and
    /// the HAL's continuation PSWs.
    pub const COPIED: core::ops::Range<u64> = 0x1a0..0x330;
}

/// Interruption classes, as the stubs record them.
const EXTERNAL: u64 = 1;
const SVC: u64 = 2;
const PROGRAM: u64 = 3;
const IO: u64 = 4;

/// PSW mask bits.
pub(super) mod psw {
    pub const DAT: u64 = 1 << 58;
    pub const IO: u64 = 1 << 57;
    pub const EXTERNAL: u64 = 1 << 56;
    pub const WAIT: u64 = 1 << 49;
    pub const PROBLEM: u64 = 1 << 48;
    pub const HOME_SPACE: u64 = 3 << 46;
    pub const EXTENDED: u64 = 1 << 32;
    pub const BASIC: u64 = 1 << 31;
    /// The kernel: DAT on in the home space, interruptions off, 64-bit addressing.
    pub const KERNEL: u64 = DAT | HOME_SPACE | EXTENDED | BASIC;
    /// Real mode, interruptions off, 64-bit addressing: the stubs'.
    pub const REAL: u64 = EXTENDED | BASIC;
}

core::arch::global_asm!(
    r#"
    .pushsection .text.interrupts, "ax"
    .balign 4096
    .global s390x_external_stub
s390x_external_stub:
    stmg        %r0, %r15, {save}
    mvc         {old_psw}(16), {external_old}
    mvghi       {class}, {external}
    llgh        %r1, {external_code}
    stg         %r1, {code}
    j           s390x_common_stub
    .global s390x_svc_stub
s390x_svc_stub:
    stmg        %r0, %r15, {save}
    mvc         {old_psw}(16), {svc_old}
    mvghi       {class}, {svc}
    llgh        %r1, {svc_code}
    stg         %r1, {code}
    j           s390x_common_stub
    .global s390x_program_stub
s390x_program_stub:
    stmg        %r0, %r15, {save}
    mvc         {old_psw}(16), {program_old}
    mvghi       {class}, {program}
    llgh        %r1, {program_code}
    stg         %r1, {code}
    mvc         {teid_copy}(8), {teid}
    j           s390x_common_stub
    .global s390x_io_stub
s390x_io_stub:
    stmg        %r0, %r15, {save}
    mvc         {old_psw}(16), {io_old}
    mvghi       {class}, {io}
    mvghi       {code}, 0
    j           s390x_common_stub
    .global s390x_machine_check_stub
s390x_machine_check_stub:
    lpswe       {machine_check_old}

s390x_common_stub:
    tm          {old_psw} + 1, 0x01
    jnz         1f
    lpswe       {kernel_entry_psw}
1:  lg          %r1, {context_physical}
    mvc         0(128, %r1), {save}
    mvc         128(16, %r1), {old_psw}
    stam        %a0, %a15, 144(%r1)
    mvc         216(24, %r1), {class}
    lg          %r15, {kernel_sp}
    lpswe       {user_exit_psw}

    # Real mode, from `s390x_kernel_entry`'s way out: the frame's registers and PSW are in the
    # lowcore.
    .global s390x_kernel_return_real
s390x_kernel_return_real:
    lmg         %r0, %r15, {save}
    lpswe       {old_psw}

    # Real mode, from `s390x_enter_user`: r3 is the context's real address.
    .global s390x_to_user_real
s390x_to_user_real:
    lam         %a0, %a15, 144(%r3)
    lmg         %r0, %r15, 0(%r3)
    lpswe       {user_psw}
    .popsection

    .pushsection .text.trap, "ax"
    .balign 4
    .global s390x_kernel_entry
s390x_kernel_entry:
    aghi        %r15, -{frame}
    llihh       %r1, {direct_map_high}
    mvc         160(128, %r15), {save}(%r1)
    mvc         288(16, %r15), {old_psw}(%r1)
    mvc         304(24, %r15), {class}(%r1)
    la          %r2, 160(%r15)
    brasl       %r14, {handler}
    llihh       %r1, {direct_map_high}
    mvc         {save}(128, %r1), 160(%r15)
    mvc         {old_psw}(16, %r1), 288(%r15)
    lpswe       {kernel_return_psw}(%r1)

    # `s390x_enter_user(context)`: keeps the kernel's r6 to r15 in the caller's save area and
    # the stack pointer and the context in the lowcore, puts the program's PSW there, and goes
    # through real mode to load its registers.
    .global s390x_enter_user
s390x_enter_user:
    stmg        %r6, %r15, 48(%r15)
    llihh       %r1, {direct_map_high}
    stg         %r15, {kernel_sp}(%r1)
    stg         %r2, {context}(%r1)
    lgr         %r3, %r2
    llihh       %r4, {direct_map_high}
    sgr         %r3, %r4
    stg         %r3, {context_physical}(%r1)
    mvc         {user_psw}(16, %r1), 128(%r2)
    lpswe       {to_user_psw}(%r1)

    # Back from problem state, in the home space: r15 is the stack `s390x_enter_user` left.
    .global s390x_user_exit
s390x_user_exit:
    lmg         %r6, %r15, 48(%r15)
    br          %r14
    .popsection

    .macro SYS number
    lghi        %r1, \number
    svc         0
    .endm
    .macro OK
    ltgr        %r2, %r2
    jne         9f
    .endm
    .macro FAILED_WITH errno
    cghi        %r2, -\errno
    jne         9f
    .endm
    .macro LI64 reg, value
    llihf       \reg, ((\value) >> 32) & 0xffffffff
    oilf        \reg, (\value) & 0xffffffff
    .endm
    .macro EXPECT reg, value
    LI64        %r0, \value
    cgr         \reg, %r0
    jne         9f
    .endm
    .macro READ_TP reg
    lghi        \reg, 0
    ear         \reg, %a0
    sllg        \reg, \reg, 32
    lghi        %r0, 0
    ear         %r0, %a1
    ogr         \reg, %r0
    .endm
    .macro EXIT_OK_OR_FAIL
    lghi        %r2, 0
    SYS         248
9:  lghi        %r2, 1
    SYS         248
8:  j           8b
    .endm

    .pushsection .rodata.user_test, "a"
    .balign 4
    .global s390x_user_test_start
s390x_user_test_start:
    .global s390x_user_test_checks
s390x_user_test_checks:
    lghi        %r2, 1
    lghi        %r3, 2
    lghi        %r4, 3
    lghi        %r5, 4
    lghi        %r6, 5
    lghi        %r7, 6
    .irp        n, 0,8,9,10,11,12,13,14
    lghi        %r\n, 0x100 + \n
    .endr
    lghi        %r1, {syscall_first}
    svc         0
    lgr         %r6, %r2
    lghi        %r3, 0
    .irp        n, 0,8,9,10,11,12,13,14
    aghi        %r\n, -(0x100 + \n)
    ogr         %r3, %r\n
    .endr
    lgr         %r2, %r6
    lghi        %r1, {syscall_report}
    svc         0
    lg          %r3, 0(%r2)
1:  j           1b
    .global s390x_user_test_floating_point
s390x_user_test_floating_point:
    lzdr        %f8
    .global s390x_user_test_null_syscalls
s390x_user_test_null_syscalls:
    lghi        %r1, {syscall_null}
2:  svc         0
    j           2b
    .global s390x_user_test_spin
s390x_user_test_spin:
3:  j           3b
    .global s390x_user_test_linux_hello
s390x_user_test_linux_hello:
    lghi        %r2, 1
    larl        %r3, 9f
    lghi        %r4, {hello_length}
    SYS         4
    lghi        %r6, 0
    cghi        %r2, {hello_length}
    je          1f
    lghi        %r6, 1
1:  lghi        %r2, 1
    lghi        %r3, 0
    lghi        %r4, 1
    SYS         4
    cghi        %r2, -14
    je          2f
    lghi        %r6, 1
2:  SYS         {unknown_syscall}
    cghi        %r2, -38
    je          3f
    lghi        %r6, 1
3:  lgr         %r2, %r6
    SYS         248
4:  j           4b
9:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4

    .global s390x_user_test_linux_start
s390x_user_test_linux_start:
    lgr         %r10, %r15
    tmll        %r15, 15
    jne         9f
    aghi        %r15, -160
    SYS         20
    lgr         %r6, %r2
    SYS         236
    cgr         %r2, %r6
    jne         9f
    la          %r2, 0(%r15)
    lghi        %r3, 16
    lghi        %r4, 0
    SYS         349
    cghi        %r2, 16
    jne         9f
    lg          %r0, 0(%r15)
    og          %r0, 8(%r15)
    ltgr        %r0, %r0
    je          9f
    lghi        %r2, 0
    lghi        %r3, 3
    lghi        %r4, 0
    la          %r5, 16(%r15)
    SYS         334
    OK
    lg          %r1, 16(%r15)
    EXPECT      %r1, 0x800000
    lg          %r1, 24(%r15)
    EXPECT      %r1, -1
    mvghi       32(%r15), 1
    mvghi       40(%r15), 0
    mvghi       48(%r15), 0
    mvghi       56(%r15), 0
    lghi        %r2, 13
    la          %r3, 32(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         174
    OK
    lghi        %r2, 13
    lghi        %r3, 0
    la          %r4, 64(%r15)
    lghi        %r5, 8
    SYS         174
    OK
    lg          %r1, 64(%r15)
    EXPECT      %r1, 1
    lghi        %r2, 0
    la          %r3, 96(%r15)
    SYS         186
    OK
    lgf         %r1, 104(%r15)
    EXPECT      %r1, 2
    lg          %r6, 0(%r10)
    la          %r7, 8(%r10)
    lghi        %r8, 2
6:  lg          %r3, 0(%r7)
    aghi        %r7, 8
    ltgr        %r3, %r3
    je          8f
    lghi        %r4, 0
7:  la          %r1, 0(%r3,%r4)
    cli         0(%r1), 0
    je          5f
    aghi        %r4, 1
    j           7b
5:  stg         %r3, 128(%r15)
    stg         %r4, 136(%r15)
    lghi        %r2, 1
    la          %r3, 128(%r15)
    lghi        %r4, 1
    SYS         146
    j           6b
8:  brctg       %r8, 6b
    lgr         %r2, %r6
    SYS         248
9:  lghi        %r2, 255
    SYS         248
4:  j           4b
    .global s390x_user_test_linux_start_end
s390x_user_test_linux_start_end:

    .global s390x_user_test_linux_unknown_syscalls
s390x_user_test_linux_unknown_syscalls:
    lghi        %r1, {unknown_syscall}
5:  svc         0
    j           5b

    .global s390x_user_test_linux_thread_pointer
s390x_user_test_linux_thread_pointer:
    READ_TP     %r6
    SYS         20
    sllg        %r7, %r2, 12
    sar         %a1, %r7
    srlg        %r1, %r7, 32
    sar         %a0, %r1
    lghi        %r8, {thread_pointer_rounds}
7:  SYS         158
    ogr         %r6, %r2
    READ_TP     %r1
    xgr         %r1, %r7
    ogr         %r6, %r1
    brctg       %r8, 7b
    lghi        %r2, 0
    ltgr        %r6, %r6
    je          5f
    lghi        %r2, 1
5:  SYS         248
8:  j           8b

    .global s390x_user_test_linux_poll
s390x_user_test_linux_poll:
    aghi        %r15, -64
    mvghi       0(%r15), 0
    mvhi        8(%r15), 1
    mvhhi       12(%r15), 4
    mvhhi       14(%r15), 0
    mvhi        16(%r15), 2
    mvhi        20(%r15), 0
    mvghi       32(%r15), 0
    mvghi       40(%r15), 0
    la          %r2, 0(%r15)
    lghi        %r3, 3
    la          %r4, 32(%r15)
    lghi        %r5, 0
    lghi        %r6, 0
    SYS         302
    cghi        %r2, 1
    jne         9f
    llgh        %r1, 6(%r15)
    EXPECT      %r1, 0
    llgh        %r1, 14(%r15)
    EXPECT      %r1, 4
    llgh        %r1, 22(%r15)
    EXPECT      %r1, 0
    lghi        %r2, 1
    lghi        %r3, 0x5413
    la          %r4, 48(%r15)
    SYS         54
    OK
    mvghi       32(%r15), 0
    LI64        %r1, {poll_wait}
    stg         %r1, 40(%r15)
    lghi        %r2, 0
    lghi        %r3, 0
    la          %r4, 32(%r15)
    lghi        %r5, 0
    lghi        %r6, 0
    SYS         302
    OK
    lg          %r1, 32(%r15)
    og          %r1, 40(%r15)
    EXPECT      %r1, 0
    EXIT_OK_OR_FAIL

    .global s390x_user_test_linux_signals
s390x_user_test_linux_signals:
    aghi        %r15, -64
    lghi        %r2, 0
    lghi        %r3, 17
    SYS         37
    OK
    mvghi       0(%r15), 512
    lghi        %r2, 0
    la          %r3, 0(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         175
    OK
    SYS         20
    lgr         %r3, %r2
    lghi        %r4, 10
    SYS         241
    OK
    la          %r2, 16(%r15)
    lghi        %r3, 8
    SYS         176
    OK
    lg          %r1, 16(%r15)
    EXPECT      %r1, 512
    lghi        %r2, 1
    la          %r3, 0(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         175
9:  lghi        %r2, 1
    SYS         248
8:  j           8b

    .global s390x_user_test_linux_signal_handler
s390x_user_test_linux_signal_handler:
    aghi        %r15, -128
    lay         %r1, -0x2800(%r15)
    stg         %r1, 0(%r15)
    mvghi       8(%r15), 0
    mvghi       16(%r15), 0x2000
    la          %r2, 0(%r15)
    lghi        %r3, 0
    SYS         186
    OK
    larl        %r1, 5f
    stg         %r1, 32(%r15)
    LI64        %r1, 0x08000004
    stg         %r1, 40(%r15)
    mvghi       48(%r15), 0
    mvghi       56(%r15), 0
    lghi        %r2, 10
    la          %r3, 32(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         174
    OK
    SYS         20
    lgr         %r3, %r2
    lghi        %r4, 10
    lghi        %r7, 0x5a5
    lghi        %r6, 1
    LI64        %r1, {signal_fp_kept}
    ldgr        %f8, %r1
    lghi        %r1, {signal_rounding}
    sfpc        %r1
    SYS         241
    OK
    EXPECT      %r7, 0x5a5
    EXPECT      %r6, 2
    lgdr        %r1, %f8
    EXPECT      %r1, {signal_fp_kept}
    lgdr        %r1, %f9
    EXPECT      %r1, {signal_fp_changed}
    lghi        %r1, 0
    efpc        %r1
    lghi        %r0, 3
    ngr         %r1, %r0
    EXPECT      %r1, {signal_rounding}
    lghi        %r2, 0
    lghi        %r3, 0
    la          %r4, 64(%r15)
    lghi        %r5, 8
    SYS         175
    OK
    lg          %r1, 64(%r15)
    EXPECT      %r1, 0
    EXIT_OK_OR_FAIL
5:  cghi        %r2, 10
    jne         7f
    lgf         %r1, 0(%r3)
    cghi        %r1, 10
    jne         7f
    lgf         %r1, 8(%r3)
    cghi        %r1, -6
    jne         7f
    lg          %r1, {uc_stack_sp}(%r4)
    clgr        %r15, %r1
    jl          7f
    ag          %r1, {uc_stack_size}(%r4)
    clgr        %r15, %r1
    jhe         7f
    LI64        %r0, {signal_fp_kept}
    lgdr        %r1, %f8
    cgr         %r1, %r0
    jne         7f
    cg          %r0, {saved_f8}(%r4)
    jne         7f
    llgf        %r1, {saved_fpc}(%r4)
    lghi        %r0, 3
    ngr         %r1, %r0
    cghi        %r1, {signal_rounding}
    jne         7f
    LI64        %r1, {signal_fp_changed}
    stg         %r1, {saved_f9}(%r4)
    lghi        %r1, 0
    ldgr        %f8, %r1
    mvghi       {saved_r6}(%r4), 2
7:  lghi        %r7, 0
    br          %r14

    .global s390x_user_test_linux_fault_handler
s390x_user_test_linux_fault_handler:
    aghi        %r15, -96
    larl        %r1, 5f
    stg         %r1, 0(%r15)
    mvghi       8(%r15), 4
    mvghi       16(%r15), 0
    mvghi       24(%r15), 0
    lghi        %r2, 11
    la          %r3, 0(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         174
    OK
    mvghi       32(%r15), 0
    mvghi       40(%r15), 4096
    mvghi       48(%r15), 0
    mvghi       56(%r15), 0x22
    mvghi       64(%r15), -1
    mvghi       72(%r15), 0
    la          %r2, 32(%r15)
    SYS         90
    lghi        %r0, -4096
    clgr        %r2, %r0
    jhe         9f
    lgr         %r10, %r2
    lghi        %r6, 0
    LI64        %r7, {fault_address}
    lghi        %r8, 1
    larl        %r9, 3f
    lg          %r1, 0(%r7)
    j           9f
3:  lgr         %r7, %r10
    lghi        %r8, 2
    larl        %r9, 6f
    lg          %r1, 0(%r7)
    j           9f
6:  EXPECT      %r6, 2
    EXIT_OK_OR_FAIL
5:  cghi        %r2, 11
    jne         7f
    lgf         %r1, 0(%r3)
    cghi        %r1, 11
    jne         7f
    lgf         %r1, 8(%r3)
    cg          %r1, {saved_r8}(%r4)
    jne         7f
    lg          %r1, 16(%r3)
    cg          %r1, {saved_r7}(%r4)
    jne         7f
    lg          %r1, {saved_r6}(%r4)
    aghi        %r1, 1
    stg         %r1, {saved_r6}(%r4)
7:  lg          %r1, {saved_r9}(%r4)
    stg         %r1, {saved_pc}(%r4)
    br          %r14

    .global s390x_user_test_linux_signal_queue
s390x_user_test_linux_signal_queue:
    aghi        %r15, -320
    lgr         %r12, %r15
    LI64        %r1, 1 << ({queued_signal} - 1)
    stg         %r1, 272(%r12)
    mvghi       256(%r12), 0
    mvghi       264(%r12), 0
    lghi        %r2, 0
    la          %r3, 272(%r12)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         175
    OK
    SYS         20
    lgr         %r6, %r2
    mvhi        128(%r12), {queued_signal}
    mvhi        132(%r12), 0
    mvhi        136(%r12), -1
    mvhi        140(%r12), 0
    st          %r6, 144(%r12)
    mvhi        148(%r12), 0
    LI64        %r1, {first_value}
    stg         %r1, 152(%r12)
    lgr         %r2, %r6
    lghi        %r3, {queued_signal}
    la          %r4, 128(%r12)
    SYS         178
    OK
    LI64        %r1, {second_value}
    stg         %r1, 152(%r12)
    lgr         %r2, %r6
    lghi        %r3, {queued_signal}
    la          %r4, 128(%r12)
    SYS         178
    OK
    lgr         %r2, %r6
    lghi        %r3, {queued_signal}
    SYS         37
    OK
    lghi        %r7, -1
    LI64        %r8, {first_value}
    brasl       %r14, 6f
    LI64        %r8, {second_value}
    brasl       %r14, 6f
    lghi        %r7, 0
    lghi        %r8, 0
    brasl       %r14, 6f
    la          %r2, 272(%r12)
    la          %r3, 0(%r12)
    la          %r4, 256(%r12)
    lghi        %r5, 8
    SYS         177
    FAILED_WITH 11
    EXIT_OK_OR_FAIL
6:  la          %r2, 272(%r12)
    la          %r3, 0(%r12)
    la          %r4, 256(%r12)
    lghi        %r5, 8
    SYS         177
    cghi        %r2, {queued_signal}
    jne         9b
    lgf         %r1, 0(%r12)
    cghi        %r1, {queued_signal}
    jne         9b
    lgf         %r1, 8(%r12)
    cgr         %r1, %r7
    jne         9b
    lgf         %r1, 16(%r12)
    cgr         %r1, %r6
    jne         9b
    lg          %r1, 24(%r12)
    cgr         %r1, %r8
    jne         9b
    br          %r14

    .global s390x_user_test_linux_signal_suspend
s390x_user_test_linux_signal_suspend:
    aghi        %r15, -96
    larl        %r1, 5f
    stg         %r1, 0(%r15)
    mvghi       8(%r15), 4
    mvghi       16(%r15), 0
    mvghi       24(%r15), 0
    lghi        %r2, 10
    la          %r3, 0(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         174
    OK
    mvghi       32(%r15), 512
    mvghi       40(%r15), 0
    lghi        %r2, 0
    la          %r3, 32(%r15)
    lghi        %r4, 0
    lghi        %r5, 8
    SYS         175
    OK
    SYS         20
    lgr         %r3, %r2
    lghi        %r4, 10
    SYS         241
    OK
    lghi        %r6, 1
    la          %r2, 40(%r15)
    lghi        %r3, 8
    SYS         179
    FAILED_WITH 4
    EXPECT      %r6, 2
    lghi        %r2, 0
    lghi        %r3, 0
    la          %r4, 48(%r15)
    lghi        %r5, 8
    SYS         175
    OK
    lg          %r1, 48(%r15)
    EXPECT      %r1, 512
    EXIT_OK_OR_FAIL
5:  cghi        %r2, 10
    jne         7f
    lg          %r1, {uc_sigmask}(%r4)
    cghi        %r1, 512
    jne         7f
    mvghi       {saved_r6}(%r4), 2
7:  br          %r14

    .global s390x_user_test_linux_clock
s390x_user_test_linux_clock:
    aghi        %r15, -64
    lghi        %r2, 1
    la          %r3, 0(%r15)
    SYS         260
    OK
    mvghi       32(%r15), 0
    LI64        %r1, {clock_wait}
    stg         %r1, 40(%r15)
    la          %r2, 32(%r15)
    lghi        %r3, 0
    SYS         162
    OK
    lghi        %r2, 1
    la          %r3, 16(%r15)
    SYS         260
    OK
    lg          %r1, 16(%r15)
    sg          %r1, 0(%r15)
    LI64        %r0, 1000000000
    msgr        %r1, %r0
    ag          %r1, 24(%r15)
    sg          %r1, 8(%r15)
    clg         %r1, 40(%r15)
    jl          9f
    lghi        %r2, 2
    la          %r3, 16(%r15)
    SYS         260
    OK
    EXIT_OK_OR_FAIL

    .global s390x_user_test_linux_stat
s390x_user_test_linux_stat:
    aghi        %r15, -272
    mvi         256(%r15), 0
    lghi        %r2, 1
    la          %r3, 0(%r15)
    SYS         108
    OK
    llgf        %r1, 24(%r15)
    nill        %r1, 0xf000
    EXPECT      %r1, 0x2000
    lg          %r1, 40(%r15)
    EXPECT      %r1, 0x501
    mvghi       40(%r15), 0
    lghi        %r2, 1
    la          %r3, 256(%r15)
    la          %r4, 0(%r15)
    lghi        %r5, 0x1000
    SYS         293
    OK
    lg          %r1, 40(%r15)
    EXPECT      %r1, 0x501
    mvhhi       28(%r15), 0
    mvhi        128(%r15), 0
    lghi        %r2, 1
    la          %r3, 256(%r15)
    lghi        %r4, 0x1000
    lghi        %r5, 0x7ff
    la          %r6, 0(%r15)
    SYS         379
    OK
    llgh        %r1, 28(%r15)
    nill        %r1, 0xf000
    EXPECT      %r1, 0x2000
    llgf        %r1, 128(%r15)
    EXPECT      %r1, 5
    lghi        %r2, 1
    lghi        %r3, 0
    lghi        %r4, 1
    SYS         19
    FAILED_WITH 29
    lghi        %r2, 3
    la          %r3, 0(%r15)
    SYS         108
    FAILED_WITH 9
    EXIT_OK_OR_FAIL

    .global s390x_user_test_linux_credentials
s390x_user_test_linux_credentials:
    SYS         201
    OK
    lghi        %r2, 1000
    SYS         213
    OK
    SYS         199
    cghi        %r2, 1000
    jne         9f
    lghi        %r2, 0
    SYS         213
    FAILED_WITH 1
    EXIT_OK_OR_FAIL

    .global s390x_user_test_linux_move_and_discard
s390x_user_test_linux_move_and_discard:
    aghi        %r15, -64
    mvghi       0(%r15), 0
    mvghi       8(%r15), 4096
    mvghi       16(%r15), 3
    mvghi       24(%r15), 0x22
    mvghi       32(%r15), -1
    mvghi       40(%r15), 0
    la          %r2, 0(%r15)
    SYS         90
    lghi        %r0, -4096
    clgr        %r2, %r0
    jhe         9f
    lgr         %r9, %r2
    lgr         %r2, %r9
    lghi        %r3, 4096
    lghi        %r4, 23
    SYS         219
    OK
    LI64        %r11, 0x12345678
    stg         %r11, 0(%r9)
    lgr         %r10, %r9
    LI64        %r0, 0x100000
    sgr         %r10, %r0
    lgr         %r2, %r9
    lghi        %r3, 4096
    lghi        %r4, 4096
    lghi        %r5, 3
    lgr         %r6, %r10
    SYS         163
    cgr         %r2, %r10
    jne         9f
    lg          %r1, 0(%r10)
    cgr         %r1, %r11
    jne         9f
    stg         %r9, 0(%r15)
    mvghi       24(%r15), 0x32
    la          %r2, 0(%r15)
    SYS         90
    cgr         %r2, %r9
    jne         9f
    lg          %r1, 0(%r9)
    EXPECT      %r1, 0
    lgr         %r2, %r10
    lghi        %r3, 4096
    lghi        %r4, 4
    SYS         219
    OK
    lg          %r1, 0(%r10)
    EXPECT      %r1, 0
    EXIT_OK_OR_FAIL

    .global s390x_user_test_fp_registers
s390x_user_test_fp_registers:
    lghi        %r1, {syscall_first}
    svc         0
    lgr         %r6, %r2
    lghi        %r7, 0
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    lgdr        %r1, %f\i
    ogr         %r7, %r1
    .endr
    lghi        %r1, 0
    efpc        %r1
    ogr         %r7, %r1
    lgr         %r1, %r6
    lghi        %r0, 3
    ngr         %r1, %r0
    sfpc        %r1
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    lgr         %r1, %r6
    aghi        %r1, \i
    ldgr        %f\i, %r1
    .endr
1:  lghi        %r1, {syscall_null}
    svc         0
    lgr         %r8, %r2
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    lgr         %r1, %r6
    aghi        %r1, \i
    lgdr        %r0, %f\i
    xgr         %r0, %r1
    ogr         %r7, %r0
    .endr
    lghi        %r1, 0
    efpc        %r1
    lgr         %r0, %r6
    lghi        %r9, 3
    ngr         %r0, %r9
    xgr         %r1, %r0
    ogr         %r7, %r1
    ltgr        %r8, %r8
    je          1b
    lgr         %r2, %r7
    lghi        %r1, {syscall_report}
    svc         0
2:  j           2b
    .global s390x_user_test_stack_growth
s390x_user_test_stack_growth:
    lgr         %r3, %r15
    nill        %r3, 0xf000
5:  stg         %r3, 0(%r3)
    aghi        %r3, -4096
    j           5b
    .global s390x_user_test_illegal_instruction
s390x_user_test_illegal_instruction:
    .word       0
    .global s390x_user_test_end
s390x_user_test_end:
    .popsection
"#,
    save = const lowcore::SAVE,
    old_psw = const lowcore::OLD_PSW,
    class = const lowcore::CLASS,
    code = const lowcore::CODE,
    teid = const lowcore::TEID,
    teid_copy = const lowcore::TEID_COPY,
    external_old = const lowcore::EXTERNAL_OLD,
    svc_old = const lowcore::SVC_OLD,
    program_old = const lowcore::PROGRAM_OLD,
    io_old = const lowcore::IO_OLD,
    machine_check_old = const lowcore::MACHINE_CHECK_OLD,
    external_code = const lowcore::EXTERNAL_CODE,
    svc_code = const lowcore::SVC_CODE,
    program_code = const lowcore::PROGRAM_CODE,
    external = const EXTERNAL,
    svc = const SVC,
    program = const PROGRAM,
    io = const IO,
    kernel_entry_psw = const lowcore::KERNEL_ENTRY_PSW,
    context_physical = const lowcore::CONTEXT_PHYSICAL,
    context = const lowcore::CONTEXT,
    kernel_sp = const lowcore::KERNEL_SP,
    user_exit_psw = const lowcore::USER_EXIT_PSW,
    user_psw = const lowcore::USER_PSW,
    to_user_psw = const lowcore::TO_USER_PSW,
    kernel_return_psw = const lowcore::KERNEL_RETURN_PSW,
    frame = const 160 + core::mem::size_of::<TrapFrame>(),
    direct_map_high = const DIRECT_MAP_OFFSET >> 48,
    handler = sym s390x_exception,
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
    uc_stack_sp = const UC_STACK,
    uc_stack_size = const UC_STACK + 16,
    uc_sigmask = const UC_SIGMASK,
    saved_f8 = const UC_FPRS + 8 * 8,
    saved_f9 = const UC_FPRS + 9 * 8,
    saved_fpc = const UC_FPC,
    saved_r6 = const UC_GPRS + 6 * 8,
    saved_r7 = const UC_GPRS + 7 * 8,
    saved_r8 = const UC_GPRS + 8 * 8,
    saved_r9 = const UC_GPRS + 9 * 8,
    saved_pc = const UC_PSW_ADDRESS,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
);

/// Where s390x's `ucontext` keeps what the test programs' handlers read and change: `uc_stack`
/// after `uc_flags` and `uc_link`, then `_sigregs` (the PSW's mask and address, `gprs[16]`,
/// `acrs[16]`, the FP control word and a pad, `fprs[16]`), then `uc_sigmask`.
const UC_STACK: usize = 16;
const UC_PSW_ADDRESS: usize = 48;
const UC_GPRS: usize = 56;
const UC_FPC: usize = UC_GPRS + 16 * 8 + 16 * 4;
const UC_FPRS: usize = UC_FPC + 8;
const UC_SIGMASK: usize = UC_FPRS + 16 * 8;

/// The registers the kernel entry keeps on the stack, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    gpr: [u64; 16],
    mask: u64,
    pc: u64,
    class: u64,
    code: u64,
    teid: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 168);

/// Program-interruption codes the HAL tells apart (the PER bit, 0x80, masked off).
const OPERATION: u64 = 0x01;
const PRIVILEGED: u64 = 0x02;
const PROTECTION: u64 = 0x04;
const ADDRESSING: u64 = 0x05;
const SPECIFICATION: u64 = 0x06;
const DATA: u64 = 0x07;
const FIXED_OVERFLOW: u64 = 0x08;
const FIXED_DIVIDE: u64 = 0x09;
const SEGMENT_TRANSLATION: u64 = 0x10;
const PAGE_TRANSLATION: u64 = 0x11;
const REGION_TRANSLATION: core::ops::RangeInclusive<u64> = 0x38..=0x3b;
/// The translation-exception identification's fetch-or-store indication: a store.
const STORE: u64 = 2;

fn translation_address(teid: u64) -> u64 {
    teid & !0xfff
}

fn store_indication(teid: u64) -> u64 {
    (teid >> 10) & 3
}

/// The `Exception::code` of a program interruption: its code, the store indication above it.
fn program_code(code: u64, teid: u64) -> u64 {
    (code & 0x7f) | store_indication(teid) << 8
}

/// Called by the kernel entry with the frame it built.
extern "C" fn s390x_exception(frame: &mut TrapFrame) {
    if frame.class == EXTERNAL || frame.class == IO {
        external(frame.class, frame.code);
        if frame.mask & psw::WAIT != 0 {
            frame.mask &= !(psw::WAIT | psw::EXTERNAL | psw::IO);
        }
        return;
    }
    let exception = decode(frame.class, frame.code, frame.teid, frame.pc);
    deliver(&exception);
}

/// Takes an external or I/O interruption: the clock comparator, a wake-up, the SCLP's service
/// signal; I/O waits for the channel subsystem's driver (part 3).
fn external(class: u64, code: u64) {
    if class != EXTERNAL {
        return;
    }
    match code as u16 {
        timer::CLOCK_COMPARATOR => {
            timer::end_timer_interrupt();
            crate::interrupt::deliver(Interrupt::Timer);
        }
        timer::EMERGENCY_SIGNAL => crate::interrupt::deliver(Interrupt::Wakeup),
        _ => {}
    }
}

unsafe extern "C" {
    fn s390x_enter_user(context: *mut user::s390x::Registers);
    static s390x_user_test_start: u8;
    static s390x_user_test_checks: u8;
    static s390x_user_test_floating_point: u8;
    static s390x_user_test_null_syscalls: u8;
    static s390x_user_test_spin: u8;
    static s390x_user_test_linux_hello: u8;
    static s390x_user_test_linux_start: u8;
    static s390x_user_test_linux_start_end: u8;
    static s390x_user_test_linux_unknown_syscalls: u8;
    static s390x_user_test_linux_thread_pointer: u8;
    static s390x_user_test_linux_poll: u8;
    static s390x_user_test_linux_signals: u8;
    static s390x_user_test_linux_signal_handler: u8;
    static s390x_user_test_linux_fault_handler: u8;
    static s390x_user_test_linux_signal_queue: u8;
    static s390x_user_test_linux_signal_suspend: u8;
    static s390x_user_test_linux_clock: u8;
    static s390x_user_test_linux_stat: u8;
    static s390x_user_test_linux_credentials: u8;
    static s390x_user_test_linux_move_and_discard: u8;
    static s390x_user_test_fp_registers: u8;
    static s390x_user_test_stack_growth: u8;
    static s390x_user_test_illegal_instruction: u8;
    static s390x_user_test_end: u8;
}

const SHADOW_ENTRIES: usize = 2048;
const USER_ROOT_ENTRIES: usize = 1024;

#[repr(C, align(4096))]
struct ShadowRoot([u64; SHADOW_ENTRIES]);

/// Each CPU's program root: the user half's root entries of the running process's root.
static mut SHADOWS: [ShadowRoot; super::MAX_CPUS] = [const { ShadowRoot([0; SHADOW_ENTRIES]) }; super::MAX_CPUS];
/// Each CPU's whole root, as the home space's ASCE names it.
static ROOTS: [core::sync::atomic::AtomicU64; super::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; super::MAX_CPUS];

fn shadow_of(cpu: usize) -> *mut u64 {
    // SAFETY: only `cpu`'s own CPU reaches its shadow, with interruptions off.
    unsafe { (&raw mut SHADOWS[cpu]).cast::<u64>() }
}

fn load_control(register: u32, value: u64) {
    // SAFETY: the callers load CR1 (the primary space's ASCE) or CR13 (the home space's) with
    // a root the kernel built; nothing in the kernel runs in the primary space.
    unsafe {
        match register {
            1 => core::arch::asm!("lctlg %c1, %c1, 0({0})", in(reg) &raw const value, options(nostack, readonly)),
            _ => core::arch::asm!("lctlg %c13, %c13, 0({0})", in(reg) &raw const value, options(nostack, readonly)),
        }
    }
}

/// Copies the user half of the whole root at physical `root` into this CPU's program root and
/// leaves the kernel half invalid; points the primary space at it.
pub(super) fn sync_program_root(root: u64) {
    let cpu = super::cpu_index();
    ROOTS[cpu].store(root, core::sync::atomic::Ordering::Relaxed);
    let whole = (root + DIRECT_MAP_OFFSET) as *const u64;
    let shadow = shadow_of(cpu);
    let empty = S390x::empty(S390x::LEVELS - 1);
    for index in 0..SHADOW_ENTRIES {
        let entry = match index < USER_ROOT_ENTRIES {
            // SAFETY: an entry of the process's root table, 2048 entries long, reached through
            // the direct map; the kernel writes root entries before it runs the program.
            true => unsafe { whole.add(index).read_volatile() },
            false => empty,
        };
        // SAFETY: an entry of this CPU's program root.
        unsafe { shadow.add(index).write_volatile(entry) };
    }
    let shadow_physical = shadow as u64 - KERNEL_VIRTUAL_OFFSET;
    load_control(1, shadow_physical | ASCE_REGION_SECOND);
    purge_translations();
}

/// Points the home space at the whole root at physical `root`.
pub(super) fn set_home_root(root: u64) {
    load_control(13, root | ASCE_REGION_SECOND);
}

/// Drops this CPU's cached translations.
pub(super) fn purge_translations() {
    // SAFETY: `ptlb` only empties this CPU's TLB.
    unsafe { core::arch::asm!("ptlb", options(nostack, nomem)) };
}

/// [`crate::Platform::run_user`]: the caller guarantees interruptions are off and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    loop {
        let registers = &mut context.arch;
        // SAFETY: interruptions are off (the caller's duty), so nothing arrives before the
        // program's PSW is loaded; the interruption that ends it saves its registers into the
        // context, which `&mut` keeps valid, and returns here with the kernel's r6 to r15. The
        // program runs in problem state in the primary space, whose root holds the user half
        // alone.
        unsafe { s390x_enter_user(registers) };
        let registers = &mut context.arch;
        match registers.class {
            SVC => {
                registers.syscall_r2 = registers.gpr[2];
                return UserExit::Syscall;
            }
            EXTERNAL | IO => {
                external(registers.class, registers.code);
                return UserExit::Interrupt;
            }
            PROGRAM if REGION_TRANSLATION.contains(&(registers.code & 0x7f)) && root_entry_appeared(registers.teid) => {
                continue;
            }
            _ => {}
        }
        return UserExit::Exception(registers_exception(registers));
    }
}

/// Whether the whole root has an entry for `teid`'s address that this CPU's program root
/// lacked: then it is copied in.
fn root_entry_appeared(teid: u64) -> bool {
    let cpu = super::cpu_index();
    let root = ROOTS[cpu].load(core::sync::atomic::Ordering::Relaxed);
    let index = S390x::index(translation_address(teid), S390x::LEVELS - 1);
    if root == 0 || index >= USER_ROOT_ENTRIES {
        return false;
    }
    // SAFETY: an entry of the process's root, through the direct map.
    let entry = unsafe { ((root + DIRECT_MAP_OFFSET) as *const u64).add(index).read_volatile() };
    let shadow = shadow_of(cpu);
    // SAFETY: an entry of this CPU's program root.
    let current = unsafe { shadow.add(index).read_volatile() };
    if entry == current || entry == S390x::empty(S390x::LEVELS - 1) {
        return false;
    }
    // SAFETY: as above.
    unsafe { shadow.add(index).write_volatile(entry) };
    purge_translations();
    true
}

fn registers_exception(registers: &user::s390x::Registers) -> Exception {
    decode(registers.class, registers.code, registers.teid, registers.pc)
}

impl crate::user::s390x::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned.
    pub(crate) fn last_exception(&self) -> Exception {
        registers_exception(self)
    }
}

/// [`crate::Platform::user_test_program`]: every program, the Linux ones in s390x's numbers
/// (D417).
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const s390x_user_test_start) as usize;
    let end = (&raw const s390x_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const s390x_user_test_checks),
        floating_point: at(&raw const s390x_user_test_floating_point),
        null_syscalls: at(&raw const s390x_user_test_null_syscalls),
        spin: at(&raw const s390x_user_test_spin),
        fp_registers: at(&raw const s390x_user_test_fp_registers),
        illegal_instruction: at(&raw const s390x_user_test_illegal_instruction),
        linux_hello: at(&raw const s390x_user_test_linux_hello),
        linux_start: at(&raw const s390x_user_test_linux_start),
        linux_start_end: at(&raw const s390x_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const s390x_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const s390x_user_test_stack_growth),
        linux_thread_pointer: at(&raw const s390x_user_test_linux_thread_pointer),
        linux_poll: at(&raw const s390x_user_test_linux_poll),
        linux_signals: at(&raw const s390x_user_test_linux_signals),
        linux_signal_handler: at(&raw const s390x_user_test_linux_signal_handler),
        linux_fault_handler: at(&raw const s390x_user_test_linux_fault_handler),
        linux_signal_queue: at(&raw const s390x_user_test_linux_signal_queue),
        linux_signal_suspend: at(&raw const s390x_user_test_linux_signal_suspend),
        linux_clock: at(&raw const s390x_user_test_linux_clock),
        linux_stat: at(&raw const s390x_user_test_linux_stat),
        linux_credentials: at(&raw const s390x_user_test_linux_credentials),
        linux_move_and_discard: at(&raw const s390x_user_test_linux_move_and_discard),
    }
}

/// [`crate::Platform::page_fault`]: translation exceptions (pages, segments, regions) and
/// protection, by the store indication.
pub(super) fn page_fault(kind: ExceptionKind, code: u64) -> Option<PageFault> {
    let store = code >> 8 == STORE;
    let access = match kind {
        ExceptionKind::InstructionFault => Access::Execute,
        _ if store => Access::Write,
        _ => Access::Read,
    };
    match code & 0x7f {
        SEGMENT_TRANSLATION | PAGE_TRANSLATION => Some(PageFault { access, may_be_stale: true }),
        program if REGION_TRANSLATION.contains(&program) => Some(PageFault { access, may_be_stale: true }),
        PROTECTION => Some(PageFault {
            access: Access::Write,
            may_be_stale: false,
        }),
        _ => None,
    }
}

fn decode(class: u64, code: u64, teid: u64, pc: u64) -> Exception {
    let address = translation_address(teid);
    let fetch = address == pc & !0xfff;
    let program = code & 0x7f;
    let (kind, name, has_address) = match class {
        EXTERNAL | IO => (ExceptionKind::Interrupt, "interrupt", false),
        SVC => (ExceptionKind::Other, "supervisor call", false),
        _ => match program {
            OPERATION => (ExceptionKind::IllegalInstruction, "operation", false),
            PRIVILEGED => (ExceptionKind::IllegalInstruction, "privileged operation", false),
            SPECIFICATION => (ExceptionKind::IllegalInstruction, "specification", false),
            DATA => (ExceptionKind::IllegalInstruction, "data", false),
            FIXED_OVERFLOW | FIXED_DIVIDE => (ExceptionKind::Arithmetic, "fixed point", false),
            PROTECTION => (ExceptionKind::DataFault, "protection", true),
            ADDRESSING => (ExceptionKind::DataFault, "addressing", true),
            SEGMENT_TRANSLATION | PAGE_TRANSLATION if fetch => (ExceptionKind::InstructionFault, "translation (fetch)", true),
            SEGMENT_TRANSLATION | PAGE_TRANSLATION => (ExceptionKind::DataFault, "translation", true),
            program if REGION_TRANSLATION.contains(&program) => (ExceptionKind::DataFault, "region translation", true),
            _ => (ExceptionKind::Other, "program interruption", false),
        },
    };
    Exception {
        kind,
        name,
        code: match class {
            PROGRAM => program_code(code, teid),
            _ => class << 32 | code,
        },
        pc,
        address: has_address.then_some(address),
    }
}
