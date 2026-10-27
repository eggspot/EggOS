//! The proleptic Gregorian calendar in UTC, for `gmtime_r`, `timegm` and `strftime` in the C
//! locale: days are counted from 1970-01-01 with the civil-from-days and days-from-civil
//! algorithms (eras of 400 years), which hold for every year an `i64` count of seconds reaches.

const SECONDS_PER_DAY: i64 = 86_400;
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
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

/// A time broken down as `struct tm` holds it: the month from 0, the year from 1900, the
/// weekday from Sunday and the day of the year from 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BrokenDown {
    pub second: i32,
    pub minute: i32,
    pub hour: i32,
    pub day: i32,
    pub month: i32,
    pub year: i32,
    pub weekday: i32,
    pub year_day: i32,
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `seconds` since 1970-01-01 00:00:00 UTC, broken down; `None` if the year does not fit
/// `struct tm`.
pub fn from_seconds(seconds: i64) -> Option<BrokenDown> {
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let year_day = days - days_from_civil(year, 1, 1);
    Some(BrokenDown {
        second: (of_day % 60) as i32,
        minute: (of_day / 60 % 60) as i32,
        hour: (of_day / 3600) as i32,
        day: day as i32,
        month: (month - 1) as i32,
        year: i32::try_from(year - 1900).ok()?,
        weekday: (days + 4).rem_euclid(7) as i32,
        year_day: year_day as i32,
    })
}

/// The seconds since 1970-01-01 00:00:00 UTC of `time`, whose fields may be out of their
/// ranges (`timegm` normalizes them: month 12 is January of the next year).
pub fn to_seconds(time: &BrokenDown) -> i64 {
    let month = i64::from(time.month);
    let year = i64::from(time.year) + 1900 + month.div_euclid(12);
    let month = month.rem_euclid(12) + 1;
    let days = days_from_civil(year, month, 1) + i64::from(time.day) - 1;
    days * SECONDS_PER_DAY
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second)
}

struct Output<'a> {
    bytes: &'a mut [u8],
    length: usize,
    overflowed: bool,
}

impl Output<'_> {
    fn push(&mut self, text: &[u8]) {
        match self.bytes.get_mut(self.length..self.length + text.len()) {
            Some(room) => {
                room.copy_from_slice(text);
                self.length += text.len();
            }
            None => self.overflowed = true,
        }
    }

    fn number(&mut self, value: i64, width: usize, pad: u8) {
        let mut digits = [0u8; 24];
        let mut at = digits.len();
        let negative = value < 0;
        let mut rest = value.unsigned_abs();
        loop {
            at -= 1;
            digits[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        let length = digits.len() - at + usize::from(negative);
        if negative {
            self.push(b"-");
        }
        for _ in length..width {
            self.push(&[pad]);
        }
        let digits = &digits[at..];
        self.push(digits);
    }
}

fn twelve_hour(hour: i32) -> i64 {
    match hour % 12 {
        0 => 12,
        other => i64::from(other),
    }
}

fn name(names: &[&'static str], index: i32) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|index| names.get(index))
        .copied()
        .unwrap_or("?")
}

fn convert(out: &mut Output<'_>, conversion: u8, time: &BrokenDown, seconds: i64) {
    let year = i64::from(time.year) + 1900;
    match conversion {
        b'a' => out.push(&name(&DAYS, time.weekday).as_bytes()[..3]),
        b'A' => out.push(name(&DAYS, time.weekday).as_bytes()),
        b'b' | b'h' => out.push(&name(&MONTHS, time.month).as_bytes()[..3]),
        b'B' => out.push(name(&MONTHS, time.month).as_bytes()),
        b'c' => format_into(out, b"%a %b %e %H:%M:%S %Y", time, seconds),
        b'C' => out.number(year.div_euclid(100), 2, b'0'),
        b'd' => out.number(i64::from(time.day), 2, b'0'),
        b'D' | b'x' => format_into(out, b"%m/%d/%y", time, seconds),
        b'e' => out.number(i64::from(time.day), 2, b' '),
        b'F' => format_into(out, b"%Y-%m-%d", time, seconds),
        b'H' => out.number(i64::from(time.hour), 2, b'0'),
        b'I' => out.number(twelve_hour(time.hour), 2, b'0'),
        b'j' => out.number(i64::from(time.year_day) + 1, 3, b'0'),
        b'k' => out.number(i64::from(time.hour), 2, b' '),
        b'l' => out.number(twelve_hour(time.hour), 2, b' '),
        b'm' => out.number(i64::from(time.month) + 1, 2, b'0'),
        b'M' => out.number(i64::from(time.minute), 2, b'0'),
        b'n' => out.push(b"\n"),
        b'p' => out.push(if time.hour < 12 { b"AM" } else { b"PM" }),
        b'r' => format_into(out, b"%I:%M:%S %p", time, seconds),
        b'R' => format_into(out, b"%H:%M", time, seconds),
        b's' => out.number(seconds, 1, b'0'),
        b'S' => out.number(i64::from(time.second), 2, b'0'),
        b't' => out.push(b"\t"),
        b'T' | b'X' => format_into(out, b"%H:%M:%S", time, seconds),
        b'u' => out.number(i64::from((time.weekday + 6) % 7 + 1), 1, b'0'),
        b'w' => out.number(i64::from(time.weekday), 1, b'0'),
        b'y' => out.number(year.rem_euclid(100), 2, b'0'),
        b'Y' => out.number(year, 1, b'0'),
        b'z' => out.push(b"+0000"),
        b'Z' => out.push(b"UTC"),
        b'%' => out.push(b"%"),
        other => out.push(&[b'%', other]),
    }
}

fn format_into(out: &mut Output<'_>, format: &[u8], time: &BrokenDown, seconds: i64) {
    let mut bytes = format.iter().copied();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            out.push(&[byte]);
            continue;
        }
        let mut conversion = bytes.next();
        if matches!(conversion, Some(b'E' | b'O')) {
            conversion = bytes.next();
        }
        match conversion {
            Some(conversion) => convert(out, conversion, time, seconds),
            None => out.push(b"%"),
        }
    }
}

/// `strftime` in the C locale and UTC: `format` expanded for `time` into `buffer`, with room
/// left for the NUL; `None` if it does not fit, when `strftime` returns 0.
pub fn format(format: &[u8], time: &BrokenDown, buffer: &mut [u8]) -> Option<usize> {
    let seconds = to_seconds(time);
    let mut out = Output {
        bytes: buffer,
        length: 0,
        overflowed: false,
    };
    format_into(&mut out, format, time, seconds);
    (!out.overflowed && out.length < out.bytes.len()).then_some(out.length)
}
