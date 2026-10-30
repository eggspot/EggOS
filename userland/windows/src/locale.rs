//! The layer's locales (D178): English (United States), `en-US` (LCID 0x0409), every program's
//! user and system locale, whose ANSI and OEM code page is UTF-8 (65001, D118), and the
//! invariant locale. Their date and time pictures as `GetDateFormat` and `GetTimeFormat` read
//! them (Microsoft's "Day, Month, Year, and Era Format Pictures" and "Hour, Minute, and Second
//! Format Pictures"), the character types `GetStringType` reports and the sort keys
//! `LCMapString` makes, which order as the layer's ordinal comparisons do.

use crate::calendar::SystemTime;

/// `en-US`'s locale identifier.
pub const UNITED_STATES: u32 = 0x0409;

/// A locale the layer formats for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    UnitedStates,
    Invariant,
}

/// Which of a locale's standard pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    ShortDate,
    LongDate,
    YearMonth,
    MonthDay,
    Time,
}

impl Locale {
    /// The locale's standard picture.
    pub fn picture(self, standard: Standard) -> &'static str {
        match (self, standard) {
            (Locale::UnitedStates, Standard::ShortDate) => "M/d/yyyy",
            (Locale::UnitedStates, Standard::LongDate) => "dddd, MMMM d, yyyy",
            (Locale::UnitedStates, Standard::YearMonth) => "MMMM yyyy",
            (Locale::UnitedStates, Standard::MonthDay) => "MMMM d",
            (Locale::UnitedStates, Standard::Time) => "h:mm:ss tt",
            (Locale::Invariant, Standard::ShortDate) => "MM/dd/yyyy",
            (Locale::Invariant, Standard::LongDate) => "dddd, dd MMMM yyyy",
            (Locale::Invariant, Standard::YearMonth) => "yyyy MMMM",
            (Locale::Invariant, Standard::MonthDay) => "MMMM dd",
            (Locale::Invariant, Standard::Time) => "HH:mm:ss",
        }
    }
}

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

/// `GetTimeFormat`'s flags.
pub mod time_flags {
    pub const NO_MINUTES_OR_SECONDS: u32 = 0x1;
    pub const NO_SECONDS: u32 = 0x2;
    pub const NO_TIME_MARKER: u32 = 0x4;
    pub const FORCE_24_HOUR: u32 = 0x8;
}

enum Token {
    Literal(char),
    Element(char, usize),
}

fn tokens(picture: impl Iterator<Item = char>, letters: &str) -> impl Iterator<Item = Token> {
    let mut picture = picture.peekable();
    let mut quoted = false;
    core::iter::from_fn(move || {
        loop {
            let c = picture.next()?;
            if c == '\'' {
                if picture.peek() == Some(&'\'') {
                    picture.next();
                    return Some(Token::Literal('\''));
                }
                quoted = !quoted;
                continue;
            }
            if quoted || !letters.contains(c) {
                return Some(Token::Literal(c));
            }
            let mut count = 1;
            while picture.peek() == Some(&c) {
                picture.next();
                count += 1;
            }
            return Some(Token::Element(c, count));
        }
    })
}

