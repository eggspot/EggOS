use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_mcp::json::JsonWriter;

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
    source: Source,
}

const FIELDS: [Field; 8] = [
    Field {
        letter: b's',
        source: Source::Kernel(0),
    },
    Field {
        letter: b'n',
        source: Source::Kernel(1),
    },
    Field {
        letter: b'r',
        source: Source::Kernel(2),
    },
    Field {
        letter: b'v',
        source: Source::Kernel(3),
    },
    Field {
        letter: b'm',
        source: Source::Kernel(4),
    },
    Field {
        letter: b'p',
        source: Source::Unknown,
    },
    Field {
        letter: b'i',
        source: Source::Unknown,
    },
    Field {
        letter: b'o',
        source: Source::OperatingSystem,
    },
];

const KERNEL_NAME: u8 = 1;

const ALL_KNOWN: u8 = 0b1001_1111;

const JSON_BYTES: usize = 1024;

const JSON_KEYS: [Option<&str>; 8] = [
    Some("kernel_name"),
    Some("node_name"),
    Some("kernel_release"),
    Some("kernel_version"),
    Some("machine"),
    None,
    None,
    Some("operating_system"),
];

pub fn uname<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut selected = 0u8;
    let mut json = false;
    let parsed = parse_defined(output, arguments, &commands::UNAME, |_, given| {
        match given.option.short {
            Some(b'a') => selected |= ALL_KNOWN,
            Some(letter) => {
                if let Some(index) = FIELDS.iter().position(|field| field.letter == letter) {
                    selected |= 1 << index;
                }
            }
            None => json = true,
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    if let Some(extra) = operands.all().first() {
        output.coded(
            &errors::USAGE,
            &[b"extra operand '", extra.to_bytes(), b"'"],
        );
        return FAILURE;
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
    if json {
        let mut buffer = [0u8; JSON_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        for (key, field) in JSON_KEYS.into_iter().zip(&FIELDS) {
            let Some(key) = key else {
                continue;
            };
            out.key(key);
            out.string_lossy(match field.source {
                Source::Kernel(index) => name.field(index),
                Source::Unknown => UNKNOWN,
                Source::OperatingSystem => OPERATING_SYSTEM,
            });
        }
        out.end_object();
        out.end_line();
        output.write(out.as_bytes());
        return 0;
    }
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
    fn json_writes_every_known_field() {
        let (status, output, _) = run(&[c"--json"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "{\"kernel_name\":\"Linux\",\"node_name\":\"localhost\",\
             \"kernel_release\":\"6.1.0-eggos\",\"kernel_version\":\"#1 SMP EggOS 0.2.0\",\
             \"machine\":\"aarch64\",\"operating_system\":\"EggOS\"}\n"
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
