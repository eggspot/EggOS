//! Faults delivered as signals (M6, D65): what Linux tells a program about an exception its
//! code raised, a page fault it cannot resolve, an illegal instruction, a breakpoint, a
//! misaligned access, a division by zero.
//!
//! [`classify`] turns the exception into the signal Linux sends for it, the `siginfo_t`'s
//! `si_code` and `si_addr`, and what the signal frame's machine context reports of it
//! ([`Trap`]): x86_64's `trapno`, `err` and `cr2`, AArch64's `fault_address` and
//! `esr_context`; RISC-V's frame has nothing of the kind. The signal is then forced, as Linux
//! does for a fault: a program that blocks or ignores it dies of it
//! ([`super::signals::deliver_fault`]).
//!
//! Each architecture's rules are Linux's observed behaviour, as researched for D65; where the
//! sources disagreed, sigaction(2)'s words decide (`si_addr` is the faulting memory address for
//! `SIGSEGV` and `SIGBUS`, the instruction's for `SIGILL`, `SIGFPE` and `SIGTRAP`). Whether a
//! page fault is `SEGV_MAPERR` or `SEGV_ACCERR` depends on the process's regions alone: an
//! address in one of them whose protection forbids the access is `SEGV_ACCERR`, any other
//! (the kernel's half included) `SEGV_MAPERR`. Nothing the kernel reports for an address
//! outside user space depends on the kernel's own page tables, so a program cannot learn
//! from its faults what the kernel has mapped.

use super::signal;
use crate::process::Process;
use eggos_hal::paging::Format;
use eggos_hal::{Exception, Platform};

/// `si_code`s for faults (sigaction(2)): each signal numbers its own from 1.
pub mod code {
    /// `SIGSEGV`: the address is in no mapping.
    pub const SEGV_MAPERR: i32 = 1;
    /// `SIGSEGV`: the address's mapping does not allow the access.
    pub const SEGV_ACCERR: i32 = 2;
    /// `SIGBUS`: a misaligned address.
    pub const BUS_ADRALN: i32 = 1;
    /// `SIGBUS`: a hardware error at the address.
    pub const BUS_OBJERR: i32 = 3;
    /// `SIGILL`: an illegal opcode.
    pub const ILL_ILLOPC: i32 = 1;
    /// `SIGILL`: an illegal operand (x86_64's `#UD`).
    pub const ILL_ILLOPN: i32 = 2;
    /// `SIGFPE`: integer division by zero (or overflow, which x86_64 cannot tell apart).
    pub const FPE_INTDIV: i32 = 1;
    /// `SIGTRAP`: a breakpoint.
    pub const TRAP_BRKPT: i32 = 1;
    /// Any signal: raised by the kernel, with no address.
    pub const SI_KERNEL: i32 = 0x80;
}

/// A fault, as the signal Linux sends for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub signal: u32,
    /// The `siginfo_t`'s `si_code` ([`code`]).
    pub code: i32,
    /// Its `si_addr`.
    pub address: u64,
    pub trap: Trap,
}

/// What a fault's signal frame reports of the exception in its machine context, as Linux
/// does. Signals that are not faults report zeros (Linux reports the last fault's, stale).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trap {
    /// x86_64's `trapno`: the vector.
    pub number: u64,
    /// x86_64's `err` (the error code the CPU pushed) and AArch64's `esr_context` (`ESR_EL1`),
    /// each with nothing about the kernel's mappings for an address outside user space.
    pub cause: u64,
    /// x86_64's `cr2` (page faults only) and AArch64's `fault_address` (`si_addr`).
    pub address: u64,
}

/// The fault `exception` in the program of `process`, whose stack pointer was `stack`: see the
/// module documentation. A page fault here is one the process did not resolve.
#[cold]
pub fn classify<P: Platform>(process: &Process<P>, exception: &Exception, stack: u64) -> Fault {
    let in_user_space = |address: u64| address < <P::PageTableFormat as Format>::USER_END;
    let segv = |address: u64| {
        let code = if in_user_space(address) && process.region(address).is_some() {
            code::SEGV_ACCERR
        } else {
            code::SEGV_MAPERR
        };
        (signal::SIGSEGV, code, address)
    };
    arch::classify(exception, stack, in_user_space, segv)
}

