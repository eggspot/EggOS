//! LoongArch64's Linux ABI (D218, D290), on the generic table and structs: see [`super`].

/// The calls LoongArch keeps beyond the generic table (Linux's `__ARCH_WANT_*`): none; `fstat`
/// and `newfstatat` are in the generic table here since Linux 6.11.
pub mod wants {
    pub const LEGACY_CALLS: bool = false;
    pub const RENAMEAT: bool = false;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on LoongArch: the generic table.
pub mod number {
    pub use crate::linux::abi::generic::number::*;
}

/// The terminal and descriptor `ioctl` numbers: the generic ones.
pub use super::generic::tty;

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "loongarch64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::LOONGARCH;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = false;
/// Whether `clone`'s fourth argument is `child_tid` and its fifth `tls` (x86_64's order), not
/// the other way round.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// Whether `struct sigaction` has `sa_restorer`: not on LoongArch, whose handlers return through
/// the vDSO, here the kernel's trampoline.
pub const SIGACTION_HAS_RESTORER: bool = false;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 4096;

const NOP: u32 = 0x0340_0000;
const ORI_A7_ZERO_RT_SIGRETURN: u32 = 0x0380_0000 | (number::RT_SIGRETURN as u32) << 10 | 11;
const SYSCALL: u32 = 0x002b_0000;
/// The kernel's `rt_sigreturn` trampoline: `nop`, `li.w $a7, 139` and `syscall 0`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([NOP, ORI_A7_ZERO_RT_SIGRETURN, SYSCALL]));

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
    /// `MAP_32BIT`: none here.
    pub const MAP_32BIT: u64 = 0;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `uc_mcontext` is `struct sigcontext { u64 sc_pc; u64 sc_regs[32]; u32 sc_flags; }`, then,
/// 16-byte aligned 272 bytes in, the extended contexts: the FPU context (`FPU_CTX_MAGIC`, its
/// size, `f0` to `f31`, the condition flags `fcc0` to `fcc7` one a byte, `fcsr0`), then an
/// all-zero end record.
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

    /// `sc_flags`: the FPU context is there.
    const USED_FP: u64 = 1;
    /// The FPU context's record: its header, then its 272 bytes.
    const FPU_CONTEXT_MAGIC: u32 = 0x4650_5501;
    const FPU_RECORD: usize = MCONTEXT + 272;
    const FPU_RECORD_SIZE: u32 = 16 + 272;
    pub const FP_PART: usize = FPU_RECORD + 16;
    const END_RECORD: usize = FP_PART + 272;
    const FCC: usize = 256;
    const FCSR: usize = 264;

