//! Linux's signal frames (M6, D64): what the kernel writes on a program's stack before one of
//! its signal handlers runs, and reads back when the handler returns with `rt_sigreturn`.
//!
//! A frame is Linux's `struct rt_sigframe` on the architecture: the signal's `siginfo_t` and a
//! `ucontext_t` holding the interrupted registers, blocked mask and alternate stack. Every
//! offset below is the architecture's Linux ABI (its `asm/sigcontext.h` and `asm/ucontext.h`),
//! so C libraries, unwinders and debuggers that read a frame find what they expect. The
//! handler starts with the signal number, the `siginfo_t` and the `ucontext_t` as its three
//! arguments, and returns to its restorer: the code at `sa_restorer` that calls
//! `rt_sigreturn` (x86_64 and AArch64), or the kernel's own return trampoline
//! ([`super::trampoline`]).
//!
//! Every frame has its FP/SIMD part in Linux's shape (D64): x86_64's `FXSAVE` image above the
//! frame, which `fpstate` points at, AArch64's `fpsimd_context` record and RISC-V's
//! D-extension state. It holds the thread's registers as the interrupted code left them, or
//! the architecture's initial state for a thread that has none (D82). `rt_sigreturn` checks
//! the records' shape and loads the registers from them, so a handler that changes them in the
//! frame changes them for the interrupted code, as on Linux.
//!
//! A fault's frame (D65) also reports the exception ([`Trap`]): x86_64's `err`, `trapno` and
//! `cr2` in the `sigcontext`, AArch64's `fault_address` and an `esr_context` record after the
//! FP/SIMD one (aborts and alignment faults only, D67). Other signals' frames hold zeros
//! there and no `esr_context`.

use super::fault::{Fault, Trap};
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::fp::FpState;
use eggos_hal::user::UserRegisters;

pub use arch::FpPart;

/// The size of a `siginfo_t`.
pub const INFO_SIZE: usize = 128;

/// The size of a `stack_t`.
pub const STACK_SIZE: usize = 24;

/// The `siginfo_t` of `fault`: `si_signo`, `si_errno` 0, `si_code` and `si_addr`, the
/// sender's fields' place.
pub fn fault_info(fault: &Fault) -> [u8; INFO_SIZE] {
    let mut info = [0; INFO_SIZE];
    info[..4].copy_from_slice(&(fault.signal as i32).to_ne_bytes());
    info[8..12].copy_from_slice(&fault.code.to_ne_bytes());
    info[16..24].copy_from_slice(&fault.address.to_ne_bytes());
    info
}

/// What a frame saves of the interrupted thread and `rt_sigreturn` restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    pub registers: UserRegisters,
    /// The signals it blocked.
    pub blocked: u64,
    /// Its alternate stack, a `stack_t`.
    pub alternate_stack: [u8; STACK_SIZE],
    /// Its FP/SIMD registers, as the frame holds them ([`fp_part`]).
    pub fp: FpPart,
}

/// The FP/SIMD part of a frame holding `state`'s registers.
pub fn fp_part(state: &FpState) -> FpPart {
    arch::fp_part(state)
}

/// The FP/SIMD part of a frame for a thread that has no FP/SIMD registers: the initial state,
/// what it would start with.
pub fn initial_fp_part() -> FpPart {
    arch::fp_part(&FpState::new())
}

/// Sets `state`'s registers to those `part` holds. [`BadFrame`], changing nothing, if a load
/// would fault on them (x86_64's reserved `MXCSR` bits), as Linux's `rt_sigreturn` refuses them.
pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
    arch::load_fp_part(part, state)
}

/// Whether a handler starts with the initial FP/SIMD state rather than the interrupted code's,
/// as on Linux: x86_64 only.
pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = arch::HANDLER_STARTS_WITH_INITIAL_FP;

/// Why a frame could not be written or read: its memory is not the program's (or, for a
/// handler with no way to return, there is none). Linux then sends `SIGSEGV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadFrame;

