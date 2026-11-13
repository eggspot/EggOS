//! x86_64 exception and interrupt vectors (Intel SDM vol. 3A, chapter 6 "Interrupt and
//! Exception Handling").
//!
//! The IDT has a gate for each of the 32 architecture-defined exception vectors and for each
//! interrupt vector in use (the legacy PICs' 0x20-0x2f, the APIC timer's 0x30, the wake-up
//! interrupt's 0x31, the console input's 0x32, the device lines' 0x33-0x52 and the APIC's
//! spurious vector 0xff, see [`super::timer`] and [`super::message_interrupts`]). Each points at
//! a 16-byte stub that pushes the vector number (and a 0 error code for the vectors where the
//! CPU pushes none), so every exception and interrupt reaches
//! `exception_common` with the same frame. From the kernel, that saves the registers a call may
//! change (the handler keeps the others itself), calls [`x86_64_exception`] with the frame, and
//! restores them and `iretq`s if it returns. From ring 3 (D47), the CPU has switched to this CPU's
//! `TSS.RSP0`, which points into the running program's `UserContext`: the entry saves every
//! register there, switches back to the kernel's GS base and returns from [`run_user`]; `syscall`
//! builds the same frame and joins it. The kernel target uses no SSE/x87 registers
//! (`x86_64-unknown-none` is soft-float), so the entry does not save them: a program's are saved
//! and loaded by the scheduler, per thread, once it has used them (`fp`, D54); until then `CR0.TS`
//! traps them (D49).
//!
//! Exceptions run on the current stack, except the double fault, NMI, machine check and debug
//! exceptions, which each CPU takes on stacks of its own from the interrupt stack table once its
//! TSS is loaded (D248): a kernel stack overflow is reported instead of resetting the machine,
//! and an NMI in the moment `syscall` and `sysretq` spend in ring 0 on a program's stack does not
//! run on it. One of those from ring 3 has its frame moved to where `TSS.RSP0` would have put it.

use super::message_interrupts::{self, FIRST_DEVICE_VECTOR, LAST_DEVICE_VECTOR};
use super::timer::{
    self, APIC_SPURIOUS_VECTOR, CONSOLE_INPUT_VECTOR, LEGACY_PIC_VECTORS, POWER_BUTTON_VECTOR,
    TIMER_VECTOR, WAKEUP_VECTOR,
};
use crate::cpu::{
    CONTEXT_TOP_OFFSET, ENTRY_STACK_TOP_OFFSET, GDT_ENTRIES, HalCpuLocal, KERNEL_CR3_OFFSET,
    SEGMENT_BASES_ID_OFFSET, TSS_RSP0_OFFSET, Tss, USER_CR3_OFFSET, USER_RSP_OFFSET,
};
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Format, x86_64::X86_64};
use crate::user::x86_64::{RESTORE_ALL_VECTOR, Registers, SYSCALL_VECTOR};
use crate::user::{TestProgram, UserContext, UserExit};
use core::cell::UnsafeCell;

