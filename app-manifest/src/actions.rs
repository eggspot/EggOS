//! An app's own actions (D292, CLAUDE.md note 6): what an installed app offers people, scripts
//! and agents besides opening it, declared in the file `app.actions` in its folder. The
//! component manager and the MCP server publish each one as `app do <app> <action>` and as the
//! MCP tool `app_<app>_<action>`; uninstalling the app removes the file, and with it the
//! actions.
//!
//! The file is text: the line `eggos-actions 1`, then blocks that each start with
//! `action=<name>` and hold `key=value` lines: `title` and `description` (for people and
//! agents), `run` (the program, absolute or relative to the app's folder, then its words,
//! where `{argument}` in a word stands for an argument's value inside that word, which is left out
//! when an optional argument has none), `effect` (`read`,
//! `change`, the default, or `destroy`, which needs the user's yes) and up to
//! [`MAX_ARGUMENTS`] `argument=<name>:<type>:<required|optional>:<description>` lines, the type
//! `text`, `number` or `boolean`. A value is never split into words or read by a shell, so it
//! cannot add words of its own. Reading is tolerant as the manifest's is; nothing here
//! allocates.

/// The file's name inside an app's folder.
pub const FILE_NAME: &str = "app.actions";

/// The file's first line.
pub const HEADER: &[u8] = b"eggos-actions 1";

/// The longest actions file that is read; a longer one offers no actions.
pub const LARGEST: usize = 16 * 1024;

/// The longest action or argument name, in bytes.
pub const NAME_CAPACITY: usize = 32;

/// The most arguments an action takes; later `argument` lines are ignored.
pub const MAX_ARGUMENTS: usize = 8;

/// The most actions a file offers; later ones are ignored.
pub const MAX_ACTIONS: usize = 32;

/// The most words a `run` line expands to.
pub const MAX_WORDS: usize = 32;

/// What running an action can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only reads.
    Read,
    /// Changes something the app keeps.
    Change,
    /// Deletes or overwrites something: the user says yes first.
    Destroy,
}

impl Effect {
    fn named(value: &[u8]) -> Option<Self> {
        match value {
            b"read" => Some(Self::Read),
            b"change" => Some(Self::Change),
            b"destroy" => Some(Self::Destroy),
            _ => None,
        }
    }

    /// Its name in the file.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Change => "change",
            Self::Destroy => "destroy",
        }
    }
}

/// An argument's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentType {
    Text,
    Number,
    Boolean,
}

impl ArgumentType {
    fn named(value: &[u8]) -> Option<Self> {
        match value {
            b"text" => Some(Self::Text),
            b"number" => Some(Self::Number),
            b"boolean" => Some(Self::Boolean),
            _ => None,
        }
    }

    /// Its JSON schema type.
    pub const fn json_type(self) -> &'static str {
        match self {
            Self::Text => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }

    /// Whether `value` is one of this type, as its words carry it.
    pub fn accepts(self, value: &[u8]) -> bool {
        match self {
            Self::Text => !value.contains(&0),
            Self::Boolean => value == b"true" || value == b"false",
            Self::Number => {
                let digits = value.strip_prefix(b"-").unwrap_or(value);
                let mut parts = digits.splitn(2, |&byte| byte == b'.');
                let whole = parts.next().unwrap_or_default();
                let fraction = parts.next();
                !whole.is_empty()
                    && whole.iter().all(u8::is_ascii_digit)
                    && fraction.is_none_or(|fraction| !fraction.is_empty() && fraction.iter().all(u8::is_ascii_digit))
            }
        }
    }
}

/// One argument an action takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentSpec<'a> {
    pub name: &'a [u8],
    pub kind: ArgumentType,
    pub required: bool,
    pub description: &'a [u8],
}

/// One action of an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action<'a> {
    pub name: &'a [u8],
    pub title: &'a [u8],
    pub description: &'a [u8],
    pub run: &'a [u8],
    pub effect: Effect,
    arguments: [Option<ArgumentSpec<'a>>; MAX_ARGUMENTS],
}

impl<'a> Action<'a> {
    fn new(name: &'a [u8]) -> Self {
        Self {
            name,
            title: &[],
            description: &[],
            run: &[],
            effect: Effect::Change,
            arguments: [None; MAX_ARGUMENTS],
        }
    }

    /// Its arguments, in the file's order.
    pub fn arguments(&self) -> impl Iterator<Item = &ArgumentSpec<'a>> {
        self.arguments.iter().flatten()
    }

    fn is_valid(&self) -> bool {
        is_name(self.name) && !self.run.is_empty() && self.run.split(|&byte| byte == b' ').any(|word| !word.is_empty())
    }
}