/// A handler about to run: what to write, where, and how it starts.
#[derive(Debug, Clone, Copy)]
pub struct Handler {
    /// The handler's address.
    pub entry: u64,
    /// Where it returns to: its `sa_restorer`, or the kernel's trampoline.
    pub restorer: u64,
    pub signal: u32,
}

/// Writes the frame for `handler` below stack pointer `top` (already on the alternate stack if
/// the handler runs there) and sets `registers` to start the handler with it: on success the
/// program's next instruction is the handler's first. `saved` is what the frame keeps, `trap`
/// the exception it reports for a fault's signal. Its padding and unused fields are written as
/// zeros (but AArch64's `__reserved` after the terminating record, which Linux leaves as it
/// was too).
pub fn write<P: Platform>(
    process: &Process<P>,
    top: u64,
    saved: &Saved,
    info: &[u8; INFO_SIZE],
    trap: Option<&Trap>,
    handler: &Handler,
    registers: &mut UserRegisters,
) -> Result<(), BadFrame> {
    let (address, bytes) = arch::build(top, saved, info, trap, handler.restorer);
    process
        .write_user_memory(address, &bytes)
        .map_err(|_| BadFrame)?;
    arch::write_extra(process, top, address, saved)?;
    arch::enter(registers, address, handler);
    Ok(())
}

/// How far below the interrupted stack pointer a frame starts, unless it goes on the
/// alternate stack: the ABI's red zone (x86_64's 128 bytes), which the interrupted code may be
/// using.
pub const RED_ZONE: u64 = arch::RED_ZONE;

/// The lowest address of the frame [`write()`] puts below `top`: where the stack pointer must
/// stay above, on an alternate stack.
pub fn lowest_address(top: u64) -> u64 {
    arch::place(top)
}

/// `rt_sigreturn`'s side: reads the frame of the handler that returned, whose registers are
/// `registers` (its stack pointer where the frame is), and returns what it saved.
pub fn read<P: Platform>(
    process: &Process<P>,
    registers: &UserRegisters,
) -> Result<Saved, BadFrame> {
    let address = arch::returned_frame(registers).ok_or(BadFrame)?;
    let mut bytes = [0; arch::READ];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| BadFrame)?;
    let mut saved = arch::parse(&bytes, registers).ok_or(BadFrame)?;
    arch::read_extra(process, &bytes, &mut saved)?;
    Ok(saved)
}

/// The value in the system-call result register of `registers`, which the dispatcher writes
/// back after `rt_sigreturn` returns it.
pub fn result(registers: &UserRegisters) -> u64 {
    arch::result(registers)
}

/// The frame's layout, for tests.
#[cfg(test)]
pub use arch::layout;

/// Reads the `u64` at `offset`.
fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Writes `value` at `offset`.
fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

/// The `ucontext_t` fields every architecture puts first: `uc_flags` at 0 (only x86_64 sets
/// any), `uc_link` (null) and `uc_stack`.
mod ucontext {
    pub const STACK: usize = 16;
}

#[cfg(target_arch = "x86_64")]
mod arch {
    //! `struct rt_sigframe { char *pretcode; ucontext_t uc; siginfo_t info; }`, the
    //! `ucontext_t` holding `uc_mcontext` (`struct sigcontext`) before `uc_sigmask`.
    use super::{
        BadFrame, FpState, Handler, INFO_SIZE, STACK_SIZE, Saved, Trap, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::user::UserRegisters;

    const UCONTEXT: usize = 8;
    const MCONTEXT: usize = UCONTEXT + 40;
    const SIGMASK: usize = UCONTEXT + 296;
    const INFO: usize = UCONTEXT + 304;
    /// The whole frame.
    pub const WRITTEN: usize = INFO + INFO_SIZE;
    const _: () = assert!(WRITTEN == 440);
    /// What `rt_sigreturn` reads first: the frame, then the FP/SIMD image its `fpstate` points
    /// at ([`read_extra`]).
    pub const READ: usize = WRITTEN;

    /// The `FXSAVE` image (`struct _fpstate`): 512 bytes, 64-byte aligned, above the frame.
    const FP_SIZE: u64 = 512;
    /// The image, its software-reserved bytes from 464 zero: no `XSAVE` state follows.
    pub type FpPart = [u8; FP_SIZE as usize];
    const FP_SOFTWARE: usize = 464;

    pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = true;

    pub fn fp_part(state: &FpState) -> FpPart {
        let mut part = *state.fxsave_image();
        part[FP_SOFTWARE..].fill(0);
        part
    }

    pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
        state.set_fxsave_image(part).map_err(|_| BadFrame)
    }

