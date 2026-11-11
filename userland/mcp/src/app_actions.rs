//! The actions installed apps declare (D292, CLAUDE.md note 6), published as MCP tools of their
//! own: each is `app_<app>_<action>` in `tools/list`, with its arguments as the input schema,
//! and a call to it runs `app do` through `app_do`, or `app_do_confirmed` for an action that
//! destroys, which asks the user first. Nothing is cached: installing or uninstalling an app
//! changes the list at once.

use crate::json::JsonWriter;
use crate::system::System;
use eggos_app_manifest::NAME_CAPACITY;
use eggos_app_manifest::actions::{self, Action, Effect};

/// What every action tool's name starts with.
pub const PREFIX: &str = "app_";
/// The longest tool name made.
pub const NAME_BYTES: usize = 64;
const MAX_APPS: usize = 64;
const APPLICATIONS: &str = "/Applications";

/// One app's name, kept while its actions file is read.
#[derive(Clone, Copy)]
struct AppName {
    bytes: [u8; NAME_CAPACITY],
    length: usize,
}

impl AppName {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn installed(system: &mut impl System) -> ([AppName; MAX_APPS], usize) {
    let mut names = [AppName {
        bytes: [0; NAME_CAPACITY],
        length: 0,
    }; MAX_APPS];
    let mut count = 0;
    system.folder_entries(APPLICATIONS, &mut |name| {
        if count < MAX_APPS && name.len() <= NAME_CAPACITY && !name.starts_with(b".") {
            names[count].bytes[..name.len()].copy_from_slice(name);
            names[count].length = name.len();
            count += 1;
        }
    });
    (names, count)
}

fn read_actions<'a>(system: &mut impl System, app: &[u8], file: &'a mut [u8]) -> &'a [u8] {
    let mut path = [0u8; 128];
    let parts: [&[u8]; 4] = [APPLICATIONS.as_bytes(), b"/", app, b"/"];
    let mut length = 0;
    for part in parts.iter().chain([actions::FILE_NAME.as_bytes()].iter()) {
        let Some(room) = path.get_mut(length..length + part.len()) else {
            return &[];
        };
        room.copy_from_slice(part);
        length += part.len();
    }
    let Ok(path) = core::str::from_utf8(&path[..length]) else {
        return &[];
    };
    match system.read_file(path, file) {
        Ok(read) => &file[..read],
        Err(_) => &[],
    }
}

/// The tool name of `app`'s `action`: [`PREFIX`], the app's name and the action's, joined by
/// `_`, every byte but letters, digits, `-` and `_` turned into `-` and capitals lowered; `None`
/// past [`NAME_BYTES`].
pub fn tool_name(app: &[u8], action: &[u8], out: &mut [u8; NAME_BYTES]) -> Option<usize> {
    let mut length = 0;
    for &byte in PREFIX.as_bytes().iter().chain(app).chain(b"_").chain(action) {
        let byte = match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => byte,
            b'A'..=b'Z' => byte.to_ascii_lowercase(),
            _ => b'-',
        };
        *out.get_mut(length)? = byte;
        length += 1;
    }
    Some(length)
}

fn write_tool(out: &mut JsonWriter<'_>, name: &str, action: &Action<'_>) {
    out.begin_object();
    out.key("name");
    out.string(name);
    out.key("title");
    out.string_lossy(action.title);
    out.key("description");
    out.string_lossy(action.description);
    out.key("inputSchema");
    out.begin_object();
    out.key("type");
    out.string("object");
    out.key("properties");
    out.begin_object();
    for argument in action.arguments() {
        out.key(core::str::from_utf8(argument.name).unwrap_or("argument"));
        out.begin_object();
        out.key("type");
        out.string(argument.kind.json_type());
        out.key("description");
        out.string_lossy(argument.description);
        out.end_object();
    }
    out.end_object();
    out.key("required");
    out.begin_array();
    for argument in action.arguments().filter(|argument| argument.required) {
        out.string_lossy(argument.name);
    }
    out.end_array();
    out.key("additionalProperties");
    out.bool(false);
    out.end_object();
    out.key("annotations");
    out.begin_object();
    out.key("title");
    out.string_lossy(action.title);
    out.key("readOnlyHint");
    out.bool(action.effect == Effect::Read);
    out.key("destructiveHint");
    out.bool(action.effect == Effect::Destroy);
    out.key("idempotentHint");
    out.bool(action.effect == Effect::Read);
    out.key("openWorldHint");
    out.bool(false);
    out.end_object();
    out.end_object();
}

/// Writes every installed app's actions as tools, inside `tools/list`'s array.
pub fn write_tools(out: &mut JsonWriter<'_>, system: &mut impl System, file: &mut [u8]) {
    let (names, count) = installed(system);
    for app in &names[..count] {
        let app = app.as_bytes();
        let bytes = read_actions(system, app, file);
        for action in actions::actions(bytes) {
            let mut name = [0u8; NAME_BYTES];
            let Some(length) = tool_name(app, action.name, &mut name) else {
                continue;
            };
            let Ok(name) = core::str::from_utf8(&name[..length]) else {
                continue;
            };
            write_tool(out, name, &action);
        }
    }
}

/// The app and action a tool name stands for, and the generic tool its calls go through.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub app: [u8; NAME_CAPACITY],
    pub app_length: usize,
    pub action: [u8; actions::NAME_CAPACITY],
    pub action_length: usize,
    pub through: &'static str,
}

/// The installed app action whose tool is `name`, if any.
pub fn find(system: &mut impl System, name: &str, file: &mut [u8]) -> Option<Found> {
    if !name.starts_with(PREFIX) {
        return None;
    }
    let (names, count) = installed(system);
    for app in &names[..count] {
        let app = app.as_bytes();
        let bytes = read_actions(system, app, file);
        for action in actions::actions(bytes) {
            let mut made = [0u8; NAME_BYTES];
            let Some(length) = tool_name(app, action.name, &mut made) else {
                continue;
            };
            if &made[..length] != name.as_bytes() {
                continue;
            }
            let mut found = Found {
                app: [0; NAME_CAPACITY],
                app_length: app.len(),
                action: [0; actions::NAME_CAPACITY],
                action_length: action.name.len(),
                through: match action.effect {
                    Effect::Destroy => "app_do_confirmed",
                    _ => "app_do",
                },
            };
            found.app[..app.len()].copy_from_slice(app);
            found.action[..action.name.len()].copy_from_slice(action.name);
            return Some(found);
        }
    }
    None
}

/// The parameters of the call to [`Found::through`] that a call of the action's own tool with
/// `arguments` (its JSON object's text, or none) becomes, written into `out`: their length.
pub fn redirected(found: &Found, arguments: Option<&[u8]>, out: &mut [u8]) -> usize {
    let mut writer = JsonWriter::new(out);
    writer.begin_object();
    writer.key("name");
    writer.string(found.through);
    writer.key("arguments");
    writer.begin_object();
    writer.key("name");
    writer.string_lossy(&found.app[..found.app_length]);
    writer.key("action");
    writer.string_lossy(&found.action[..found.action_length]);
    if let Some(arguments) = arguments {
        writer.key("values");
        writer.string_lossy(arguments);
    }
    writer.end_object();
    writer.end_object();
    writer.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_keep_letters_digits_and_dashes() {
        let mut out = [0u8; NAME_BYTES];
        let length = tool_name(b"Paint Pro", b"export-png", &mut out).unwrap();
        assert_eq!(&out[..length], b"app_paint-pro_export-png");
        assert!(tool_name(&[b'a'; 40], &[b'b'; 32], &mut out).is_none());
    }
}