core::arch::global_asm!(
    r#"
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
    .balign 4096
    .global x86_64_entry_text_start
x86_64_entry_text_start:
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
    .irp vector, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50
    EXCEPTION_STUB \vector
    .endr
    .irp vector, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66
    EXCEPTION_STUB \vector
    .endr
    .irp vector, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83
    EXCEPTION_STUB \vector
    .endr
    .balign 16                      // so the label is the stub, not the padding before it
    .global apic_spurious_stub
apic_spurious_stub:
    EXCEPTION_STUB 255

exception_common:
    testb   $3, 24(%rsp)
    jnz     user_exception
    pushq   %rax
    movq    8(%rsp), %rax
    cmpq    $2, %rax
    je      8f
    cmpq    $18, %rax
    je      8f
    cmpq    $1, %rax
    je      8f
    cmpq    $8, %rax
    jne     9f
8:  movq    %cr3, %rax
    testq   ${user_view_marker}, %rax
    jz      9f
    movq    %rax, 64(%rsp)
    movq    72(%rsp), %rax
    movq    %rax, %cr3
9:  popq    %rax
    lfence
    cmpb    $0, {smap}(%rip)
    je      7f
    clac
7:
    pushq   %rax
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
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
    pushq   %rax
    movq    8(%rsp), %rax
    cmpq    $2, %rax
    je      8f
    cmpq    $18, %rax
    je      8f
    cmpq    $1, %rax
    je      8f
    cmpq    $8, %rax
    jne     9f
8:  movq    64(%rsp), %rax
    testq   %rax, %rax
    jz      9f
    movq    $0, 64(%rsp)
    movq    %rax, %cr3
9:  popq    %rax
    addq    $16, %rsp               // vector and error code
    iretq
    int3                            // never runs: stops straight-line speculation (D49)

user_exception:
    swapgs
    cmpq    $0, %gs:{user_cr3}
    je      10f
    pushq   %rax
    movq    %gs:{kernel_cr3}, %rax
    movq    %rax, %cr3
    popq    %rax
10: lfence
    pushq   %rdx
    pushq   %rcx
    pushq   %rax
    movq    %gs:{context_top}, %rax
    subq    $56, %rax
    leaq    24(%rsp), %rcx
    cmpq    %rax, %rcx
    je      4f
    movq    0(%rcx), %rdx
    movq    %rdx, 0(%rax)
    movq    8(%rcx), %rdx
    movq    %rdx, 8(%rax)
    movq    16(%rcx), %rdx
    movq    %rdx, 16(%rax)
    movq    24(%rcx), %rdx
    movq    %rdx, 24(%rax)
    movq    32(%rcx), %rdx
    movq    %rdx, 32(%rax)
    movq    40(%rcx), %rdx
    movq    %rdx, 40(%rax)
    movq    48(%rcx), %rdx
    movq    %rdx, 48(%rax)
    movq    %rsp, %rcx
    movq    %rax, %rsp
    movq    16(%rcx), %rdx
    movq    0(%rcx), %rax
    movq    8(%rcx), %rcx
    jmp     user_exception_save
4:  popq    %rax
    popq    %rcx
    popq    %rdx
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
    cld
    cmpb    $0, {ibrs_on_entry}(%rip)
    je      .Lx86_64_entry_predictions_set
    movl    ${ia32_spec_ctrl}, %ecx
    movl    ${spec_ctrl_ibrs}, %eax
    xorl    %edx, %edx
    wrmsr
.Lx86_64_entry_predictions_set:
    cmpb    $0, {ibpb_on_entry}(%rip)
    je      .Lx86_64_entry_returns_flushed
    movl    ${ia32_pred_cmd}, %ecx
    movl    ${pred_cmd_ibpb}, %eax
    xorl    %edx, %edx
    wrmsr
.Lx86_64_entry_returns_flushed:
    movq    {kernel_rsp}(%rsp), %rsp
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %rbp
    popq    %rbx
    retq
    int3

    .balign 16
    .global x86_64_syscall_entry
x86_64_syscall_entry:
    swapgs
    movq    %rsp, %gs:{user_rsp}
    cmpq    $0, %gs:{user_cr3}
    je      11f
    movq    %gs:{kernel_cr3}, %rsp
    movq    %rsp, %cr3
11: movq    %gs:{context_top}, %rsp
    pushq   ${user_ss}
    pushq   %gs:{user_rsp}
    pushq   %r11                    // rflags
    pushq   ${user_cs}
    pushq   %rcx                    // rip
    pushq   $0                      // error code
    pushq   ${syscall_vector}
    jmp     user_exception_save

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
    movq    %rax, %gs:{context_top}
    cmpq    $0, %gs:{user_cr3}
    jne     12f
    movq    %rax, %gs:{tss_rsp0}
12:
    movq    {segment_bases_id}(%rdi), %rax
    cmpq    %rax, %gs:{cpu_segment_bases_id}
    jne     2f
3:  cmpb    $0, {ibrs_on_entry}(%rip)
    je      .Lx86_64_exit_predictions_cleared
    movl    ${ia32_spec_ctrl}, %ecx
    xorl    %eax, %eax
    xorl    %edx, %edx
    wrmsr
.Lx86_64_exit_predictions_cleared:
    cmpq    ${syscall_vector}, {vector}(%rdi)
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
    cmpb    $0, {clear_buffers}(%rip)
    je      5f
    verw    {verw_selector}(%rip)
5:  cmpq    $0, %gs:{user_cr3}
    je      13f
    pushq   %rax
    pushq   %rdx
    movq    %gs:{entry_stack_top}, %rax
    subq    $56, %rax
    movq    0(%rsp), %rdx
    movq    %rdx, 0(%rax)
    movq    8(%rsp), %rdx
    movq    %rdx, 8(%rax)
    movq    16(%rsp), %rdx
    movq    %rdx, 16(%rax)
    movq    24(%rsp), %rdx
    movq    %rdx, 24(%rax)
    movq    32(%rsp), %rdx
    movq    %rdx, 32(%rax)
    movq    40(%rsp), %rdx
    movq    %rdx, 40(%rax)
    movq    48(%rsp), %rdx
    movq    %rdx, 48(%rax)
    movq    %rax, %rsp
    movq    %gs:{user_cr3}, %rax
    movq    %rax, %cr3
    popq    %rdx
    popq    %rax
13: swapgs
    iretq
    int3
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
    cmpq    $0, %gs:{user_cr3}
    jne     14f
    movq    {rsp}(%rdi), %rsp       // the program's stack
    movq    {rdi}(%rdi), %rdi
    cmpb    $0, {clear_buffers}(%rip)
    je      6f
    verw    {verw_selector}(%rip)
6:  swapgs
    sysretq
    int3
14: movq    %gs:{entry_stack_top}, %rsp
    pushq   {rsp}(%rdi)
    pushq   {rdi}(%rdi)
    cmpb    $0, {clear_buffers}(%rip)
    je      15f
    verw    {verw_selector}(%rip)
15: movq    %gs:{user_cr3}, %rdi
    movq    %rdi, %cr3
    popq    %rdi
    popq    %rsp
    swapgs
    sysretq
    int3
2:  movq    %rax, %gs:{cpu_segment_bases_id}
    movq    {fs_base}(%rdi), %rax
    movq    %rax, %rdx
    shrq    $32, %rdx
    movl    ${ia32_fs_base}, %ecx
    wrmsr
    movq    {gs_base}(%rdi), %rax
    movq    %rax, %rdx
    shrq    $32, %rdx
    movl    ${ia32_kernel_gs_base}, %ecx
    wrmsr
    jmp     3b
    int3
    .balign 4096
    .global x86_64_entry_text_end
x86_64_entry_text_end:
    .popsection

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
    movq    %rsp, %rax
    andq    $-4096, %rax
6:  movq    %rax, (%rax)
    subq    $4096, %rax
    jmp     6b
    .global x86_64_user_test_linux_thread_pointer
x86_64_user_test_linux_thread_pointer:
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
    movl    ${signal_fp_kept}, %eax
    movq    %rax, %xmm8
    movl    ${signal_mxcsr}, -80(%rsp)
    ldmxcsr -80(%rsp)
    movl    $234, %eax
    syscall                         // the handler runs before this returns
    testq   %rax, %rax
    jnz     9f
    cmpq    $0x5a5, %rbx
    jne     9f
    cmpq    $2, %r12
    jne     9f
    movq    %xmm8, %rax
    cmpq    ${signal_fp_kept}, %rax
    jne     9f
    movq    %xmm9, %rax
    cmpq    ${signal_fp_changed}, %rax
    jne     9f
    stmxcsr -80(%rsp)
    cmpl    ${signal_mxcsr}, -80(%rsp)
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
    movq    %xmm8, %rax
    testq   %rax, %rax
    jnz     7f
    stmxcsr -8(%rsp)
    cmpl    ${initial_mxcsr}, -8(%rsp)
    jne     7f
    movq    {saved_fpstate}(%rdx), %rax
    cmpl    ${signal_mxcsr}, {fxsave_mxcsr}(%rax)
    jne     7f
    cmpq    ${signal_fp_kept}, {fxsave_xmm8}(%rax)
    jne     7f
    movq    ${signal_fp_changed}, {fxsave_xmm9}(%rax)
    movq    $2, {saved_r12}(%rdx)
7:  xorl    %ebx, %ebx
    ret
6:  movl    $15, %eax
    syscall
    int3
    .global x86_64_user_test_linux_fault_handler
x86_64_user_test_linux_fault_handler:
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
    testq   %rax, %rax
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
    .global x86_64_user_test_fp_registers
x86_64_user_test_fp_registers:
    movl    ${syscall_first}, %eax
    syscall
    movq    %rax, %rbx
    xorl    %r12d, %r12d
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    movq    %xmm\i, %rcx
    orq     %rcx, %r12
    pshufd  $0x4e, %xmm\i, %xmm\i
    movq    %xmm\i, %rcx
    orq     %rcx, %r12
    .endr
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    leaq    \i(%rbx), %rcx
    movq    %rcx, %xmm\i
    punpcklqdq %xmm\i, %xmm\i
    .endr
7:  movl    ${syscall_null}, %eax
    syscall
    movq    %rax, %r13
    .irp    i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    leaq    \i(%rbx), %rdx
    movq    %xmm\i, %rcx
    xorq    %rdx, %rcx
    orq     %rcx, %r12
    pshufd  $0x4e, %xmm\i, %xmm\i
    movq    %xmm\i, %rcx
    xorq    %rdx, %rcx
    orq     %rcx, %r12
    .endr
    testq   %r13, %r13
    jz      7b
    movq    %r12, %rdi
    movl    ${syscall_report}, %eax
    syscall
8:  jmp     8b
    .global x86_64_user_test_illegal_instruction
x86_64_user_test_illegal_instruction:
    ud2
    .global x86_64_user_test_linux_move_and_discard
x86_64_user_test_linux_move_and_discard:
    xorl    %edi, %edi
    movl    $4096, %esi
    movl    $3, %edx
    movl    $0x22, %r10d
    movq    $-1, %r8
    xorl    %r9d, %r9d
    movl    $9, %eax
    syscall
    cmpq    $-4096, %rax
    jae     9f
    movq    %rax, %rbx
    movq    %rax, %rdi
    movl    $4096, %esi
    movl    $23, %edx
    movl    $28, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    movq    $0x12345678, %r12
    movq    %r12, (%rbx)
    leaq    -0x100000(%rbx), %r13
    movq    %rbx, %rdi
    movl    $4096, %esi
    movl    $4096, %edx
    movl    $3, %r10d
    movq    %r13, %r8
    movl    $25, %eax
    syscall
    cmpq    %r13, %rax
    jne     9f
    cmpq    %r12, (%r13)
    jne     9f
    movq    %rbx, %rdi
    movl    $4096, %esi
    movl    $3, %edx
    movl    $0x32, %r10d
    movq    $-1, %r8
    xorl    %r9d, %r9d
    movl    $9, %eax
    syscall
    cmpq    %rbx, %rax
    jne     9f
    cmpq    $0, (%rbx)
    jne     9f
    movq    %r13, %rdi
    movl    $4096, %esi
    movl    $4, %edx
    movl    $28, %eax
    syscall
    testq   %rax, %rax
    jnz     9f
    cmpq    $0, (%r13)
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
    segment_bases_id = const core::mem::offset_of!(Registers, segment_bases_id),
    cpu_segment_bases_id = const SEGMENT_BASES_ID_OFFSET,
    ia32_fs_base = const IA32_FS_BASE,
    gs_base = const core::mem::offset_of!(Registers, gs_base),
    ia32_kernel_gs_base = const IA32_KERNEL_GS_BASE,
    tss_rsp0 = const TSS_RSP0_OFFSET,
    user_cr3 = const USER_CR3_OFFSET,
    kernel_cr3 = const KERNEL_CR3_OFFSET,
    context_top = const CONTEXT_TOP_OFFSET,
    entry_stack_top = const ENTRY_STACK_TOP_OFFSET,
    user_view_marker = const USER_VIEW_MARKER,
    clear_buffers = sym CLEAR_CPU_BUFFERS,
    ibrs_on_entry = sym IBRS_ON_ENTRY,
    ia32_spec_ctrl = const IA32_SPEC_CTRL,
    spec_ctrl_ibrs = const SPEC_CTRL_IBRS,
    ibpb_on_entry = sym IBPB_ON_ENTRY,
    ia32_pred_cmd = const IA32_PRED_CMD,
    pred_cmd_ibpb = const PRED_CMD_IBPB,
    smap = sym SMAP_ON,
    verw_selector = sym VERW_SELECTOR,
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
    saved_r12 = const 40 + 4 * 8,
    saved_fpstate = const 40 + 184,
    fxsave_mxcsr = const 24,
    fxsave_xmm8 = const 160 + 8 * 16,
    fxsave_xmm9 = const 160 + 9 * 16,
    signal_fp_kept = const TestProgram::LINUX_SIGNAL_FP_KEPT,
    signal_fp_changed = const TestProgram::LINUX_SIGNAL_FP_CHANGED,
    signal_mxcsr = const 0x3f80,
    initial_mxcsr = const 0x1f80,
    saved_r13 = const 40 + 5 * 8,
    saved_r14 = const 40 + 6 * 8,
    saved_r15 = const 40 + 7 * 8,
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

const VECTORS: usize = 256;
const CONTIGUOUS_STUBS: usize = POWER_BUTTON_VECTOR as usize + 1;
const _: () = assert!(POWER_BUTTON_VECTOR as usize == LAST_DEVICE_VECTOR as usize + 1);
const _: () = assert!(TIMER_VECTOR < WAKEUP_VECTOR && WAKEUP_VECTOR < CONSOLE_INPUT_VECTOR);
const _: () = assert!(CONSOLE_INPUT_VECTOR == 50 && FIRST_DEVICE_VECTOR == 51);
const _: () = assert!(LAST_DEVICE_VECTOR == 82);
const FIRST_INTERRUPT_VECTOR: u64 = 32;
const STUB_SIZE: usize = 16;

const KERNEL_CODE_SELECTOR: u16 = 0x08;
const INTERRUPT_GATE: u8 = 0x8e;

const PAGE_FAULT_VECTOR: u64 = 14;
const PAGE_FAULT_PRESENT: u64 = 1 << 0;
const PAGE_FAULT_WRITE: u64 = 1 << 1;
const PAGE_FAULT_USER: u64 = 1 << 2;
const PAGE_FAULT_RESERVED_BIT: u64 = 1 << 3;
const PAGE_FAULT_INSTRUCTION_FETCH: u64 = 1 << 4;

unsafe extern "C" {
    fn exception_stubs();
    fn apic_spurious_stub();
}

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

static IDT_WITH_STACKS: InterruptDescriptorTable =
    InterruptDescriptorTable(UnsafeCell::new([GateDescriptor::MISSING; VECTORS]));

const STACK_VECTORS: [(usize, u8); 4] = [(8, 1), (2, 2), (18, 3), (1, 4)];

static CLEAR_CPU_BUFFERS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

static VERW_SELECTOR: u16 = 0x10;

static USER_VIEW: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
const ARCH_CAPABILITIES_RDCL_NO: u64 = 1;
const USER_VIEW_MARKER: u64 = 1 << 11;
const IST_SLOT_BYTES: u64 = 16;
const CR3_ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const CR4_GLOBAL_PAGES: u64 = 1 << 7;
const USER_HALF_ENTRIES: usize = 256;

fn needs_user_view() -> bool {
    if !is_intel() {
        return false;
    }
    if super::timer::cpuid(0, 0)[0] < 7
        || super::timer::cpuid(7, 0)[3] & CPUID_ARCH_CAPABILITIES == 0
    {
        return true;
    }
    super::read_msr(IA32_ARCH_CAPABILITIES) & ARCH_CAPABILITIES_RDCL_NO == 0
}

/// [`crate::Platform::isolates_user_view`] on x86_64.
pub(super) fn isolates_user_view() -> bool {
    USER_VIEW.load(core::sync::atomic::Ordering::Relaxed) != 0
}

unsafe extern "C" {
    static x86_64_entry_text_start: u8;
    static x86_64_entry_text_end: u8;
}

/// [`crate::Platform::user_view_ranges`] on x86_64, for the CPU whose block is at `block`: the
/// entry and exit code (its own pages), both IDTs and the block up to the end of its HAL part.
pub(super) fn user_view_ranges(block: usize, range: &mut dyn FnMut(u64, u64)) {
    let start = (&raw const x86_64_entry_text_start) as u64;
    let end = (&raw const x86_64_entry_text_end) as u64;
    range(start, end - start);
    let table_bytes = core::mem::size_of::<InterruptDescriptorTable>() as u64;
    range((&raw const IDT) as u64, table_bytes);
    range((&raw const IDT_WITH_STACKS) as u64, table_bytes);
    let block_bytes = crate::cpu::HAL_CPU_LOCAL_OFFSET + core::mem::size_of::<HalCpuLocal>();
    range(block as u64, block_bytes as u64);
}

fn tables_of(block: usize) -> *mut crate::cpu::X86Tables {
    // SAFETY: `block` is a per-CPU block with a `HalCpuLocal` at its offset
    // (`set_cpu_local`'s contract); only its address is formed here.
    let hal = unsafe { &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const HalCpuLocal) };
    hal.x86.0.get()
}

