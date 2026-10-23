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
//! Programs get no FP/SIMD registers until they are saved per thread (D54), but every frame
//! has its FP/SIMD part in Linux's shape all the same, holding the architecture's initial
//! state, which is all a program can have: x86_64's `FXSAVE` image below the frame, AArch64's
//! `fpsimd_context` record and RISC-V's D-extension state. Programs and crash reporters that
//! read them find them, and saving the real registers later changes only what is written.
//! `rt_sigreturn` checks the records' shape and ignores their values.

use super::signals::Origin;
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::user::UserRegisters;

/// The size of a `siginfo_t`.
pub const INFO_SIZE: usize = 128;

/// The size of a `stack_t`.
pub const STACK_SIZE: usize = 24;

/// `si_code` for a signal sent with `kill` (`SI_USER`).
const SI_USER: i32 = 0;
/// `si_code` for a signal sent with `tkill` or `tgkill` (`SI_TKILL`).
const SI_TKILL: i32 = -6;
/// `si_code` for a signal the kernel raised (`SI_KERNEL`).
const SI_KERNEL: i32 = 0x80;

/// The `siginfo_t` of `signal` from `origin`, sent by process `pid` (ignored for the kernel's):
/// `si_signo`, `si_errno` 0 and `si_code`, then the sender's id and user id (0: processes have
/// no credentials yet, and every process is root's until they do).
pub fn info(signal: u32, origin: Origin, pid: u32) -> [u8; INFO_SIZE] {
    let mut info = [0; INFO_SIZE];
    let code = match origin {
        Origin::Process => SI_USER,
        Origin::Thread => SI_TKILL,
        Origin::Kernel => SI_KERNEL,
    };
    info[..4].copy_from_slice(&(signal as i32).to_ne_bytes());
    info[8..12].copy_from_slice(&code.to_ne_bytes());
    if origin != Origin::Kernel {
        info[16..20].copy_from_slice(&pid.to_ne_bytes());
    }
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
}

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
/// program's next instruction is the handler's first. `saved` is what the frame keeps. Its
/// padding and unused fields are written as zeros (but AArch64's `__reserved` after the
/// terminating record, which Linux leaves as it was too).
pub fn write<P: Platform>(
    process: &Process<P>,
    top: u64,
    saved: &Saved,
    info: &[u8; INFO_SIZE],
    handler: &Handler,
    registers: &mut UserRegisters,
) -> Result<(), BadFrame> {
    let (address, bytes) = arch::build(top, saved, info, handler.restorer);
    process
        .write_user_memory(address, &bytes)
        .map_err(|_| BadFrame)?;
    arch::write_extra(process, top, address, saved).map_err(|_| BadFrame)?;
    arch::enter(registers, address, handler);
    Ok(())
}

/// How far below the interrupted stack pointer a frame starts, unless it goes on the
/// alternate stack: the ABI's red zone (x86_64's 128 bytes), which the interrupted code may be
/// using.
pub const RED_ZONE: u64 = arch::RED_ZONE;

