//! Automation's `DATE`: a double counting days from 1899-12-30, its fraction the time of day
//! (for a date before then, the integer part counts days back and the fraction still goes
//! forward: -1.25 is 1899-12-29 06:00). Its text is the English (United States) short date and
//! long time, `M/D/YYYY h:mm:ss AM`, the date alone at midnight and the time alone on day 0, as
//! `VarBstrFromDate` writes it; reading takes that form, `YYYY-MM-DD` and `YYYY/M/D` dates, and
//! `h:mm[:ss]` times with or without `AM`/`PM`. Years run from 100 to 9999.

use super::AsciiText;

const SECONDS_PER_DAY: i64 = 86_400;
const DATE_DAYS_AT_1970: i64 = 25_569;
const SMALLEST_YEAR: i64 = 100;
const LARGEST_YEAR: i64 = 9999;
const TWO_DIGIT_YEAR_PIVOT: i64 = 30;
const DAY_ZERO: (i64, u32, u32) = (1899, 12, 30);

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = i64::from((month + 9) % 12);
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = ((shifted_month + 2) % 12 + 1) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A calendar date and time of day, to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub second_of_day: u32,
}

/// The moment `date` names, rounded to the second; `None` outside years 100 to 9999.
pub fn moment(date: f64) -> Option<Moment> {
    if !date.is_finite() || date.abs() > 3.0e6 {
        return None;
    }
    let mut days = date as i64;
    let fraction = (date - days as f64).abs();
    let mut seconds = (fraction * SECONDS_PER_DAY as f64 + 0.5) as i64;
    if seconds >= SECONDS_PER_DAY {
        seconds -= SECONDS_PER_DAY;
        days += 1;
    }
    let (year, month, day) = civil_from_days(days - DATE_DAYS_AT_1970);
    if !(SMALLEST_YEAR..=LARGEST_YEAR).contains(&year) {
        return None;
    }
    Some(Moment {
        year,
        month,
        day,
        second_of_day: seconds as u32,
    })
}

/// The `DATE` of `moment`; `None` for a date that does not exist or outside years 100 to 9999.
pub fn date(moment: Moment) -> Option<f64> {
    let valid = (SMALLEST_YEAR..=LARGEST_YEAR).contains(&moment.year)
        && (1..=12).contains(&moment.month)
        && (1..=days_in_month(moment.year, moment.month)).contains(&moment.day)
        && i64::from(moment.second_of_day) < SECONDS_PER_DAY;
    if !valid {
        return None;
    }
    let days = days_from_civil(moment.year, moment.month, moment.day) + DATE_DAYS_AT_1970;
    let time = f64::from(moment.second_of_day) / SECONDS_PER_DAY as f64;
    Some(if days < 0 {
        days as f64 - time
    } else {
        days as f64 + time
    })
}

/// `date`'s English (United States) text; `None` outside years 100 to 9999.
pub fn text(date: f64) -> Option<AsciiText> {
    let moment = moment(date)?;
    let mut text = AsciiText::new();
    if (moment.year, moment.month, moment.day) != DAY_ZERO {
        text.number(u128::from(moment.month), 1);
        text.push(b"/");
        text.number(u128::from(moment.day), 1);
        text.push(b"/");
        text.number(moment.year as u128, 4);
        if moment.second_of_day == 0 {
            return Some(text);
        }
        text.push(b" ");
    }
    write_time(&mut text, moment.second_of_day);
    Some(text)
}

fn write_time(text: &mut AsciiText, second_of_day: u32) {
    let hour = second_of_day / 3600;
    let twelve_hour = match hour % 12 {
        0 => 12,
        other => other,
    };
    text.number(u128::from(twelve_hour), 1);
    text.push(b":");
    text.number(u128::from(second_of_day / 60 % 60), 2);
    text.push(b":");
    text.number(u128::from(second_of_day % 60), 2);
    text.push(if hour < 12 { b" AM" } else { b" PM" });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Number(u32, usize),
    Separator(u8),
    Word(bool),
}

fn tokens(text: &[u16], out: &mut [Token; 12]) -> Option<usize> {
    let mut count = 0;
    let mut at = 0;
    while at < text.len() {
        let unit = text[at];
        let token = if (u16::from(b'0')..=u16::from(b'9')).contains(&unit) {
            let start = at;
            let mut value = 0u32;
            while let Some(&digit) = text.get(at).filter(|&&unit| (48..=57).contains(&unit)) {
                value = value.checked_mul(10)?.checked_add(u32::from(digit - 48))?;
                at += 1;
            }
            Token::Number(value, at - start)
        } else if unit == u16::from(b' ') || unit == u16::from(b'\t') {
            at += 1;
            continue;
        } else if [b'/', b'-', b':', b'T']
            .iter()
            .any(|&b| unit == u16::from(b))
        {
            at += 1;
            Token::Separator(unit as u8)
        } else {
            let word = &text[at..];
            let marker = |letter: u8| {
                word.len() >= 2
                    && (word[0] | 0x20) == u16::from(letter)
                    && (word[1] | 0x20) == u16::from(b'm')
            };
            let afternoon = if marker(b'a') {
                false
            } else if marker(b'p') {
                true
            } else {
                return None;
            };
            at += 2;
            Token::Word(afternoon)
        };
        *out.get_mut(count)? = token;
        count += 1;
    }
    Some(count)
}