    /// `uc_flags`: `UC_SIGCONTEXT_SS` and `UC_STRICT_RESTORE_SS`, as Linux sets them for
    /// every 64-bit frame (the saved `ss` is the real one).
    const FLAGS: u64 = 0x2 | 0x4;

    /// `struct sigcontext`'s fields, from `uc_mcontext`, in the order of `regs`' below.
    const REGISTERS: usize = 18;
    const SELECTORS: usize = 144;
    const ERR: usize = 152;
    const TRAPNO: usize = 160;
    const OLDMASK: usize = 168;
    const CR2: usize = 176;
    const FPSTATE: usize = 184;
    // Linux's offsets from the frame, written out: what programs read.
    const _: () = assert!(UCONTEXT == 8 && MCONTEXT == 48 && SIGMASK == 304 && INFO == 312);
    const _: () = assert!(MCONTEXT + SELECTORS == 192 && MCONTEXT + FPSTATE == 232);

    /// The flags a handler starts without, as on Linux: trap, direction and resume.
    const HANDLER_CLEARS: u64 = 1 << 8 | 1 << 10 | 1 << 16;

    fn regs(registers: &UserRegisters) -> [u64; REGISTERS] {
        [
            registers.r8,
            registers.r9,
            registers.r10,
            registers.r11,
            registers.r12,
            registers.r13,
            registers.r14,
            registers.r15,
            registers.rdi,
            registers.rsi,
            registers.rbp,
            registers.rbx,
            registers.rdx,
            registers.rax,
            registers.rcx,
            registers.rsp,
            registers.rip,
            registers.rflags,
        ]
    }

    /// The 128 bytes below the stack pointer that leaf functions may use (the ABI's red
    /// zone).
    pub const RED_ZONE: u64 = 128;

    /// The FP/SIMD image, then the frame, so that `(rsp + 8) % 16 == 0` when the handler
    /// starts, as after a `call`.
    fn fp_image(top: u64) -> u64 {
        top.wrapping_sub(FP_SIZE) & !63
    }

    pub fn place(top: u64) -> u64 {
        (fp_image(top).wrapping_sub(WRITTEN as u64) & !15).wrapping_sub(8)
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
        trap: Option<&Trap>,
        restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        put(&mut frame, 0, restorer);
        if let Some(trap) = trap {
            put(&mut frame, MCONTEXT + ERR, trap.cause);
            put(&mut frame, MCONTEXT + TRAPNO, trap.number);
            put(&mut frame, MCONTEXT + CR2, trap.address);
        }
        put(&mut frame, UCONTEXT, FLAGS);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        for (index, value) in regs(&saved.registers).into_iter().enumerate() {
            put(&mut frame, MCONTEXT + index * 8, value);
        }
        let selectors = u64::from(saved.registers.cs) | u64::from(saved.registers.ss) << 48;
        put(&mut frame, MCONTEXT + SELECTORS, selectors);
        put(&mut frame, MCONTEXT + OLDMASK, saved.blocked);
        put(&mut frame, MCONTEXT + FPSTATE, fp_image(top));
        put(&mut frame, SIGMASK, saved.blocked);
        frame[INFO..].copy_from_slice(info);
        (place(top), frame)
    }