/// The lowest address of the frame [`write`] puts below `top`: where the stack pointer must
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
    arch::parse(&bytes, registers).ok_or(BadFrame)
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
    use super::{Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word};
    use eggos_hal::user::UserRegisters;

    const UCONTEXT: usize = 8;
    const MCONTEXT: usize = UCONTEXT + 40;
    const SIGMASK: usize = UCONTEXT + 296;
    const INFO: usize = UCONTEXT + 304;
    /// The whole frame.
    pub const WRITTEN: usize = INFO + INFO_SIZE;
    const _: () = assert!(WRITTEN == 440);
    /// What `rt_sigreturn` reads: the frame, not the FP/SIMD image its `fpstate` points at,
    /// whose values it ignores (any pointer is accepted, null included, as on Linux).
    pub const READ: usize = WRITTEN;

    /// The `FXSAVE` image (`struct _fpstate`): 512 bytes, 64-byte aligned, above the frame.
    const FP_SIZE: u64 = 512;
    /// Its initial state, as `FNINIT` and the reset value of `MXCSR` leave it: the x87 control
    /// word 0x037f (every exception masked, double-extended precision, round to nearest) and
    /// `MXCSR` 0x1f80 (every SSE exception masked, round to nearest); everything else zero,
    /// the software-reserved bytes too (no `XSAVE` state follows).
    const FP_CONTROL_WORD: u16 = 0x037f;
    const FP_MXCSR: u32 = 0x1f80;

    /// `uc_flags`: `UC_SIGCONTEXT_SS` and `UC_STRICT_RESTORE_SS`, as Linux sets them for
    /// every 64-bit frame (the saved `ss` is the real one).
    const FLAGS: u64 = 0x2 | 0x4;

    /// `struct sigcontext`'s fields, from `uc_mcontext`, in the order of `regs`' below.
    const REGISTERS: usize = 18;
    const SELECTORS: usize = 144;
    const OLDMASK: usize = 168;
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
        restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        put(&mut frame, 0, restorer);
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
    pub fn write_extra<P: eggos_hal::Platform>(
        process: &crate::process::Process<P>,
        top: u64,
        _frame: u64,
        _saved: &Saved,
    ) -> Result<(), crate::process::InvalidUserAddress> {
        let mut image = [0; FP_SIZE as usize];
        image[..2].copy_from_slice(&FP_CONTROL_WORD.to_ne_bytes());
        image[24..28].copy_from_slice(&FP_MXCSR.to_ne_bytes());
        process.write_user_memory(fp_image(top), &image)
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
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.rax
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
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
    //! size: here the FP/SIMD one (`fpsimd_context`), which Linux requires, then the
    //! terminating null one. Above the frame is a frame record (`x29`, `x30`) that `x29`
    //! points at, so a backtrace from the handler walks into the interrupted code.
    use super::generic::{MCONTEXT, SIGMASK, UCONTEXT};
    use super::{Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word};
    use eggos_hal::user::UserRegisters;

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
    /// `fpsimd_context`: its magic number and size, then `fpsr`, `fpcr` and `v0` to `v31`,
    /// all zero in the initial state.
    const FPSIMD_MAGIC: u32 = 0x4650_8001;
    const FPSIMD_SIZE: usize = 528;
    /// `esr_context`, which Linux adds after a fault: skipped when read back.
    const ESR_MAGIC: u32 = 0x4553_5201;
    const ESR_SIZE: usize = 16;
    /// What the kernel writes of the frame: up to the terminating record (8 bytes, zero).
    /// The rest of `__reserved` is left as it was, as Linux leaves it.
    pub const WRITTEN: usize = RESERVED + FPSIMD_SIZE + 8;
    /// What `rt_sigreturn` reads: room for an `esr_context` too.
    pub const READ: usize = WRITTEN + ESR_SIZE;
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
        _restorer: u64,
    ) -> (u64, [u8; WRITTEN]) {
        let mut frame = [0; WRITTEN];
        frame[..INFO_SIZE].copy_from_slice(info);
        frame[UCONTEXT + ucontext::STACK..][..STACK_SIZE].copy_from_slice(&saved.alternate_stack);
        put(&mut frame, SIGMASK, saved.blocked);
        let registers = &saved.registers;
        for (index, &value) in registers.x.iter().enumerate() {
            put(&mut frame, REGS + index * 8, value);
        }
        put(&mut frame, SP, registers.sp);
        put(&mut frame, PC, registers.pc);
        put(&mut frame, PSTATE, registers.pstate);
        frame[RESERVED..][..4].copy_from_slice(&FPSIMD_MAGIC.to_ne_bytes());
        frame[RESERVED + 4..][..4].copy_from_slice(&(FPSIMD_SIZE as u32).to_ne_bytes());
        (place(top), frame)
    }

    /// The frame record: the interrupted `x29` and `x30`.
    pub fn write_extra<P: eggos_hal::Platform>(
        process: &crate::process::Process<P>,
        _top: u64,
        frame: u64,
        saved: &Saved,
    ) -> Result<(), crate::process::InvalidUserAddress> {
        let mut record = [0; RECORD as usize];
        put(&mut record, 0, saved.registers.x[29]);
        put(&mut record, 8, saved.registers.x[30]);
        process.write_user_memory(frame + SIZE, &record)
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
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.x[0]
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    //! `uc_mcontext` is `struct sigcontext { struct user_regs_struct sc_regs; union
    //! __riscv_fp_state sc_fpregs; }`, the registers `pc` then `x1` to `x31`, the FP part 528
    //! bytes: the D extension's `f0` to `f31` and `fcsr`, all zero in the initial state, and
    //! at its end the header of the extensions that follow, here none (a reserved word and
    //! an end header, all zero).
    use super::generic::{MCONTEXT, SIGMASK, UCONTEXT};
    use super::{Handler, INFO_SIZE, STACK_SIZE, Saved, put, ucontext, word};
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

    /// 16-byte aligned below `top`.
    pub fn place(top: u64) -> u64 {
        top.wrapping_sub(SIZE) & !15
    }

    pub fn build(
        top: u64,
        saved: &Saved,
        info: &[u8; INFO_SIZE],
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
        (place(top), frame)
    }

    /// Nothing outside the frame.
    pub fn write_extra<P: eggos_hal::Platform>(
        _: &crate::process::Process<P>,
        _: u64,
        _: u64,
        _: &Saved,
    ) -> Result<(), crate::process::InvalidUserAddress> {
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
        })
    }

    pub fn result(registers: &UserRegisters) -> u64 {
        registers.x[10]
    }

    #[cfg(test)]
    pub mod layout {
        pub const SIGMASK: usize = super::SIGMASK;
        pub const UCONTEXT: usize = super::UCONTEXT;
    }
}
