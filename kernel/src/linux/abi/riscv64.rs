//! RISC-V's (RV64) Linux ABI (D218), on the generic table and structs: see [`super`].

/// The calls RISC-V keeps beyond the generic table (Linux's `__ARCH_WANT_*`): none.
pub mod wants {
    pub const LEGACY_CALLS: bool = false;
    pub const RENAMEAT: bool = false;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on RISC-V: the generic table.
pub mod number {
    pub use crate::linux::abi::generic::number::*;
}

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "riscv64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::RISCV;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// Whether `clone`'s fourth argument is `child_tid` and its fifth `tls` (x86_64's order), not
/// the other way round.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// Whether `struct sigaction` has `sa_restorer`: not on RISC-V, whose handlers return through
/// the vDSO, here the kernel's trampoline.
pub const SIGACTION_HAS_RESTORER: bool = false;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 2048;

const NOP: u32 = 0x0000_0013;
const ADDI_A7_ZERO_RT_SIGRETURN: u32 = (number::RT_SIGRETURN as u32) << 20 | 17 << 7 | 0x13;
const ECALL: u32 = 0x0000_0073;
/// The kernel's `rt_sigreturn` trampoline: `nop`, `li a7, 139` and `ecall`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([NOP, ADDI_A7_ZERO_RT_SIGRETURN, ECALL]));

/// Linux's `stat` here.
pub mod stat {
    use crate::linux::abi::StatLayout;

    /// The size of `struct stat`.
    pub const SIZE: usize = 128;
    /// Its fields' layout.
    pub const LAYOUT: StatLayout = StatLayout::Generic;
}

/// The `open` flags whose values differ between architectures.
pub mod open {
    /// `O_DIRECTORY`.
    pub const DIRECTORY: u32 = 0o200_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o400_000;
}

/// The `mmap` bits only some architectures have.
pub mod mmap {
    /// The architecture's own `prot` bits: none.
    pub const PROT_ARCH: u64 = 0;
    /// `MAP_32BIT`: somewhere in the second gigabyte, none here.
    pub const MAP_32BIT: u64 = 0;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `uc_mcontext` is `struct sigcontext { struct user_regs_struct sc_regs; union
/// __riscv_fp_state sc_fpregs; }`, the registers `pc` then `x1` to `x31`, the FP part 528
/// bytes: the D extension's `f0` to `f31` and `fcsr`, and at its end the header of the
/// extensions that follow, here none (a reserved word and an end header, all zero).
pub mod signal_frame {
    use crate::linux::abi::generic::signal_frame::{MCONTEXT, SIGMASK, UCONTEXT};
    use crate::linux::fault::Trap;
    use crate::linux::signal_frame::{
        BadFrame, Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::fp::FpState;
    use eggos_hal::user::UserRegisters;

    /// The whole frame: the `ucontext_t` is 960 bytes.
    pub const SIZE: u64 = (UCONTEXT + 960) as u64;
    const _: () = assert!(SIZE == 1088);
    /// What the kernel writes and reads: all of it.
    pub const WRITTEN: usize = SIZE as usize;
    pub const READ: usize = WRITTEN;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;
    /// The extensions' header at the end of `sc_fpregs`: a reserved word, then the first
    /// extension's magic number and size, 0 for none.
    pub const EXTENSIONS: usize = WRITTEN - 12;
    const _: () = assert!(EXTENSIONS == 1076);
    /// `struct __riscv_d_ext_state`: `f0` to `f31` (64 bits each), then `fcsr` (32 bits).
    pub type FpPart = [u8; 260];
    pub const FP_PART: usize = MCONTEXT + 256;
    pub const FCSR: usize = 256;
    const _: () = assert!(FP_PART == 560);

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; 260];
        for (index, f) in state.f().iter().enumerate() {
            part[index * 8..][..8].copy_from_slice(&f.to_ne_bytes());
        }
        part[FCSR..].copy_from_slice(&state.fcsr().to_ne_bytes());
        part
    }

    /// Every value as it is, as on Linux: `fcsr`'s reserved bits ignore writes.
    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for (index, f) in state.f_mut().iter_mut().enumerate() {
            *f = word(part, index * 8);
        }
        state.set_fcsr(u32::from_ne_bytes(part[FCSR..].try_into().unwrap()));
        Ok(())
    }

