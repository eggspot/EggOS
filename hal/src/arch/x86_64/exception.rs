//! x86_64 exception and interrupt vectors (Intel SDM vol. 3A, chapter 6 "Interrupt and
//! Exception Handling").
//!
//! The IDT has a gate for each of the 32 architecture-defined exception vectors and for each
//! interrupt vector in use (the legacy PICs' 0x20-0x2f, the APIC timer's 0x30, the wake-up
//! interrupt's 0x31 and the APIC's spurious vector 0xff, see [`super::timer`]). Each points at
//! a 16-byte stub that pushes the vector number (and a 0 error code for the vectors where the
//! CPU pushes none), so every exception and interrupt reaches `exception_common` with the same
//! frame. From the kernel, that saves the registers a call may change (the handler keeps the
//! others itself), calls [`x86_64_exception`] with the frame, and restores them and `iretq`s if
//! it returns. From ring 3 (D47), the CPU has switched to this CPU's `TSS.RSP0`, which points
//! into the running program's `UserContext`: the entry saves every register there, switches
//! back to the kernel's GS base and returns from [`run_user`]; `syscall` builds the same frame
//! and joins it. The kernel target uses no SSE/x87 registers (`x86_64-unknown-none` is
//! soft-float), so they are not saved, and programs may not use them either: `CR0.TS` traps
//! them (D49).
//!
//! All exceptions run on the current stack: there is no IST stack yet, so a stack overflow
//! that faults again ends in a triple fault (a reset, which the QEMU runner detects). Separate
//! stacks for double faults, NMIs and machine checks are needed before untrusted programs run
//! (D47): `syscall` and `sysretq` leave a moment in ring 0 on the program's stack.

use super::timer::{self, APIC_SPURIOUS_VECTOR, LEGACY_PIC_VECTORS, TIMER_VECTOR, WAKEUP_VECTOR};
use crate::cpu::{
    FS_BASE_ID_OFFSET, GDT_ENTRIES, HalCpuLocal, TSS_RSP0_OFFSET, Tss, USER_RSP_OFFSET,
};
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Format, x86_64::X86_64};
use crate::user::x86_64::{RESTORE_ALL_VECTOR, Registers, SYSCALL_VECTOR};
use crate::user::{TestProgram, UserContext, UserExit};
use core::cell::UnsafeCell;

