//! User mode, the same way on every architecture (D47).
//!
//! The kernel runs a program in user mode with [`crate::Platform::run_user`]: it loads the
//! program's registers from a [`UserContext`], enters user mode (EL0, U-mode, ring 3) and
//! returns once anything takes the CPU back to the kernel, a system call, an interrupt or an
//! exception, with the program's registers saved in the same [`UserContext`] and the reason as
//! a [`UserExit`]. The kernel's side is plain Rust: a loop that handles each exit and runs the
//! program again. All the architecture's state handling lives in two short HAL routines, one in
//! and one out, and the kernel never sees a trap frame.
//!
//! The system-call registers are Linux's on each architecture, ready for its ABI (M6): the
//! number in `x8` / `a7` / `rax`, up to six arguments in `x0`-`x5` / `a0`-`a5` /
//! `rdi, rsi, rdx, r10, r8, r9`, the result in `x0` / `a0` / `rax`. Every other register is
//! the program's own and comes back unchanged (x86_64's `syscall` itself overwrites `rcx` and
//! `r11`).

use crate::Exception;

/// Why [`crate::Platform::run_user`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserExit {
    /// The program made a system call: [`UserContext::syscall_number`] and
    /// [`UserContext::syscall_arguments`] say which. Running the program again returns from it
    /// with [`UserContext::set_syscall_result`]'s value, after the instruction that made it.
    Syscall,
    /// An interrupt arrived while the program ran. The HAL has already handed it to the
    /// kernel's interrupt handler, as it does in the kernel; running the program again resumes
    /// it where it was. The kernel may switch threads first (preemption on return to user mode).
    Interrupt,
    /// The program caused an exception: a fault, an illegal or privileged instruction, a
    /// breakpoint. Its program counter is the faulting instruction's.
    Exception(Exception),
}

/// A user program's registers while the kernel runs, laid out as the architecture's entry code
/// saves them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct UserContext {
    #[cfg(target_arch = "aarch64")]
    pub(crate) arch: aarch64::Registers,
    #[cfg(target_arch = "riscv64")]
    pub(crate) arch: riscv64::Registers,
    #[cfg(target_arch = "x86_64")]
    pub(crate) arch: x86_64::Registers,
}

impl UserContext {
    /// A program about to run its first instruction at `pc` with its stack pointer at `stack`
    /// and every other register zero.
    pub fn new(pc: u64, stack: u64) -> Self {
        Self {
            arch: Registers::new(pc, stack),
        }
    }

    /// The address of the program's next instruction: after a system call, the one after it;
    /// after an exception, the faulting one.
    pub fn pc(&self) -> u64 {
        self.arch.pc()
    }

    /// The program's stack pointer.
    pub fn stack(&self) -> u64 {
        self.arch.stack()
    }

    /// After [`UserExit::Syscall`]: the system call's number.
    pub fn syscall_number(&self) -> u64 {
        self.arch.syscall_number()
    }

    /// After [`UserExit::Syscall`]: the system call's six argument registers, in order.
    pub fn syscall_arguments(&self) -> [u64; 6] {
        self.arch.syscall_arguments()
    }

    /// Sets the value the system call returns when the program runs again.
    pub fn set_syscall_result(&mut self, value: u64) {
        self.arch.set_syscall_result(value);
    }
}

/// Host builds only: for a test platform that stands in for the CPU in the kernel's host tests.
#[cfg(not(target_os = "none"))]
impl UserContext {
    /// The value the program's last system call returns ([`Self::set_syscall_result`]).
    pub fn syscall_result(&self) -> u64 {
        self.arch.result()
    }

    /// As if the program had run on and made system call `number` with `arguments`, its next
    /// instruction at `pc`.
    pub fn simulate_syscall(&mut self, number: u64, arguments: [u64; 6], pc: u64) {
        self.arch.simulate_syscall(number, arguments, pc);
    }
}

#[cfg(target_arch = "aarch64")]
use aarch64::Registers;
#[cfg(target_arch = "riscv64")]
use riscv64::Registers;
#[cfg(target_arch = "x86_64")]
use x86_64::Registers;