fn number(value: u32, width: usize, out: &mut impl FnMut(char)) {
    let mut digits = [0u8; 10];
    let mut length = 0;
    let mut rest = value;
    loop {
        digits[length] = b'0' + (rest % 10) as u8;
        length += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for _ in length..width {
        out('0');
    }
    for digit in digits[..length].iter().rev() {
        out(char::from(*digit));
    }
}

fn text(word: &str, letters: usize, out: &mut impl FnMut(char)) {
    word.chars().take(letters).for_each(out);
}

/// Writes `date` through `out` as the date picture `picture` says (`d`, `dd`, `ddd`, `dddd`,
/// `M` to `MMMM`, `y`, `yy`, `yyyy`, `g`, quoted text); `date`'s day of the week must be right.
pub fn format_date(
    picture: impl Iterator<Item = char>,
    date: &SystemTime,
    out: &mut impl FnMut(char),
) {
    for token in tokens(picture, "dMyg") {
        match token {
            Token::Literal(c) => out(c),
            Token::Element('d', width @ (1 | 2)) => number(u32::from(date.day), width, out),
            Token::Element('M', width @ (1 | 2)) => number(u32::from(date.month), width, out),
            Token::Element('d', 3) => text(DAYS[usize::from(date.day_of_week % 7)], 3, out),
            Token::Element('d', _) => text(DAYS[usize::from(date.day_of_week % 7)], 9, out),
            Token::Element('M', 3) => {
                text(MONTHS[usize::from(date.month.clamp(1, 12) - 1)], 3, out)
            }
            Token::Element('M', _) => {
                text(MONTHS[usize::from(date.month.clamp(1, 12) - 1)], 9, out)
            }
            Token::Element('y', 1) => number(u32::from(date.year % 10), 1, out),
            Token::Element('y', 2) => number(u32::from(date.year % 100), 2, out),
            Token::Element('y', _) => number(u32::from(date.year), 1, out),
            Token::Element(_, _) => text("A.D.", 4, out),
        }
    }
}

/// Writes `time` through `out` as the time picture `picture` says (`h`, `hh`, `H`, `HH`, `m`,
/// `mm`, `s`, `ss`, `t`, `tt`, quoted text) with `GetTimeFormat`'s `flags` ([`time_flags`]): an
/// element a flag leaves out takes the text before it along.
pub fn format_time(
    picture: impl Iterator<Item = char>,
    time: &SystemTime,
    flags: u32,
    out: &mut impl FnMut(char),
) {
    let mut pending = [' '; 32];
    let mut pending_length = 0;
    let mut emitted_element = false;
    for token in tokens(picture, "hHmst") {
        let (letter, count) = match token {
            Token::Literal(c) => {
                if !emitted_element || pending_length == pending.len() {
                    out(c);
                } else {
                    pending[pending_length] = c;
                    pending_length += 1;
                }
                continue;
            }
            Token::Element(letter, count) => (letter, count),
        };
        let left_out = match letter {
            'm' => flags & time_flags::NO_MINUTES_OR_SECONDS != 0,
            's' => flags & (time_flags::NO_MINUTES_OR_SECONDS | time_flags::NO_SECONDS) != 0,
            't' => flags & (time_flags::NO_TIME_MARKER | time_flags::FORCE_24_HOUR) != 0,
            _ => false,
        };
        if left_out {
            pending_length = 0;
            continue;
        }
        pending[..pending_length]
            .iter()
            .copied()
            .for_each(&mut *out);
        pending_length = 0;
        emitted_element = true;
        let width = count.min(2);
        let twelve_hour = match time.hour % 12 {
            0 => 12,
            hour => hour,
        };
        match letter {
            'h' if flags & time_flags::FORCE_24_HOUR == 0 => {
                number(u32::from(twelve_hour), width, out)
            }
            'h' | 'H' => number(u32::from(time.hour), width, out),
            'm' => number(u32::from(time.minute), width, out),
            's' => number(u32::from(time.second), width, out),
            _ => {
                let marker = if time.hour < 12 { "AM" } else { "PM" };
                text(marker, width, out);
            }
        }
    }
    pending[..pending_length].iter().copied().for_each(out);
}

/// `GetStringType`'s information types.
pub mod character_type {
    pub const CTYPE1: u32 = 1;
    pub const CTYPE2: u32 = 2;
    pub const CTYPE3: u32 = 4;
}

/// The bits `GetStringType` reports for UTF-16 unit `unit` under `information`
/// ([`character_type`]); `None` for an unknown type.
pub fn character_type(information: u32, unit: u16) -> Option<u16> {
    let surrogate = (0xd800..0xe000).contains(&unit);
    let c = char::from_u32(u32::from(unit));
    let alphabetic = c.is_some_and(char::is_alphabetic);
    let digit = c.is_some_and(|c| c.is_ascii_digit());
    let space = c.is_some_and(char::is_whitespace);
    let control = c.is_some_and(char::is_control);
    let punctuation =
        c.is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control());
    Some(match information {
        character_type::CTYPE1 => {
            if surrogate {
                return Some(0);
            }
            let mut bits = 0x200;
            let upper = c.is_some_and(char::is_uppercase);
            let lower = c.is_some_and(char::is_lowercase);
            for (set, bit) in [
                (upper, 0x1),
                (lower, 0x2),
                (digit, 0x4),
                (space, 0x8),
                (punctuation, 0x10),
                (control, 0x20),
                (matches!(unit, 0x09 | 0x20 | 0xa0 | 0x3000), 0x40),
                (c.is_some_and(|c| c.is_ascii_hexdigit()), 0x80),
                (alphabetic, 0x100),
            ] {
                if set {
                    bits |= bit;
                }
            }
            bits
        }
        character_type::CTYPE2 => match unit {
            0x30..=0x39 => 3,
            0x2b | 0x2d => 4,
            0x23..=0x25 | 0xa2..=0xa5 | 0xb0 | 0xb1 => 5,
            0x2c | 0x2e | 0x2f | 0x3a => 7,
            0x0a | 0x0d | 0x1c..=0x1e | 0x85 | 0x2029 => 8,
            0x09 | 0x0b | 0x1f => 9,
            0x0c | 0x20 | 0x2028 => 10,
            0x0590..=0x05ff | 0x0600..=0x06ff => 2,
            _ if alphabetic => 1,
            _ if control || surrogate => 0,
            _ => 11,
        },
        character_type::CTYPE3 => {
            let mut bits = 0;
            for (set, bit) in [
                (
                    c.is_some_and(|c| c.is_ascii_punctuation()) || (punctuation && unit > 0x7f),
                    0x8,
                ),
                ((0x30a0..=0x30ff).contains(&unit), 0x10),
                ((0x3040..=0x309f).contains(&unit), 0x20),
                (
                    (0xff61..=0xffdc).contains(&unit) || (0x20..0x7f).contains(&unit),
                    0x40,
                ),
                ((0xff01..=0xff60).contains(&unit) || unit == 0x3000, 0x80),
                (
                    (0x4e00..=0x9fff).contains(&unit) || (0x3400..=0x4dbf).contains(&unit),
                    0x100,
                ),
                ((0xd800..0xdc00).contains(&unit), 0x800),
                ((0xdc00..0xe000).contains(&unit), 0x1000),
                (alphabetic || digit, 0x8000),
                ((0x0300..=0x036f).contains(&unit), 0x1 | 0x2),
            ] {
                if set {
                    bits |= bit;
                }
            }
            bits
        }
        _ => return None,
    })
}