core::arch::global_asm!(
    r#"
    // Each stub is at most 15 bytes (two pushes of up to 5 bytes and a 5-byte jmp), padded
    // to 16.
    .macro EXCEPTION_STUB vector
    .balign 16
    pushq   $0
    pushq   $\vector
    jmp     exception_common
    .endm

    .macro EXCEPTION_STUB_WITH_ERROR_CODE vector
    .balign 16
    pushq   $\vector
    jmp     exception_common
    .endm

    .pushsection .text.exceptions, "ax"
    .balign 16
    .global exception_stubs
exception_stubs:
    EXCEPTION_STUB 0
    EXCEPTION_STUB 1
    EXCEPTION_STUB 2
    EXCEPTION_STUB 3
    EXCEPTION_STUB 4
    EXCEPTION_STUB 5
    EXCEPTION_STUB 6
    EXCEPTION_STUB 7
    EXCEPTION_STUB_WITH_ERROR_CODE 8
    EXCEPTION_STUB 9
    EXCEPTION_STUB_WITH_ERROR_CODE 10
    EXCEPTION_STUB_WITH_ERROR_CODE 11
    EXCEPTION_STUB_WITH_ERROR_CODE 12
    EXCEPTION_STUB_WITH_ERROR_CODE 13
    EXCEPTION_STUB_WITH_ERROR_CODE 14
    EXCEPTION_STUB 15
    EXCEPTION_STUB 16
    EXCEPTION_STUB_WITH_ERROR_CODE 17
    EXCEPTION_STUB 18
    EXCEPTION_STUB 19
    EXCEPTION_STUB 20
    EXCEPTION_STUB_WITH_ERROR_CODE 21
    EXCEPTION_STUB 22
    EXCEPTION_STUB 23
    EXCEPTION_STUB 24
    EXCEPTION_STUB 25
    EXCEPTION_STUB 26
    EXCEPTION_STUB 27
    EXCEPTION_STUB 28
    EXCEPTION_STUB_WITH_ERROR_CODE 29
    EXCEPTION_STUB_WITH_ERROR_CODE 30
    EXCEPTION_STUB 31
    // Interrupts 0x20-0x31 (legacy PIC inputs 0-15, APIC timer, wake-up); the CPU pushes no
    // error code.
    .irp vector, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49
    EXCEPTION_STUB \vector
    .endr
    .balign 16                      // so the label is the stub, not the padding before it
    .global apic_spurious_stub
apic_spurious_stub:
    EXCEPTION_STUB 255

exception_common:
    // From ring 3 (D47)? The code segment the CPU pushed is above the vector and error code.
    testb   $3, 24(%rsp)
    jnz     user_exception
    // From the kernel: the handler keeps the callee-saved registers itself, as any call does.
    pushq   %rax
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
    // The CPU aligned the stack to 16 bytes before pushing its 5-word frame; with the error
    // code, the vector and 9 registers (16 words) it is aligned again, as the call needs.
    movq    %rsp, %rdi
    cld
    call    {handler}
    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rax
    addq    $16, %rsp               // vector and error code
    iretq
    int3                            // never runs: stops straight-line speculation (D49)

    // From ring 3: the CPU switched to `TSS.RSP0`, which points just past the saved part of the
    // running program's `UserContext`, so the frame and the registers go straight into it.
    // Then back to the kernel's GS base, and return from `x86_64_enter_user` on the kernel
    // stack it left.
user_exception:
    swapgs
user_exception_save:
    pushq   %rax
    pushq   %rbx
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %rbp
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    // The program's direction flag is its own; the kernel's code needs it clear.
    cld
    movq    {kernel_rsp}(%rsp), %rsp
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %rbp
    popq    %rbx
    retq
    int3

    // `syscall` from ring 3 (IA32_LSTAR): interrupts, the trap and direction flags are masked
    // (IA32_FMASK), the stack is still the program's. Builds the frame an interrupt from ring 3
    // would have in the `UserContext`, with the `syscall` vector, and saves the rest as above.
    .balign 16
    .global x86_64_syscall_entry
x86_64_syscall_entry:
    swapgs
    movq    %rsp, %gs:{user_rsp}
    movq    %gs:{tss_rsp0}, %rsp
    pushq   ${user_ss}
    pushq   %gs:{user_rsp}
    pushq   %r11                    // rflags
    pushq   ${user_cs}
    pushq   %rcx                    // rip
    pushq   $0                      // error code
    pushq   ${syscall_vector}
    jmp     user_exception_save

    // `x86_64_enter_user(context)`: keeps the kernel's callee-saved registers on its stack and
    // the stack pointer in the context, points `TSS.RSP0` just past the context's saved part
    // and enters ring 3 with the program's registers: with `sysretq` after a system call
    // (`rcx` and `r11` take its `rip` and `rflags`), with `iretq` otherwise. Returns through
    // `user_exception_save`.
    .global x86_64_enter_user
x86_64_enter_user:
    pushq   %rbx
    pushq   %rbp
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    movq    %rsp, {kernel_rsp}(%rdi)
    leaq    {kernel_rsp}(%rdi), %rax
    movq    %rax, %gs:{tss_rsp0}
    // The program's FS base, unless this CPU's is already the one its `fs_base_id` names.
    movq    {fs_base_id}(%rdi), %rax
    cmpq    %rax, %gs:{cpu_fs_base_id}
    jne     2f
3:  cmpq    ${syscall_vector}, {vector}(%rdi)
    je      1f
    movq    %rdi, %rsp
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rbp
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rbx
    popq    %rax
    addq    $16, %rsp               // vector and error code
    swapgs
    iretq
    int3
    // After a system call: `sysretq` takes `rcx` and `r11`, so theirs are not loaded.
1:  movq    {r15}(%rdi), %r15
    movq    {r14}(%rdi), %r14
    movq    {r13}(%rdi), %r13
    movq    {r12}(%rdi), %r12
    movq    {r10}(%rdi), %r10
    movq    {r9}(%rdi), %r9
    movq    {r8}(%rdi), %r8
    movq    {rbp}(%rdi), %rbp
    movq    {rsi}(%rdi), %rsi
    movq    {rdx}(%rdi), %rdx
    movq    {rbx}(%rdi), %rbx
    movq    {rax}(%rdi), %rax
    movq    {rip}(%rdi), %rcx
    movq    {rflags}(%rdi), %r11
    movq    {rsp}(%rdi), %rsp       // the program's stack
    movq    {rdi}(%rdi), %rdi
    swapgs
    sysretq
    int3
    // Loads the FS base (`rax` holds the context's `fs_base_id`): `rax`, `rcx` and `rdx` are
    // the program's to load after this.
2:  movq    %rax, %gs:{cpu_fs_base_id}
    movq    {fs_base}(%rdi), %rax
    movq    %rax, %rdx
    shrq    $32, %rdx
    movl    ${ia32_fs_base}, %ecx
    wrmsr
    jmp     3b
    int3
    .popsection

    // The user-mode test program (D47, `TestProgram`): position-independent, no stack, no
    // data. Copied to a user page, never run where it is linked.
    .pushsection .rodata.user_test, "a"
    .global x86_64_user_test_start
x86_64_user_test_start:
    .global x86_64_user_test_checks
x86_64_user_test_checks:
    movl    $1, %edi
    movl    $2, %esi
    movl    $3, %edx
    movl    $4, %r10d
    movl    $5, %r8d
    movl    $6, %r9d
    movl    $0x103, %ebx
    movl    $0x105, %ebp
    movl    $0x10c, %r12d
    movl    $0x10d, %r13d
    movl    $0x10e, %r14d
    movl    $0x10f, %r15d
    movl    ${syscall_first}, %eax
    syscall
    // Every register but rax (and rcx and r11, which `syscall` takes) must be as it was: rsi
    // collects the differences.
    subq    $2, %rsi
    subq    $1, %rdi
    orq     %rdi, %rsi
    subq    $3, %rdx
    orq     %rdx, %rsi
    subq    $4, %r10
    orq     %r10, %rsi
    subq    $5, %r8
    orq     %r8, %rsi
    subq    $6, %r9
    orq     %r9, %rsi
    subq    $0x103, %rbx
    orq     %rbx, %rsi
    subq    $0x105, %rbp
    orq     %rbp, %rsi
    subq    $0x10c, %r12
    orq     %r12, %rsi
    subq    $0x10d, %r13
    orq     %r13, %rsi
    subq    $0x10e, %r14
    orq     %r14, %rsi
    subq    $0x10f, %r15
    orq     %r15, %rsi
    movq    %rax, %rdi
    movl    ${syscall_report}, %eax
    syscall
    movq    (%rax), %rcx
1:  jmp     1b
    .global x86_64_user_test_floating_point
x86_64_user_test_floating_point:
    .byte   0xd9, 0xe8              // fld1 (x87), spelled out: the target has no x87 feature
    .global x86_64_user_test_null_syscalls
x86_64_user_test_null_syscalls:
2:  movl    ${syscall_null}, %eax
    syscall
    jmp     2b
    .global x86_64_user_test_spin
x86_64_user_test_spin:
3:  jmp     3b
    .global x86_64_user_test_linux_hello
x86_64_user_test_linux_hello:
    // Linux's x86_64 numbers: write 1, exit_group 231. rbx collects the differences.
    movl    $1, %edi
    leaq    9f(%rip), %rsi
    movl    ${hello_length}, %edx
    movl    $1, %eax
    syscall
    movq    %rax, %rbx
    subq    ${hello_length}, %rbx
    movl    $1, %edi
    xorl    %esi, %esi
    movl    $1, %edx
    movl    $1, %eax
    syscall
    addq    $14, %rax               // -EFAULT
    orq     %rax, %rbx
    movl    ${unknown_syscall}, %eax
    syscall
    addq    $38, %rax               // -ENOSYS
    orq     %rax, %rbx
    xorl    %edi, %edi
    testq   %rbx, %rbx
    setnz   %dil
    movl    $231, %eax
    syscall
4:  jmp     4b
9:  .ascii  "EGGOS-USER: hello from user mode\n"
    .global x86_64_user_test_linux_start
x86_64_user_test_linux_start:
    // Linux's x86_64 numbers: rt_sigaction 13, writev 20, getpid 39, sigaltstack 131, gettid
    // 186, exit_group 231, prlimit64 302, getrandom 318. rbx is argc, r12 walks argv and envp,
    // r13 counts the two lists, r14 holds the process id. Each string is written by writev
    // from one iovec on the stack. Ends with exit_group(255) if the stack is misaligned, the
    // thread's id is not the process's, getrandom does not fill the 16 bytes below the stack
    // pointer with something not zero, prlimit64 does not report the stack's limits as 8 MiB
    // soft and unlimited hard, SIGPIPE's action does not read back as SIG_IGN once set, or
    // sigaltstack does not report no alternate stack.
    movl    $255, %edi
    testq   $15, %rsp
    jnz     9f
    movl    $39, %eax
    syscall
    movq    %rax, %r14
    movl    $186, %eax
    syscall
    movl    $255, %edi
    cmpq    %rax, %r14
    jne     9f
    leaq    -16(%rsp), %rdi
    movl    $16, %esi
    xorl    %edx, %edx
    movl    $318, %eax
    syscall
    movl    $255, %edi
    cmpq    $16, %rax
    jne     9f
    movq    -16(%rsp), %rax
    orq     -8(%rsp), %rax
    jz      9f
    xorl    %edi, %edi
    movl    $3, %esi                // RLIMIT_STACK
    xorl    %edx, %edx
    leaq    -32(%rsp), %r10
    movl    $302, %eax
    syscall
    movl    $255, %edi
    testq   %rax, %rax
    jnz     9f
    cmpq    $0x800000, -32(%rsp)
    jne     9f
    cmpq    $-1, -24(%rsp)
    jne     9f
    movq    $1, -64(%rsp)           // SIG_IGN
    movq    $0, -56(%rsp)
    movq    $0, -48(%rsp)
    movq    $0, -40(%rsp)
    movl    $13, %edi               // SIGPIPE
    leaq    -64(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $13, %eax
    syscall
    movl    $255, %edi
    testq   %rax, %rax
    jnz     9f
    movl    $13, %edi
    xorl    %esi, %esi
    leaq    -96(%rsp), %rdx
    movl    $8, %r10d
    movl    $13, %eax
    syscall
    movl    $255, %edi
    testq   %rax, %rax
    jnz     9f
    cmpq    $1, -96(%rsp)
    jne     9f
    xorl    %edi, %edi
    leaq    -128(%rsp), %rsi
    movl    $131, %eax
    syscall
    movl    $255, %edi
    testq   %rax, %rax
    jnz     9f
    cmpl    $2, -120(%rsp)          // SS_DISABLE
    jne     9f
    movq    (%rsp), %rbx
    leaq    8(%rsp), %r12
    movl    $2, %r13d
6:  movq    (%r12), %rsi
    addq    $8, %r12
    testq   %rsi, %rsi
    jz      8f
    xorl    %edx, %edx
7:  cmpb    $0, (%rsi,%rdx)
    je      5f
    incq    %rdx
    jmp     7b
5:  pushq   %rdx
    pushq   %rsi
    movl    $1, %edi
    movq    %rsp, %rsi
    movl    $1, %edx
    movl    $20, %eax
    syscall
    addq    $16, %rsp
    jmp     6b
8:  decl    %r13d
    jnz     6b
    movq    %rbx, %rdi
9:  movl    $231, %eax
    syscall
4:  jmp     4b
    .global x86_64_user_test_linux_start_end
x86_64_user_test_linux_start_end:
    .global x86_64_user_test_linux_unknown_syscalls
x86_64_user_test_linux_unknown_syscalls:
5:  movl    ${unknown_syscall}, %eax
    syscall
    jmp     5b
    .global x86_64_user_test_stack_growth
x86_64_user_test_stack_growth:
    // From the stack pointer's page down, a page at a time: each page's address in its
    // first word, until the stack runs out.
    movq    %rsp, %rax
    andq    $-4096, %rax
6:  movq    %rax, (%rax)
    subq    $4096, %rax
    jmp     6b
    .global x86_64_user_test_linux_thread_pointer
x86_64_user_test_linux_thread_pointer:
    // Linux's x86_64 numbers: getpid 39, sched_yield 24, arch_prctl 158, exit_group 231.
    // A program cannot read its FS base, so it reads the marker below through FS, at the
    // marker's address less the base it expects: that finds the marker only if the FS base
    // is that one (another base reads elsewhere or faults). First with the base a new program
    // starts with, zero; then with getpid() << 12, set with arch_prctl(ARCH_SET_FS), after
    // every sched_yield. rbx collects the differences.
    leaq    8f(%rip), %r13
    movq    %fs:(%r13), %rbx
    xorq    8f(%rip), %rbx
    movl    $39, %eax
    syscall
    shlq    $12, %rax
    movq    %rax, %r12
    subq    %rax, %r13
    movq    %rax, %rsi
    movl    ${arch_set_fs}, %edi
    movl    $158, %eax
    syscall
    orq     %rax, %rbx
    movl    ${thread_pointer_rounds}, %r14d
7:  movl    $24, %eax
    syscall
    orq     %rax, %rbx
    movq    %fs:(%r13), %rax
    xorq    8f(%rip), %rax
    orq     %rax, %rbx
    decl    %r14d
    jnz     7b
    xorl    %edi, %edi
    testq   %rbx, %rbx
    setnz   %dil
    movl    $231, %eax
    syscall
9:  jmp     9b
    .balign 8
8:  .quad   {thread_pointer_marker}
    .global x86_64_user_test_linux_poll
x86_64_user_test_linux_poll:
    // Linux's x86_64 numbers: poll 7, ioctl 16, exit_group 231, ppoll 271. ebx is the exit
    // status until every check passed. The three `struct pollfd`s are at rsp-32, the window
    // size at rsp-64, the timeout at rsp-48.
    movl    $1, %ebx
    movq    $0, -32(%rsp)           // fd 0, no events
    movq    $1, -24(%rsp)           // fd 1, POLLOUT
    movw    $4, -20(%rsp)
    movq    $2, -16(%rsp)           // fd 2, no events
    leaq    -32(%rsp), %rdi
    movl    $3, %esi
    xorl    %edx, %edx
    movl    $7, %eax
    syscall
    cmpq    $1, %rax
    jne     9f
    cmpw    $0, -26(%rsp)
    jne     9f
    cmpw    $4, -18(%rsp)
    jne     9f
    cmpw    $0, -10(%rsp)
    jne     9f
    movl    $1, %edi
    movl    $0x5413, %esi           // TIOCGWINSZ
    leaq    -64(%rsp), %rdx
    movl    $16, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $0, -48(%rsp)
    movq    ${poll_wait}, -40(%rsp)
    xorl    %edi, %edi
    xorl    %esi, %esi
    leaq    -48(%rsp), %rdx
    xorl    %r10d, %r10d
    xorl    %r8d, %r8d
    movl    $271, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    -48(%rsp), %rax
    orq     -40(%rsp), %rax
    jnz     9f
    xorl    %ebx, %ebx
9:  movl    %ebx, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_linux_signals
x86_64_user_test_linux_signals:
    // Linux's x86_64 numbers: rt_sigprocmask 14, getpid 39, kill 62, rt_sigpending 127,
    // exit_group 231, tgkill 234. The signal set is at rsp-16, the pending set at rsp-32.
    xorl    %edi, %edi
    movl    $17, %esi               // SIGCHLD, ignored
    movl    $62, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $(1 << 9), -16(%rsp)    // SIGUSR1
    xorl    %edi, %edi              // SIG_BLOCK
    leaq    -16(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $39, %eax
    syscall
    movq    %rax, %rdi
    movq    %rax, %rsi
    movl    $10, %edx               // SIGUSR1
    movl    $234, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    leaq    -32(%rsp), %rdi
    movl    $8, %esi
    movl    $127, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    cmpq    $(1 << 9), -32(%rsp)
    jne     9f
    movl    $1, %edi                // SIG_UNBLOCK: SIGUSR1 ends the program here
    leaq    -16(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_linux_signal_handler
x86_64_user_test_linux_signal_handler:
    // Linux's x86_64 numbers: rt_sigaction 13, rt_sigprocmask 14, rt_sigreturn 15, getpid 39,
    // sigaltstack 131, exit_group 231, tgkill 234. Its `struct sigaction` is at rsp-64, its
    // `stack_t` at rsp-32, its alternate stack 0x2000 bytes from rsp-0x2800, the mask read
    // back at rsp-72. The handler returns through its restorer, as every x86_64 one must.
    leaq    -0x2800(%rsp), %rax
    movq    %rax, -32(%rsp)
    movq    $0, -24(%rsp)
    movq    $0x2000, -16(%rsp)
    leaq    -32(%rsp), %rdi
    xorl    %esi, %esi
    movl    $131, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    leaq    5f(%rip), %rax
    movq    %rax, -64(%rsp)
    movq    $0x0c000004, -56(%rsp)  // SA_ONSTACK | SA_RESTORER | SA_SIGINFO
    leaq    6f(%rip), %rax
    movq    %rax, -48(%rsp)
    movq    $0, -40(%rsp)           // no mask
    movl    $10, %edi               // SIGUSR1
    leaq    -64(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $13, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $39, %eax
    syscall
    movq    %rax, %rdi
    movq    %rax, %rsi
    movl    $10, %edx
    movl    $0x5a5, %ebx            // kept across the handler, which clobbers it
    movl    $1, %r12d               // the handler sets it to 2 in its frame
    movl    $234, %eax
    syscall                         // the handler runs before this returns
    testq   %rax, %rax
    jnz     9f
    cmpq    $0x5a5, %rbx
    jne     9f
    cmpq    $2, %r12
    jne     9f
    xorl    %edi, %edi              // the mask is back: nothing blocked
    xorl    %esi, %esi
    leaq    -72(%rsp), %rdx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    cmpq    $0, -72(%rsp)
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    // The handler: SIGUSR1 from tgkill (SI_TKILL), on the alternate stack the ucontext names;
    // then it sets the frame's r12 to 2 and clobbers rbx.
5:  cmpl    $10, %edi
    jne     7f
    cmpl    $10, (%rsi)
    jne     7f
    cmpl    $-6, 8(%rsi)
    jne     7f
    movq    16(%rdx), %rax          // uc_stack.ss_sp
    cmpq    %rax, %rsp
    jb      7f
    addq    32(%rdx), %rax          // uc_stack.ss_size
    cmpq    %rax, %rsp
    jae     7f
    movq    $2, {saved_r12}(%rdx)
7:  xorl    %ebx, %ebx
    ret
    // Its restorer.
6:  movl    $15, %eax
    syscall
    int3
    .global x86_64_user_test_linux_fault_handler
x86_64_user_test_linux_fault_handler:
    // Linux's x86_64 numbers: mmap 9, rt_sigaction 13, rt_sigreturn 15, exit_group 231. Its
    // `struct sigaction` is at rsp-64; the handler returns through its restorer.
    leaq    5f(%rip), %rax
    movq    %rax, -64(%rsp)
    movq    $0x04000004, -56(%rsp)  // SA_RESTORER | SA_SIGINFO
    leaq    6f(%rip), %rax
    movq    %rax, -48(%rsp)
    movq    $0, -40(%rsp)           // no mask
    movl    $11, %edi               // SIGSEGV
    leaq    -64(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $13, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    xorl    %edi, %edi              // mmap(0, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS)
    movl    $4096, %esi
    xorl    %edx, %edx
    movl    $0x22, %r10d
    movq    $-1, %r8
    xorl    %r9d, %r9d
    movl    $9, %eax
    syscall
    cmpq    $-4095, %rax
    jae     9f
    movq    %rax, %rbx
    xorl    %r12d, %r12d            // the faults the handler counted
    movl    ${fault_address}, %r13d // nothing mapped there: SEGV_MAPERR
    movl    $1, %r14d
    leaq    1f(%rip), %r15
    movq    (%r13), %rax
    jmp     9f
1:  movq    %rbx, %r13              // PROT_NONE: SEGV_ACCERR
    movl    $2, %r14d
    leaq    2f(%rip), %r15
    movq    (%r13), %rax
    jmp     9f
2:  cmpq    $2, %r12
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    // The handler: SIGSEGV with the si_code and si_addr in the frame's r14 and r13, which cr2
    // reports too, with trapno 14 (#PF); it counts it in the frame's r12, and resumes the
    // program at its r15, whatever the checks found.
5:  cmpl    $11, %edi
    jne     7f
    cmpl    $11, (%rsi)
    jne     7f
    movl    8(%rsi), %eax
    cmpq    {saved_r14}(%rdx), %rax
    jne     7f
    movq    16(%rsi), %rax
    cmpq    {saved_r13}(%rdx), %rax
    jne     7f
    cmpq    {saved_cr2}(%rdx), %rax
    jne     7f
    cmpq    $14, {saved_trapno}(%rdx)
    jne     7f
    incq    {saved_r12}(%rdx)
7:  movq    {saved_r15}(%rdx), %rax
    movq    %rax, {saved_rip}(%rdx)
    ret
    // Its restorer.
6:  movl    $15, %eax
    syscall
    int3
    .global x86_64_user_test_linux_signal_queue
x86_64_user_test_linux_signal_queue:
    leaq    -288(%rsp), %r12
    movabsq $(1 << ({queued_signal} - 1)), %rax
    movq    %rax, 272(%r12)
    movq    $0, 256(%r12)
    movq    $0, 264(%r12)
    movq    $0, 144(%r12)
    xorl    %edi, %edi
    leaq    272(%r12), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $39, %eax
    syscall
    movq    %rax, %rbx
    movl    ${queued_signal}, 128(%r12)
    movl    $-1, 136(%r12)
    movl    %ebx, 144(%r12)
    movq    ${first_value}, 152(%r12)
    movq    %rbx, %rdi
    movl    ${queued_signal}, %esi
    leaq    128(%r12), %rdx
    movl    $129, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    ${second_value}, 152(%r12)
    movq    %rbx, %rdi
    movl    ${queued_signal}, %esi
    leaq    128(%r12), %rdx
    movl    $129, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    %rbx, %rdi
    movl    ${queued_signal}, %esi
    movl    $62, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $-1, %r13
    movq    ${first_value}, %r14
    call    6f
    movq    ${second_value}, %r14
    call    6f
    xorl    %r13d, %r13d
    xorl    %r14d, %r14d
    call    6f
    leaq    272(%r12), %rdi
    movq    %r12, %rsi
    leaq    256(%r12), %rdx
    movl    $8, %r10d
    movl    $128, %eax
    syscall
    cmpq    $-11, %rax
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
6:  leaq    272(%r12), %rdi
    movq    %r12, %rsi
    leaq    256(%r12), %rdx
    movl    $8, %r10d
    movl    $128, %eax
    syscall
    cmpq    ${queued_signal}, %rax
    jne     9b
    cmpl    ${queued_signal}, (%r12)
    jne     9b
    movslq  8(%r12), %rax
    cmpq    %r13, %rax
    jne     9b
    cmpq    %rbx, 16(%r12)
    jne     9b
    cmpq    %r14, 24(%r12)
    jne     9b
    ret
    .global x86_64_user_test_linux_signal_suspend
x86_64_user_test_linux_signal_suspend:
    subq    $64, %rsp
    leaq    5f(%rip), %rax
    movq    %rax, (%rsp)
    movq    $0x04000004, 8(%rsp)
    leaq    6f(%rip), %rax
    movq    %rax, 16(%rsp)
    movq    $0, 24(%rsp)
    movl    $10, %edi
    movq    %rsp, %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $13, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $0x200, 32(%rsp)
    movq    $0, 40(%rsp)
    xorl    %edi, %edi
    leaq    32(%rsp), %rsi
    xorl    %edx, %edx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $39, %eax
    syscall
    movq    %rax, %rdi
    movq    %rax, %rsi
    movl    $10, %edx
    movl    $234, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $1, %r12d
    leaq    40(%rsp), %rdi
    movl    $8, %esi
    movl    $130, %eax
    syscall
    cmpq    $-4, %rax
    jne     9f
    cmpq    $2, %r12
    jne     9f
    xorl    %edi, %edi
    xorl    %esi, %esi
    leaq    48(%rsp), %rdx
    movl    $8, %r10d
    movl    $14, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    cmpq    $0x200, 48(%rsp)
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
5:  cmpl    $10, %edi
    jne     7f
    cmpq    $0x200, {uc_sigmask}(%rdx)
    jne     7f
    movq    $2, {saved_r12}(%rdx)
7:  ret
6:  movl    $15, %eax
    syscall
    int3
    .global x86_64_user_test_linux_clock
x86_64_user_test_linux_clock:
    subq    $64, %rsp
    movl    $1, %edi
    movq    %rsp, %rsi
    movl    $228, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $0, 32(%rsp)
    movq    ${clock_wait}, 40(%rsp)
    leaq    32(%rsp), %rdi
    xorl    %esi, %esi
    movl    $35, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $1, %edi
    leaq    16(%rsp), %rsi
    movl    $228, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    16(%rsp), %rax
    subq    (%rsp), %rax
    imulq   $1000000000, %rax, %rax
    addq    24(%rsp), %rax
    subq    8(%rsp), %rax
    cmpq    40(%rsp), %rax
    jb      9f
    movl    $2, %edi
    leaq    16(%rsp), %rsi
    movl    $228, %eax
    syscall
    cmpq    $-22, %rax
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_linux_stat
x86_64_user_test_linux_stat:
    subq    $272, %rsp
    movb    $0, 256(%rsp)
    movl    $1, %edi
    movq    %rsp, %rsi
    movl    $5, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    24(%rsp), %eax
    andl    $0xf000, %eax
    cmpl    $0x2000, %eax
    jne     9f
    cmpq    $0x501, 40(%rsp)
    jne     9f
    movq    $0, 40(%rsp)
    movl    $1, %edi
    leaq    256(%rsp), %rsi
    movq    %rsp, %rdx
    movl    $0x1000, %r10d
    movl    $262, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    cmpq    $0x501, 40(%rsp)
    jne     9f
    movw    $0, 28(%rsp)
    movl    $0, 128(%rsp)
    movl    $1, %edi
    leaq    256(%rsp), %rsi
    movl    $0x1000, %edx
    movl    $0x7ff, %r10d
    movq    %rsp, %r8
    movl    $332, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movzwl  28(%rsp), %eax
    andl    $0xf000, %eax
    cmpl    $0x2000, %eax
    jne     9f
    cmpl    $5, 128(%rsp)
    jne     9f
    movl    $1, %edi
    xorl    %esi, %esi
    movl    $1, %edx
    movl    $8, %eax
    syscall
    cmpq    $-29, %rax
    jne     9f
    movl    $3, %edi
    movq    %rsp, %rsi
    movl    $5, %eax
    syscall
    cmpq    $-9, %rax
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_linux_credentials
x86_64_user_test_linux_credentials:
    movl    $107, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $1000, %edi
    movl    $105, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movl    $102, %eax
    syscall
    cmpq    $1000, %rax
    jne     9f
    xorl    %edi, %edi
    movl    $105, %eax
    syscall
    cmpq    $-1, %rax
    jne     9f
    xorl    %edi, %edi
    movl    $231, %eax
    syscall
9:  movl    $1, %edi
    movl    $231, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_end
x86_64_user_test_end:
    .popsection
"#,
    handler = sym x86_64_exception,
    kernel_rsp = const core::mem::offset_of!(Registers, kernel_rsp),
    vector = const core::mem::offset_of!(Registers, vector),
    r15 = const core::mem::offset_of!(Registers, r15),
    r14 = const core::mem::offset_of!(Registers, r14),
    r13 = const core::mem::offset_of!(Registers, r13),
    r12 = const core::mem::offset_of!(Registers, r12),
    r10 = const core::mem::offset_of!(Registers, r10),
    r9 = const core::mem::offset_of!(Registers, r9),
    r8 = const core::mem::offset_of!(Registers, r8),
    rbp = const core::mem::offset_of!(Registers, rbp),
    rdi = const core::mem::offset_of!(Registers, rdi),
    rsi = const core::mem::offset_of!(Registers, rsi),
    rdx = const core::mem::offset_of!(Registers, rdx),
    rbx = const core::mem::offset_of!(Registers, rbx),
    rax = const core::mem::offset_of!(Registers, rax),
    rip = const core::mem::offset_of!(Registers, rip),
    rflags = const core::mem::offset_of!(Registers, rflags),
    rsp = const core::mem::offset_of!(Registers, rsp),
    fs_base = const core::mem::offset_of!(Registers, fs_base),
    fs_base_id = const core::mem::offset_of!(Registers, fs_base_id),
    cpu_fs_base_id = const FS_BASE_ID_OFFSET,
    ia32_fs_base = const IA32_FS_BASE,
    tss_rsp0 = const TSS_RSP0_OFFSET,
    user_rsp = const USER_RSP_OFFSET,
    user_cs = const USER_CODE_SELECTOR,
    user_ss = const USER_DATA_SELECTOR,
    syscall_vector = const SYSCALL_VECTOR,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    // `uc_mcontext.r12` in the ucontext: the sigcontext at 40, r8 to r15 from its start.
    saved_r12 = const 40 + 4 * 8,
    saved_r13 = const 40 + 5 * 8,
    saved_r14 = const 40 + 6 * 8,
    saved_r15 = const 40 + 7 * 8,
    // `uc_mcontext`'s `rip`, `trapno` and `cr2`.
    saved_rip = const 40 + 16 * 8,
    saved_trapno = const 40 + 160,
    saved_cr2 = const 40 + 176,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    uc_sigmask = const 296,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
    thread_pointer_marker = const TestProgram::LINUX_THREAD_POINTER_MARKER,
    arch_set_fs = const TestProgram::LINUX_ARCH_SET_FS,
    options(att_syntax)
);

/// Every vector the IDT can hold.
const VECTORS: usize = 256;
/// The vectors with a stub at `exception_stubs + vector * STUB_SIZE`: the exceptions 0-31 and
/// the interrupts up to the wake-up interrupt's.
const CONTIGUOUS_STUBS: usize = WAKEUP_VECTOR as usize + 1;
const _: () = assert!(TIMER_VECTOR < WAKEUP_VECTOR);
/// Vectors below this are the architecture's exceptions.
const FIRST_INTERRUPT_VECTOR: u64 = 32;
/// Every stub starts on a 16-byte boundary.
const STUB_SIZE: usize = 16;

/// Code segment selector of the boot GDT (`boot_gdt` in the entry code).
const KERNEL_CODE_SELECTOR: u16 = 0x08;
/// Present, DPL 0, 64-bit interrupt gate (interrupts stay masked in the handler).
const INTERRUPT_GATE: u8 = 0x8e;

/// `#PF`'s vector.
const PAGE_FAULT_VECTOR: u64 = 14;
/// Page-fault error code bit 0: the entry was present (a protection violation).
const PAGE_FAULT_PRESENT: u64 = 1 << 0;
/// Page-fault error code bit 1: the access wrote memory.
const PAGE_FAULT_WRITE: u64 = 1 << 1;
/// Page-fault error code bit 2: the access was made from user mode.
const PAGE_FAULT_USER: u64 = 1 << 2;
/// Page-fault error code bit 3: a reserved bit was set in an entry.
const PAGE_FAULT_RESERVED_BIT: u64 = 1 << 3;
/// Page-fault error code bit 4: the fault was an instruction fetch.
const PAGE_FAULT_INSTRUCTION_FETCH: u64 = 1 << 4;

unsafe extern "C" {
    /// Start of the stubs; not callable from Rust, only its address is used.
    fn exception_stubs();
    /// The stub for [`APIC_SPURIOUS_VECTOR`]; likewise only its address is used.
    fn apic_spurious_stub();
}

/// What the stub and `exception_common` save for a trap in the kernel, lowest address first,
/// followed by what the CPU pushed: the registers a call may change, the others the handler
/// keeps itself.
#[repr(C)]
struct TrapFrame {
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    vector: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 16 * 8);

/// A 64-bit IDT gate descriptor (SDM vol. 3A, figure 6-8).
#[derive(Clone, Copy)]
#[repr(C)]
struct GateDescriptor {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

const _: () = assert!(core::mem::size_of::<GateDescriptor>() == 16);

impl GateDescriptor {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn interrupt_gate(handler: u64) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            ist: 0,
            attributes: INTERRUPT_GATE,
            offset_middle: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

/// The operand of `lidt`.
#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

struct InterruptDescriptorTable(UnsafeCell<[GateDescriptor; VECTORS]>);

// SAFETY: the table is written only by `install`, which runs once on the boot CPU before any
// other code (and before any other CPU starts); afterwards only the CPU reads it.
unsafe impl Sync for InterruptDescriptorTable {}

static IDT: InterruptDescriptorTable =
    InterruptDescriptorTable(UnsafeCell::new([GateDescriptor::MISSING; VECTORS]));

/// Fills the IDT and loads it. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn install() {
    fill();
    load();
}

/// Fills the IDT, before any CPU loads it.
fn fill() {
    let stubs = exception_stubs as *const () as u64;
    let gates = IDT.0.get();
    let stub_gates = (0..CONTIGUOUS_STUBS)
        .map(|vector| (vector, stubs + (vector * STUB_SIZE) as u64))
        .chain([(
            APIC_SPURIOUS_VECTOR.into(),
            apic_spurious_stub as *const () as u64,
        )]);
    // Vectors without a stub keep a not-present gate: delivering one raises #NP (reported).
    for (vector, stub) in stub_gates {
        // SAFETY: `fill` is the only writer and runs once, from `install`, before any CPU loads
        // the IDT, so nothing else accesses the table; `vector` is below 256, in bounds.
        unsafe { (*gates)[vector] = GateDescriptor::interrupt_gate(stub) };
    }
}

/// Loads the IDT on this CPU: the boot CPU's entry code once `fill` has run, and each secondary
/// CPU's (D40).
pub(super) extern "C" fn load() {
    let gates = IDT.0.get();
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[GateDescriptor; VECTORS]>() - 1) as u16,
        base: gates as u64,
    };
    // SAFETY: the IDT is a static, filled by `fill` before any CPU loads it, whose gates point
    // at the stubs in this module with the boot GDT's code selector.
    unsafe {
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

/// Called by `exception_common` with the saved registers, for a trap in the kernel.
extern "C" fn x86_64_exception(frame: &mut TrapFrame) {
    if interrupt(frame.vector) {
        return;
    }
    // `int3` is a trap: the saved `rip` already points after it, so resuming needs no fix-up.
    deliver(&exception(frame.vector, frame.error_code, frame.rip));
}

/// Handles the interrupt `vector`, if the kernel takes it; returns whether it did (if not, it
/// is an exception or an unexpected interrupt, to be reported).
#[inline(always)]
fn interrupt(vector: u64) -> bool {
    match vector {
        vector if vector == u64::from(TIMER_VECTOR) => {
            timer::interrupt();
            true
        }
        // The exceptions, without the rarer interrupts' checks.
        vector if vector < FIRST_INTERRUPT_VECTOR => false,
        vector if vector == u64::from(WAKEUP_VECTOR) => {
            timer::wakeup_interrupt();
            true
        }
        // Spurious: the APIC needs no end of interrupt for it.
        vector if vector == u64::from(APIC_SPURIOUS_VECTOR) => true,
        vector => LEGACY_PIC_VECTORS.contains(&vector) && timer::legacy_pic_interrupt(vector),
    }
}

/// [`crate::Platform::page_fault`]: a `#PF` (the only exception decoded as a data or
/// instruction fault with an error code), unless a reserved bit was set in an entry, which is
/// the kernel's bug and would fault again. Error-code bit 0 clear means no present entry: the
/// only kind that can be stale (x86 caches no not-present entry, but another CPU may have
/// just made one present).
pub(super) fn page_fault(kind: ExceptionKind, error_code: u64) -> Option<crate::paging::PageFault> {
    use crate::paging::{Access, PageFault};
    if error_code & PAGE_FAULT_RESERVED_BIT != 0 {
        return None;
    }
    let access = match kind {
        ExceptionKind::InstructionFault => Access::Execute,
        ExceptionKind::DataFault if error_code & PAGE_FAULT_WRITE != 0 => Access::Write,
        ExceptionKind::DataFault => Access::Read,
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale: error_code & PAGE_FAULT_PRESENT == 0,
    })
}

/// Describes the exception or unexpected interrupt `vector`, with the faulting address for a
/// page fault.
fn exception(vector: u64, error_code: u64, rip: u64) -> Exception {
    let fault_address = if vector == PAGE_FAULT_VECTOR {
        let cr2: u64;
        // SAFETY: reading CR2 at ring 0 has no side effects.
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags))
        };
        Some(cr2)
    } else {
        None
    };
    decode(vector, error_code, rip, fault_address)
}

/// Selectors of the per-CPU GDT (D47): the boot GDT's two, then user data and code in the
/// order `sysretq` needs (`IA32_STAR`), then the TSS.
const KERNEL_DATA_SELECTOR: u64 = 0x10;
const USER_DATA_SELECTOR: u64 = 0x18 | 3;
const USER_CODE_SELECTOR: u64 = 0x20 | 3;
const TSS_SELECTOR: u16 = 0x28;

/// `rflags` bits a program may set: carry, parity, adjust, zero, sign, direction, overflow and
/// ID (which old-style CPUID detection toggles). Interrupts are always enabled in ring 3 and
/// I/O privilege stays 0. Alignment check stays clear: interrupt gates keep it, and with SMAP
/// it would open user memory to the kernel.
const USER_RFLAGS: u64 = 0xcd5 | 1 << 21;
/// CR0 bits for the FP/SIMD units (D49): monitor coprocessor, emulation, task switched,
/// numeric error.
const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
/// `CR4.FSGSBASE`: lets ring 3 write its own FS and GS bases (`wrfsbase`, `wrgsbase`).
const CR4_FSGSBASE: u64 = 1 << 16;
const RFLAGS_INTERRUPTS: u64 = 1 << 9;
/// `rflags` bit 1, always set.
const RFLAGS_FIXED: u64 = 1 << 1;
/// What `syscall` clears in `rflags` (IA32_FMASK): interrupts, trap, direction, alignment
/// check, nested task. The entry runs on the program's stack until it switches; nothing may
/// interrupt it, and the kernel's code needs the direction flag clear.
const SYSCALL_MASKED_RFLAGS: u64 = 1 << 9 | 1 << 8 | 1 << 10 | 1 << 18 | 1 << 14;

const IA32_EFER: u32 = 0xc000_0080;
const EFER_SYSCALL: u64 = 1 << 0;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_FMASK: u32 = 0xc000_0084;
/// The FS segment's base in 64-bit mode: the running program's thread pointer.
const IA32_FS_BASE: u32 = 0xc000_0100;
/// The GS base `swapgs` swaps in on the way to ring 3: the program's GS base.
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;

unsafe extern "C" {
    /// Enters ring 3 with `context`'s registers; returns once an interrupt, exception or
    /// `syscall` from ring 3 has saved them back.
    fn x86_64_enter_user(context: *mut Registers);
    /// `syscall`'s entry point; only its address is used.
    fn x86_64_syscall_entry();
    static x86_64_user_test_start: u8;
    static x86_64_user_test_checks: u8;
    static x86_64_user_test_floating_point: u8;
    static x86_64_user_test_null_syscalls: u8;
    static x86_64_user_test_spin: u8;
    static x86_64_user_test_linux_hello: u8;
    static x86_64_user_test_linux_start: u8;
    static x86_64_user_test_linux_start_end: u8;
    static x86_64_user_test_linux_unknown_syscalls: u8;
    static x86_64_user_test_stack_growth: u8;
    static x86_64_user_test_linux_thread_pointer: u8;
    static x86_64_user_test_linux_poll: u8;
    static x86_64_user_test_linux_signals: u8;
    static x86_64_user_test_linux_signal_handler: u8;
    static x86_64_user_test_linux_fault_handler: u8;
    static x86_64_user_test_linux_signal_queue: u8;
    static x86_64_user_test_linux_signal_suspend: u8;
    static x86_64_user_test_linux_clock: u8;
    static x86_64_user_test_linux_stat: u8;
    static x86_64_user_test_linux_credentials: u8;
    static x86_64_user_test_end: u8;
}

/// Sets this CPU up for user mode (D47), from `set_cpu_local`: its own GDT, with user segments
/// and its TSS, in its per-CPU block's HAL part `hal`, then `syscall`'s MSRs.
///
/// # Safety
/// `hal` must be this CPU's per-CPU block's HAL part, which lives as long as the kernel runs,
/// and nothing else may use its tables. Interrupts must be masked.
pub(super) unsafe fn set_up_user_mode(hal: &HalCpuLocal) {
    let tables = hal.x86.0.get();
    // SAFETY: the caller guarantees the block is this CPU's and that nothing else uses its
    // tables; they are written here before the CPU loads them.
    unsafe { (*tables).tss.io_map_base = core::mem::size_of::<Tss>() as u16 };
    // SAFETY: as above; only the address is taken.
    let tss_base = unsafe { &raw const (*tables).tss } as u64;
    let tss_limit = core::mem::size_of::<Tss>() as u64 - 1;
    let gdt: [u64; GDT_ENTRIES] = [
        0,
        // Accessed bits preset, so the CPU never writes them.
        0x00af_9b00_0000_ffff, // 0x08: 64-bit code, ring 0 (the boot GDT's)
        0x00cf_9300_0000_ffff, // 0x10: data, ring 0 (the boot GDT's)
        0x00cf_f300_0000_ffff, // 0x18: data, ring 3
        0x00af_fb00_0000_ffff, // 0x20: 64-bit code, ring 3
        // 0x28: an available 64-bit TSS (type 9), present, ring 0, then its base's top half.
        tss_limit | (tss_base & 0xff_ffff) << 16 | 0x89 << 40 | (tss_base >> 24 & 0xff) << 56,
        tss_base >> 32,
    ];
    // SAFETY: as above.
    unsafe { (*tables).gdt = gdt };
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[u64; GDT_ENTRIES]>() - 1) as u16,
        // SAFETY: as above; only the address is taken.
        base: unsafe { &raw const (*tables).gdt } as u64,
    };
    // SAFETY: the GDT lives as long as the kernel runs and keeps the boot GDT's kernel code and
    // data descriptors at the same selectors, so the loaded CS, DS, ES, SS (and FS and GS, whose
    // bases are MSRs) stay valid without a reload. `ltr` loads this CPU's own TSS, marking its
    // descriptor busy in this CPU's own GDT.
    unsafe {
        core::arch::asm!(
            "lgdt [{pointer}]",
            "ltr {tss:x}",
            pointer = in(reg) &pointer,
            tss = in(reg) TSS_SELECTOR,
            options(nostack, preserves_flags),
        );
    }
    // SAFETY: CR0 only changes what x87, MMX and SSE instructions do: with TS and MP set and EM
    // clear, each raises #NM (SSE raises #UD or #NM while CR4.OSFXSR is clear), in ring 3 and
    // in the kernel, which uses none. Programs get no FP/SIMD registers until they are saved
    // per thread, so none can read what another left in them (D49); NE reports x87 errors as
    // #MF once they can happen.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr0",
            "and {tmp}, {clear}",
            "or {tmp}, {set}",
            "mov cr0, {tmp}",
            tmp = out(reg) _,
            clear = in(reg) !CR0_EM,
            set = in(reg) CR0_MP | CR0_TS | CR0_NE,
            options(nomem, nostack),
        );
    }
    // SAFETY: the `syscall` MSRs are architectural. EFER.SCE enables `syscall`, whose MSRs are
    // all set below before anything runs in ring 3.
    unsafe { super::write_msr(IA32_EFER, super::read_msr(IA32_EFER) | EFER_SYSCALL) };
    // SAFETY: `IA32_STAR` names the kernel code selector for `syscall` (its SS is the next,
    // kernel data) and the base for `sysretq` (+8 user data, +16 user code, ring 3), matching
    // the GDT above.
    unsafe {
        super::write_msr(
            IA32_STAR,
            (USER_DATA_SELECTOR - 3 - 8) << 48 | u64::from(KERNEL_CODE_SELECTOR) << 32,
        )
    };
    // SAFETY: the entry switches to the running program's context before anything can
    // interrupt it (`IA32_FMASK` below masks interrupts).
    unsafe { super::write_msr(IA32_LSTAR, x86_64_syscall_entry as *const () as u64) };
    // SAFETY: as above.
    unsafe { super::write_msr(IA32_FMASK, SYSCALL_MASKED_RFLAGS) };
    // Programs may not write their FS or GS base themselves, whatever firmware left: the way
    // in loads the FS base only when it changed (`fs_base_id`), and a program-chosen GS base
    // needs the SWAPGS fence first (D49).
    // SAFETY: clearing `CR4.FSGSBASE` only takes four instructions away from ring 3; the
    // kernel uses none of them.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr4",
            "and {tmp}, {clear}",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            clear = in(reg) !CR4_FSGSBASE,
            options(nomem, nostack),
        );
    }
    // Programs' FS and GS bases start at zero, whatever firmware left: the FS base as the
    // per-CPU block's `fs_base_id` 0 says (`x86_64_enter_user` loads another only for a
    // context that set one), the GS base for good, since no program can set it yet
    // (`ARCH_SET_GS` waits for the SWAPGS fence, D49). Neither is used in the kernel.
    // SAFETY: both MSRs are architectural, and nothing runs in ring 3 on this CPU yet.
    unsafe { super::write_msr(IA32_FS_BASE, 0) };
    // SAFETY: as above; the kernel's own GS base is `IA32_GS_BASE`, which this leaves alone.
    unsafe { super::write_msr(IA32_KERNEL_GS_BASE, 0) };
}

