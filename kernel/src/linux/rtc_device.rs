//! `/dev/rtc0` (D194): Linux's real-time clock device over the wall clock (D150), for a
//! machine with a real-time clock. Its `ioctl`s read and set the time (`RTC_RD_TIME`,
//! `RTC_SET_TIME`, privileged), and set, read, turn on and off an alarm (`RTC_ALM_SET`,
//! `RTC_ALM_READ`, `RTC_WKALM_SET`, `RTC_WKALM_RD`, `RTC_AIE_ON`, `RTC_AIE_OFF`), each in a
//! `struct rtc_time` in UTC. The alarm is kept, and reported pending once its time has passed,
//! as EggOS never suspends; waiting for it by reading the device is not answered yet.

use super::Errno;
use crate::process::Process;
use crate::sync::LeafSpinLock;
use eggos_calendar::DateTime;
use eggos_hal::Platform;

/// `RTC_ALM_SET`: the alarm's time of day, its next occurrence.
pub const ALARM_SET: u32 = 0x4024_7007;
/// `RTC_ALM_READ`.
pub const ALARM_READ: u32 = 0x8024_7008;
/// `RTC_RD_TIME`.
pub const READ_TIME: u32 = 0x8024_7009;
/// `RTC_SET_TIME`.
pub const SET_TIME: u32 = 0x4024_700a;
/// `RTC_WKALM_SET`: the alarm's whole date and time, and whether it is on.
pub const WAKE_ALARM_SET: u32 = 0x4028_700f;
/// `RTC_WKALM_RD`.
pub const WAKE_ALARM_READ: u32 = 0x8028_7010;
/// `RTC_AIE_ON`.
pub const ALARM_ON: u32 = 0x7001;
/// `RTC_AIE_OFF`.
pub const ALARM_OFF: u32 = 0x7002;

const TIME_BYTES: usize = 36;
const WAKE_ALARM_BYTES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Alarm {
    seconds: i64,
    enabled: bool,
}

static ALARM: LeafSpinLock<Alarm> = LeafSpinLock::new_leaf(Alarm {
    seconds: 0,
    enabled: false,
});

/// A `struct rtc_time` for `seconds` since the epoch (UTC).
pub fn rtc_time(seconds: i64) -> [u8; TIME_BYTES] {
    let moment = DateTime::from_unix(seconds);
    let fields = [
        moment.second as i32,
        moment.minute as i32,
        moment.hour as i32,
        moment.day as i32,
        moment.month as i32 - 1,
        (moment.year - 1900) as i32,
        moment.weekday() as i32,
        moment.day_of_year() as i32 - 1,
        0,
    ];
    let mut bytes = [0u8; TIME_BYTES];
    for (place, field) in bytes.chunks_exact_mut(4).zip(fields) {
        place.copy_from_slice(&field.to_ne_bytes());
    }
    bytes
}

fn field(bytes: &[u8], index: usize) -> i64 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[4 * index..4 * index + 4]);
    i64::from(i32::from_ne_bytes(word))
}

/// The seconds since the epoch a `struct rtc_time` names; `None` when it is not a real date
/// and time.
pub fn seconds_of(bytes: &[u8; TIME_BYTES]) -> Option<i64> {
    let to_u32 = |value: i64| u32::try_from(value).ok();
    DateTime {
        year: field(bytes, 5) + 1900,
        month: to_u32(field(bytes, 4) + 1)?,
        day: to_u32(field(bytes, 3))?,
        hour: to_u32(field(bytes, 2))?,
        minute: to_u32(field(bytes, 1))?,
        second: to_u32(field(bytes, 0))?,
    }
    .to_unix()
}

/// The next time after `now` that has the time of day of `bytes` (`RTC_ALM_SET` ignores the
/// date, as Linux's does).
pub fn next_time_of_day(bytes: &[u8; TIME_BYTES], now: i64) -> Option<i64> {
    let (hour, minute, second) = (field(bytes, 2), field(bytes, 1), field(bytes, 0));
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    let of_day = hour * 3600 + minute * 60 + second;
    let today = now.div_euclid(86_400) * 86_400 + of_day;
    Some(if today > now { today } else { today + 86_400 })
}