    /// 16-byte aligned below `top`.
    pub const fn place(top: u64) -> u64 {
        top.wrapping_sub(SIZE) & !15
    }

    /// No exception is reported beyond the `siginfo_t`: `trap` is unused.
    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        _trap: Option<&Trap>,
        _restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        frame[..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, SIGMASK, saved.blocked);
        put(&mut frame, MCONTEXT, saved.registers.pc);
        for index in 1..32 {
            put(&mut frame, MCONTEXT + index * 8, saved.registers.x[index]);
        }
        frame[FP_PART..][..saved.fp.len()].copy_from_slice(&saved.fp);
        (place(top), frame)
    }

    /// Nothing outside the frame.
    pub fn write_extra<P: Platform>(
        _: &Process<P>,
        _: u64,
        _: u64,
        _: &Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(
        _: &Process<P>,
        _: &[u8; READ],
        _: &mut Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.x[10] = u64::from(handler.signal);
        registers.x[11] = frame;
        registers.x[12] = frame + UCONTEXT as u64;
        registers.x[1] = handler.restorer;
        registers.x[2] = frame;
        registers.pc = handler.entry;
    }

    /// At the stack pointer.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.x[2])
    }

    /// `None` unless the extensions' header says there are none, as the kernel writes it.
    pub fn parse(frame: &[u8; READ], _current: &UserRegisters) -> Option<Saved> {
        if frame[EXTENSIONS..] != [0; 12] {
            return None;
        }
        let mut x = [0; 32];
        for (index, value) in x.iter_mut().enumerate().skip(1) {
            *value = word(frame, MCONTEXT + index * 8);
        }
        Some(Saved {
            registers: UserRegisters {
                pc: word(frame, MCONTEXT),
                x,
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: frame[FP_PART..][..260].try_into().unwrap(),
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.x[10]
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
        /// `f0`.
        pub const FP: usize = super::FP_PART;
    }

    /// The most stack a frame takes below an alternate stack's top, worked out by hand.
    #[cfg(test)]
    pub const LARGEST_FRAME_SPACE_EXPECTED: u64 = 1088 + 15;

    /// FP/SIMD registers holding values a new thread's do not.
    #[cfg(test)]
    pub fn state_with_values() -> FpState {
        let mut state = FpState::new();
        state.f_mut()[8] = 0x0123_4567_89ab_cdef;
        state.f_mut()[31] = u64::MAX;
        state.set_fcsr(2 << 5 | 1);
        state
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::linux::signal_frame::initial_fp_part;

        #[test]
        fn the_initial_part_is_all_zero() {
            assert!(initial_fp_part().iter().all(|&byte| byte == 0));
        }

        #[test]
        fn riscv_s_part_is_the_f_registers_then_fcsr() {
            let part = fp_part(&state_with_values());
            assert_eq!(part[64..72], 0x0123_4567_89ab_cdefu64.to_ne_bytes());
            assert_eq!(part[256..], (2u32 << 5 | 1).to_ne_bytes());
        }
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// The signal, `si_code` and `si_addr` Linux gives `exception`, from `stval` for the faults
    /// that set it: page faults and access faults alike by the process's regions; misaligned
    /// accesses, which the kernel does not emulate, `SIGBUS`; `ebreak` `SIGTRAP` at its PC,
    /// which stays on it, so a handler that returns runs it again; illegal instructions and
    /// every other cause from U-mode `SIGILL` (the HAL reports no arithmetic, debug or segment
    /// exceptions on RISC-V). The frame reports nothing more.
    pub fn classify(
        exception: &Exception,
        _stack: u64,
        _in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        let address = exception.address.unwrap_or(0);
        let reported = match exception.kind {
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => segv(address),
            ExceptionKind::Alignment => (signal::SIGBUS, code::BUS_ADRALN, address),
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Arithmetic
            | ExceptionKind::Debug
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        fault(reported, Trap::default())
    }
}

/// What the kernel's host tests need to know of this architecture's registers and frames.
#[cfg(test)]
pub mod test_support {
    use super::signal_frame::layout;
    use crate::linux::signal;
    use eggos_hal::ExceptionKind;
    use eggos_hal::user::UserRegisters;

    pub const SYSCALL_INSTRUCTION_SIZE: u64 = 4;
    /// `st_mode`, `st_uid`, `st_gid` and `st_rdev` in `struct stat`.
    pub const STAT_FIELDS: [usize; 4] = [16, 24, 28, 32];

    /// The test platform's exception code: the cause in `scause`.
    pub fn exception_code(class: u64, status: u64) -> u64 {
        class | status
    }

    /// Exceptions with no handler: kind, cause, status and the signal that ends the process.
    pub const FAULTS_WITHOUT_HANDLERS: &[(ExceptionKind, u64, u64, u32)] = &[
        (ExceptionKind::Alignment, 0, 0, signal::SIGBUS),
        (ExceptionKind::InstructionFault, 1, 0, signal::SIGSEGV),
        (ExceptionKind::IllegalInstruction, 2, 0, signal::SIGILL),
        (ExceptionKind::Breakpoint, 3, 0, signal::SIGTRAP),
        (ExceptionKind::DataFault, 13, 0, signal::SIGSEGV),
        (ExceptionKind::Other, 9, 0, signal::SIGILL),
    ];

    /// Where a handler returns to and its stack pointer then: `ra` and `sp`.
    pub fn return_point(registers: &UserRegisters, _read_word: impl Fn(u64) -> u64) -> (u64, u64) {
        (registers.x[1], registers.x[2])
    }

    pub fn set_returned_stack(_registers: &mut UserRegisters, _stack: u64) {}

    /// A callee-saved register.
    pub fn callee_saved(registers: &mut UserRegisters) -> &mut u64 {
        &mut registers.x[9]
    }

    /// Whether a handler's frame at `frame` is 16-byte aligned below `stack`.
    pub fn frame_is_placed(frame: u64, stack: u64) -> bool {
        frame.is_multiple_of(16) && frame < stack
    }

    /// The FP/SIMD part's address: in the frame.
    pub fn fp_part_address(frame: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        frame + layout::FP as u64
    }

    /// Nothing more to check of a new frame's FP/SIMD part.
    pub fn check_initial_fp_part(_fp: u64, _part: &[u8], _read: impl Fn(u64, &mut [u8])) {}

    /// Where the frame points at its FP/SIMD image, if it does.
    pub const FPSTATE_POINTER: Option<usize> = None;

    /// A store page fault.
    pub const PAGE_FAULT_CLASS: u64 = 15;
    pub const PAGE_FAULT_STATUS: u64 = 0;

    /// Nothing: the frame reports no exception.
    pub fn reported_trap(_word: impl Fn(usize) -> u64) -> [u64; 3] {
        [0; 3]
    }

    /// Nothing to check: the frame reports no exception.
    pub fn expected_trap(_code: u64, _kernel_half: bool, _address: u64) -> Option<[u64; 3]> {
        None
    }

    /// Handlers return through `ra`: nothing is popped.
    pub fn pop_return_address(_registers: &mut UserRegisters) {}

    /// The registers the simulated signal-handler program sets: `s1` and `s2`.
    pub fn kept_and_changed(registers: &mut UserRegisters) -> [&mut u64; 2] {
        let (low, high) = registers.x.split_at_mut(18);
        [&mut low[9], &mut high[0]]
    }
    pub const CHANGED_IN_UCONTEXT: u64 = 176 + 18 * 8;

    /// Where the simulated handler sets `f9` in the frame whose `ucontext_t` is at `ucontext`.
    pub fn fp_changed_in_frame(ucontext: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        ucontext + 176 + 256 + 9 * 8
    }

    /// The four registers the simulated fault-handler program sets (`s2` to `s5`).
    pub fn fault_registers(registers: &mut UserRegisters) -> [&mut u64; 4] {
        let [count, address, code, resume] = &mut registers.x[18..22] else {
            unreachable!()
        };
        [count, address, code, resume]
    }
    pub const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] =
        [176 + 18 * 8, 176 + 19 * 8, 176 + 20 * 8, 176 + 21 * 8, 176];

    /// The machine context reports nothing of a fault.
    pub fn reports_fault(_word: impl Fn(u64) -> u64, _address: u64) -> bool {
        true
    }

    /// A read's load page fault as the test platform encodes it.
    pub const READ_FAULT_CODE: u64 = 13;
}