/// [`crate::Platform::set_user_view`] on x86_64: the entry stack becomes `TSS.RSP0` for good,
/// the root is kept with its marker, and global pages are turned off, so no kernel translation
/// outlives a switch to the user view.
///
/// # Safety
/// As [`crate::Platform::set_user_view`]; `block` is this CPU's block.
pub(super) unsafe fn set_user_view(block: usize, root: u64) {
    let tables = tables_of(block);
    // SAFETY: the block is this CPU's and interrupts are masked (the caller's duty), so nothing
    // else reads or writes its tables now; the entry code reads them only from the next entry.
    unsafe {
        let top = (&raw const (*tables).entry_stack) as u64 + crate::cpu::ENTRY_STACK_BYTES as u64;
        (*tables).entry_stack_top = top;
        (*tables).tss.rsp[0] = top;
        (*tables).user_cr3 = root & CR3_ADDRESS | USER_VIEW_MARKER;
        crate::paging::x86_64::set_global_kernel_pages(false);
        core::arch::asm!(
            "mov {tmp}, cr4",
            "and {tmp}, {keep}",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            keep = in(reg) !CR4_GLOBAL_PAGES,
            options(nostack, preserves_flags),
        );
    }
}

unsafe fn fill_user_view() {
    let tables = tables_of(<super::X86_64 as crate::Platform>::cpu_local());
    // SAFETY: this CPU's block, with interrupts masked (the caller's duty); both roots are page
    // tables in RAM the direct map covers, the user view's kept for good by the kernel and the
    // active one live while it is loaded; their user halves are 256 entries each.
    unsafe {
        let user_cr3 = (*tables).user_cr3;
        if user_cr3 == 0 {
            return;
        }
        let kernel_cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) kernel_cr3, options(nostack, preserves_flags));
        let offset = super::DIRECT_MAP_OFFSET as u64;
        let from = ((kernel_cr3 & CR3_ADDRESS) + offset) as *const u64;
        let to = ((user_cr3 & CR3_ADDRESS) + offset) as *mut u64;
        core::ptr::copy_nonoverlapping(from, to, USER_HALF_ENTRIES);
        (*tables).kernel_cr3 = kernel_cr3;
        for slot in 0..crate::cpu::EXCEPTION_STACKS {
            let stack = (&raw mut (*tables).exception_stacks[slot]) as u64;
            let top = stack + crate::cpu::EXCEPTION_STACK_BYTES as u64;
            ((top - 8) as *mut u64).write(kernel_cr3);
        }
    }
}

