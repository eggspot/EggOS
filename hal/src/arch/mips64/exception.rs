//! MIPS64 exception entry and the TLB (D313).
//!
//! `EBase` points at [`mips64_vectors`]: both TLB refill vectors jump to the refill handler,
//! which walks the page tables (`paging::mips64`) with `k0` and `k1` only and writes a random
//! TLB entry, an invalid pair where no page is mapped so the access raises a TLB-invalid
//! exception the kernel sees; the general vector saves the registers on the kernel stack, clears
//! `Status.EXL` so the kernel's handler may take TLB refills itself, calls
//! [`mips64_exception`] and restores everything, unless the exception came from user mode
//! (`Status.KSU`), whose registers go into its `UserContext` (`KScratch2` holds it) before
//! [`run_user`] returns (D47). `KScratch1` holds a program's root table and [`KERNEL_ROOT`] the
//! kernel's, both as cached-window addresses.

use super::{CACHED_WINDOW, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Access, PageFault};
use crate::user::{self, TestProgram, UserContext, UserExit};
use core::sync::atomic::{AtomicU64, Ordering};

core::arch::global_asm!(
    r#"
    .set        noat
    .pushsection .text.vectors, "ax"
    .balign 4096
    .global mips64_vectors
mips64_vectors:
    j           mips64_tlb_refill
    .org        mips64_vectors + 0x080
    j           mips64_tlb_refill
    .org        mips64_vectors + 0x100
    j           mips64_cache_error
    .org        mips64_vectors + 0x180
    j           mips64_general_entry
    .org        mips64_vectors + 0x200
    j           mips64_general_entry
    .org        mips64_vectors + 0x280

mips64_tlb_refill:
    dmfc0       $26, $8
    bltz        $26, 1f
    dmfc0       $27, $31, 2
    b           2f
1:  dla         $27, {kernel_root}
    ld          $27, 0($27)
2:  beqz        $27, 9f
    dsrl        $26, $26, 36
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 27
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 18
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 9
    andi        $26, $26, 0xff0
    daddu       $27, $27, $26
    ld          $26, 0($27)
    ld          $27, 8($27)
    dmtc0       $26, $2
    dmtc0       $27, $3
    ehb
    tlbwr
    eret
9:  dmtc0       $zero, $2
    dmtc0       $zero, $3
    ehb
    tlbwr
    eret

mips64_cache_error:
    wait
    b           mips64_cache_error

mips64_general_entry:
    mfc0        $26, $12
    andi        $26, $26, 0x18
    bnez        $26, mips64_user_entry
    move        $27, $29
    daddiu      $29, $29, -{frame_size}
    sd          $27, 29 * 8($29)
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    sd          $\n, \n * 8($29)
    .endr
    mfhi        $12
    sd          $12, 256($29)
    mflo        $12
    sd          $12, 264($29)
    dmfc0       $12, $14
    sd          $12, 272($29)
    mfc0        $12, $12
    sd          $12, 280($29)
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x1f
    mtc0        $12, $12
    ehb
    move        $4, $29
    jal         {handler}
    ld          $12, 280($29)
    mtc0        $12, $12
    ehb
    ld          $12, 272($29)
    dmtc0       $12, $14
    ld          $12, 256($29)
    mthi        $12
    ld          $12, 264($29)
    mtlo        $12
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    ld          $\n, \n * 8($29)
    .endr
    ld          $29, 29 * 8($29)
    eret

    // From user mode (D47): the registers go into the `UserContext` `KScratch2` holds, then
    // `mips64_enter_user` returns on the kernel stack it left, with the kernel's callee-saved
    // registers and `gp`, in kernel mode with `EXL` clear and interrupts masked.
mips64_user_entry:
    dmfc0       $26, $31, 3
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,29,30,31
    sd          $\n, \n * 8($26)
    .endr
    mfhi        $12
    sd          $12, 264($26)
    mflo        $12
    sd          $12, 272($26)
    dmfc0       $12, $14
    sd          $12, 256($26)
    dmfc0       $12, $4, 2
    sd          $12, 280($26)
    ld          $29, 288($26)
    ld          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    ld          $\n, (\n - 15) * 8($29)
    .endr
    ld          $30, 72($29)
    ld          $28, 80($29)
    daddiu      $29, $29, 96
    mfc0        $12, $12
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x1f
    mtc0        $12, $12
    ehb
    jr          $31

    // `mips64_enter_user(context)`: keeps the kernel's callee-saved registers and `gp` on its
    // stack and the stack pointer in the context, points `KScratch2` at the context, sets
    // `UserLocal`, `EPC`, `hi` and `lo`, then `Status` to user mode with interrupts enabled
    // (`EXL` keeps both off until the `eret`), loads the program's registers and returns to it.
    .global mips64_enter_user
mips64_enter_user:
    daddiu      $29, $29, -96
    sd          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    sd          $\n, (\n - 15) * 8($29)
    .endr
    sd          $30, 72($29)
    sd          $28, 80($29)
    sd          $29, 288($4)
    dmtc0       $4, $31, 3
    ld          $12, 280($4)
    dmtc0       $12, $4, 2
    ld          $12, 256($4)
    dmtc0       $12, $14
    ld          $12, 264($4)
    mthi        $12
    ld          $12, 272($4)
    mtlo        $12
    mfc0        $12, $12
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x0c
    mtc0        $12, $12
    ehb
    .irp        n, 1,2,3,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,29,30,31
    ld          $\n, \n * 8($4)
    .endr
    ld          $4, 4 * 8($4)
    eret
    .popsection

    // The user-mode test programs (D47, `TestProgram`): position-independent, no data. Copied to
    // a user page, never run where they are linked. The Linux ones use n64's numbers, whose
    // errors set `a3` and leave the error number positive in `v0`.
    .macro SYS number
    li          $2, \number
    syscall
    .endm
    .macro OK
    bnez        $7, 9f
    bnez        $2, 9f
    .endm
    .macro FAILED_WITH errno
    beqz        $7, 9f
    li          $12, \errno
    bne         $2, $12, 9f
    .endm
    .macro EXPECT reg, value
    dli         $13, \value
    bne         \reg, $13, 9f
    .endm
    .macro EXPECT_BACK reg, value
    dli         $13, \value
    bne         \reg, $13, 9b
    .endm
    .macro READ_TP reg
    .set        push
    .set        mips64r2
    rdhwr       \reg, $29
    .set        pop
    .endm
    .macro EXIT_OK_OR_FAIL
    li          $4, 0
    SYS         5205
9:  li          $4, 1
    SYS         5205
8:  b           8b
    .endm

    .pushsection .rodata.user_test, "a"
    .set        hardfloat
    .balign 4
    .global mips64_user_test_start
mips64_user_test_start:
    .global mips64_user_test_checks
mips64_user_test_checks:
    li          $4, 1
    li          $5, 2
    li          $6, 3
    li          $7, 4
    li          $8, 5
    li          $9, 6
    .irp        n, 1,3,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    li          $\n, 0x100 + \n
    .endr
    li          $2, {syscall_first}
    syscall
    daddiu      $5, $5, -2
    daddiu      $6, $6, -3
    or          $5, $5, $6
    daddiu      $8, $8, -5
    or          $5, $5, $8
    daddiu      $9, $9, -6
    or          $5, $5, $9
    .irp        n, 1,3,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    daddiu      $\n, $\n, -(0x100 + \n)
    or          $5, $5, $\n
    .endr
    move        $4, $2
    li          $2, {syscall_report}
    syscall
    ld          $5, 0($2)
1:  b           1b
    .global mips64_user_test_floating_point
mips64_user_test_floating_point:
    dmtc1       $zero, $f0
    .global mips64_user_test_null_syscalls
mips64_user_test_null_syscalls:
    li          $2, {syscall_null}
2:  syscall
    b           2b
    .global mips64_user_test_spin
mips64_user_test_spin:
3:  b           3b
    .global mips64_user_test_linux_hello
mips64_user_test_linux_hello:
    li          $4, 1
    bal         5f
5:  daddiu      $5, $31, 9f - 5b
    li          $6, {hello_length}
    li          $2, {linux_write}
    syscall
    daddiu      $16, $2, -{hello_length}
    or          $16, $16, $7
    li          $4, 1
    li          $5, 0
    li          $6, 1
    li          $2, {linux_write}
    syscall
    daddiu      $12, $2, -{efault}
    or          $16, $16, $12
    daddiu      $12, $7, -1
    or          $16, $16, $12
    li          $2, {unknown_syscall}
    syscall
    daddiu      $12, $2, -{enosys}
    or          $16, $16, $12
    daddiu      $12, $7, -1
    or          $16, $16, $12
    sltu        $4, $zero, $16
    li          $2, {linux_exit_group}
    syscall
4:  b           4b
9:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global mips64_user_test_linux_start
mips64_user_test_linux_start:
    andi        $12, $29, 15
    bnez        $12, 9f
    SYS         5038
    move        $16, $2
    SYS         5178
    bne         $2, $16, 9f
    daddiu      $4, $29, -16
    li          $5, 16
    li          $6, 0
    SYS         5313
    bnez        $7, 9f
    EXPECT      $2, 16
    ld          $12, -16($29)
    ld          $13, -8($29)
    or          $12, $12, $13
    beqz        $12, 9f
    li          $4, 0
    li          $5, 3
    li          $6, 0
    daddiu      $7, $29, -32
    SYS         5297
    OK
    ld          $12, -32($29)
    EXPECT      $12, 0x800000
    ld          $12, -24($29)
    EXPECT      $12, -1
    sd          $zero, -64($29)
    li          $12, 1
    sd          $12, -56($29)
    sd          $zero, -48($29)
    sd          $zero, -40($29)
    li          $4, 13
    daddiu      $5, $29, -64
    li          $6, 0
    li          $7, 16
    SYS         5013
    OK
    li          $4, 13
    li          $5, 0
    daddiu      $6, $29, -112
    li          $7, 16
    SYS         5013
    OK
    ld          $12, -104($29)
    EXPECT      $12, 1
    li          $4, 0
    daddiu      $5, $29, -136
    SYS         5129
    OK
    lw          $12, -120($29)
    EXPECT      $12, 2
    ld          $16, 0($29)
    daddiu      $17, $29, 8
    li          $18, 2
6:  ld          $5, 0($17)
    daddiu      $17, $17, 8
    beqz        $5, 8f
    li          $6, 0
7:  daddu       $12, $5, $6
    lbu         $12, 0($12)
    beqz        $12, 5f
    daddiu      $6, $6, 1
    b           7b
5:  daddiu      $29, $29, -16
    sd          $5, 0($29)
    sd          $6, 8($29)
    li          $4, 1
    move        $5, $29
    li          $6, 1
    SYS         5019
    daddiu      $29, $29, 16
    b           6b
8:  daddiu      $18, $18, -1
    bnez        $18, 6b
    move        $4, $16
    SYS         5205
9:  li          $4, 255
    SYS         5205
4:  b           4b
    .global mips64_user_test_linux_start_end
mips64_user_test_linux_start_end:
    .global mips64_user_test_linux_unknown_syscalls
mips64_user_test_linux_unknown_syscalls:
    li          $2, {unknown_syscall}
7:  syscall
    b           7b
    .global mips64_user_test_stack_growth
mips64_user_test_stack_growth:
    li          $12, -4096
    and         $12, $29, $12
    li          $13, 4096
8:  sd          $12, 0($12)
    dsubu       $12, $12, $13
    b           8b

    .global mips64_user_test_linux_thread_pointer
mips64_user_test_linux_thread_pointer:
    READ_TP     $16
    SYS         5038
    dsll        $17, $2, 12
    move        $4, $17
    SYS         5242
    or          $16, $16, $7
    li          $18, {thread_pointer_rounds}
7:  SYS         5023
    or          $16, $16, $7
    or          $16, $16, $2
    READ_TP     $12
    xor         $12, $12, $17
    or          $16, $16, $12
    daddiu      $18, $18, -1
    bnez        $18, 7b
    sltu        $4, $zero, $16
    SYS         5205
8:  b           8b

    .global mips64_user_test_linux_poll
mips64_user_test_linux_poll:
    daddiu      $29, $29, -64
    sd          $zero, 0($29)
    li          $12, 1
    sw          $12, 8($29)
    li          $12, 4
    sh          $12, 12($29)
    sh          $zero, 14($29)
    li          $12, 2
    sw          $12, 16($29)
    sw          $zero, 20($29)
    sd          $zero, 32($29)
    sd          $zero, 40($29)
    move        $4, $29
    li          $5, 3
    daddiu      $6, $29, 32
    li          $7, 0
    li          $8, 0
    SYS         5261
    bnez        $7, 9f
    EXPECT      $2, 1
    lhu         $12, 6($29)
    bnez        $12, 9f
    lhu         $12, 14($29)
    EXPECT      $12, 4
    lhu         $12, 22($29)
    bnez        $12, 9f
    li          $4, 1
    dli         $5, 0x40087468
    daddiu      $6, $29, 48
    SYS         5015
    OK
    sd          $zero, 32($29)
    dli         $12, {poll_wait}
    sd          $12, 40($29)
    li          $4, 0
    li          $5, 0
    daddiu      $6, $29, 32
    li          $7, 0
    li          $8, 0
    SYS         5261
    OK
    ld          $12, 32($29)
    ld          $13, 40($29)
    or          $12, $12, $13
    bnez        $12, 9f
    EXIT_OK_OR_FAIL

    .global mips64_user_test_linux_signals
mips64_user_test_linux_signals:
    daddiu      $29, $29, -64
    li          $4, 0
    li          $5, 18
    SYS         5060
    OK
    li          $12, 0x8000
    sd          $12, 0($29)
    sd          $zero, 8($29)
    li          $4, 1
    move        $5, $29
    li          $6, 0
    li          $7, 16
    SYS         5014
    OK
    SYS         5038
    move        $4, $2
    move        $5, $2
    li          $6, 16
    SYS         5225
    OK
    daddiu      $4, $29, 16
    li          $5, 16
    SYS         5125
    OK
    ld          $12, 16($29)
    EXPECT      $12, 0x8000
    li          $4, 2
    move        $5, $29
    li          $6, 0
    li          $7, 16
    SYS         5014
9:  li          $4, 1
    SYS         5205
8:  b           8b

    .global mips64_user_test_linux_signal_handler
mips64_user_test_linux_signal_handler:
    daddiu      $29, $29, -128
    daddiu      $12, $29, -0x2800
    sd          $12, 0($29)
    li          $12, 0x2000
    sd          $12, 8($29)
    sd          $zero, 16($29)
    move        $4, $29
    li          $5, 0
    SYS         5129
    OK
    dli         $12, 0x08000008
    sw          $12, 32($29)
    sw          $zero, 36($29)
    bal         1f
1:  daddiu      $12, $31, 5f - 1b
    sd          $12, 40($29)
    sd          $zero, 48($29)
    sd          $zero, 56($29)
    li          $4, 16
    daddiu      $5, $29, 32
    li          $6, 0
    li          $7, 16
    SYS         5013
    OK
    SYS         5038
    move        $4, $2
    move        $5, $2
    li          $6, 16
    li          $17, 0x5a5
    li          $16, 1
    dli         $12, {signal_fp_kept}
    dmtc1       $12, $f24
    li          $12, {signal_rounding}
    ctc1        $12, $31
    SYS         5225
    OK
    EXPECT      $17, 0x5a5
    EXPECT      $16, 2
    dmfc1       $12, $f24
    EXPECT      $12, {signal_fp_kept}
    dmfc1       $12, $f25
    EXPECT      $12, {signal_fp_changed}
    cfc1        $12, $31
    andi        $12, $12, 3
    EXPECT      $12, {signal_rounding}
    li          $4, 1
    li          $5, 0
    daddiu      $6, $29, 64
    li          $7, 16
    SYS         5014
    OK
    ld          $12, 64($29)
    bnez        $12, 9f
    EXIT_OK_OR_FAIL
5:  li          $12, 16
    bne         $4, $12, 7f
    lw          $13, 0($5)
    bne         $13, $12, 7f
    lw          $13, 4($5)
    li          $12, -6
    bne         $13, $12, 7f
    ld          $12, {uc_stack_sp}($6)
    sltu        $13, $29, $12
    bnez        $13, 7f
    ld          $13, {uc_stack_size}($6)
    daddu       $12, $12, $13
    sltu        $13, $29, $12
    beqz        $13, 7f
    dli         $12, {signal_fp_kept}
    dmfc1       $13, $f24
    bne         $13, $12, 7f
    ld          $13, {saved_f24}($6)
    bne         $13, $12, 7f
    lw          $13, {saved_fcsr}($6)
    andi        $13, $13, 3
    li          $12, {signal_rounding}
    bne         $13, $12, 7f
    dli         $12, {signal_fp_changed}
    sd          $12, {saved_f25}($6)
    dmtc1       $zero, $f24
    li          $12, 2
    sd          $12, {saved_r16}($6)
7:  li          $17, 0
    jr          $31

    .global mips64_user_test_linux_fault_handler
mips64_user_test_linux_fault_handler:
    daddiu      $29, $29, -64
    li          $12, 8
    sw          $12, 0($29)
    sw          $zero, 4($29)
    bal         1f
1:  daddiu      $12, $31, 5f - 1b
    sd          $12, 8($29)
    sd          $zero, 16($29)
    sd          $zero, 24($29)
    li          $4, 11
    move        $5, $29
    li          $6, 0
    li          $7, 16
    SYS         5013
    OK
    li          $4, 0
    li          $5, 4096
    li          $6, 0
    li          $7, 0x802
    li          $8, -1
    li          $9, 0
    SYS         5009
    bnez        $7, 9f
    move        $20, $2
    li          $16, 0
    dli         $17, {fault_address}
    li          $18, 1
    bal         2f
2:  daddiu      $19, $31, 3f - 2b
    ld          $12, 0($17)
    b           9f
3:  move        $17, $20
    li          $18, 2
    bal         4f
4:  daddiu      $19, $31, 6f - 4b
    ld          $12, 0($17)
    b           9f
6:  EXPECT      $16, 2
    EXIT_OK_OR_FAIL
5:  li          $12, 11
    bne         $4, $12, 7f
    lw          $13, 0($5)
    bne         $13, $12, 7f
    lw          $13, 4($5)
    ld          $12, {saved_r18}($6)
    bne         $13, $12, 7f
    ld          $13, 16($5)
    ld          $12, {saved_r17}($6)
    bne         $13, $12, 7f
    ld          $12, {saved_r16}($6)
    daddiu      $12, $12, 1
    sd          $12, {saved_r16}($6)
7:  ld          $12, {saved_r19}($6)
    sd          $12, {saved_pc}($6)
    jr          $31

    .global mips64_user_test_linux_signal_queue
mips64_user_test_linux_signal_queue:
    daddiu      $29, $29, -320
    move        $22, $29
    dli         $12, 1 << ({queued_signal} - 1)
    sd          $12, 272($22)
    sd          $zero, 280($22)
    sd          $zero, 256($22)
    sd          $zero, 264($22)
    li          $4, 1
    daddiu      $5, $22, 272
    li          $6, 0
    li          $7, 16
    SYS         5014
    OK
    SYS         5038
    move        $16, $2
    li          $12, {queued_signal}
    sw          $12, 128($22)
    li          $12, -1
    sw          $12, 132($22)
    sw          $zero, 136($22)
    sw          $16, 144($22)
    sw          $zero, 148($22)
    dli         $12, {first_value}
    sd          $12, 152($22)
    move        $4, $16
    li          $5, {queued_signal}
    daddiu      $6, $22, 128
    SYS         5127
    OK
    dli         $12, {second_value}
    sd          $12, 152($22)
    move        $4, $16
    li          $5, {queued_signal}
    daddiu      $6, $22, 128
    SYS         5127
    OK
    move        $4, $16
    li          $5, {queued_signal}
    SYS         5060
    OK
    li          $17, -1
    dli         $18, {first_value}
    bal         6f
    dli         $18, {second_value}
    bal         6f
    li          $17, 0
    li          $18, 0
    bal         6f
    daddiu      $4, $22, 272
    move        $5, $22
    daddiu      $6, $22, 256
    li          $7, 16
    SYS         5126
    FAILED_WITH 11
    EXIT_OK_OR_FAIL
6:  move        $23, $31
    daddiu      $4, $22, 272
    move        $5, $22
    daddiu      $6, $22, 256
    li          $7, 16
    SYS         5126
    bnez        $7, 9b
    EXPECT_BACK $2, {queued_signal}
    lw          $12, 0($22)
    EXPECT_BACK $12, {queued_signal}
    lw          $12, 4($22)
    bne         $12, $17, 9b
    lw          $12, 16($22)
    bne         $12, $16, 9b
    ld          $12, 24($22)
    bne         $12, $18, 9b
    jr          $23

    .global mips64_user_test_linux_signal_suspend
mips64_user_test_linux_signal_suspend:
    daddiu      $29, $29, -96
    li          $12, 8
    sw          $12, 0($29)
    sw          $zero, 4($29)
    bal         1f
1:  daddiu      $12, $31, 5f - 1b
    sd          $12, 8($29)
    sd          $zero, 16($29)
    sd          $zero, 24($29)
    li          $4, 16
    move        $5, $29
    li          $6, 0
    li          $7, 16
    SYS         5013
    OK
    li          $12, 0x8000
    sd          $12, 32($29)
    sd          $zero, 40($29)
    sd          $zero, 48($29)
    sd          $zero, 56($29)
    li          $4, 1
    daddiu      $5, $29, 32
    li          $6, 0
    li          $7, 16
    SYS         5014
    OK
    SYS         5038
    move        $4, $2
    move        $5, $2
    li          $6, 16
    SYS         5225
    OK
    li          $16, 1
    daddiu      $4, $29, 48
    li          $5, 16
    SYS         5128
    FAILED_WITH 4
    EXPECT      $16, 2
    li          $4, 1
    li          $5, 0
    daddiu      $6, $29, 64
    li          $7, 16
    SYS         5014
    OK
    ld          $12, 64($29)
    EXPECT      $12, 0x8000
    EXIT_OK_OR_FAIL
5:  li          $12, 16
    bne         $4, $12, 7f
    ld          $12, {uc_sigmask}($6)
    li          $13, 0x8000
    bne         $12, $13, 7f
    li          $12, 2
    sd          $12, {saved_r16}($6)
7:  jr          $31

    .global mips64_user_test_linux_clock
mips64_user_test_linux_clock:
    daddiu      $29, $29, -64
    li          $4, 1
    move        $5, $29
    SYS         5222
    OK
    sd          $zero, 32($29)
    dli         $12, {clock_wait}
    sd          $12, 40($29)
    daddiu      $4, $29, 32
    li          $5, 0
    SYS         5034
    OK
    li          $4, 1
    daddiu      $5, $29, 16
    SYS         5222
    OK
    ld          $12, 0($29)
    ld          $13, 16($29)
    dsubu       $13, $13, $12
    dli         $12, 1000000000
    dmultu      $13, $12
    mflo        $13
    ld          $12, 24($29)
    daddu       $13, $13, $12
    ld          $12, 8($29)
    dsubu       $13, $13, $12
    ld          $12, 40($29)
    sltu        $12, $13, $12
    bnez        $12, 9f
    li          $4, 2
    daddiu      $5, $29, 16
    SYS         5222
    OK
    EXIT_OK_OR_FAIL

    .global mips64_user_test_linux_stat
mips64_user_test_linux_stat:
    daddiu      $29, $29, -272
    sb          $zero, 256($29)
    li          $4, 1
    move        $5, $29
    SYS         5005
    OK
    lwu         $12, 24($29)
    andi        $12, $12, 0xf000
    EXPECT      $12, 0x2000
    lwu         $12, 40($29)
    EXPECT      $12, 0x501
    sw          $zero, 40($29)
    li          $4, 1
    daddiu      $5, $29, 256
    move        $6, $29
    li          $7, 0x1000
    SYS         5252
    OK
    lwu         $12, 40($29)
    EXPECT      $12, 0x501
    sh          $zero, 28($29)
    sw          $zero, 128($29)
    li          $4, 1
    daddiu      $5, $29, 256
    li          $6, 0x1000
    li          $7, 0x7ff
    move        $8, $29
    SYS         5326
    OK
    lhu         $12, 28($29)
    andi        $12, $12, 0xf000
    EXPECT      $12, 0x2000
    lwu         $12, 128($29)
    EXPECT      $12, 5
    li          $4, 1
    li          $5, 0
    li          $6, 1
    SYS         5008
    FAILED_WITH 29
    li          $4, 3
    move        $5, $29
    SYS         5005
    FAILED_WITH 9
    EXIT_OK_OR_FAIL

    .global mips64_user_test_linux_credentials
mips64_user_test_linux_credentials:
    SYS         5105
    OK
    li          $4, 1000
    SYS         5103
    OK
    SYS         5100
    bnez        $7, 9f
    EXPECT      $2, 1000
    li          $4, 0
    SYS         5103
    FAILED_WITH 1
    EXIT_OK_OR_FAIL

    .global mips64_user_test_linux_move_and_discard
mips64_user_test_linux_move_and_discard:
    li          $4, 0
    li          $5, 4096
    li          $6, 3
    li          $7, 0x802
    li          $8, -1
    li          $9, 0
    SYS         5009
    bnez        $7, 9f
    move        $16, $2
    move        $4, $16
    li          $5, 4096
    li          $6, 23
    SYS         5027
    OK
    dli         $18, 0x12345678
    sd          $18, 0($16)
    li          $12, 0x100000
    dsubu       $17, $16, $12
    move        $4, $16
    li          $5, 4096
    li          $6, 4096
    li          $7, 3
    move        $8, $17
    SYS         5024
    bnez        $7, 9f
    bne         $2, $17, 9f
    ld          $12, 0($17)
    bne         $12, $18, 9f
    move        $4, $16
    li          $5, 4096
    li          $6, 3
    li          $7, 0x812
    li          $8, -1
    li          $9, 0
    SYS         5009
    bnez        $7, 9f
    bne         $2, $16, 9f
    ld          $12, 0($16)
    bnez        $12, 9f
    move        $4, $17
    li          $5, 4096
    li          $6, 4
    SYS         5027
    OK
    ld          $12, 0($17)
    bnez        $12, 9f
    EXIT_OK_OR_FAIL
    .global mips64_user_test_fp_registers
mips64_user_test_fp_registers:
    li          $2, {syscall_first}
    syscall
    move        $16, $2
    li          $17, 0
    cfc1        $12, $31
    or          $17, $17, $12
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    dmfc1       $12, $f\i
    or          $17, $17, $12
    .endr
    andi        $12, $16, 3
    ctc1        $12, $31
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    daddiu      $12, $16, \i
    dmtc1       $12, $f\i
    .endr
10: li          $2, {syscall_null}
    syscall
    move        $18, $2
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    daddiu      $12, $16, \i
    dmfc1       $13, $f\i
    xor         $13, $13, $12
    or          $17, $17, $13
    .endr
    cfc1        $12, $31
    andi        $13, $16, 3
    xor         $12, $12, $13
    or          $17, $17, $12
    beqz        $18, 10b
    move        $4, $17
    li          $2, {syscall_report}
    syscall
11: b           11b
    .global mips64_user_test_illegal_instruction
mips64_user_test_illegal_instruction:
    .word       {reserved_instruction}
    .global mips64_user_test_end
mips64_user_test_end:
    .popsection
"#,
    kernel_root = sym KERNEL_ROOT,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym mips64_exception,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    linux_write = const 5001,
    linux_exit_group = const 5205,
    efault = const 14,
    enosys = const 89,
    reserved_instruction = const 5,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    signal_fp_kept = const TestProgram::LINUX_SIGNAL_FP_KEPT,
    signal_fp_changed = const TestProgram::LINUX_SIGNAL_FP_CHANGED,
    signal_rounding = const 2,
    uc_stack_sp = const UC_STACK,
    uc_stack_size = const UC_STACK + 8,
    uc_sigmask = const UC_SIGMASK,
    saved_f24 = const UC_FP_REGS + 24 * 8,
    saved_f25 = const UC_FP_REGS + 25 * 8,
    saved_fcsr = const UC_FCSR,
    saved_r16 = const UC_REGS + 16 * 8,
    saved_r17 = const UC_REGS + 17 * 8,
    saved_r18 = const UC_REGS + 18 * 8,
    saved_r19 = const UC_REGS + 19 * 8,
    saved_pc = const UC_PC,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
);