fn full_year(value: u32, digits: usize) -> i64 {
    let year = i64::from(value);
    match digits {
        1 | 2 if year < TWO_DIGIT_YEAR_PIVOT => 2000 + year,
        1 | 2 => 1900 + year,
        _ => year,
    }
}

fn read_time(tokens: &[Token]) -> Option<u32> {
    let (hour, minute, second, rest) = match tokens {
        [
            Token::Number(hour, _),
            Token::Separator(b':'),
            Token::Number(minute, _),
            Token::Separator(b':'),
            Token::Number(second, _),
            rest @ ..,
        ] => (*hour, *minute, *second, rest),
        [
            Token::Number(hour, _),
            Token::Separator(b':'),
            Token::Number(minute, _),
            rest @ ..,
        ] => (*hour, *minute, 0, rest),
        _ => return None,
    };
    let hour = match rest {
        [] if hour < 24 => hour,
        [Token::Word(afternoon)] if (1..=12).contains(&hour) => {
            hour % 12 + 12 * u32::from(*afternoon)
        }
        _ => return None,
    };
    (minute < 60 && second < 60).then_some(hour * 3600 + minute * 60 + second)
}

/// The `DATE` the English (United States) text `text` names; `None` when it names none.
pub fn parse(text: &[u16]) -> Option<f64> {
    let mut storage = [Token::Separator(0); 12];
    let count = tokens(text, &mut storage)?;
    let all = &storage[..count];
    let (year, month, day, rest) = match all {
        [
            Token::Number(first, first_digits),
            Token::Separator(separator),
            Token::Number(second, _),
            Token::Separator(again),
            Token::Number(third, third_digits),
            rest @ ..,
        ] if separator == again && (*separator == b'/' || *separator == b'-') => {
            if *first_digits >= 3 {
                (i64::from(*first), *second, *third, rest)
            } else {
                (full_year(*third, *third_digits), *first, *second, rest)
            }
        }
        _ => {
            let second_of_day = read_time(all)?;
            return Some(f64::from(second_of_day) / SECONDS_PER_DAY as f64);
        }
    };
    let rest = match rest {
        [Token::Separator(b'T'), rest @ ..] => rest,
        rest => rest,
    };
    let second_of_day = match rest {
        [] => 0,
        rest => read_time(rest)?,
    };
    date(Moment {
        year,
        month,
        day,
        second_of_day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn written(date: f64) -> String {
        String::from_utf8(text(date).unwrap().as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn dates_count_days_from_1899_12_30() {
        assert_eq!(
            moment(0.0).map(|m| (m.year, m.month, m.day)),
            Some((1899, 12, 30))
        );
        assert_eq!(
            moment(2.0).map(|m| (m.year, m.month, m.day)),
            Some((1900, 1, 1))
        );
        let new_year = moment(36_526.5).unwrap();
        assert_eq!((new_year.year, new_year.month, new_year.day), (2000, 1, 1));
        assert_eq!(new_year.second_of_day, 43_200);
        let before = moment(-1.25).unwrap();
        assert_eq!((before.year, before.month, before.day), (1899, 12, 29));
        assert_eq!(before.second_of_day, 6 * 3600);
        assert_eq!(moment(-700_000.0), None);
        assert_eq!(moment(f64::NAN), None);
    }

    #[test]
    fn dates_are_written_as_english_short_dates_and_long_times() {
        assert_eq!(written(36_526.0), "1/1/2000");
        assert_eq!(written(36_526.75), "1/1/2000 6:00:00 PM");
        assert_eq!(written(0.5), "12:00:00 PM");
        assert_eq!(written(0.0), "12:00:00 AM");
        assert_eq!(written(45_000.0 + 1.0 / 86_400.0), "3/15/2023 12:00:01 AM");
        assert_eq!(written(-1.25), "12/29/1899 6:00:00 AM");
    }

    #[test]
    fn date_text_is_read_back() {
        for date in [36_526.0, 36_526.75, 0.5, 45_000.25, -1.25, 2.0] {
            let text = written(date);
            assert_eq!(parse(&wide(&text)), Some(date), "{text}");
        }
        assert_eq!(parse(&wide("2000-01-01")), Some(36_526.0));
        assert_eq!(parse(&wide("2000/1/1 18:00")), Some(36_526.75));
        assert_eq!(parse(&wide("1/1/99")), Some(36_161.0));
        assert_eq!(parse(&wide("1/1/29")), parse(&wide("1/1/2029")));
        assert_eq!(parse(&wide("13:30")), Some(0.5625));
        for wrong in [
            "2/30/2000",
            "1/1/2000 25:00",
            "hello",
            "1/1",
            "13:00 PM",
            "1/1/10000",
        ] {
            assert_eq!(parse(&wide(wrong)), None, "{wrong}");
        }
    }
}
