//! Time zones (D194): POSIX TZ rules (POSIX.1-2017 section 8.3, with RFC 8536's extension of a
//! transition's hour to -167..167) and the IANA zones' current rules ([`zones::ZONES`]). A zone
//! is its current rule, so local times before its last change of rules are shown with today's
//! rule. EggOS keeps the clock in UTC; a zone only changes how a time is shown and read.

mod zones;

pub use zones::ZONES;

use crate::{SECONDS_PER_DAY, civil_from_days, days_from_civil, is_leap_year};

/// The most bytes of a zone abbreviation kept, such as `CEST` or `+0530`.
pub const MAX_ABBREVIATION_BYTES: usize = 8;
/// The most bytes of a zone name, such as `America/Argentina/ComodRivadavia`.
pub const MAX_ZONE_NAME_BYTES: usize = 40;

/// A zone's abbreviation, as `date` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abbreviation {
    bytes: [u8; MAX_ABBREVIATION_BYTES],
    length: usize,
}

impl Abbreviation {
    fn new(text: &[u8]) -> Option<Self> {
        if text.len() < 3 || text.len() > MAX_ABBREVIATION_BYTES {
            return None;
        }
        let mut bytes = [0u8; MAX_ABBREVIATION_BYTES];
        bytes[..text.len()].copy_from_slice(text);
        Some(Self {
            bytes,
            length: text.len(),
        })
    }

    /// The abbreviation.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("UTC")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Day {
    Julian(u16),
    Zero(u16),
    Month { month: u8, week: u8, weekday: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    day: Day,
    seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Daylight {
    abbreviation: Abbreviation,
    west: i64,
    start: Transition,
    end: Transition,
}

/// A zone's rule: its standard time and, when it has one, its daylight saving time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    abbreviation: Abbreviation,
    west: i64,
    daylight: Option<Daylight>,
}

/// The local time at an instant: its offset east of UTC, its abbreviation and whether it is
/// daylight saving time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    /// Seconds east of UTC (`+3600` for Paris in winter).
    pub offset: i64,
    /// The abbreviation.
    pub abbreviation: Abbreviation,
    /// Whether daylight saving time is in force.
    pub daylight: bool,
}

impl Local {
    /// The offset as `date +%z` writes it, `+0100`, `-0330`.
    pub fn numeric_offset(&self) -> [u8; 5] {
        let sign = if self.offset < 0 { b'-' } else { b'+' };
        let minutes = self.offset.abs() / 60;
        let hours = minutes / 60;
        let rest = minutes % 60;
        [
            sign,
            b'0' + (hours / 10 % 10) as u8,
            b'0' + (hours % 10) as u8,
            b'0' + (rest / 10) as u8,
            b'0' + (rest % 10) as u8,
        ]
    }
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn take(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn abbreviation(&mut self) -> Option<Abbreviation> {
        let start = self.at;
        if self.take(b'<') {
            let inner = self.at;
            while self.peek().is_some_and(|byte| byte != b'>') {
                self.at += 1;
            }
            let name = self.text.get(inner..self.at)?;
            if !self.take(b'>') {
                return None;
            }
            return Abbreviation::new(name);
        }
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            self.at += 1;
        }
        Abbreviation::new(self.text.get(start..self.at)?)
    }

    fn number(&mut self, most: i64) -> Option<i64> {
        let start = self.at;
        let mut value = 0i64;
        while let Some(digit) = self.peek().filter(u8::is_ascii_digit) {
            value = value * 10 + i64::from(digit - b'0');
            self.at += 1;
            if value > most {
                return None;
            }
        }
        (self.at > start).then_some(value)
    }

    fn time(&mut self, most_hours: i64) -> Option<i64> {
        let negative = self.take(b'-');
        if !negative {
            self.take(b'+');
        }
        let mut seconds = self.number(most_hours)? * 3600;
        if self.take(b':') {
            seconds += self.number(59)? * 60;
            if self.take(b':') {
                seconds += self.number(59)?;
            }
        }
        Some(if negative { -seconds } else { seconds })
    }

