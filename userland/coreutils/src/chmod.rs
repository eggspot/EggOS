use crate::links::PathText;
use crate::ls::{mode_string, record_names};
use crate::options::parse_defined;
use crate::output::{FAILURE, Output, error_for};
use crate::system::{Status, SymbolicLinks, System};
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_runtime::errno::Errno;

const ENTRIES_CHUNK: usize = 2048;
const UMASK: u32 = 0o022;
const PERMISSION_BITS: u32 = 0o7777;
const WHO: &[u8] = b"ugoa";
const OPERATIONS: &[u8] = b"+-=";
const PERMISSIONS: &[u8] = b"rwxXst";
const MODE_BYTES: &[u8] = b"rwxXstugoa+-=,";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode<'a> {
    Octal(u32),
    Symbolic(&'a [u8]),
}

impl<'a> Mode<'a> {
    fn parse(text: &'a [u8]) -> Option<Self> {
        let octal = !text.is_empty() && text.len() <= 4;
        if octal && text.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
            let value = text
                .iter()
                .fold(0, |value, &digit| value * 8 + u32::from(digit - b'0'));
            return Some(Self::Octal(value));
        }
        let valid = !text.is_empty() && text.split(|&byte| byte == b',').all(is_clause);
        valid.then_some(Self::Symbolic(text))
    }

    fn applied(self, mode: u32, directory: bool) -> u32 {
        match self {
            Self::Octal(value) => value,
            Self::Symbolic(text) => text
                .split(|&byte| byte == b',')
                .fold(mode & PERMISSION_BITS, |mode, clause| {
                    applied_clause(clause, mode, directory)
                }),
        }
    }
}

fn who_end(clause: &[u8]) -> usize {
    clause
        .iter()
        .position(|byte| !WHO.contains(byte))
        .unwrap_or(clause.len())
}

fn operation_end(after: &[u8]) -> usize {
    after
        .iter()
        .position(|byte| OPERATIONS.contains(byte))
        .unwrap_or(after.len())
}

fn is_clause(clause: &[u8]) -> bool {
    let mut rest = &clause[who_end(clause)..];
    if rest.is_empty() {
        return false;
    }
    while let Some((operation, after)) = rest.split_first() {
        if !OPERATIONS.contains(operation) {
            return false;
        }
        let end = operation_end(after);
        let permissions = &after[..end];
        let copied = matches!(permissions, [b'u' | b'g' | b'o']);
        if !copied && !permissions.iter().all(|byte| PERMISSIONS.contains(byte)) {
            return false;
        }
        rest = &after[end..];
    }
    true
}

fn who_bits(letter: u8) -> u32 {
    match letter {
        b'u' => 0o4700,
        b'g' => 0o2070,
        b'o' => 0o1007,
        _ => PERMISSION_BITS,
    }
}

fn permission_bits(letter: u8, mode: u32, directory: bool) -> u32 {
    match letter {
        b'r' => 0o444,
        b'w' => 0o222,
        b'x' => 0o111,
        b'X' if directory || mode & 0o111 != 0 => 0o111,
        b's' => 0o6000,
        b't' => 0o1000,
        _ => 0,
    }
}

fn copied_bits(letter: u8, mode: u32) -> u32 {
    let shift = match letter {
        b'u' => 6,
        b'g' => 3,
        _ => 0,
    };
    ((mode >> shift) & 0o7) * 0o111
}

fn applied_clause(clause: &[u8], mut mode: u32, directory: bool) -> u32 {
    let start = who_end(clause);
    let who = clause[..start]
        .iter()
        .fold(0, |who, &letter| who | who_bits(letter));
    let affected = match who {
        0 => PERMISSION_BITS & !UMASK,
        who => who,
    };
    let mut rest = &clause[start..];
    while let Some((&operation, after)) = rest.split_first() {
        let end = operation_end(after);
        let bits = match &after[..end] {
            [letter @ (b'u' | b'g' | b'o')] => copied_bits(*letter, mode),
            permissions => permissions.iter().fold(0, |bits, &letter| {
                bits | permission_bits(letter, mode, directory)
            }),
        } & affected;
        mode = match operation {
            b'+' => mode | bits,
            b'-' => mode & !bits,
            _ => (mode & !affected) | bits,
        };
        rest = &after[end..];
    }
    mode
}