const CPUID_MD_CLEAR: u32 = 1 << 10;
const CPUID_ARCH_CAPABILITIES: u32 = 1 << 29;
const IA32_ARCH_CAPABILITIES: u32 = 0x10a;
const ARCH_CAPABILITIES_MDS_NO: u64 = 1 << 5;
const ARCH_CAPABILITIES_TAA_NO: u64 = 1 << 8;

fn needs_buffer_clearing() -> bool {
    if super::timer::cpuid(0, 0)[0] < 7 {
        return false;
    }
    let features = super::timer::cpuid(7, 0)[3];
    if features & CPUID_MD_CLEAR == 0 {
        return false;
    }
    if features & CPUID_ARCH_CAPABILITIES == 0 {
        return true;
    }
    let capabilities = super::read_msr(IA32_ARCH_CAPABILITIES);
    capabilities & ARCH_CAPABILITIES_MDS_NO == 0 || capabilities & ARCH_CAPABILITIES_TAA_NO == 0
}

/// Fills the IDT and loads it. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn install() {
    fill();
    load();
    if needs_buffer_clearing() {
        CLEAR_CPU_BUFFERS.store(1, core::sync::atomic::Ordering::Relaxed);
    }
    if needs_user_view() {
        USER_VIEW.store(1, core::sync::atomic::Ordering::Relaxed);
    }
}

