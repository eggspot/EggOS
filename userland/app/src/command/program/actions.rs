//! `app actions` and `app do` (D292): the actions installed apps declare in `app.actions`,
//! listed, and one of them run with its arguments' values as whole words, never through a
//! shell, in the app's folder, its output and exit status passed through.

use super::{FAILED, USAGE_ERROR, report, say};
use crate::installer::{self, APPLICATIONS};
use crate::linux::{LinuxFiles, STANDARD_OUTPUT, write_all};
use core::ffi::CStr;
use eggos_app_manifest::actions::{self, Action, Effect, ExpandError};
use eggos_commands::errors;
use eggos_mcp::json::{self, JsonWriter, Value};
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{ChildStack, Spawn, spawn};
use eggos_runtime::syscall::{self, number};

const PATH_BYTES: usize = 512;
const VALUE_BYTES: usize = 4096;
const WORDS_BYTES: usize = 8192;

static mut FILE: [u8; actions::LARGEST] = [0; actions::LARGEST];
static mut STACK: ChildStack = ChildStack::new();

fn file_buffer() -> &'static mut [u8; actions::LARGEST] {
    // SAFETY: the program has one thread and reads one actions file at a time through this
    // buffer, finishing with each before the next read.
    unsafe { &mut *(&raw mut FILE) }
}

fn path_of(app: &[u8], out: &mut [u8; PATH_BYTES]) -> Option<usize> {
    let parts: [&[u8]; 4] = [APPLICATIONS.as_bytes(), b"/", app, b"/"];
    let mut length = 0;
    for part in parts.iter().chain([actions::FILE_NAME.as_bytes()].iter()) {
        out.get_mut(length..length + part.len())?.copy_from_slice(part);
        length += part.len();
    }
    Some(length)
}

/// Reads `app`'s actions file: its bytes, empty when it has none.
fn read(files: &mut LinuxFiles, app: &[u8]) -> &'static [u8] {
    use crate::files::Files;
    let mut path = [0u8; PATH_BYTES];
    let Some(length) = path_of(app, &mut path) else {
        return &[];
    };
    let Ok(handle) = files.open(&path[..length]) else {
        return &[];
    };
    let buffer = file_buffer();
    let mut filled = 0;
    while filled < buffer.len() {
        match files.read(handle, &mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => filled += count,
        }
    }
    files.close(handle);
    &buffer[..filled]
}

fn write_record(out: &mut JsonWriter<'_>, app: &[u8], action: &Action<'_>) {
    out.begin_object();
    out.key("app");
    out.string_lossy(app);
    out.key("action");
    out.string_lossy(action.name);
    out.key("title");
    out.string_lossy(action.title);
    out.key("description");
    out.string_lossy(action.description);
    out.key("effect");
    out.string(action.effect.name());
    out.key("arguments");
    out.begin_array();
    for argument in action.arguments() {
        out.begin_object();
        out.key("name");
        out.string_lossy(argument.name);
        out.key("type");
        out.string(argument.kind.json_type());
        out.key("required");
        out.bool(argument.required);
        out.key("description");
        out.string_lossy(argument.description);
        out.end_object();
    }
    out.end_array();
    out.end_object();
}

/// `app actions`: one line an action, `<app> <action> (<effect>): <title>`, or one JSON record
/// a line with `--json`; only `name`'s with a name.
pub fn list(name: Option<&[u8]>, json: bool) -> i32 {
    let mut files = LinuxFiles::new();
    let mut names = [[0u8; eggos_app_manifest::NAME_CAPACITY]; 64];
    let mut lengths = [0usize; 64];
    let mut count = 0;
    let listed = installer::for_each_installed(&mut files, |app| {
        let bytes = app.name.as_bytes();
        if count < names.len() && name.is_none_or(|wanted| wanted == bytes) {
            names[count][..bytes.len()].copy_from_slice(bytes);
            lengths[count] = bytes.len();
            count += 1;
        }
    });
    if listed.is_err() || (name.is_some() && count == 0) {
        report(json, &errors::NO_SUCH_APP, "no such app is installed");
        return FAILED;
    }
    for index in 0..count {
        let app = &names[index][..lengths[index]];
        for action in actions::actions(read(&mut files, app)) {
            match json {
                true => {
                    let mut record = [0u8; 4096];
                    let mut out = JsonWriter::new(&mut record);
                    write_record(&mut out, app, &action);
                    out.end_line();
                    write_all(STANDARD_OUTPUT, out.as_bytes());
                }
                false => {
                    write_all(STANDARD_OUTPUT, app);
                    write_all(STANDARD_OUTPUT, b" ");
                    write_all(STANDARD_OUTPUT, action.name);
                    write_all(STANDARD_OUTPUT, b" (");
                    write_all(STANDARD_OUTPUT, action.effect.name().as_bytes());
                    write_all(STANDARD_OUTPUT, b"): ");
                    write_all(STANDARD_OUTPUT, action.title);
                    write_all(STANDARD_OUTPUT, b"\n");
                }
            }
        }
    }
    0
}

