use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;

pub const OPERATING_SYSTEM: &[u8] = b"EggOS";

const UNKNOWN: &[u8] = b"unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Kernel(usize),
    Unknown,
    OperatingSystem,
}

#[derive(Debug)]
struct Field {
    letter: u8,
    long: &'static [u8],
    source: Source,
}

const FIELDS: [Field; 8] = [
    Field {
        letter: b's',
        long: b"--kernel-name",
        source: Source::Kernel(0),
    },
    Field {
        letter: b'n',
        long: b"--nodename",
        source: Source::Kernel(1),
    },
    Field {
        letter: b'r',
        long: b"--kernel-release",
        source: Source::Kernel(2),
    },
    Field {
        letter: b'v',
        long: b"--kernel-version",
        source: Source::Kernel(3),
    },
    Field {
        letter: b'm',
        long: b"--machine",
        source: Source::Kernel(4),
    },
    Field {
        letter: b'p',
        long: b"--processor",
        source: Source::Unknown,
    },
    Field {
        letter: b'i',
        long: b"--hardware-platform",
        source: Source::Unknown,
    },
    Field {
        letter: b'o',
        long: b"--operating-system",
        source: Source::OperatingSystem,
    },
];

const KERNEL_NAME: u8 = 1;

const ALL_KNOWN: u8 = 0b1001_1111;

pub fn uname<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut selected = 0u8;
    let mut options_end = false;
    for argument in arguments {
        let bytes = argument.to_bytes();
        if options_end || bytes.len() < 2 || bytes[0] != b'-' {
            output.error(&[b"extra operand '", bytes, b"'"]);
            return FAILURE;
        }
        if bytes == b"--" {
            options_end = true;
        } else if bytes == b"--all" {
            selected |= ALL_KNOWN;
        } else if bytes.starts_with(b"--") {
            match FIELDS.iter().position(|field| field.long == bytes) {
                Some(index) => selected |= 1 << index,
                None => return output.invalid_option(bytes),
            }
        } else {
            for &letter in &bytes[1..] {
                match FIELDS.iter().position(|field| field.letter == letter) {
                    Some(index) => selected |= 1 << index,
                    None if letter == b'a' => selected |= ALL_KNOWN,
                    None => return output.invalid_option(&[b'-', letter]),
                }
            }
        }
    }
    if selected == 0 {
        selected = KERNEL_NAME;
    }
    let name = match output.system.system_name() {
        Ok(name) => name,
        Err(errno) => {
            output.error(&[b"cannot get system name: ", errno.message()]);
            return FAILURE;
        }
    };
    let mut first = true;
    for (index, field) in FIELDS.iter().enumerate() {
        if selected & 1 << index == 0 {
            continue;
        }
        if !first {
            output.write(b" ");
        }
        first = false;
        output.write(match field.source {
            Source::Kernel(index) => name.field(index),
            Source::Unknown => UNKNOWN,
            Source::OperatingSystem => OPERATING_SYSTEM,
        });
    }
    output.write(b"\n");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(arguments: &[&CStr]) -> (i32, String, String) {
        let mut output = Output::new(Fake::default(), b"uname");
        let status = uname(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn the_kernel_name_is_the_default_and_fields_come_in_a_fixed_order() {
        assert_eq!(run(&[]).1, "Linux\n");
        assert_eq!(run(&[c"-m", c"-s"]).1, "Linux aarch64\n");
        assert_eq!(run(&[c"-rn"]).1, "localhost 6.1.0-eggos\n");
        assert_eq!(run(&[c"--machine", c"--",]).1, "aarch64\n");
        assert_eq!(run(&[c"-o"]).1, "EggOS\n");
    }

    #[test]
    fn dash_a_shows_every_known_field_and_unknown_ones_only_when_asked() {
        let all = "Linux localhost 6.1.0-eggos #1 SMP EggOS 0.2.0 aarch64 EggOS\n";
        assert_eq!(run(&[c"-a"]).1, all);
        assert_eq!(run(&[c"--all"]).1, all);
        assert_eq!(run(&[c"-p", c"-i"]).1, "unknown unknown\n");
        assert_eq!(
            run(&[c"-ap"]).1,
            all.replace(" EggOS\n", " unknown EggOS\n")
        );
    }

    #[test]
    fn bad_options_and_operands_are_usage_errors() {
        assert_eq!(
            run(&[c"-x"]),
            (1, String::new(), "uname: invalid option -- 'x'\n".into())
        );
        assert_eq!(
            run(&[c"--bogus"]),
            (
                1,
                String::new(),
                "uname: unrecognized option '--bogus'\n".into()
            )
        );
        assert_eq!(
            run(&[c"-s", c"extra"]),
            (1, String::new(), "uname: extra operand 'extra'\n".into())
        );
        assert_eq!(run(&[c"--", c"-s"]).0, 1);
    }
}
