use crate::lines::{STANDARD_INPUT_NAME, each_line};
use crate::options::{Item, parse};
use crate::output::Output;
use crate::system::System;
use core::ffi::CStr;
use core::ops::ControlFlow;

const SELECTED: i32 = 0;
const NONE_SELECTED: i32 = 1;
const TROUBLE: i32 = 2;

const MAX_PATTERN: usize = 1024;

#[derive(Debug, Clone, Copy, Default)]
struct Options {
    ignore_case: bool,
    invert: bool,
    line_numbers: bool,
    count: bool,
    fixed: bool,
    quiet: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Atom {
    Byte(u8),
    Any,
    Class {
        start: usize,
        end: usize,
        negated: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    atom: Atom,
    repeated: bool,
}

#[derive(Debug)]
struct Pattern<'p> {
    source: &'p [u8],
    pieces: [Piece; MAX_PATTERN],
    count: usize,
    anchored_start: bool,
    anchored_end: bool,
    ignore_case: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternError {
    UnmatchedBracket,
    TrailingBackslash,
    TooLong,
}

fn fold(byte: u8, ignore_case: bool) -> u8 {
    match ignore_case {
        true => byte.to_ascii_lowercase(),
        false => byte,
    }
}

impl<'p> Pattern<'p> {
    fn compile(source: &'p [u8], fixed: bool, ignore_case: bool) -> Result<Self, PatternError> {
        let mut pattern = Self {
            source,
            pieces: [Piece {
                atom: Atom::Any,
                repeated: false,
            }; MAX_PATTERN],
            count: 0,
            anchored_start: false,
            anchored_end: false,
            ignore_case,
        };
        let mut at = 0;
        let mut end = source.len();
        if !fixed {
            if source.first() == Some(&b'^') {
                pattern.anchored_start = true;
                at = 1;
            }
            if end > at && source[end - 1] == b'$' && (end < 2 || source[end - 2] != b'\\') {
                pattern.anchored_end = true;
                end -= 1;
            }
        }
        while at < end {
            let byte = source[at];
            let atom = match byte {
                _ if fixed => {
                    at += 1;
                    Atom::Byte(byte)
                }
                b'.' => {
                    at += 1;
                    Atom::Any
                }
                b'\\' => {
                    let escaped = *source
                        .get(at + 1)
                        .filter(|_| at + 1 < end)
                        .ok_or(PatternError::TrailingBackslash)?;
                    at += 2;
                    Atom::Byte(escaped)
                }
                b'[' => {
                    let negated = source.get(at + 1) == Some(&b'^');
                    let start = at + 1 + usize::from(negated);
                    let first_end = start + 1;
                    let close = source[first_end.min(end)..end]
                        .iter()
                        .position(|&byte| byte == b']')
                        .ok_or(PatternError::UnmatchedBracket)?;
                    at = first_end + close + 1;
                    Atom::Class {
                        start,
                        end: first_end + close,
                        negated,
                    }
                }
                b'*' if pattern.count == 0 => {
                    at += 1;
                    Atom::Byte(b'*')
                }
                b'*' => {
                    pattern.pieces[pattern.count - 1].repeated = true;
                    at += 1;
                    continue;
                }
                _ => {
                    at += 1;
                    Atom::Byte(byte)
                }
            };
            if pattern.count == MAX_PATTERN {
                return Err(PatternError::TooLong);
            }
            pattern.pieces[pattern.count] = Piece {
                atom,
                repeated: false,
            };
            pattern.count += 1;
        }
        Ok(pattern)
    }

    fn atom_matches(&self, atom: Atom, byte: u8) -> bool {
        let byte = fold(byte, self.ignore_case);
        match atom {
            Atom::Byte(expected) => fold(expected, self.ignore_case) == byte,
            Atom::Any => byte != b'\n',
            Atom::Class {
                start,
                end,
                negated,
            } => {
                let class = &self.source[start..end];
                let mut index = 0;
                let mut found = false;
                while index < class.len() {
                    let low = fold(class[index], self.ignore_case);
                    if index + 2 < class.len() && class[index + 1] == b'-' {
                        let high = fold(class[index + 2], self.ignore_case);
                        found |= (low..=high).contains(&byte);
                        index += 3;
                    } else {
                        found |= low == byte;
                        index += 1;
                    }
                }
                found != negated
            }
        }
    }

    fn matches_here(&self, pieces: &[Piece], text: &[u8]) -> bool {
        let Some((&piece, rest)) = pieces.split_first() else {
            return !self.anchored_end || text.is_empty();
        };
        if piece.repeated {
            let mut taken = 0;
            while taken < text.len() && self.atom_matches(piece.atom, text[taken]) {
                taken += 1;
            }
            return (0..=taken)
                .rev()
                .any(|count| self.matches_here(rest, &text[count..]));
        }
        match text.first() {
            Some(&byte) if self.atom_matches(piece.atom, byte) => {
                self.matches_here(rest, &text[1..])
            }
            _ => false,
        }
    }

    fn is_found_in(&self, line: &[u8]) -> bool {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let pieces = &self.pieces[..self.count];
        match self.anchored_start {
            true => self.matches_here(pieces, line),
            false => (0..=line.len()).any(|start| self.matches_here(pieces, &line[start..])),
        }
    }
}

pub fn grep<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut options = Options::default();
    let parsed = parse(output, arguments, b"ivncFGq", b"", |_, item| {
        if let Item::Flag(letter) = item {
            match letter {
                b'i' => options.ignore_case = true,
                b'v' => options.invert = true,
                b'n' => options.line_numbers = true,
                b'c' => options.count = true,
                b'F' => options.fixed = true,
                b'G' => options.fixed = false,
                _ => options.quiet = true,
            }
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return TROUBLE;
    };
    let Some((&source, names)) = operands.all().split_first() else {
        output.error(&[b"usage: grep [-icnqvF] pattern [file ...]"]);
        return TROUBLE;
    };
    let pattern = match Pattern::compile(source.to_bytes(), options.fixed, options.ignore_case) {
        Ok(pattern) => pattern,
        Err(error) => {
            output.error(&[match error {
                PatternError::UnmatchedBracket => b"Unmatched [, [^, [:, [., or [=",
                PatternError::TrailingBackslash => b"Trailing backslash",
                PatternError::TooLong => b"pattern too long",
            }]);
            return TROUBLE;
        }
    };
    let names = match names {
        [] => &[STANDARD_INPUT_NAME],
        names => names,
    };
    let prefixed = names.len() > 1;
    let mut selected_any = false;
    let mut trouble = false;
    for &name in names {
        let label = match name.to_bytes() {
            b"-" => b"(standard input)",
            name => name,
        };
        let mut selected = 0u64;
        let mut line_number = 0u64;
        let status = each_line(output, name, |output, line| {
            line_number += 1;
            if pattern.is_found_in(line) == options.invert {
                return ControlFlow::Continue(());
            }
            selected += 1;
            if options.quiet {
                return ControlFlow::Break(());
            }
            if !options.count {
                if prefixed {
                    output.write(label);
                    output.write(b":");
                }
                if options.line_numbers {
                    output.write_number(line_number, 0);
                    output.write(b":");
                }
                output.write(line);
                if !line.ends_with(b"\n") {
                    output.write(b"\n");
                }
            }
            ControlFlow::Continue(())
        });
        trouble |= status != 0;
        selected_any |= selected > 0;
        if options.count && !options.quiet {
            if prefixed {
                output.write(label);
                output.write(b":");
            }
            output.write_number(selected, 0);
            output.write(b"\n");
        }
        if options.quiet && selected_any {
            return SELECTED;
        }
    }
    match (trouble, selected_any) {
        (true, _) => TROUBLE,
        (false, true) => SELECTED,
        (false, false) => NONE_SELECTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn found(pattern: &str, line: &str) -> bool {
        Pattern::compile(pattern.as_bytes(), false, false)
            .unwrap()
            .is_found_in(line.as_bytes())
    }

    #[test]
    fn the_basic_regular_expressions_match_as_posix_says() {
        assert!(found("bin", "/sbin\n"));
        assert!(!found("^bin", "sbin"));
        assert!(found("^bin$", "bin\n"));
        assert!(!found("bin$", "binary"));
        assert!(found("b.n", "ban"));
        assert!(found("ab*c", "ac"));
        assert!(found("ab*c", "abbbc"));
        assert!(found("^a.*z$", "a to z"));
        assert!(found("[0-9][a-c]", "x7b"));
        assert!(!found("[^0-9]", "123"));
        assert!(found("a\\.b", "a.b"));
        assert!(!found("a\\.b", "axb"));
        assert!(found("*star", "a *star"));
        assert!(found("", "anything"));
        assert!(found("[]x]", "]"));
    }

    #[test]
    fn bad_patterns_are_refused() {
        assert_eq!(
            Pattern::compile(b"[abc", false, false).unwrap_err(),
            PatternError::UnmatchedBracket
        );
        assert_eq!(
            Pattern::compile(b"ab\\", false, false).unwrap_err(),
            PatternError::TrailingBackslash
        );
        assert!(Pattern::compile(b"[abc", true, false).is_ok());
    }

    fn run(input: &[u8], arguments: &[&CStr]) -> (i32, String, String) {
        let mut fake = Fake::default()
            .with_file("/a", 0o644, b"Bin\nboot\nsbin\n", 0)
            .with_file("/b", 0o644, b"nothing", 0);
        fake.standard_input = input.to_vec();
        let mut output = Output::new(fake, b"grep");
        let status = grep(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn selected_lines_are_written_with_their_file_and_number_when_asked() {
        assert_eq!(
            run(b"bin\nboot\n", &[c"bin"]),
            (0, "bin\n".into(), "".into())
        );
        assert_eq!(run(b"", &[c"-i", c"bin", c"/a"]).1, "Bin\nsbin\n");
        assert_eq!(run(b"", &[c"-n", c"bin", c"/a"]).1, "3:sbin\n");
        assert_eq!(run(b"", &[c"-v", c"bin", c"/a"]).1, "Bin\nboot\n");
        assert_eq!(run(b"", &[c"-c", c"o", c"/a", c"/b"]).1, "/a:1\n/b:1\n");
        assert_eq!(run(b"", &[c"thing", c"/a", c"/b"]).1, "/b:nothing\n");
        assert_eq!(run(b"a.b\naxb\n", &[c"-F", c"a.b"]).1, "a.b\n");
        assert_eq!(run(b"x\n", &[c"-q", c"x"]), (0, "".into(), "".into()));
    }

    #[test]
    fn the_status_says_whether_a_line_was_selected_or_something_failed() {
        assert_eq!(run(b"a\n", &[c"z"]).0, 1);
        let (status, _, errors) = run(b"", &[c"x", c"/missing"]);
        assert_eq!(status, 2);
        assert_eq!(errors, "grep: /missing: No such file or directory\n");
        let (status, _, errors) = run(b"", &[]);
        assert_eq!(status, 2);
        assert!(errors.starts_with("grep: usage"));
        assert_eq!(
            run(b"", &[c"["]).2,
            "grep: Unmatched [, [^, [:, [., or [=\n"
        );
    }
}
