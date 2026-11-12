//! RV32's Linux ABI (M9, D484): RV64's ([`super::riscv64`]: the generic table, flags, `stat`,
//! fault classification and test support), with the machine named `riscv32` and the calls a
//! 32-bit program makes for 64-bit time in place of the ones RV32 Linux leaves out. RV32 has no
//! 32-bit-time calls at all, so the generic numbers of those calls (`clock_gettime`, `futex`,
//! `ppoll` and the rest) name nothing here and their `time64` numbers take their place: the same
//! arguments with 64-bit `timespec`s, which is what the kernel's handlers read already.

use super::{code_bytes, generic};

#[allow(dead_code)]
#[path = "riscv64.rs"]
mod rv64;

pub use rv64::{
    CLONE_CHILD_TID_BEFORE_TLS, ELF_MACHINE, EPOLL_EVENT_SIZE, MIN_SIGNAL_STACK_SIZE,
    RUNS_WINDOWS_AND_MACOS_PROGRAMS, SIGACTION_HAS_RESTORER, SIGRETURN_TRAMPOLINE, fault, mmap,
    open, signal_frame, stat, tty, wants,
};

/// The architecture, as `uname` names it.
pub const MACHINE: &str = "riscv32";

/// Linux's system-call numbers on RV32: the generic table, with the `time64` calls in place of
/// the 32-bit-time ones RV32 leaves out.
pub mod number {
    pub use crate::linux::abi::generic::number::*;

    pub const CLOCK_GETTIME: u64 = 403;
    pub const CLOCK_SETTIME: u64 = 404;
    pub const CLOCK_GETRES: u64 = 406;
    pub const CLOCK_NANOSLEEP: u64 = 407;
    pub const TIMERFD_GETTIME: u64 = 410;
    pub const TIMERFD_SETTIME: u64 = 411;
    pub const UTIMENSAT: u64 = 412;
    pub const PSELECT6: u64 = 413;
    pub const PPOLL: u64 = 414;
    pub const RT_SIGTIMEDWAIT: u64 = 421;
    pub const FUTEX: u64 = 422;
}
