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
            0x0590..=0x06ff => 2,
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

/// The C runtime's locale categories (`LC_*`, `locale.h`).
pub mod category {
    pub const ALL: i32 = 0;
    pub const COLLATE: i32 = 1;
    pub const CTYPE: i32 = 2;
    pub const MONETARY: i32 = 3;
    pub const NUMERIC: i32 = 4;
    pub const TIME: i32 = 5;
}

const CATEGORY_NAMES: [&str; 5] = [
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
];
const UTF8_CODE_PAGE: u32 = 65001;
const LONGEST_NAME: usize = 40;

/// A locale the C runtime's `setlocale` and `_create_locale` take (D229): "C", or English
/// (United States) with UTF-8, EggOS's ANSI code page, under the name it was asked by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLocale {
    C,
    EnglishUnitedStates,
    UnitedStates,
    UnitedStatesUtf8,
}

impl RuntimeLocale {
    /// The locale `name` names, matched without regard to case: `C`; the user's default (``,
    /// `.ACP`, `.OCP`, `.UTF-8`, `.UTF8`, `.65001`, `English`, `English_United States` with or
    /// without a UTF-8 code page); `en-US` or `en_US`; and those with `.UTF-8`. `None` for every
    /// other locale and code page, whose data EggOS has not.
    pub fn named(name: impl Iterator<Item = char>) -> Option<RuntimeLocale> {
        let mut folded = [0u8; LONGEST_NAME];
        let mut length = 0;
        for c in name {
            if !c.is_ascii() || length == LONGEST_NAME {
                return None;
            }
            folded[length] = c.to_ascii_lowercase() as u8;
            length += 1;
        }
        let folded = core::str::from_utf8(&folded[..length]).ok()?;
        let (language, code_page) = match folded.split_once('.') {
            Some((language, code_page)) => (language, Some(code_page)),
            None => (folded, None),
        };
        let utf8 = match code_page {
            None => false,
            Some("utf-8" | "utf8" | "65001") => true,
            Some("acp" | "ocp") if language.is_empty() => true,
            Some(_) => return None,
        };
        match (language, utf8) {
            ("c", false) => Some(RuntimeLocale::C),
            ("" | "english" | "english_united states", _) => {
                Some(RuntimeLocale::EnglishUnitedStates)
            }
            ("en-us" | "en_us", false) => Some(RuntimeLocale::UnitedStates),
            ("en-us" | "en_us", true) => Some(RuntimeLocale::UnitedStatesUtf8),
            _ => None,
        }
    }

    /// The name `setlocale` returns for the locale.
    pub fn name(self) -> &'static str {
        match self {
            RuntimeLocale::C => "C",
            RuntimeLocale::EnglishUnitedStates => "English_United States.utf8",
            RuntimeLocale::UnitedStates => "en-US",
            RuntimeLocale::UnitedStatesUtf8 => "en-US.utf8",
        }
    }

    /// The code page of the locale's multibyte characters: 0 for "C".
    pub fn code_page(self) -> u32 {
        match self {
            RuntimeLocale::C => 0,
            _ => UTF8_CODE_PAGE,
        }
    }

    /// `MB_CUR_MAX`: the most bytes one multibyte character takes.
    pub fn longest_character(self) -> i32 {
        match self {
            RuntimeLocale::C => 1,
            _ => 4,
        }
    }
}

/// The locale of each category, as the C runtime keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLocales([RuntimeLocale; 5]);

impl RuntimeLocales {
    /// Every category in "C", where a program starts.
    pub const C: RuntimeLocales = RuntimeLocales([RuntimeLocale::C; 5]);

    fn slot(category: i32) -> Option<usize> {
        match category {
            category::COLLATE..=category::TIME => Some(category as usize - 1),
            _ => None,
        }
    }

    /// The locale of `category` (one category, not `LC_ALL`).
    pub fn of(&self, category: i32) -> Option<RuntimeLocale> {
        Self::slot(category).map(|slot| self.0[slot])
    }

    /// Sets `category` (one, or `LC_ALL` for all) to the locale `name` names; `LC_ALL` also
    /// takes the `LC_COLLATE=...;LC_CTYPE=...` form `describe` gives. Nothing changes, and the
    /// answer is `false`, for an unknown category or name.
    pub fn set(&mut self, category: i32, name: &str) -> bool {
        if category == category::ALL {
            if name.contains('=') {
                return self.set_each(name);
            }
            return match RuntimeLocale::named(name.chars()) {
                Some(locale) => {
                    self.0 = [locale; 5];
                    true
                }
                None => false,
            };
        }
        match (Self::slot(category), RuntimeLocale::named(name.chars())) {
            (Some(slot), Some(locale)) => {
                self.0[slot] = locale;
                true
            }
            _ => false,
        }
    }