    fn transition(&mut self) -> Option<Transition> {
        let day = if self.take(b'J') {
            Day::Julian(
                u16::try_from(self.number(365)?)
                    .ok()
                    .filter(|&day| day >= 1)?,
            )
        } else if self.take(b'M') {
            let month = self.number(12)?;
            if !self.take(b'.') {
                return None;
            }
            let week = self.number(5)?;
            if !self.take(b'.') {
                return None;
            }
            let weekday = self.number(6)?;
            if month < 1 || week < 1 {
                return None;
            }
            Day::Month {
                month: month as u8,
                week: week as u8,
                weekday: weekday as u8,
            }
        } else {
            Day::Zero(u16::try_from(self.number(365)?).ok()?)
        };
        let seconds = match self.take(b'/') {
            true => self.time(167)?,
            false => 2 * 3600,
        };
        Some(Transition { day, seconds })
    }
}

/// Reads a POSIX TZ rule such as `CET-1CEST,M3.5.0,M10.5.0/3`; `None` when it is not one.
pub fn parse(text: &str) -> Option<Rule> {
    let mut reader = Reader {
        text: text.as_bytes(),
        at: 0,
    };
    let abbreviation = reader.abbreviation()?;
    let west = reader.time(24)?;
    let mut rule = Rule {
        abbreviation,
        west,
        daylight: None,
    };
    if reader.peek().is_none() {
        return Some(rule);
    }
    let daylight_abbreviation = reader.abbreviation()?;
    let daylight_west = match reader.peek() {
        Some(b',') | None => west - 3600,
        Some(_) => reader.time(24)?,
    };
    let (start, end) = match reader.take(b',') {
        true => {
            let start = reader.transition()?;
            if !reader.take(b',') {
                return None;
            }
            (start, reader.transition()?)
        }
        false => (
            Transition {
                day: Day::Month {
                    month: 3,
                    week: 2,
                    weekday: 0,
                },
                seconds: 2 * 3600,
            },
            Transition {
                day: Day::Month {
                    month: 11,
                    week: 1,
                    weekday: 0,
                },
                seconds: 2 * 3600,
            },
        ),
    };
    if reader.peek().is_some() {
        return None;
    }
    rule.daylight = Some(Daylight {
        abbreviation: daylight_abbreviation,
        west: daylight_west,
        start,
        end,
    });
    Some(rule)
}

/// The rule of the IANA zone `name`, such as `Europe/Paris`.
pub fn find(name: &str) -> Option<&'static str> {
    ZONES
        .binary_search_by(|(zone, _)| (*zone).cmp(name))
        .ok()
        .and_then(|index| ZONES.get(index))
        .map(|(_, rule)| *rule)
}

/// The rule of a zone given as `TZ` gives it: an IANA name (with or without a leading `:`) or a
/// POSIX rule.
pub fn zone(text: &str) -> Option<Rule> {
    let name = text.strip_prefix(':').unwrap_or(text);
    match find(name) {
        Some(rule) => parse(rule),
        None => parse(name),
    }
}

/// UTC.
pub const UTC: Rule = Rule {
    abbreviation: Abbreviation {
        bytes: *b"UTC\0\0\0\0\0",
        length: 3,
    },
    west: 0,
    daylight: None,
};

fn day_of_year(day: Day, year: i64) -> i64 {
    match day {
        Day::Julian(day) => {
            let day = i64::from(day) - 1;
            match is_leap_year(year) && day >= 59 {
                true => day + 1,
                false => day,
            }
        }
        Day::Zero(day) => i64::from(day),
        Day::Month {
            month,
            week,
            weekday,
        } => {
            let first = days_from_civil(year, u32::from(month), 1);
            let first_weekday = (first + 4).rem_euclid(7);
            let mut date =
                1 + (i64::from(weekday) - first_weekday).rem_euclid(7) + 7 * (i64::from(week) - 1);
            let last = i64::from(crate::days_in_month(year, u32::from(month)));
            while date > last {
                date -= 7;
            }
            first + date - 1 - days_from_civil(year, 1, 1)
        }
    }
}

