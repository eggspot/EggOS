//! `windows registry get|set|delete|reset` (D231), the registry's CLI front-end, which the MCP
//! tools run as their twin: the same words, types and errors as Settings' Windows pane
//! (docs/ux/windows-registry-v0.3.md sections 3 and 6). A value name `@` names a key's default
//! value, as `.reg` files write it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eggos_commands::ErrorDefinition;
use eggos_commands::errors::{self, by_code};
pub use eggos_commands::tools::CODED_ERRORS_ENVIRONMENT;

use crate::hive::Value;
use crate::path::{KeyPath, utf8, utf16};
use crate::registry::{HiveFile, Registry, Store, USER_HIVE_PATH};
use crate::value::{
    REG_DWORD, REG_DWORD_BIG_ENDIAN, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ, data_text,
    hex_text, number_of, parse_data, string_of, strings_of, type_from_name, type_name,
};
use crate::{Error, MAX_DATA_BYTES, MAX_DEPTH, MAX_HIVE_BYTES, MAX_KEY_NAME_UNITS};

/// The name the command line gives a key's default value.
pub const DEFAULT_VALUE_WORD: &str = "@";
/// How a key's default value is shown.
pub const DEFAULT_VALUE_TEXT: &str = "(Default)";
/// The data shown for a default value that was never set.
pub const VALUE_NOT_SET: &str = "(value not set)";

const USAGE_STATUS: i32 = 2;
const FAILURE_STATUS: i32 = 1;
const SEPARATOR: &str = "  ";

/// Where the command writes and whom it asks.
pub trait Console {
    /// Writes to standard output.
    fn out(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn err(&mut self, bytes: &[u8]);
    /// Asks the person `question`; `None` when there is no one to ask.
    fn confirm(&mut self, question: &str) -> Option<bool>;
    /// Whether a text error ends with its JSON line too (`EGGOS_CODED_ERRORS=1`, D226).
    fn wants_coded_errors(&self) -> bool;
}

/// What an error says, in the words the pane, the command and MCP share; `path` is the key's
/// full path and `name` the value's name, where they matter.
pub fn message(error: Error, path: &str, name: &str) -> String {
    match error {
        Error::NoSuchKey => format!("No such key: {path}."),
        Error::NoSuchValue => format!("No such value: {name} in {path}."),
        Error::ReadOnlyKey => String::from(
            "Not saved: HKEY_LOCAL_MACHINE is machine-wide. Only an administrator can change it.",
        ),
        Error::RootKey => String::from("Roots cannot be created, deleted or edited."),
        Error::HasSubkeys => format!("Not deleted: {path} has subkeys."),
        Error::BadName => {
            format!("Key names cannot contain \\ and are {MAX_KEY_NAME_UNITS} characters at most.")
        }
        Error::BadData => String::from(
            "Not saved: the data does not fit the type (a whole number in decimal or after 0x, \
             or pairs of hex digits for binary).",
        ),
        Error::ValueTooLarge => format!(
            "Not saved: a value is {} MiB at most.",
            MAX_DATA_BYTES >> 20
        ),
        Error::KeyTooDeep => format!("Not saved: keys can be {MAX_DEPTH} levels deep at most."),
        Error::HiveFull => format!(
            "Not saved: the registry file has reached its limit of {} MiB. Delete keys you no \
             longer need.",
            MAX_HIVE_BYTES >> 20
        ),
        Error::DiskFull => {
            String::from("Not saved: the disk is full. Free some space and try again.")
        }
        Error::HiveDamaged => String::from("Your registry file is damaged."),
        Error::ReadFailed => String::from("The registry file could not be read."),
        Error::WriteFailed => String::from("Not saved: the registry file could not be written."),
    }
}

/// The command model's definition of `error`.
pub fn definition(error: Error) -> &'static ErrorDefinition {
    by_code(error.code().as_bytes()).unwrap_or(&errors::FAILED)
}

/// How a value's name is shown: [`DEFAULT_VALUE_TEXT`] for the default value.
pub fn name_text(name: &[u16]) -> String {
    match name.is_empty() {
        true => String::from(DEFAULT_VALUE_TEXT),
        false => utf8(name),
    }
}