/// Where n64's `ucontext` keeps what the test programs' handlers read and change: `uc_stack`
/// after `uc_flags` and `uc_link`, then `uc_mcontext`'s `sc_regs[32]`, `sc_fpregs[32]`, the
/// `hi` and `lo` words, `sc_pc` and `sc_fpc_csr`, then `uc_sigmask`.
const UC_STACK: usize = 16;
const UC_REGS: usize = 40;
const UC_FP_REGS: usize = UC_REGS + 32 * 8;
const UC_PC: usize = UC_REGS + 576;
const UC_FCSR: usize = UC_REGS + 584;
const UC_SIGMASK: usize = UC_REGS + 600;

/// The kernel's root table, as a cached-window address; 0 until the kernel's tables are active.
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// The registers saved by the kernel entry, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// `$0` to `$31` by register number; `r[0]`, `r[26]` and `r[27]` are unused and `r[29]` is
    /// the `sp` at the time of the exception.
    r: [u64; 32],
    hi: u64,
    lo: u64,
    epc: u64,
    status: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 288);

/// `Cause.ExcCode` of the exceptions the HAL tells apart.
const INTERRUPT: u64 = 0;
const TLB_MODIFIED: u64 = 1;
const TLB_LOAD: u64 = 2;
const TLB_STORE: u64 = 3;
const ADDRESS_ERROR_LOAD: u64 = 4;
const ADDRESS_ERROR_STORE: u64 = 5;
const BUS_ERROR_FETCH: u64 = 6;
const BUS_ERROR_DATA: u64 = 7;
const SYSCALL: u64 = 8;
const BREAKPOINT: u64 = 9;
const RESERVED_INSTRUCTION: u64 = 10;
const COPROCESSOR_UNUSABLE: u64 = 11;
const OVERFLOW: u64 = 12;
const TRAP: u64 = 13;
const FLOATING_POINT: u64 = 15;
const TLB_READ_INHIBIT: u64 = 19;
const TLB_EXECUTE_INHIBIT: u64 = 20;
const WATCH: u64 = 23;
/// The size of every MIPS64 instruction.
const INSTRUCTION_SIZE: u64 = 4;
/// `Cause.BD`: the exception's instruction is in a branch delay slot, `EPC` the branch's.
const BRANCH_DELAY: u64 = 1 << 31;