/// A tiny position-independent program in the architecture's machine code, for the kernel's
/// boot self-tests and benchmarks of user mode, run from one page mapped
/// [`crate::paging::Attributes::USER_TEXT`]. It needs no stack and no data. Offsets are from
/// the start of [`TestProgram::code`].
#[derive(Debug, Clone, Copy)]
pub struct TestProgram {
    pub code: &'static [u8],
    /// Puts patterns in every register it may, makes system call 0 with arguments 1 to 6, then
    /// system call 1 with that call's result and, second, 0 if every register but the result
    /// came back unchanged (anything else if not). Then it loads a word from the address system
    /// call 1 returned, which the kernel makes a kernel address, so the program faults there.
    pub checks: usize,
    /// Runs one floating-point instruction, which must trap: user programs get no FP/SIMD
    /// registers until they are saved per thread. If it does not, it goes on as
    /// `null_syscalls`.
    pub floating_point: usize,
    /// Makes system call 2 in a loop.
    pub null_syscalls: usize,
    /// Loops forever without a system call, for an interrupt to stop.
    pub spin: usize,
}

impl TestProgram {
    /// System call numbers the test program makes.
    pub const SYSCALL_FIRST: u64 = 0;
    pub const SYSCALL_REPORT: u64 = 1;
    pub const SYSCALL_NULL: u64 = 2;
}

#[cfg(target_arch = "aarch64")]
pub(crate) mod aarch64 {
    /// What the lower-EL exception entry saves (`exception.rs`), and what the entry to user mode
    /// needs. The entry code stores by offset: keep the layout in step with it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[repr(C)]
    pub(crate) struct Registers {
        /// `x0` to `x30`.
        pub(crate) x: [u64; 31],
        /// `ELR_EL1`: the program counter.
        pub(crate) elr: u64,
        /// `SPSR_EL1`: only the condition flags are the program's; the rest is set on entry.
        pub(crate) spsr: u64,
        /// `SP_EL0`.
        pub(crate) sp: u64,
        /// The kernel's stack pointer while the program runs; the frame above ends here, where
        /// `SP_EL1` points in user mode.
        pub(crate) kernel_sp: u64,
        /// The exception vector entry (0-15) that took the CPU back to the kernel.
        pub(crate) vector: u64,
    }