/// The values `--values` holds, a JSON object of strings, numbers and booleans, appended to
/// `pairs` with their text in `text`.
fn values_from_json<'a>(
    values: &[u8],
    text: &'a mut [u8; VALUE_BYTES],
    pairs: &mut [(&'a [u8], &'a [u8]); 2 * actions::MAX_ARGUMENTS],
    count: &mut usize,
) -> Result<(), &'static str> {
    let parsed = json::parse(values).map_err(|_| "--values is not JSON")?;
    let Value::Object(object) = parsed.value else {
        return Err("--values is not a JSON object");
    };
    let mut spans = [(0usize, 0usize, 0usize, 0usize); 2 * actions::MAX_ARGUMENTS];
    let mut used = 0;
    let mut found = 0;
    for (key, value) in object.members() {
        if found == spans.len() {
            return Err("--values holds too many values");
        }
        let (key_text, _) = key.decode_prefix_into(&mut text[used..]);
        let key_start = used;
        used += key_text.len();
        let value_start = used;
        let room = &mut text[used..];
        let length = match value {
            Value::Str(string) => string.decode_prefix_into(room).0.len(),
            Value::Number(number) => {
                let raw = number.raw();
                room.get_mut(..raw.len()).ok_or("--values is too long")?.copy_from_slice(raw);
                raw.len()
            }
            Value::Bool(flag) => {
                let raw: &[u8] = if flag { b"true" } else { b"false" };
                room.get_mut(..raw.len()).ok_or("--values is too long")?.copy_from_slice(raw);
                raw.len()
            }
            _ => return Err("--values holds a value that is not text, a number or a boolean"),
        };
        used += length;
        spans[found] = (key_start, value_start - key_start, value_start, length);
        found += 1;
    }
    let text: &'a [u8] = text;
    for &(key_at, key_length, value_at, value_length) in &spans[..found] {
        pairs[*count] = (&text[key_at..key_at + key_length], &text[value_at..value_at + value_length]);
        *count += 1;
    }
    Ok(())
}

/// `app do`: runs `action_name` of `app` with `given` and `values`' arguments, refusing an
/// action that destroys without `confirmed`; returns the program's exit status.
pub fn run(
    app: &[u8],
    action_name: &[u8],
    values: Option<&[u8]>,
    given: &[(&[u8], &[u8])],
    confirmed: bool,
) -> i32 {
    let mut files = LinuxFiles::new();
    let mut installed = false;
    let _ = installer::for_each_installed(&mut files, |found| installed |= found.name.as_bytes() == app);
    if !installed {
        report(false, &errors::NO_SUCH_APP, "no such app is installed");
        return FAILED;
    }
    let Some(action) = actions::actions(read(&mut files, app)).find(|action| action.name == action_name) else {
        report(false, &errors::NO_SUCH_ACTION, "the app offers no such action");
        return FAILED;
    };
    if action.effect == Effect::Destroy && !confirmed {
        report(
            false,
            &errors::NEEDS_CONFIRMATION,
            "the action deletes or overwrites: run it again with --confirmed once the user agreed",
        );
        return FAILED;
    }
    let mut text = [0u8; VALUE_BYTES];
    let mut pairs = [(&b""[..], &b""[..]); 2 * actions::MAX_ARGUMENTS];
    let mut count = 0;
    for &pair in given {
        pairs[count] = pair;
        count += 1;
    }
    if let Some(values) = values
        && let Err(problem) = values_from_json(values, &mut text, &mut pairs, &mut count)
    {
        report(false, &errors::USAGE, problem);
        return USAGE_ERROR;
    }
    let mut words = [0u8; WORDS_BYTES];
    let word_count = match actions::expand(&action, &pairs[..count], &mut words) {
        Ok(count) => count,
        Err(error) => {
            let line = match error {
                ExpandError::Missing => "a required argument has no value",
                ExpandError::WrongType => "a value is not of its argument's type",
                ExpandError::Unknown => "a value names no argument of the action",
                ExpandError::TooLong => "the action's words are too long",
                ExpandError::BadPlaceholder => "the app's action names an argument it does not have",
            };
            report(false, &errors::USAGE, line);
            return USAGE_ERROR;
        }
    };
    execute(app, &words, word_count)
}