/// TLB entries written by [`flush_tlb`] match addresses in this unused part of `XKSEG`, each
/// its own page pair, so no two entries match one address.
const UNUSED_PAGES: u64 = 0xc000_7000_0000_0000;
/// `EntryHi.ASID`.
const ASID_MASK: u64 = 0xff;
/// `Index.P`: `tlbp` found no entry.
const PROBE_FAILED: u64 = 1 << 31;

fn exception_code(cause: u64) -> u64 {
    (cause >> 2) & 0x1f
}

/// The number of TLB entries, from `Config1.MMUSize`.
fn tlb_entries() -> u64 {
    let config1: u64;
    // SAFETY: reading `Config1` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $16, 1", out(reg) config1, options(nomem, nostack)) };
    ((config1 >> 25) & 0x3f) + 1
}

fn entry_hi() -> u64 {
    let value: u64;
    // SAFETY: reading `EntryHi` has no side effects.
    unsafe { core::arch::asm!("dmfc0 {0}, $10", out(reg) value, options(nomem, nostack)) };
    value
}

/// Writes an invalid entry matching nothing used at TLB index `index`, `asid` its identifier.
fn invalidate_entry(index: u64, asid: u64) {
    // SAFETY: an entry with both halves invalid translates nothing; its page pair is one of
    // `UNUSED_PAGES`'s, distinct per index, so it duplicates no entry. The sequence touches no
    // mapped memory, so no refill changes `EntryHi` in between.
    unsafe {
        core::arch::asm!(
            "dmtc0 {hi}, $10",
            "dmtc0 $zero, $2",
            "dmtc0 $zero, $3",
            "mtc0 {index}, $0",
            "ehb",
            "tlbwi",
            "ehb",
            hi = in(reg) UNUSED_PAGES + (index << 13) | asid,
            index = in(reg) index,
            options(nomem, nostack),
        )
    };
}