/// Whether `name` is an action's or argument's name: 1 to [`NAME_CAPACITY`] lower-case letters,
/// digits and `-`, starting with a letter.
pub fn is_name(name: &[u8]) -> bool {
    (1..=NAME_CAPACITY).contains(&name.len())
        && name[0].is_ascii_lowercase()
        && name.iter().all(|&byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The actions `file` offers, in its order; nothing when it is not an actions file.
pub fn actions(file: &[u8]) -> impl Iterator<Item = Action<'_>> {
    let valid = file.len() <= LARGEST && file.split(|&byte| byte == b'\n').next().map(trim) == Some(HEADER);
    let mut lines = file.split(|&byte| byte == b'\n').skip(1).map(trim).peekable();
    let mut offered = 0;
    core::iter::from_fn(move || {
        if !valid {
            return None;
        }
        loop {
            if offered == MAX_ACTIONS {
                return None;
            }
            let line = lines.next()?;
            let Some(name) = line.strip_prefix(b"action=") else {
                continue;
            };
            let mut action = Action::new(name);
            let mut count = 0;
            while let Some(line) = lines.next_if(|line| !line.starts_with(b"action=")) {
                let Some(at) = line.iter().position(|&byte| byte == b'=') else {
                    continue;
                };
                let (key, value) = (&line[..at], &line[at + 1..]);
                match key {
                    b"title" => action.title = value,
                    b"description" => action.description = value,
                    b"run" => action.run = value,
                    b"effect" => action.effect = Effect::named(value).unwrap_or(action.effect),
                    b"argument" if count < MAX_ARGUMENTS => {
                        if let Some(argument) = argument(value) {
                            action.arguments[count] = Some(argument);
                            count += 1;
                        }
                    }
                    _ => {}
                }
            }
            if action.is_valid() {
                offered += 1;
                return Some(action);
            }
        }
    })
}

/// A word's `{name}`: the text before it, the name and the text after it.
fn placeholder(word: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let open = word.iter().position(|&byte| byte == b'{')?;
    let close = open + word[open..].iter().position(|&byte| byte == b'}')?;
    Some((&word[..open], &word[open + 1..close], &word[close + 1..]))
}

fn trim(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn argument(value: &[u8]) -> Option<ArgumentSpec<'_>> {
    let mut parts = value.splitn(4, |&byte| byte == b':');
    let name = parts.next()?;
    let kind = ArgumentType::named(parts.next()?)?;
    let required = match parts.next()? {
        b"required" => true,
        b"optional" => false,
        _ => return None,
    };
    let description = parts.next().unwrap_or_default();
    is_name(name).then_some(ArgumentSpec {
        name,
        kind,
        required,
        description,
    })
}

/// Why an action's command line could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandError {
    /// A required argument has no value.
    Missing,
    /// A value is not of its argument's type.
    WrongType,
    /// A value names no argument of the action.
    Unknown,
    /// The words do not fit the buffer, or are more than [`MAX_WORDS`].
    TooLong,
    /// `run` names `{x}` for an `x` that is no argument.
    BadPlaceholder,
}

/// The program and words `action` runs with `values` (each `(name, value)` once), written into
/// `out` NUL-terminated one after the other: how many words. A placeholder whose optional
/// argument has no value drops its word.
pub fn expand(
    action: &Action<'_>,
    values: &[(&[u8], &[u8])],
    out: &mut [u8],
) -> Result<usize, ExpandError> {
    for (name, value) in values {
        let argument = action.arguments().find(|argument| argument.name == *name).ok_or(ExpandError::Unknown)?;
        if !argument.kind.accepts(value) {
            return Err(ExpandError::WrongType);
        }
    }
    if action
        .arguments()
        .any(|argument| argument.required && !values.iter().any(|(name, _)| *name == argument.name))
    {
        return Err(ExpandError::Missing);
    }
    let mut length = 0;
    let mut words = 0;
    for word in action.run.split(|&byte| byte == b' ').filter(|word| !word.is_empty()) {
        let (prefix, value, suffix) = match placeholder(word) {
            Some((prefix, name, suffix)) => {
                if !action.arguments().any(|argument| argument.name == name) {
                    return Err(ExpandError::BadPlaceholder);
                }
                match values.iter().find(|(given, _)| *given == name) {
                    Some((_, value)) => (prefix, *value, suffix),
                    None => continue,
                }
            }
            None => (word, &[][..], &[][..]),
        };
        let end = length + prefix.len() + value.len() + suffix.len() + 1;
        if end > out.len() || words == MAX_WORDS {
            return Err(ExpandError::TooLong);
        }
        let mut at = length;
        for part in [prefix, value, suffix] {
            out[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        out[at] = 0;
        length = end;
        words += 1;
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &[u8] = b"eggos-actions 1\n\
        action=export\n\
        title=Export a drawing\n\
        description=Writes the drawing as a PNG.\n\
        run=bin/paint --export {file} --scale={scale}\n\
        argument=file:text:required:Where to write it.\n\
        argument=scale:number:optional:How many times larger.\n\
        effect=change\n\
        action=clear\n\
        run=bin/paint --clear\n\
        effect=destroy\n\
        action=Bad Name\n\
        run=x\n";

    #[test]
    fn a_file_offers_its_valid_actions_in_order() {
        let found: Vec<_> = actions(FILE).collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, b"export");
        assert_eq!(found[0].arguments().count(), 2);
        assert_eq!(found[1].effect, Effect::Destroy);
        assert_eq!(actions(b"eggos-app 1\naction=x\nrun=y\n").count(), 0);
    }

    #[test]
    fn values_fill_whole_words_and_are_checked() {
        let export = actions(FILE).next().unwrap();
        let mut out = [0u8; 128];
        let words = expand(&export, &[(b"file", b"/home/a b.png")], &mut out).unwrap();
        assert_eq!(words, 3);
        assert_eq!(&out[..33], b"bin/paint\0--export\0/home/a b.png\0");
        let words = expand(&export, &[(b"file", b"x"), (b"scale", b"2")], &mut out).unwrap();
        assert_eq!((words, &out[..30]), (4, &b"bin/paint\0--export\0x\0--scale=2\0"[..]));
        assert_eq!(expand(&export, &[], &mut out), Err(ExpandError::Missing));
        assert_eq!(expand(&export, &[(b"file", b"x"), (b"scale", b"two")], &mut out), Err(ExpandError::WrongType));
        assert_eq!(expand(&export, &[(b"file", b"x"), (b"other", b"1")], &mut out), Err(ExpandError::Unknown));
        assert!(ArgumentType::Number.accepts(b"-2.5"));
        assert!(!ArgumentType::Number.accepts(b"2."));
    }
}
