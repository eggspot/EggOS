//! Darwin's signal numbers, signal sets, `sigaction` flags, `siginfo_t`, `sigprocmask`'s and
//! `waitpid`'s arguments and wait statuses, and their Linux counterparts. Pure translation,
//! host-tested (D190).

use crate::darwin::linux_signal;

/// Darwin's highest signal number, `SIGUSR2`.
pub const LAST_SIGNAL: i32 = 31;
/// Darwin's `SIGCONT`, which a continued child's wait status names.
pub const SIGCONT: i32 = 19;
/// Darwin's `SIGSTOP`.
pub const SIGSTOP: i32 = 17;

const LINUX_SIGNALS: usize = 64;
const LINUX_SIGILL: i32 = 4;
const LINUX_SIGTRAP: i32 = 5;
const LINUX_SIGBUS: i32 = 7;
const LINUX_SIGFPE: i32 = 8;
const LINUX_SIGSEGV: i32 = 11;
const LINUX_SIGCHLD: i32 = 17;
const CLD_EXITED: i32 = 1;
const STOPPED: i32 = 0x7f;
const CORE_DUMPED: i32 = 0x80;
const LINUX_CONTINUED: i32 = 0xffff;

/// Darwin's `sa_flags` bits.
pub mod action {
    pub const ONSTACK: i32 = 0x1;
    pub const RESTART: i32 = 0x2;
    pub const RESETHAND: i32 = 0x4;
    pub const NOCLDSTOP: i32 = 0x8;
    pub const NODEFER: i32 = 0x10;
    pub const NOCLDWAIT: i32 = 0x20;
    pub const SIGINFO: i32 = 0x40;
}

/// Linux's `sa_flags` bits.
pub mod linux_action {
    pub const NOCLDSTOP: u64 = 0x1;
    pub const NOCLDWAIT: u64 = 0x2;
    pub const SIGINFO: u64 = 0x4;
    pub const ONSTACK: u64 = 0x0800_0000;
    pub const RESTART: u64 = 0x1000_0000;
    pub const NODEFER: u64 = 0x4000_0000;
    pub const RESETHAND: u64 = 0x8000_0000;
}

const ACTION_FLAGS: [(i32, u64); 7] = [
    (action::ONSTACK, linux_action::ONSTACK),
    (action::RESTART, linux_action::RESTART),
    (action::RESETHAND, linux_action::RESETHAND),
    (action::NOCLDSTOP, linux_action::NOCLDSTOP),
    (action::NODEFER, linux_action::NODEFER),
    (action::NOCLDWAIT, linux_action::NOCLDWAIT),
    (action::SIGINFO, linux_action::SIGINFO),
];

/// Darwin's number for Linux signal `linux`; `None` for the signals Darwin lacks
/// (`SIGSTKFLT`, `SIGPWR` and the real-time ones).
pub fn darwin_signal(linux: usize) -> Option<i32> {
    let darwin = match linux {
        1..=6 | 8 | 9 | 11 | 13..=15 | 21 | 22 | 24..=28 => linux as i32,
        7 => 10,
        10 => 30,
        12 => 31,
        17 => 20,
        18 => 19,
        19 => 17,
        20 => 18,
        23 => 16,
        29 => 23,
        31 => 12,
        _ => return None,
    };
    Some(darwin)
}

/// Linux's `sigset_t` word for Darwin's `sigset_t`; Darwin's `SIGEMT` and `SIGINFO`, which
/// Linux lacks, drop out.
pub fn linux_signal_set(darwin: u32) -> u64 {
    (1..=LAST_SIGNAL)
        .filter(|signal| darwin & (1 << (signal - 1)) != 0)
        .filter_map(linux_signal)
        .fold(0, |set, linux| set | (1 << (linux - 1)))
}

/// Darwin's `sigset_t` for Linux's first `sigset_t` word.
pub fn darwin_signal_set(linux: u64) -> u32 {
    (1..=LINUX_SIGNALS)
        .filter(|signal| linux & (1 << (signal - 1)) != 0)
        .filter_map(darwin_signal)
        .fold(0, |set, darwin| set | (1 << (darwin - 1)))
}

/// Linux's `sa_flags` for Darwin's; bits Darwin does not define are dropped.
pub fn linux_action_flags(darwin: i32) -> u64 {
    ACTION_FLAGS
        .iter()
        .filter(|(flag, _)| darwin & flag != 0)
        .fold(0, |flags, (_, linux)| flags | linux)
}

/// Linux's `how` for Darwin's `SIG_BLOCK` (1), `SIG_UNBLOCK` (2) and `SIG_SETMASK` (3).
pub fn linux_mask_how(darwin: i32) -> Option<usize> {
    match darwin {
        1 => Some(0),
        2 => Some(1),
        3 => Some(2),
        _ => None,
    }
}

