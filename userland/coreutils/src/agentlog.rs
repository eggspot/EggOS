use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_files::clock::deletion_date;
use eggos_mcp::audit::{self, Outcome, Record};
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: agentlog [-n <count>] [--raw]";
const USAGE_STATUS: i32 = 2;
const LOGS: [&CStr; 2] = [c"/var/log/agent-audit.log.1", c"/var/log/agent-audit.log"];
const LINE_BYTES: usize = 4096;
const CHUNK: usize = 4096;
const HEADER: &[u8] = b"TIME  CLIENT  TOOL  RESULT  ARGUMENTS  DETAIL\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Rows,
    Raw,
}

pub fn agentlog<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut mode = Mode::Rows;
    let mut last = None;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match argument.to_bytes() {
            b"--raw" => mode = Mode::Raw,
            b"-n" => {
                index += 1;
                let Some(count) = arguments
                    .get(index)
                    .and_then(|count| number(count.to_bytes()))
                else {
                    output.error(&[USAGE]);
                    return USAGE_STATUS;
                };
                last = Some(count);
            }
            bytes if bytes.starts_with(b"-n") => match number(&bytes[2..]) {
                Some(count) => last = Some(count),
                None => {
                    output.error(&[USAGE]);
                    return USAGE_STATUS;
                }
            },
            bytes if bytes.starts_with(b"-") => {
                output.invalid_option(bytes);
                output.error(&[USAGE]);
                return USAGE_STATUS;
            }
            _ => {
                output.error(&[USAGE]);
                return USAGE_STATUS;
            }
        }
        index += 1;
    }
    let total = match each_row(output, mode, &mut |_, _| {}) {
        Ok(total) => total,
        Err(status) => return status,
    };
    let skip = last.map_or(0, |last| total.saturating_sub(last));
    if mode == Mode::Rows && total > skip {
        output.write(HEADER);
    }
    let mut seen = 0;
    match each_row(output, mode, &mut |output, row| {
        if seen >= skip {
            output.write(row);
        }
        seen += 1;
    }) {
        Ok(_) => 0,
        Err(status) => status,
    }
}

fn number(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(bytes).ok()?.parse().ok()
}

struct Pending {
    line: [u8; LINE_BYTES],
    length: usize,
}

impl Pending {
    fn record(&self) -> Option<Record<'_>> {
        (self.length > 0)
            .then(|| audit::parse_line(&self.line[..self.length]))
            .flatten()
    }
}

fn each_row<S: System>(
    output: &mut Output<S>,
    mode: Mode,
    row: &mut dyn FnMut(&mut Output<S>, &[u8]),
) -> Result<usize, i32> {
    let mut rows = 0;
    let mut pending = Pending {
        line: [0; LINE_BYTES],
        length: 0,
    };
    let mut text = [0u8; LINE_BYTES * 2];
    for log in LOGS {
        let descriptor = match output.system.open(log) {
            Ok(descriptor) => descriptor,
            Err(Errno::ENOENT) => continue,
            Err(errno) => {
                output.file_error(log, errno);
                return Err(FAILURE);
            }
        };
        let mut line = [0u8; LINE_BYTES];
        let mut length = 0;
        let mut too_long = false;
        let mut chunk = [0u8; CHUNK];
        loop {
            let count = match output.system.read(descriptor, &mut chunk) {
                Ok(0) => break,
                Ok(count) => count,
                Err(errno) => {
                    output.system.close(descriptor);
                    output.file_error(log, errno);
                    return Err(FAILURE);
                }
            };
            for &byte in &chunk[..count] {
                if byte != b'\n' {
                    match line.get_mut(length) {
                        Some(slot) => {
                            *slot = byte;
                            length += 1;
                        }
                        None => too_long = true,
                    }
                    continue;
                }
                if !too_long {
                    rows += take_line(output, mode, &line[..length], &mut pending, &mut text, row);
                }
                length = 0;
                too_long = false;
            }
        }
        output.system.close(descriptor);
    }
    if let Some(record) = pending.record() {
        let shown = format_row(&record, Some(b"interrupted"), &mut text);
        row(output, &text[..shown]);
        rows += 1;
    }
    Ok(rows)
}

fn take_line<S: System>(
    output: &mut Output<S>,
    mode: Mode,
    line: &[u8],
    pending: &mut Pending,
    text: &mut [u8],
    row: &mut dyn FnMut(&mut Output<S>, &[u8]),
) -> usize {
    let Some(record) = audit::parse_line(line) else {
        return 0;
    };
    if mode == Mode::Raw {
        let shown = format_raw(line, text);
        row(output, &text[..shown]);
        return 1;
    }
    let mut rows = 0;
    let pairs = pending
        .record()
        .is_some_and(|started| started.call == record.call && record.outcome.ends_a_call());
    if !pairs && let Some(started) = pending.record() {
        let shown = format_row(&started, Some(b"interrupted"), text);
        row(output, &text[..shown]);
        rows += 1;
    }
    if record.outcome == Outcome::Started {
        pending.line[..line.len()].copy_from_slice(line);
        pending.length = line.len();
        return rows;
    }
    pending.length = 0;
    let shown = format_row(&record, None, text);
    row(output, &text[..shown]);
    rows + 1
}

