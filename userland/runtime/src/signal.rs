//! Linux's signal numbers: the generic ones every port uses but MIPS, whose own these are
//! there (D315). Real-time signals start at the same number everywhere.

/// `SIGRTMIN` as C libraries number it, the first two real-time signals being theirs.
pub const SIGRTMIN: u32 = 34;

/// `SIG_IGN`.
pub const IGNORE: usize = 1;

/// The `sigsetsize` the calls take: 8 bytes, or MIPS's 16, whose first 8 hold the 64 signals
/// the kernel has (D315).
#[cfg(not(target_arch = "mips64"))]
pub const SET_BYTES: usize = 8;
#[cfg(target_arch = "mips64")]
pub const SET_BYTES: usize = 16;

/// A `struct sigaction` with `handler`, no flags, no restorer and an empty mask, in the
/// architecture's layout: the handler first, or MIPS's flags first (D315).
pub const fn action(handler: usize) -> [usize; 4] {
    match cfg!(target_arch = "mips64") {
        true => [0, handler, 0, 0],
        false => [handler, 0, 0, 0],
    }
}

#[cfg(not(target_arch = "mips64"))]
mod numbers {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGKILL: u32 = 9;
    pub const SIGUSR1: u32 = 10;
    pub const SIGUSR2: u32 = 12;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGCHLD: u32 = 17;
    pub const SIGCONT: u32 = 18;
    pub const SIGSTOP: u32 = 19;
    pub const SIGTSTP: u32 = 20;
}

#[cfg(target_arch = "mips64")]
mod numbers {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGKILL: u32 = 9;
    pub const SIGUSR1: u32 = 16;
    pub const SIGUSR2: u32 = 17;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGCHLD: u32 = 18;
    pub const SIGCONT: u32 = 25;
    pub const SIGSTOP: u32 = 23;
    pub const SIGTSTP: u32 = 24;
}

pub use numbers::*;
