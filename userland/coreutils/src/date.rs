use crate::output::{FAILURE, Output};
use crate::system::{SymbolicLinks, System};
use core::ffi::CStr;
use eggos_calendar::{DateTime, MONTHS, ParseError, WEEKDAYS, parse_setting};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown<'a> {
    Now,
    File(&'a CStr),
}

pub fn date<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut set: Option<&CStr> = None;
    let mut shown = Shown::Now;
    let mut format: Option<&[u8]> = None;
    let mut options_end = false;
    let mut index = 0;
    while let Some(&argument) = arguments.get(index) {
        index += 1;
        let bytes = argument.to_bytes();
        if !options_end && bytes == b"--" {
            options_end = true;
            continue;
        }
        if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
            match bytes {
                b"-u" | b"--utc" | b"--universal" => {}
                b"-s" | b"--set" | b"-r" | b"--reference" => {
                    let Some(&value) = arguments.get(index) else {
                        output.error(&[b"option requires an argument -- '", &bytes[1..], b"'"]);
                        return FAILURE;
                    };
                    index += 1;
                    match bytes {
                        b"-s" | b"--set" => set = Some(value),
                        _ => shown = Shown::File(value),
                    }
                }
                _ => match (
                    bytes.strip_prefix(b"--set="),
                    bytes.strip_prefix(b"--reference="),
                ) {
                    (Some(value), _) => set = Some(cstr_tail(argument, bytes.len() - value.len())),
                    (_, Some(value)) => {
                        shown = Shown::File(cstr_tail(argument, bytes.len() - value.len()));
                    }
                    _ => return output.invalid_option(bytes),
                },
            }
            continue;
        }
        match (bytes.strip_prefix(b"+"), format) {
            (Some(wanted), None) => format = Some(wanted),
            (Some(_), Some(_)) => {
                output.error(&[b"extra operand '", bytes, b"'"]);
                return FAILURE;
            }
            (None, _) => {
                output.error(&[
                    b"invalid date '",
                    bytes,
                    b"': set the clock with -s 'YYYY-MM-DD HH:MM[:SS]' (UTC)",
                ]);
                return FAILURE;
            }
        }
    }
    let seconds = match (set, shown) {
        (Some(_), Shown::File(_)) => {
            output.error(&[b"the options to set the time and to show a file's time are exclusive"]);
            return FAILURE;
        }
        (Some(text), Shown::Now) => match set_clock(output, text) {
            Ok(seconds) => seconds,
            Err(status) => return status,
        },
        (None, Shown::File(path)) => {
            match output.system.status(None, path, SymbolicLinks::Follow) {
                Ok(status) => status.modified,
                Err(errno) => {
                    output.file_error(path, errno);
                    return FAILURE;
                }
            }
        }
        (None, Shown::Now) => output.system.now() as i64,
    };
    write_formatted(output, format.unwrap_or(DEFAULT_FORMAT), seconds);
    output.write(b"\n");
    0
}

const DEFAULT_FORMAT: &[u8] = b"%a %b %e %H:%M:%S %Z %Y";

fn cstr_tail(argument: &CStr, skip: usize) -> &CStr {
    CStr::from_bytes_with_nul(&argument.to_bytes_with_nul()[skip..]).unwrap_or(c"")
}

fn set_clock<S: System>(output: &mut Output<S>, text: &CStr) -> Result<i64, i32> {
    let typed = core::str::from_utf8(text.to_bytes()).unwrap_or("");
    let now = output.system.now() as i64;
    let seconds = match parse_setting(typed, now) {
        Ok(seconds) => seconds,
        Err(ParseError::NotADateAndTime) => {
            output.error(&[
                b"invalid date '",
                text.to_bytes(),
                b"': type YYYY-MM-DD HH:MM[:SS] or HH:MM[:SS], in UTC",
            ]);
            return Err(FAILURE);
        }
        Err(ParseError::NeedsTheDate) => {
            output.error(&[b"the clock is not set: give the date too, YYYY-MM-DD HH:MM"]);
            return Err(FAILURE);
        }
        Err(ParseError::YearOutOfRange) => {
            output.error(&[
                b"invalid date '",
                text.to_bytes(),
                b"': the year must be 2000 to 2099",
            ]);
            return Err(FAILURE);
        }
    };
    match output.system.set_time(seconds as u64) {
        Ok(()) => Ok(seconds),
        Err(errno) => {
            output.error(&[b"cannot set date: ", errno.message()]);
            Err(FAILURE)
        }
    }
}