fn fill() {
    let stubs = exception_stubs as *const () as u64;
    let gates = IDT.0.get();
    let stub_gates = (0..CONTIGUOUS_STUBS)
        .map(|vector| (vector, stubs + (vector * STUB_SIZE) as u64))
        .chain([(
            APIC_SPURIOUS_VECTOR.into(),
            apic_spurious_stub as *const () as u64,
        )]);
    for (vector, stub) in stub_gates {
        // SAFETY: `fill` is the only writer and runs once, from `install`, before any CPU loads
        // the IDT, so nothing else accesses the table; `vector` is below 256, in bounds.
        unsafe { (*gates)[vector] = GateDescriptor::interrupt_gate(stub) };
    }
    let stacked = IDT_WITH_STACKS.0.get();
    // SAFETY: as above, for the second table, which only `fill` writes, before any CPU loads it.
    unsafe {
        *stacked = *gates;
        for (vector, index) in STACK_VECTORS {
            (*stacked)[vector].ist = index;
        }
    }
}

fn load_with_stacks() {
    let gates = IDT_WITH_STACKS.0.get();
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[GateDescriptor; VECTORS]>() - 1) as u16,
        base: gates as u64,
    };
    // SAFETY: filled by `fill` before any CPU runs this, with the same stubs and selector as
    // `IDT`; the caller's CPU has its TSS loaded with every stack the table names.
    unsafe {
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
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

extern "C" fn x86_64_exception(frame: &mut TrapFrame) {
    if interrupt(frame.vector) {
        return;
    }
    deliver(&exception(frame.vector, frame.error_code, frame.rip));
}

#[inline(always)]
fn interrupt(vector: u64) -> bool {
    match vector {
        vector if vector == u64::from(TIMER_VECTOR) => {
            timer::interrupt();
            true
        }
        vector if vector < FIRST_INTERRUPT_VECTOR => false,
        vector if vector == u64::from(WAKEUP_VECTOR) => {
            timer::wakeup_interrupt();
            true
        }
        vector if vector == u64::from(CONSOLE_INPUT_VECTOR) => {
            super::console_input::interrupt();
            true
        }
        vector if vector == u64::from(POWER_BUTTON_VECTOR) => {
            super::power_button::interrupt();
            true
        }
        vector
            if (u64::from(FIRST_DEVICE_VECTOR)..=u64::from(LAST_DEVICE_VECTOR))
                .contains(&vector) =>
        {
            message_interrupts::interrupt(vector);
            true
        }
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

const KERNEL_DATA_SELECTOR: u64 = 0x10;
const USER_DATA_SELECTOR: u64 = 0x18 | 3;
const USER_CODE_SELECTOR: u64 = 0x20 | 3;
const TSS_SELECTOR: u16 = 0x28;

const USER_RFLAGS: u64 = 0xcd5 | 1 << 21;
const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
const CR4_FSGSBASE: u64 = 1 << 16;
const CR4_OSXSAVE: u64 = 1 << 18;
const CR4_PKE: u64 = 1 << 22;
const CR4_CET: u64 = 1 << 23;
const CR4_UINTR: u64 = 1 << 25;
const CR4_DENIED_TO_PROGRAMS: u64 = CR4_FSGSBASE | CR4_OSXSAVE | CR4_PKE | CR4_CET | CR4_UINTR;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_UMIP: u64 = 1 << 11;
const CR4_SMEP: u64 = 1 << 20;
const CR4_SMAP: u64 = 1 << 21;
const CPUID_7_EBX_SMAP: u32 = 1 << 20;

static SMAP_ON: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

static PREDICTION_BARRIER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

static IBRS_ON_ENTRY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

static IBPB_ON_ENTRY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
const CPUID_EXTENDED_IBPB: u32 = 1 << 12;
const CPUID_EXTENDED_IBPB_BRTYPE: u32 = 1 << 28;
const CPUID_EXTENDED_SRSO_NO: u32 = 1 << 29;
const AMD_ZEN_1_2: u32 = 0x17;
const AMD_ZEN_3_4: u32 = 0x19;

/// Whether every kernel thread switch fills the return stack buffer with 32 harmless entries
/// (D289): on CPUs without enhanced IBRS, so a `ret` after the switch never predicts from what
/// the last thread (or a program before it) left there, and an empty buffer never falls back to
/// the indirect predictor (Skylake's underflow). Read by the switch code.
pub(super) static RSB_FILL_ON_SWITCH: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(0);
const IA32_SPEC_CTRL: u32 = 0x48;
const IA32_PRED_CMD: u32 = 0x49;
const SPEC_CTRL_IBRS: u64 = 1;
const PRED_CMD_IBPB: u64 = 1;
const CPUID_7_EDX_IBRS_IBPB: u32 = 1 << 26;
const ARCH_CAPABILITIES_ENHANCED_IBRS: u64 = 1 << 1;

fn set_up_branch_predictions() {
    if super::timer::cpuid(0, 0)[0] < 7 {
        return;
    }
    let edx = super::timer::cpuid(7, 0)[3];
    if edx & CPUID_7_EDX_IBRS_IBPB == 0 {
        return;
    }
    PREDICTION_BARRIER.store(1, core::sync::atomic::Ordering::Relaxed);
    let enhanced = edx & CPUID_ARCH_CAPABILITIES != 0
        && super::read_msr(IA32_ARCH_CAPABILITIES) & ARCH_CAPABILITIES_ENHANCED_IBRS != 0;
    if !enhanced {
        RSB_FILL_ON_SWITCH.store(1, core::sync::atomic::Ordering::Relaxed);
    }
    let on_entry = !enhanced && is_intel();
    if !enhanced && !on_entry {
        return;
    }
    if on_entry {
        IBRS_ON_ENTRY.store(1, core::sync::atomic::Ordering::Relaxed);
    }
    // SAFETY: `IA32_SPEC_CTRL` exists when CPUID 7 EDX[26] says so (checked); setting IBRS keeps
    // lower-privilege predictions from steering the kernel's indirect branches and changes
    // nothing else. With enhanced IBRS it stays set; without it the kernel runs with it set and
    // the entry code clears it only while a program runs.
    unsafe { super::write_msr(IA32_SPEC_CTRL, SPEC_CTRL_IBRS) };
}

fn set_up_return_predictions() {
    if !is_amd() || super::timer::cpuid(0x8000_0000, 0)[0] < 0x8000_0008 {
        return;
    }
    let signature = super::timer::cpuid(1, 0)[0];
    let family = ((signature >> 8) & 0xf) + ((signature >> 20) & 0xff);
    let extended = super::timer::cpuid(0x8000_0008, 0)[1];
    let srso_no = super::timer::cpuid(0x8000_0000, 0)[0] >= 0x8000_0021
        && super::timer::cpuid(0x8000_0021, 0)[0] & CPUID_EXTENDED_SRSO_NO != 0;
    let flushes_returns = match family {
        AMD_ZEN_1_2 => true,
        AMD_ZEN_3_4 => extended & CPUID_EXTENDED_IBPB_BRTYPE != 0,
        _ => false,
    };
    if !srso_no && flushes_returns && extended & CPUID_EXTENDED_IBPB != 0 {
        IBPB_ON_ENTRY.store(1, core::sync::atomic::Ordering::Relaxed);
    }
}

fn is_amd() -> bool {
    let [_, ebx, ecx, edx] = super::timer::cpuid(0, 0);
    [ebx, edx, ecx]
        == [
            u32::from_le_bytes(*b"Auth"),
            u32::from_le_bytes(*b"enti"),
            u32::from_le_bytes(*b"cAMD"),
        ]
}

fn is_intel() -> bool {
    let [_, ebx, ecx, edx] = super::timer::cpuid(0, 0);
    [ebx, edx, ecx]
        == [
            u32::from_le_bytes(*b"Genu"),
            u32::from_le_bytes(*b"ineI"),
            u32::from_le_bytes(*b"ntel"),
        ]
}

/// Issues an indirect branch prediction barrier where the CPU has one, so a program cannot
/// steer the next one's indirect branches (D260).
pub(super) fn barrier_predictions() {
    if PREDICTION_BARRIER.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return;
    }
    // SAFETY: `IA32_PRED_CMD` exists when CPUID 7 EDX[26] says so, which `PREDICTION_BARRIER`
    // records; writing IBPB only discards learned predictions.
    unsafe { super::write_msr(IA32_PRED_CMD, PRED_CMD_IBPB) };
}

/// Whether SMAP is on, so user accesses need `stac` and `clac`.
pub(super) fn smap_on() -> bool {
    SMAP_ON.load(core::sync::atomic::Ordering::Relaxed) != 0
}
const CPUID_7_EBX_SMEP: u32 = 1 << 7;
const CPUID_7_ECX_UMIP: u32 = 1 << 2;

fn supervisor_protections() -> u64 {
    if super::timer::cpuid(0, 0)[0] < 7 {
        return 0;
    }
    let [_, ebx, ecx, _] = super::timer::cpuid(7, 0);
    let smep = if ebx & CPUID_7_EBX_SMEP != 0 {
        CR4_SMEP
    } else {
        0
    };
    let umip = if ecx & CPUID_7_ECX_UMIP != 0 {
        CR4_UMIP
    } else {
        0
    };
    set_up_branch_predictions();
    set_up_return_predictions();
    let smap = if ebx & CPUID_7_EBX_SMAP != 0 {
        SMAP_ON.store(1, core::sync::atomic::Ordering::Relaxed);
        CR4_SMAP
    } else {
        0
    };
    smep | umip | smap | tagged_translations()
}

const CR4_PCIDE: u64 = 1 << 17;
const CPUID_1_ECX_PCID: u32 = 1 << 17;
static PCIDS_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Global kernel pages and PCIDs (D552), on CPUs that need no separate user view: with one,
/// every switch to it must drop the kernel's translations, which global pages would keep, and
/// the entry code's CR3 loads carry no PCID.
fn tagged_translations() -> u64 {
    if needs_user_view() {
        return 0;
    }
    crate::paging::x86_64::set_global_kernel_pages(true);
    if super::timer::cpuid(1, 0)[2] & CPUID_1_ECX_PCID != 0 {
        PCIDS_ON.store(true, core::sync::atomic::Ordering::Relaxed);
        CR4_GLOBAL_PAGES | CR4_PCIDE
    } else {
        CR4_GLOBAL_PAGES
    }
}

/// Whether CR3 carries a PCID on this machine (D552).
pub(super) fn pcids_on() -> bool {
    PCIDS_ON.load(core::sync::atomic::Ordering::Relaxed) && !isolates_user_view()
}
const CPUID_SMEP: u32 = 1 << 7;
const RFLAGS_INTERRUPTS: u64 = 1 << 9;
const RFLAGS_FIXED: u64 = 1 << 1;
const SYSCALL_MASKED_RFLAGS: u64 = 1 << 9 | 1 << 8 | 1 << 10 | 1 << 18 | 1 << 14;

const IA32_EFER: u32 = 0xc000_0080;
const EFER_SYSCALL: u64 = 1 << 0;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_FMASK: u32 = 0xc000_0084;
const IA32_FS_BASE: u32 = 0xc000_0100;
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;

unsafe extern "C" {
    fn x86_64_enter_user(context: *mut Registers);
    fn x86_64_syscall_entry();
    static x86_64_user_test_start: u8;
    static x86_64_user_test_checks: u8;
    static x86_64_user_test_floating_point: u8;
    static x86_64_user_test_null_syscalls: u8;
    static x86_64_user_test_spin: u8;
    static x86_64_user_test_fp_registers: u8;
    static x86_64_user_test_illegal_instruction: u8;
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
    static x86_64_user_test_linux_move_and_discard: u8;
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
    for (slot, (_, index)) in STACK_VECTORS.iter().enumerate() {
        // SAFETY: as above; the stack is this CPU's own, in its block, used only by the CPU when
        // it takes one of these exceptions; its end is its top.
        unsafe {
            let stack = &raw const (*tables).exception_stacks[slot];
            let top = stack as u64 + crate::cpu::EXCEPTION_STACK_BYTES as u64 - IST_SLOT_BYTES;
            (*tables).tss.ist[usize::from(*index) - 1] = top;
        }
    }
    // SAFETY: as above; only the address is taken.
    let tss_base = unsafe { &raw const (*tables).tss } as u64;
    let tss_limit = core::mem::size_of::<Tss>() as u64 - 1;
    let gdt: [u64; GDT_ENTRIES] = [
        0,
        0x00af_9b00_0000_ffff,
        0x00cf_9300_0000_ffff,
        0x00cf_f300_0000_ffff,
        0x00af_fb00_0000_ffff,
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
    load_with_stacks();
    // SAFETY: CR0 and CR4 only change what x87, MMX and SSE instructions do: with TS and MP
    // set and EM clear, each raises #NM, in ring 3 and in the kernel, which uses none, until
    // the scheduler clears TS for a program that has its own FP/SIMD registers (D54), so none
    // can read what another left in them (D49). NE reports x87 errors as #MF; OSFXSR enables
    // SSE and `FXSAVE`/`FXRSTOR`, OSXMMEXCPT reports unmasked SSE exceptions as #XM. SMEP and
    // UMIP (D251) only add faults: the kernel never executes user pages, and programs lose only
    // the descriptor-table reads.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr4",
            "or {tmp}, {set}",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            set = in(reg) CR4_OSFXSR | CR4_OSXMMEXCPT | supervisor_protections(),
            options(nomem, nostack),
        );
    }
    // SAFETY: as above.
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
    let has_smep = core::arch::x86_64::__cpuid(0).eax >= 7
        && core::arch::x86_64::__cpuid_count(7, 0).ebx & CPUID_SMEP != 0;
    if has_smep {
        // SAFETY: with SMEP the CPU refuses to run, in ring 0, an instruction from a page user
        // mode may reach, which the kernel never does: its code is in the kernel half, mapped
        // for the kernel alone. A kernel bug that jumps into a program's memory then faults
        // instead of running what the program put there.
        unsafe {
            core::arch::asm!(
                "mov {tmp}, cr4",
                "or {tmp}, {set}",
                "mov cr4, {tmp}",
                tmp = out(reg) _,
                set = in(reg) CR4_SMEP,
                options(nomem, nostack),
            );
        }
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
    // SAFETY: clearing these CR4 bits, whatever firmware left, only takes away from ring 3
    // what the kernel does not track per program (D83): writing the FS and GS bases (D49,
    // D57), `XSAVE` with AVX and later state, `WRPKRU`, shadow stacks and indirect-branch
    // tracking, and user interrupts. The kernel uses none of them.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr4",
            "and {tmp}, {clear}",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            clear = in(reg) !CR4_DENIED_TO_PROGRAMS,
            options(nomem, nostack),
        );
    }
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
    if registers.rip >= X86_64::USER_END {
        registers.vector = PAGE_FAULT_VECTOR;
        registers.error_code = PAGE_FAULT_USER | PAGE_FAULT_INSTRUCTION_FETCH;
        registers.fault_address = registers.rip;
        return UserExit::Exception(registers.last_exception());
    }
    registers.cs = USER_CODE_SELECTOR;
    registers.ss = USER_DATA_SELECTOR;
    registers.rflags = registers.rflags & USER_RFLAGS | RFLAGS_INTERRUPTS | RFLAGS_FIXED;
    if isolates_user_view() {
        // SAFETY: interrupts are masked and the active tables are the program's (the caller's
        // duty).
        unsafe { fill_user_view() };
    }
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
            debug_assert_ne!(vector, RESTORE_ALL_VECTOR);
            let exception = exception(vector, registers.error_code, registers.rip);
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
        fp_registers: (&raw const x86_64_user_test_fp_registers) as usize - start,
        illegal_instruction: (&raw const x86_64_user_test_illegal_instruction) as usize - start,
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
        linux_move_and_discard: (&raw const x86_64_user_test_linux_move_and_discard) as usize
            - start,
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