    /// Bytes the lower-EL entry code stores below `SP_EL1`: `x`, `elr`, `spsr` and `sp`.
    pub(crate) const SAVED_SIZE: usize = 34 * 8;
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_sp) == SAVED_SIZE);
    const _: () = assert!(core::mem::offset_of!(Registers, vector) == SAVED_SIZE + 8);

    impl Registers {
        pub(crate) fn new(pc: u64, stack: u64) -> Self {
            Self {
                x: [0; 31],
                elr: pc,
                spsr: 0,
                sp: stack,
                kernel_sp: 0,
                vector: 0,
            }
        }
        pub(crate) fn pc(&self) -> u64 {
            self.elr
        }
        pub(crate) fn stack(&self) -> u64 {
            self.sp
        }
        pub(crate) fn syscall_number(&self) -> u64 {
            self.x[8]
        }
        pub(crate) fn syscall_arguments(&self) -> [u64; 6] {
            [
                self.x[0], self.x[1], self.x[2], self.x[3], self.x[4], self.x[5],
            ]
        }
        pub(crate) fn set_syscall_result(&mut self, value: u64) {
            self.x[0] = value;
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn result(&self) -> u64 {
            self.x[0]
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn simulate_syscall(&mut self, number: u64, arguments: [u64; 6], pc: u64) {
            self.x[8] = number;
            self.x[..6].copy_from_slice(&arguments);
            self.elr = pc;
        }
    }
}

#[cfg(target_arch = "riscv64")]
pub(crate) mod riscv64 {
    /// What the user-mode trap entry saves (`exception.rs`), and what the entry to user mode
    /// needs. The entry code stores by offset: keep the layout in step with it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[repr(C)]
    pub(crate) struct Registers {
        /// `x0` to `x31` by register number; `x[0]` is unused.
        pub(crate) x: [u64; 32],
        /// `sepc`: the program counter.
        pub(crate) pc: u64,
        /// The kernel's stack pointer while the program runs.
        pub(crate) kernel_sp: u64,
    }

    const _: () = assert!(core::mem::offset_of!(Registers, pc) == 256);
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_sp) == 264);

    impl Registers {
        pub(crate) fn new(pc: u64, stack: u64) -> Self {
            let mut x = [0; 32];
            x[2] = stack;
            Self {
                x,
                pc,
                kernel_sp: 0,
            }
        }
        pub(crate) fn pc(&self) -> u64 {
            self.pc
        }
        pub(crate) fn stack(&self) -> u64 {
            self.x[2]
        }
        pub(crate) fn syscall_number(&self) -> u64 {
            self.x[17]
        }
        pub(crate) fn syscall_arguments(&self) -> [u64; 6] {
            [
                self.x[10], self.x[11], self.x[12], self.x[13], self.x[14], self.x[15],
            ]
        }
        pub(crate) fn set_syscall_result(&mut self, value: u64) {
            self.x[10] = value;
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn result(&self) -> u64 {
            self.x[10]
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn simulate_syscall(&mut self, number: u64, arguments: [u64; 6], pc: u64) {
            self.x[17] = number;
            self.x[10..16].copy_from_slice(&arguments);
            self.pc = pc;
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod x86_64 {
    /// What the ring-3 entry code saves (`exception.rs`): the general-purpose registers in the
    /// order `exception_common` pushes them, the vector and error code the stub pushes, the
    /// frame the CPU pushes (at `TSS.RSP0`, which points at `kernel_rsp`), and the kernel's
    /// stack pointer. The entry code stores by offset: keep the layout in step with it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[repr(C)]
    pub(crate) struct Registers {
        pub(crate) r15: u64,
        pub(crate) r14: u64,
        pub(crate) r13: u64,
        pub(crate) r12: u64,
        pub(crate) r11: u64,
        pub(crate) r10: u64,
        pub(crate) r9: u64,
        pub(crate) r8: u64,
        pub(crate) rbp: u64,
        pub(crate) rdi: u64,
        pub(crate) rsi: u64,
        pub(crate) rdx: u64,
        pub(crate) rcx: u64,
        pub(crate) rbx: u64,
        pub(crate) rax: u64,
        /// The IDT vector that took the CPU back to the kernel, or [`SYSCALL_VECTOR`].
        pub(crate) vector: u64,
        pub(crate) error_code: u64,
        pub(crate) rip: u64,
        pub(crate) cs: u64,
        pub(crate) rflags: u64,
        pub(crate) rsp: u64,
        pub(crate) ss: u64,
        /// The kernel's stack pointer while the program runs; `TSS.RSP0` points here.
        pub(crate) kernel_rsp: u64,
        _pad: u64,
    }

    /// [`Registers::vector`] after a `syscall`: above every IDT vector.
    #[cfg_attr(not(target_os = "none"), allow(dead_code))]
    pub(crate) const SYSCALL_VECTOR: u64 = 0x100;
    /// [`Registers::vector`] for a program that has not run yet: entered with `iretq`.
    pub(crate) const NOT_RUN_VECTOR: u64 = u64::MAX;

    const _: () = assert!(core::mem::offset_of!(Registers, vector) == 15 * 8);
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_rsp) == 22 * 8);
    // `TSS.RSP0` must be 16-byte aligned: the CPU aligns it down before pushing.
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_rsp) % 16 == 0);

    impl Registers {
        pub(crate) fn new(pc: u64, stack: u64) -> Self {
            Self {
                r15: 0,
                r14: 0,
                r13: 0,
                r12: 0,
                r11: 0,
                r10: 0,
                r9: 0,
                r8: 0,
                rbp: 0,
                rdi: 0,
                rsi: 0,
                rdx: 0,
                rcx: 0,
                rbx: 0,
                rax: 0,
                vector: NOT_RUN_VECTOR,
                error_code: 0,
                rip: pc,
                cs: 0,
                rflags: 0,
                rsp: stack,
                ss: 0,
                kernel_rsp: 0,
                _pad: 0,
            }
        }
        pub(crate) fn pc(&self) -> u64 {
            self.rip
        }
        pub(crate) fn stack(&self) -> u64 {
            self.rsp
        }
        pub(crate) fn syscall_number(&self) -> u64 {
            self.rax
        }
        pub(crate) fn syscall_arguments(&self) -> [u64; 6] {
            [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9]
        }
        pub(crate) fn set_syscall_result(&mut self, value: u64) {
            self.rax = value;
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn result(&self) -> u64 {
            self.rax
        }
        #[cfg(not(target_os = "none"))]
        pub(crate) fn simulate_syscall(&mut self, number: u64, arguments: [u64; 6], pc: u64) {
            self.rax = number;
            [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9] = arguments;
            self.rip = pc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_context_starts_at_its_entry_with_its_stack() {
        let mut context = UserContext::new(0x40_0000, 0x7000);
        assert_eq!(context.pc(), 0x40_0000);
        assert_eq!(context.stack(), 0x7000);
        assert_eq!(context.syscall_arguments(), [0; 6]);
        assert_eq!(context.syscall_number(), 0);
        context.set_syscall_result(42);
        // The result goes where the ABI returns it: the first argument's register on AArch64
        // and RISC-V, the number's on x86_64.
        if cfg!(target_arch = "x86_64") {
            assert_eq!(context.syscall_number(), 42);
        } else {
            assert_eq!(context.syscall_arguments(), [42, 0, 0, 0, 0, 0]);
        }
    }
}
