//! `eggsh`'s built-ins as definitions (D107, D165): the shell runs them itself and does not link
//! the registry (it stays small), so they are described here for the docs and the tests only,
//! command line only, and are not in [`crate::COMMANDS`] or `help --json`.

use crate::errors::{FAILED, NO_SUCH_COMMAND, NO_SUCH_FILE, USAGE};
use crate::{
    CommandDefinition, CommandError, Count, Effect, ErrorDefinition, OperandDefinition,
    OptionDefinition, OptionValue, Output, Outputs, Surface, ValueType, flag, operand,
};

const fn exits(error: &'static ErrorDefinition, exit: i32) -> CommandError {
    CommandError { error, exit }
}

const BUILT_IN: Surface = Surface::NoGui("a built-in of the shell, eggsh");

const TEXT: Outputs = Outputs {
    text: Output::Text,
    json: Output::None,
};

const NOTHING: Outputs = Outputs {
    text: Output::None,
    json: Output::None,
};

const ANY_WORDS: [OperandDefinition; 1] = [operand("word", ValueType::Text, Count::ZeroOrMore)];

const NAMES: [OperandDefinition; 1] = [operand("name", ValueType::Text, Count::ZeroOrMore)];

const FILE: [OperandDefinition; 1] = [operand("file", ValueType::Path, Count::One)];

const FOLDER: [OperandDefinition; 1] = [operand("folder", ValueType::Path, Count::ZeroOrOne)];

const COUNT: [OperandDefinition; 1] = [operand(
    "count",
    ValueType::Integer {
        minimum: 1,
        maximum: 999_999_999,
    },
    Count::ZeroOrOne,
)];

const EXPRESSION: [OperandDefinition; 1] =
    [operand("expression", ValueType::Text, Count::ZeroOrMore)];

const READ_ERRORS: [CommandError; 1] = [exits(&FAILED, 1)];

const STATUS: [OperandDefinition; 1] = [operand(
    "status",
    ValueType::Integer {
        minimum: 0,
        maximum: 255,
    },
    Count::ZeroOrOne,
)];

const WORDS: [OperandDefinition; 1] = [operand("words", ValueType::Text, Count::ZeroOrMore)];

const SOURCE_ERRORS: [CommandError; 2] = [exits(&USAGE, 2), exits(&NO_SUCH_FILE, 1)];

const CD_ERRORS: [CommandError; 2] = [exits(&USAGE, 1), exits(&NO_SUCH_FILE, 1)];

const USAGE_2: [CommandError; 1] = [exits(&USAGE, 2)];

const USAGE_1: [CommandError; 1] = [exits(&USAGE, 1)];

const NOT_FOUND: [CommandError; 1] = [exits(&NO_SUCH_COMMAND, 1)];

const JOB: [OperandDefinition; 1] = [operand("job", ValueType::Text, Count::ZeroOrOne)];

const TARGETS: [OperandDefinition; 1] = [operand("target", ValueType::Text, Count::OneOrMore)];

const WAITED: [OperandDefinition; 1] = [operand("target", ValueType::Text, Count::ZeroOrMore)];

const JOB_ERRORS: [CommandError; 1] = [exits(&FAILED, 1)];

const KILL_ERRORS: [CommandError; 2] = [exits(&USAGE, 2), exits(&FAILED, 1)];

const WAIT_ERRORS: [CommandError; 1] = [exits(&FAILED, 127)];

const KILL_OPTIONS: [OptionDefinition; 2] = [
    OptionDefinition {
        short: Some(b's'),
        long: None,
        value: Some(OptionValue {
            name: "signal",
            value: ValueType::Text,
        }),
        repeatable: false,
        default: Some("TERM"),
        help: "The signal to send, by name (TERM, SIGKILL, cont) or number; -NAME says it too.",
    },
    flag(b'l', "List the signals' names."),
];

const fn built_in(
    words: &'static [&'static str],
    summary: &'static str,
    description: &'static str,
    operands: &'static [OperandDefinition],
    outputs: Outputs,
    max_effect: Effect,
    errors: &'static [CommandError],
) -> CommandDefinition {
    CommandDefinition {
        words,
        summary,
        description,
        options: &[],
        operands,
        outputs,
        max_effect,
        errors,
        examples: &[],
        surface: BUILT_IN,
    }
}

