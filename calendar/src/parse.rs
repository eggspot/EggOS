use crate::zone::Rule;
use crate::{DateTime, FIRST_SETTABLE_YEAR, LAST_SETTABLE_YEAR};

/// Why a text cannot set the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// It is not a date and time in a form [`parse_setting`] takes, or not a real one.
    NotADateAndTime,
    /// A time alone while the clock reads a year it cannot be set to (an unset clock reads
    /// 1970), so there is no date to keep.
    NeedsTheDate,
    /// A year outside [`FIRST_SETTABLE_YEAR`] to [`LAST_SETTABLE_YEAR`].
    YearOutOfRange,
}

/// The moment `text` names, in seconds since the epoch, to set the clock to (D150; the same
/// rules for `date -s`, Settings and the MCP tool `clock_set`). After spaces at either end and
/// a trailing `Z` or ` UTC`: `YYYY-MM-DD HH:MM`, `YYYY-MM-DD HH:MM:SS` (a `T` may stand for the
/// space), `HH:MM` or `HH:MM:SS`, each field with exactly its digits, in UTC. A time alone keeps
/// the date of the clock reading `now` seconds, if that date's year is one the clock can be set
/// to. The date must be a real one, the time before 24:00:00 and the year
/// [`FIRST_SETTABLE_YEAR`] to [`LAST_SETTABLE_YEAR`], which every real-time clock EggOS writes
/// can hold.
pub fn parse_setting(text: &str, now: i64) -> Result<i64, ParseError> {
    let (text, _) = without_utc_mark(text);
    settable(moment_named(text, now)?)
}

/// The moment `text` names, as [`parse_setting`] reads it, but read as local time in the zone
/// `rule` gives (D194, docs/ux/date-time-v0.3.md section 8.5): a time alone keeps the local
/// date of `now`, a trailing `Z` or ` UTC` still means UTC, and the year range applies to the
/// instant, not to the local reading.
pub fn parse_local_setting(text: &str, now: i64, rule: &Rule) -> Result<i64, ParseError> {
    let (bare, marked_utc) = without_utc_mark(text);
    if marked_utc {
        return parse_setting(text, now);
    }
    let local = moment_named(bare, now + rule.at(now).offset)?;
    settable(rule.to_utc(local))
}

fn without_utc_mark(text: &str) -> (&str, bool) {
    let text = text.trim_matches(' ');
    match text.strip_suffix('Z') {
        Some(rest) => (rest, true),
        None => match text.strip_suffix(" UTC") {
            Some(rest) => (rest, true),
            None => (text, false),
        },
    }
}

fn settable(seconds: i64) -> Result<i64, ParseError> {
    let year = DateTime::from_unix(seconds).year;
    match (FIRST_SETTABLE_YEAR..=LAST_SETTABLE_YEAR).contains(&year) {
        true => Ok(seconds),
        false => Err(ParseError::YearOutOfRange),
    }
}

fn moment_named(text: &str, now: i64) -> Result<i64, ParseError> {
    let bytes = text.as_bytes();
    let (date, time) = match bytes.len() {
        5 | 8 => (None, bytes),
        16 | 19 if matches!(bytes[10], b' ' | b'T') => (Some(&bytes[..10]), &bytes[11..]),
        _ => return Err(ParseError::NotADateAndTime),
    };
    let (hour, minute, second) = clock_time(time).ok_or(ParseError::NotADateAndTime)?;
    let (year, month, day) = match date {
        Some(date) => calendar_date(date).ok_or(ParseError::NotADateAndTime)?,
        None => {
            let today = DateTime::from_unix(now);
            if !(FIRST_SETTABLE_YEAR..=LAST_SETTABLE_YEAR).contains(&today.year) {
                return Err(ParseError::NeedsTheDate);
            }
            (today.year, today.month, today.day)
        }
    };
    let moment = DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    };
    moment.to_unix().ok_or(ParseError::NotADateAndTime)
}

/// The time a `/proc/driver/rtc` text says the real-time clock reads, in seconds since the
/// epoch: its `rtc_date` (`YYYY-MM-DD`) and `rtc_time` (`HH:MM:SS`) lines, in Linux's layout
/// ([`crate::format::proc_driver_rtc`]); `None` if either is missing or not a real moment.
pub fn parse_proc_driver_rtc(text: &[u8]) -> Option<i64> {
    let field = |name: &[u8]| {
        text.split(|&byte| byte == b'\n').find_map(|line| {
            let colon = line.iter().position(|&byte| byte == b':')?;
            let (key, value) = (&line[..colon], &line[colon + 1..]);
            (key.trim_ascii() == name).then(|| value.trim_ascii())
        })
    };
    let (date, time) = (field(b"rtc_date")?, field(b"rtc_time")?);
    if date.len() != 10 || time.len() != 8 {
        return None;
    }
    let (year, month, day) = calendar_date(date)?;
    let (hour, minute, second) = clock_time(time)?;
    DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
    .to_unix()
}

