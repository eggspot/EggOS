//! The network clock's place in init (D225, docs/ux/date-time-v0.3.md section 9.1): init starts
//! a clock keeper, a child of its own, that runs `date --sync --automatic` once the network has
//! an address and `time.automatic` is on, then every [`POLL_S`] (less or more by up to
//! [`JITTER_PERCENT`] %), or after a failure in [`FIRST_RETRY_S`], twice as long each time up
//! to [`LAST_RETRY_S`]. The setting is read again before each sync, so `settings set
//! time.automatic off` stops the next one without a restart; `date`'s lines go to init's log.
//! Without `/bin/date` no keeper starts (D18).

/// The program, with its terminating NUL.
pub const DATE: &[u8] = b"/bin/date\0";

/// Its arguments after its name, each with its terminating NUL.
pub const SYNC_ARGUMENTS: [&[u8]; 2] = [b"--sync\0", b"--automatic\0"];

/// The settings kept for this session without a disk, with its terminating NUL.
pub const SESSION_SETTINGS: &[u8] = b"/run/eggos/settings\0";

/// The network interfaces' state, with its terminating NUL.
pub const INTERFACES: &[u8] = b"/proc/net/interfaces\0";

/// The time between syncs that succeed.
pub const POLL_S: u64 = 86_400;

/// How far each wait of [`POLL_S`] may move, in percent.
pub const JITTER_PERCENT: u64 = 10;

/// The wait after the first failure.
pub const FIRST_RETRY_S: u64 = 60;

/// The longest wait after failures.
pub const LAST_RETRY_S: u64 = 3600;

/// When the next sync is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    next_retry_s: u64,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            next_retry_s: FIRST_RETRY_S,
        }
    }
}

impl Schedule {
    /// The seconds to wait after a sync that set the clock, with `random` choosing the
    /// jitter; the retries start over.
    pub fn after_success(&mut self, random: u64) -> u64 {
        *self = Self::default();
        let spread = POLL_S * JITTER_PERCENT / 100;
        POLL_S - spread + random % (2 * spread + 1)
    }

    /// The seconds to wait after a sync that failed.
    pub fn after_failure(&mut self) -> u64 {
        let delay = self.next_retry_s;
        self.next_retry_s = (delay * 2).min(LAST_RETRY_S);
        delay
    }
}

/// Whether `/proc/net/interfaces` (`interfaces`) shows an adapter other than `lo` with an
/// address.
pub fn has_address(interfaces: &[u8]) -> bool {
    interfaces.split(|&byte| byte == b'\n').any(|line| {
        let mut fields = line
            .split(|&byte| byte == b' ')
            .filter(|field| !field.is_empty());
        let name = fields.next();
        let subnet = fields.nth(4);
        name.is_some_and(|name| name != b"lo") && subnet.is_some_and(|subnet| subnet != b"-")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_and_arguments_are_c_strings() {
        for text in [
            DATE,
            SESSION_SETTINGS,
            INTERFACES,
            SYNC_ARGUMENTS[0],
            SYNC_ARGUMENTS[1],
        ] {
            assert_eq!(
                text.iter().position(|&byte| byte == 0),
                Some(text.len() - 1)
            );
        }
        assert_eq!(
            &SESSION_SETTINGS[..SESSION_SETTINGS.len() - 1],
            eggos_settings_file::SESSION_PATH.as_bytes()
        );
    }

    #[test]
    fn failures_wait_a_minute_then_twice_as_long_up_to_an_hour() {
        let mut schedule = Schedule::default();
        let delays: Vec<u64> = (0..8).map(|_| schedule.after_failure()).collect();
        assert_eq!(delays, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        assert_eq!(schedule.after_success(0), 77_760);
        assert_eq!(schedule.after_failure(), 60);
    }

    #[test]
    fn a_success_waits_a_day_give_or_take_a_tenth() {
        let mut schedule = Schedule::default();
        assert_eq!(schedule.after_success(0), 77_760);
        assert_eq!(schedule.after_success(17_280), 95_040);
        assert_eq!(schedule.after_success(8_640), 86_400);
        for random in [1, 999_999, u64::MAX] {
            let delay = schedule.after_success(random);
            assert!((77_760..=95_040).contains(&delay), "{delay}");
        }
    }

    #[test]
    fn an_address_on_an_adapter_other_than_loopback_counts() {
        let loopback = b"lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - 12 1008 0 0 12\n";
        assert!(!has_address(loopback));
        let waiting = b"eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 0 0 0 0 0 0 0\n";
        assert!(!has_address(&[&loopback[..], waiting].concat()));
        let connected = b"eth0 up 52:54:00:12:34:56 1500 configured 10.0.2.15/24 10.0.2.2 \
                          10.0.2.3 10.0.2.2 86342 86400 2905 3600000 0 4 812 1300000 0\n";
        assert!(has_address(&[&loopback[..], connected].concat()));
        assert!(!has_address(b""));
    }
}
