use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{
    COMMANDS, CommandDefinition, Count, OperandDefinition, OptionDefinition, Output as Form,
    Surface, ValueType, commands, errors, find_command,
};
use eggos_mcp::json::JsonWriter;

const USAGE_STATUS: i32 = 2;
const MAX_WORDS: usize = 4;
const WORDS_COLUMN: usize = 16;
const OPTION_COLUMN: usize = 20;
const DEFINITION_BYTES: usize = 16 * 1024;

pub fn help<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut json = false;
    let Ok(operands) = parse_defined(output, arguments, &commands::HELP, |_, given| {
        json |= given.is_long("json");
        Ok(())
    }) else {
        return USAGE_STATUS;
    };
    let operands = operands.all();
    if operands.len() > MAX_WORDS {
        output.coded(&errors::USAGE, &[b"at most 4 words name a command"]);
        return USAGE_STATUS;
    }
    if operands.is_empty() {
        for command in &COMMANDS {
            let status = match json {
                true => write_definition(output, command),
                false => {
                    write_summary(output, command);
                    0
                }
            };
            if status != 0 {
                return status;
            }
        }
        return 0;
    }
    let mut words: [&[u8]; MAX_WORDS] = [b""; MAX_WORDS];
    for (word, operand) in words.iter_mut().zip(operands) {
        *word = operand.to_bytes();
    }
    let words = &words[..operands.len()];
    let Some(command) = find_command(words) else {
        let mut named = [0u8; 256];
        let length = joined(words, &mut named);
        output.coded(
            &errors::NO_SUCH_COMMAND,
            &[b"no command is named '", &named[..length], b"'"],
        );
        return FAILURE;
    };
    match json {
        true => write_definition(output, command),
        false => {
            write_usage(output, command);
            0
        }
    }
}

fn joined(words: &[&[u8]], out: &mut [u8]) -> usize {
    let mut length = 0;
    for (index, word) in words.iter().enumerate() {
        let separator: &[u8] = if index > 0 { b" " } else { b"" };
        for part in [separator, *word] {
            let end = (length + part.len()).min(out.len());
            let taken = end - length;
            out[length..end].copy_from_slice(&part[..taken]);
            length = end;
        }
    }
    length
}

fn write_words<S: System>(output: &mut Output<S>, command: &CommandDefinition) -> usize {
    let mut written = 0;
    for (index, word) in command.words.iter().enumerate() {
        if index > 0 {
            output.write(b" ");
            written += 1;
        }
        output.write(word.as_bytes());
        written += word.len();
    }
    written
}

fn pad<S: System>(output: &mut Output<S>, written: usize, column: usize) {
    for _ in written..column {
        output.write(b" ");
    }
    output.write(b" ");
}

fn write_summary<S: System>(output: &mut Output<S>, command: &CommandDefinition) {
    let written = write_words(output, command);
    pad(output, written, WORDS_COLUMN);
    output.write(command.summary.as_bytes());
    output.write(b"\n");
}

fn option_names<S: System>(output: &mut Output<S>, option: &OptionDefinition) -> usize {
    let mut written = 0;
    if let Some(letter) = option.short {
        output.write(&[b'-', letter]);
        written += 2;
    }
    if let Some(long) = option.long {
        if written > 0 {
            output.write(b"|");
            written += 1;
        }
        output.write(b"--");
        output.write(long.as_bytes());
        written += 2 + long.len();
    }
    if let Some(value) = option.value {
        output.write(b" <");
        output.write(value.name.as_bytes());
        output.write(b">");
        written += 3 + value.name.len();
    }
    written
}

fn write_operand<S: System>(output: &mut Output<S>, operand: &OperandDefinition) {
    let optional = matches!(operand.count, Count::ZeroOrOne | Count::ZeroOrMore);
    let many = matches!(operand.count, Count::OneOrMore | Count::ZeroOrMore);
    output.write(if optional { b" [<" } else { b" <" });
    output.write(operand.name.as_bytes());
    output.write(b">");
    if many {
        output.write(b"...");
    }
    if optional {
        output.write(b"]");
    }
}

fn write_usage<S: System>(output: &mut Output<S>, command: &CommandDefinition) {
    output.write(b"usage: ");
    write_words(output, command);
    for option in command.options {
        output.write(b" [");
        option_names(output, option);
        output.write(b"]");
    }
    for operand in command.operands {
        write_operand(output, operand);
    }
    output.write(b"\n");
    output.write(command.description.as_bytes());
    output.write(b"\n");
    if !command.options.is_empty() {
        output.write(b"options:\n");
        for option in command.options {
            output.write(b"  ");
            let written = option_names(output, option);
            pad(output, written, OPTION_COLUMN);
            output.write(option.help.as_bytes());
            output.write(b"\n");
        }
    }
    output.write(b"errors (exit status):");
    for (index, declared) in command.errors.iter().enumerate() {
        output.write(if index > 0 { b", " } else { b" " });
        output.write(declared.error.code.as_bytes());
        output.write(b" ");
        output.write_number(u64::from(declared.exit.unsigned_abs()), 0);
    }
    output.write(b"\n");
    write_examples(output, command);
    match command.surface {
        Surface::Gui(place) => {
            output.write(b"GUI: ");
            output.write(place.as_bytes());
        }
        Surface::NoGui(why) => {
            output.write(b"No GUI: ");
            output.write(why.as_bytes());
        }
    }
    output.write(b"\n");
}

