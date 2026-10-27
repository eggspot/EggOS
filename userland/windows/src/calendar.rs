//! Windows time values: a `FILETIME` counts 100 ns intervals since 1601-01-01 UTC, and a
//! `SYSTEMTIME` is its calendar date and time (proleptic Gregorian, as Windows uses).

pub const INTERVALS_PER_SECOND: u64 = 10_000_000;
pub const UNIX_EPOCH_SECONDS: u64 = 11_644_473_600;
const SECONDS_PER_DAY: u64 = 86_400;
const DAYS_FROM_1601_TO_0000_03_01: i64 = 584_694;
const MAX_YEAR: u16 = 30_827;

/// A `SYSTEMTIME`'s eight fields, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemTime {
    pub year: u16,
    pub month: u16,
    pub day_of_week: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

impl SystemTime {
    /// The structure's bytes, as Windows lays them out.
    pub fn to_bytes(self) -> [u8; 16] {
        let fields = [
            self.year,
            self.month,
            self.day_of_week,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.milliseconds,
        ];
        let mut bytes = [0u8; 16];
        for (pair, field) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(fields) {
            pair.copy_from_slice(&field.to_le_bytes());
        }
        bytes
    }

    /// The structure read from its bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        let field = |index: usize| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]);
        Self {
            year: field(0),
            month: field(1),
            day_of_week: field(2),
            day: field(3),
            hour: field(4),
            minute: field(5),
            second: field(6),
            milliseconds: field(7),
        }
    }
}

/// The `FILETIME` of a Unix time (seconds and nanoseconds since 1970 UTC); 0 before 1601.
pub fn file_time_from_unix(seconds: i64, nanoseconds: u32) -> u64 {
    let Some(seconds) = seconds.checked_add(UNIX_EPOCH_SECONDS as i64) else {
        return u64::MAX;
    };
    match u64::try_from(seconds) {
        Ok(seconds) => seconds
            .saturating_mul(INTERVALS_PER_SECOND)
            .saturating_add(u64::from(nanoseconds / 100)),
        Err(_) => 0,
    }
}

/// The calendar date and time of `file_time`; `None` past what `SYSTEMTIME` holds (30827).
pub fn system_time(file_time: u64) -> Option<SystemTime> {
    let seconds = file_time / INTERVALS_PER_SECOND;
    let days = (seconds / SECONDS_PER_DAY) as i64;
    let in_day = seconds % SECONDS_PER_DAY;
    let shifted = days + DAYS_FROM_1601_TO_0000_03_01;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let year = u16::try_from(year).ok().filter(|year| *year <= MAX_YEAR)?;
    Some(SystemTime {
        year,
        month: month as u16,
        day_of_week: ((days + 1).rem_euclid(7)) as u16,
        day: day as u16,
        hour: (in_day / 3600) as u16,
        minute: (in_day / 60 % 60) as u16,
        second: (in_day % 60) as u16,
        milliseconds: (file_time % INTERVALS_PER_SECOND / 10_000) as u16,
    })
}

/// The `FILETIME` of a calendar date and time; `None` for an invalid one (`day_of_week` is
/// ignored, as `SystemTimeToFileTime` ignores it).
pub fn file_time(time: &SystemTime) -> Option<u64> {
    let valid = (1601..=MAX_YEAR).contains(&time.year)
        && (1..=12).contains(&time.month)
        && time.day >= 1
        && time.day <= days_in_month(time.year, time.month)
        && time.hour < 24
        && time.minute < 60
        && time.second < 60
        && time.milliseconds < 1000;
    if !valid {
        return None;
    }
    let year = i64::from(time.year) - i64::from(time.month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(time.month);
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(time.day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - DAYS_FROM_1601_TO_0000_03_01;
    let seconds = days as u64 * SECONDS_PER_DAY
        + u64::from(time.hour) * 3600
        + u64::from(time.minute) * 60
        + u64::from(time.second);
    Some(seconds * INTERVALS_PER_SECOND + u64::from(time.milliseconds) * 10_000)
}

fn days_in_month(year: u16, month: u16) -> u16 {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: u16, month: u16, day: u16, hour: u16, minute: u16, second: u16) -> SystemTime {
        SystemTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            ..SystemTime::default()
        }
    }

    #[test]
    fn unix_times_become_file_times() {
        assert_eq!(file_time_from_unix(0, 0), 116_444_736_000_000_000);
        assert_eq!(file_time_from_unix(1, 250), 116_444_736_010_000_002);
        assert_eq!(file_time_from_unix(-(UNIX_EPOCH_SECONDS as i64) - 1, 0), 0);
    }

    #[test]
    fn file_times_and_calendar_dates_convert_both_ways() {
        let epoch = system_time(0).unwrap();
        assert_eq!(
            epoch,
            SystemTime {
                day_of_week: 1,
                ..at(1601, 1, 1, 0, 0, 0)
            }
        );
        let unix = system_time(file_time_from_unix(0, 0)).unwrap();
        assert_eq!(
            unix,
            SystemTime {
                day_of_week: 4,
                ..at(1970, 1, 1, 0, 0, 0)
            }
        );
        let time = system_time(file_time_from_unix(1_793_000_000, 123_000_000)).unwrap();
        assert_eq!(
            time,
            SystemTime {
                day_of_week: 1,
                milliseconds: 123,
                ..at(2026, 10, 26, 7, 33, 20)
            }
        );
        for unix in [
            0i64,
            951_782_400,
            4_107_542_399,
            1_709_208_000,
            -5_000_000_000,
        ] {
            let file = file_time_from_unix(unix, 0);
            let calendar = system_time(file).unwrap();
            assert_eq!(file_time(&calendar), Some(file), "{unix}");
        }
        let leap_day = at(2000, 2, 29, 12, 0, 0);
        assert_eq!(system_time(file_time(&leap_day).unwrap()).unwrap().day, 29);
        for invalid in [
            at(1600, 12, 31, 0, 0, 0),
            at(2023, 2, 29, 0, 0, 0),
            at(1900, 2, 29, 0, 0, 0),
            at(2024, 13, 1, 0, 0, 0),
            at(2024, 4, 31, 0, 0, 0),
            at(2024, 1, 0, 0, 0, 0),
            at(2024, 1, 1, 24, 0, 0),
        ] {
            assert_eq!(file_time(&invalid), None, "{invalid:?}");
        }
        assert_eq!(system_time(u64::MAX), None);
    }

    #[test]
    fn system_times_have_windows_layout() {
        let time = SystemTime {
            milliseconds: 999,
            day_of_week: 3,
            ..at(2026, 10, 28, 1, 2, 3)
        };
        let bytes = time.to_bytes();
        assert_eq!(&bytes[0..2], &2026u16.to_le_bytes());
        assert_eq!(&bytes[4..6], &3u16.to_le_bytes());
        assert_eq!(&bytes[14..16], &999u16.to_le_bytes());
        assert_eq!(SystemTime::from_bytes(bytes), time);
    }
}
