//! Starting a process the way `posix_spawn` does: `clone` with the child on a stack of its
//! own, and the return path a signal handler needs on x86_64.

use crate::syscall::number;
use core::arch::asm;

/// The `clone` flag that shares the caller's memory with the child.
pub const CLONE_VM: usize = 0x100;

/// The `clone` flag that has the caller sleep until the child runs `execve` or ends.
pub const CLONE_VFORK: usize = 0x4000;

/// `clone(flags, stack_top)`: the child starts on `stack_top` and calls `child(argument)`,
/// which never returns; the caller gets the child's id, or a negative errno.
///
/// # Safety
///
/// `stack_top` is the 16-byte aligned end of memory the child may use as its stack for as long
/// as it runs, which nothing else uses meanwhile; with [`CLONE_VM`] the child shares the
/// caller's memory, so `child` must touch only what the caller lets it.
#[inline(always)]
pub unsafe fn clone_onto_stack(
    flags: usize,
    stack_top: usize,
    child: extern "C" fn(usize) -> !,
    argument: usize,
) -> isize {
    let result: isize;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: Linux's x86_64 `clone`: the caller's own registers and memory are as before but
    // `rax`, `rcx` and `r11`; the child starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "syscall",
            "test rax, rax",
            "jnz 2f",
            "mov rdi, r13",
            "call r12",
            "ud2",
            "2:",
            inlateout("rax") number::CLONE as isize => result,
            in("rdi") flags, in("rsi") stack_top, in("rdx") 0usize, in("r10") 0usize,
            in("r8") 0usize, in("r12") child, in("r13") argument,
            lateout("rcx") _, lateout("r11") _,
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: Linux's AArch64 `clone`: the caller's registers are as before but `x0`; the
    // child starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "svc 0",
            "cbnz x0, 2f",
            "mov x0, x10",
            "blr x9",
            "brk #0",
            "2:",
            in("x8") number::CLONE,
            inlateout("x0") flags as isize => result,
            in("x1") stack_top, in("x2") 0usize, in("x3") 0usize, in("x4") 0usize,
            in("x9") child, in("x10") argument,
        );
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: Linux's RISC-V `clone`: the caller's registers are as before but `a0`; the child
    // starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "ecall",
            "bnez a0, 2f",
            "mv a0, t2",
            "jalr t1",
            "unimp",
            "2:",
            in("a7") number::CLONE,
            inlateout("a0") flags as isize => result,
            in("a1") stack_top, in("a2") 0usize, in("a3") 0usize, in("a4") 0usize,
            in("t1") child, in("t2") argument,
        );
    }
    result
}

/// Where a signal handler returns on x86_64, given as `sa_restorer` with `SA_RESTORER`: it
/// calls `rt_sigreturn`. AArch64 and RISC-V handlers return through the kernel's own
/// trampoline.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub extern "C" fn restore_after_signal() -> ! {
    core::arch::naked_asm!(
        "mov eax, {number}",
        "syscall",
        "ud2",
        number = const number::RT_SIGRETURN,
    )
}
