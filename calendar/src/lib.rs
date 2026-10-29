//! The proleptic Gregorian calendar in UTC (D150), shared by the kernel's real-time clock
//! drivers, `date`, `hwclock`, Settings and the desktop's clock: seconds since the Unix epoch as
//! a civil date and time and back ([`DateTime`]), what a person types to set the clock
//! ([`parse_setting`]) and the texts the clock is shown in ([`format`]). Days are counted from
//! 1970-01-01 with the days-from-civil and civil-from-days algorithms over eras of 400 years,
//! which hold for every year an `i64` count of seconds reaches. There are no time zones: EggOS
//! keeps UTC.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod format;
mod parse;

pub use parse::{ParseError, parse_proc_driver_rtc, parse_setting};

/// Seconds in a day.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// The first year the clock may be set to ([`parse_setting`]).
pub const FIRST_SETTABLE_YEAR: i64 = 2000;

/// The last year the clock may be set to.
pub const LAST_SETTABLE_YEAR: i64 = 2099;

/// The days of the week, from Sunday, as [`DateTime::weekday`] numbers them.
pub const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The months, from January.
pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const DAYS_PER_ERA: i64 = 146_097;
const DAYS_FROM_0000_03_01_TO_EPOCH: i64 = 719_468;
const THURSDAY: i64 = 4;

/// Whether `year` has a 29 February.
pub const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The days of `month` (1 to 12) in `year`; 0 for a month that is not one.
pub const fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// The days from 1970-01-01 to the date `year`-`month`-`day`, negative before it. The date
/// must be one ([`DateTime::is_valid`]).
pub const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = month as i64;
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * DAYS_PER_ERA + day_of_era - DAYS_FROM_0000_03_01_TO_EPOCH
}

/// The date `days` after 1970-01-01 (before it when negative): its year, month (1 to 12) and
/// day (1 to 31).
pub const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + DAYS_FROM_0000_03_01_TO_EPOCH;
    let era = shifted.div_euclid(DAYS_PER_ERA);
    let day_of_era = shifted.rem_euclid(DAYS_PER_ERA);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month as u32, day)
}

/// The earliest moment [`rfc3339`] writes: 0000-01-01T00:00:00Z.
pub const EARLIEST_RFC3339: i64 = -62_167_219_200;
/// The latest moment [`rfc3339`] writes: 9999-12-31T23:59:59Z.
pub const LATEST_RFC3339: i64 = 253_402_300_799;