    /// The whole frame.
    pub const SIZE: u64 = (END_RECORD + 16) as u64;
    const _: () = assert!(SIZE == 880);
    /// What the kernel writes and reads: all of it.
    pub const WRITTEN: usize = SIZE as usize;
    pub const READ: usize = WRITTEN;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;
    /// `struct fpu_context`: `f0` to `f31`, `fcc`, `fcsr` and padding.
    pub type FpPart = [u8; 272];

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; 272];
        for (index, f) in state.f().iter().enumerate() {
            part[index * 8..][..8].copy_from_slice(&f.to_ne_bytes());
        }
        part[FCC..][..8].copy_from_slice(&state.fcc().to_ne_bytes());
        part[FCSR..][..4].copy_from_slice(&state.fcsr().to_ne_bytes());
        part
    }

    /// Every value as it is, as on Linux; only one bit of each condition flag counts.
    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        for (index, f) in state.f_mut().iter_mut().enumerate() {
            *f = word(part, index * 8);
        }
        state.set_fcc(word(part, FCC) & 0x0101_0101_0101_0101);
        state.set_fcsr(u32::from_ne_bytes(part[FCSR..][..4].try_into().unwrap()));
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
            put(&mut frame, MCONTEXT + 8 + index * 8, saved.registers.r[index]);
        }
        frame[MCONTEXT + 264..][..4].copy_from_slice(&(USED_FP as u32).to_ne_bytes());
        frame[FPU_RECORD..][..4].copy_from_slice(&FPU_CONTEXT_MAGIC.to_ne_bytes());
        frame[FPU_RECORD + 4..][..4].copy_from_slice(&FPU_RECORD_SIZE.to_ne_bytes());
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
        registers.r[4] = u64::from(handler.signal);
        registers.r[5] = frame;
        registers.r[6] = frame + UCONTEXT as u64;
        registers.r[1] = handler.restorer;
        registers.r[3] = frame;
        registers.pc = handler.entry;
    }

    /// At the stack pointer.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        Some(registers.r[3])
    }

    /// `None` unless the extended contexts are the FPU context and the end record, as the kernel
    /// writes them.
    pub fn parse(frame: &[u8; READ], _current: &UserRegisters) -> Option<Saved> {
        let magic = u32::from_ne_bytes(frame[FPU_RECORD..][..4].try_into().ok()?);
        let size = u32::from_ne_bytes(frame[FPU_RECORD + 4..][..4].try_into().ok()?);
        if magic != FPU_CONTEXT_MAGIC || size != FPU_RECORD_SIZE || frame[END_RECORD..] != [0; 16] {
            return None;
        }
        let mut r = [0; 32];
        for (index, value) in r.iter_mut().enumerate().skip(1) {
            *value = word(frame, MCONTEXT + 8 + index * 8);
        }
        Some(Saved {
            registers: UserRegisters {
                pc: word(frame, MCONTEXT),
                r,
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: frame[FP_PART..][..272].try_into().unwrap(),
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.r[4]
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
    pub const LARGEST_FRAME_SPACE_EXPECTED: u64 = 880 + 15;

    /// FP registers holding values a new thread's do not.
    #[cfg(test)]
    pub fn state_with_values() -> FpState {
        let mut state = FpState::new();
        state.f_mut()[8] = 0x0123_4567_89ab_cdef;
        state.f_mut()[31] = u64::MAX;
        state.set_fcc(1);
        state.set_fcsr(2 << 8);
        state
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// `FPE_FLTINV`: an invalid floating-point operation, what an unmasked FP exception reports
    /// when the kernel does not tell its causes apart.
    const FPE_FLTINV: i32 = 7;

    /// The signal, `si_code` and `si_addr` Linux gives `exception`, from `BADV` for the faults
    /// that set it: page faults and address errors by the process's regions; misaligned accesses
    /// `SIGBUS`; `break` `SIGTRAP` at its PC; FP exceptions `SIGFPE`; illegal instructions and
    /// every other cause `SIGILL`. The frame reports nothing more.
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
            ExceptionKind::Breakpoint | ExceptionKind::Debug => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            ExceptionKind::Arithmetic => (signal::SIGFPE, FPE_FLTINV, pc),
            ExceptionKind::IllegalInstruction
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

    /// The test platform's exception code: `ESTAT` with the code in bits 21:16.
    pub fn exception_code(class: u64, status: u64) -> u64 {
        class << 16 | status
    }

    /// Exceptions with no handler: kind, code, status and the signal that ends the process.
    pub const FAULTS_WITHOUT_HANDLERS: &[(ExceptionKind, u64, u64, u32)] = &[
        (ExceptionKind::Alignment, 0x9, 0, signal::SIGBUS),
        (ExceptionKind::InstructionFault, 0x3, 0, signal::SIGSEGV),
        (ExceptionKind::IllegalInstruction, 0xd, 0, signal::SIGILL),
        (ExceptionKind::Breakpoint, 0xc, 0, signal::SIGTRAP),
        (ExceptionKind::DataFault, 0x1, 0, signal::SIGSEGV),
        (ExceptionKind::Other, 0xa, 0, signal::SIGILL),
    ];

    /// Where a handler returns to and its stack pointer then: `ra` and `sp`.
    pub fn return_point(registers: &UserRegisters, _read_word: impl Fn(u64) -> u64) -> (u64, u64) {
        (registers.r[1], registers.r[3])
    }

    pub fn set_returned_stack(_registers: &mut UserRegisters, _stack: u64) {}

    /// A callee-saved register.
    pub fn callee_saved(registers: &mut UserRegisters) -> &mut u64 {
        &mut registers.r[24]
    }

    /// Whether a handler's frame at `frame` is 16-byte aligned below `stack`.
    pub fn frame_is_placed(frame: u64, stack: u64) -> bool {
        frame.is_multiple_of(16) && frame < stack
    }

    /// The FP part's address: in the frame.
    pub fn fp_part_address(frame: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        frame + layout::FP as u64
    }

    /// Nothing more to check of a new frame's FP part.
    pub fn check_initial_fp_part(_fp: u64, _part: &[u8], _read: impl Fn(u64, &mut [u8])) {}

    /// Where the frame points at its FP image, if it does.
    pub const FPSTATE_POINTER: Option<usize> = None;

    /// A store page fault (page invalid, store).
    pub const PAGE_FAULT_CLASS: u64 = 0x2;
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
        let (low, high) = registers.r.split_at_mut(25);
        [&mut low[24], &mut high[0]]
    }
    pub const CHANGED_IN_UCONTEXT: u64 = 176 + 8 + 25 * 8;

    /// Where the simulated handler sets `fs1` in the frame whose `ucontext_t` is at `ucontext`.
    pub fn fp_changed_in_frame(ucontext: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        ucontext + 176 + 272 + 16 + 25 * 8
    }

    /// The four registers the simulated fault-handler program sets (`s2` to `s5`).
    pub fn fault_registers(registers: &mut UserRegisters) -> [&mut u64; 4] {
        let [count, address, code, resume] = &mut registers.r[25..29] else {
            unreachable!()
        };
        [count, address, code, resume]
    }
    pub const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] =
        [176 + 8 + 25 * 8, 176 + 8 + 26 * 8, 176 + 8 + 27 * 8, 176 + 8 + 28 * 8, 176];

    /// The machine context reports nothing of a fault.
    pub fn reports_fault(_word: impl Fn(u64) -> u64, _address: u64) -> bool {
        true
    }

    /// A read's page fault (page invalid, load) as the test platform encodes it.
    pub const READ_FAULT_CODE: u64 = 0x1 << 16;
}