fn number(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0u32, |value, &digit| {
        digit
            .is_ascii_digit()
            .then(|| value * 10 + u32::from(digit - b'0'))
    })
}

fn clock_time(text: &[u8]) -> Option<(u32, u32, u32)> {
    let second = match text.len() {
        5 => 0,
        8 if text[5] == b':' => number(&text[6..8])?,
        _ => return None,
    };
    if text[2] != b':' {
        return None;
    }
    Some((number(&text[..2])?, number(&text[3..5])?, second))
}

fn calendar_date(text: &[u8]) -> Option<(i64, u32, u32)> {
    if text[4] != b'-' || text[7] != b'-' {
        return None;
    }
    Some((
        i64::from(number(&text[..4])?),
        number(&text[5..7])?,
        number(&text[8..10])?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_282_709;

    #[test]
    fn dates_with_times_and_times_alone_are_read_in_utc() {
        for (text, expected) in [
            ("2026-10-29 14:05", 1_793_282_700),
            ("2026-10-29 14:05:09", NOW),
            ("2026-10-29T14:05:09Z", NOW),
            ("  2026-10-29 14:05:09 UTC ", NOW),
            ("14:05:30", 1_793_282_730),
            ("00:00", 1_793_232_000),
            ("2000-01-01 00:00", 946_684_800),
            ("2099-12-31 23:59:59", 4_102_444_799),
            ("2024-02-29 12:00", 1_709_208_000),
        ] {
            assert_eq!(parse_setting(text, NOW), Ok(expected), "{text}");
        }
    }

    #[test]
    fn what_is_not_a_date_and_time_is_refused() {
        for text in [
            "",
            "now",
            "2026-02-30 10:00",
            "2026-13-01 10:00",
            "29/10/2026",
            "14:5",
            "24:00",
            "12:60",
            "12:00:60",
            "2026-10-29",
            "2026-10-29 14:05:9",
            "2026-10-29_14:05",
            "+026-10-29 14:05",
            "2026-10-29 14:05ZZ",
            "２026-10-29 14:05",
        ] {
            assert_eq!(
                parse_setting(text, NOW),
                Err(ParseError::NotADateAndTime),
                "{text}"
            );
        }
    }

    #[test]
    fn the_proc_driver_rtc_text_needs_both_its_date_and_its_time() {
        let linux = b"rtc_time\t: 14:05:09\nrtc_date\t: 2026-10-29\nalrm_time\t: 00:00:00\n\
                      24hr\t\t: yes\n";
        assert_eq!(parse_proc_driver_rtc(linux), Some(NOW));
        assert_eq!(parse_proc_driver_rtc(b"rtc_time\t: 14:05:09\n"), None);
        assert_eq!(
            parse_proc_driver_rtc(b"rtc_time : 14:05:09\nrtc_date : 2026-02-30\n"),
            None
        );
        assert_eq!(parse_proc_driver_rtc(b""), None);
    }

    #[test]
    fn a_local_setting_is_read_in_the_zone_unless_marked_utc() {
        let paris = crate::zone::zone("Europe/Paris").unwrap();
        for (text, expected) in [
            ("2026-10-29 15:05", Ok(1_793_282_700)),
            ("2026-10-29 15:05 UTC", Ok(1_793_286_300)),
            ("15:05:30", Ok(1_793_282_730)),
            ("2026-03-29 02:30", Ok(1_774_747_800)),
            ("2000-01-01 00:30", Err(ParseError::YearOutOfRange)),
        ] {
            assert_eq!(parse_local_setting(text, NOW, &paris), expected, "{text}");
        }
        let new_york = crate::zone::zone("America/New_York").unwrap();
        assert_eq!(
            parse_local_setting("2099-12-31 23:30", NOW, &new_york),
            Err(ParseError::YearOutOfRange)
        );
        let utc = crate::zone::zone("UTC").unwrap();
        assert_eq!(
            parse_local_setting("2026-10-29 14:05:09", NOW, &utc),
            Ok(NOW)
        );
    }

    #[test]
    fn a_time_alone_needs_a_set_clock_and_the_year_its_range() {
        assert_eq!(parse_setting("14:05", 12), Err(ParseError::NeedsTheDate));
        assert_eq!(parse_setting("1:05", 12), Err(ParseError::NotADateAndTime));
        assert_eq!(
            parse_setting("14:05", 86_400 + 12),
            Err(ParseError::NeedsTheDate)
        );
        assert_eq!(
            parse_setting("14:05", 4_102_444_800),
            Err(ParseError::NeedsTheDate)
        );
        for text in ["1999-12-31 23:59", "2100-01-01 00:00", "1970-01-01 00:00"] {
            assert_eq!(
                parse_setting(text, NOW),
                Err(ParseError::YearOutOfRange),
                "{text}"
            );
        }
    }
}
