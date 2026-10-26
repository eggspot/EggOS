//! How long init waits before it starts the login program again (D106), so that one which
//! cannot start or crashes at once does not take the machine in a loop: one that ran for at
//! least [`STEADY_RUN_MS`] starts again at once; one that ended sooner waits
//! [`FIRST_DELAY_SECONDS`], then twice as long each time it ends early again, up to
//! [`MAX_DELAY_SECONDS`].

/// A run at least this long counts as a working program: the next start is at once, and the
/// delay starts over.
pub const STEADY_RUN_MS: u64 = 5000;

/// The delay after the first early end.
pub const FIRST_DELAY_SECONDS: u64 = 1;

/// The longest delay.
pub const MAX_DELAY_SECONDS: u64 = 30;

/// The delay the next early end waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    next_delay_seconds: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            next_delay_seconds: FIRST_DELAY_SECONDS,
        }
    }
}

impl Backoff {
    /// The seconds to wait before starting the program again, which ran for `ran_ms`.
    pub fn delay_after(&mut self, ran_ms: u64) -> u64 {
        if ran_ms >= STEADY_RUN_MS {
            *self = Self::default();
            return 0;
        }
        let delay = self.next_delay_seconds;
        self.next_delay_seconds = (delay * 2).min(MAX_DELAY_SECONDS);
        delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_ends_wait_twice_as_long_each_time_up_to_the_cap() {
        let mut backoff = Backoff::default();
        let delays: Vec<u64> = (0..8).map(|_| backoff.delay_after(10)).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_steady_run_starts_again_at_once_and_resets_the_delay() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.delay_after(0), 1);
        assert_eq!(backoff.delay_after(STEADY_RUN_MS - 1), 2);
        assert_eq!(backoff.delay_after(STEADY_RUN_MS), 0);
        assert_eq!(backoff.delay_after(100), 1);
    }
}