/// Builds a [`Fault`].
fn fault((signal, code, address): (u32, i32, u64), trap: Trap) -> Fault {
    Fault {
        signal,
        code,
        address,
        trap,
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// The page-fault error code's protection bit: set for every address outside user space,
    /// mapped or not, as Linux does.
    const PAGE_FAULT_PROTECTION: u64 = 1 << 0;

    /// `trapno`: the vector of an exception of `kind`. The HAL's kinds tell every vector a
    /// program can raise apart but `#MF` and `#XM` from `#DE` (reported as 0: they cannot
    /// happen while programs have no FP/SIMD registers, D49), `#NP` from `#SS` (12), and
    /// `#NM` from `#UD` (6: FP/SIMD use, which D54 turns into a first use, not a fault).
    fn vector(kind: ExceptionKind) -> u64 {
        match kind {
            ExceptionKind::Arithmetic => 0,
            ExceptionKind::Debug => 1,
            ExceptionKind::Breakpoint => 3,
            ExceptionKind::IllegalInstruction => 6,
            ExceptionKind::Segment => 12,
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => 14,
            ExceptionKind::Alignment => 17,
            // `#GP`, and the vectors no program raises.
            ExceptionKind::Interrupt | ExceptionKind::Other => 13,
        }
    }

    pub fn classify(
        exception: &Exception,
        _stack: u64,
        in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        let mut trap = Trap {
            number: vector(exception.kind),
            cause: exception.code,
            address: 0,
        };
        let (signal, code, address) = match exception.kind {
            // Only `#PF`s are faults on memory.
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => {
                let address = exception.address.unwrap_or(0);
                if !in_user_space(address) {
                    trap.cause |= PAGE_FAULT_PROTECTION;
                }
                trap.address = address;
                segv(address)
            }
            // `int3`: the kernel's, with no address; `rip` is past it already.
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::SI_KERNEL, 0),
            ExceptionKind::IllegalInstruction => (signal::SIGILL, code::ILL_ILLOPN, pc),
            // `#AC`: no address.
            ExceptionKind::Alignment => (signal::SIGBUS, code::BUS_ADRALN, 0),
            // `#DE` (`#MF` and `#XM` take their `si_code` from the FP status with D54).
            ExceptionKind::Arithmetic => (signal::SIGFPE, code::FPE_INTDIV, pc),
            // `int1` (`icebp`): programs cannot set the trap flag or debug registers yet.
            ExceptionKind::Debug => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            // `#NP` and `#SS`.
            ExceptionKind::Segment => (signal::SIGBUS, code::SI_KERNEL, 0),
            // `#GP` (a privileged instruction, a non-canonical address) and the rest.
            ExceptionKind::Interrupt | ExceptionKind::Other => {
                (signal::SIGSEGV, code::SI_KERNEL, 0)
            }
        };
        fault((signal, code, address), trap)
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    /// Exception classes (`ESR_EL1.EC`).
    const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
    const EC_INSTRUCTION_ABORT: u64 = 0x21;
    const EC_DATA_ABORT_LOWER: u64 = 0x24;
    const EC_DATA_ABORT: u64 = 0x25;
    const EC_SP_ALIGNMENT: u64 = 0x26;
    /// `ESR_EL1` bits a sanitized syndrome keeps: the class and the instruction length, and
    /// for a data abort, write-not-read and cache maintenance.
    const ESR_CLASS_AND_LENGTH: u64 = 0x3f << 26 | 1 << 25;
    const ESR_DATA_ABORT_KEPT: u64 = 1 << 6 | 1 << 8;
    /// The fault status code: an alignment fault, a level 0 translation fault.
    const STATUS_ALIGNMENT: u64 = 0b10_0001;
    const STATUS_TRANSLATION_LEVEL_0: u64 = 0b00_0100;

    /// Whether fault status code `status` is a synchronous external abort or a parity or ECC
    /// error, on the access itself or on a table walk for it at levels 0 to 3 (0b010001 is
    /// MTE's tag check fault, not one).
    fn external_abort(status: u64) -> bool {
        matches!(status, 0b01_0000 | 0b01_0100..=0b01_1000 | 0b01_1100..=0b01_1111)
    }

    /// The syndrome a program sees for a fault at an address outside user space: the class
    /// and instruction length, and for an abort a level 0 translation fault (with, for a data
    /// abort, whether it wrote), whatever the kernel's tables hold there; 0 for any other
    /// class.
    pub fn sanitized(esr: u64) -> u64 {
        let class = esr >> 26 & 0x3f;
        match class {
            EC_INSTRUCTION_ABORT_LOWER | EC_INSTRUCTION_ABORT => {
                esr & ESR_CLASS_AND_LENGTH | STATUS_TRANSLATION_LEVEL_0
            }
            EC_DATA_ABORT_LOWER | EC_DATA_ABORT => {
                esr & (ESR_CLASS_AND_LENGTH | ESR_DATA_ABORT_KEPT) | STATUS_TRANSLATION_LEVEL_0
            }
            _ => 0,
        }
    }

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
                // The FAR, when the abort says it is valid.
                let address = exception.address.unwrap_or(0);
                match esr & 0x3f {
                    STATUS_ALIGNMENT => (signal::SIGBUS, code::BUS_ADRALN, address),
                    status if external_abort(status) => (signal::SIGBUS, code::BUS_OBJERR, address),
                    _ => segv(address),
                }
            }
            // The FAR is unknown after an SP alignment fault: the stack pointer it had.
            ExceptionKind::Alignment if esr >> 26 & 0x3f == EC_SP_ALIGNMENT => {
                (signal::SIGBUS, code::BUS_ADRALN, stack)
            }
            // A PC alignment fault: the misaligned PC, in the FAR.
            ExceptionKind::Alignment => (
                signal::SIGBUS,
                code::BUS_ADRALN,
                exception.address.unwrap_or(pc),
            ),
            // `brk`: the PC stays on it, so a handler that returns runs it again.
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            // Undefined instructions, the FP/SIMD trap, and every other class from EL0 (the
            // HAL reports no arithmetic, debug or segment exceptions on AArch64).
            ExceptionKind::IllegalInstruction
            | ExceptionKind::Arithmetic
            | ExceptionKind::Debug
            | ExceptionKind::Segment
            | ExceptionKind::Interrupt
            | ExceptionKind::Other => (signal::SIGILL, code::ILL_ILLOPC, pc),
        };
        let reported = match exception.address {
            Some(far) if !in_user_space(far) => sanitized(esr),
            _ => esr,
        };
        let trap = Trap {
            number: 0,
            cause: reported,
            address,
        };
        fault((signal, code, address), trap)
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    use super::{Fault, Trap, code, fault};
    use crate::linux::signal;
    use eggos_hal::{Exception, ExceptionKind};

    pub fn classify(
        exception: &Exception,
        _stack: u64,
        _in_user_space: impl Fn(u64) -> bool,
        segv: impl Fn(u64) -> (u32, i32, u64),
    ) -> Fault {
        let pc = exception.pc;
        // `stval`, for the faults that set it.
        let address = exception.address.unwrap_or(0);
        let reported = match exception.kind {
            // Page faults and access faults alike: by the process's regions.
            ExceptionKind::DataFault | ExceptionKind::InstructionFault => segv(address),
            // Misaligned accesses the kernel does not emulate.
            ExceptionKind::Alignment => (signal::SIGBUS, code::BUS_ADRALN, address),
            // `ebreak`: the PC stays on it, so a handler that returns runs it again.
            ExceptionKind::Breakpoint => (signal::SIGTRAP, code::TRAP_BRKPT, pc),
            // Illegal instructions and every other cause from U-mode (the HAL reports no
            // arithmetic, debug or segment exceptions on RISC-V).
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

#[cfg(all(test, target_arch = "aarch64"))]
pub use arch::sanitized as sanitized_syndrome;

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::sanitized_syndrome;

    #[test]
    fn syndromes_for_kernel_addresses_say_nothing_of_the_kernel_s_tables() {
        // A write's permission fault at level 3 (data abort from EL0): a level 0 translation
        // fault that wrote, with its class and length.
        assert_eq!(sanitized_syndrome(0x9200_004f), 0x9200_0044);
        // An instruction abort's access flag fault at level 2.
        assert_eq!(sanitized_syndrome(0x8200_000a), 0x8200_0004);
        // A read's translation fault, other ISS bits dropped (the FAR-not-valid bit too).
        assert_eq!(sanitized_syndrome(0x9200_0405), 0x9200_0004);
        // Any other class: nothing.
        assert_eq!(sanitized_syndrome(0xf200_0000), 0);
    }
}
