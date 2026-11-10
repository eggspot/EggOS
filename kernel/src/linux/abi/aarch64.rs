//! AArch64's Linux ABI (D218), on the generic table and structs: see [`super`].

/// The calls AArch64 keeps beyond the generic table (Linux's `__ARCH_WANT_*`): `renameat`.
pub mod wants {
    pub const LEGACY_CALLS: bool = false;
    pub const RENAMEAT: bool = true;
    pub const ARCH_PRCTL: bool = false;
}

/// Linux's system-call numbers on AArch64: the generic table and `renameat`.
pub mod number {
    pub use crate::linux::abi::generic::number::*;
    pub const RENAMEAT: u64 = 38;
}

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "aarch64";
/// The `e_machine` of the programs the kernel runs natively (D17).
pub const ELF_MACHINE: u16 = crate::elf::machine::AARCH64;
/// Whether the Windows and macOS layers run programs here (D152).
pub const RUNS_WINDOWS_AND_MACOS_PROGRAMS: bool = true;
/// Whether `clone`'s fourth argument is `child_tid` and its fifth `tls` (x86_64's order), not
/// the other way round.
pub const CLONE_CHILD_TID_BEFORE_TLS: bool = false;
/// The size of a `struct epoll_event`: the 64-bit data aligned.
pub const EPOLL_EVENT_SIZE: usize = 16;
/// Whether `struct sigaction` has `sa_restorer`.
pub const SIGACTION_HAS_RESTORER: bool = true;
/// `MINSIGSTKSZ`: room for the largest signal frame.
pub const MIN_SIGNAL_STACK_SIZE: u64 = 5120;

const NOP: u32 = 0xd503_201f;
const MOVZ_X8_RT_SIGRETURN: u32 = 0xd280_0000 | (number::RT_SIGRETURN as u32) << 5 | 8;
const SVC_0: u32 = 0xd400_0001;
/// The kernel's `rt_sigreturn` trampoline for handlers without `SA_RESTORER`: `nop`,
/// `movz x8, #139` and `svc #0`.
pub const SIGRETURN_TRAMPOLINE: Option<&[u8]> =
    Some(&super::code_bytes([NOP, MOVZ_X8_RT_SIGRETURN, SVC_0]));

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
    pub const DIRECTORY: u32 = 0o40_000;
    /// `O_NOFOLLOW`.
    pub const NOFOLLOW: u32 = 0o100_000;
}

/// The `mmap` bits only some architectures have.
pub mod mmap {
    /// The architecture's own `prot` bits: `PROT_BTI`, branch-target pages.
    pub const PROT_ARCH: u64 = 0x10;
    /// `MAP_32BIT`: somewhere in the second gigabyte, none here.
    pub const MAP_32BIT: u64 = 0;

    /// Whether `flags` asks for [`MAP_32BIT`].
    pub fn wants_32_bit(flags: u64) -> bool {
        let _ = flags;
        false
    }
}

