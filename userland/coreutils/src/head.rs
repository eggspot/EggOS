use crate::lines::{LINE_CAPACITY, STANDARD_INPUT_NAME, each_line};
use crate::options::{Refused, parse_count, parse_defined};
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use core::ops::ControlFlow;
use eggos_commands::{CommandDefinition, commands, errors};

const DEFAULT_LINES: u64 = 10;

const TAIL_CAPACITY: usize = 64 * 1024;

const TAIL_MAX_LINES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Last(u64),
    FromLine(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Head,
    Tail,
}

pub fn head<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    run(output, arguments, End::Head)
}

pub fn tail<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    run(output, arguments, End::Tail)
}

fn run<S: System>(output: &mut Output<S>, arguments: &[&CStr], end: End) -> i32 {
    let mut start = Start::Last(DEFAULT_LINES);
    let mut headers = None;
    let definition: &'static CommandDefinition = match end {
        End::Head => &commands::HEAD,
        End::Tail => &commands::TAIL,
    };
    let parsed = parse_defined(output, arguments, definition, |output, given| {
        match (given.option.short, given.value) {
            (Some(b'n'), Some(value)) => {
                let (from_start, digits) = match value.strip_prefix(b"+") {
                    Some(digits) if end == End::Tail => (true, digits),
                    _ => (false, value),
                };
                let Some(count) = parse_count(digits) else {
                    output.coded(
                        &errors::USAGE,
                        &[b"invalid number of lines: '", value, b"'"],
                    );
                    return Err(Refused::Reported);
                };
                start = match from_start {
                    true => Start::FromLine(count),
                    false => Start::Last(count),
                };
            }
            (letter, _) => headers = Some(letter == Some(b'v')),
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let mut names = operands.all();
    if names.is_empty() {
        names = &[STANDARD_INPUT_NAME];
    }
    let headers = headers.unwrap_or(names.len() > 1);
    let mut status = 0;
    for (index, &name) in names.iter().enumerate() {
        if headers {
            if index > 0 {
                output.write(b"\n");
            }
            output.write(b"==> ");
            output.write(match name.to_bytes() {
                b"-" => b"standard input",
                name => name,
            });
            output.write(b" <==\n");
        }
        let result = match (end, start) {
            (End::Head, Start::Last(count)) => first_lines(output, name, count),
            (End::Tail, Start::FromLine(line)) => from_line(output, name, line),
            (_, Start::Last(count)) => last_lines(output, name, count),
            (End::Head, Start::FromLine(_)) => unreachable!(),
        };
        status = status.max(result);
    }
    status
}

fn first_lines<S: System>(output: &mut Output<S>, name: &CStr, count: u64) -> i32 {
    let mut left = count;
    if left == 0 {
        return 0;
    }
    each_line(output, name, |output, line| {
        output.write(line);
        if line.ends_with(b"\n") {
            left -= 1;
        }
        match left {
            0 => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    })
}

fn from_line<S: System>(output: &mut Output<S>, name: &CStr, first: u64) -> i32 {
    let mut line_number = 1;
    each_line(output, name, |output, line| {
        if line_number >= first {
            output.write(line);
        }
        if line.ends_with(b"\n") {
            line_number += 1;
        }
        ControlFlow::Continue(())
    })
}

struct Kept {
    bytes: [u8; TAIL_CAPACITY],
    start: usize,
    length: usize,
    lines: [usize; TAIL_MAX_LINES],
    first_line: usize,
    line_count: usize,
}

impl Kept {
    fn drop_oldest(&mut self) {
        let oldest = self.lines[self.first_line];
        self.start = (self.start + oldest) % TAIL_CAPACITY;
        self.length -= oldest;
        self.first_line = (self.first_line + 1) % TAIL_MAX_LINES;
        self.line_count -= 1;
    }

    fn keep(&mut self, line: &[u8], limit: usize) {
        let line = &line[line.len().saturating_sub(TAIL_CAPACITY)..];
        while self.line_count > 0
            && (self.line_count >= limit
                || self.line_count == TAIL_MAX_LINES
                || self.length + line.len() > TAIL_CAPACITY)
        {
            self.drop_oldest();
        }
        if limit == 0 {
            return;
        }
        for (offset, &byte) in line.iter().enumerate() {
            self.bytes[(self.start + self.length + offset) % TAIL_CAPACITY] = byte;
        }
        self.length += line.len();
        self.lines[(self.first_line + self.line_count) % TAIL_MAX_LINES] = line.len();
        self.line_count += 1;
    }

    fn write_to<S: System>(&self, output: &mut Output<S>) {
        let first = self.length.min(TAIL_CAPACITY - self.start);
        output.write(&self.bytes[self.start..self.start + first]);
        output.write(&self.bytes[..self.length - first]);
    }
}

fn last_lines<S: System>(output: &mut Output<S>, name: &CStr, count: u64) -> i32 {
    let limit = usize::try_from(count).unwrap_or(usize::MAX);
    let mut kept = Kept {
        bytes: [0; TAIL_CAPACITY],
        start: 0,
        length: 0,
        lines: [0; TAIL_MAX_LINES],
        first_line: 0,
        line_count: 0,
    };
    let mut partial = false;
    let status = each_line(output, name, |_, line| {
        if partial {
            kept.extend_newest(line);
        } else {
            kept.keep(line, limit);
        }
        partial = !line.ends_with(b"\n") && line.len() == LINE_CAPACITY;
        ControlFlow::Continue(())
    });
    kept.write_to(output);
    status
}

impl Kept {
    fn extend_newest(&mut self, piece: &[u8]) {
        if self.line_count == 0 {
            return;
        }
        let newest = (self.first_line + self.line_count - 1) % TAIL_MAX_LINES;
        let room = TAIL_CAPACITY - self.length;
        let piece = &piece[..piece.len().min(room)];
        for (offset, &byte) in piece.iter().enumerate() {
            self.bytes[(self.start + self.length + offset) % TAIL_CAPACITY] = byte;
        }
        self.length += piece.len();
        self.lines[newest] += piece.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use std::string::String;

    fn numbered(count: usize) -> Vec<u8> {
        (1..=count)
            .map(|line| format!("{line}\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn run(utility: End, input: &[u8], arguments: &[&CStr]) -> (i32, String, String) {
        let mut fake = Fake::default()
            .with_file("/twelve", 0o644, &numbered(12), 0)
            .with_file("/short", 0o644, b"a\nb", 0);
        fake.standard_input = input.to_vec();
        let mut output = Output::new(
            fake,
            if utility == End::Head {
                b"head"
            } else {
                b"tail"
            },
        );
        let status = super::run(&mut output, arguments, utility);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn head_writes_the_first_ten_lines_or_as_many_as_asked() {
        let (status, output, _) = run(End::Head, b"", &[c"/twelve"]);
        assert_eq!(
            (status, output),
            (0, String::from_utf8(numbered(10)).unwrap())
        );
        let (_, output, _) = run(End::Head, b"x\ny\nz\n", &[c"-n", c"2"]);
        assert_eq!(output, "x\ny\n");
        let (_, output, _) = run(End::Head, b"", &[c"-n0", c"/twelve"]);
        assert_eq!(output, "");
        let (_, output, _) = run(End::Head, b"", &[c"-n1", c"/short", c"/twelve"]);
        assert_eq!(output, "==> /short <==\na\n\n==> /twelve <==\n1\n");
    }

    #[test]
    fn tail_writes_the_last_lines_or_from_a_line_on() {
        let (_, output, _) = run(End::Tail, b"", &[c"/twelve"]);
        assert_eq!(
            output,
            (3..=12).map(|line| format!("{line}\n")).collect::<String>()
        );
        let (_, output, _) = run(End::Tail, b"", &[c"-n", c"2", c"/short"]);
        assert_eq!(output, "a\nb");
        let (_, output, _) = run(End::Tail, b"", &[c"-n", c"+11", c"/twelve"]);
        assert_eq!(output, "11\n12\n");
        let (_, output, _) = run(End::Tail, b"1\n2\n3\n", &[c"-n1"]);
        assert_eq!(output, "3\n");
        let (_, output, _) = run(End::Tail, b"1\n2\n", &[c"-n0"]);
        assert_eq!(output, "");
    }

    #[test]
    fn tail_keeps_a_line_longer_than_the_line_buffer_whole() {
        let mut input = b"first\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', LINE_CAPACITY + 100));
        input.extend_from_slice(b"\nlast\n");
        let (_, output, _) = run(End::Tail, &input, &[c"-n2"]);
        assert_eq!(output.len(), LINE_CAPACITY + 101 + 5);
        assert!(output.ends_with("x\nlast\n"));
    }

    #[test]
    fn bad_counts_are_refused() {
        let (status, _, errors) = run(End::Head, b"", &[c"-n", c"x"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "head: invalid number of lines: 'x'\n");
        let (status, _, _) = run(End::Head, b"", &[c"-n", c"+1"]);
        assert_eq!(status, 1);
    }
}
