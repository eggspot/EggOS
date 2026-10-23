//! AArch64 exception vectors (Arm ARM, D1.10 "Exception entry").
//!
//! `VBAR_EL1` points at a 2 KiB-aligned table of 16 entries, 128 bytes each: four groups
//! (current EL with SP_EL0, current EL with SP_ELx, lower EL in AArch64, lower EL in AArch32)
//! of four types (synchronous, IRQ, FIQ, SError). Every entry from EL1 saves the general-purpose
//! registers, `ELR_EL1` and `SPSR_EL1` on the current stack, then calls [`aarch64_exception`]
//! with the frame and the entry's index, and restores everything and `eret`s if it returns.
//! Every entry from EL0 (D47) saves the program's registers, `SP_EL0` and its thread pointer
//! `TPIDR_EL0` (D57) included, into its `UserContext`, where `SP_EL1` points while it runs,
//! and returns from [`run_user`] instead. The kernel uses no FP/SIMD registers (D10), so the
//! entry does not save them: a program's are saved and loaded by the scheduler, per thread,
//! once it has used them (`fp`, D54); until then `CPACR_EL1` traps them at EL0 (D49).

use super::timer;
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::user::{self, TestProgram, UserContext, UserExit};

core::arch::global_asm!(
    r#"
    .macro EXCEPTION_ENTRY index
    .balign 0x80
    sub     sp, sp, #{frame_size}
    stp     x0, x1, [sp, #0]
    mov     x1, #\index
    b       exception_common
    .endm

    // From EL0 (D47): `SP_EL1` points just past the saved part of the running program's
    // `UserContext`, so the registers go straight into it.
    .macro USER_ENTRY index
    .balign 0x80
    sub     sp, sp, #{user_saved_size}
    stp     x0, x1, [sp, #0]
    mov     x1, #\index
    b       user_exception_common
    .endm

    .pushsection .text.vectors, "ax"
    .balign 2048
    .global exception_vectors
exception_vectors:
    EXCEPTION_ENTRY 0
    EXCEPTION_ENTRY 1
    EXCEPTION_ENTRY 2
    EXCEPTION_ENTRY 3
    EXCEPTION_ENTRY 4
    EXCEPTION_ENTRY 5
    EXCEPTION_ENTRY 6
    EXCEPTION_ENTRY 7
    USER_ENTRY 8
    USER_ENTRY 9
    USER_ENTRY 10
    USER_ENTRY 11
    USER_ENTRY 12
    USER_ENTRY 13
    USER_ENTRY 14
    USER_ENTRY 15

exception_common:
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    mrs     x2, elr_el1
    stp     x30, x2, [sp, #240]
    mrs     x2, spsr_el1
    str     x2, [sp, #256]

    mov     x0, sp
    bl      {handler}

    ldr     x2, [sp, #256]
    msr     spsr_el1, x2
    ldp     x30, x2, [sp, #240]
    msr     elr_el1, x2
    ldp     x28, x29, [sp, #224]
    ldp     x26, x27, [sp, #208]
    ldp     x24, x25, [sp, #192]
    ldp     x22, x23, [sp, #176]
    ldp     x20, x21, [sp, #160]
    ldp     x18, x19, [sp, #144]
    ldp     x16, x17, [sp, #128]
    ldp     x14, x15, [sp, #112]
    ldp     x12, x13, [sp, #96]
    ldp     x10, x11, [sp, #80]
    ldp     x8, x9, [sp, #64]
    ldp     x6, x7, [sp, #48]
    ldp     x4, x5, [sp, #32]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp, #0]
    add     sp, sp, #{frame_size}
    eret
    dsb     nsh                     // never reached: as after the user-mode `eret`
    isb

    // The rest of the way out of EL0: save the program's registers in its `UserContext`
    // (`sp`), its thread pointer (`TPIDR_EL0`) included, record the vector entry, then return
    // from `aarch64_enter_user` on the kernel stack it left.
user_exception_common:
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    mrs     x2, elr_el1
    stp     x30, x2, [sp, #240]
    mrs     x2, spsr_el1
    mrs     x3, sp_el0
    stp     x2, x3, [sp, #256]
    mrs     x2, tpidr_el0
    str     x2, [sp, #272]
    ldr     x2, [sp, #{user_saved_size}]
    str     x1, [sp, #{user_saved_size} + 8]
    mov     sp, x2
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     x19, x20, [sp], #96
    ret

    // `aarch64_enter_user(context)`: keeps the kernel's callee-saved registers on its stack and
    // the stack pointer in the context, points `SP_EL1` just past the context's saved part and
    // enters EL0 with the program's registers, `TPIDR_EL0` included. Returns through
    // `user_exception_common`.
    .global aarch64_enter_user
aarch64_enter_user:
    stp     x19, x20, [sp, #-96]!
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    mov     x1, sp
    str     x1, [x0, #{user_saved_size}]
    ldp     x2, x3, [x0, #248]
    msr     elr_el1, x2
    msr     spsr_el1, x3
    ldp     x2, x3, [x0, #264]
    msr     sp_el0, x2
    msr     tpidr_el0, x3
    add     sp, x0, #{user_saved_size}
    ldp     x2, x3, [x0, #16]
    ldp     x4, x5, [x0, #32]
    ldp     x6, x7, [x0, #48]
    ldp     x8, x9, [x0, #64]
    ldp     x10, x11, [x0, #80]
    ldp     x12, x13, [x0, #96]
    ldp     x14, x15, [x0, #112]
    ldp     x16, x17, [x0, #128]
    ldp     x18, x19, [x0, #144]
    ldp     x20, x21, [x0, #160]
    ldp     x22, x23, [x0, #176]
    ldp     x24, x25, [x0, #192]
    ldp     x26, x27, [x0, #208]
    ldp     x28, x29, [x0, #224]
    ldr     x30, [x0, #240]
    ldp     x0, x1, [x0, #0]
    eret
    // Never reached: no speculation past the `eret` (straight-line speculation).
    dsb     nsh
    isb
    .popsection

    // The user-mode test program (D47, `TestProgram`): position-independent, no stack, no
    // data. Copied to a user page, never run where it is linked.
    .pushsection .rodata.user_test, "a"
    .balign 4
    .global aarch64_user_test_start
aarch64_user_test_start:
    .global aarch64_user_test_checks
aarch64_user_test_checks:
    mov     x0, #1
    mov     x1, #2
    mov     x2, #3
    mov     x3, #4
    mov     x4, #5
    mov     x5, #6
    .irp n, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
    mov     x\n, #(0x100 + \n)
    .endr
    mov     x8, #{syscall_first}
    svc     #0
    // Every register but x0 must be as it was: x1 collects the differences.
    sub     x1, x1, #2
    .irp n, 2, 3, 4, 5
    sub     x\n, x\n, #(\n + 1)
    orr     x1, x1, x\n
    .endr
    sub     x8, x8, #{syscall_first}
    orr     x1, x1, x8
    .irp n, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
    sub     x\n, x\n, #(0x100 + \n)
    orr     x1, x1, x\n
    .endr
    mov     x8, #{syscall_report}
    svc     #0
    ldr     x1, [x0]
1:  b       1b
    .global aarch64_user_test_floating_point
aarch64_user_test_floating_point:
    .inst   0x9e6703e0              // fmov d0, xzr, spelled out: the target has no FP
    .global aarch64_user_test_null_syscalls
aarch64_user_test_null_syscalls:
    mov     x8, #{syscall_null}
2:  svc     #0
    b       2b
    .global aarch64_user_test_spin
aarch64_user_test_spin:
3:  b       3b
    .global aarch64_user_test_linux_hello
aarch64_user_test_linux_hello:
    // Linux's generic numbers: write 64, exit_group 94. x19 collects the differences.
    mov     x0, #1
    adr     x1, 9f
    mov     x2, #{hello_length}
    mov     x8, #64
    svc     #0
    sub     x19, x0, #{hello_length}
    mov     x0, #1
    mov     x1, #0
    mov     x2, #1
    svc     #0
    add     x0, x0, #14             // -EFAULT
    orr     x19, x19, x0
    mov     x8, #{unknown_syscall}
    svc     #0
    add     x0, x0, #38             // -ENOSYS
    orr     x19, x19, x0
    cmp     x19, #0
    cset    x0, ne
    mov     x8, #94
    svc     #0
4:  b       4b
9:  .ascii  "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global aarch64_user_test_linux_start
aarch64_user_test_linux_start:
    // Linux's generic numbers: writev 66, exit_group 94, sigaltstack 132, rt_sigaction 134,
    // getpid 172, gettid 178, prlimit64 261, getrandom 278. x19 is argc, x20 walks argv and
    // envp, x21 counts the two lists, x22 holds the process id. Each string is written by
    // writev from one iovec on the stack. Ends with exit_group(255) if the stack is
    // misaligned, the thread's id is not the process's, getrandom does not fill the 16 bytes
    // below the stack pointer with something not zero, prlimit64 does not report the stack's
    // limits as 8 MiB soft and unlimited hard, SIGPIPE's action does not read back as SIG_IGN
    // once set, or sigaltstack does not report no alternate stack.
    mov     x9, sp
    mov     x0, #255
    tst     x9, #15
    b.ne    9f
    mov     x8, #172
    svc     #0
    mov     x22, x0
    mov     x8, #178
    svc     #0
    cmp     x0, x22
    mov     x0, #255
    b.ne    9f
    sub     x0, sp, #16
    mov     x1, #16
    mov     x2, #0
    mov     x8, #278
    svc     #0
    cmp     x0, #16
    mov     x0, #255
    b.ne    9f
    ldp     x9, x10, [sp, #-16]
    orr     x9, x9, x10
    cbz     x9, 9f
    mov     x0, #0
    mov     x1, #3                  // RLIMIT_STACK
    mov     x2, #0
    sub     x3, sp, #32
    mov     x8, #261
    svc     #0
    mov     x9, x0
    mov     x0, #255
    cbnz    x9, 9f
    ldp     x9, x10, [sp, #-32]
    mov     x11, #0x800000
    cmp     x9, x11
    b.ne    9f
    cmn     x10, #1
    b.ne    9f
    mov     x9, #1                  // SIG_IGN
    stp     x9, xzr, [sp, #-64]
    stp     xzr, xzr, [sp, #-48]
    mov     x0, #13                 // SIGPIPE
    sub     x1, sp, #64
    mov     x2, #0
    mov     x3, #8
    mov     x8, #134
    svc     #0
    mov     x9, x0
    mov     x0, #255
    cbnz    x9, 9f
    mov     x0, #13
    mov     x1, #0
    sub     x2, sp, #96
    mov     x3, #8
    mov     x8, #134
    svc     #0
    mov     x9, x0
    mov     x0, #255
    cbnz    x9, 9f
    ldr     x9, [sp, #-96]
    cmp     x9, #1
    b.ne    9f
    mov     x0, #0
    sub     x1, sp, #128
    mov     x8, #132
    svc     #0
    mov     x9, x0
    mov     x0, #255
    cbnz    x9, 9f
    ldr     w9, [sp, #-120]
    cmp     w9, #2                  // SS_DISABLE
    b.ne    9f
    ldr     x19, [sp]
    add     x20, sp, #8
    mov     x21, #2
6:  ldr     x1, [x20], #8
    cbz     x1, 8f
    mov     x2, #0
7:  ldrb    w3, [x1, x2]
    cbz     w3, 5f
    add     x2, x2, #1
    b       7b
5:  stp     x1, x2, [sp, #-16]!
    mov     x0, #1
    mov     x1, sp
    mov     x2, #1
    mov     x8, #66
    svc     #0
    add     sp, sp, #16
    b       6b
8:  subs    x21, x21, #1
    b.ne    6b
    mov     x0, x19
9:  mov     x8, #94
    svc     #0
4:  b       4b
    .global aarch64_user_test_linux_start_end
aarch64_user_test_linux_start_end:
    .global aarch64_user_test_linux_unknown_syscalls
aarch64_user_test_linux_unknown_syscalls:
    mov     x8, #{unknown_syscall}
5:  svc     #0
    b       5b
    .global aarch64_user_test_stack_growth
aarch64_user_test_stack_growth:
    // From the stack pointer's page down, a page at a time: each page's address in its
    // first word, until the stack runs out.
    mov     x9, sp
    and     x9, x9, #~0xfff
6:  str     x9, [x9]
    sub     x9, x9, #0x1000
    b       6b
    .global aarch64_user_test_linux_thread_pointer
aarch64_user_test_linux_thread_pointer:
    // Linux's generic numbers: getpid 172, sched_yield 124, exit_group 94. The thread pointer
    // starts at zero; then getpid() << 12, set with `msr`, must outlast every sched_yield.
    // x19 collects the differences.
    mrs     x19, tpidr_el0
    mov     x8, #172
    svc     #0
    lsl     x20, x0, #12
    msr     tpidr_el0, x20
    mov     x21, #{thread_pointer_rounds}
7:  mov     x8, #124
    svc     #0
    orr     x19, x19, x0
    mrs     x0, tpidr_el0
    eor     x0, x0, x20
    orr     x19, x19, x0
    subs    x21, x21, #1
    b.ne    7b
    cmp     x19, #0
    cset    x0, ne
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_linux_poll
aarch64_user_test_linux_poll:
    // Linux's generic numbers: ioctl 29, ppoll 73, exit_group 94. x19 is the exit status
    // until every check passed. The three `struct pollfd`s are at sp-32, the window size at
    // sp-64, the timeout at sp-48.
    mov     x19, #1
    mov     x10, #0                 // fd 0, no events
    mov     x11, #1
    movk    x11, #4, lsl #32        // fd 1, POLLOUT
    mov     x12, #2                 // fd 2, no events
    stp     x10, x11, [sp, #-32]
    str     x12, [sp, #-16]
    stp     xzr, xzr, [sp, #-48]
    sub     x0, sp, #32
    mov     x1, #3
    sub     x2, sp, #48
    mov     x3, #0
    mov     x4, #0
    mov     x8, #73
    svc     #0
    cmp     x0, #1
    b.ne    9f
    ldurh   w9, [sp, #-26]
    cbnz    w9, 9f
    ldurh   w9, [sp, #-18]
    cmp     w9, #4
    b.ne    9f
    ldurh   w9, [sp, #-10]
    cbnz    w9, 9f
    mov     x0, #1
    mov     x1, #0x5413             // TIOCGWINSZ
    sub     x2, sp, #64
    mov     x8, #29
    svc     #0
    cbnz    x0, 9f
    movz    x9, #({poll_wait} & 0xffff)
    movk    x9, #({poll_wait} >> 16), lsl #16
    stp     xzr, x9, [sp, #-48]
    mov     x0, #0
    mov     x1, #0
    sub     x2, sp, #48
    mov     x3, #0
    mov     x4, #0
    mov     x8, #73
    svc     #0
    cbnz    x0, 9f
    ldp     x9, x10, [sp, #-48]
    orr     x9, x9, x10
    cbnz    x9, 9f
    mov     x19, #0
9:  mov     x0, x19
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_linux_signals
aarch64_user_test_linux_signals:
    // Linux's generic numbers: exit_group 94, kill 129, tgkill 131, rt_sigprocmask 135,
    // rt_sigpending 136, getpid 172. The signal set is at sp-16, the pending set at sp-32.
    mov     x0, #0
    mov     x1, #17                 // SIGCHLD, ignored
    mov     x8, #129
    svc     #0
    cbnz    x0, 9f
    mov     x9, #(1 << 9)           // SIGUSR1
    str     x9, [sp, #-16]
    mov     x0, #0                  // SIG_BLOCK
    sub     x1, sp, #16
    mov     x2, #0
    mov     x3, #8
    mov     x8, #135
    svc     #0
    cbnz    x0, 9f
    mov     x8, #172
    svc     #0
    mov     x1, x0
    mov     x2, #10                 // SIGUSR1
    mov     x8, #131
    svc     #0
    cbnz    x0, 9f
    sub     x0, sp, #32
    mov     x1, #8
    mov     x8, #136
    svc     #0
    cbnz    x0, 9f
    ldur    x9, [sp, #-32]
    cmp     x9, #(1 << 9)
    b.ne    9f
    mov     x0, #1                  // SIG_UNBLOCK: SIGUSR1 ends the program here
    sub     x1, sp, #16
    mov     x2, #0
    mov     x3, #8
    mov     x8, #135
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_linux_signal_handler
aarch64_user_test_linux_signal_handler:
    // Linux's generic numbers: exit_group 94, tgkill 131, sigaltstack 132, rt_sigaction 134,
    // rt_sigprocmask 135, getpid 172. Its `struct sigaction` is at sp-64, its `stack_t` at
    // sp-32, its alternate stack 0x2000 bytes from sp-0x2800, the mask read back at sp-72.
    // No SA_RESTORER, as glibc: the handler returns through the kernel's trampoline.
    sub     x9, sp, #0x2000
    sub     x9, x9, #0x800
    stp     x9, xzr, [sp, #-32]
    mov     x9, #0x2000
    str     x9, [sp, #-16]
    sub     x0, sp, #32
    mov     x1, #0
    mov     x8, #132
    svc     #0
    cbnz    x0, 9f
    adr     x9, 5f
    movz    x10, #0x0004            // SA_SIGINFO
    movk    x10, #0x0800, lsl #16   // SA_ONSTACK
    stp     x9, x10, [sp, #-64]
    stp     xzr, xzr, [sp, #-48]    // no restorer, no mask
    mov     x0, #10                 // SIGUSR1
    sub     x1, sp, #64
    mov     x2, #0
    mov     x3, #8
    mov     x8, #134
    svc     #0
    cbnz    x0, 9f
    mov     x8, #172
    svc     #0
    mov     x1, x0
    mov     x2, #10
    mov     x19, #0x5a5             // kept across the handler, which clobbers it
    mov     x20, #1                 // the handler sets it to 2 in its frame
    mov     x8, #131
    svc     #0                      // the handler runs before this returns
    cbnz    x0, 9f
    cmp     x19, #0x5a5
    b.ne    9f
    cmp     x20, #2
    b.ne    9f
    mov     x0, #0                  // the mask is back: nothing blocked
    mov     x1, #0
    sub     x2, sp, #72
    mov     x3, #8
    mov     x8, #135
    svc     #0
    cbnz    x0, 9f
    ldur    x9, [sp, #-72]
    cbnz    x9, 9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    // The handler: SIGUSR1 from tgkill (SI_TKILL), on the alternate stack the ucontext names;
    // then it sets the frame's x20 to 2 and clobbers x19.
5:  cmp     x0, #10
    b.ne    7f
    ldr     w9, [x1]
    cmp     w9, #10
    b.ne    7f
    ldr     w9, [x1, #8]
    cmn     w9, #6
    b.ne    7f
    ldr     x9, [x2, #16]           // uc_stack.ss_sp
    mov     x10, sp
    cmp     x10, x9
    b.lo    7f
    ldr     x11, [x2, #32]          // uc_stack.ss_size
    add     x9, x9, x11
    cmp     x10, x9
    b.hs    7f
    mov     x9, #2
    str     x9, [x2, #{saved_x20}]
7:  mov     x19, #0
    ret
    .global aarch64_user_test_linux_fault_handler
aarch64_user_test_linux_fault_handler:
    // Linux's generic numbers: exit_group 94, rt_sigaction 134, mmap 222. Its `struct
    // sigaction` is at sp-64. No SA_RESTORER: the handler returns through the trampoline.
    adr     x9, 5f
    mov     x10, #4                 // SA_SIGINFO
    stp     x9, x10, [sp, #-64]
    stp     xzr, xzr, [sp, #-48]    // no restorer, no mask
    mov     x0, #11                 // SIGSEGV
    sub     x1, sp, #64
    mov     x2, #0
    mov     x3, #8
    mov     x8, #134
    svc     #0
    cbnz    x0, 9f
    mov     x0, #0                  // mmap(0, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS)
    mov     x1, #4096
    mov     x2, #0
    mov     x3, #0x22
    mov     x4, #-1
    mov     x5, #0
    mov     x8, #222
    svc     #0
    cmn     x0, #4095
    b.hs    9f
    mov     x24, x0
    mov     x20, #0                 // the faults the handler counted
    mov     x21, #{fault_address}   // nothing mapped there: SEGV_MAPERR
    mov     x22, #1
    adr     x23, 1f
    ldr     x9, [x21]
    b       9f
1:  mov     x21, x24                // PROT_NONE: SEGV_ACCERR
    mov     x22, #2
    adr     x23, 2f
    ldr     x9, [x21]
    b       9f
2:  cmp     x20, #2
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    // The handler: SIGSEGV with the si_code and si_addr in the frame's x22 and x21, which
    // fault_address and the esr_context record report too; it counts it in the frame's x20,
    // and resumes the program at its x23, whatever the checks found.
5:  cmp     x0, #11
    b.ne    7f
    ldr     w9, [x1]
    cmp     w9, #11
    b.ne    7f
    ldr     w9, [x1, #8]
    ldr     x10, [x2, #{saved_x22}]
    cmp     x9, x10
    b.ne    7f
    ldr     x9, [x1, #16]
    ldr     x10, [x2, #{saved_x21}]
    cmp     x9, x10
    b.ne    7f
    ldr     x10, [x2, #{fault_address_in_ucontext}]
    cmp     x9, x10
    b.ne    7f
    ldr     x9, [x2, #{esr_record}]
    movz    x10, #0x5201
    movk    x10, #0x4553, lsl #16
    movk    x10, #0x10, lsl #32     // ESR_MAGIC, 16 bytes
    cmp     x9, x10
    b.ne    7f
    ldr     x9, [x2, #{saved_x20}]
    add     x9, x9, #1
    str     x9, [x2, #{saved_x20}]
7:  ldr     x9, [x2, #{saved_x23}]
    str     x9, [x2, #{saved_pc}]
    ret
    .global aarch64_user_test_linux_signal_queue
aarch64_user_test_linux_signal_queue:
    sub     x22, sp, #288
    mov     x9, #(1 << ({queued_signal} - 1))
    str     x9, [x22, #272]
    stp     xzr, xzr, [x22, #256]
    str     xzr, [x22, #144]
    mov     x0, #0
    add     x1, x22, #272
    mov     x2, #0
    mov     x3, #8
    mov     x8, #135
    svc     #0
    cbnz    x0, 9f
    mov     x8, #172
    svc     #0
    mov     x19, x0
    mov     w9, #{queued_signal}
    str     w9, [x22, #128]
    mov     w9, #-1
    str     w9, [x22, #136]
    str     w19, [x22, #144]
    mov     x9, #{first_value}
    str     x9, [x22, #152]
    mov     x0, x19
    mov     x1, #{queued_signal}
    add     x2, x22, #128
    mov     x8, #138
    svc     #0
    cbnz    x0, 9f
    mov     x9, #{second_value}
    str     x9, [x22, #152]
    mov     x0, x19
    mov     x1, #{queued_signal}
    add     x2, x22, #128
    mov     x8, #138
    svc     #0
    cbnz    x0, 9f
    mov     x0, x19
    mov     x1, #{queued_signal}
    mov     x8, #129
    svc     #0
    cbnz    x0, 9f
    mov     x20, #-1
    mov     x21, #{first_value}
    bl      6f
    mov     x21, #{second_value}
    bl      6f
    mov     x20, #0
    mov     x21, #0
    bl      6f
    add     x0, x22, #272
    mov     x1, x22
    add     x2, x22, #256
    mov     x3, #8
    mov     x8, #137
    svc     #0
    cmn     x0, #11
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
6:  add     x0, x22, #272
    mov     x1, x22
    add     x2, x22, #256
    mov     x3, #8
    mov     x8, #137
    svc     #0
    cmp     x0, #{queued_signal}
    b.ne    9b
    ldr     w9, [x22]
    cmp     w9, #{queued_signal}
    b.ne    9b
    ldrsw   x9, [x22, #8]
    cmp     x9, x20
    b.ne    9b
    ldr     x9, [x22, #16]
    cmp     x9, x19
    b.ne    9b
    ldr     x9, [x22, #24]
    cmp     x9, x21
    b.ne    9b
    ret
    .global aarch64_user_test_linux_signal_suspend
aarch64_user_test_linux_signal_suspend:
    sub     sp, sp, #64
    adr     x9, 5f
    mov     x10, #4
    stp     x9, x10, [sp]
    stp     xzr, xzr, [sp, #16]
    mov     x0, #10
    mov     x1, sp
    mov     x2, #0
    mov     x3, #8
    mov     x8, #134
    svc     #0
    cbnz    x0, 9f
    mov     x9, #(1 << 9)
    str     x9, [sp, #32]
    str     xzr, [sp, #40]
    mov     x0, #0
    add     x1, sp, #32
    mov     x2, #0
    mov     x3, #8
    mov     x8, #135
    svc     #0
    cbnz    x0, 9f
    mov     x8, #172
    svc     #0
    mov     x1, x0
    mov     x2, #10
    mov     x8, #131
    svc     #0
    cbnz    x0, 9f
    mov     x20, #1
    add     x0, sp, #40
    mov     x1, #8
    mov     x8, #133
    svc     #0
    cmn     x0, #4
    b.ne    9f
    cmp     x20, #2
    b.ne    9f
    mov     x0, #0
    mov     x1, #0
    add     x2, sp, #48
    mov     x3, #8
    mov     x8, #135
    svc     #0
    cbnz    x0, 9f
    ldr     x9, [sp, #48]
    cmp     x9, #(1 << 9)
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
5:  cmp     x0, #10
    b.ne    7f
    ldr     x9, [x2, #{uc_sigmask}]
    cmp     x9, #(1 << 9)
    b.ne    7f
    mov     x9, #2
    str     x9, [x2, #{saved_x20}]
7:  ret
    .global aarch64_user_test_linux_clock
aarch64_user_test_linux_clock:
    sub     sp, sp, #64
    mov     x0, #1
    mov     x1, sp
    mov     x8, #113
    svc     #0
    cbnz    x0, 9f
    movz    x9, #({clock_wait} & 0xffff)
    movk    x9, #({clock_wait} >> 16), lsl #16
    stp     xzr, x9, [sp, #32]
    add     x0, sp, #32
    mov     x1, #0
    mov     x8, #101
    svc     #0
    cbnz    x0, 9f
    mov     x0, #1
    add     x1, sp, #16
    mov     x8, #113
    svc     #0
    cbnz    x0, 9f
    ldp     x10, x11, [sp]
    ldp     x12, x13, [sp, #16]
    sub     x12, x12, x10
    movz    x14, #0xca00
    movk    x14, #0x3b9a, lsl #16
    mul     x12, x12, x14
    add     x12, x12, x13
    sub     x12, x12, x11
    ldr     x9, [sp, #40]
    cmp     x12, x9
    b.lo    9f
    mov     x0, #2
    add     x1, sp, #16
    mov     x8, #113
    svc     #0
    cmn     x0, #22
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_linux_stat
aarch64_user_test_linux_stat:
    sub     sp, sp, #272
    strb    wzr, [sp, #256]
    mov     x0, #1
    mov     x1, sp
    mov     x8, #80
    svc     #0
    cbnz    x0, 9f
    ldr     w9, [sp, #16]
    and     w9, w9, #0xf000
    cmp     w9, #0x2000
    b.ne    9f
    ldr     x9, [sp, #32]
    cmp     x9, #0x501
    b.ne    9f
    str     xzr, [sp, #32]
    mov     x0, #1
    add     x1, sp, #256
    mov     x2, sp
    mov     x3, #0x1000
    mov     x8, #79
    svc     #0
    cbnz    x0, 9f
    ldr     x9, [sp, #32]
    cmp     x9, #0x501
    b.ne    9f
    strh    wzr, [sp, #28]
    str     wzr, [sp, #128]
    mov     x0, #1
    add     x1, sp, #256
    mov     x2, #0x1000
    mov     x3, #0x7ff
    mov     x4, sp
    mov     x8, #291
    svc     #0
    cbnz    x0, 9f
    ldrh    w9, [sp, #28]
    and     w9, w9, #0xf000
    cmp     w9, #0x2000
    b.ne    9f
    ldr     w9, [sp, #128]
    cmp     w9, #5
    b.ne    9f
    mov     x0, #1
    mov     x1, #0
    mov     x2, #1
    mov     x8, #62
    svc     #0
    cmn     x0, #29
    b.ne    9f
    mov     x0, #3
    mov     x1, sp
    mov     x8, #80
    svc     #0
    cmn     x0, #9
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_linux_credentials
aarch64_user_test_linux_credentials:
    mov     x8, #175
    svc     #0
    cbnz    x0, 9f
    mov     x0, #1000
    mov     x8, #146
    svc     #0
    cbnz    x0, 9f
    mov     x8, #174
    svc     #0
    cmp     x0, #1000
    b.ne    9f
    mov     x0, #0
    mov     x8, #146
    svc     #0
    cmn     x0, #1
    b.ne    9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_fp_registers
aarch64_user_test_fp_registers:
    mov     x8, #{syscall_first}
    svc     #0
    mov     x19, x0
    mov     x20, #0
    .arch_extension fp
    .arch_extension simd
    mrs     x9, fpcr
    orr     x20, x20, x9
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fmov    x9, d\i
    orr     x20, x20, x9
    mov     x9, v\i\().d[1]
    orr     x20, x20, x9
    .endr
    and     x9, x19, #3
    lsl     x9, x9, #22
    msr     fpcr, x9
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    add     x9, x19, #\i
    dup     v\i\().2d, x9
    .endr
7:  mov     x8, #{syscall_null}
    svc     #0
    mov     x21, x0
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    add     x9, x19, #\i
    fmov    x10, d\i
    eor     x10, x10, x9
    orr     x20, x20, x10
    mov     x10, v\i\().d[1]
    eor     x10, x10, x9
    orr     x20, x20, x10
    .endr
    mrs     x9, fpcr
    and     x10, x19, #3
    lsl     x10, x10, #22
    eor     x9, x9, x10
    orr     x20, x20, x9
    .arch_extension nosimd
    .arch_extension nofp
    cbz     x21, 7b
    mov     x0, x20
    mov     x8, #{syscall_report}
    svc     #0
8:  b       8b
    .global aarch64_user_test_illegal_instruction
aarch64_user_test_illegal_instruction:
    udf     #0
    .global aarch64_user_test_linux_move_and_discard
aarch64_user_test_linux_move_and_discard:
    mov     x0, #0
    mov     x1, #4096
    mov     x2, #3
    mov     x3, #0x22
    mov     x4, #-1
    mov     x5, #0
    mov     x8, #222
    svc     #0
    cmn     x0, #4096
    b.hs    9f
    mov     x19, x0
    mov     x1, #4096
    mov     x2, #23
    mov     x8, #233
    svc     #0
    cbnz    x0, 9f
    movz    x9, #0x5678
    movk    x9, #0x1234, lsl #16
    str     x9, [x19]
    sub     x20, x19, #0x100000
    mov     x0, x19
    mov     x1, #4096
    mov     x2, #4096
    mov     x3, #3
    mov     x4, x20
    mov     x8, #216
    svc     #0
    cmp     x0, x20
    b.ne    9f
    ldr     x10, [x20]
    cmp     x10, x9
    b.ne    9f
    mov     x0, x19
    mov     x1, #4096
    mov     x2, #3
    mov     x3, #0x32
    mov     x4, #-1
    mov     x5, #0
    mov     x8, #222
    svc     #0
    cmp     x0, x19
    b.ne    9f
    ldr     x10, [x19]
    cbnz    x10, 9f
    mov     x0, x20
    mov     x1, #4096
    mov     x2, #4
    mov     x8, #233
    svc     #0
    cbnz    x0, 9f
    ldr     x10, [x20]
    cbnz    x10, 9f
    mov     x0, #0
    mov     x8, #94
    svc     #0
9:  mov     x0, #1
    mov     x8, #94
    svc     #0
8:  b       8b
    .global aarch64_user_test_end
aarch64_user_test_end:
    .popsection
"#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    user_saved_size = const user::aarch64::SAVED_SIZE,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    // `uc_mcontext.regs[20]` in the ucontext: the sigcontext at 176, its registers at 8.
    saved_x20 = const 176 + 8 + 20 * 8,
    saved_x21 = const 176 + 8 + 21 * 8,
    saved_x22 = const 176 + 8 + 22 * 8,
    saved_x23 = const 176 + 8 + 23 * 8,
    // `uc_mcontext.pc` and `fault_address`, and the record after `__reserved`'s FP/SIMD one.
    saved_pc = const 176 + 264,
    fault_address_in_ucontext = const 176,
    esr_record = const 176 + 288 + 528,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    uc_sigmask = const 40,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
    handler = sym aarch64_exception,
);

/// The registers saved by the entry code, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// x0 to x30.
    x: [u64; 31],
    elr: u64,
    spsr: u64,
    /// Keeps the frame a multiple of 16 bytes, as the stack pointer must stay aligned.
    _pad: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);

/// Exception classes (`ESR_EL1.EC`, Arm ARM D24.2.40).
const EC_UNKNOWN: u64 = 0x00;
const EC_FP_ACCESS: u64 = 0x07;
const EC_ILLEGAL_STATE: u64 = 0x0e;
const EC_SVC64: u64 = 0x15;
const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
const EC_INSTRUCTION_ABORT: u64 = 0x21;
const EC_PC_ALIGNMENT: u64 = 0x22;
const EC_DATA_ABORT_LOWER: u64 = 0x24;
const EC_DATA_ABORT: u64 = 0x25;
const EC_SP_ALIGNMENT: u64 = 0x26;
const EC_BRK64: u64 = 0x3c;

/// `ESR_EL1.ISS.FnV` for aborts: set when `FAR_EL1` does not hold the faulting address.
const ISS_FAR_NOT_VALID: u64 = 1 << 10;
/// `ESR_EL1.ISS.WnR` for data aborts: the access wrote memory.
const ISS_WRITE_NOT_READ: u64 = 1 << 6;
/// `ESR_EL1.ISS.CM` for data aborts: a cache maintenance instruction faulted, which reports
/// `WnR` set though it writes nothing.
const ISS_CACHE_MAINTENANCE: u64 = 1 << 8;

/// Entry types within each group of four vector entries.
const TYPE_SYNCHRONOUS: u64 = 0;
const TYPE_IRQ: u64 = 1;
const TYPE_FIQ: u64 = 2;

/// Size of every A64 instruction, `brk` included.
const INSTRUCTION_SIZE: u64 = 4;

/// Called by the entry code with the saved registers and the vector entry's index (0-15).
extern "C" fn aarch64_exception(frame: &mut TrapFrame, index: u64) {
    if index % 4 == TYPE_IRQ {
        irq(frame.elr);
        return;
    }
    let esr: u64;
    let far: u64;
    // SAFETY: reading the syndrome register at EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack)) };
    // SAFETY: reading the fault address register at EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack)) };

    let exception = decode(index % 4, esr, far, frame.elr);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint {
        frame.elr += INSTRUCTION_SIZE;
    }
}

unsafe extern "C" {
    /// Enters EL0 with `context`'s registers; returns once an exception from EL0 has saved them
    /// back, with the vector entry in `context.vector`.
    fn aarch64_enter_user(context: *mut user::aarch64::Registers);
    static aarch64_user_test_start: u8;
    static aarch64_user_test_checks: u8;
    static aarch64_user_test_floating_point: u8;
    static aarch64_user_test_null_syscalls: u8;
    static aarch64_user_test_spin: u8;
    static aarch64_user_test_fp_registers: u8;
    static aarch64_user_test_illegal_instruction: u8;
    static aarch64_user_test_linux_hello: u8;
    static aarch64_user_test_linux_start: u8;
    static aarch64_user_test_linux_start_end: u8;
    static aarch64_user_test_linux_unknown_syscalls: u8;
    static aarch64_user_test_stack_growth: u8;
    static aarch64_user_test_linux_thread_pointer: u8;
    static aarch64_user_test_linux_poll: u8;
    static aarch64_user_test_linux_signals: u8;
    static aarch64_user_test_linux_signal_handler: u8;
    static aarch64_user_test_linux_fault_handler: u8;
    static aarch64_user_test_linux_signal_queue: u8;
    static aarch64_user_test_linux_signal_suspend: u8;
    static aarch64_user_test_linux_clock: u8;
    static aarch64_user_test_linux_stat: u8;
    static aarch64_user_test_linux_credentials: u8;
    static aarch64_user_test_linux_move_and_discard: u8;
    static aarch64_user_test_end: u8;
}

/// `SPSR_EL1`'s condition flags (N, Z, C, V): all a program may keep of it. The rest is zero:
/// EL0 with `SP_EL0` (EL0t), AArch64, and debug, SError, IRQ and FIQ unmasked.
const SPSR_FLAGS: u64 = 0xf << 28;

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    registers.spsr &= SPSR_FLAGS;
    // SAFETY: interrupts are masked (the caller's duty), so nothing runs between the switch
    // of `SP_EL1` to the context and the `eret`; the exception that ends the program saves its
    // registers into the context, which `&mut` keeps valid and unaliased, and returns here on
    // the kernel stack with the callee-saved registers restored. `SPSR_EL1` is sanitized above,
    // so the program runs at EL0, where it reaches only user mappings and cannot change
    // `VBAR_EL1`, `SP_EL1` or the translation tables.
    unsafe { aarch64_enter_user(registers) };
    let index = registers.vector;
    if index % 4 == TYPE_IRQ {
        irq(registers.elr);
        return UserExit::Interrupt;
    }
    let esr: u64;
    let far: u64;
    // SAFETY: as in `aarch64_exception`: no side effects. Nothing since the exception from EL0
    // has taken another (interrupts are masked and the kernel code above cannot fault).
    unsafe { core::arch::asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack)) };
    // SAFETY: as above.
    unsafe { core::arch::asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack)) };
    if index % 4 == TYPE_SYNCHRONOUS && esr >> 26 & 0x3f == EC_SVC64 {
        // `ELR_EL1` already points after the `svc`.
        return UserExit::Syscall;
    }
    // With `vector`, all `last_exception` needs.
    registers.esr = esr;
    registers.far = far;
    UserExit::Exception(decode(index % 4, esr, far, registers.elr))
}

impl crate::user::aarch64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the vector
    /// entry, `ESR_EL1` and `FAR_EL1` it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.vector % 4, self.esr, self.far, self.elr)
    }
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const aarch64_user_test_start) as usize;
    let end = (&raw const aarch64_user_test_end) as usize;
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, which is part of the
    // kernel image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: (&raw const aarch64_user_test_checks) as usize - start,
        floating_point: (&raw const aarch64_user_test_floating_point) as usize - start,
        null_syscalls: (&raw const aarch64_user_test_null_syscalls) as usize - start,
        spin: (&raw const aarch64_user_test_spin) as usize - start,
        fp_registers: (&raw const aarch64_user_test_fp_registers) as usize - start,
        illegal_instruction: (&raw const aarch64_user_test_illegal_instruction) as usize - start,
        linux_hello: (&raw const aarch64_user_test_linux_hello) as usize - start,
        linux_start: (&raw const aarch64_user_test_linux_start) as usize - start,
        linux_start_end: (&raw const aarch64_user_test_linux_start_end) as usize - start,
        linux_unknown_syscalls: (&raw const aarch64_user_test_linux_unknown_syscalls) as usize
            - start,
        stack_growth: (&raw const aarch64_user_test_stack_growth) as usize - start,
        linux_thread_pointer: (&raw const aarch64_user_test_linux_thread_pointer) as usize - start,
        linux_poll: (&raw const aarch64_user_test_linux_poll) as usize - start,
        linux_signals: (&raw const aarch64_user_test_linux_signals) as usize - start,
        linux_signal_handler: (&raw const aarch64_user_test_linux_signal_handler) as usize - start,
        linux_fault_handler: (&raw const aarch64_user_test_linux_fault_handler) as usize - start,
        linux_signal_queue: (&raw const aarch64_user_test_linux_signal_queue) as usize - start,
        linux_signal_suspend: (&raw const aarch64_user_test_linux_signal_suspend) as usize - start,
        linux_clock: (&raw const aarch64_user_test_linux_clock) as usize - start,
        linux_stat: (&raw const aarch64_user_test_linux_stat) as usize - start,
        linux_credentials: (&raw const aarch64_user_test_linux_credentials) as usize - start,
        linux_move_and_discard: (&raw const aarch64_user_test_linux_move_and_discard) as usize
            - start,
    }
}

/// Takes the pending interrupt from the GIC and handles it.
// Inlined into both callers, as when the kernel's vectors were its only one: the kernel's
// trap path keeps its length (D47).
#[inline(always)]
fn irq(pc: u64) {
    let Some(interrupt) = timer::acknowledge() else {
        return; // Spurious: the interrupt went away before it was acknowledged.
    };
    // End the interrupt before handing it on: the kernel may not return here soon (later, a
    // context switch), and `PSTATE.I` keeps this CPU from taking another one meanwhile.
    let id = interrupt.id();
    let timer_fired = id == timer::VIRTUAL_TIMER_INTERRUPT && timer::expired();
    if timer_fired {
        timer::disarm();
    }
    timer::end_of_interrupt(interrupt);
    match id {
        // A stale interrupt for a deadline since moved later is dropped: never early.
        timer::VIRTUAL_TIMER_INTERRUPT => {
            if timer_fired {
                crate::interrupt::deliver(Interrupt::Timer);
            }
        }
        timer::WAKEUP_SGI => crate::interrupt::deliver(Interrupt::Wakeup),
        _ => deliver(&Exception {
            kind: ExceptionKind::Interrupt,
            name: "unexpected IRQ",
            code: id.into(),
            pc,
            address: None,
        }),
    }
}

// Inlined into both callers, as when the kernel's vectors were its only one: the kernel's
// trap path keeps its length (D47).
#[inline(always)]
fn decode(entry_type: u64, esr: u64, far: u64, pc: u64) -> Exception {
    let class = esr >> 26 & 0x3f;
    let (kind, name, far_valid) = match entry_type {
        TYPE_SYNCHRONOUS => {
            let (kind, name) = synchronous_kind(class);
            let far_valid = match class {
                EC_PC_ALIGNMENT => true,
                EC_INSTRUCTION_ABORT_LOWER
                | EC_INSTRUCTION_ABORT
                | EC_DATA_ABORT_LOWER
                | EC_DATA_ABORT => esr & ISS_FAR_NOT_VALID == 0,
                _ => false,
            };
            (kind, name, far_valid)
        }
        TYPE_IRQ => (ExceptionKind::Interrupt, "IRQ", false),
        TYPE_FIQ => (ExceptionKind::Interrupt, "FIQ", false),
        _ => (ExceptionKind::Other, "SError (asynchronous abort)", false),
    };
    Exception {
        kind,
        name,
        code: esr,
        pc,
        address: far_valid.then_some(far),
    }
}

/// [`crate::Platform::page_fault`]: an instruction or data abort whose fault status code is a
/// translation fault (`0b0001LL`, no valid entry at level `LL`), an access flag fault
/// (`0b0010LL`) or a permission fault (`0b0011LL`); alignment faults, external aborts, TLB
/// conflicts and unsupported atomics are not page faults. A data abort writes if `WnR` is set,
/// unless a cache maintenance instruction faulted (it reports `WnR` though it writes nothing).
/// AArch64 never caches an invalid entry, but a translation fault on an entry another CPU
/// has just made valid can still race with it, so only permission faults are never stale.
pub(super) fn page_fault(esr: u64) -> Option<crate::paging::PageFault> {
    use crate::paging::{Access, PageFault};
    /// Fault status codes: translation faults are `0b0001LL`, access flag faults `0b0010LL`,
    /// permission faults `0b0011LL`.
    const TRANSLATION_FAULT: u64 = 0b00_0100;
    const PERMISSION_FAULT: u64 = 0b00_1100;
    let status = esr & 0x3f;
    if !(TRANSLATION_FAULT..=PERMISSION_FAULT | 0b11).contains(&status) {
        return None;
    }
    let access = match esr >> 26 & 0x3f {
        EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => Access::Execute,
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT
            if esr & (ISS_WRITE_NOT_READ | ISS_CACHE_MAINTENANCE) == ISS_WRITE_NOT_READ =>
        {
            Access::Write
        }
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT => Access::Read,
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale: status < PERMISSION_FAULT,
    })
}

fn synchronous_kind(class: u64) -> (ExceptionKind, &'static str) {
    match class {
        EC_UNKNOWN => (ExceptionKind::IllegalInstruction, "undefined instruction"),
        EC_FP_ACCESS => (ExceptionKind::IllegalInstruction, "SIMD/FP access trap"),
        EC_ILLEGAL_STATE => (ExceptionKind::IllegalInstruction, "illegal execution state"),
        EC_SVC64 => (ExceptionKind::Other, "supervisor call (svc)"),
        EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => {
            (ExceptionKind::InstructionFault, "instruction abort")
        }
        EC_PC_ALIGNMENT => (ExceptionKind::Alignment, "PC alignment fault"),
        EC_DATA_ABORT_LOWER | EC_DATA_ABORT => (ExceptionKind::DataFault, "data abort"),
        EC_SP_ALIGNMENT => (ExceptionKind::Alignment, "SP alignment fault"),
        EC_BRK64 => (ExceptionKind::Breakpoint, "breakpoint (brk)"),
        _ => (ExceptionKind::Other, "synchronous exception"),
    }
}
