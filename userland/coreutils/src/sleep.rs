use crate::output::{FAILURE, Output};
use crate::system::{Duration, System};
use core::ffi::CStr;
use eggos_commands::errors;

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;
const FRACTION_DIGITS: usize = 9;

pub fn sleep<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut total: u128 = 0;
    let mut operands = 0;
    let mut status = 0;
    let mut options_end = false;
    for argument in arguments {
        let bytes = argument.to_bytes();
        if !options_end && bytes == b"--" {
            options_end = true;
            continue;
        }
        if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
            return output.invalid_option(bytes);
        }
        operands += 1;
        match interval(bytes) {
            Some(nanoseconds) => total = total.saturating_add(nanoseconds),
            None => {
                output.coded(&errors::USAGE, &[b"invalid time interval '", bytes, b"'"]);
                status = FAILURE;
            }
        }
    }
    if operands == 0 {
        output.coded(&errors::USAGE, &[b"missing operand"]);
        return FAILURE;
    }
    if status != 0 {
        return status;
    }
    output.flush();
    let seconds = total / NANOSECONDS_PER_SECOND;
    let duration = Duration {
        seconds: u64::try_from(seconds).unwrap_or(u64::MAX),
        nanoseconds: (total % NANOSECONDS_PER_SECOND) as u32,
    };
    match output.system.sleep(duration) {
        Ok(()) => 0,
        Err(errno) => {
            output.error(&[errno.message()]);
            FAILURE
        }
    }
}

fn interval(text: &[u8]) -> Option<u128> {
    let (number, unit) = match text.split_last()? {
        (b's', number) => (number, 1),
        (b'm', number) => (number, 60),
        (b'h', number) => (number, 3600),
        (b'd', number) => (number, 86_400),
        _ => (text, 1),
    };
    let (whole, fraction) = match number.iter().position(|&byte| byte == b'.') {
        Some(point) => (&number[..point], &number[point + 1..]),
        None => (number, &b""[..]),
    };
    let all_digits = |part: &[u8]| part.iter().all(u8::is_ascii_digit);
    if whole.is_empty() && fraction.is_empty() || !all_digits(whole) || !all_digits(fraction) {
        return None;
    }
    let seconds = whole.iter().fold(0u128, |value, &digit| {
        value
            .saturating_mul(10)
            .saturating_add(u128::from(digit - b'0'))
    });
    let mut nanoseconds = 0u128;
    for index in 0..FRACTION_DIGITS {
        let digit = fraction.get(index).map_or(0, |&digit| digit - b'0');
        nanoseconds = nanoseconds * 10 + u128::from(digit);
    }
    if fraction
        .iter()
        .skip(FRACTION_DIGITS)
        .any(|&digit| digit != b'0')
    {
        nanoseconds += 1;
    }
    Some(
        seconds
            .saturating_mul(NANOSECONDS_PER_SECOND)
            .saturating_add(nanoseconds)
            .saturating_mul(unit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(arguments: &[&CStr]) -> (i32, Fake) {
        let mut output = Output::new(Fake::default(), b"sleep");
        let status = sleep(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    fn slept(arguments: &[&CStr]) -> Vec<(u64, u32)> {
        let (status, fake) = run(arguments);
        assert_eq!(status, 0, "{}", fake.errors());
        fake.slept
            .iter()
            .map(|duration| (duration.seconds, duration.nanoseconds))
            .collect()
    }

    #[test]
    fn intervals_are_seconds_with_fractions_and_units_summed() {
        assert_eq!(slept(&[c"0"]), [(0, 0)]);
        assert_eq!(slept(&[c"1.5"]), [(1, 500_000_000)]);
        assert_eq!(slept(&[c".25", c"2."]), [(2, 250_000_000)]);
        assert_eq!(slept(&[c"1m", c"1h", c"1d", c"1s"]), [(90_061, 0)]);
        assert_eq!(slept(&[c"0.5m"]), [(30, 0)]);
        assert_eq!(slept(&[c"0.0000000001"]), [(0, 1)]);
        assert_eq!(slept(&[c"0.0000000010"]), [(0, 1)]);
        assert_eq!(slept(&[c"--", c"3"]), [(3, 0)]);
        assert_eq!(
            slept(&[c"99999999999999999999999999999999999999999d"]),
            [(u64::MAX, (u128::MAX % NANOSECONDS_PER_SECOND) as u32)]
        );
    }

    #[test]
    fn bad_intervals_are_all_reported_and_nothing_sleeps() {
        let (status, fake) = run(&[c"x", c"1", c".", c"1.2.3", c"5w", c""]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "sleep: invalid time interval 'x'\nsleep: invalid time interval '.'\n\
             sleep: invalid time interval '1.2.3'\nsleep: invalid time interval '5w'\n\
             sleep: invalid time interval ''\n"
        );
        assert!(fake.slept.is_empty());
    }

    #[test]
    fn no_operand_or_an_option_is_a_usage_error() {
        let (status, fake) = run(&[]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "sleep: missing operand\n")
        );
        let (status, fake) = run(&[c"--"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "sleep: missing operand\n")
        );
        let (status, fake) = run(&[c"-1"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "sleep: invalid option -- '1'\n")
        );
    }
}