const _: () = assert!(USER_DATA_SELECTOR - 3 - 8 == KERNEL_DATA_SELECTOR);

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // `sysretq` to a non-canonical `rip` faults in ring 0 on the program's stack, and `iretq`
    // to one faults in ring 0 with the program's GS base: never enter with one.
    if registers.rip >= X86_64::USER_END {
        // As the fetch's page fault would report it, from user mode, kept as for one.
        registers.vector = PAGE_FAULT_VECTOR;
        registers.error_code = PAGE_FAULT_USER | PAGE_FAULT_INSTRUCTION_FETCH;
        registers.fault_address = registers.rip;
        return UserExit::Exception(registers.last_exception());
    }
    registers.cs = USER_CODE_SELECTOR;
    registers.ss = USER_DATA_SELECTOR;
    registers.rflags = registers.rflags & USER_RFLAGS | RFLAGS_INTERRUPTS | RFLAGS_FIXED;
    // SAFETY: interrupts are masked (the caller's duty), so nothing interrupts the way in
    // between `swapgs` and `iretq`/`sysretq`; the trap or `syscall` that ends the program saves
    // its registers into the context, which `&mut` keeps valid and unaliased, switches back to
    // the kernel's GS base and returns here on the kernel stack with the callee-saved registers
    // restored. The selectors and flags are set above, so the program runs in ring 3 with I/O
    // privilege 0, where it reaches only user mappings, and its `rip` is canonical.
    unsafe { x86_64_enter_user(registers) };
    match registers.vector {
        SYSCALL_VECTOR => UserExit::Syscall,
        vector if interrupt(vector) => UserExit::Interrupt,
        vector => {
            // Unchanged for an exception taken again on the way in (none can be).
            debug_assert_ne!(vector, RESTORE_ALL_VECTOR);
            let exception = exception(vector, registers.error_code, registers.rip);
            // With `vector` and `error_code`, all `last_exception` needs.
            registers.fault_address = exception.address.unwrap_or(0);
            UserExit::Exception(exception)
        }
    }
}