/// Linux's `wait4` options for Darwin's `WNOHANG` (1), `WUNTRACED` (2) and `WCONTINUED`
/// (0x10); `None` for any other bit.
pub fn linux_wait_options(darwin: i32) -> Option<usize> {
    const NOHANG: i32 = 1;
    const UNTRACED: i32 = 2;
    const CONTINUED: i32 = 0x10;
    const LINUX_CONTINUED_OPTION: usize = 8;
    if darwin & !(NOHANG | UNTRACED | CONTINUED) != 0 {
        return None;
    }
    let continued = if darwin & CONTINUED != 0 {
        LINUX_CONTINUED_OPTION
    } else {
        0
    };
    Some((darwin & (NOHANG | UNTRACED)) as usize | continued)
}

/// Darwin's wait status for Linux's: the same encoding, with the signal that ended or stopped
/// the child in Darwin's numbers, and a continued child as Darwin reports it (stopped by
/// `SIGCONT`).
pub fn darwin_wait_status(linux: i32) -> i32 {
    let signal = |number: i32| darwin_signal(number as usize).unwrap_or(number);
    if linux & 0xffff == LINUX_CONTINUED {
        return (SIGCONT << 8) | STOPPED;
    }
    match linux & 0x7f {
        0 => linux & 0xff00,
        STOPPED => (signal((linux >> 8) & 0xff) << 8) | STOPPED,
        ended => signal(ended) | (linux & CORE_DUMPED),
    }
}

/// Darwin's `siginfo_t`, which a handler installed with `SA_SIGINFO` reads.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SignalInfo {
    pub signal: i32,
    pub errno: i32,
    pub code: i32,
    pub pid: i32,
    pub uid: u32,
    pub status: i32,
    pub address: u64,
    pub value: u64,
    pub band: i64,
    pub reserved: [u64; 7],
}

/// How many bytes of Linux's `siginfo_t` [`darwin_signal_info`] reads.
pub const LINUX_INFO_READ: usize = 32;

/// Darwin's `si_code` values for signals a process sent.
pub mod code {
    pub const USER: i32 = 0x10001;
    pub const QUEUE: i32 = 0x10002;
    pub const TIMER: i32 = 0x10003;
    pub const ASYNC_IO: i32 = 0x10004;
    pub const MESSAGE_QUEUE: i32 = 0x10005;
}

fn word32(bytes: &[u8; LINUX_INFO_READ], at: usize) -> i32 {
    i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn word64(bytes: &[u8; LINUX_INFO_READ], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn sender_code(linux: i32) -> i32 {
    match linux {
        -1 => code::QUEUE,
        -2 => code::TIMER,
        -3 => code::MESSAGE_QUEUE,
        -4 => code::ASYNC_IO,
        _ => code::USER,
    }
}

fn fault_code(linux_signal: i32, linux: i32) -> i32 {
    const ILLEGAL: [i32; 8] = [1, 4, 5, 2, 3, 6, 7, 8];
    const ARITHMETIC: [i32; 8] = [7, 8, 1, 2, 3, 4, 5, 6];
    let table = match linux_signal {
        LINUX_SIGILL => &ILLEGAL,
        LINUX_SIGFPE => &ARITHMETIC,
        _ => return linux,
    };
    usize::try_from(linux - 1)
        .ok()
        .and_then(|index| table.get(index))
        .copied()
        .unwrap_or(linux)
}

/// Darwin's `siginfo_t` for the first [`LINUX_INFO_READ`] bytes of Linux's: a fault's address
/// and code, a child's pid, user and status, or the sender's pid, user and value.
pub fn darwin_signal_info(linux: &[u8; LINUX_INFO_READ]) -> SignalInfo {
    let linux_signal = word32(linux, 0);
    let linux_code = word32(linux, 8);
    let mut info = SignalInfo {
        signal: darwin_signal(linux_signal as usize).unwrap_or(0),
        errno: word32(linux, 4),
        ..SignalInfo::default()
    };
    let fault = matches!(
        linux_signal,
        LINUX_SIGILL | LINUX_SIGTRAP | LINUX_SIGBUS | LINUX_SIGFPE | LINUX_SIGSEGV
    );
    if fault && linux_code > 0 {
        info.code = fault_code(linux_signal, linux_code);
        info.address = word64(linux, 16);
        return info;
    }
    info.pid = word32(linux, 16);
    info.uid = word32(linux, 20) as u32;
    if linux_signal == LINUX_SIGCHLD && linux_code > 0 {
        let status = word32(linux, 24);
        info.code = linux_code;
        info.status = if linux_code == CLD_EXITED {
            status
        } else {
            darwin_signal(status as usize).unwrap_or(status)
        };
        return info;
    }
    info.code = sender_code(linux_code);
    info.value = word64(linux, 24);
    info
}