/// `uc_mcontext` is `struct sigcontext { u64 fault_address; u64 regs[31]; u64 sp, pc,
/// pstate; u8 __reserved[4096]; }`; `__reserved` holds records, each a magic number and a
/// size: here the FP/SIMD one (`fpsimd_context`), which Linux requires, for a fault's
/// signal an `esr_context` with the syndrome, then the terminating null one. Above the
/// frame is a frame record (`x29`, `x30`) that `x29` points at, so a backtrace from the
/// handler walks into the interrupted code.
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

    pub const FAULT_ADDRESS: usize = MCONTEXT;
    pub const REGS: usize = MCONTEXT + 8;
    pub const SP: usize = MCONTEXT + 256;
    pub const PC: usize = MCONTEXT + 264;
    pub const PSTATE: usize = MCONTEXT + 272;
    pub const RESERVED: usize = MCONTEXT + 288;
    const _: () = assert!(REGS == 312 && SP == 560 && PC == 568 && PSTATE == 576);
    const _: () = assert!(RESERVED == 592);
    /// The whole frame: the `ucontext_t` is 4560 bytes.
    pub const SIZE: u64 = (UCONTEXT + 4560) as u64;
    const _: () = assert!(SIZE == 4688);
    /// `fpsimd_context`: its magic number and size, then `fpsr`, `fpcr` and `v0` to `v31`.
    pub const FPSIMD_MAGIC: u32 = 0x4650_8001;
    pub const FPSIMD_SIZE: usize = 528;
    /// What follows the record's magic number and size: `fpsr` (32 bits), `fpcr` (32 bits),
    /// then `v0` to `v31` (128 bits each).
    pub type FpPart = [u8; FPSIMD_SIZE - 8];
    pub const FP_PART: usize = RESERVED + 8;
    pub const FP_V: usize = 8;
    const _: () = assert!(FP_PART + FP_V == 608);

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = false;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = [0; FPSIMD_SIZE - 8];
        part[..4].copy_from_slice(&state.fpsr().to_ne_bytes());
        part[4..8].copy_from_slice(&state.fpcr().to_ne_bytes());
        for (index, v) in state.v().iter().enumerate() {
            part[FP_V + index * 16..][..16].copy_from_slice(&v.to_ne_bytes());
        }
        part
    }

    /// Every value as it is, as on Linux: the reserved bits of `FPCR` and `FPSR` ignore writes.
    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        let half = |at: usize| u32::from_ne_bytes(part[at..at + 4].try_into().unwrap());
        state.set_fpcr_and_fpsr(half(4), half(0));
        for (index, v) in state.v_mut().iter_mut().enumerate() {
            *v = u128::from_ne_bytes(part[FP_V + index * 16..][..16].try_into().unwrap());
        }
        Ok(())
    }
    /// `esr_context`, after a fault's: its magic number and size, then `ESR_EL1`. Skipped
    /// when read back.
    pub const ESR_MAGIC: u32 = 0x4553_5201;
    pub const ESR_SIZE: usize = 16;
    /// What the kernel writes of the frame: up to the terminating record (8 bytes, zero),
    /// with room for an `esr_context` before it (zeros after the terminator without one).
    /// The rest of `__reserved` is left as it was, as Linux leaves it.
    pub const WRITTEN: usize = RESERVED + FPSIMD_SIZE + ESR_SIZE + 8;
    /// What `rt_sigreturn` reads: all of that.
    pub const READ: usize = WRITTEN;
    /// The frame record above the frame.
    pub const RECORD: u64 = 16;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;

    /// The frame record 16-byte aligned below `top`, the frame below it.
    pub const fn place(top: u64) -> u64 {
        (top.wrapping_sub(RECORD) & !15).wrapping_sub(SIZE)
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        trap: Option<&Trap>,
        _restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        frame[..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, SIGMASK, saved.blocked);
        if let Some(trap) = trap.filter(|trap| trap.cause != 0) {
            put(&mut frame, FAULT_ADDRESS, trap.address);
            let esr = RESERVED + FPSIMD_SIZE;
            frame[esr..][..4].copy_from_slice(&ESR_MAGIC.to_ne_bytes());
            frame[esr + 4..][..4].copy_from_slice(&(ESR_SIZE as u32).to_ne_bytes());
            put(&mut frame, esr + 8, trap.cause);
        }
        let registers = &saved.registers;
        for (index, &value) in registers.x.iter().enumerate() {
            put(&mut frame, REGS + index * 8, value);
        }
        put(&mut frame, SP, registers.sp);
        put(&mut frame, PC, registers.pc);
        put(&mut frame, PSTATE, registers.pstate);
        frame[RESERVED..][..4].copy_from_slice(&FPSIMD_MAGIC.to_ne_bytes());
        frame[RESERVED + 4..][..4].copy_from_slice(&(FPSIMD_SIZE as u32).to_ne_bytes());
        frame[FP_PART..][..saved.fp.len()].copy_from_slice(&saved.fp);
        (place(top), frame)
    }

    /// The frame record: the interrupted `x29` and `x30`.
    pub fn write_extra<P: Platform>(
        process: &Process<P>,
        _top: u64,
        frame: u64,
        saved: &Saved,
    ) -> Result<(), BadFrame> {
        let mut record = [0; RECORD as usize];
        put(&mut record, 0, saved.registers.x[29]);
        put(&mut record, 8, saved.registers.x[30]);
        process
            .write_user_memory(frame + SIZE, &record)
            .map_err(|_| BadFrame)
    }

    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.x[0] = u64::from(handler.signal);
        registers.x[1] = frame;
        registers.x[2] = frame + UCONTEXT as u64;
        registers.x[29] = frame + SIZE;
        registers.x[30] = handler.restorer;
        registers.sp = frame;
        registers.pc = handler.entry;
    }

    /// At the stack pointer, which Linux requires to be 16-byte aligned there.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        registers.sp.is_multiple_of(16).then_some(registers.sp)
    }

    /// A record's magic number and size at `offset`.
    pub fn record(frame: &[u8; READ], offset: usize) -> (u32, usize) {
        let half = |at: usize| u32::from_ne_bytes(frame[at..at + 4].try_into().unwrap());
        (half(offset), half(offset + 4) as usize)
    }

    /// `None` unless `__reserved` holds the FP/SIMD record of its size, then optionally an
    /// `esr_context`, then the terminating record, as the kernel writes them.
    pub fn parse(frame: &[u8; READ], _current: &UserRegisters) -> Option<Saved> {
        if record(frame, RESERVED) != (FPSIMD_MAGIC, FPSIMD_SIZE) {
            return None;
        }
        let mut next = RESERVED + FPSIMD_SIZE;
        if record(frame, next) == (ESR_MAGIC, ESR_SIZE) {
            next += ESR_SIZE;
        }
        if record(frame, next) != (0, 0) {
            return None;
        }
        Some(Saved {
            registers: UserRegisters {
                x: core::array::from_fn(|index| word(frame, REGS + index * 8)),
                sp: word(frame, SP),
                pc: word(frame, PC),
                pstate: word(frame, PSTATE),
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: frame[FP_PART..][..FPSIMD_SIZE - 8].try_into().unwrap(),
        })
    }

    /// Nothing outside the frame.
    pub fn read_extra<P: Platform>(
        _: &Process<P>,
        _: &[u8; READ],
        _: &mut Saved,
    ) -> Result<(), BadFrame> {
        Ok(())
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.x[0]
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
        /// `fpsimd_context`'s `fpsr`.
        pub const FP: usize = super::FP_PART;
        pub const FAULT_ADDRESS: usize = super::FAULT_ADDRESS;
        /// The record after the FP/SIMD one.
        pub const ESR: usize = super::RESERVED + super::FPSIMD_SIZE;
    }

    /// The most stack a frame takes below an alternate stack's top, worked out by hand.
    #[cfg(test)]
    pub const LARGEST_FRAME_SPACE_EXPECTED: u64 = 16 + 15 + 4688;

    /// FP/SIMD registers holding values a new thread's do not.
    #[cfg(test)]
    pub fn state_with_values() -> FpState {
        let mut state = FpState::new();
        state.v_mut()[8] = 0x0123_4567_89ab_cdef_0011_2233_4455_6677;
        state.v_mut()[31] = u128::MAX;
        state.set_fpcr_and_fpsr(1 << 22, 1);
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
        fn aarch64_s_part_is_fpsr_fpcr_then_the_v_registers() {
            let part = fp_part(&state_with_values());
            assert_eq!(part[..8], [1, 0, 0, 0, 0, 0, 0x40, 0]);
            assert_eq!(
                part[8 + 8 * 16..][..16],
                0x0123_4567_89ab_cdef_0011_2233_4455_6677u128.to_ne_bytes()
            );
        }
    }
}