impl crate::user::x86_64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the vector and
    /// error code the entry code saved and the faulting address it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        let address = (self.vector == PAGE_FAULT_VECTOR).then_some(self.fault_address);
        decode(self.vector, self.error_code, self.rip, address)
    }
}

/// [`crate::Platform::user_test_program`].
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const x86_64_user_test_start) as usize;
    let end = (&raw const x86_64_user_test_end) as usize;
    // SAFETY: the symbols bound the program's bytes in `.rodata.user_test`, which is part of the
    // kernel image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: (&raw const x86_64_user_test_checks) as usize - start,
        floating_point: (&raw const x86_64_user_test_floating_point) as usize - start,
        null_syscalls: (&raw const x86_64_user_test_null_syscalls) as usize - start,
        spin: (&raw const x86_64_user_test_spin) as usize - start,
        linux_hello: (&raw const x86_64_user_test_linux_hello) as usize - start,
        linux_start: (&raw const x86_64_user_test_linux_start) as usize - start,
        linux_start_end: (&raw const x86_64_user_test_linux_start_end) as usize - start,
        linux_unknown_syscalls: (&raw const x86_64_user_test_linux_unknown_syscalls) as usize
            - start,
        stack_growth: (&raw const x86_64_user_test_stack_growth) as usize - start,
        linux_thread_pointer: (&raw const x86_64_user_test_linux_thread_pointer) as usize - start,
        linux_poll: (&raw const x86_64_user_test_linux_poll) as usize - start,
        linux_signals: (&raw const x86_64_user_test_linux_signals) as usize - start,
        linux_signal_handler: (&raw const x86_64_user_test_linux_signal_handler) as usize - start,
        linux_fault_handler: (&raw const x86_64_user_test_linux_fault_handler) as usize - start,
        linux_signal_queue: (&raw const x86_64_user_test_linux_signal_queue) as usize - start,
        linux_signal_suspend: (&raw const x86_64_user_test_linux_signal_suspend) as usize - start,
        linux_clock: (&raw const x86_64_user_test_linux_clock) as usize - start,
        linux_stat: (&raw const x86_64_user_test_linux_stat) as usize - start,
        linux_credentials: (&raw const x86_64_user_test_linux_credentials) as usize - start,
    }
}