fn looks_like_mode(argument: &[u8]) -> bool {
    argument.len() > 1
        && argument != b"--"
        && argument[0] == b'-'
        && argument.iter().all(|byte| MODE_BYTES.contains(byte))
        && Mode::parse(argument).is_some()
}

fn octal_text(mode: u32) -> [u8; 4] {
    let digit = |shift: u32| b'0' + ((mode >> shift) & 0o7) as u8;
    [digit(9), digit(6), digit(3), digit(0)]
}

fn say_mode<S: System>(output: &mut Output<S>, mode: u32) {
    output.write(&octal_text(mode));
    output.write(b" (");
    output.write(&mode_string(mode)[1..]);
    output.write(b")");
}

fn change_with<S: System>(
    output: &mut Output<S>,
    mode: Mode<'_>,
    path: &CStr,
    status: &Status,
    verbose: bool,
) -> bool {
    let before = status.mode & PERMISSION_BITS;
    let after = mode.applied(before, status.is_directory()) & PERMISSION_BITS;
    if let Err(errno) = output.system.set_mode(path, after) {
        output.coded(
            error_for(errno),
            &[
                b"changing permissions of '",
                path.to_bytes(),
                b"': ",
                errno.message(),
            ],
        );
        return false;
    }
    if verbose {
        output.write(b"mode of '");
        output.write(path.to_bytes());
        match before == after {
            true => output.write(b"' retained as "),
            false => {
                output.write(b"' changed from ");
                say_mode(output, before);
                output.write(b" to ");
            }
        }
        say_mode(output, after);
        output.write(b"\n");
    }
    true
}

fn change_tree<S: System>(
    output: &mut Output<S>,
    mode: Mode<'_>,
    folder: &PathText,
    verbose: bool,
) -> bool {
    let descriptor = match output.system.open(folder.as_c_str()) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.coded(
                error_for(errno),
                &[
                    b"cannot read directory '",
                    folder.as_bytes(),
                    b"': ",
                    errno.message(),
                ],
            );
            return false;
        }
    };
    let mut changed = true;
    let mut chunk = [0u8; ENTRIES_CHUNK];
    loop {
        let length = match output.system.directory_entries(descriptor, &mut chunk) {
            Ok(0) => break,
            Ok(length) => length,
            Err(errno) => {
                output.file_error(folder.as_c_str(), errno);
                changed = false;
                break;
            }
        };
        for name in record_names(&chunk[..length]) {
            if matches!(name, b"." | b"..") {
                continue;
            }
            let child = PathText::of(folder.as_bytes()).and_then(|mut child| {
                if !child.as_bytes().ends_with(b"/") {
                    child.push(b"/")?;
                }
                child.push(name)?;
                Ok(child)
            });
            let child = match child {
                Ok(child) => child,
                Err(errno) => {
                    output.coded(
                        error_for(errno),
                        &[folder.as_bytes(), b": ", errno.message()],
                    );
                    changed = false;
                    continue;
                }
            };
            let found = output
                .system
                .status(None, child.as_c_str(), SymbolicLinks::DoNotFollow);
            let status = match found {
                Ok(status) if status.is_link() => continue,
                Ok(status) => status,
                Err(errno) => {
                    cannot_access(output, child.as_c_str(), errno);
                    changed = false;
                    continue;
                }
            };
            changed &= change_with(output, mode, child.as_c_str(), &status, verbose);
            if status.is_directory() {
                changed &= change_tree(output, mode, &child, verbose);
            }
        }
    }
    output.system.close(descriptor);
    changed
}

