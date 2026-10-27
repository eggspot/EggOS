use crate::lines::{STANDARD_INPUT_NAME, each_line, is_standard_input};
use crate::options::{Item, parse};
use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use core::ops::ControlFlow;

const UNKNOWN_SIZE_WIDTH: usize = 7;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    lines: u64,
    words: u64,
    bytes: u64,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Shown {
    lines: bool,
    words: bool,
    bytes: bool,
}

pub fn wc<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut shown = Shown::default();
    let Ok(operands) = parse(output, arguments, b"lwc", b"", |_, item| {
        match item {
            Item::Flag(b'l') => shown.lines = true,
            Item::Flag(b'w') => shown.words = true,
            _ => shown.bytes = true,
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    if !(shown.lines || shown.words || shown.bytes) {
        shown = Shown {
            lines: true,
            words: true,
            bytes: true,
        };
    }
    let mut names = operands.all();
    if names.is_empty() {
        names = &[STANDARD_INPUT_NAME];
    }
    let mut counts = [Counts::default(); MAX_ARGUMENTS];
    let mut failed = [false; MAX_ARGUMENTS];
    let mut total = Counts::default();
    for (index, &name) in names.iter().enumerate() {
        let (result, status) = count(output, name);
        counts[index] = result;
        failed[index] = status != 0;
        total.add(result);
    }
    let fields = usize::from(shown.lines) + usize::from(shown.words) + usize::from(shown.bytes);
    let width = if fields == 1 && names.len() == 1 {
        0
    } else if names.iter().any(|&name| is_standard_input(name)) {
        UNKNOWN_SIZE_WIDTH
    } else {
        digits(total.lines.max(total.words).max(total.bytes))
    };
    let named = !operands.all().is_empty();
    for (index, &name) in names.iter().enumerate() {
        if !failed[index] {
            write_counts(
                output,
                counts[index],
                shown,
                width,
                named.then_some(name.to_bytes()),
            );
        }
    }
    if names.len() > 1 {
        write_counts(output, total, shown, width, Some(b"total"));
    }
    if failed[..names.len()].contains(&true) {
        FAILURE
    } else {
        0
    }
}

fn digits(value: u64) -> usize {
    value.checked_ilog10().map_or(1, |log| log as usize + 1)
}

fn count<S: System>(output: &mut Output<S>, name: &CStr) -> (Counts, i32) {
    let mut counts = Counts::default();
    let mut in_word = false;
    let status = each_line(output, name, |_, line| {
        counts.bytes += line.len() as u64;
        for &byte in line {
            let blank = byte.is_ascii_whitespace();
            if !blank && !in_word {
                counts.words += 1;
            }
            in_word = !blank;
            counts.lines += u64::from(byte == b'\n');
        }
        ControlFlow::Continue(())
    });
    (counts, status)
}

fn write_counts<S: System>(
    output: &mut Output<S>,
    counts: Counts,
    shown: Shown,
    width: usize,
    name: Option<&[u8]>,
) {
    let mut first = true;
    for (show, value) in [
        (shown.lines, counts.lines),
        (shown.words, counts.words),
        (shown.bytes, counts.bytes),
    ] {
        if show {
            if !first {
                output.write(b" ");
            }
            output.write_number(value, width);
            first = false;
        }
    }
    if let Some(name) = name {
        output.write(b" ");
        output.write(name);
    }
    output.write(b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(standard_input: &[u8], arguments: &[&CStr]) -> (i32, String, String) {
        let mut fake = Fake::default()
            .with_file("/a", 0o644, b"one two\nthree\n", 0)
            .with_file("/b", 0o644, b"x", 0);
        fake.standard_input = standard_input.to_vec();
        let mut output = Output::new(fake, b"wc");
        let status = wc(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn standard_input_gets_lines_words_and_bytes_seven_wide() {
        let (status, output, _) = run(b"hello\n", &[]);
        assert_eq!((status, output.as_str()), (0, "      1       1       6\n"));
        let (_, output, _) = run(b"a b\nc", &[c"-l"]);
        assert_eq!(output, "1\n");
        let (_, output, _) = run(b"  a\tb  \n\n", &[c"-wc"]);
        assert_eq!(output, "      2       9\n");
    }

    #[test]
    fn files_are_named_and_totalled_as_wide_as_the_largest_count() {
        let (status, output, errors) = run(b"", &[c"/a", c"/missing", c"/b"]);
        assert_eq!(status, 1);
        assert_eq!(output, " 2  3 14 /a\n 0  1  1 /b\n 2  4 15 total\n");
        assert_eq!(errors, "wc: /missing: No such file or directory\n");
        let (_, output, _) = run(b"", &[c"-l", c"/a"]);
        assert_eq!(output, "2 /a\n");
    }

    #[test]
    fn unknown_options_are_refused() {
        let (status, _, errors) = run(b"", &[c"-x"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "wc: invalid option -- 'x'\n");
    }
}