fn write_examples<S: System>(output: &mut Output<S>, command: &CommandDefinition) {
    if command.examples.is_empty() {
        return;
    }
    output.write(b"examples:\n");
    for example in command.examples {
        output.write(b"  $ ");
        output.write(example.line.as_bytes());
        output.write(b"\n");
        for line in example.output.split_inclusive('\n') {
            output.write(b"    ");
            output.write(line.as_bytes());
        }
        if !example.output.is_empty() && !example.output.ends_with('\n') {
            output.write(b"\n");
        }
        if example.status != 0 {
            output.write(b"    (exit status ");
            output.write_number(u64::from(example.status.unsigned_abs()), 0);
            output.write(b")\n");
        }
    }
}

fn write_value_type(out: &mut JsonWriter<'_>, value: ValueType) {
    out.key("type");
    out.string(value.name());
    match value {
        ValueType::Integer { minimum, maximum } => {
            out.key("minimum");
            out.i64(minimum);
            out.key("maximum");
            out.i64(maximum);
        }
        ValueType::Choice(choices) => {
            out.key("choices");
            out.begin_array();
            for choice in choices {
                out.string(choice);
            }
            out.end_array();
        }
        ValueType::Path | ValueType::Text | ValueType::Bytes => {}
    }
}

fn optional_string(out: &mut JsonWriter<'_>, key: &str, value: Option<&str>) {
    out.key(key);
    match value {
        Some(value) => out.string(value),
        None => out.null(),
    }
}

fn write_form(out: &mut JsonWriter<'_>, key: &str, form: Form) {
    out.key(key);
    out.begin_object();
    out.key("kind");
    out.string(form.name());
    if let Form::Records(fields) | Form::Object(fields) = form {
        out.key("fields");
        out.begin_array();
        for field in fields {
            out.begin_object();
            out.key("name");
            out.string(field.name);
            out.key("type");
            out.string(field.value.name());
            out.key("nullable");
            out.bool(field.nullable);
            out.end_object();
        }
        out.end_array();
    }
    out.end_object();
}

