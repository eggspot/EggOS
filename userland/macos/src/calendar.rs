//! The proleptic Gregorian calendar in UTC, for `gmtime_r`, `timegm`, `strftime` and `strptime`
//! in the C and en_US locales, which differ in `%x` and `%c` only: days are counted from
//! 1970-01-01 with the civil-from-days and days-from-civil algorithms (eras of 400 years), which
//! hold for every year an `i64` count of seconds reaches.

use core::ffi::CStr;

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

/// The pictures `%c` and `%x` stand for in a locale's `LC_TIME`, as `nl_langinfo` gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeFormats {
    pub date_time: &'static CStr,
    pub date: &'static CStr,
}

impl TimeFormats {
    pub const C: Self = Self {
        date_time: c"%a %b %e %H:%M:%S %Y",
        date: c"%m/%d/%y",
    };
    pub const UNITED_STATES: Self = Self {
        date_time: c"%a %b %e %X %Y",
        date: c"%m/%d/%Y",
    };
}

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

fn abbreviated(names: &[&'static str], index: i32) -> &'static [u8] {
    let name = name(names, index).as_bytes();
    &name[..name.len().min(3)]
}

fn convert(
    out: &mut Output<'_>,
    formats: TimeFormats,
    conversion: u8,
    time: &BrokenDown,
    seconds: i64,
) {
    let year = i64::from(time.year) + 1900;
    match conversion {
        b'a' => out.push(abbreviated(&DAYS, time.weekday)),
        b'A' => out.push(name(&DAYS, time.weekday).as_bytes()),
        b'b' | b'h' => out.push(abbreviated(&MONTHS, time.month)),
        b'B' => out.push(name(&MONTHS, time.month).as_bytes()),
        b'c' => format_into(out, formats, formats.date_time.to_bytes(), time, seconds),
        b'C' => out.number(year.div_euclid(100), 2, b'0'),
        b'd' => out.number(i64::from(time.day), 2, b'0'),
        b'D' => format_into(out, formats, b"%m/%d/%y", time, seconds),
        b'x' => format_into(out, formats, formats.date.to_bytes(), time, seconds),
        b'e' => out.number(i64::from(time.day), 2, b' '),
        b'F' => format_into(out, formats, b"%Y-%m-%d", time, seconds),
        b'H' => out.number(i64::from(time.hour), 2, b'0'),
        b'I' => out.number(twelve_hour(time.hour), 2, b'0'),
        b'j' => out.number(i64::from(time.year_day) + 1, 3, b'0'),
        b'k' => out.number(i64::from(time.hour), 2, b' '),
        b'l' => out.number(twelve_hour(time.hour), 2, b' '),
        b'm' => out.number(i64::from(time.month) + 1, 2, b'0'),
        b'M' => out.number(i64::from(time.minute), 2, b'0'),
        b'n' => out.push(b"\n"),
        b'p' => out.push(if time.hour < 12 { b"AM" } else { b"PM" }),
        b'r' => format_into(out, formats, b"%I:%M:%S %p", time, seconds),
        b'R' => format_into(out, formats, b"%H:%M", time, seconds),
        b's' => out.number(seconds, 1, b'0'),
        b'S' => out.number(i64::from(time.second), 2, b'0'),
        b't' => out.push(b"\t"),
        b'T' | b'X' => format_into(out, formats, b"%H:%M:%S", time, seconds),
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

fn format_into(
    out: &mut Output<'_>,
    formats: TimeFormats,
    format: &[u8],
    time: &BrokenDown,
    seconds: i64,
) {
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
            Some(conversion) => convert(out, formats, conversion, time, seconds),
            None => out.push(b"%"),
        }
    }
}

/// `strftime` in the C locale and UTC: `format` expanded for `time` into `buffer`, with room
/// left for the NUL; `None` if it does not fit, when `strftime` returns 0.
pub fn format(format: &[u8], time: &BrokenDown, buffer: &mut [u8]) -> Option<usize> {
    format_in(TimeFormats::C, format, time, buffer)
}

/// [`format`] in a locale whose `%c` and `%x` are `formats`.
pub fn format_in(
    formats: TimeFormats,
    format: &[u8],
    time: &BrokenDown,
    buffer: &mut [u8],
) -> Option<usize> {
    let seconds = to_seconds(time);
    let mut out = Output {
        bytes: buffer,
        length: 0,
        overflowed: false,
    };
    format_into(&mut out, formats, format, time, seconds);
    (!out.overflowed && out.length < out.bytes.len()).then_some(out.length)
}

/// What [`parse`] read besides the broken-down time: how many bytes of the text it used, the
/// offset from UTC `%z` gave, and whether `%Z` or `%s` named standard time (`tm_isdst` 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Parsed {
    pub used: usize,
    pub offset: Option<i64>,
    pub standard_time: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Seen {
    year: bool,
    month: bool,
    day: bool,
    year_day: bool,
    weekday: bool,
    century: Option<i32>,
    short_year: Option<i32>,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t'..=b'\r')
}

fn starts_with_ignoring_case(text: &[u8], prefix: &[u8]) -> bool {
    text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

struct Parser<'t> {
    text: &'t [u8],
    at: usize,
    formats: TimeFormats,
    seen: Seen,
    parsed: Parsed,
}

impl Parser<'_> {
    fn rest(&self) -> &[u8] {
        &self.text[self.at..]
    }

    fn skip_space(&mut self) {
        while self.rest().first().is_some_and(|&byte| is_space(byte)) {
            self.at += 1;
        }
    }

    fn literal(&mut self, expected: u8) -> Option<()> {
        (self.rest().first() == Some(&expected)).then(|| self.at += 1)
    }

    fn number(&mut self, digits: usize, low: i32, high: i32) -> Option<i32> {
        let start = self.at;
        let mut value = 0;
        while self.at - start < digits {
            match self.rest().first() {
                Some(&byte) if byte.is_ascii_digit() => {
                    value = value * 10 + i32::from(byte - b'0');
                    self.at += 1;
                }
                _ => break,
            }
        }
        (self.at > start && (low..=high).contains(&value)).then_some(value)
    }

    fn name(&mut self, names: &[&str]) -> Option<i32> {
        for (index, name) in names.iter().enumerate() {
            let name = name.as_bytes();
            for candidate in [name, &name[..3]] {
                if starts_with_ignoring_case(self.rest(), candidate) {
                    self.at += candidate.len();
                    return Some(index as i32);
                }
            }
        }
        None
    }

    fn meridiem(&mut self, time: &mut BrokenDown) -> Option<()> {
        let afternoon = if starts_with_ignoring_case(self.rest(), b"AM") {
            false
        } else if starts_with_ignoring_case(self.rest(), b"PM") {
            true
        } else {
            return None;
        };
        if time.hour > 12 {
            return None;
        }
        self.at += 2;
        if afternoon && time.hour != 12 {
            time.hour += 12;
        } else if !afternoon && time.hour == 12 {
            time.hour = 0;
        }
        Some(())
    }

    fn seconds(&mut self, time: &mut BrokenDown) -> Option<()> {
        let negative = self.literal(b'-').is_some();
        let start = self.at;
        let mut value: i64 = 0;
        while let Some(byte) = self.rest().first().copied().filter(u8::is_ascii_digit) {
            value = value.checked_mul(10)?.checked_add(i64::from(byte - b'0'))?;
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        *time = from_seconds(if negative { -value } else { value })?;
        self.seen = Seen {
            year: true,
            month: true,
            day: true,
            year_day: true,
            weekday: true,
            ..Seen::default()
        };
        self.parsed.standard_time = true;
        Some(())
    }

    fn zone_offset(&mut self) -> Option<()> {
        let sign: i64 = match self.rest().first() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            Some(b'Z') => {
                self.at += 1;
                self.parsed.offset = Some(0);
                return Some(());
            }
            _ => return None,
        };
        self.at += 1;
        let hours = self.number(2, 0, 23)?;
        let _ = self.literal(b':');
        let minutes = self.number(2, 0, 59)?;
        self.parsed.offset = Some(sign * (i64::from(hours) * 3600 + i64::from(minutes) * 60));
        Some(())
    }

    fn zone_name(&mut self) -> Option<()> {
        if !(self.rest().starts_with(b"UTC") || self.rest().starts_with(b"GMT")) {
            return None;
        }
        self.at += 3;
        self.parsed.standard_time = true;
        Some(())
    }

    fn run(&mut self, time: &mut BrokenDown, format: &[u8]) -> Option<()> {
        let mut bytes = format.iter().copied();
        while let Some(byte) = bytes.next() {
            if is_space(byte) {
                self.skip_space();
                continue;
            }
            if byte != b'%' {
                self.literal(byte)?;
                continue;
            }
            let mut conversion = bytes.next()?;
            if matches!(conversion, b'E' | b'O') {
                conversion = bytes.next()?;
            }
            self.convert(time, conversion)?;
        }
        Some(())
    }

    fn convert(&mut self, time: &mut BrokenDown, conversion: u8) -> Option<()> {
        if matches!(conversion, b'e' | b'k' | b'l') {
            self.skip_space();
        }
        match conversion {
            b'a' | b'A' => {
                time.weekday = self.name(&DAYS)?;
                self.seen.weekday = true;
            }
            b'b' | b'B' | b'h' => {
                time.month = self.name(&MONTHS)?;
                self.seen.month = true;
            }
            b'c' => {
                let picture = self.formats.date_time.to_bytes();
                self.run(time, picture)?;
            }
            b'C' => self.seen.century = Some(self.number(2, 0, 99)?),
            b'd' | b'e' => {
                time.day = self.number(2, 0, 31)?;
                self.seen.day = true;
            }
            b'D' => self.run(time, b"%m/%d/%y")?,
            b'F' => self.run(time, b"%Y-%m-%d")?,
            b'H' | b'k' => time.hour = self.number(2, 0, 23)?,
            b'I' | b'l' => time.hour = self.number(2, 1, 12)?,
            b'j' => {
                time.year_day = self.number(3, 1, 366)? - 1;
                self.seen.year_day = true;
            }
            b'm' => {
                time.month = self.number(2, 1, 12)? - 1;
                self.seen.month = true;
            }
            b'M' => time.minute = self.number(2, 0, 59)?,
            b'n' | b't' => self.skip_space(),
            b'p' => self.meridiem(time)?,
            b'r' => self.run(time, b"%I:%M:%S %p")?,
            b'R' => self.run(time, b"%H:%M")?,
            b's' => self.seconds(time)?,
            b'S' => time.second = self.number(2, 0, 60)?,
            b'T' | b'X' => self.run(time, b"%H:%M:%S")?,
            b'u' => {
                time.weekday = self.number(1, 1, 7)? % 7;
                self.seen.weekday = true;
            }
            b'w' => {
                time.weekday = self.number(1, 0, 6)?;
                self.seen.weekday = true;
            }
            b'x' => {
                let picture = self.formats.date.to_bytes();
                self.run(time, picture)?;
            }
            b'y' => self.seen.short_year = Some(self.number(2, 0, 99)?),
            b'Y' => {
                time.year = self.number(4, 0, 9999)? - 1900;
                self.seen.year = true;
                self.seen.century = None;
                self.seen.short_year = None;
            }
            b'z' => self.zone_offset()?,
            b'Z' => self.zone_name()?,
            b'%' => self.literal(b'%')?,
            _ => return None,
        }
        Some(())
    }

    fn finish(&self, time: &mut BrokenDown) {
        let seen = self.seen;
        if seen.century.is_some() || seen.short_year.is_some() {
            let short = seen.short_year.unwrap_or(0);
            let century = seen.century.unwrap_or(if short < 69 { 20 } else { 19 });
            time.year = century * 100 + short - 1900;
        } else if !seen.year {
            return;
        }
        let year = i64::from(time.year) + 1900;
        let new_year = days_from_civil(year, 1, 1);
        let dated = seen.month && seen.day;
        if dated && !seen.year_day {
            let days = days_from_civil(year, i64::from(time.month) + 1, i64::from(time.day));
            time.year_day = (days - new_year) as i32;
        } else if seen.year_day && !dated {
            let (_, month, day) = civil_from_days(new_year + i64::from(time.year_day));
            time.month = (month - 1) as i32;
            time.day = day as i32;
        }
        if (dated || seen.year_day) && !seen.weekday {
            time.weekday = (new_year + i64::from(time.year_day) + 4).rem_euclid(7) as i32;
        }
    }
}

/// `strptime`: reads `text` as `format` says into `time`, changing only the fields the format
/// sets, and the day of the year and weekday when the year and the date are known; `None` when
/// the text does not match, leaving `time` as it was.
pub fn parse(
    text: &[u8],
    format: &[u8],
    formats: TimeFormats,
    time: &mut BrokenDown,
) -> Option<Parsed> {
    let mut parser = Parser {
        text,
        at: 0,
        formats,
        seen: Seen::default(),
        parsed: Parsed::default(),
    };
    let mut parsed_time = *time;
    parser.run(&mut parsed_time, format)?;
    parser.finish(&mut parsed_time);
    *time = parsed_time;
    Some(Parsed {
        used: parser.at,
        ..parser.parsed
    })
}
