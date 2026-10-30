//! How init takes the machine down (D183): asked by a signal (`poweroff`, `reboot` and
//! `halt`, the desktop's power menu) or as its configuration says once the login program ends,
//! it sends every other process `SIGTERM`, then `SIGCONT` so a stopped one sees it, waits until
//! they have all ended or [`TERM_GRACE_MS`] has passed, sends those left `SIGKILL` and waits
//! [`KILL_GRACE_MS`] more at most, then puts the disks' changes on them and calls `reboot`.
//! The signals are systemd's (`SIGRTMIN+3` halts, `+4` powers off, `+5` restarts), with
//! busybox's `SIGUSR1` (halt) and `SIGUSR2` (power off) too; `SIGTERM`, which a plain `kill 1`
//! sends, does nothing, so init is not taken down by accident.

/// `SIGUSR1`.
pub const SIGUSR1: usize = 10;

/// `SIGUSR2`.
pub const SIGUSR2: usize = 12;

/// `SIGRTMIN` as C libraries number it, the first two real-time signals being theirs.
pub const SIGRTMIN: usize = 34;

/// The signals init takes as requests ([`Request::of_signal`]).
pub const SIGNALS: [usize; 5] = [SIGUSR1, SIGUSR2, SIGRTMIN + 3, SIGRTMIN + 4, SIGRTMIN + 5];

/// The longest init waits after `SIGTERM` before it sends `SIGKILL`.
pub const TERM_GRACE_MS: u64 = 3000;

/// The longest init waits after `SIGKILL` before it takes the machine down anyway.
pub const KILL_GRACE_MS: u64 = 1000;

/// How often init looks whether every other process has ended.
pub const LOOK_EVERY_MS: u64 = 10;

/// `reboot`'s commands (`LINUX_REBOOT_CMD_*`).
pub mod command {
    /// Powers the machine off.
    pub const POWER_OFF: usize = 0x4321_fedc;
    /// Resets the machine.
    pub const RESTART: usize = 0x0123_4567;
    /// Stops the machine.
    pub const HALT: usize = 0xcdef_0123;
}

/// How the machine goes down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Powered off.
    PowerOff,
    /// Reset.
    Restart,
    /// Stopped without powering off.
    Halt,
}

impl Request {
    /// The request signal `signal` makes, `None` for any other signal.
    pub fn of_signal(signal: usize) -> Option<Self> {
        match signal {
            SIGUSR2 => Some(Self::PowerOff),
            SIGUSR1 => Some(Self::Halt),
            _ if signal == SIGRTMIN + 3 => Some(Self::Halt),
            _ if signal == SIGRTMIN + 4 => Some(Self::PowerOff),
            _ if signal == SIGRTMIN + 5 => Some(Self::Restart),
            _ => None,
        }
    }

    /// The `reboot` command that does it.
    pub fn reboot_command(self) -> usize {
        match self {
            Self::PowerOff => command::POWER_OFF,
            Self::Restart => command::RESTART,
            Self::Halt => command::HALT,
        }
    }
}

/// What init does next while the other processes end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Look again in [`LOOK_EVERY_MS`].
    Wait,
    /// Send `SIGKILL` to every process left.
    Kill,
    /// Take the machine down.
    GoDown,
}

/// The wait between `SIGTERM` and going down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    terminated_ms: u64,
    killed_ms: Option<u64>,
}

impl Ending {
    /// The wait for processes sent `SIGTERM` at `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        Self {
            terminated_ms: now_ms,
            killed_ms: None,
        }
    }

    /// The next step at `now_ms`, when every other process has ended (`all_ended`) or not.
    pub fn next(&mut self, all_ended: bool, now_ms: u64) -> Step {
        if all_ended {
            return Step::GoDown;
        }
        match self.killed_ms {
            None if now_ms.saturating_sub(self.terminated_ms) >= TERM_GRACE_MS => {
                self.killed_ms = Some(now_ms);
                Step::Kill
            }
            Some(killed) if now_ms.saturating_sub(killed) >= KILL_GRACE_MS => Step::GoDown,
            _ => Step::Wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_s_and_busybox_s_signals_are_requests_and_sigterm_is_not() {
        assert_eq!(Request::of_signal(37), Some(Request::Halt));
        assert_eq!(Request::of_signal(38), Some(Request::PowerOff));
        assert_eq!(Request::of_signal(39), Some(Request::Restart));
        assert_eq!(Request::of_signal(SIGUSR1), Some(Request::Halt));
        assert_eq!(Request::of_signal(SIGUSR2), Some(Request::PowerOff));
        for other in [1, 2, 9, 15, 17, 34, 36, 40, 64] {
            assert_eq!(Request::of_signal(other), None, "signal {other}");
        }
        for signal in SIGNALS {
            assert!(Request::of_signal(signal).is_some());
        }
    }

    #[test]
    fn each_request_has_its_reboot_command() {
        assert_eq!(Request::PowerOff.reboot_command(), 0x4321_fedc);
        assert_eq!(Request::Restart.reboot_command(), 0x0123_4567);
        assert_eq!(Request::Halt.reboot_command(), 0xcdef_0123);
    }

    #[test]
    fn the_machine_goes_down_as_soon_as_every_process_has_ended() {
        let mut ending = Ending::new(1000);
        assert_eq!(ending.next(false, 1000), Step::Wait);
        assert_eq!(ending.next(false, 1010), Step::Wait);
        assert_eq!(ending.next(true, 1020), Step::GoDown);
    }

    #[test]
    fn processes_left_after_the_grace_are_killed_then_the_machine_goes_down() {
        let mut ending = Ending::new(1000);
        assert_eq!(ending.next(false, 1000 + TERM_GRACE_MS - 1), Step::Wait);
        assert_eq!(ending.next(false, 1000 + TERM_GRACE_MS), Step::Kill);
        let killed = 1000 + TERM_GRACE_MS;
        assert_eq!(ending.next(false, killed + 1), Step::Wait);
        assert_eq!(ending.next(true, killed + 2), Step::GoDown);
        let mut stubborn = Ending::new(0);
        assert_eq!(stubborn.next(false, TERM_GRACE_MS), Step::Kill);
        assert_eq!(
            stubborn.next(false, TERM_GRACE_MS + KILL_GRACE_MS - 1),
            Step::Wait
        );
        assert_eq!(
            stubborn.next(false, TERM_GRACE_MS + KILL_GRACE_MS),
            Step::GoDown
        );
    }

    #[test]
    fn looking_every_few_milliseconds_kills_after_the_grace_then_goes_down() {
        let mut ending = Ending::new(0);
        let mut now = 0;
        let mut steps = Vec::new();
        while steps.last().is_none_or(|&(step, _)| step != Step::GoDown) {
            match ending.next(false, now) {
                Step::Wait => now += LOOK_EVERY_MS,
                step => steps.push((step, now)),
            }
        }
        assert_eq!(
            steps,
            [
                (Step::Kill, TERM_GRACE_MS),
                (Step::GoDown, TERM_GRACE_MS + KILL_GRACE_MS)
            ]
        );
    }

    #[test]
    fn a_clock_that_goes_back_only_waits() {
        let mut ending = Ending::new(5000);
        assert_eq!(ending.next(false, 10), Step::Wait);
    }
}