fn transition_utc(transition: Transition, year: i64, west: i64) -> i64 {
    (days_from_civil(year, 1, 1) + day_of_year(transition.day, year)) * SECONDS_PER_DAY
        + transition.seconds
        + west
}

impl Rule {
    /// The local time at `utc`, seconds since the epoch.
    pub fn at(&self, utc: i64) -> Local {
        let standard = Local {
            offset: -self.west,
            abbreviation: self.abbreviation,
            daylight: false,
        };
        let Some(daylight) = self.daylight else {
            return standard;
        };
        let (year, _, _) = civil_from_days((utc - self.west).div_euclid(SECONDS_PER_DAY));
        let start = transition_utc(daylight.start, year, self.west);
        let end = transition_utc(daylight.end, year, daylight.west);
        let in_daylight = match start < end {
            true => start <= utc && utc < end,
            false => !(end <= utc && utc < start),
        };
        match in_daylight {
            true => Local {
                offset: -daylight.west,
                abbreviation: daylight.abbreviation,
                daylight: true,
            },
            false => standard,
        }
    }

    /// The instant a local time names (seconds since the epoch, counted as if local were UTC):
    /// the earlier one when the clocks went back, and the time after the gap when they went
    /// forward over it.
    pub fn to_utc(&self, local: i64) -> i64 {
        let guess = local - self.at(local).offset;
        let first = local - self.at(guess).offset;
        let second = local - self.at(first).offset;
        first.min(second)
    }

    /// Whether the zone has daylight saving time.
    pub fn has_daylight(&self) -> bool {
        self.daylight.is_some()
    }

    /// Standard time's offset, seconds east of UTC.
    pub fn standard_offset(&self) -> i64 {
        -self.west
    }

    /// Standard time's abbreviation.
    pub fn standard_abbreviation(&self) -> Abbreviation {
        self.abbreviation
    }

    /// Its daylight saving time as a rule of months, weeks and weekdays, the form Windows's
    /// `TIME_ZONE_INFORMATION` holds; `None` without daylight saving time or when a change is
    /// given as a day of the year.
    pub fn month_rules(&self) -> Option<MonthRules> {
        let daylight = self.daylight?;
        let month_rule = |transition: Transition| match transition.day {
            Day::Month {
                month,
                week,
                weekday,
            } => Some(MonthRule {
                month,
                week,
                weekday,
                seconds: transition.seconds,
            }),
            _ => None,
        };
        Some(MonthRules {
            start: month_rule(daylight.start)?,
            end: month_rule(daylight.end)?,
            daylight_offset: -daylight.west,
            daylight_abbreviation: daylight.abbreviation,
        })
    }
}

/// A change between standard and daylight saving time: the `weekday` (0 is Sunday) of `week`
/// (1 to 5, 5 the last) of `month` (1 to 12), at `seconds` past midnight local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthRule {
    /// The month, 1 to 12.
    pub month: u8,
    /// The week of the month, 1 to 5; 5 is the last.
    pub week: u8,
    /// The day of the week, 0 (Sunday) to 6.
    pub weekday: u8,
    /// Seconds past midnight, local time; may be negative or past a day.
    pub seconds: i64,
}

/// Daylight saving time as month rules ([`Rule::month_rules`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthRules {
    /// When daylight saving time starts, in standard time.
    pub start: MonthRule,
    /// When it ends, in daylight saving time.
    pub end: MonthRule,
    /// Its offset, seconds east of UTC.
    pub daylight_offset: i64,
    /// Its abbreviation.
    pub daylight_abbreviation: Abbreviation,
}

#[cfg(test)]
mod tests;
