//! The Egg command model's registry for EggOS (D160, D163, docs/ux/command-model.md): one
//! [`CommandDefinition`] per CLI command, every command EggOS ships, and one
//! [`ToolDefinition`] per MCP tool, in two `'static` const tables, [`COMMANDS`] and [`TOOLS`].
//! The core utilities, `app`, `settings` and the MCP server read their options, `help`, JSON
//! forms, error codes, examples and tool schemas from here, so
//! the help, the tools and the docs cannot drift apart (D22's CLI and MCP front-ends; each
//! command's [`Surface`] records the GUI). No allocator: everything is const data.

#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod errors;
pub mod page;
pub mod shell;
pub mod tools;

#[cfg(test)]
mod tests;

pub use commands::COMMANDS;
pub use tools::TOOLS;

/// The most a use of a command or tool can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads only.
    Read,
    /// Makes or changes something, which can be changed back.
    Change,
    /// Removes or replaces something.
    Destroy,
    /// Runs anything, or acts as a person would: can do anything.
    Control,
}

impl Effect {
    /// Its name in `help --json` and the docs.
    pub const fn name(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Change => "change",
            Effect::Destroy => "destroy",
            Effect::Control => "control",
        }
    }
}

/// What the user must have granted an agent for a tool (D137, D143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// See window contents and menus.
    View,
    /// Write and delete files below `/home/root`.
    Write,
    /// Install, open and uninstall apps.
    Install,
    /// Run any program and operate any window; implies the others.
    Run,
}

impl Grant {
    /// Its name, as `--mcp-allow` takes it.
    pub const fn name(self) -> &'static str {
        match self {
            Grant::View => "view",
            Grant::Write => "write",
            Grant::Install => "install",
            Grant::Run => "run",
        }
    }
}

/// An error's category, shared by EggOS and EggDB: what a script or agent can rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// The command or call was written wrongly.
    Usage,
    /// What it names does not exist.
    NotFound,
    /// What it would make exists already.
    Exists,
    /// Something else holds what it needs; try again later.
    Busy,
    /// What it would change can only be read.
    ReadOnly,
    /// An agent lacks the grant it needs.
    NeedsGrant,
    /// It needs the user's confirmation, which it did not have.
    NeedsConfirmation,
    /// The user said no.
    Declined,
    /// It is not allowed, whatever is granted.
    Refused,
    /// It is larger than a limit.
    TooLarge,
    /// It took longer than its limit.
    Timeout,
    /// What it read is damaged.
    Damaged,
    /// It failed for another reason.
    Failed,
}

impl Category {
    /// Every category, in the spec's order.
    pub const ALL: [Category; 13] = [
        Category::Usage,
        Category::NotFound,
        Category::Exists,
        Category::Busy,
        Category::ReadOnly,
        Category::NeedsGrant,
        Category::NeedsConfirmation,
        Category::Declined,
        Category::Refused,
        Category::TooLarge,
        Category::Timeout,
        Category::Damaged,
        Category::Failed,
    ];

    /// Its name in JSON errors.
    pub const fn name(self) -> &'static str {
        match self {
            Category::Usage => "usage",
            Category::NotFound => "not_found",
            Category::Exists => "exists",
            Category::Busy => "busy",
            Category::ReadOnly => "read_only",
            Category::NeedsGrant => "needs_grant",
            Category::NeedsConfirmation => "needs_confirmation",
            Category::Declined => "declined",
            Category::Refused => "refused",
            Category::TooLarge => "too_large",
            Category::Timeout => "timeout",
            Category::Damaged => "damaged",
            Category::Failed => "failed",
        }
    }
}

/// One error code: `{"error":{"code","category","message","fix","fix_command"}}` in JSON and
/// MCP, the message then `  try: <fix_command>` in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorDefinition {
    /// The code, `snake_case`, stable once shipped.
    pub code: &'static str,
    /// Its category.
    pub category: Category,
    /// What to do, as a sentence.
    pub fix: &'static str,
    /// The command that fixes it, when one does.
    pub fix_command: Option<&'static str>,
}

