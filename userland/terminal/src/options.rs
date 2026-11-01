//! `terminal`'s command line (docs/ux/terminal-selection-v0.3.md section 2):
//! `terminal [-t <title>] [-e <program> [arguments...]]`.

/// What the command line asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options<'a, T> {
    /// The window's title text (`-t`).
    pub title: Option<T>,
    /// The program to run instead of the shell and its arguments (`-e` and the rest of the
    /// line), the program first.
    pub command: Option<&'a [T]>,
}

/// Why a command line was refused; each exits with status 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsError {
    /// `-e` with nothing after it.
    MissingProgram,
    /// `-t` with nothing after it, or a word the program does not know.
    Usage,
}

impl OptionsError {
    /// What the program writes to standard error.
    pub fn message(self) -> &'static str {
        match self {
            OptionsError::MissingProgram => "terminal: -e needs a program\n",
            OptionsError::Usage => "usage: terminal [-t title] [-e program [arguments...]]\n",
        }
    }
}

/// Reads `arguments` (the program's name left out), each viewed as bytes by `bytes`.
pub fn parse<'a, T: Copy>(
    arguments: &'a [T],
    bytes: impl Fn(T) -> &'a [u8],
) -> Result<Options<'a, T>, OptionsError> {
    let mut options = Options {
        title: None,
        command: None,
    };
    let mut index = 0;
    while index < arguments.len() {
        match bytes(arguments[index]) {
            b"-t" => {
                let title = arguments.get(index + 1).ok_or(OptionsError::Usage)?;
                options.title = Some(*title);
                index += 2;
            }
            b"-e" => {
                let rest = &arguments[index + 1..];
                if rest.is_empty() {
                    return Err(OptionsError::MissingProgram);
                }
                options.command = Some(rest);
                return Ok(options);
            }
            _ => return Err(OptionsError::Usage),
        }
    }
    Ok(options)
}

/// The title text of a window running `program` with no `-t`: its last path component.
pub fn program_title(program: &[u8]) -> &[u8] {
    match program.iter().rposition(|&byte| byte == b'/') {
        Some(slash) => &program[slash + 1..],
        None => program,
    }
}