    fn set_each(&mut self, names: &str) -> bool {
        let mut changed = *self;
        for part in names.split(';').filter(|part| !part.is_empty()) {
            let Some((category, name)) = part.split_once('=') else {
                return false;
            };
            let Some(slot) = CATEGORY_NAMES.iter().position(|known| *known == category) else {
                return false;
            };
            let Some(locale) = RuntimeLocale::named(name.chars()) else {
                return false;
            };
            changed.0[slot] = locale;
        }
        *self = changed;
        true
    }

    /// Writes the name `setlocale(category, NULL)` returns through `out`: one locale's name, or
    /// for `LC_ALL` with categories in different locales each category's
    /// (`LC_COLLATE=C;LC_CTYPE=en-US;...`). `false` for an unknown category.
    pub fn describe(&self, category: i32, out: &mut impl FnMut(char)) -> bool {
        if category != category::ALL {
            return match self.of(category) {
                Some(locale) => {
                    locale.name().chars().for_each(&mut *out);
                    true
                }
                None => false,
            };
        }
        if self.0.iter().all(|locale| *locale == self.0[0]) {
            self.0[0].name().chars().for_each(out);
            return true;
        }
        for (index, (category, locale)) in CATEGORY_NAMES.iter().zip(self.0).enumerate() {
            if index > 0 {
                out(';');
            }
            category.chars().for_each(&mut *out);
            out('=');
            locale.name().chars().for_each(&mut *out);
        }
        true
    }

    /// The code page of the multibyte characters (`LC_CTYPE`'s).
    pub fn code_page(&self) -> u32 {
        self.0[category::CTYPE as usize - 1].code_page()
    }

    /// `MB_CUR_MAX` (`LC_CTYPE`'s).
    pub fn longest_character(&self) -> i32 {
        self.0[category::CTYPE as usize - 1].longest_character()
    }

    /// Whether `localeconv`'s numeric and monetary halves are "C"'s (empty, `CHAR_MAX`) rather
    /// than English (United States)'s.
    pub fn conventions_are_c(&self) -> (bool, bool) {
        let is_c = |category: i32| self.0[category as usize - 1] == RuntimeLocale::C;
        (is_c(category::NUMERIC), is_c(category::MONETARY))
    }
}

/// `localeconv`'s fields for English (United States): the numeric ones, then the monetary.
pub mod united_states_conventions {
    pub const DECIMAL_POINT: &str = ".";
    pub const THOUSANDS_SEPARATOR: &str = ",";
    pub const GROUPING: &str = "\u{3}";
    pub const INTERNATIONAL_CURRENCY_SYMBOL: &str = "USD ";
    pub const CURRENCY_SYMBOL: &str = "$";
    pub const MONETARY_DECIMAL_POINT: &str = ".";
    pub const MONETARY_THOUSANDS_SEPARATOR: &str = ",";
    pub const MONETARY_GROUPING: &str = "\u{3}";
    pub const POSITIVE_SIGN: &str = "";
    pub const NEGATIVE_SIGN: &str = "-";
    /// `int_frac_digits`, `frac_digits`, `p_cs_precedes`, `p_sep_by_space`, `n_cs_precedes`,
    /// `n_sep_by_space`, `p_sign_posn`, `n_sign_posn`.
    pub const MONETARY_NUMBERS: [u8; 8] = [2, 2, 1, 0, 1, 0, 3, 0];
}

/// The classification bits of the C runtime's character type table (`_UPPER` to `_ALPHA`).
pub mod ctype_flag {
    pub const UPPER: u16 = 0x1;
    pub const LOWER: u16 = 0x2;
    pub const DIGIT: u16 = 0x4;
    pub const SPACE: u16 = 0x8;
    pub const PUNCT: u16 = 0x10;
    pub const CONTROL: u16 = 0x20;
    pub const BLANK: u16 = 0x40;
    pub const HEX: u16 = 0x80;
    pub const ALPHA: u16 = 0x100;
}