/// `eggsh`'s built-ins, in the order of [`crate::SHELL_BUILTINS`].
pub const SHELL_COMMANDS: [CommandDefinition; 27] = [
    built_in(
        &["."],
        "Runs a file's commands in this shell.",
        "Reads the file and runs its commands in the shell itself, so the variables they set \
         stay set; nested at most a few files deep.",
        &FILE,
        NOTHING,
        Effect::Control,
        &SOURCE_ERRORS,
    ),
    built_in(
        &[":"],
        "Does nothing, successfully.",
        "Takes any words, expanded as usual, and ends with status 0.",
        &ANY_WORDS,
        NOTHING,
        Effect::Read,
        &[],
    ),
    built_in(
        &["["],
        "Tests an expression, ended by ].",
        "As test, with ] as its last word: ends with status 0 if the expression is true, 1 if \
         it is false and 2 if it cannot be read.",
        &EXPRESSION,
        NOTHING,
        Effect::Read,
        &USAGE_2,
    ),
    built_in(
        &["bg"],
        "Continues a stopped job in the background.",
        "Sends a stopped job SIGCONT and leaves it running in the background, writing [n]+ \
         command &; the job is the current one, or the one %n, n, %%, %- or %name names; \
         needs an interactive shell.",
        &JOB,
        TEXT,
        Effect::Control,
        &JOB_ERRORS,
    ),
    built_in(
        &["break"],
        "Leaves a loop.",
        "Leaves the innermost for, while or until loop, or the count-th one out; in a \
         function, only the function's own loops.",
        &COUNT,
        NOTHING,
        Effect::Control,
        &USAGE_1,
    ),
    built_in(
        &["cd"],
        "Changes the working folder.",
        "Changes the shell's working folder to the operand, to $HOME without one, or back to \
         $OLDPWD with -; sets PWD and OLDPWD.",
        &FOLDER,
        NOTHING,
        Effect::Read,
        &CD_ERRORS,
    ),
    built_in(
        &["continue"],
        "Starts a loop's next round.",
        "Goes on with the next round of the innermost for, while or until loop, or of the \
         count-th one out.",
        &COUNT,
        NOTHING,
        Effect::Control,
        &USAGE_1,
    ),
    CommandDefinition {
        options: &[flag(b'n', "No newline at the end.")],
        ..built_in(
            &["echo"],
            "Writes its words.",
            "Writes its words separated by spaces, then a newline unless the first word is -n; \
             other options are words too.",
            &ANY_WORDS,
            TEXT,
            Effect::Read,
            &[],
        )
    },
    built_in(
        &["eval"],
        "Runs words as commands.",
        "Joins its words with spaces and runs the result as commands in this shell, ending \
         with the last one's status.",
        &ANY_WORDS,
        NOTHING,
        Effect::Control,
        &[],
    ),
    built_in(
        &["exit"],
        "Ends the shell.",
        "Ends the shell with the status given, 0 to 255, or the last command's.",
        &STATUS,
        NOTHING,
        Effect::Change,
        &USAGE_2,
    ),
    built_in(
        &["export"],
        "Gives variables to the commands the shell runs.",
        "Marks each NAME or NAME=value for the environment of the commands the shell runs; \
         without operands, writes the exported variables as export lines.",
        &NAMES,
        TEXT,
        Effect::Change,
        &USAGE_1,
    ),
    built_in(
        &["false"],
        "Fails, doing nothing.",
        "Takes any words and ends with status 1.",
        &ANY_WORDS,
        NOTHING,
        Effect::Read,
        &[],
    ),
    built_in(
        &["fg"],
        "Brings a job to the foreground.",
        "Writes the job's command line, gives it the terminal, sends it SIGCONT if it is \
         stopped and waits for it as for a command typed at the prompt (the current job \
         without an operand); needs an interactive shell.",
        &JOB,
        TEXT,
        Effect::Control,
        &JOB_ERRORS,
    ),
    built_in(
        &["help"],
        "Describes the shell, or an EggOS command.",
        "Alone, describes the shell's syntax and built-ins; with words, runs /bin/help, which \
         describes the command those words name.",
        &WORDS,
        TEXT,
        Effect::Read,
        &NOT_FOUND,
    ),
    CommandDefinition {
        options: &[
            flag(b'l', "Also write each job's process id."),
            flag(b'p', "Write only each job's process id."),
        ],
        ..built_in(
            &["jobs"],
            "Lists the shell's jobs.",
            "Writes one line a job, [n]+ state command: Running, Stopped, Done or Exit n, + for \
             the current job and - for the previous one; jobs that are done are listed once.",
            &[],
            TEXT,
            Effect::Read,
            &USAGE_2,
        )
    },
    CommandDefinition {
        options: &KILL_OPTIONS,
        ..built_in(
            &["kill"],
            "Sends a signal to processes or jobs.",
            "Sends the signal, TERM unless named, to each process id, -group, or %job (its \
             process group; a stopped job is continued so it takes the signal); kill -l lists \
             the signals.",
            &TARGETS,
            TEXT,
            Effect::Control,
            &KILL_ERRORS,
        )
    },
    built_in(
        &["local"],
        "Makes variables local to a function.",
        "Keeps each NAME or NAME=value to the function running: when it returns, the \
         variables have their earlier values again.",
        &NAMES,
        NOTHING,
        Effect::Change,
        &USAGE_1,
    ),
    built_in(
        &["pwd"],
        "Writes the working folder.",
        "Writes the shell's working folder, as $PWD holds it.",
        &[],
        TEXT,
        Effect::Read,
        &[],
    ),
    CommandDefinition {
        options: &[flag(b'r', "Keep backslashes as they are.")],
        ..built_in(
            &["read"],
            "Reads a line into variables.",
            "Reads a line of standard input and splits it at the characters of IFS into the \
             named variables, the last taking the rest of the line (REPLY without a name); \
             status 1 at the end of the input.",
            &NAMES,
            NOTHING,
            Effect::Read,
            &READ_ERRORS,
        )
    },
    built_in(
        &["return"],
        "Ends a function.",
        "Ends the function running, or the file . runs, with the status given or the last \
         command's.",
        &STATUS,
        NOTHING,
        Effect::Control,
        &USAGE_1,
    ),
    built_in(
        &["set"],
        "Writes the shell's variables, or sets its options and arguments.",
        "Alone, writes every variable with a value as NAME='value', one a line; -e ends the \
         shell when a command fails, +e stops that, and the words after the options (or \
         after --) become the positional parameters $1, $2, ....",
        &WORDS,
        TEXT,
        Effect::Change,
        &USAGE_2,
    ),
    built_in(
        &["shift"],
        "Drops positional parameters.",
        "Drops the first positional parameter, or the first count of them: $2 becomes $1.",
        &COUNT,
        NOTHING,
        Effect::Change,
        &USAGE_1,
    ),
    built_in(
        &["test"],
        "Tests an expression.",
        "Ends with status 0 if the expression is true, 1 if it is false and 2 if it cannot be \
         read: -e, -f, -d, -r, -w, -x, -s and other file tests, -n and -z, = and != for \
         words, -eq, -ne, -lt, -le, -gt and -ge for numbers, with !, -a, -o and parentheses.",
        &EXPRESSION,
        NOTHING,
        Effect::Read,
        &USAGE_2,
    ),
    built_in(
        &["true"],
        "Succeeds, doing nothing.",
        "Takes any words and ends with status 0.",
        &ANY_WORDS,
        NOTHING,
        Effect::Read,
        &[],
    ),
    built_in(
        &["type"],
        "Says what each name runs.",
        "Writes for each name whether it is a function, a shell built-in or which program in \
         $PATH it runs.",
        &NAMES,
        TEXT,
        Effect::Read,
        &NOT_FOUND,
    ),
    built_in(
        &["unset"],
        "Removes variables.",
        "Removes each named variable from the shell and the environment it gives commands; \
         with -f, each named function.",
        &NAMES,
        NOTHING,
        Effect::Change,
        &USAGE_1,
    ),
    built_in(
        &["wait"],
        "Waits for background jobs.",
        "Waits for every job, or for each process id or %job named, and ends with the last \
         one's status, 127 for one that is not the shell's child.",
        &WAITED,
        NOTHING,
        Effect::Read,
        &WAIT_ERRORS,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{COMMANDS, SHELL_BUILTINS};

    #[test]
    fn every_built_in_is_defined_once_in_the_shell_s_order() {
        let names: Vec<&str> = SHELL_COMMANDS
            .iter()
            .map(|command| command.words[0])
            .collect();
        assert_eq!(names, SHELL_BUILTINS);
        for command in &SHELL_COMMANDS {
            let name = command.words[0];
            assert_eq!(command.words.len(), 1, "{name}");
            assert!(command.summary.ends_with('.'), "{name}");
            assert!(command.description.len() > command.summary.len(), "{name}");
            assert!(!command.has_json(), "{name}");
            assert!(matches!(command.surface, Surface::NoGui(_)), "{name}");
            assert!(
                COMMANDS.iter().all(|other| other.words != command.words)
                    || matches!(name, "echo" | "false" | "help" | "true"),
                "{name}"
            );
        }
    }
}