/// Drops every TLB entry; `EntryHi` keeps this CPU's address-space identifier.
pub(super) extern "C" fn flush_tlb() {
    let asid = entry_hi() & ASID_MASK;
    for index in 0..tlb_entries() {
        invalidate_entry(index, asid);
    }
    set_entry_hi(asid);
}

fn set_entry_hi(value: u64) {
    // SAFETY: `EntryHi`'s identifier is the one the refills and probes that follow use; its page
    // field is only an argument to them.
    unsafe { core::arch::asm!("dmtc0 {0}, $10", "ehb", in(reg) value, options(nomem, nostack)) };
}

/// Points the refill handler at a program's root table `low` and, when given, the kernel's
/// `high` (physical addresses).
pub(super) fn set_roots(low: u64, high: Option<u64>) {
    if let Some(high) = high {
        KERNEL_ROOT.store(CACHED_WINDOW | high, Ordering::Release);
    }
    // SAFETY: `KScratch1` is the refill handler's alone; the next refill from the low half walks
    // from this root.
    unsafe { core::arch::asm!("dmtc0 {0}, $31, 2", "ehb", in(reg) CACHED_WINDOW | low, options(nomem, nostack)) };
}

/// Gives this CPU a fresh address-space identifier, so no entry another program's tables left
/// matches; when they run out, every entry goes and they start again.
pub(super) fn next_address_space() {
    let next = (entry_hi() & ASID_MASK) + 1;
    if next > ASID_MASK {
        set_entry_hi(1);
        flush_tlb();
        return;
    }
    set_entry_hi(next);
}