/// Linux's fault encodings here ([`crate::linux::fault`]).
pub mod fault {
    use crate::linux::fault::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// Exception classes (`ESR_EL1.EC`).
    pub const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
    pub const EC_INSTRUCTION_ABORT: u64 = 0x21;
    pub const EC_DATA_ABORT_LOWER: u64 = 0x24;
    pub const EC_DATA_ABORT: u64 = 0x25;
    pub const EC_SP_ALIGNMENT: u64 = 0x26;
    pub const EC_PC_ALIGNMENT: u64 = 0x22;
    /// `ESR_EL1` bits a sanitized syndrome keeps: the class and the instruction length, and
    /// for a data abort, write-not-read and cache maintenance.
    pub const ESR_CLASS_AND_LENGTH: u64 = 0x3f << 26 | 1 << 25;
    pub const ESR_DATA_ABORT_KEPT: u64 = 1 << 6 | 1 << 8;
    /// The fault status code: an alignment fault, a level 0 translation fault.
    pub const STATUS_ALIGNMENT: u64 = 0b10_0001;
    pub const STATUS_TRANSLATION_LEVEL_0: u64 = 0b00_0100;

    /// Whether fault status code `status` is a synchronous external abort or a parity or ECC
    /// error, on the access itself or on a table walk for it at levels 0 to 3 (0b010001 is
    /// MTE's tag check fault, not one).
    pub fn external_abort(status: u64) -> bool {
        matches!(status, 0b01_0000 | 0b01_0100..=0b01_1000 | 0b01_1100..=0b01_1111)
    }