fn execute(app: &[u8], words: &[u8], count: usize) -> i32 {
    let mut folder = [0u8; PATH_BYTES];
    let parts: [&[u8]; 3] = [APPLICATIONS.as_bytes(), b"/", app];
    let mut length = 0;
    for part in parts {
        folder[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    }
    let folder_end = length;
    folder[folder_end] = 0;
    let first_end = words.iter().position(|&byte| byte == 0).unwrap_or(0);
    let first = &words[..first_end];
    let mut program = [0u8; PATH_BYTES];
    let program_length = match first.first() {
        Some(b'/') => {
            program[..first.len()].copy_from_slice(first);
            first.len()
        }
        _ => {
            let total = folder_end + 1 + first.len();
            if total >= PATH_BYTES {
                report(false, &errors::FAILED, "the action's program path is too long");
                return FAILED;
            }
            program[..folder_end].copy_from_slice(&folder[..folder_end]);
            program[folder_end] = b'/';
            program[folder_end + 1..total].copy_from_slice(first);
            total
        }
    };
    let (Ok(program), Ok(directory)) = (
        CStr::from_bytes_with_nul(&program[..program_length + 1]),
        CStr::from_bytes_with_nul(&folder[..folder_end + 1]),
    ) else {
        report(false, &errors::FAILED, "the action's program path is not valid");
        return FAILED;
    };
    let mut pointers = [core::ptr::null::<u8>(); actions::MAX_WORDS + 1];
    let mut at = 0;
    for slot in pointers.iter_mut().take(count) {
        *slot = words[at..].as_ptr();
        at += words[at..].iter().position(|&byte| byte == 0).unwrap_or(0) + 1;
    }
    pointers[0] = program.as_ptr().cast();
    let environment = [b"PATH=/bin\0".as_ptr(), core::ptr::null()];
    let request = Spawn::new(program, &pointers[..count + 1], &environment, [0, 1, 2]).in_directory(directory);
    // SAFETY: the program has one thread and starts one child at a time on this stack, which
    // `spawn` uses only until the child's `execve`.
    let stack = unsafe { &mut *(&raw mut STACK) };
    let child = match spawn(stack, &request) {
        Ok(child) => child,
        Err(error) => {
            let mut line = crate::messages::Line::new();
            line.push_bytes(b"could not run the action: ");
            line.push_bytes(error.message());
            report(false, super::errno_error(error), line.as_str());
            return FAILED;
        }
    };
    let mut status = 0i32;
    loop {
        // SAFETY: `wait4` writes the child's status into `status`, which this function owns.
        let waited = unsafe { syscall::call(number::WAIT4, [child, &raw mut status as usize, 0, 0, 0, 0]) };
        match Errno::result(waited) {
            Ok(_) => break,
            Err(Errno::EINTR) => {}
            Err(_) => return FAILED,
        }
    }
    let exited = status & 0x7f == 0;
    match exited {
        true => (status >> 8) & 0xff,
        false => 128 + (status & 0x7f),
    }
}

#[allow(dead_code)]
fn usage_line() {
    say(STANDARD_OUTPUT, "app do <name> <action> [<argument>=<value>]...");
}