fn write_definition<S: System>(output: &mut Output<S>, command: &CommandDefinition) -> i32 {
    let mut buffer = [0u8; DEFINITION_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("words");
    out.begin_array();
    for word in command.words {
        out.string(word);
    }
    out.end_array();
    out.key("summary");
    out.string(command.summary);
    out.key("description");
    out.string(command.description);
    out.key("options");
    out.begin_array();
    for option in command.options {
        out.begin_object();
        out.key("short");
        match option.short {
            Some(letter) => out.string_lossy(&[letter]),
            None => out.null(),
        }
        optional_string(&mut out, "long", option.long);
        out.key("value");
        match option.value {
            Some(value) => {
                out.begin_object();
                out.key("name");
                out.string(value.name);
                write_value_type(&mut out, value.value);
                out.end_object();
            }
            None => out.null(),
        }
        out.key("repeatable");
        out.bool(option.repeatable);
        optional_string(&mut out, "default", option.default);
        out.key("help");
        out.string(option.help);
        out.end_object();
    }
    out.end_array();
    out.key("operands");
    out.begin_array();
    for operand in command.operands {
        out.begin_object();
        out.key("name");
        out.string(operand.name);
        write_value_type(&mut out, operand.value);
        out.key("count");
        out.string(operand.count.name());
        out.key("dash_is_stdin");
        out.bool(operand.dash_is_stdin);
        out.end_object();
    }
    out.end_array();
    out.key("outputs");
    out.begin_object();
    write_form(&mut out, "text", command.outputs.text);
    write_form(&mut out, "json", command.outputs.json);
    out.end_object();
    out.key("max_effect");
    out.string(command.max_effect.name());
    out.key("errors");
    out.begin_array();
    for declared in command.errors {
        out.begin_object();
        out.key("code");
        out.string(declared.error.code);
        out.key("category");
        out.string(declared.error.category.name());
        out.key("exit");
        out.i64(i64::from(declared.exit));
        out.key("fix");
        out.string(declared.error.fix);
        optional_string(&mut out, "fix_command", declared.error.fix_command);
        out.end_object();
    }
    out.end_array();
    out.key("examples");
    out.begin_array();
    for example in command.examples {
        out.begin_object();
        out.key("line");
        out.string(example.line);
        out.key("output");
        out.string(example.output);
        out.key("status");
        out.i64(i64::from(example.status));
        out.end_object();
    }
    out.end_array();
    out.key("surface");
    out.begin_object();
    match command.surface {
        Surface::Gui(place) => {
            out.key("gui");
            out.string(place);
        }
        Surface::NoGui(why) => {
            out.key("no_gui");
            out.string(why);
        }
    }
    out.end_object();
    out.end_object();
    out.end_line();
    if out.overflowed() {
        output.error(&[b"a definition does not fit the JSON buffer"]);
        return FAILURE;
    }
    output.write(out.as_bytes());
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use eggos_mcp::json::{self, Value};

    fn run(arguments: &[&CStr]) -> (i32, String, String) {
        let mut output = Output::new(Fake::default(), b"help");
        let status = help(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn help_lists_every_command_of_the_registry_with_its_summary() {
        let (status, output, _) = run(&[]);
        assert_eq!(status, 0);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), COMMANDS.len());
        for (line, command) in lines.iter().zip(&COMMANDS) {
            assert!(line.starts_with(&command.words.join(" ")), "{line}");
            assert!(line.ends_with(command.summary), "{line}");
        }
    }

    #[test]
    fn help_json_lists_exactly_the_registry_as_parsable_definitions() {
        let (status, output, _) = run(&[c"--json"]);
        assert_eq!(status, 0);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), COMMANDS.len());
        for (line, command) in lines.iter().zip(&COMMANDS) {
            let value = json::parse(line.as_bytes()).unwrap().value;
            let Some(Value::Array(words)) = value.get("words") else {
                panic!("words: {line}");
            };
            let words: Vec<String> = words
                .elements()
                .map(|word| word.as_str().unwrap().chars().collect())
                .collect();
            assert_eq!(words, command.words);
            assert!(
                value
                    .get("max_effect")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .equals(command.max_effect.name())
            );
            let Some(Value::Array(errors)) = value.get("errors") else {
                panic!("errors: {line}");
            };
            assert_eq!(errors.elements().count(), command.errors.len());
        }
    }

    #[test]
    fn help_shows_one_command_s_usage_options_errors_and_gui() {
        let (status, output, _) = run(&[c"wm", c"list"]);
        assert_eq!(status, 0);
        assert!(
            output.starts_with("usage: wm list [--json] [--limit <count>] [--after <cursor>]\n"),
            "{output}"
        );
        assert!(
            output.contains("errors (exit status): usage 2, no_window_server 1, bad_cursor 2\n"),
            "{output}"
        );
        assert!(output.contains("GUI: the taskbar"));
        let (status, output, _) = run(&[c"ls"]);
        assert_eq!(status, 0);
        assert!(
            output.starts_with(
                "usage: ls [-a] [-A] [-l] [-d] [-1] [--json] [--limit <count>] \
                 [--after <cursor>] [<path>...]\n"
            ),
            "{output}"
        );
        let (_, output, _) = run(&[c"agentlog"]);
        assert!(output.contains("  -n <count>"), "{output}");
        let (status, output, _) = run(&[c"--json", c"app", c"list"]);
        assert_eq!(status, 0);
        assert_eq!(output.lines().count(), 1);
        assert!(output.contains("\"fields\":[{\"name\":\"name\""));
    }

    #[test]
    fn help_shows_a_command_s_examples_with_their_output_and_status() {
        let (status, output, _) = run(&[c"grep"]);
        assert_eq!(status, 0);
        assert!(
            output.contains(
                "examples:\n  $ grep milk /home/root/notes.txt\n    milk\n\
                 \x20 $ grep -c e /home/root/notes.txt\n    2\n\
                 \x20 $ grep tea /home/root/notes.txt\n    (exit status 1)\n"
            ),
            "{output}"
        );
        let (_, output, _) = run(&[c"echo"]);
        assert!(output.contains("  $ echo -n hi\n    hi\n"), "{output}");
        let (status, output, _) = run(&[c"--json", c"settings", c"get"]);
        assert_eq!(status, 0);
        assert!(
            output.contains(
                "\"examples\":[{\"line\":\"settings get shell.profile\",\"output\":\"macos\\n\",\
                 \"status\":0}"
            ),
            "{output}"
        );
    }

    #[test]
    fn an_unknown_command_is_not_found_with_a_fix() {
        let (status, _, errors) = run(&[c"frobnicate"]);
        assert_eq!(status, 1);
        assert_eq!(
            errors,
            "help: no command is named 'frobnicate'\n  try: help\n"
        );
        let (status, _, errors) = run(&[c"--json", c"wm", c"fly"]);
        assert_eq!(status, 1);
        assert!(errors.contains("\"code\":\"no_such_command\""), "{errors}");
        let (status, _, _) = run(&[c"a", c"b", c"c", c"d", c"e"]);
        assert_eq!(status, 2);
    }
}