/// [`crate::Platform::drop_user_translations`]: probes for each page pair of the range and
/// invalidates what it finds; a long range takes a fresh identifier instead.
pub(super) fn drop_translations(start: u64, length: u64) {
    let pages = length / crate::memory::FRAME_SIZE;
    if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
        return next_address_space();
    }
    let asid = entry_hi() & ASID_MASK;
    let mut pair = start & !0x1fff;
    while pair < start + length {
        let index: u64;
        // SAFETY: `tlbp` only looks the pair up and writes `Index`; nothing mapped is touched
        // in between, so no refill changes `EntryHi`.
        unsafe {
            core::arch::asm!(
                "dmtc0 {hi}, $10",
                "ehb",
                "tlbp",
                "ehb",
                "mfc0 {index}, $0",
                hi = in(reg) pair | asid,
                index = out(reg) index,
                options(nomem, nostack),
            )
        };
        if index & PROBE_FAILED == 0 {
            invalidate_entry(index & 0xfff, asid);
        }
        pair += 0x2000;
    }
    set_entry_hi(asid);
}

/// Called by the kernel entry with the saved registers.
extern "C" fn mips64_exception(frame: &mut TrapFrame) {
    let cause = read_cause();
    let bad = read_bad_address();
    if exception_code(cause) == INTERRUPT && interrupt(cause) {
        return;
    }
    let exception = decode(cause, bad, frame.epc);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint && cause & BRANCH_DELAY == 0 {
        frame.epc += INSTRUCTION_SIZE;
    }
}