fn decode(vector: u64, error_code: u64, pc: u64, address: Option<u64>) -> Exception {
    let (kind, name) = match vector {
        0 => (ExceptionKind::Arithmetic, "#DE divide error"),
        1 => (ExceptionKind::Debug, "#DB debug"),
        2 => (ExceptionKind::Interrupt, "NMI"),
        3 => (ExceptionKind::Breakpoint, "#BP breakpoint"),
        4 => (ExceptionKind::Other, "#OF overflow"),
        5 => (ExceptionKind::Other, "#BR bound range exceeded"),
        6 => (ExceptionKind::IllegalInstruction, "#UD invalid opcode"),
        7 => (
            ExceptionKind::IllegalInstruction,
            "#NM device not available",
        ),
        8 => (ExceptionKind::Other, "#DF double fault"),
        10 => (ExceptionKind::Other, "#TS invalid TSS"),
        11 => (ExceptionKind::Segment, "#NP segment not present"),
        12 => (ExceptionKind::Segment, "#SS stack-segment fault"),
        13 => (ExceptionKind::Other, "#GP general protection"),
        14 if error_code & PAGE_FAULT_INSTRUCTION_FETCH != 0 => {
            (ExceptionKind::InstructionFault, "#PF page fault")
        }
        14 => (ExceptionKind::DataFault, "#PF page fault"),
        16 => (ExceptionKind::Arithmetic, "#MF x87 floating-point error"),
        17 => (ExceptionKind::Alignment, "#AC alignment check"),
        18 => (ExceptionKind::Other, "#MC machine check"),
        19 => (
            ExceptionKind::Arithmetic,
            "#XM SIMD floating-point exception",
        ),
        20 => (ExceptionKind::Other, "#VE virtualization exception"),
        21 => (ExceptionKind::Other, "#CP control protection"),
        28 => (ExceptionKind::Other, "#HV hypervisor injection"),
        29 => (ExceptionKind::Other, "#VC VMM communication"),
        30 => (ExceptionKind::Other, "#SX security exception"),
        32.. => (ExceptionKind::Interrupt, "unexpected interrupt"),
        _ => (ExceptionKind::Other, "reserved exception vector"),
    };
    Exception {
        kind,
        name,
        code: error_code,
        pc,
        address,
    }
}