    /// The FP/SIMD image above the frame.
    pub fn write_extra<P: Platform>(
        process: &Process<P>,
        top: u64,
        _frame: u64,
        saved: &Saved,
    ) -> Result<(), BadFrame> {
        process
            .write_user_memory(fp_image(top), &saved.fp)
            .map_err(|_| BadFrame)
    }

    /// The FP/SIMD image `fpstate` points at; the initial state if it is null. One that is
    /// not 16-byte aligned, as `FXRSTOR64` needs, is refused, as on Linux, which loads it from
    /// the program's memory.
    pub fn read_extra<P: Platform>(
        process: &Process<P>,
        frame: &[u8; READ],
        saved: &mut Saved,
    ) -> Result<(), BadFrame> {
        match word(frame, MCONTEXT + FPSTATE) {
            0 => {
                saved.fp = fp_part(&FpState::new());
                Ok(())
            }
            image if image.is_multiple_of(16) => process
                .read_user_memory(image, &mut saved.fp)
                .map_err(|_| BadFrame),
            _ => Err(BadFrame),
        }
    }

    pub fn enter(registers: &mut UserRegisters, frame: u64, handler: &Handler) {
        registers.rdi = u64::from(handler.signal);
        registers.rsi = frame + INFO as u64;
        registers.rdx = frame + UCONTEXT as u64;
        registers.rax = 0;
        registers.rsp = frame;
        registers.rip = handler.entry;
        registers.rflags &= !HANDLER_CLEARS;
    }

    /// The handler's `ret` popped `pretcode`.
    pub fn returned_frame(registers: &UserRegisters) -> Option<u64> {
        registers.rsp.checked_sub(8)
    }