fn write_padded<S: System>(output: &mut Output<S>, value: i64, width: usize, pad: u8) {
    if value < 0 {
        output.write(b"-");
    }
    let mut digits = [0u8; 20];
    let text = crate::output::decimal(value.unsigned_abs(), &mut digits);
    for _ in text.len()..width {
        output.write(&[pad]);
    }
    output.write(text);
}

fn write_formatted<S: System>(output: &mut Output<S>, format: &[u8], seconds: i64) {
    let moment = DateTime::from_unix(seconds);
    let weekday = WEEKDAYS[moment.weekday()].as_bytes();
    let month = MONTHS[moment.month as usize - 1].as_bytes();
    let hour_12 = match moment.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let mut rest = format;
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte != b'%' {
            output.write(&[byte]);
            continue;
        }
        let Some((&conversion, after)) = rest.split_first() else {
            output.write(b"%");
            break;
        };
        rest = after;
        match conversion {
            b'a' => output.write(&weekday[..3]),
            b'A' => output.write(weekday),
            b'b' | b'h' => output.write(&month[..3]),
            b'B' => output.write(month),
            b'c' => write_formatted(output, b"%a %b %e %H:%M:%S %Y", seconds),
            b'C' => write_padded(output, moment.year.div_euclid(100), 2, b'0'),
            b'd' => write_padded(output, i64::from(moment.day), 2, b'0'),
            b'D' => write_formatted(output, b"%m/%d/%y", seconds),
            b'e' => write_padded(output, i64::from(moment.day), 2, b' '),
            b'F' => write_formatted(output, b"%Y-%m-%d", seconds),
            b'H' => write_padded(output, i64::from(moment.hour), 2, b'0'),
            b'I' => write_padded(output, i64::from(hour_12), 2, b'0'),
            b'j' => write_padded(output, i64::from(moment.day_of_year()), 3, b'0'),
            b'k' => write_padded(output, i64::from(moment.hour), 2, b' '),
            b'l' => write_padded(output, i64::from(hour_12), 2, b' '),
            b'm' => write_padded(output, i64::from(moment.month), 2, b'0'),
            b'M' => write_padded(output, i64::from(moment.minute), 2, b'0'),
            b'n' => output.write(b"\n"),
            b'p' => output.write(if moment.hour < 12 { b"AM" } else { b"PM" }),
            b'r' => write_formatted(output, b"%I:%M:%S %p", seconds),
            b'R' => write_formatted(output, b"%H:%M", seconds),
            b's' => write_padded(output, seconds, 1, b'0'),
            b'S' => write_padded(output, i64::from(moment.second), 2, b'0'),
            b't' => output.write(b"\t"),
            b'T' => write_formatted(output, b"%H:%M:%S", seconds),
            b'u' => write_padded(output, (moment.weekday() as i64 + 6) % 7 + 1, 1, b'0'),
            b'w' => write_padded(output, moment.weekday() as i64, 1, b'0'),
            b'y' => write_padded(output, moment.year.rem_euclid(100), 2, b'0'),
            b'Y' => write_padded(output, moment.year, 4, b'0'),
            b'z' => output.write(b"+0000"),
            b'Z' => output.write(b"UTC"),
            b'%' => output.write(b"%"),
            other => output.write(&[b'%', other]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use eggos_runtime::errno::Errno;

    const NOW: i64 = 1_793_282_709;

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
        let mut output = Output::new(fake, b"date");
        let status = date(&mut output, arguments);
        let status = output.finish(status);
        let fake = output.system;
        let (out, errors) = (fake.output(), fake.errors());
        (status, out, errors, fake)
    }

    fn at_now() -> Fake {
        let mut fake = Fake::default();
        fake.now = NOW;
        fake
    }

    #[test]
    fn with_no_format_it_shows_the_time_as_date_does_in_the_c_locale() {
        let (status, out, _, _) = run(at_now(), &[]);
        assert_eq!(
            (status, out.as_str()),
            (0, "Thu Oct 29 14:05:09 UTC 2026\n")
        );
        let (_, out, _, _) = run(at_now(), &[c"-u"]);
        assert_eq!(out, "Thu Oct 29 14:05:09 UTC 2026\n");
    }

    #[test]
    fn a_format_takes_strftime_s_conversions() {
        let (status, out, _, _) = run(
            at_now(),
            &[c"+%s %F %T %Y%m%d %H%M%S %a %A %b %B %e %j %y %C %I %p %u %w %Z %z %% %q %D %R"],
        );
        assert_eq!(status, 0);
        assert_eq!(
            out,
            "1793282709 2026-10-29 14:05:09 20261029 140509 Thu Thursday Oct October 29 302 26 \
             20 02 PM 4 4 UTC +0000 % %q 10/29/26 14:05\n"
        );
        let (_, out, _, _) = run(at_now(), &[c"+EGGOS-DATE %s"]);
        assert_eq!(out, "EGGOS-DATE 1793282709\n");
        let (_, out, _, _) = run(Fake::default(), &[c"+%e|%k|%l|%I|%p"]);
        assert_eq!(out, "14|22|10|10|PM\n");
    }

    #[test]
    fn reference_shows_a_file_s_modification_time() {
        let file = || at_now().with_file("/tmp/x", 0o644, b"", 951_782_400);
        let (status, out, _, _) = run(file(), &[c"-r", c"/tmp/x", c"+%F %T"]);
        assert_eq!((status, out.as_str()), (0, "2000-02-29 00:00:00\n"));
        let (_, out, _, _) = run(file(), &[c"--reference=/tmp/x", c"+%s"]);
        assert_eq!(out, "951782400\n");
        let (status, _, errors, _) = run(at_now(), &[c"-r", c"/nope"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "date: /nope: No such file or directory\n");
    }

    #[test]
    fn set_parses_the_moment_sets_the_clock_and_shows_it() {
        let (status, out, _, fake) = run(at_now(), &[c"-s", c"2030-01-02 03:04:05"]);
        assert_eq!(
            (status, out.as_str()),
            (0, "Wed Jan  2 03:04:05 UTC 2030\n")
        );
        assert_eq!(fake.set_times, [1_893_553_445]);
        let (_, _, _, fake) = run(at_now(), &[c"--set=15:00", c"+%s"]);
        assert_eq!(fake.set_times, [1_793_286_000]);
        let (_, _, _, fake) = run(at_now(), &[c"-u", c"--set", c"2026-10-29T14:05:09Z"]);
        assert_eq!(fake.set_times, [NOW as u64]);
    }

    #[test]
    fn set_refuses_what_it_cannot_set_with_the_reason() {
        for (arguments, error) in [
            (
                &[c"-s", c"tomorrow"][..],
                "date: invalid date 'tomorrow': type YYYY-MM-DD HH:MM[:SS] or HH:MM[:SS], in UTC\n",
            ),
            (
                &[c"-s", c"1999-12-31 23:59"][..],
                "date: invalid date '1999-12-31 23:59': the year must be 2000 to 2099\n",
            ),
            (&[c"-s"][..], "date: option requires an argument -- 's'\n"),
            (
                &[c"-s", c"2030-01-02 03:04", c"-r", c"/tmp/x"][..],
                "date: the options to set the time and to show a file's time are exclusive\n",
            ),
            (
                &[c"0102030430"][..],
                "date: invalid date '0102030430': set the clock with -s 'YYYY-MM-DD HH:MM[:SS]' \
                 (UTC)\n",
            ),
            (&[c"+%s", c"+%F"][..], "date: extra operand '+%F'\n"),
            (&[c"-x"][..], "date: invalid option -- 'x'\n"),
        ] {
            let (status, out, errors, fake) = run(at_now(), arguments);
            assert_eq!((status, out.as_str()), (1, ""), "{arguments:?}");
            assert_eq!(errors, error);
            assert!(fake.set_times.is_empty());
        }
        let mut unset = Fake::default();
        unset.now = 12;
        let (_, _, errors, _) = run(unset, &[c"-s", c"14:05"]);
        assert_eq!(
            errors,
            "date: the clock is not set: give the date too, YYYY-MM-DD HH:MM\n"
        );
        let mut refused = at_now();
        refused.set_time_error = Some(Errno::EPERM);
        let (status, _, errors, _) = run(refused, &[c"-s", c"2030-01-02 03:04"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "date: cannot set date: Operation not permitted\n");
    }
}
