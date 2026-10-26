//! How a program ended, as `wait4` says it, and what the shell makes of it: `$?` is the exit
//! status, or 128 plus the signal that ended it.

/// `$?` for a command that was not found.
pub const NOT_FOUND: i32 = 127;

/// `$?` for a command that was found but could not run.
pub const NOT_EXECUTABLE: i32 = 126;

/// `$?` after a syntax error or a builtin's wrong usage.
pub const USAGE: i32 = 2;

/// `SIGINT`'s number on every architecture.
pub const SIGINT: u8 = 2;

const SIGPIPE: u8 = 13;

/// How a program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this status.
    Exited(u8),
    /// This signal ended it.
    Killed(u8),
}

impl Ended {
    /// How `wait4`'s `status` says the program ended.
    pub fn from_wait_status(status: i32) -> Self {
        match (status & 0x7f) as u8 {
            0 => Self::Exited((status >> 8) as u8),
            signal => Self::Killed(signal),
        }
    }

    /// `$?` for it.
    pub fn shell_status(self) -> i32 {
        match self {
            Self::Exited(status) => i32::from(status),
            Self::Killed(signal) => 128 + i32::from(signal),
        }
    }

    /// What the shell writes when a program ends this way: nothing for an exit or `SIGPIPE`,
    /// a newline for `SIGINT` (the terminal already shows `^C`), the signal's description for
    /// the others.
    pub fn report(self) -> &'static [u8] {
        match self {
            Self::Exited(_) | Self::Killed(SIGPIPE) => b"",
            Self::Killed(SIGINT) => b"\n",
            Self::Killed(signal) => description(signal),
        }
    }
}

fn description(signal: u8) -> &'static [u8] {
    match signal {
        1 => b"Hangup\n",
        3 => b"Quit\n",
        4 => b"Illegal instruction\n",
        5 => b"Trace/breakpoint trap\n",
        6 => b"Aborted\n",
        7 => b"Bus error\n",
        8 => b"Floating point exception\n",
        9 => b"Killed\n",
        10 => b"User defined signal 1\n",
        11 => b"Segmentation fault\n",
        12 => b"User defined signal 2\n",
        14 => b"Alarm clock\n",
        15 => b"Terminated\n",
        16 => b"Stack fault\n",
        24 => b"CPU time limit exceeded\n",
        25 => b"File size limit exceeded\n",
        26 => b"Virtual timer expired\n",
        27 => b"Profiling timer expired\n",
        29 => b"I/O possible\n",
        30 => b"Power failure\n",
        31 => b"Bad system call\n",
        _ => b"Killed by a signal\n",
    }
}

/// The exit status `exit` gives for its argument: its value modulo 256, `None` if it is not a
/// decimal number.
pub fn parse_exit_status(text: &[u8]) -> Option<i32> {
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u8 = 0;
    for &digit in digits {
        value = value.wrapping_mul(10).wrapping_add(digit - b'0');
    }
    Some(i32::from(if negative {
        value.wrapping_neg()
    } else {
        value
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_statuses_and_signals_become_shell_statuses() {
        let exited = Ended::from_wait_status(3 << 8);
        assert_eq!(exited, Ended::Exited(3));
        assert_eq!(exited.shell_status(), 3);
        assert_eq!(exited.report(), b"");
        let terminated = Ended::from_wait_status(15);
        assert_eq!(terminated, Ended::Killed(15));
        assert_eq!(terminated.shell_status(), 143);
        assert_eq!(terminated.report(), b"Terminated\n");
        assert_eq!(Ended::from_wait_status(2).shell_status(), 130);
        assert_eq!(Ended::from_wait_status(2).report(), b"\n");
        assert_eq!(Ended::from_wait_status(13).report(), b"");
        assert_eq!(Ended::from_wait_status(9).report(), b"Killed\n");
        assert_eq!(
            Ended::from_wait_status(11).report(),
            b"Segmentation fault\n"
        );
        assert_eq!(
            Ended::from_wait_status(40).report(),
            b"Killed by a signal\n"
        );
        assert_eq!(Ended::from_wait_status(255 << 8), Ended::Exited(255));
    }

    #[test]
    fn exit_takes_its_status_modulo_256() {
        assert_eq!(parse_exit_status(b"0"), Some(0));
        assert_eq!(parse_exit_status(b"7"), Some(7));
        assert_eq!(parse_exit_status(b"256"), Some(0));
        assert_eq!(parse_exit_status(b"257"), Some(1));
        assert_eq!(parse_exit_status(b"-1"), Some(255));
        assert_eq!(parse_exit_status(b"+3"), Some(3));
        assert_eq!(
            parse_exit_status(b"99999999999999999999"),
            Some(99999999999999999999u128 as u8 as i32)
        );
        for wrong in [&b""[..], b"x", b"1x", b"-", b" 1"] {
            assert_eq!(parse_exit_status(wrong), None);
        }
    }
}