struct Options<'a> {
    json: bool,
    yes: bool,
    dry_run: bool,
    machine: bool,
    kind: Option<&'a str>,
    operands: Vec<&'a str>,
}

fn options<'a>(words: &[&'a str]) -> Option<Options<'a>> {
    let mut options = Options {
        json: false,
        yes: false,
        dry_run: false,
        machine: false,
        kind: None,
        operands: Vec::new(),
    };
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match *word {
            "--" => {
                options.operands.extend(words.by_ref().copied());
                break;
            }
            "--json" => options.json = true,
            "--yes" => options.yes = true,
            "--dry-run" => options.dry_run = true,
            "--machine" => options.machine = true,
            "--type" => options.kind = Some(*words.next()?),
            option if option.starts_with("--type=") => options.kind = Some(&option[7..]),
            option if option.starts_with('-') && option.len() > 1 => return None,
            operand => options.operands.push(operand),
        }
    }
    Some(options)
}

struct Run<'c, C: Console> {
    console: &'c mut C,
    json: bool,
}

impl<C: Console> Run<'_, C> {
    fn fail(&mut self, definition: &ErrorDefinition, text: &str) -> i32 {
        if !self.json {
            self.console.err(b"windows: ");
            self.console.err(text.as_bytes());
            self.console.err(b"\n");
            if let Some(command) = definition.fix_command {
                self.console.err(b"  try: ");
                self.console.err(command.as_bytes());
                self.console.err(b"\n");
            }
        }
        if self.json || self.console.wants_coded_errors() {
            let mut line = String::from("{\"error\":{\"code\":");
            json_string(&mut line, definition.code);
            line.push_str(",\"category\":");
            json_string(&mut line, definition.category.name());
            line.push_str(",\"message\":");
            json_string(&mut line, text);
            line.push_str(",\"fix\":");
            json_string(&mut line, definition.fix);
            line.push_str("}}\n");
            self.console.err(line.as_bytes());
        }
        match definition.category {
            eggos_commands::Category::Usage => USAGE_STATUS,
            _ => FAILURE_STATUS,
        }
    }

    fn usage(&mut self, text: &str) -> i32 {
        self.fail(&errors::USAGE, text)
    }

    fn error(&mut self, error: Error, path: &str, name: &str) -> i32 {
        self.fail(definition(error), &message(error, path, name))
    }
}

/// Runs `windows <arguments>`; its exit status.
pub fn run<S: Store, C: Console>(
    registry: &mut Registry<S>,
    console: &mut C,
    arguments: &[&str],
) -> i32 {
    let (verb, rest) = match arguments {
        ["registry", verb, rest @ ..] => (*verb, rest),
        _ => {
            return Run {
                console,
                json: false,
            }
            .usage("usage: windows registry get|set|delete|reset ...");
        }
    };
    let Some(options) = options(rest) else {
        return Run {
            console,
            json: false,
        }
        .usage("unknown option: help windows registry shows the options");
    };
    let mut run = Run {
        console,
        json: options.json,
    };
    match verb {
        "get" => get(registry, &mut run, &options),
        "set" => set(registry, &mut run, &options),
        "delete" => delete(registry, &mut run, &options),
        "reset" => reset(registry, &mut run, &options),
        _ => run.usage("usage: windows registry get|set|delete|reset ..."),
    }
}

fn key_operand<C: Console>(run: &mut Run<'_, C>, text: Option<&&str>) -> Result<KeyPath, i32> {
    let Some(text) = text else {
        return Err(run.usage("a key is needed, like HKCU\\Software\\AutoHotkey"));
    };
    KeyPath::parse(text).map_err(|error| run.error(error, text, ""))
}

fn value_name(word: &str) -> Vec<u16> {
    match word {
        DEFAULT_VALUE_WORD => Vec::new(),
        name => utf16(name),
    }
}