/// The moment as RFC 3339 text in UTC, `YYYY-MM-DDTHH:MM:SSZ`, the command model's time form
/// (D160); `None` outside the years 0 to 9999.
pub const fn rfc3339(seconds: i64) -> Option<[u8; 20]> {
    if seconds < EARLIEST_RFC3339 || seconds > LATEST_RFC3339 {
        return None;
    }
    let moment = DateTime::from_unix(seconds);
    let mut text = *b"0000-00-00T00:00:00Z";
    let fields = [
        (0, moment.year as u32, 4),
        (5, moment.month, 2),
        (8, moment.day, 2),
        (11, moment.hour, 2),
        (14, moment.minute, 2),
        (17, moment.second, 2),
    ];
    let mut field = 0;
    while field < fields.len() {
        let (start, mut value, width) = fields[field];
        let mut index = start + width;
        while index > start {
            index -= 1;
            text[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        field += 1;
    }
    Some(text)
}

/// A moment as a civil date and time in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateTime {
    /// The moment `seconds` after 1970-01-01 00:00:00 UTC (before it when negative).
    pub const fn from_unix(seconds: i64) -> Self {
        let days = seconds.div_euclid(SECONDS_PER_DAY);
        let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (of_day / 3600) as u32,
            minute: (of_day % 3600 / 60) as u32,
            second: (of_day % 60) as u32,
        }
    }

    /// Whether it is a real date (its month has its day) and a time before 24:00:00.
    pub const fn is_valid(&self) -> bool {
        self.month >= 1
            && self.month <= 12
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
    }

    /// Its seconds since the epoch; `None` if it is not valid or too far to count.
    pub fn to_unix(&self) -> Option<i64> {
        if !self.is_valid() {
            return None;
        }
        let of_day =
            i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second);
        days_from_civil(self.year, self.month, self.day)
            .checked_mul(SECONDS_PER_DAY)?
            .checked_add(of_day)
    }

    /// Its day of the week, an index into [`WEEKDAYS`] (0 is Sunday).
    pub const fn weekday(&self) -> usize {
        (days_from_civil(self.year, self.month, self.day) + THURSDAY).rem_euclid(7) as usize
    }

    /// Its day of the year, from 1.
    pub const fn day_of_year(&self) -> u32 {
        (days_from_civil(self.year, self.month, self.day) - days_from_civil(self.year, 1, 1) + 1)
            as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_writes_utc_times_of_the_years_0_to_9999() {
        assert_eq!(rfc3339(0), Some(*b"1970-01-01T00:00:00Z"));
        assert_eq!(rfc3339(1_793_196_309), Some(*b"2026-10-28T14:05:09Z"));
        assert_eq!(rfc3339(EARLIEST_RFC3339), Some(*b"0000-01-01T00:00:00Z"));
        assert_eq!(rfc3339(LATEST_RFC3339), Some(*b"9999-12-31T23:59:59Z"));
        assert_eq!(rfc3339(EARLIEST_RFC3339 - 1), None);
        assert_eq!(rfc3339(LATEST_RFC3339 + 1), None);
    }

    #[test]
    fn known_moments_convert_both_ways() {
        for (seconds, moment, weekday) in [
            (0, (1970, 1, 1, 0, 0, 0), "Thursday"),
            (-1, (1969, 12, 31, 23, 59, 59), "Wednesday"),
            (946_684_800, (2000, 1, 1, 0, 0, 0), "Saturday"),
            (951_782_400, (2000, 2, 29, 0, 0, 0), "Tuesday"),
            (1_793_282_709, (2026, 10, 29, 14, 5, 9), "Thursday"),
            (4_107_542_400, (2100, 3, 1, 0, 0, 0), "Monday"),
        ] {
            let (year, month, day, hour, minute, second) = moment;
            let expected = DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
            };
            assert_eq!(DateTime::from_unix(seconds), expected, "{seconds}");
            assert_eq!(expected.to_unix(), Some(seconds));
            assert_eq!(WEEKDAYS[expected.weekday()], weekday, "{seconds}");
        }
    }

    #[test]
    fn days_round_trip_over_four_hundred_years_either_side_of_the_epoch() {
        let mut expected = civil_from_days(-146_097);
        for days in -146_097..146_097 {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
            assert_eq!((year, month, day), expected, "{days}");
            expected = match (day < days_in_month(year, month), month) {
                (true, _) => (year, month, day + 1),
                (false, 12) => (year + 1, 1, 1),
                (false, _) => (year, month + 1, 1),
            };
        }
    }

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        assert!(is_leap_year(2000) && is_leap_year(2024) && is_leap_year(1972));
        assert!(!is_leap_year(1900) && !is_leap_year(2100) && !is_leap_year(2026));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 13), 0);
    }

    #[test]
    fn invalid_moments_have_no_seconds() {
        let moment = DateTime::from_unix(0);
        for invalid in [
            DateTime { month: 0, ..moment },
            DateTime {
                month: 13,
                ..moment
            },
            DateTime { day: 0, ..moment },
            DateTime {
                year: 2026,
                month: 2,
                day: 29,
                ..moment
            },
            DateTime { hour: 24, ..moment },
            DateTime {
                minute: 60,
                ..moment
            },
            DateTime {
                second: 60,
                ..moment
            },
        ] {
            assert_eq!(invalid.to_unix(), None, "{invalid:?}");
        }
    }

    #[test]
    fn the_day_of_the_year_counts_from_one() {
        assert_eq!(DateTime::from_unix(0).day_of_year(), 1);
        assert_eq!(DateTime::from_unix(1_793_282_709).day_of_year(), 302);
        assert_eq!(
            DateTime::from_unix(951_782_400 + 306 * 86_400).day_of_year(),
            366
        );
    }
}