fn read_cause() -> u64 {
    let value: u64;
    // SAFETY: reading `Cause` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $13", out(reg) value, options(nomem, nostack)) };
    value & 0xffff_ffff
}

fn read_bad_address() -> u64 {
    let value: u64;
    // SAFETY: reading `BadVAddr` has no side effects.
    unsafe { core::arch::asm!("dmfc0 {0}, $8", out(reg) value, options(nomem, nostack)) };
    value
}

/// Handles the pending interrupts `cause` names that the kernel takes; returns whether there
/// was one (if not, the interrupt is reported).
#[inline(always)]
fn interrupt(cause: u64) -> bool {
    let (timer_due, wakeup) = timer::pending(cause);
    if timer_due {
        timer::end_timer_interrupt();
        crate::interrupt::deliver(Interrupt::Timer);
    }
    if wakeup {
        timer::end_wakeup();
        crate::interrupt::deliver(Interrupt::Wakeup);
    }
    let device = cause & super::liointc::PENDING != 0;
    if device {
        super::liointc::handle();
    }
    timer_due || wakeup || device
}

unsafe extern "C" {
    /// Enters user mode with `context`'s registers; returns once an exception from it saved
    /// them back.
    fn mips64_enter_user(context: *mut user::mips64::Registers);
    static mips64_user_test_start: u8;
    static mips64_user_test_checks: u8;
    static mips64_user_test_floating_point: u8;
    static mips64_user_test_null_syscalls: u8;
    static mips64_user_test_spin: u8;
    static mips64_user_test_fp_registers: u8;
    static mips64_user_test_illegal_instruction: u8;
    static mips64_user_test_linux_hello: u8;
    static mips64_user_test_linux_start: u8;
    static mips64_user_test_linux_start_end: u8;
    static mips64_user_test_linux_unknown_syscalls: u8;
    static mips64_user_test_stack_growth: u8;
    static mips64_user_test_linux_thread_pointer: u8;
    static mips64_user_test_linux_poll: u8;
    static mips64_user_test_linux_signals: u8;
    static mips64_user_test_linux_signal_handler: u8;
    static mips64_user_test_linux_fault_handler: u8;
    static mips64_user_test_linux_signal_queue: u8;
    static mips64_user_test_linux_signal_suspend: u8;
    static mips64_user_test_linux_clock: u8;
    static mips64_user_test_linux_stat: u8;
    static mips64_user_test_linux_credentials: u8;
    static mips64_user_test_linux_move_and_discard: u8;
    static mips64_user_test_end: u8;
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked (the caller's duty), so nothing but TLB refills, which do
    // not use `KScratch2`, arrives before the `eret`; the exception that ends the program saves
    // its registers into the context, which `&mut` keeps valid and unaliased, and returns here
    // on the kernel stack with the callee-saved registers and `gp` restored. The program runs in
    // user mode, where only `XUSEG` is reachable.
    unsafe { mips64_enter_user(registers) };
    let cause = read_cause();
    let bad = read_bad_address();
    match exception_code(cause) {
        SYSCALL => {
            registers.pc += INSTRUCTION_SIZE;
            registers.syscall_a3 = registers.r[7];
            return UserExit::Syscall;
        }
        INTERRUPT if interrupt(cause) => return UserExit::Interrupt,
        _ => {}
    }
    registers.cause = cause;
    registers.bad_address = bad;
    UserExit::Exception(decode(cause, bad, registers.pc))
}

impl crate::user::mips64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the `Cause` and
    /// `BadVAddr` it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.cause, self.bad_address, self.pc)
    }
}

