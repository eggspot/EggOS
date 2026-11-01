//! `kill` (D200): POSIX's. `kill [-s <signal> | -<signal>] [--] <pid>...` sends a signal, `TERM`
//! by default, to each process; `kill -l` lists the signals' names and `kill -l <number>`
//! names one. A signal is a name with or without `SIG`, in any case, or a number.

use crate::output::Output;
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: kill [-s <signal> | -<signal>] [--] <pid>... | kill -l [<number>]";
const USAGE_STATUS: i32 = 2;

/// The signals `kill` names, with their numbers.
pub const SIGNALS: [(&str, u32); 13] = [
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("KILL", 9),
    ("USR1", 10),
    ("USR2", 12),
    ("PIPE", 13),
    ("ALRM", 14),
    ("TERM", 15),
    ("CHLD", 17),
    ("CONT", 18),
    ("STOP", 19),
    ("TSTP", 20),
];

/// The signal `text` names: a name, `SIG` optional, any case, or a number from 0 to 64.
pub fn signal_named(text: &[u8]) -> Option<u32> {
    if !text.is_empty() && text.len() <= 2 && text.iter().all(u8::is_ascii_digit) {
        let number = text
            .iter()
            .fold(0u32, |number, &digit| number * 10 + u32::from(digit - b'0'));
        return (number <= 64).then_some(number);
    }
    let name = match text.get(..3) {
        Some(prefix) if prefix.eq_ignore_ascii_case(b"SIG") => &text[3..],
        _ => text,
    };
    SIGNALS
        .iter()
        .find(|(known, _)| known.as_bytes().eq_ignore_ascii_case(name))
        .map(|&(_, number)| number)
}

fn pid_of(text: &[u8]) -> Option<i32> {
    let (negative, digits) = match text.strip_prefix(b"-") {
        Some(digits) => (true, digits),
        None => (false, text),
    };
    if digits.is_empty() || digits.len() > 9 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0i32, |value, &digit| value * 10 + i32::from(digit - b'0'));
    Some(if negative { -value } else { value })
}

fn list<S: System>(output: &mut Output<S>, number: Option<&CStr>) -> i32 {
    match number {
        None => {
            for (index, (name, _)) in SIGNALS.iter().enumerate() {
                if index > 0 {
                    output.write(b" ");
                }
                output.write(name.as_bytes());
            }
            output.write(b"\n");
            0
        }
        Some(number) => {
            let wanted = signal_named(number.to_bytes())
                .filter(|_| number.to_bytes().iter().all(u8::is_ascii_digit));
            match wanted.and_then(|wanted| SIGNALS.iter().find(|(_, known)| *known == wanted)) {
                Some((name, _)) => {
                    output.write(name.as_bytes());
                    output.write(b"\n");
                    0
                }
                None => {
                    output.coded(
                        &errors::INVALID_VALUE,
                        &[number.to_bytes(), b": invalid signal"],
                    );
                    1
                }
            }
        }
    }
}

pub fn kill<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut signal = 15;
    let mut rest = arguments;
    match rest.first().map(|first| first.to_bytes()) {
        Some(b"-l") => {
            return match &rest[1..] {
                [] => list(output, None),
                [number] => list(output, Some(number)),
                _ => {
                    output.coded(&errors::USAGE, &[USAGE]);
                    USAGE_STATUS
                }
            };
        }
        Some(b"-s") => {
            let Some(name) = rest.get(1) else {
                output.coded(&errors::USAGE, &[USAGE]);
                return USAGE_STATUS;
            };
            let Some(named) = signal_named(name.to_bytes()) else {
                output.coded(
                    &errors::INVALID_VALUE,
                    &[name.to_bytes(), b": invalid signal"],
                );
                return USAGE_STATUS;
            };
            signal = named;
            rest = &rest[2..];
        }
        Some(option) if option.len() > 1 && option.starts_with(b"-") && option != b"--" => {
            match signal_named(&option[1..]) {
                Some(named) => {
                    signal = named;
                    rest = &rest[1..];
                }
                None if pid_of(option).is_some() => {}
                None => {
                    output.coded(&errors::INVALID_VALUE, &[&option[1..], b": invalid signal"]);
                    return USAGE_STATUS;
                }
            }
        }
        _ => {}
    }
    if rest.first().is_some_and(|first| first.to_bytes() == b"--") {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        output.coded(&errors::USAGE, &[USAGE]);
        return USAGE_STATUS;
    }
    let mut status = 0;
    for argument in rest {
        let Some(pid) = pid_of(argument.to_bytes()) else {
            output.coded(
                &errors::INVALID_VALUE,
                &[argument.to_bytes(), b": not a process id"],
            );
            status = 1;
            continue;
        };
        if let Err(errno) = output.system.send_signal(pid, signal) {
            let error = match errno {
                Errno::ESRCH => &errors::NO_SUCH_PROCESS,
                Errno::EPERM => &errors::PERMISSION_DENIED,
                _ => &errors::FAILED,
            };
            output.coded(error, &[argument.to_bytes(), b": ", errno.message()]);
            status = 1;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: &mut Fake, arguments: &[&CStr]) -> (i32, String, String) {
        let mut output = Output::new(&mut *fake, b"kill");
        let status = kill(&mut output, arguments);
        let status = output.finish(status);
        (
            status,
            String::from_utf8_lossy(&fake.standard_output).into_owned(),
            String::from_utf8_lossy(&fake.standard_error).into_owned(),
        )
    }

    #[test]
    fn signals_are_named_or_numbered_and_term_is_the_default() {
        assert_eq!(signal_named(b"KILL"), Some(9));
        assert_eq!(signal_named(b"sigterm"), Some(15));
        assert_eq!(signal_named(b"9"), Some(9));
        assert_eq!(signal_named(b"NOPE"), None);
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &[c"42"]).0, 0);
        assert_eq!(run(&mut fake, &[c"-KILL", c"43"]).0, 0);
        assert_eq!(run(&mut fake, &[c"-s", c"STOP", c"--", c"42"]).0, 0);
        assert_eq!(run(&mut fake, &[c"-9", c"-1"]).0, 0);
        assert_eq!(fake.signals_sent, [(42, 15), (43, 9), (42, 19), (-1, 9)]);
    }

    #[test]
    fn failures_say_which_process_and_the_rest_still_go() {
        let mut fake = Fake::default();
        let (status, _, errors) = run(&mut fake, &[c"7", c"1", c"42"]);
        assert_eq!(status, 1);
        assert!(errors.contains("7: No such process"), "{errors}");
        assert!(errors.contains("1: Operation not permitted"), "{errors}");
        assert_eq!(fake.signals_sent, [(42, 15)]);
        assert_eq!(run(&mut Fake::default(), &[c"-x", c"42"]).0, USAGE_STATUS);
        assert_eq!(run(&mut Fake::default(), &[]).0, USAGE_STATUS);
    }

    #[test]
    fn l_lists_the_names_or_names_one() {
        let (status, output, _) = run(&mut Fake::default(), &[c"-l"]);
        assert_eq!(status, 0);
        assert!(output.starts_with("HUP INT QUIT KILL"));
        assert_eq!(run(&mut Fake::default(), &[c"-l", c"15"]).1, "TERM\n");
        assert_eq!(run(&mut Fake::default(), &[c"-l", c"99"]).0, 1);
    }
}