fn read_bytes<P: Platform, const N: usize>(
    process: &Process<P>,
    argument: u64,
) -> Result<[u8; N], Errno> {
    let mut bytes = [0u8; N];
    process
        .read_user_memory(argument, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes)
}

fn write_bytes<P: Platform>(
    process: &Process<P>,
    argument: u64,
    bytes: &[u8],
) -> Result<u64, Errno> {
    process
        .write_user_memory(argument, bytes)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// An `ioctl` on `/dev/rtc0` by `process`.
pub fn ioctl<P: Platform>(process: &Process<P>, request: u64, argument: u64) -> Result<u64, Errno> {
    let now = crate::wall_clock::seconds::<P>() as i64;
    match request as u32 {
        READ_TIME => write_bytes(process, argument, &rtc_time(now)),
        SET_TIME => {
            let bytes = read_bytes::<P, TIME_BYTES>(process, argument)?;
            let seconds = seconds_of(&bytes)
                .filter(|&seconds| seconds >= 0)
                .ok_or(Errno::EINVAL)?;
            if !super::time::is_privileged(process) {
                return Err(Errno::EACCES);
            }
            super::time::set_wall_clock::<P>(seconds as u64 * 1_000_000_000);
            Ok(0)
        }
        ALARM_SET => {
            let bytes = read_bytes::<P, TIME_BYTES>(process, argument)?;
            let seconds = next_time_of_day(&bytes, now).ok_or(Errno::EINVAL)?;
            ALARM.lock().seconds = seconds;
            Ok(0)
        }
        ALARM_READ => {
            let seconds = ALARM.lock().seconds;
            write_bytes(process, argument, &rtc_time(seconds))
        }
        WAKE_ALARM_SET => {
            let bytes = read_bytes::<P, WAKE_ALARM_BYTES>(process, argument)?;
            let mut time = [0u8; TIME_BYTES];
            time.copy_from_slice(&bytes[4..]);
            let seconds = seconds_of(&time).ok_or(Errno::EINVAL)?;
            *ALARM.lock() = Alarm {
                seconds,
                enabled: bytes[0] != 0,
            };
            Ok(0)
        }
        WAKE_ALARM_READ => {
            let alarm = *ALARM.lock();
            let mut bytes = [0u8; WAKE_ALARM_BYTES];
            bytes[0] = u8::from(alarm.enabled);
            bytes[1] = u8::from(alarm.enabled && now >= alarm.seconds);
            bytes[4..].copy_from_slice(&rtc_time(alarm.seconds));
            write_bytes(process, argument, &bytes)
        }
        ALARM_ON => {
            ALARM.lock().enabled = true;
            Ok(0)
        }
        ALARM_OFF => {
            ALARM.lock().enabled = false;
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_goes_into_a_struct_rtc_time_and_back() {
        let seconds = 1_774_746_000;
        let bytes = rtc_time(seconds);
        assert_eq!(field(&bytes, 5), 126);
        assert_eq!(field(&bytes, 4), 2);
        assert_eq!(field(&bytes, 3), 29);
        assert_eq!(field(&bytes, 2), 1);
        assert_eq!(field(&bytes, 6), 0);
        assert_eq!(seconds_of(&bytes), Some(seconds));
        let mut wrong = bytes;
        wrong[16..20].copy_from_slice(&12i32.to_ne_bytes());
        assert_eq!(seconds_of(&wrong), None);
    }

    #[test]
    fn an_alarm_time_of_day_is_its_next_occurrence() {
        let now = 1_774_746_000;
        let mut later = rtc_time(now);
        later[8..12].copy_from_slice(&5i32.to_ne_bytes());
        assert_eq!(next_time_of_day(&later, now), Some(now + 4 * 3600));
        let earlier = rtc_time(now - 60);
        assert_eq!(next_time_of_day(&earlier, now), Some(now - 60 + 86_400));
        let mut bad = later;
        bad[8..12].copy_from_slice(&24i32.to_ne_bytes());
        assert_eq!(next_time_of_day(&bad, now), None);
    }
}
