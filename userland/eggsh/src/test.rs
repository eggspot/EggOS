//! `test` and `[` (D182), as POSIX describes them: one to four arguments are read by their
//! count (so `[ "!" = "!" ]` compares two strings), more by precedence, `-o` below `-a` below
//! `!`, with `(` and `)` for grouping. Strings: `-n`, `-z`, `=` (or `==`), `!=`, `<`, `>`;
//! integers: `-eq`, `-ne`, `-lt`, `-le`, `-gt`, `-ge`; files: `-e`, `-f`, `-d`, `-h` and `-L`,
//! `-b`, `-c`, `-p`, `-S`, `-s`, `-r`, `-w`, `-x`, `-u`, `-g`, `-k`, and `-t` for a terminal.

/// What a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// A regular file.
    Regular,
    /// A directory.
    Directory,
    /// A symbolic link.
    SymbolicLink,
    /// A character device.
    CharacterDevice,
    /// A block device.
    BlockDevice,
    /// A pipe.
    Fifo,
    /// A socket.
    Socket,
}

/// What `test` needs to know of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStatus {
    /// What it is.
    pub file_type: FileType,
    /// Its size in bytes.
    pub size: u64,
    /// Its mode's permission bits, set-user-id, set-group-id and sticky included.
    pub mode: u32,
}

/// What a process may do with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read it.
    Read,
    /// Write it.
    Write,
    /// Execute it, or search it if it is a directory.
    Execute,
}

/// Where `test` learns about files.
pub trait FileTests {
    /// The status of the file at `path`, of a symbolic link itself unless `follow`.
    fn status(&mut self, path: &[u8], follow: bool) -> Option<FileStatus>;
    /// Whether the shell may `access` the file at `path`.
    fn accessible(&mut self, path: &[u8], access: Access) -> bool;
    /// Whether `descriptor` is a terminal.
    fn is_terminal(&mut self, descriptor: usize) -> bool;
}

/// Why `test` could not tell: its status is 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Argument `index` should be an integer.
    NotAnInteger(usize),
    /// An operator should come at argument `index`.
    OperatorExpected(usize),
    /// The arguments ended where one more was needed.
    ArgumentExpected,
    /// A `(` has no `)`.
    CloseExpected,
    /// Argument `index` is one too many.
    TooMany(usize),
}

const SETUID: u32 = 0o4000;
const SETGID: u32 = 0o2000;
const STICKY: u32 = 0o1000;

/// Evaluates `arguments` (without `test` or the `[` and `]`).
pub fn evaluate(arguments: &[&[u8]], files: &mut impl FileTests) -> Result<bool, Problem> {
    let mut parser = Parser {
        arguments,
        at: 0,
        files,
    };
    match arguments.len() {
        0 => Ok(false),
        1 => Ok(!arguments[0].is_empty()),
        2 => parser.two(0),
        3 => parser.three(0),
        4 if arguments[0] == b"!" => parser.three(1).map(|value| !value),
        4 if arguments[0] == b"(" && arguments[3] == b")" => parser.two(1),
        _ => parser.whole(),
    }
}

fn is_unary(operator: &[u8]) -> bool {
    matches!(
        operator,
        b"-n"
            | b"-z"
            | b"-e"
            | b"-f"
            | b"-d"
            | b"-h"
            | b"-L"
            | b"-b"
            | b"-c"
            | b"-p"
            | b"-S"
            | b"-s"
            | b"-r"
            | b"-w"
            | b"-x"
            | b"-u"
            | b"-g"
            | b"-k"
            | b"-t"
    )
}

fn is_binary(operator: &[u8]) -> bool {
    matches!(
        operator,
        b"=" | b"==" | b"!=" | b"<" | b">" | b"-eq" | b"-ne" | b"-lt" | b"-le" | b"-gt" | b"-ge"
    )
}

fn integer(text: &[u8]) -> Option<i64> {
    let text = text.trim_ascii();
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits.iter().try_fold(0i64, |value, &digit| {
        value.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
    })?;
    Some(if negative { -value } else { value })
}

struct Parser<'a, 'f, F> {
    arguments: &'a [&'a [u8]],
    at: usize,
    files: &'f mut F,
}