fn cannot_access<S: System>(output: &mut Output<S>, path: &CStr, errno: Errno) {
    output.coded(
        error_for(errno),
        &[b"cannot access '", path.to_bytes(), b"': ", errno.message()],
    );
}

pub fn chmod<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut kept = [c""; MAX_ARGUMENTS];
    let mut count = 0;
    let mut dashed_mode = None;
    let mut options_end = false;
    for &argument in arguments {
        let bytes = argument.to_bytes();
        if !options_end && dashed_mode.is_none() && looks_like_mode(bytes) {
            dashed_mode = Some(argument);
            continue;
        }
        options_end |= bytes == b"--";
        let Some(slot) = kept.get_mut(count) else {
            output.coded(&errors::USAGE, &[b"too many operands"]);
            return FAILURE;
        };
        *slot = argument;
        count += 1;
    }
    let (mut recursive, mut verbose) = (false, false);
    let Ok(operands) = parse_defined(output, &kept[..count], &commands::CHMOD, |_, given| {
        match given.option.short {
            Some(b'R') => recursive = true,
            _ => verbose = true,
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    let (mode_text, files) = match (dashed_mode, operands.all()) {
        (Some(mode), files) => (mode, files),
        (None, [mode, files @ ..]) => (*mode, files),
        (None, []) => {
            output.coded(&errors::USAGE, &[b"missing operand"]);
            return FAILURE;
        }
    };
    if files.is_empty() {
        output.coded(
            &errors::USAGE,
            &[b"missing operand after '", mode_text.to_bytes(), b"'"],
        );
        return FAILURE;
    }
    let Some(mode) = Mode::parse(mode_text.to_bytes()) else {
        output.coded(
            &errors::USAGE,
            &[b"invalid mode: '", mode_text.to_bytes(), b"'"],
        );
        return FAILURE;
    };
    let mut status = 0;
    for &path in files {
        let found = output.system.status(None, path, SymbolicLinks::Follow);
        let changed = match found {
            Ok(found) => change_with(output, mode, path, &found, verbose),
            Err(errno) => {
                cannot_access(output, path, errno);
                false
            }
        };
        let walks = recursive
            && output
                .system
                .status(None, path, SymbolicLinks::DoNotFollow)
                .is_ok_and(|own| own.is_directory());
        let walked = match (walks, PathText::of(path.to_bytes())) {
            (false, _) => true,
            (true, Ok(folder)) => change_tree(output, mode, &folder, verbose),
            (true, Err(errno)) => {
                output.file_error(path, errno);
                false
            }
        };
        if !changed || !walked {
            status = FAILURE;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn applied(mode: &str, before: u32, directory: bool) -> u32 {
        Mode::parse(mode.as_bytes())
            .unwrap_or_else(|| panic!("{mode}"))
            .applied(before, directory)
    }

    #[test]
    fn modes_are_octal_or_gnu_s_symbolic_clauses() {
        for (mode, before, after) in [
            ("755", 0o644, 0o755),
            ("0", 0o777, 0),
            ("4755", 0o644, 0o4755),
            ("u+x", 0o644, 0o744),
            ("go-w", 0o666, 0o644),
            ("a=r", 0o777, 0o444),
            ("u=rwx,g=rx,o=", 0o000, 0o750),
            ("+x", 0o644, 0o755),
            ("-w", 0o666, 0o466),
            ("=r", 0o666, 0o466),
            ("u+s", 0o755, 0o4755),
            ("g+s,o+t", 0o755, 0o3755),
            ("+t", 0o755, 0o1755),
            ("u-x+w", 0o544, 0o644),
            ("g=u", 0o640, 0o660),
            ("o=g", 0o750, 0o755),
            ("a+X", 0o644, 0o644),
            ("a+X", 0o744, 0o755),
            ("u+", 0o600, 0o600),
        ] {
            assert_eq!(applied(mode, before, false), after, "{mode} on {before:o}");
        }
        assert_eq!(applied("a+X", 0o644, true), 0o755);
        for bad in [
            "", "8", "75555", "u", "x", "u+q", "u+ug", ",u+x", "u+x,", "a+w=k",
        ] {
            assert_eq!(Mode::parse(bad.as_bytes()), None, "{bad}");
        }
        assert!(looks_like_mode(b"-w") && looks_like_mode(b"-x,u+r"));
        for option in [&b"-R"[..], b"-v", b"-", b"--"] {
            assert!(!looks_like_mode(option));
        }
        assert_eq!(&octal_text(0o4755), b"4755");
    }

    fn tree() -> Fake {
        Fake::default()
            .with_directory("/home", 0)
            .with_directory("/home/d", 0)
            .with_directory("/home/d/inner", 0)
            .with_file("/home/d/a.sh", 0o644, b"#!/bin/eggsh\n", 0)
            .with_file("/home/d/inner/b", 0o600, b"b", 0)
            .with_file("/home/elsewhere", 0o644, b"e", 0)
            .with_link("/home/d/outside", "/home/elsewhere")
            .with_link("/home/to-d", "d")
    }

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        let mut output = Output::new(fake, b"chmod");
        let status = chmod(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn mode_of(fake: &mut Fake, path: &CStr) -> u32 {
        let found = fake.status(None, path, SymbolicLinks::DoNotFollow);
        found.unwrap().mode & PERMISSION_BITS
    }

    #[test]
    fn a_script_is_made_executable_and_verbose_says_how() {
        let (status, mut fake) = run(tree(), &[c"-v", c"+x", c"/home/d/a.sh"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(mode_of(&mut fake, c"/home/d/a.sh"), 0o755);
        assert_eq!(
            fake.output(),
            "mode of '/home/d/a.sh' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)\n"
        );
        fake.standard_output.clear();
        let (_, fake) = run(fake, &[c"-v", c"755", c"/home/d/a.sh"]);
        assert_eq!(
            fake.output(),
            "mode of '/home/d/a.sh' retained as 0755 (rwxr-xr-x)\n"
        );
        let (status, mut fake) = run(tree(), &[c"-w", c"/home/d/a.sh"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(mode_of(&mut fake, c"/home/d/a.sh"), 0o444);
    }

    #[test]
    fn a_link_operand_is_followed_and_links_inside_a_walk_are_left_alone() {
        let (status, mut fake) = run(tree(), &[c"600", c"/home/d/outside"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(mode_of(&mut fake, c"/home/elsewhere"), 0o600);
        let (status, mut fake) = run(fake, &[c"-R", c"go+rX", c"/home/d"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(mode_of(&mut fake, c"/home/d"), 0o755);
        assert_eq!(mode_of(&mut fake, c"/home/d/a.sh"), 0o644);
        assert_eq!(mode_of(&mut fake, c"/home/d/inner/b"), 0o644);
        assert_eq!(mode_of(&mut fake, c"/home/elsewhere"), 0o600);
        assert_eq!(mode_of(&mut fake, c"/home/d/outside"), 0o777);
        let (status, mut fake) = run(fake, &[c"-R", c"700", c"/home/to-d"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(mode_of(&mut fake, c"/home/d"), 0o700);
        assert_eq!(mode_of(&mut fake, c"/home/d/inner/b"), 0o644);
    }

    #[test]
    fn bad_modes_missing_operands_and_files_are_reported() {
        for (arguments, message) in [
            (&[][..], "chmod: missing operand\n"),
            (&[c"755"][..], "chmod: missing operand after '755'\n"),
            (&[c"u+q", c"/home/d"][..], "chmod: invalid mode: 'u+q'\n"),
            (
                &[c"755", c"/home/none"][..],
                "chmod: cannot access '/home/none': No such file or directory\n",
            ),
        ] {
            let (status, fake) = run(tree(), arguments);
            assert_eq!((status, fake.errors().as_str()), (1, message));
        }
    }
}