fn get<S: Store, C: Console>(
    registry: &mut Registry<S>,
    run: &mut Run<'_, C>,
    options: &Options<'_>,
) -> i32 {
    if options.operands.len() > 2 {
        return run.usage("get takes a key and at most one value name");
    }
    let path = match key_operand(run, options.operands.first()) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let key_text = path.text();
    if let Some(word) = options.operands.get(1) {
        let name = value_name(word);
        return match registry.value(&path, &name) {
            Ok(value) => {
                let line = match options.json {
                    true => value_record(&key_text, &value),
                    false => value_line(&value),
                };
                run.console.out(line.as_bytes());
                0
            }
            Err(error) => run.error(error, &key_text, &name_text(&name)),
        };
    }
    let listed = registry
        .subkeys(&path)
        .and_then(|subkeys| Ok((subkeys, registry.values(&path)?)));
    let (subkeys, values) = match listed {
        Ok(listed) => listed,
        Err(error) => return run.error(error, &key_text, ""),
    };
    let mut out = String::new();
    for subkey in &subkeys {
        if options.json {
            out.push_str("{\"key\":");
            json_string(&mut out, &key_text);
            out.push_str(",\"subkey\":");
            json_string(&mut out, &utf8(subkey));
            out.push_str("}\n");
        } else {
            out.push_str(&utf8(subkey));
            out.push_str("\\\n");
        }
    }
    let default = values.iter().find(|value| value.name.is_empty());
    if !options.json && default.is_none() {
        out.push_str(&format!(
            "{DEFAULT_VALUE_TEXT}{SEPARATOR}REG_SZ{SEPARATOR}{VALUE_NOT_SET}\n"
        ));
    }
    for value in default
        .into_iter()
        .chain(values.iter().filter(|value| !value.name.is_empty()))
    {
        out.push_str(&match options.json {
            true => value_record(&key_text, value),
            false => value_line(value),
        });
    }
    run.console.out(out.as_bytes());
    0
}

/// A value as the command's text line: name, type and data, two spaces apart.
pub fn value_line(value: &Value) -> String {
    format!(
        "{}{SEPARATOR}{}{SEPARATOR}{}\n",
        name_text(&value.name),
        type_name(value.kind),
        data_text(value.kind, &value.data)
    )
}

/// A value as the command's JSON record (and `windows_registry_get`'s).
pub fn value_record(key: &str, value: &Value) -> String {
    let mut out = String::from("{\"key\":");
    json_string(&mut out, key);
    out.push_str(",\"name\":");
    json_string(&mut out, &utf8(&value.name));
    out.push_str(",\"type\":");
    json_string(&mut out, &type_name(value.kind));
    out.push_str(",\"data\":");
    match value.kind {
        REG_SZ | REG_EXPAND_SZ => json_string(&mut out, &string_of(&value.data)),
        REG_MULTI_SZ => {
            out.push('[');
            for (index, string) in strings_of(&value.data).iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                json_string(&mut out, string);
            }
            out.push(']');
        }
        REG_DWORD | REG_DWORD_BIG_ENDIAN | REG_QWORD => match number_of(value.kind, &value.data) {
            Some(number) => out.push_str(&format!("{number}")),
            None => json_string(&mut out, &hex_text(&value.data, "")),
        },
        _ => json_string(&mut out, &hex_text(&value.data, "")),
    }
    out.push_str("}\n");
    out
}

fn set<S: Store, C: Console>(
    registry: &mut Registry<S>,
    run: &mut Run<'_, C>,
    options: &Options<'_>,
) -> i32 {
    let path = match key_operand(run, options.operands.first()) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let key_text = path.text();
    let kind = match options.kind {
        None => REG_SZ,
        Some(word) => match type_from_name(word) {
            Some(kind) => kind,
            None => {
                return run.usage("--type takes sz, expand_sz, multi_sz, dword, qword or binary");
            }
        },
    };
    let Some(word) = options.operands.get(1) else {
        if options.kind.is_some() {
            return run.usage("--type needs a value name");
        }
        return match registry.create_key(&path) {
            Ok(_) => 0,
            Err(error) => run.error(error, &key_text, ""),
        };
    };
    let name = value_name(word);
    let data = match parse_data(kind, &options.operands[2..]) {
        Ok(data) => data,
        Err(error) => return run.error(error, &key_text, &name_text(&name)),
    };
    let saved = registry
        .create_key(&path)
        .and_then(|_| registry.set_value(&path, &name, kind, data));
    match saved {
        Ok(()) => 0,
        Err(error) => run.error(error, &key_text, &name_text(&name)),
    }
}

