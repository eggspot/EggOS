//! `taskset` (D198): runs a command on chosen CPUs, as util-linux's does.
//! `taskset <mask> <command> [arguments...]` takes a hexadecimal mask (bit n for CPU n, `0x`
//! optional); `taskset -c <list> <command> [arguments...]` a list such as `0,2-3`. The command
//! is looked up in `/bin` unless its name holds a `/`, and its threads inherit the CPUs. Only
//! the first 64 CPUs can be chosen.

use crate::output::Output;
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: taskset [-c] <mask|list> [--] <command> [arguments...]";
const USAGE_STATUS: i32 = 2;
const NOT_RUN: i32 = 126;
const NOT_FOUND: i32 = 127;
const PROGRAM_BYTES: usize = 256;
const BIN: &[u8] = b"/bin/";

/// The CPUs a hexadecimal mask names, `0x` optional; `None` when it is not one or sets a CPU
/// past the 64th.
pub fn parse_mask(text: &[u8]) -> Option<u64> {
    let digits = text
        .strip_prefix(b"0x")
        .or_else(|| text.strip_prefix(b"0X"))
        .unwrap_or(text);
    if digits.is_empty() || digits.len() > 16 {
        return None;
    }
    digits.iter().try_fold(0u64, |mask, &digit| {
        let value = (digit as char).to_digit(16)?;
        Some(mask << 4 | u64::from(value))
    })
}

fn number(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 2 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    text.iter()
        .try_fold(0u32, |value, &digit| {
            Some(value * 10 + u32::from(digit - b'0'))
        })
        .filter(|&cpu| cpu < 64)
}

/// The CPUs a list such as `0,2-3` names; `None` when it is not one or names a CPU past the
/// 64th.
pub fn parse_list(text: &[u8]) -> Option<u64> {
    let mut mask = 0u64;
    for part in text.split(|&byte| byte == b',') {
        let (first, last) = match part.iter().position(|&byte| byte == b'-') {
            Some(dash) => (number(&part[..dash])?, number(&part[dash + 1..])?),
            None => {
                let cpu = number(part)?;
                (cpu, cpu)
            }
        };
        if first > last {
            return None;
        }
        for cpu in first..=last {
            mask |= 1 << cpu;
        }
    }
    (mask != 0).then_some(mask)
}

pub fn taskset<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let (mask, rest) = match arguments {
        [option, list, rest @ ..] if matches!(option.to_bytes(), b"-c" | b"--cpu-list") => {
            (parse_list(list.to_bytes()), rest)
        }
        [mask, rest @ ..] if !mask.to_bytes().starts_with(b"-") => {
            (parse_mask(mask.to_bytes()), rest)
        }
        _ => (None, &[][..]),
    };
    let command = match rest {
        [dashes, command @ ..] if dashes.to_bytes() == b"--" => command,
        command => command,
    };
    let (Some(mask), Some(program)) = (mask, command.first()) else {
        output.coded(&errors::USAGE, &[USAGE]);
        return USAGE_STATUS;
    };
    if let Err(errno) = output.system.set_affinity(mask) {
        let why: &[u8] = match errno {
            Errno::EINVAL => b"none of those CPUs is online",
            other => other.message(),
        };
        output.coded(&errors::FAILED, &[b"cannot set the CPUs: ", why]);
        return 1;
    }
    let name = program.to_bytes();
    let mut path = [0u8; PROGRAM_BYTES];
    let program_path = match name.contains(&b'/') {
        true => *program,
        false => {
            let Some(place) = path.get_mut(..BIN.len() + name.len() + 1) else {
                output.coded(&errors::FAILED, &[name, b": the name is too long"]);
                return NOT_FOUND;
            };
            place[..BIN.len()].copy_from_slice(BIN);
            place[BIN.len()..BIN.len() + name.len()].copy_from_slice(name);
            match CStr::from_bytes_with_nul(place) {
                Ok(path) => path,
                Err(_) => {
                    output.coded(&errors::USAGE, &[USAGE]);
                    return USAGE_STATUS;
                }
            }
        }
    };
    let errno = output.system.execute(program_path, command);
    output.coded(&errors::FAILED, &[name, b": ", errno.message()]);
    match errno {
        Errno::ENOENT => NOT_FOUND,
        _ => NOT_RUN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
        let mut output = Output::new(&mut *fake, b"taskset");
        let status = taskset(&mut output, arguments);
        output.finish(status)
    }

    #[test]
    fn masks_and_lists_name_the_cpus() {
        assert_eq!(parse_mask(b"0x5"), Some(5));
        assert_eq!(parse_mask(b"ff"), Some(255));
        assert_eq!(parse_mask(b"0x"), None);
        assert_eq!(parse_mask(b"12345678123456789"), None);
        assert_eq!(parse_list(b"0,2-3"), Some(0b1101));
        assert_eq!(parse_list(b"63"), Some(1 << 63));
        for wrong in [&b"64"[..], b"3-1", b"", b"1,,2", b"a"] {
            assert_eq!(parse_list(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn the_command_runs_from_bin_on_the_cpus_given() {
        let mut fake = Fake::default();
        assert_eq!(
            run(&mut fake, &[c"-c", c"0-1", c"--", c"ls", c"-l"]),
            NOT_FOUND
        );
        assert_eq!(fake.affinity, Some(3));
        assert_eq!(fake.executed, [b"/bin/ls ls -l".to_vec()]);
        let mut fake = Fake::default();
        run(&mut fake, &[c"0x2", c"/work/tool"]);
        assert_eq!(fake.executed, [b"/work/tool /work/tool".to_vec()]);
    }

    #[test]
    fn no_online_cpu_or_no_command_is_refused() {
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &[c"0x10", c"ls"]), 1);
        assert!(fake.executed.is_empty());
        assert_eq!(run(&mut Fake::default(), &[c"0x1"]), USAGE_STATUS);
        assert_eq!(
            run(&mut Fake::default(), &[c"-c", c"x", c"ls"]),
            USAGE_STATUS
        );
    }
}