/// [`crate::Platform::user_test_program`]: every program, the Linux ones in n64's numbers and
/// values (D416).
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const mips64_user_test_start) as usize;
    let end = (&raw const mips64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const mips64_user_test_checks),
        floating_point: at(&raw const mips64_user_test_floating_point),
        null_syscalls: at(&raw const mips64_user_test_null_syscalls),
        spin: at(&raw const mips64_user_test_spin),
        fp_registers: at(&raw const mips64_user_test_fp_registers),
        illegal_instruction: at(&raw const mips64_user_test_illegal_instruction),
        linux_hello: at(&raw const mips64_user_test_linux_hello),
        linux_start: at(&raw const mips64_user_test_linux_start),
        linux_start_end: at(&raw const mips64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const mips64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const mips64_user_test_stack_growth),
        linux_thread_pointer: at(&raw const mips64_user_test_linux_thread_pointer),
        linux_poll: at(&raw const mips64_user_test_linux_poll),
        linux_signals: at(&raw const mips64_user_test_linux_signals),
        linux_signal_handler: at(&raw const mips64_user_test_linux_signal_handler),
        linux_fault_handler: at(&raw const mips64_user_test_linux_fault_handler),
        linux_signal_queue: at(&raw const mips64_user_test_linux_signal_queue),
        linux_signal_suspend: at(&raw const mips64_user_test_linux_signal_suspend),
        linux_clock: at(&raw const mips64_user_test_linux_clock),
        linux_stat: at(&raw const mips64_user_test_linux_stat),
        linux_credentials: at(&raw const mips64_user_test_linux_credentials),
        linux_move_and_discard: at(&raw const mips64_user_test_linux_move_and_discard),
    }
}