/// The "C" locale's character type table (`_pctype`): entry 0 is `EOF`'s, then one for each
/// byte; only ASCII is classified.
pub const fn classic_character_types() -> [u16; 257] {
    let mut table = [0u16; 257];
    let mut byte = 0;
    while byte < 128 {
        let c = byte as u8;
        let mut flags = 0;
        if c.is_ascii_uppercase() {
            flags |= ctype_flag::UPPER | ctype_flag::ALPHA;
        }
        if c.is_ascii_lowercase() {
            flags |= ctype_flag::LOWER | ctype_flag::ALPHA;
        }
        if c.is_ascii_digit() {
            flags |= ctype_flag::DIGIT;
        }
        if c.is_ascii_hexdigit() {
            flags |= ctype_flag::HEX;
        }
        if matches!(c, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ') {
            flags |= ctype_flag::SPACE;
        }
        if matches!(c, b'\t' | b' ') {
            flags |= ctype_flag::BLANK;
        }
        if c.is_ascii_control() {
            flags |= ctype_flag::CONTROL;
        }
        if c.is_ascii_punctuation() {
            flags |= ctype_flag::PUNCT;
        }
        table[byte + 1] = flags;
        byte += 1;
    }
    table
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

    fn described(locales: &RuntimeLocales, category: i32) -> Option<String> {
        let mut text = String::new();
        let known = locales.describe(category, &mut |c| text.push(c));
        known.then_some(text)
    }

    #[test]
    fn runtime_locales_are_named_as_the_research_answer_lists() {
        let named = |name: &str| RuntimeLocale::named(name.chars());
        assert_eq!(named("C"), Some(RuntimeLocale::C));
        assert_eq!(named("c"), Some(RuntimeLocale::C));
        for user_default in [
            "",
            ".ACP",
            ".OCP",
            ".UTF-8",
            ".UTF8",
            ".utf8",
            ".65001",
            "English",
            "english_united states",
            "English_United States.utf8",
        ] {
            assert_eq!(
                named(user_default),
                Some(RuntimeLocale::EnglishUnitedStates),
                "{user_default}"
            );
        }
        assert_eq!(named("en-US"), Some(RuntimeLocale::UnitedStates));
        assert_eq!(named("en_US"), Some(RuntimeLocale::UnitedStates));
        assert_eq!(named("en-US.UTF-8"), Some(RuntimeLocale::UnitedStatesUtf8));
        assert_eq!(named("en-us.utf8"), Some(RuntimeLocale::UnitedStatesUtf8));
        for refused in [
            ".1252",
            "English_United States.1252",
            "de-DE",
            "fr-FR",
            "C.UTF-8",
            "POSIX",
        ] {
            assert_eq!(named(refused), None, "{refused}");
        }
        assert_eq!(
            RuntimeLocale::EnglishUnitedStates.name(),
            "English_United States.utf8"
        );
        assert_eq!(RuntimeLocale::UnitedStatesUtf8.name(), "en-US.utf8");
        for locale in [
            RuntimeLocale::C,
            RuntimeLocale::EnglishUnitedStates,
            RuntimeLocale::UnitedStates,
            RuntimeLocale::UnitedStatesUtf8,
        ] {
            assert_eq!(named(locale.name()), Some(locale));
        }
    }

    #[test]
    fn categories_change_alone_and_lc_all_describes_them_together() {
        let mut locales = RuntimeLocales::C;
        assert_eq!(described(&locales, category::ALL).as_deref(), Some("C"));
        assert_eq!((locales.code_page(), locales.longest_character()), (0, 1));
        assert!(locales.set(category::ALL, ""));
        assert_eq!(
            described(&locales, category::ALL).as_deref(),
            Some("English_United States.utf8")
        );
        assert_eq!(
            (locales.code_page(), locales.longest_character()),
            (65001, 4)
        );
        assert!(locales.set(category::NUMERIC, "C"));
        assert_eq!(
            described(&locales, category::ALL).as_deref(),
            Some(
                "LC_COLLATE=English_United States.utf8;LC_CTYPE=English_United States.utf8;\
                 LC_MONETARY=English_United States.utf8;LC_NUMERIC=C;\
                 LC_TIME=English_United States.utf8"
            )
        );
        assert_eq!(locales.conventions_are_c(), (true, false));
        assert_eq!(described(&locales, category::NUMERIC).as_deref(), Some("C"));
        let saved = described(&locales, category::ALL).unwrap();
        let mut restored = RuntimeLocales::C;
        assert!(restored.set(category::ALL, &saved));
        assert_eq!(restored, locales);
        assert!(!locales.set(category::ALL, "de-DE"));
        assert!(!locales.set(category::CTYPE, ".1252"));
        assert!(!locales.set(6, "C"));
        assert!(!locales.set(category::ALL, "LC_CTYPE=C;LC_BOGUS=C"));
        assert_eq!(restored, locales);
        assert!(locales.set(category::CTYPE, "C"));
        assert_eq!((locales.code_page(), locales.longest_character()), (0, 1));
        assert_eq!(described(&locales, 7), None);
    }

    #[test]
    fn the_classic_character_types_classify_ascii_only() {
        let table = classic_character_types();
        let of = |c: u8| table[usize::from(c) + 1];
        assert_eq!(table[0], 0);
        assert_eq!(
            of(b'A'),
            ctype_flag::UPPER | ctype_flag::ALPHA | ctype_flag::HEX
        );
        assert_eq!(of(b'z'), ctype_flag::LOWER | ctype_flag::ALPHA);
        assert_eq!(of(b'7'), ctype_flag::DIGIT | ctype_flag::HEX);
        assert_eq!(of(b' '), ctype_flag::SPACE | ctype_flag::BLANK);
        assert_eq!(
            of(b'\t'),
            ctype_flag::SPACE | ctype_flag::BLANK | ctype_flag::CONTROL
        );
        assert_eq!(of(b'\n'), ctype_flag::SPACE | ctype_flag::CONTROL);
        assert_eq!(of(b'!'), ctype_flag::PUNCT);
        assert_eq!(of(0x7f), ctype_flag::CONTROL);
        assert_eq!(of(0xe9), 0);
    }
}
