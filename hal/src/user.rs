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
    /// Host builds: the exception a test platform's `run_user` last returned
    /// ([`Self::simulate_exception`]), which [`Self::last_exception`] reports.
    #[cfg(not(target_os = "none"))]
    simulated_exception: Option<Exception>,
}

impl UserContext {
    /// A program about to run its first instruction at `pc` with its stack pointer at `stack`
    /// and every other register zero.
    pub fn new(pc: u64, stack: u64) -> Self {
        Self {
            arch: Registers::new(pc, stack),
            #[cfg(not(target_os = "none"))]
            simulated_exception: None,
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

    /// The program's thread pointer: where its thread-local storage is, in the register the
    /// architecture's ABI keeps it in (`TPIDR_EL0`, `tp`, the FS base). Zero for a new program,
    /// as Linux starts one.
    pub fn thread_pointer(&self) -> u64 {
        self.arch.thread_pointer()
    }

    /// Sets the program's thread pointer, as x86_64's `arch_prctl(ARCH_SET_FS)` and `clone`'s
    /// `CLONE_SETTLS` do (on AArch64 and RISC-V the program can also set it itself). It takes
    /// effect when the program runs again. On x86_64 it must be below the end of user space
    /// ([`crate::paging::Format::USER_END`]): above, the FS base would be the kernel's half or
    /// non-canonical, and loading it would fault in the kernel; such a value is refused and
    /// the thread pointer left as it was. Any value is accepted elsewhere, as the program
    /// itself could set it.
    pub fn set_thread_pointer(&mut self, value: u64) -> Result<(), InvalidThreadPointer> {
        self.arch.set_thread_pointer(value)
    }

    /// Every register of the program's that user mode can change, but the thread pointer where
    /// it is a system register (`TPIDR_EL0`, the FS base): what a signal frame saves (M6).
    pub fn registers(&self) -> UserRegisters {
        self.arch.registers()
    }

    /// After [`UserExit::Exception`]: that exception, as `run_user` returned it, decoded again
    /// from what its way out of user mode kept in the context (the cause and the faulting
    /// address, stored only for an exception, so a system call's exit pays nothing). The
    /// program counter is the context's: read it before changing the registers. Only the next
    /// `run_user` with this context replaces it, so it holds across preemption, a move to
    /// another CPU and the kernel's own exceptions, unlike the CPU's cause registers.
    pub fn last_exception(&self) -> Exception {
        #[cfg(target_os = "none")]
        {
            self.arch.last_exception()
        }
        #[cfg(not(target_os = "none"))]
        {
            self.simulated_exception
                .expect("the test platform's run_user returned no exception")
        }
    }

    /// Sets every register [`Self::registers`] returns, as returning from a signal handler
    /// does: the program runs on with exactly these, the ones a system call's return would
    /// otherwise overwrite (x86_64's `rcx` and `r11`) included. Only the flags a program may
    /// set are kept (the rest are the kernel's to choose when it enters user mode).
    pub fn set_registers(&mut self, registers: &UserRegisters) {
        self.arch.set_registers(registers);
    }
}

/// [`UserContext::set_thread_pointer`]'s value is not a user address, where the architecture
/// needs one (x86_64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidThreadPointer;

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

    /// As if the program had run on and raised `exception`: what the test platform's
    /// `run_user` returns for it, kept for [`Self::last_exception`] as the CPU's way out keeps
    /// it.
    pub fn simulate_exception(&mut self, exception: Exception) -> UserExit {
        self.simulated_exception = Some(exception);
        UserExit::Exception(exception)
    }
}

#[cfg(target_arch = "aarch64")]
use aarch64::Registers;
#[cfg(target_arch = "aarch64")]
pub use aarch64::UserRegisters;
#[cfg(target_arch = "riscv64")]
use riscv64::Registers;
#[cfg(target_arch = "riscv64")]
pub use riscv64::UserRegisters;
#[cfg(target_arch = "x86_64")]
use x86_64::Registers;
#[cfg(target_arch = "x86_64")]
pub use x86_64::UserRegisters;

/// A tiny position-independent program in the architecture's machine code, for the kernel's
/// boot self-tests and benchmarks of user mode, run from one page mapped
/// [`crate::paging::Attributes::USER_TEXT`]. It needs no data, and no stack but the one
/// [`Self::linux_start`] reads. Offsets are from
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
    /// A Linux program: writes [`Self::LINUX_HELLO`] to standard output, writes one byte from
    /// address 0 (`EFAULT`), makes system call [`Self::LINUX_UNKNOWN_SYSCALL`] (`ENOSYS`), then
    /// calls `exit_group(0)` if each returned what Linux returns, `exit_group(1)` if not. In
    /// Linux's numbers for the architecture.
    pub linux_hello: usize,
    /// A Linux program started as Linux starts one (D51): its stack pointer at `argc`, then
    /// the `argv` and `envp` pointer arrays, each ended by a null pointer. Checks that
    /// `gettid` returns what `getpid` does, that `getrandom` fills the 16 bytes below its
    /// stack pointer (not with zeros), that `prlimit64` reports the stack's limits as 8 MiB
    /// soft and unlimited hard into the 16 below those, that `rt_sigaction` reads `SIGPIPE`'s
    /// action back as `SIG_IGN` once set, and that `sigaltstack` reports no alternate stack
    /// (each through the 128 bytes below the stack pointer), then writes each argument string,
    /// then each environment string, to standard output with `writev` from an iovec on its
    /// stack, then calls `exit_group(argc)`; `exit_group(255)` if the stack pointer is not
    /// 16-byte aligned, the ids differ, or any of those calls failed.
    pub linux_start: usize,
    /// Where [`Self::linux_start`]'s code ends: the offset of the first byte after it. Its
    /// ELF image for `linux_exec_load` holds the code only up to here, so programs added after
    /// it do not grow what the benchmark copies (one added before it still would).
    pub linux_start_end: usize,
    /// Makes Linux system call [`Self::LINUX_UNKNOWN_SYSCALL`] in a loop: `ENOSYS` each time,
    /// the Linux layer's cheapest dispatch, for its benchmark.
    pub linux_unknown_syscalls: usize,
    /// Walks down its stack from the stack pointer's page, a page at a time, storing each
    /// page's address in the page's first word, until a store faults and the kernel ends it:
    /// every page of the stack is touched once, for demand paging's self-test and benchmark
    /// (D52).
    pub stack_growth: usize,
    /// A Linux program whose thread pointer must be zero when it starts, then hold what it
    /// sets it to, `getpid() << 12`, across [`Self::LINUX_THREAD_POINTER_ROUNDS`] calls to
    /// `sched_yield`, while other programs run on the same CPU with their own (or it moves to
    /// another CPU). It sets it as the architecture's C library does: `msr tpidr_el0`, `mv tp`,
    /// `arch_prctl(ARCH_SET_FS)`. Then `exit_group(0)`, or `exit_group(1)` if any check or
    /// call failed.
    pub linux_thread_pointer: usize,
    /// A Linux program that polls descriptors 0 to 2 with no wait, as Rust's std does at
    /// startup but asking for room to write on 1 (only 1 may report, with `POLLOUT`), reads
    /// the console's window size with `ioctl(TIOCGWINSZ)`, then waits on no descriptors for
    /// [`Self::LINUX_POLL_WAIT_NS`] with `ppoll`, which must return 0 and write a zero
    /// remaining time back. x86_64 polls first with `poll`, the others with `ppoll`. Then
    /// `exit_group(0)`, or `exit_group(1)` if any check or call failed.
    pub linux_poll: usize,
    /// A Linux program that sends itself signals (D63): `kill(0, SIGCHLD)`, which is ignored,
    /// then blocks `SIGUSR1`, sends it with `tgkill` to its own id, checks that
    /// `rt_sigpending` reports it, and unblocks it, which must end the program with
    /// `SIGUSR1` before `rt_sigprocmask` returns. `exit_group(1)` if any call failed or
    /// returned.
    pub linux_signals: usize,
    /// A Linux program whose signal handler runs on an alternate stack (D64): it sets one up
    /// with `sigaltstack`, installs a `SIGUSR1` handler with `SA_SIGINFO` and `SA_ONSTACK`
    /// (x86_64 with its own restorer, the others returning through the kernel's trampoline),
    /// and sends itself `SIGUSR1` with `tgkill`. The handler checks its arguments (the signal,
    /// `si_signo`, `si_code` `SI_TKILL`) and that its stack is the alternate stack its
    /// `ucontext` names, then sets one register in its frame (`x20`, `s2`, `r12`) to 2 and
    /// clobbers another (`x19`, `s1`, `rbx`), and returns. `tgkill` must then return 0 with the
    /// clobbered register restored, the changed one 2, and nothing blocked:
    /// `exit_group(0)`, or `exit_group(1)` if any check or call failed.
    pub linux_signal_handler: usize,
    /// A Linux program whose faults go to its `SIGSEGV` handler (D65): it installs one with
    /// `SA_SIGINFO` (x86_64 with its own restorer, the others returning through the kernel's
    /// trampoline) and maps a `PROT_NONE` page with `mmap`, then reads
    /// [`Self::LINUX_FAULT_ADDRESS`], where nothing is mapped, and the `PROT_NONE` page. Before
    /// each read it sets three registers to the `si_addr` and `si_code` (`SEGV_MAPERR`, then
    /// `SEGV_ACCERR`) the handler must see and to where the handler resumes it (`x21` to `x23`,
    /// `s3` to `s5`, `r13` to `r15`). The handler checks the signal and its `siginfo_t`
    /// against those registers in its frame, and the fault address its machine context reports
    /// (AArch64's `fault_address` and `esr_context` record, x86_64's `cr2` and `trapno` 14),
    /// counts each fault that passed in a fourth register in its frame (`x20`, `s2`, `r12`),
    /// sets the frame's program counter to the resume point, and returns. The program then
    /// requires the count to be 2: `exit_group(0)`, or `exit_group(1)` if any check or call
    /// failed.
    pub linux_fault_handler: usize,
}