    /// The registers, but the segment selectors, which stay the kernel's.
    pub fn parse(frame: &[u8; READ], current: &UserRegisters) -> Option<Saved> {
        let regs: [u64; REGISTERS] =
            core::array::from_fn(|index| word(frame, MCONTEXT + index * 8));
        let [
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            rdi,
            rsi,
            rbp,
            rbx,
            rdx,
            rax,
            rcx,
            rsp,
            rip,
            rflags,
        ] = regs;
        Some(Saved {
            registers: UserRegisters {
                r8,
                r9,
                r10,
                r11,
                r12,
                r13,
                r14,
                r15,
                rdi,
                rsi,
                rbp,
                rbx,
                rdx,
                rax,
                rcx,
                rsp,
                rip,
                rflags,
                ..*current
            },
            blocked: word(frame, SIGMASK),
            alternate_stack: frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE]
                .try_into()
                .unwrap(),
            fp: [0; FP_SIZE as usize],
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.rax
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
        pub const FPSTATE: usize = super::MCONTEXT + super::FPSTATE;
        pub const ERR: usize = super::MCONTEXT + super::ERR;
        pub const TRAPNO: usize = super::MCONTEXT + super::TRAPNO;
        pub const CR2: usize = super::MCONTEXT + super::CR2;
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
mod generic {
    //! What AArch64's and RISC-V's frames share: `struct rt_sigframe { siginfo_t info;
    //! ucontext_t uc; }`, the `ucontext_t` holding `uc_sigmask`, 120 unused bytes (room for a
    //! larger `sigset_t`), then `uc_mcontext`, 16-byte aligned.
    use super::INFO_SIZE;

    pub const UCONTEXT: usize = INFO_SIZE;
    pub const SIGMASK: usize = UCONTEXT + 40;
    pub const MCONTEXT: usize = UCONTEXT + 176;
    // Linux's offsets from the frame, written out: what programs read.
    const _: () = assert!(UCONTEXT == 128 && SIGMASK == 168 && MCONTEXT == 304);
}

#[cfg(target_arch = "aarch64")]
mod arch {
    //! `uc_mcontext` is `struct sigcontext { u64 fault_address; u64 regs[31]; u64 sp, pc,
    //! pstate; u8 __reserved[4096]; }`; `__reserved` holds records, each a magic number and a
    //! size: here the FP/SIMD one (`fpsimd_context`), which Linux requires, for a fault's
    //! signal an `esr_context` with the syndrome, then the terminating null one. Above the
    //! frame is a frame record (`x29`, `x30`) that `x29` points at, so a backtrace from the
    //! handler walks into the interrupted code.
    use super::generic::{MCONTEXT, SIGMASK, UCONTEXT};
    use super::{
        BadFrame, FpState, Handler, INFO_SIZE, STACK_SIZE, Saved, Trap, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::user::UserRegisters;

    const FAULT_ADDRESS: usize = MCONTEXT;
    const REGS: usize = MCONTEXT + 8;
    const SP: usize = MCONTEXT + 256;
    const PC: usize = MCONTEXT + 264;
    const PSTATE: usize = MCONTEXT + 272;
    const RESERVED: usize = MCONTEXT + 288;
    const _: () = assert!(REGS == 312 && SP == 560 && PC == 568 && PSTATE == 576);
    const _: () = assert!(RESERVED == 592);
    /// The whole frame: the `ucontext_t` is 4560 bytes.
    const SIZE: u64 = (UCONTEXT + 4560) as u64;
    const _: () = assert!(SIZE == 4688);
    /// `fpsimd_context`: its magic number and size, then `fpsr`, `fpcr` and `v0` to `v31`.
    const FPSIMD_MAGIC: u32 = 0x4650_8001;
    const FPSIMD_SIZE: usize = 528;
    /// What follows the record's magic number and size: `fpsr` (32 bits), `fpcr` (32 bits),
    /// then `v0` to `v31` (128 bits each).
    pub type FpPart = [u8; FPSIMD_SIZE - 8];
    const FP_PART: usize = RESERVED + 8;
    const FP_V: usize = 8;
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
    const ESR_MAGIC: u32 = 0x4553_5201;
    const ESR_SIZE: usize = 16;
    /// What the kernel writes of the frame: up to the terminating record (8 bytes, zero),
    /// with room for an `esr_context` before it (zeros after the terminator without one).
    /// The rest of `__reserved` is left as it was, as Linux leaves it.
    pub const WRITTEN: usize = RESERVED + FPSIMD_SIZE + ESR_SIZE + 8;
    /// What `rt_sigreturn` reads: all of that.
    pub const READ: usize = WRITTEN;
    /// The frame record above the frame.
    const RECORD: u64 = 16;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;

    /// The frame record 16-byte aligned below `top`, the frame below it.
    pub fn place(top: u64) -> u64 {
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
    fn record(frame: &[u8; READ], offset: usize) -> (u32, usize) {
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
}

#[cfg(target_arch = "riscv64")]
mod arch {
    //! `uc_mcontext` is `struct sigcontext { struct user_regs_struct sc_regs; union
    //! __riscv_fp_state sc_fpregs; }`, the registers `pc` then `x1` to `x31`, the FP part 528
    //! bytes: the D extension's `f0` to `f31` and `fcsr`, and at its end the header of the
    //! extensions that follow, here none (a reserved word and an end header, all zero).
    use super::generic::{MCONTEXT, SIGMASK, UCONTEXT};
    use super::{
        BadFrame, FpState, Handler, INFO_SIZE, STACK_SIZE, Saved, Trap, put, ucontext, word,
    };
    use crate::process::Process;
    use eggos_hal::Platform;
    use eggos_hal::user::UserRegisters;

    /// The whole frame: the `ucontext_t` is 960 bytes.
    const SIZE: u64 = (UCONTEXT + 960) as u64;
    const _: () = assert!(SIZE == 1088);
    /// What the kernel writes and reads: all of it.
    pub const WRITTEN: usize = SIZE as usize;
    pub const READ: usize = WRITTEN;
    /// No red zone in the ABI.
    pub const RED_ZONE: u64 = 0;
    /// The extensions' header at the end of `sc_fpregs`: a reserved word, then the first
    /// extension's magic number and size, 0 for none.
    const EXTENSIONS: usize = WRITTEN - 12;
    const _: () = assert!(EXTENSIONS == 1076);
    /// `struct __riscv_d_ext_state`: `f0` to `f31` (64 bits each), then `fcsr` (32 bits).
    pub type FpPart = [u8; 260];
    const FP_PART: usize = MCONTEXT + 256;
    const FCSR: usize = 256;
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
    pub fn place(top: u64) -> u64 {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_values() -> FpState {
        let mut state = FpState::new();
        #[cfg(target_arch = "x86_64")]
        {
            let mut image = *state.fxsave_image();
            image[24..28].copy_from_slice(&0x3f80u32.to_le_bytes());
            image[160 + 8 * 16] = 0x23;
            image[160 + 15 * 16 + 15] = 0x45;
            state.set_fxsave_image(&image).unwrap();
        }
        #[cfg(target_arch = "aarch64")]
        {
            state.v_mut()[8] = 0x0123_4567_89ab_cdef_0011_2233_4455_6677;
            state.v_mut()[31] = u128::MAX;
            state.set_fpcr_and_fpsr(1 << 22, 1);
        }
        #[cfg(target_arch = "riscv64")]
        {
            state.f_mut()[8] = 0x0123_4567_89ab_cdef;
            state.f_mut()[31] = u64::MAX;
            state.set_fcsr(2 << 5 | 1);
        }
        state
    }

    #[test]
    fn a_frame_s_fp_simd_part_loads_back_into_the_same_registers() {
        let part = fp_part(&state_with_values());
        assert_ne!(part, initial_fp_part());
        let mut loaded = FpState::new();
        load_fp_part(&part, &mut loaded).unwrap();
        assert_eq!(fp_part(&loaded), part);
    }

    #[test]
    fn the_initial_part_is_a_new_program_s_registers() {
        let part = initial_fp_part();
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(u16::from_ne_bytes([part[0], part[1]]), 0x037f);
            assert_eq!(u32::from_ne_bytes(part[24..28].try_into().unwrap()), 0x1f80);
            assert!(
                part.iter()
                    .enumerate()
                    .all(|(offset, &byte)| matches!(offset, 0 | 1 | 24 | 25) || byte == 0)
            );
        }
        #[cfg(not(target_arch = "x86_64"))]
        assert!(part.iter().all(|&byte| byte == 0));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn aarch64_s_part_is_fpsr_fpcr_then_the_v_registers() {
        let part = fp_part(&state_with_values());
        assert_eq!(part[..8], [1, 0, 0, 0, 0, 0, 0x40, 0]);
        assert_eq!(
            part[8 + 8 * 16..][..16],
            0x0123_4567_89ab_cdef_0011_2233_4455_6677u128.to_ne_bytes()
        );
    }

    #[cfg(target_arch = "riscv64")]
    #[test]
    fn riscv_s_part_is_the_f_registers_then_fcsr() {
        let part = fp_part(&state_with_values());
        assert_eq!(part[64..72], 0x0123_4567_89ab_cdefu64.to_ne_bytes());
        assert_eq!(part[256..], (2u32 << 5 | 1).to_ne_bytes());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x86_64_s_part_leaves_the_software_bytes_zero_and_refuses_reserved_mxcsr_bits() {
        let mut part = fp_part(&state_with_values());
        assert!(part[464..].iter().all(|&byte| byte == 0));
        part[24..28].copy_from_slice(&(0x1f80u32 | 1 << 16).to_le_bytes());
        let mut state = state_with_values();
        assert_eq!(load_fp_part(&part, &mut state), Err(BadFrame));
        assert_eq!(fp_part(&state), fp_part(&state_with_values()));
    }
}