impl<'a, F: FileTests> Parser<'a, '_, F> {
    fn two(&mut self, at: usize) -> Result<bool, Problem> {
        let (first, second) = (self.arguments[at], self.arguments[at + 1]);
        if first == b"!" {
            return Ok(second.is_empty());
        }
        if is_unary(first) {
            return Ok(self.unary(first, second));
        }
        Err(Problem::OperatorExpected(at))
    }

    fn three(&mut self, at: usize) -> Result<bool, Problem> {
        let operator = self.arguments[at + 1];
        if is_binary(operator) {
            return self.binary(at);
        }
        if self.arguments[at] == b"!" {
            return self.two(at + 1).map(|value| !value);
        }
        if self.arguments[at] == b"(" && self.arguments[at + 2] == b")" {
            return Ok(!operator.is_empty());
        }
        if operator == b"-a" || operator == b"-o" {
            let (left, right) = (
                !self.arguments[at].is_empty(),
                !self.arguments[at + 2].is_empty(),
            );
            return Ok(if operator == b"-a" {
                left && right
            } else {
                left || right
            });
        }
        Err(Problem::OperatorExpected(at + 1))
    }

    fn whole(&mut self) -> Result<bool, Problem> {
        let value = self.or()?;
        match self.at < self.arguments.len() {
            true => Err(Problem::TooMany(self.at)),
            false => Ok(value),
        }
    }

    fn peek(&self) -> Option<&'a [u8]> {
        self.arguments.get(self.at).copied()
    }

    fn next(&mut self) -> Result<&'a [u8], Problem> {
        let argument = self.peek().ok_or(Problem::ArgumentExpected)?;
        self.at += 1;
        Ok(argument)
    }

    fn or(&mut self) -> Result<bool, Problem> {
        let mut value = self.and()?;
        while self.peek() == Some(&b"-o"[..]) {
            self.at += 1;
            let right = self.and()?;
            value = value || right;
        }
        Ok(value)
    }

    fn and(&mut self) -> Result<bool, Problem> {
        let mut value = self.not()?;
        while self.peek() == Some(&b"-a"[..]) {
            self.at += 1;
            let right = self.not()?;
            value = value && right;
        }
        Ok(value)
    }

    fn not(&mut self) -> Result<bool, Problem> {
        if self.peek() == Some(&b"!"[..]) && self.at + 1 < self.arguments.len() {
            self.at += 1;
            return self.not().map(|value| !value);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<bool, Problem> {
        let first = self.next()?;
        if first == b"(" && self.at < self.arguments.len() {
            let value = self.or()?;
            return match self.next() {
                Ok(b")") => Ok(value),
                _ => Err(Problem::CloseExpected),
            };
        }
        if is_unary(first) && self.at < self.arguments.len() {
            let operand = self.next()?;
            return Ok(self.unary(first, operand));
        }
        if self.peek().is_some_and(is_binary) && self.at + 1 < self.arguments.len() {
            let value = self.binary(self.at - 1)?;
            self.at += 2;
            return Ok(value);
        }
        Ok(!first.is_empty())
    }

    fn binary(&mut self, at: usize) -> Result<bool, Problem> {
        let (left, operator, right) = (
            self.arguments[at],
            self.arguments[at + 1],
            self.arguments[at + 2],
        );
        match operator {
            b"=" | b"==" => return Ok(left == right),
            b"!=" => return Ok(left != right),
            b"<" => return Ok(left < right),
            b">" => return Ok(left > right),
            _ => {}
        }
        let left = integer(left).ok_or(Problem::NotAnInteger(at))?;
        let right = integer(right).ok_or(Problem::NotAnInteger(at + 2))?;
        Ok(match operator {
            b"-eq" => left == right,
            b"-ne" => left != right,
            b"-lt" => left < right,
            b"-le" => left <= right,
            b"-gt" => left > right,
            _ => left >= right,
        })
    }

    fn unary(&mut self, operator: &[u8], operand: &[u8]) -> bool {
        let follow = !matches!(operator, b"-h" | b"-L");
        match operator {
            b"-n" => !operand.is_empty(),
            b"-z" => operand.is_empty(),
            b"-r" => self.files.accessible(operand, Access::Read),
            b"-w" => self.files.accessible(operand, Access::Write),
            b"-x" => self.files.accessible(operand, Access::Execute),
            b"-t" => integer(operand)
                .and_then(|descriptor| usize::try_from(descriptor).ok())
                .is_some_and(|descriptor| self.files.is_terminal(descriptor)),
            _ => {
                let Some(status) = self.files.status(operand, follow) else {
                    return false;
                };
                match operator {
                    b"-f" => status.file_type == FileType::Regular,
                    b"-d" => status.file_type == FileType::Directory,
                    b"-h" | b"-L" => status.file_type == FileType::SymbolicLink,
                    b"-b" => status.file_type == FileType::BlockDevice,
                    b"-c" => status.file_type == FileType::CharacterDevice,
                    b"-p" => status.file_type == FileType::Fifo,
                    b"-S" => status.file_type == FileType::Socket,
                    b"-s" => status.size > 0,
                    b"-u" => status.mode & SETUID != 0,
                    b"-g" => status.mode & SETGID != 0,
                    b"-k" => status.mode & STICKY != 0,
                    _ => true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl FileTests for Fake {
        fn status(&mut self, path: &[u8], follow: bool) -> Option<FileStatus> {
            let (file_type, size) = match path {
                b"/file" => (FileType::Regular, 3),
                b"/empty" => (FileType::Regular, 0),
                b"/dir" => (FileType::Directory, 0),
                b"/link" if follow => (FileType::Regular, 3),
                b"/link" => (FileType::SymbolicLink, 5),
                b"/char" => (FileType::CharacterDevice, 0),
                b"/block" => (FileType::BlockDevice, 0),
                b"/fifo" => (FileType::Fifo, 0),
                b"/socket" => (FileType::Socket, 0),
                _ => return None,
            };
            Some(FileStatus {
                file_type,
                size,
                mode: if path == b"/dir" { 0o1777 } else { 0o644 },
            })
        }

        fn accessible(&mut self, path: &[u8], access: Access) -> bool {
            path == b"/file" && access != Access::Execute
        }

        fn is_terminal(&mut self, descriptor: usize) -> bool {
            descriptor == 0
        }
    }

    fn test(arguments: &str) -> Result<bool, Problem> {
        let words: Vec<&[u8]> = arguments
            .split(' ')
            .filter(|word| !word.is_empty())
            .map(|word| {
                if word == "''" {
                    &b""[..]
                } else {
                    word.as_bytes()
                }
            })
            .collect();
        evaluate(&words, &mut Fake)
    }

    #[test]
    fn the_count_of_arguments_decides_how_they_are_read() {
        assert_eq!(test(""), Ok(false));
        assert_eq!(test("x"), Ok(true));
        assert_eq!(test("''"), Ok(false));
        assert_eq!(test("! x"), Ok(false));
        assert_eq!(test("! = !"), Ok(true));
        assert_eq!(test("-n -n"), Ok(true));
        assert_eq!(test("( -z )"), Ok(true));
        assert_eq!(test("! a = b"), Ok(true));
        assert_eq!(test("a b"), Err(Problem::OperatorExpected(0)));
    }

    #[test]
    fn strings_and_integers_compare() {
        assert_eq!(test("abc = abc"), Ok(true));
        assert_eq!(test("abc != abc"), Ok(false));
        assert_eq!(test("-z ''"), Ok(true));
        assert_eq!(test("10 -gt 9"), Ok(true));
        assert_eq!(test("-3 -lt -2"), Ok(true));
        assert_eq!(test("7 -eq 007"), Ok(true));
        assert_eq!(test("a -eq 1"), Err(Problem::NotAnInteger(0)));
        assert_eq!(test("a < b"), Ok(true));
    }

    #[test]
    fn files_are_tested_by_type_size_and_access() {
        assert_eq!(test("-f /file"), Ok(true));
        assert_eq!(test("-d /file"), Ok(false));
        assert_eq!(test("-d /dir"), Ok(true));
        assert_eq!(test("-e /missing"), Ok(false));
        assert_eq!(test("-s /file"), Ok(true));
        assert_eq!(test("-s /empty"), Ok(false));
        assert_eq!(test("-L /link"), Ok(true));
        assert_eq!(test("-f /link"), Ok(true));
        assert_eq!(test("-r /file"), Ok(true));
        assert_eq!(test("-x /file"), Ok(false));
        assert_eq!(test("-k /dir"), Ok(true));
        assert_eq!(test("-t 0"), Ok(true));
        assert_eq!(test("-t x"), Ok(false));
        assert_eq!(test("-c /char"), Ok(true));
        assert_eq!(test("-b /block"), Ok(true));
        assert_eq!(test("-p /fifo"), Ok(true));
        assert_eq!(test("-S /socket"), Ok(true));
        assert_eq!(test("-S /fifo"), Ok(false));
    }

    #[test]
    fn more_arguments_are_read_by_precedence() {
        assert_eq!(test("-f /file -a -d /dir"), Ok(true));
        assert_eq!(test("-f /missing -o -d /dir"), Ok(true));
        assert_eq!(test("x -o '' -a ''"), Ok(true));
        assert_eq!(test("! -f /missing -a 1 -eq 1"), Ok(true));
        assert_eq!(test("( a = b -o b = b ) -a x"), Ok(true));
        assert_eq!(test("( a = b"), Err(Problem::CloseExpected));
        assert_eq!(test("a = a b c"), Err(Problem::TooMany(3)));
        assert_eq!(test("-f /file -a"), Err(Problem::OperatorExpected(1)));
    }
}