/// The three bytes UTF-16 unit `unit` takes in a sort key: none of them zero, so the key's
/// final zero ends it, and keys compare byte by byte as their units compare.
pub fn sort_key_bytes(unit: u16) -> [u8; 3] {
    [
        (unit >> 14) as u8 + 1,
        ((unit >> 7) & 0x7f) as u8 + 1,
        (unit & 0x7f) as u8 + 1,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(year: u16, month: u16, day: u16, hour: u16, minute: u16, second: u16) -> SystemTime {
        let mut time = SystemTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            ..SystemTime::default()
        };
        let file_time = crate::calendar::file_time(&time).unwrap();
        time.day_of_week = crate::calendar::system_time(file_time).unwrap().day_of_week;
        time
    }

    fn date_text(picture: &str, at: &SystemTime) -> String {
        let mut out = String::new();
        format_date(picture.chars(), at, &mut |c| out.push(c));
        out
    }

    fn time_text(picture: &str, at: &SystemTime, flags: u32) -> String {
        let mut out = String::new();
        format_time(picture.chars(), at, flags, &mut |c| out.push(c));
        out
    }

    #[test]
    fn date_pictures_follow_microsofts_elements() {
        let at = time(2026, 3, 7, 0, 0, 0);
        let us = Locale::UnitedStates;
        assert_eq!(date_text(us.picture(Standard::ShortDate), &at), "3/7/2026");
        assert_eq!(
            date_text(us.picture(Standard::LongDate), &at),
            "Saturday, March 7, 2026"
        );
        assert_eq!(
            date_text(us.picture(Standard::YearMonth), &at),
            "March 2026"
        );
        assert_eq!(date_text("dd.MM.yy ddd MMM", &at), "07.03.26 Sat Mar");
        assert_eq!(date_text("y yyyyy g", &at), "6 2026 A.D.");
        assert_eq!(date_text("'day' d 'o''clock' ''", &at), "day 7 o'clock '");
        assert_eq!(date_text("hh:mm", &at), "hh:mm");
        assert_eq!(
            date_text(Locale::Invariant.picture(Standard::ShortDate), &at),
            "03/07/2026"
        );
    }

    #[test]
    fn time_pictures_follow_microsofts_elements_and_flags() {
        let morning = time(2026, 3, 7, 0, 5, 9);
        let evening = time(2026, 3, 7, 17, 45, 30);
        let us = Locale::UnitedStates.picture(Standard::Time);
        assert_eq!(time_text(us, &morning, 0), "12:05:09 AM");
        assert_eq!(time_text(us, &evening, 0), "5:45:30 PM");
        assert_eq!(time_text("HH:mm:ss t", &evening, 0), "17:45:30 P");
        assert_eq!(time_text(us, &evening, time_flags::NO_SECONDS), "5:45 PM");
        assert_eq!(
            time_text(us, &evening, time_flags::NO_MINUTES_OR_SECONDS),
            "5 PM"
        );
        assert_eq!(
            time_text(us, &evening, time_flags::NO_TIME_MARKER),
            "5:45:30"
        );
        assert_eq!(
            time_text(us, &evening, time_flags::FORCE_24_HOUR),
            "17:45:30"
        );
        assert_eq!(time_text("hh 'h' mm", &morning, 0), "12 h 05");
        assert_eq!(time_text("dd hh", &morning, 0), "dd 12");
    }

    #[test]
    fn character_types_classify_letters_digits_spaces_and_marks() {
        use character_type::*;
        assert_eq!(
            character_type(CTYPE1, u16::from(b'A')),
            Some(0x200 | 0x1 | 0x80 | 0x100)
        );
        assert_eq!(
            character_type(CTYPE1, u16::from(b'z')),
            Some(0x200 | 0x2 | 0x100)
        );
        assert_eq!(
            character_type(CTYPE1, u16::from(b'7')),
            Some(0x200 | 0x4 | 0x80)
        );
        assert_eq!(
            character_type(CTYPE1, u16::from(b' ')),
            Some(0x200 | 0x8 | 0x40)
        );
        assert_eq!(character_type(CTYPE1, u16::from(b'!')), Some(0x200 | 0x10));
        assert_eq!(character_type(CTYPE1, 0x0a), Some(0x200 | 0x8 | 0x20));
        assert_eq!(character_type(CTYPE1, 0xe9), Some(0x200 | 0x2 | 0x100));
        assert_eq!(character_type(CTYPE2, u16::from(b'5')), Some(3));
        assert_eq!(character_type(CTYPE2, u16::from(b'a')), Some(1));
        assert_eq!(character_type(CTYPE2, u16::from(b' ')), Some(10));
        assert_eq!(character_type(CTYPE3, 0x3042), Some(0x20 | 0x8000));
        assert_eq!(character_type(CTYPE3, 0xd83d), Some(0x800));
        assert_eq!(character_type(3, 0x41), None);
    }

    #[test]
    fn sort_keys_order_as_units_and_hold_no_zero() {
        let mut previous = [0u8; 3];
        for unit in [0u16, 1, 0x41, 0x7f, 0x80, 0x3fff, 0x4000, 0xffff] {
            let key = sort_key_bytes(unit);
            assert!(key.iter().all(|byte| *byte != 0));
            assert!(key > previous, "{unit:#x}");
            previous = key;
        }
    }
}