/// [`crate::Platform::page_fault`]: the TLB exceptions, which a stale entry may also raise, and
/// the read and execute inhibits.
pub(super) fn page_fault(kind: ExceptionKind, cause: u64) -> Option<PageFault> {
    let (access, may_be_stale) = match exception_code(cause) {
        TLB_MODIFIED => (Access::Write, true),
        TLB_LOAD if kind == ExceptionKind::InstructionFault => (Access::Execute, true),
        TLB_LOAD => (Access::Read, true),
        TLB_STORE => (Access::Write, true),
        TLB_READ_INHIBIT => (Access::Read, false),
        TLB_EXECUTE_INHIBIT => (Access::Execute, false),
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale,
    })
}

fn decode(cause: u64, bad: u64, pc: u64) -> Exception {
    let fetch = bad == pc;
    let (kind, name, has_address) = match exception_code(cause) {
        INTERRUPT => (ExceptionKind::Interrupt, "interrupt", false),
        TLB_MODIFIED => (ExceptionKind::DataFault, "TLB modified", true),
        TLB_LOAD if fetch => (ExceptionKind::InstructionFault, "TLB (fetch)", true),
        TLB_LOAD => (ExceptionKind::DataFault, "TLB (load)", true),
        TLB_STORE => (ExceptionKind::DataFault, "TLB (store)", true),
        ADDRESS_ERROR_LOAD if fetch => (ExceptionKind::InstructionFault, "address error (fetch)", true),
        ADDRESS_ERROR_LOAD | ADDRESS_ERROR_STORE if bad < (1 << 40) && bad & 7 != 0 => {
            (ExceptionKind::Alignment, "address not aligned", true)
        }
        ADDRESS_ERROR_LOAD => (ExceptionKind::DataFault, "address error (load)", true),
        ADDRESS_ERROR_STORE => (ExceptionKind::DataFault, "address error (store)", true),
        BUS_ERROR_FETCH | BUS_ERROR_DATA => (ExceptionKind::Other, "bus error", false),
        SYSCALL => (ExceptionKind::Other, "syscall", false),
        BREAKPOINT => (ExceptionKind::Breakpoint, "breakpoint (break)", false),
        RESERVED_INSTRUCTION => (ExceptionKind::IllegalInstruction, "reserved instruction", false),
        COPROCESSOR_UNUSABLE => (ExceptionKind::IllegalInstruction, "coprocessor unusable", false),
        OVERFLOW => (ExceptionKind::Arithmetic, "integer overflow", false),
        TRAP => (ExceptionKind::Arithmetic, "trap", false),
        FLOATING_POINT => (ExceptionKind::Arithmetic, "floating-point exception", false),
        TLB_READ_INHIBIT => (ExceptionKind::DataFault, "TLB read inhibit", true),
        TLB_EXECUTE_INHIBIT => (ExceptionKind::InstructionFault, "TLB execute inhibit", true),
        WATCH => (ExceptionKind::Debug, "watch", true),
        _ => (ExceptionKind::Other, "exception", false),
    };
    Exception {
        kind,
        name,
        code: cause,
        pc,
        address: has_address.then_some(bad),
    }
}
