//! `eggsh`'s built-ins as definitions (D107, D165): the shell runs them itself and does not link
//! the registry (it stays small), so they are described here for the docs and the tests only,
//! command line only, and are not in [`crate::COMMANDS`] or `help --json`.

use crate::errors::{NO_SUCH_COMMAND, NO_SUCH_FILE, USAGE};
use crate::{
    CommandDefinition, CommandError, Count, Effect, ErrorDefinition, OperandDefinition, Output,
    Outputs, Surface, ValueType, flag, operand,
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
pub const SHELL_COMMANDS: [CommandDefinition; 13] = [
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
        &["cd"],
        "Changes the working folder.",
        "Changes the shell's working folder to the operand, to $HOME without one, or back to \
         $OLDPWD with -; sets PWD and OLDPWD.",
        &FOLDER,
        NOTHING,
        Effect::Read,
        &CD_ERRORS,
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
        &["help"],
        "Describes the shell, or an EggOS command.",
        "Alone, describes the shell's syntax and built-ins; with words, runs /bin/help, which \
         describes the command those words name.",
        &WORDS,
        TEXT,
        Effect::Read,
        &NOT_FOUND,
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
    built_in(
        &["set"],
        "Writes the shell's variables.",
        "Writes every variable with a value as NAME='value', one a line; options are not \
         supported yet.",
        &[],
        TEXT,
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
        "Writes for each name whether it is a shell built-in or which program in $PATH it \
         runs.",
        &NAMES,
        TEXT,
        Effect::Read,
        &NOT_FOUND,
    ),
    built_in(
        &["unset"],
        "Removes variables.",
        "Removes each named variable from the shell and the environment it gives commands.",
        &NAMES,
        NOTHING,
        Effect::Change,
        &USAGE_1,
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