impl TestProgram {
    /// System call numbers the test program makes.
    pub const SYSCALL_FIRST: u64 = 0;
    pub const SYSCALL_REPORT: u64 = 1;
    pub const SYSCALL_NULL: u64 = 2;
    /// What [`Self::linux_hello`] writes. Each architecture's program spells it out; the boot
    /// self-test checks that it is this, and the QEMU runner that it reached the console.
    pub const LINUX_HELLO: &str = "EGGOS-USER: hello from user mode\n";
    /// A number Linux has no system call for, on any architecture.
    pub const LINUX_UNKNOWN_SYSCALL: u64 = 1000;
    /// How many times [`Self::linux_thread_pointer`] yields and checks its thread pointer.
    pub const LINUX_THREAD_POINTER_ROUNDS: u64 = 20;
    /// x86_64: the word [`Self::linux_thread_pointer`] reads through FS ("EGGOSTLS").
    pub const LINUX_THREAD_POINTER_MARKER: u64 = u64::from_le_bytes(*b"EGGOSTLS");
    /// x86_64: `arch_prctl`'s code for setting the FS base.
    pub const LINUX_ARCH_SET_FS: u64 = 0x1002;
    /// How long [`Self::linux_poll`] waits in `ppoll`, in nanoseconds: 2 ms, a few timer
    /// ticks on every architecture.
    pub const LINUX_POLL_WAIT_NS: u64 = 2_000_000;
    /// Where [`Self::linux_fault_handler`] first faults: an address in no mapping (below
    /// Linux's lowest `mmap` address), not page-aligned.
    pub const LINUX_FAULT_ADDRESS: u64 = 0x1230;
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
        /// `TPIDR_EL0`: the thread pointer, which the program may also write itself, so it is
        /// saved on every way out of EL0 and loaded on every way in.
        pub(crate) tpidr: u64,
        _pad: u64,
        /// The kernel's stack pointer while the program runs; the frame above ends here, where
        /// `SP_EL1` points in user mode.
        pub(crate) kernel_sp: u64,
        /// The exception vector entry (0-15) that took the CPU back to the kernel.
        pub(crate) vector: u64,
        /// After an exception, `ESR_EL1` and `FAR_EL1`, kept by the way out of `run_user` for
        /// [`super::UserContext::last_exception`].
        pub(crate) esr: u64,
        pub(crate) far: u64,
    }

    /// Bytes the lower-EL entry code stores below `SP_EL1`: `x`, `elr`, `spsr`, `sp` and
    /// `tpidr`, and a word that keeps `SP_EL1` 16-byte aligned.
    pub(crate) const SAVED_SIZE: usize = 36 * 8;
    /// [`super::UserContext::registers`] on AArch64.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct UserRegisters {
        /// `x0` to `x30`.
        pub x: [u64; 31],
        pub sp: u64,
        pub pc: u64,
        /// `PSTATE` as `SPSR_EL1` holds it: only its condition flags (N, Z, C, V) are the
        /// program's.
        pub pstate: u64,
    }

    const _: () = assert!(core::mem::offset_of!(Registers, elr) == 248);
    const _: () = assert!(core::mem::offset_of!(Registers, tpidr) == 272);
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_sp) == SAVED_SIZE);
    const _: () = assert!(core::mem::offset_of!(Registers, vector) == SAVED_SIZE + 8);

    impl Registers {
        pub(crate) fn new(pc: u64, stack: u64) -> Self {
            Self {
                x: [0; 31],
                elr: pc,
                spsr: 0,
                sp: stack,
                tpidr: 0,
                _pad: 0,
                kernel_sp: 0,
                vector: 0,
                esr: 0,
                far: 0,
            }
        }
        pub(crate) fn thread_pointer(&self) -> u64 {
            self.tpidr
        }
        pub(crate) fn set_thread_pointer(
            &mut self,
            value: u64,
        ) -> Result<(), super::InvalidThreadPointer> {
            self.tpidr = value;
            Ok(())
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
        pub(crate) fn registers(&self) -> UserRegisters {
            UserRegisters {
                x: self.x,
                sp: self.sp,
                pc: self.elr,
                pstate: self.spsr,
            }
        }
        /// `run_user` keeps only `spsr`'s condition flags.
        pub(crate) fn set_registers(&mut self, registers: &UserRegisters) {
            self.x = registers.x;
            self.sp = registers.sp;
            self.elr = registers.pc;
            self.spsr = registers.pstate;
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
        /// After an exception, `scause` and `stval`, kept by the way out of `run_user` for
        /// [`super::UserContext::last_exception`].
        pub(crate) scause: u64,
        pub(crate) stval: u64,
    }

    /// [`super::UserContext::registers`] on RISC-V.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct UserRegisters {
        pub pc: u64,
        /// `x0` to `x31` by register number; `x[0]` is always zero.
        pub x: [u64; 32],
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
                scause: 0,
                stval: 0,
            }
        }
        /// `tp` (`x4`), an ordinary register for the trap entry: saved and loaded with the rest.
        pub(crate) fn thread_pointer(&self) -> u64 {
            self.x[4]
        }
        pub(crate) fn set_thread_pointer(
            &mut self,
            value: u64,
        ) -> Result<(), super::InvalidThreadPointer> {
            self.x[4] = value;
            Ok(())
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
        pub(crate) fn registers(&self) -> UserRegisters {
            let mut x = self.x;
            x[0] = 0;
            UserRegisters { pc: self.pc, x }
        }
        pub(crate) fn set_registers(&mut self, registers: &UserRegisters) {
            self.x = registers.x;
            self.x[0] = 0;
            self.pc = registers.pc;
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
        /// The FS base: the thread pointer. Programs cannot write it themselves (`CR4.FSGSBASE`
        /// is clear), so it is never saved; the way in loads it with `wrmsr` only when this
        /// CPU's FS base belongs to another value of `fs_base_id`.
        pub(crate) fs_base: u64,
        /// Names `fs_base`'s value: 0 for a context whose FS base was never set (0), and a
        /// new number, from [`NEXT_FS_BASE_ID`], each time it is set. The way in compares it
        /// with the one this CPU's FS base was last loaded for (`X86Tables::fs_base_id`).
        pub(crate) fs_base_id: u64,
        /// After a page fault, the faulting address (`CR2`), kept by the way out of `run_user`
        /// for [`super::UserContext::last_exception`] (with `vector` and `error_code`).
        pub(crate) fault_address: u64,
    }

    /// The next [`Registers::fs_base_id`]: never 0, never reused (2^64 settings).
    static NEXT_FS_BASE_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

    /// [`Registers::vector`] after a `syscall`: above every IDT vector.
    #[cfg_attr(not(target_os = "none"), allow(dead_code))]
    pub(crate) const SYSCALL_VECTOR: u64 = 0x100;
    /// [`Registers::vector`] for a context entered with `iretq`, which loads every register:
    /// a program that has not run yet, or one whose registers were all set
    /// ([`Registers::set_registers`]).
    pub(crate) const RESTORE_ALL_VECTOR: u64 = u64::MAX;

    /// [`super::UserContext::registers`] on x86_64.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct UserRegisters {
        pub r8: u64,
        pub r9: u64,
        pub r10: u64,
        pub r11: u64,
        pub r12: u64,
        pub r13: u64,
        pub r14: u64,
        pub r15: u64,
        pub rdi: u64,
        pub rsi: u64,
        pub rbp: u64,
        pub rbx: u64,
        pub rdx: u64,
        pub rax: u64,
        pub rcx: u64,
        pub rsp: u64,
        pub rip: u64,
        /// Only the flags a program may set are kept.
        pub rflags: u64,
        /// The code and stack segment selectors the program runs with: reported, never set.
        pub cs: u16,
        pub ss: u16,
    }

    const _: () = assert!(core::mem::offset_of!(Registers, vector) == 15 * 8);
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_rsp) == 22 * 8);
    // `TSS.RSP0` must be 16-byte aligned: the CPU aligns it down before pushing.
    const _: () = assert!(core::mem::offset_of!(Registers, kernel_rsp) % 16 == 0);
    const _: () = assert!(core::mem::offset_of!(Registers, fs_base) == 23 * 8);
    const _: () = assert!(core::mem::offset_of!(Registers, fs_base_id) == 24 * 8);

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
                vector: RESTORE_ALL_VECTOR,
                error_code: 0,
                rip: pc,
                cs: 0,
                rflags: 0,
                rsp: stack,
                ss: 0,
                kernel_rsp: 0,
                fs_base: 0,
                fs_base_id: 0,
                fault_address: 0,
            }
        }
        pub(crate) fn thread_pointer(&self) -> u64 {
            self.fs_base
        }
        pub(crate) fn set_thread_pointer(
            &mut self,
            value: u64,
        ) -> Result<(), super::InvalidThreadPointer> {
            use crate::paging::Format;
            if value >= crate::paging::x86_64::X86_64::USER_END {
                return Err(super::InvalidThreadPointer);
            }
            self.fs_base = value;
            self.fs_base_id = NEXT_FS_BASE_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
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
        pub(crate) fn registers(&self) -> UserRegisters {
            UserRegisters {
                r8: self.r8,
                r9: self.r9,
                r10: self.r10,
                r11: self.r11,
                r12: self.r12,
                r13: self.r13,
                r14: self.r14,
                r15: self.r15,
                rdi: self.rdi,
                rsi: self.rsi,
                rbp: self.rbp,
                rbx: self.rbx,
                rdx: self.rdx,
                rax: self.rax,
                rcx: self.rcx,
                rsp: self.rsp,
                rip: self.rip,
                rflags: self.rflags,
                cs: self.cs as u16,
                ss: self.ss as u16,
            }
        }
        /// Entered with `iretq` next, which loads `rcx` and `r11` too (`sysretq` would take
        /// them for `rip` and `rflags`); `run_user` keeps only the flags a program may set
        /// and refuses a `rip` outside user space.
        pub(crate) fn set_registers(&mut self, registers: &UserRegisters) {
            self.r8 = registers.r8;
            self.r9 = registers.r9;
            self.r10 = registers.r10;
            self.r11 = registers.r11;
            self.r12 = registers.r12;
            self.r13 = registers.r13;
            self.r14 = registers.r14;
            self.r15 = registers.r15;
            self.rdi = registers.rdi;
            self.rsi = registers.rsi;
            self.rbp = registers.rbp;
            self.rbx = registers.rbx;
            self.rdx = registers.rdx;
            self.rax = registers.rax;
            self.rcx = registers.rcx;
            self.rsp = registers.rsp;
            self.rip = registers.rip;
            self.rflags = registers.rflags;
            self.vector = RESTORE_ALL_VECTOR;
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
    fn the_thread_pointer_starts_at_zero_and_takes_user_addresses() {
        let mut context = UserContext::new(0x40_0000, 0x7000);
        assert_eq!(context.thread_pointer(), 0);
        assert_eq!(context.set_thread_pointer(0x7000_1000), Ok(()));
        assert_eq!(context.thread_pointer(), 0x7000_1000);
        // Only x86_64's FS base must stay a user address; elsewhere the program could set
        // anything itself.
        let kernel = 0xffff_8000_0000_0000;
        if cfg!(target_arch = "x86_64") {
            assert_eq!(
                context.set_thread_pointer(kernel),
                Err(InvalidThreadPointer)
            );
            assert_eq!(context.thread_pointer(), 0x7000_1000);
        } else {
            assert_eq!(context.set_thread_pointer(kernel), Ok(()));
            assert_eq!(context.thread_pointer(), kernel);
        }
    }

    #[test]
    fn the_last_exception_is_the_one_the_exit_returned() {
        let mut context = UserContext::new(0x40_0000, 0x7000);
        let exception = Exception {
            kind: crate::ExceptionKind::DataFault,
            name: "test page fault",
            code: 2,
            pc: 0x40_0000,
            address: Some(0x1230),
        };
        assert_eq!(
            context.simulate_exception(exception),
            UserExit::Exception(exception)
        );
        // A system call's exit leaves it as it was.
        context.simulate_syscall(0, [0; 6], 0x40_0004);
        assert_eq!(context.last_exception(), exception);
    }

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

    #[test]
    fn every_register_is_read_and_set_at_once() {
        let mut context = UserContext::new(0x40_0000, 0x7000);
        context.set_syscall_result(7);
        let mut registers = context.registers();
        assert_eq!((context.pc(), context.stack()), (0x40_0000, 0x7000));
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(
                (registers.pc, registers.sp, registers.x[0]),
                (0x40_0000, 0x7000, 7)
            );
            registers.x[30] = 0x40_1000;
            registers.pc = 0x40_2000;
            registers.sp = 0x6000;
        }
        #[cfg(target_arch = "riscv64")]
        {
            assert_eq!(
                (registers.pc, registers.x[2], registers.x[10]),
                (0x40_0000, 0x7000, 7)
            );
            registers.x[0] = 1;
            registers.x[1] = 0x40_1000;
            registers.pc = 0x40_2000;
            registers.x[2] = 0x6000;
        }
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(
                (registers.rip, registers.rsp, registers.rax),
                (0x40_0000, 0x7000, 7)
            );
            assert_eq!(context.arch.vector, x86_64::RESTORE_ALL_VECTOR);
            // After a system call the way back would load `rcx` and `r11` from `rip` and
            // `rflags`; setting every register makes it load them too.
            context.arch.vector = x86_64::SYSCALL_VECTOR;
            registers.rcx = 0x40_1000;
            registers.rip = 0x40_2000;
            registers.rsp = 0x6000;
        }
        context.set_registers(&registers);
        assert_eq!((context.pc(), context.stack()), (0x40_2000, 0x6000));
        #[cfg(target_arch = "riscv64")]
        {
            // `x0` is always zero.
            registers.x[0] = 0;
        }
        assert_eq!(context.registers(), registers);
        #[cfg(target_arch = "x86_64")]
        assert_eq!(context.arch.vector, x86_64::RESTORE_ALL_VECTOR);
    }
}