fn format_raw(line: &[u8], text: &mut [u8]) -> usize {
    let mut length = 0;
    for (index, field) in line.split(|&byte| byte == b'\t').enumerate() {
        if index > 0 {
            length += put(text, length, b"\t");
        }
        length += audit::display_into(field, &mut text[length..]);
    }
    length + put(text, length, b"\n")
}

fn put(text: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    match text.get_mut(at..at + bytes.len()) {
        Some(place) => {
            place.copy_from_slice(bytes);
            bytes.len()
        }
        None => 0,
    }
}

fn format_row(record: &Record<'_>, result: Option<&[u8]>, text: &mut [u8]) -> usize {
    let stamp = deletion_date(record.time);
    let mut length = put(text, 0, stamp.as_str().as_bytes());
    let result = result.unwrap_or(record.outcome.word().as_bytes());
    for field in [
        record.client,
        record.tool,
        result,
        record.arguments,
        record.detail,
    ] {
        length += put(text, length, b"  ");
        let end = text.len().min(length + audit::FIELD_BYTES);
        length += audit::display_into(field, &mut text[length..end]);
    }
    length + put(text, length, b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    const LOG: &str = "/var/log/agent-audit.log";

    fn run(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
        let mut output = Output::new(fake, b"agentlog");
        let status = agentlog(&mut output, arguments);
        output.finish(status)
    }

    fn log() -> String {
        [
            "0\t0\t-\t-\tserver-start\t\t0.2.0",
            "1\t0\tclaude-code\t-\tinitialize\t{\"requested\":\"2025-11-25\"}\t",
            "2\t1\tclaude-code\tcat\tstarted\t{\"path\":\"/home/a\"}\t/home/a",
            "3\t1\tclaude-code\tcat\tok\t{\"path\":\"/home/a\"}\texit status 0, 5 bytes",
            "4\t2\tclaude-code\tcat\trefused\t{\"path\":\"/home/../etc\"}\tRefused: outside",
            "5\t3\tclaude-code\tps\tstarted\t{}\t",
            "6\t0\t-\t-\tserver-restart\t\t0.2.0",
            "not a record",
            "7\t1\tunknown (restarted)\tls\tstarted\t{}\t/home",
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn calls_are_one_row_each_and_unfinished_ones_are_interrupted() {
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].starts_with("TIME"));
        assert_eq!(lines.len(), 1 + 7);
        assert!(lines[1].contains("server-start"));
        assert!(lines[2].contains("claude-code  -  initialize"));
        assert!(lines[3].contains("cat  ok  {\"path\":\"/home/a\"}  exit status 0, 5 bytes"));
        assert!(lines[4].contains("cat  refused"));
        assert!(lines[5].contains("ps  interrupted"));
        assert!(lines[6].contains("server-restart"));
        assert!(lines[7].contains("unknown (restarted)  ls  interrupted"));
        assert!(lines[3].starts_with("1970-01-01T00:00:03"));
    }

    #[test]
    fn the_last_rows_and_the_raw_records_are_shown_on_request() {
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"-n", c"2"]), 0);
        let output = fake.output();
        assert_eq!(output.lines().count(), 3);
        assert!(output.lines().nth(1).unwrap().contains("server-restart"));
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"--raw"]), 0);
        assert_eq!(fake.output().lines().count(), 8);
        assert!(fake.output().starts_with("0\t0\t-\t-\tserver-start"));
    }

    #[test]
    fn the_rotated_log_comes_first_and_a_missing_log_shows_nothing() {
        let mut fake = Fake::default()
            .with_file(LOG, 0o640, b"9\t0\t-\t-\tserver-start\t\tnew\n", 0)
            .with_file(
                "/var/log/agent-audit.log.1",
                0o640,
                b"1\t0\t-\t-\tserver-start\t\told\n",
                0,
            );
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        assert!(output.find("old").unwrap() < output.find("new").unwrap());
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &[]), 0);
        assert_eq!(fake.output(), "");
    }

    #[test]
    fn hidden_and_control_characters_are_shown_escaped() {
        let mut bytes = "1\t1\tevil\u{1b}[2J\u{202e}\tcat\tok\t{\"path\":\"/home/\u{202e}x\"}\t"
            .as_bytes()
            .to_vec();
        bytes.extend_from_slice(b"\xff\n");
        let mut fake = Fake::default().with_file(LOG, 0o640, &bytes, 0);
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        assert!(output.contains("evil\\e[2J\\u{202e}"));
        assert!(output.contains("/home/\\u{202e}x"));
        assert!(output.contains('\u{fffd}'));
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains('\u{202e}'));
    }

    #[test]
    fn bad_options_are_usage_errors() {
        for arguments in [&[c"-x"][..], &[c"-n"], &[c"-n", c"many"], &[c"file"]] {
            let mut fake = Fake::default();
            assert_eq!(run(&mut fake, arguments), USAGE_STATUS);
            assert!(fake.errors().contains("usage: agentlog"));
        }
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"-n1"]), 0);
        assert_eq!(fake.output().lines().count(), 2);
    }
}