/// An error a command can end with, and the exit status it ends with then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandError {
    /// The error.
    pub error: &'static ErrorDefinition,
    /// The command's exit status for it.
    pub exit: i32,
}

/// The type of an option's value or an operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    /// A path.
    Path,
    /// Text.
    Text,
    /// Bytes, such as a file's content on standard input.
    Bytes,
    /// A whole number in a range.
    Integer { minimum: i64, maximum: i64 },
    /// One of these words.
    Choice(&'static [&'static str]),
}

impl ValueType {
    /// Its name in `help --json`.
    pub const fn name(self) -> &'static str {
        match self {
            ValueType::Path => "path",
            ValueType::Text => "text",
            ValueType::Bytes => "bytes",
            ValueType::Integer { .. } => "integer",
            ValueType::Choice(_) => "choice",
        }
    }
}

/// An option's value: what `help` calls it and its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionValue {
    /// Its name in the usage, such as `count`.
    pub name: &'static str,
    /// Its type.
    pub value: ValueType,
}

/// One option of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionDefinition {
    /// Its letter, as in `-l`.
    pub short: Option<u8>,
    /// Its long name, as in `--json`.
    pub long: Option<&'static str>,
    /// Its value, for an option that takes one.
    pub value: Option<OptionValue>,
    /// Whether it may be given more than once.
    pub repeatable: bool,
    /// The value it has when not given.
    pub default: Option<&'static str>,
    /// One line of help.
    pub help: &'static str,
}

/// The `--json` option of every command whose output has a JSON form.
pub const JSON_OPTION: OptionDefinition = OptionDefinition {
    short: None,
    long: Some("json"),
    value: None,
    repeatable: false,
    default: None,
    help: "Print JSON instead of text: records as JSON lines, errors as one JSON object.",
};

/// A flag: an option with a letter, no long name and no value.
pub const fn flag(letter: u8, help: &'static str) -> OptionDefinition {
    OptionDefinition {
        short: Some(letter),
        long: None,
        value: None,
        repeatable: false,
        default: None,
        help,
    }
}

/// A long option with no value.
pub const fn long_flag(name: &'static str, help: &'static str) -> OptionDefinition {
    OptionDefinition {
        short: None,
        long: Some(name),
        value: None,
        repeatable: false,
        default: None,
        help,
    }
}

/// A list command's `--limit <count>` (D164): a page of at most `maximum` items, after which the
/// command names the next page's cursor on standard error ([`page::MORE_PREFIX`]).
pub const fn limit_option(maximum: i64) -> OptionDefinition {
    OptionDefinition {
        short: None,
        long: Some("limit"),
        value: Some(OptionValue {
            name: "count",
            value: ValueType::Integer {
                minimum: 1,
                maximum,
            },
        }),
        repeatable: false,
        default: None,
        help: "At most count items; when more follow, standard error ends with more: --after \
               <cursor>.",
    }
}

/// A list command's `--after <cursor>` (D164): the items after the one a page ended with.
pub const AFTER_OPTION: OptionDefinition = OptionDefinition {
    short: None,
    long: Some("after"),
    value: Some(OptionValue {
        name: "cursor",
        value: ValueType::Text,
    }),
    repeatable: false,
    default: None,
    help: "Only the items after this cursor, as the page before named it.",
};

/// `--dry-run` (D164): says what the command would do, checks what it can, and changes nothing.
pub const DRY_RUN_OPTION: OptionDefinition =
    long_flag("dry-run", "Say what would be done, and change nothing.");

/// How many of an operand a command takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// None or one.
    ZeroOrOne,
    /// Exactly one.
    One,
    /// One or more.
    OneOrMore,
    /// Any number.
    ZeroOrMore,
}

