//! Init's configuration (D106): `/etc/eggos/init.conf`, read once at start, lines of
//! `key=value`. `login=` names the login program and its arguments, separated by spaces
//! (default [`DEFAULT_LOGIN`]); `login-exit=` says what init does when it ends: `respawn` (the
//! default), `poweroff`, `restart` or `halt`. Blank lines and unknown keys are skipped; a value
//! init cannot take keeps the default.

/// The login program init starts unless its configuration names another: the shell.
pub const DEFAULT_LOGIN: &[u8] = b"/bin/eggsh";

/// The most bytes the login command takes, its words' terminating NULs included.
pub const COMMAND_CAPACITY: usize = 256;

/// The most words the login command has, the program included.
pub const MAX_WORDS: usize = 8;

/// What init does when the login program ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginExit {
    /// Starts it again, after the back-off ([`crate::backoff`]).
    Respawn,
    /// Powers the machine off.
    PowerOff,
    /// Resets the machine.
    Restart,
    /// Halts the machine.
    Halt,
}

/// A command: words, each ended by a NUL, one after another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    bytes: [u8; COMMAND_CAPACITY],
    length: usize,
    words: usize,
}

impl Command {
    /// The command of the words of `text` (separated by spaces or tabs), `None` if it has none,
    /// more than [`MAX_WORDS`], a NUL, or needs more than [`COMMAND_CAPACITY`] bytes.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let mut command = Self {
            bytes: [0; COMMAND_CAPACITY],
            length: 0,
            words: 0,
        };
        if text.contains(&0) {
            return None;
        }
        for word in text
            .split(|byte| matches!(byte, b' ' | b'\t'))
            .filter(|word| !word.is_empty())
        {
            let end = command.length + word.len() + 1;
            if end > COMMAND_CAPACITY || command.words == MAX_WORDS {
                return None;
            }
            command.bytes[command.length..end - 1].copy_from_slice(word);
            command.length = end;
            command.words += 1;
        }
        (command.words > 0).then_some(command)
    }

    /// Its words, each with its terminating NUL; the program first.
    pub fn words(&self) -> impl Iterator<Item = &[u8]> {
        self.bytes[..self.length].split_inclusive(|&byte| byte == 0)
    }

    /// The program, with its terminating NUL.
    pub fn program(&self) -> &[u8] {
        self.words().next().unwrap_or(b"\0")
    }

    /// The program, without its terminating NUL.
    pub fn program_name(&self) -> &[u8] {
        let program = self.program();
        &program[..program.len() - 1]
    }
}

/// Init's configuration: see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The login program and its arguments.
    pub login: Command,
    /// What init does when the login program ends.
    pub login_exit: LoginExit,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            login: Command::parse(DEFAULT_LOGIN).unwrap_or(Command {
                bytes: [0; COMMAND_CAPACITY],
                length: 0,
                words: 0,
            }),
            login_exit: LoginExit::Respawn,
        }
    }
}

impl Config {
    /// The configuration `text` gives, over the defaults.
    pub fn parse(text: &[u8]) -> Self {
        let mut config = Self::default();
        for line in text.split(|&byte| byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(equals) = line.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            let (key, value) = (&line[..equals], &line[equals + 1..]);
            match key {
                b"login" => {
                    if let Some(login) = Command::parse(value) {
                        config.login = login;
                    }
                }
                b"login-exit" => {
                    config.login_exit = match value {
                        b"respawn" => LoginExit::Respawn,
                        b"poweroff" => LoginExit::PowerOff,
                        b"restart" => LoginExit::Restart,
                        b"halt" => LoginExit::Halt,
                        _ => config.login_exit,
                    }
                }
                _ => {}
            }
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_file_init_respawns_the_shell() {
        let config = Config::default();
        assert_eq!(config.login.program(), b"/bin/eggsh\0");
        assert_eq!(config.login.words().count(), 1);
        assert_eq!(config.login_exit, LoginExit::Respawn);
        assert_eq!(Config::parse(b""), config);
    }

    #[test]
    fn the_login_command_and_its_exit_action_are_read() {
        let config =
            Config::parse(b"login=/usr/libexec/test  init-login\tx\nlogin-exit=poweroff\n");
        let words: Vec<&[u8]> = config.login.words().collect();
        assert_eq!(
            words,
            [&b"/usr/libexec/test\0"[..], b"init-login\0", b"x\0"]
        );
        assert_eq!(config.login.program_name(), b"/usr/libexec/test");
        assert_eq!(config.login_exit, LoginExit::PowerOff);
        for (value, exit) in [
            (&b"restart"[..], LoginExit::Restart),
            (b"halt", LoginExit::Halt),
            (b"respawn", LoginExit::Respawn),
        ] {
            let text = [&b"login-exit="[..], value, b"\r\n"].concat();
            assert_eq!(Config::parse(&text).login_exit, exit);
        }
    }

    #[test]
    fn unknown_keys_and_values_init_cannot_take_keep_the_defaults() {
        let long = [b'a'; COMMAND_CAPACITY];
        let text = [
            &b"shell=/bin/sh\nlogin-exit=explode\nlogin=  \nnot a pair\nlogin="[..],
            &long,
            b"\nlogin=a b c d e f g h i\n",
        ]
        .concat();
        assert_eq!(Config::parse(&text), Config::default());
        assert_eq!(Command::parse(b"a\0b"), None);
    }

    #[test]
    fn a_command_fills_its_capacity_exactly() {
        let word = [b'w'; COMMAND_CAPACITY - 1];
        let command = Command::parse(&word).unwrap();
        assert_eq!(command.program().len(), COMMAND_CAPACITY);
        let words = b"a b c d e f g h";
        assert_eq!(Command::parse(words).unwrap().words().count(), MAX_WORDS);
    }
}
