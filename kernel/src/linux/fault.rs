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
use crate::linux::abi::fault as arch;
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
    /// `SIGBUS`: an address with no backing, a page of a file mapping past the file's end.
    pub const BUS_ADRERR: i32 = 2;
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
    /// each with nothing about the kernel's mappings for an address outside user space. On
    /// AArch64, only aborts and alignment faults report one: 0 (no `esr_context`) for the rest.
    pub cause: u64,
    /// x86_64's `cr2` (page faults only) and AArch64's `fault_address` (`si_addr` for aborts
    /// and alignment faults, 0 for the rest, as on Linux).
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

/// The fault of a page fault `exception` at a page of a file mapping past the file's end, or
/// whose file could not be read ([`crate::process::UnresolvedFault::BusError`], D167):
/// `SIGBUS` with `BUS_ADRERR` at the faulting address, the machine context reporting the page
/// fault as for `SIGSEGV`.
#[cold]
pub fn bus_error<P: Platform>(process: &Process<P>, exception: &Exception, stack: u64) -> Fault {
    Fault {
        signal: signal::SIGBUS,
        code: code::BUS_ADRERR,
        ..classify(process, exception, stack)
    }
}

/// Builds a [`Fault`].
pub fn fault((signal, code, address): (u32, i32, u64), trap: Trap) -> Fault {
    Fault {
        signal,
        code,
        address,
        trap,
    }
}