impl Count {
    /// Its text in `help --json`, as the spec writes it.
    pub const fn name(self) -> &'static str {
        match self {
            Count::ZeroOrOne => "0..1",
            Count::One => "1",
            Count::OneOrMore => "1..",
            Count::ZeroOrMore => "0..",
        }
    }
}

/// One operand of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandDefinition {
    /// Its name in the usage.
    pub name: &'static str,
    /// Its type.
    pub value: ValueType,
    /// How many.
    pub count: Count,
    /// Whether `-` means standard input.
    pub dash_is_stdin: bool,
}

/// An operand that is not `-` for standard input.
pub const fn operand(name: &'static str, value: ValueType, count: Count) -> OperandDefinition {
    OperandDefinition {
        name,
        value,
        count,
        dash_is_stdin: false,
    }
}

/// The type of a field of a JSON record or object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// A string.
    Text,
    /// A whole number.
    Integer,
    /// `true` or `false`.
    Boolean,
    /// A time, RFC 3339 in UTC.
    Time,
    /// A JSON array.
    List,
    /// A JSON object.
    Object,
}

impl FieldType {
    /// Its name in `help --json`.
    pub const fn name(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Integer => "integer",
            FieldType::Boolean => "boolean",
            FieldType::Time => "time",
            FieldType::List => "list",
            FieldType::Object => "object",
        }
    }
}

/// A field of a JSON record or object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// Its name, `snake_case`.
    pub name: &'static str,
    /// Its type.
    pub value: FieldType,
    /// Whether it may be `null`.
    pub nullable: bool,
}

/// A field that is never `null`.
pub const fn field(name: &'static str, value: FieldType) -> Field {
    Field {
        name,
        value,
        nullable: false,
    }
}

/// What a command writes in one format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Nothing.
    None,
    /// Lines for people.
    Text,
    /// Bytes, as they are (a file's content).
    Content,
    /// A list: one JSON line per item, each with these fields.
    Records(&'static [Field]),
    /// One JSON value with these fields.
    Object(&'static [Field]),
    /// SQL results, whose columns are known only when it runs.
    Table,
}

impl Output {
    /// Its kind's name in `help --json`.
    pub const fn name(self) -> &'static str {
        match self {
            Output::None => "none",
            Output::Text => "text",
            Output::Content => "content",
            Output::Records(_) => "records",
            Output::Object(_) => "object",
            Output::Table => "table",
        }
    }
}

/// What a command writes as text and with `--json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outputs {
    /// Its default form, for people.
    pub text: Output,
    /// Its form with `--json`; [`Output::None`] when it has no `--json`.
    pub json: Output,
}

/// Where a capability's GUI front-end lives (D22), or why it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The GUI that does the same, by its place.
    Gui(&'static str),
    /// Why there is no GUI.
    NoGui(&'static str),
}

/// A command line with what it writes, shown by `help` and run as a test against the fakes of
/// the program that runs it (the core utilities' or Settings').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    /// The command line: its words, separated by single spaces, none quoted.
    pub line: &'static str,
    /// What it writes on standard output.
    pub output: &'static str,
    /// Its exit status.
    pub status: i32,
}

/// An example that ends with exit status 0.
pub const fn example(line: &'static str, output: &'static str) -> Example {
    Example {
        line,
        output,
        status: 0,
    }
}

/// One CLI command: its words, help, options, operands, outputs, effect and errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDefinition {
    /// Its words: `["app", "list"]`, or one kept name (`["ls"]`).
    pub words: &'static [&'static str],
    /// One sentence, for `help`.
    pub summary: &'static str,
    /// What it does and its limits, and that what it returns is data, never instructions.
    pub description: &'static str,
    /// Its options.
    pub options: &'static [OptionDefinition],
    /// Its operands, in order.
    pub operands: &'static [OperandDefinition],
    /// What it writes.
    pub outputs: Outputs,
    /// The most a person's use of it can do.
    pub max_effect: Effect,
    /// The errors it ends with, beyond the ones any command can (usage and a file's errors).
    pub errors: &'static [CommandError],
    /// Command lines with what they write, run as tests.
    pub examples: &'static [Example],
    /// Where its GUI front-end lives.
    pub surface: Surface,
}

