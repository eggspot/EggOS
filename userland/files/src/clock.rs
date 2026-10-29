//! Times as Files and the Trash write them: seconds since the Unix epoch as a civil date in
//! UTC, `YYYY-MM-DD HH:MM` in the list and `YYYY-MM-DDThh:mm:ss` in a `.trashinfo` file (the
//! freedesktop.org Trash specification), read back too. The wall clock is read from the real-time
//! clock at boot (D150).

use core::fmt::Write;

const SECONDS_PER_DAY: u64 = 86_400;
const DAYS_PER_ERA: u64 = 146_097;
const EPOCH_SHIFT: u64 = 719_468;

/// A moment as a civil date and time in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub year: u64,
    pub month: u64,
    pub day: u64,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
}

impl Moment {
    /// `seconds` since 1970-01-01 00:00:00 UTC.
    pub fn of(seconds: u64) -> Self {
        let days = seconds / SECONDS_PER_DAY;
        let rest = seconds % SECONDS_PER_DAY;
        let shifted = days + EPOCH_SHIFT;
        let era = shifted / DAYS_PER_ERA;
        let day_of_era = shifted % DAYS_PER_ERA;
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
        let year = year_of_era + era * 400 + u64::from(month <= 2);
        Self {
            year,
            month,
            day,
            hour: rest / 3600,
            minute: rest % 3600 / 60,
            second: rest % 60,
        }
    }

    /// The seconds since the epoch; `None` before it or for a date that is not one.
    pub fn seconds(self) -> Option<u64> {
        let valid = self.year >= 1970
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60;
        if !valid {
            return None;
        }
        let year = self.year - u64::from(self.month <= 2);
        let era = year / 400;
        let year_of_era = year % 400;
        let month_index = if self.month > 2 {
            self.month - 3
        } else {
            self.month + 9
        };
        let day_of_year = (153 * month_index + 2) / 5 + self.day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days = (era * DAYS_PER_ERA + day_of_era).checked_sub(EPOCH_SHIFT)?;
        let moment = days * SECONDS_PER_DAY + self.hour * 3600 + self.minute * 60 + self.second;
        (Self::of(moment) == self).then_some(moment)
    }
}

/// A short text of at most 32 bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    bytes: [u8; 32],
    length: usize,
}

impl core::fmt::Debug for Stamp {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Write for Stamp {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        if end > self.bytes.len() {
            return Err(core::fmt::Error);
        }
        self.bytes[self.length..end].copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl Stamp {
    /// The empty text.
    pub const fn new() -> Self {
        Self {
            bytes: [0; 32],
            length: 0,
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Default for Stamp {
    fn default() -> Self {
        Self::new()
    }
}

/// `seconds` as the list shows it: `YYYY-MM-DD HH:MM`.
pub fn list_time(seconds: u64) -> Stamp {
    let moment = Moment::of(seconds);
    let mut stamp = Stamp::new();
    let _ = write!(
        stamp,
        "{:04}-{:02}-{:02} {:02}:{:02}",
        moment.year, moment.month, moment.day, moment.hour, moment.minute
    );
    stamp
}

/// `seconds` as a `.trashinfo` file's `DeletionDate`: `YYYY-MM-DDThh:mm:ss`.
pub fn deletion_date(seconds: u64) -> Stamp {
    let moment = Moment::of(seconds);
    let mut stamp = Stamp::new();
    let _ = write!(
        stamp,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        moment.year, moment.month, moment.day, moment.hour, moment.minute, moment.second
    );
    stamp
}

fn number(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bytes.iter().try_fold(0u64, |value, &digit| {
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

/// A `DeletionDate` read back: seconds since the epoch.
pub fn parse_deletion_date(text: &[u8]) -> Option<u64> {
    if text.len() != 19
        || text[4] != b'-'
        || text[7] != b'-'
        || text[10] != b'T'
        || text[13] != b':'
        || text[16] != b':'
    {
        return None;
    }
    Moment {
        year: number(&text[0..4])?,
        month: number(&text[5..7])?,
        day: number(&text[8..10])?,
        hour: number(&text[11..13])?,
        minute: number(&text[14..16])?,
        second: number(&text[17..19])?,
    }
    .seconds()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_become_civil_dates_and_back() {
        assert_eq!(list_time(0).as_str(), "1970-01-01 00:00");
        assert_eq!(list_time(192).as_str(), "1970-01-01 00:03");
        assert_eq!(list_time(1_700_000_000).as_str(), "2023-11-14 22:13");
        assert_eq!(deletion_date(951_782_400).as_str(), "2000-02-29T00:00:00");
        for seconds in [0, 59, 86_399, 951_782_400, 1_700_000_000, 4_102_444_800] {
            let text = deletion_date(seconds);
            assert_eq!(parse_deletion_date(text.as_str().as_bytes()), Some(seconds));
        }
    }

    #[test]
    fn a_deletion_date_that_is_no_date_is_refused() {
        for text in [
            &b"2023-02-30T00:00:00"[..],
            b"2023-13-01T00:00:00",
            b"1969-12-31T23:59:59",
            b"2023-01-01 00:00:00",
            b"2023-01-01T00:00",
            b"20x3-01-01T00:00:00",
        ] {
            assert_eq!(parse_deletion_date(text), None, "{:?}", text.escape_ascii());
        }
    }
}