fn counted(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

fn confirmed<C: Console>(run: &mut Run<'_, C>, yes: bool, question: &str) -> Result<(), i32> {
    if yes {
        return Ok(());
    }
    match run.console.confirm(question) {
        Some(true) => Ok(()),
        Some(false) => Err(run.fail(&errors::DECLINED_BY_USER, "Nothing was deleted.")),
        None => Err(run.fail(
            &errors::NEEDS_CONFIRMATION,
            "This cannot be undone: run it again with --yes.",
        )),
    }
}

fn delete<S: Store, C: Console>(
    registry: &mut Registry<S>,
    run: &mut Run<'_, C>,
    options: &Options<'_>,
) -> i32 {
    if options.operands.len() > 2 {
        return run.usage("delete takes a key and at most one value name");
    }
    let path = match key_operand(run, options.operands.first()) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let key_text = path.text();
    if let Some(word) = options.operands.get(1) {
        let name = value_name(word);
        let shown = name_text(&name);
        if let Err(error) = registry.value(&path, &name) {
            return run.error(error, &key_text, &shown);
        }
        if options.dry_run {
            run.console
                .out(format!("would delete the value {shown} in {key_text}\n").as_bytes());
            return 0;
        }
        let question = format!("Delete the value {shown} in {key_text}? [y/N] ");
        if let Err(status) = confirmed(run, options.yes, &question) {
            return status;
        }
        return match registry.delete_value(&path, &name) {
            Ok(()) => 0,
            Err(error) => run.error(error, &key_text, &shown),
        };
    }
    if path.names.is_empty() {
        return run.error(Error::RootKey, &key_text, "");
    }
    let counts = registry.key_exists(&path).and_then(|exists| match exists {
        true => Ok((registry.count_below(&path)?, registry.values(&path)?.len())),
        false => Err(Error::NoSuchKey),
    });
    let ((keys, values_below), own_values) = match counts {
        Ok(counts) => counts,
        Err(error) => return run.error(error, &key_text, ""),
    };
    let values = values_below + own_values;
    let contents = match (keys, values) {
        (0, 0) => String::new(),
        _ => format!(
            ", {} and {}",
            counted(keys, "subkey", "subkeys"),
            counted(values, "value", "values")
        ),
    };
    if options.dry_run {
        run.console
            .out(format!("would delete {key_text}{contents}\n").as_bytes());
        return 0;
    }
    let question = format!("Delete {key_text}{contents}? [y/N] ");
    if let Err(status) = confirmed(run, options.yes, &question) {
        return status;
    }
    match registry.delete_key(&path, true) {
        Ok(()) => 0,
        Err(error) => run.error(error, &key_text, ""),
    }
}

fn reset<S: Store, C: Console>(
    registry: &mut Registry<S>,
    run: &mut Run<'_, C>,
    options: &Options<'_>,
) -> i32 {
    if !options.operands.is_empty() {
        return run.usage("reset takes no operands");
    }
    let (file, name) = match options.machine {
        true => (HiveFile::Machine, "machine.hive"),
        false => (
            HiveFile::User,
            USER_HIVE_PATH.rsplit('/').next().unwrap_or("user.hive"),
        ),
    };
    if options.dry_run {
        run.console.out(
            format!("would replace {name} with an empty registry file, keeping {name}.damaged\n")
                .as_bytes(),
        );
        return 0;
    }
    let question = format!(
        "Replace the registry file? The old file is kept beside it as {name}.damaged. [y/N] "
    );
    if let Err(status) = confirmed(run, options.yes, &question) {
        return status;
    }
    match registry.reset(file) {
        Ok(()) => {
            run.console
                .out(b"Replaced the registry file. It is empty.\n");
            0
        }
        Err(error) => run.error(error, "", ""),
    }
}

/// Appends `text` as a JSON string.
pub fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}