    /// The syndrome a program sees for a fault at an address outside user space: the class
    /// and instruction length, and for an abort a level 0 translation fault (with, for a data
    /// abort, whether it wrote), whatever the kernel's tables hold there; for an alignment
    /// fault, the class and length alone; 0 for any other class.
    pub fn sanitized(esr: u64) -> u64 {
        let class = esr >> 26 & 0x3f;
        match class {
            EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => {
                esr & ESR_CLASS_AND_LENGTH | STATUS_TRANSLATION_LEVEL_0
            }
            EC_DATA_ABORT_LOWER | EC_DATA_ABORT => {
                esr & (ESR_CLASS_AND_LENGTH | ESR_DATA_ABORT_KEPT) | STATUS_TRANSLATION_LEVEL_0
            }
            EC_PC_ALIGNMENT | EC_SP_ALIGNMENT => esr & ESR_CLASS_AND_LENGTH,
            _ => 0,
        }
    }

    /// The signal, `si_code` and `si_addr` Linux gives `exception`, and its `fault_address` and
    /// `esr_context`: an abort at the FAR (when the abort says it is valid), `SIGBUS` for an
    /// alignment fault or an external abort, else by the process's regions; an SP alignment
    /// fault `SIGBUS` at the stack pointer it had (the FAR is unknown then), a PC alignment
    /// fault at the misaligned PC in the FAR; `brk` `SIGTRAP` at its PC, which stays on it, so
    /// a handler that returns runs it again; undefined instructions, the FP/SIMD trap and every
    /// other class from EL0 `SIGILL` (the HAL reports no arithmetic, debug or segment
    /// exceptions on AArch64).
    pub fn classify(
        exception: &Exception,
        stack: u64,
        in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let esr = exception.code;
        let pc = exception.pc;
        let (signal, code, address) = match exception.kind {
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => {
                let address = exception.address.unwrap_or(0);
                match esr & 0x3f {
                    STATUS_ALIGNMENT => (signal::SIGBUS, code::BUS_ADRALN, address),
                    status if external_abort(status) => (signal::SIGBUS, code::BUS_OBJERR, address),
                    _ => segv(address),
                }
            }
            ExceptionKind::Alignment if esr >> 26 & 0x3f == EC_SP_ALIGNMENT => {
                (signal::SIGBUS, code::BUS_ADRALN, stack)
            }
            ExceptionKind::Alignment => (
                signal::SIGBUS,
                code::BUS_ADRALN,
                exception.address.unwrap_or(pc),
            ),
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Arithmetic
            | ExceptionKind::Debug
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        let trap = match exception.kind {
            ExceptionKind::DataFault
            | ExceptionKind::InstructionFault
            | ExceptionKind::Alignment => Trap {
                number: 0,
                cause: match exception.address {
                    Some(far) if !in_user_space(far) => sanitized(esr),
                    _ => esr,
                },
                address,
            },
            _ => Trap::default(),
        };
        fault((signal, code, address), trap)
    }

    #[cfg(test)]
    mod tests {
        use super::sanitized;

        #[test]
        fn a_pc_alignment_fault_at_a_kernel_address_keeps_its_class_and_length() {
            let pc_alignment_with_length_and_iss = 0x8a00_0123;
            assert_eq!(sanitized(pc_alignment_with_length_and_iss), 0x8a00_0000);
        }

        #[test]
        fn syndromes_for_kernel_addresses_say_nothing_of_the_kernel_s_tables() {
            let write_permission_fault_at_level_3 = 0x9200_004f;
            let level_0_translation_fault_that_wrote = 0x9200_0044;
            assert_eq!(
                sanitized(write_permission_fault_at_level_3),
                level_0_translation_fault_that_wrote
            );
            let instruction_access_flag_fault_at_level_2 = 0x8200_000a;
            assert_eq!(
                sanitized(instruction_access_flag_fault_at_level_2),
                0x8200_0004
            );
            let read_translation_fault_with_far_not_valid = 0x9200_0405;
            assert_eq!(
                sanitized(read_translation_fault_with_far_not_valid),
                0x9200_0004
            );
            let unknown_class = 0xf200_0000;
            assert_eq!(sanitized(unknown_class), 0);
        }
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

    /// The test platform's exception code: the class in `ESR_EL1.EC`.
    pub fn exception_code(class: u64, status: u64) -> u64 {
        class << 26 | status
    }

    /// Exceptions with no handler: kind, class, status and the signal that ends the process.
    pub const FAULTS_WITHOUT_HANDLERS: &[(ExceptionKind, u64, u64, u32)] = &[
        (ExceptionKind::IllegalInstruction, 0x00, 0, signal::SIGILL),
        (ExceptionKind::Other, 0x0e, 0, signal::SIGILL),
        (ExceptionKind::InstructionFault, 0x20, 0, signal::SIGSEGV),
        (ExceptionKind::Alignment, 0x22, 0, signal::SIGBUS),
        (ExceptionKind::DataFault, 0x24, 0, signal::SIGSEGV),
        (ExceptionKind::DataFault, 0x24, 0x21, signal::SIGBUS),
        (ExceptionKind::DataFault, 0x24, 0x10, signal::SIGBUS),
        (ExceptionKind::Alignment, 0x26, 0, signal::SIGBUS),
        (ExceptionKind::Breakpoint, 0x3c, 0, signal::SIGTRAP),
    ];

    /// Where a handler returns to and its stack pointer then: `x30` and `sp`.
    pub fn return_point(registers: &UserRegisters, _read_word: impl Fn(u64) -> u64) -> (u64, u64) {
        (registers.x[30], registers.sp)
    }

    pub fn set_returned_stack(_registers: &mut UserRegisters, _stack: u64) {}

    /// A callee-saved register.
    pub fn callee_saved(registers: &mut UserRegisters) -> &mut u64 {
        &mut registers.x[19]
    }

    /// Whether a handler's frame at `frame` is 16-byte aligned below `stack`.
    pub fn frame_is_placed(frame: u64, stack: u64) -> bool {
        frame.is_multiple_of(16) && frame < stack
    }

    /// The FP/SIMD part's address: in the frame.
    pub fn fp_part_address(frame: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        frame + layout::FP as u64
    }

    /// Checks a new frame's FP/SIMD part at `fp`: the `fpsimd_context` header before it.
    pub fn check_initial_fp_part(fp: u64, _part: &[u8], read: impl Fn(u64, &mut [u8])) {
        let mut header = [0; 8];
        read(fp - 8, &mut header);
        assert_eq!(header, [0x01, 0x80, 0x50, 0x46, 0x10, 0x02, 0, 0]);
    }

    /// Where the frame points at its FP/SIMD image, if it does.
    pub const FPSTATE_POINTER: Option<usize> = None;

    /// A data abort from EL0 and the instruction-length bit the test platform adds.
    pub const PAGE_FAULT_CLASS: u64 = 0x24;
    pub const PAGE_FAULT_STATUS: u64 = 1 << 25;

    /// `fault_address`, then the record after the FP/SIMD one (its magic number and size) and
    /// the syndrome it holds, read with `word`.
    pub fn reported_trap(word: impl Fn(usize) -> u64) -> [u64; 3] {
        [
            word(layout::FAULT_ADDRESS),
            word(layout::ESR),
            word(layout::ESR + 8),
        ]
    }

    /// What [`reported_trap`] reads for a page fault with syndrome `code` at `address`: the
    /// syndrome sanitized past user space.
    pub fn expected_trap(code: u64, kernel_half: bool, address: u64) -> Option<[u64; 3]> {
        let esr = if kernel_half {
            super::fault::sanitized(code)
        } else {
            code
        };
        Some([address, 0x10_4553_5201, esr])
    }

    /// Handlers return through `x30`: nothing is popped.
    pub fn pop_return_address(_registers: &mut UserRegisters) {}

    /// The registers the simulated signal-handler program sets: `x19` and `x20`.
    pub fn kept_and_changed(registers: &mut UserRegisters) -> [&mut u64; 2] {
        let [.., kept, changed, _, _, _, _, _, _, _, _, _, _] = &mut registers.x;
        [kept, changed]
    }
    pub const CHANGED_IN_UCONTEXT: u64 = 176 + 8 + 20 * 8;

    /// Where the simulated handler sets `v9` in the frame whose `ucontext_t` is at `ucontext`.
    pub fn fp_changed_in_frame(ucontext: u64, _read_word: impl Fn(u64) -> u64) -> u64 {
        ucontext + 176 + 288 + 16 + 9 * 16
    }

    /// The four registers the simulated fault-handler program sets (`x20` to `x23`).
    pub fn fault_registers(registers: &mut UserRegisters) -> [&mut u64; 4] {
        let [.., count, address, code, resume, _, _, _, _, _, _, _] = &mut registers.x;
        [count, address, code, resume]
    }
    pub const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] = [
        176 + 8 + 20 * 8,
        176 + 8 + 21 * 8,
        176 + 8 + 22 * 8,
        176 + 8 + 23 * 8,
        176 + 264,
    ];

    /// Whether the machine context reports a fault at `address`: `fault_address` and an
    /// `esr_context` record.
    pub fn reports_fault(word: impl Fn(u64) -> u64, address: u64) -> bool {
        word(176) == address && word(176 + 288 + 528) == 0x10_4553_5201
    }

    /// A read's data abort as the test platform encodes it.
    pub const READ_FAULT_CODE: u64 = 0x24 << 26;
}