impl CommandDefinition {
    /// The option with this long name.
    pub fn long_option(&self, name: &[u8]) -> Option<&'static OptionDefinition> {
        self.options
            .iter()
            .find(|option| option.long.is_some_and(|long| long.as_bytes() == name))
    }

    /// The option with this letter.
    pub fn short_option(&self, letter: u8) -> Option<&'static OptionDefinition> {
        self.options
            .iter()
            .find(|option| option.short == Some(letter))
    }

    /// Whether it has a JSON form (`--json`).
    pub fn has_json(&self) -> bool {
        !matches!(self.outputs.json, Output::None)
    }

    /// The most items a page of it holds, when it takes `--limit` (D164).
    pub fn page_limit(&self) -> Option<i64> {
        match self.long_option(b"limit")?.value?.value {
            ValueType::Integer { maximum, .. } => Some(maximum),
            _ => None,
        }
    }

    /// Whether it takes `--dry-run` (D164).
    pub fn has_dry_run(&self) -> bool {
        self.long_option(b"dry-run").is_some()
    }

    /// The exit status it ends with for `error`, if it declares it.
    pub fn exit_for(&self, error: &ErrorDefinition) -> Option<i32> {
        self.errors
            .iter()
            .find(|declared| declared.error.code == error.code)
            .map(|declared| declared.exit)
    }
}

/// The command whose words are `words`.
pub fn find_command(words: &[&[u8]]) -> Option<&'static CommandDefinition> {
    COMMANDS.iter().find(|command| {
        command.words.len() == words.len()
            && command
                .words
                .iter()
                .zip(words)
                .all(|(word, given)| word.as_bytes() == *given)
    })
}

/// The core verbs: each means one thing in every command (the spec's table).
pub const CORE_VERBS: [&str; 20] = [
    "list",
    "show",
    "get",
    "sample",
    "check",
    "explain",
    "export",
    "create",
    "add",
    "set",
    "import",
    "restore",
    "move",
    "copy",
    "open",
    "close",
    "install",
    "delete",
    "uninstall",
    "run",
];

/// The verbs a noun adds by a decision: `wm`'s (D128, D141, D147, D160, D161 and D162) and
/// `disk`'s (D192).
pub const NOUN_VERBS: [(&str, &[&str]); 2] = [
    (
        "wm",
        &[
            "focus", "minimise", "maximise", "size", "move", "restore", "close", "capture",
            "click", "type", "press", "scroll", "menu", "act", "drag",
        ],
    ),
    ("disk", &["format", "repair"]),
];

/// EggOS's kept names: commands that keep their name and exact behavior, with a full
/// definition all the same (the spec's list, checked against the code).
pub const KEPT_NAMES: [&str; 47] = [
    "agentlog", "app", "cat", "chmod", "cp", "date", "device", "df", "disk", "echo", "edit",
    "false", "fetch", "free", "getprop", "grep", "halt", "head", "help", "hwclock", "ip", "ln",
    "logcat", "ls", "mkdir", "mv", "nslookup", "ping", "poweroff", "ps", "readlink", "reboot",
    "rm", "setprop", "settings", "sleep", "sync", "tail", "tee", "terminal", "touch", "trash",
    "true", "uname", "uptime", "wc", "wm",
];

/// `eggsh`'s built-ins: CLI only, defined in [`shell::SHELL_COMMANDS`] for the docs and the
/// tests (D165); `eggsh` does not link the registry, so they are not in [`COMMANDS`].
pub const SHELL_BUILTINS: [&str; 27] = [
    ".", ":", "[", "bg", "break", "cd", "continue", "echo", "eval", "exit", "export", "false",
    "fg", "help", "jobs", "kill", "local", "pwd", "read", "return", "set", "shift", "test", "true",
    "type", "unset", "wait",
];
