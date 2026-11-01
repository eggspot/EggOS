//! MCP prompts (D193, slice 41e): ready-made requests a person picks in their client, each a
//! user message that names the tools to use. They only ask; every tool they name still goes
//! through the agent's grants.

use crate::json::{JsonWriter, Object, Value};

/// A prompt's argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptArgument {
    /// Its name, as `{name}` in the text.
    pub name: &'static str,
    /// What it is.
    pub description: &'static str,
}

/// A prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    /// Its name.
    pub name: &'static str,
    /// Its title, for people.
    pub title: &'static str,
    /// What it asks for.
    pub description: &'static str,
    /// Its arguments, all required.
    pub arguments: &'static [PromptArgument],
    /// The user message, with `{argument}` where an argument goes.
    pub text: &'static str,
}

/// The longest argument value taken, in bytes.
pub const MAX_ARGUMENT_BYTES: usize = 256;

/// Every prompt, in `prompts/list`'s order.
pub const PROMPTS: [Prompt; 4] = [
    Prompt {
        name: "check-the-machine",
        title: "Check this EggOS machine",
        description: "Free space, the network, the devices and what runs, in a few lines.",
        arguments: &[],
        text: "Check this EggOS machine: free space (df), the network (net_status), the \
               devices (device_list) and the running processes (ps). Report anything unusual \
               in a few lines.",
    },
    Prompt {
        name: "look-at-window",
        title: "Look at a window",
        description: "What a window shows and what can be done in it.",
        arguments: &[PromptArgument {
            name: "window",
            description: "The window's id, from wm_list.",
        }],
        text: "Show me window {window}: call wm_show with window {window}, describe what it \
               shows and which controls it has, and say what I could do there next.",
    },
    Prompt {
        name: "tidy-folder",
        title: "Tidy a folder",
        description: "A plan for sorting a folder's files; nothing moves without a yes.",
        arguments: &[PromptArgument {
            name: "folder",
            description: "The folder, such as /home/root/Downloads.",
        }],
        text: "List {folder} with ls, then suggest how to sort its files into folders. Ask me \
               before moving or removing anything.",
    },
    Prompt {
        name: "install-app",
        title: "Install an app",
        description: "Install an app from a file and open it.",
        arguments: &[PromptArgument {
            name: "path",
            description: "The app's file: a Windows .exe or .zip, a macOS .app, .dmg, .pkg or \
                          .zip, or a Linux program.",
        }],
        text: "Install the app at {path} with app_install, open it with app_open, and tell me \
               what its window shows.",
    },
];

/// The prompt `name`.
pub fn find(name: &str) -> Option<&'static Prompt> {
    PROMPTS.iter().find(|prompt| prompt.name == name)
}

/// Writes `prompts/list`'s result.
pub fn write_list(out: &mut JsonWriter<'_>) {
    out.begin_object();
    out.key("prompts");
    out.begin_array();
    for prompt in &PROMPTS {
        out.begin_object();
        out.key("name");
        out.string(prompt.name);
        out.key("title");
        out.string(prompt.title);
        out.key("description");
        out.string(prompt.description);
        out.key("arguments");
        out.begin_array();
        for argument in prompt.arguments {
            out.begin_object();
            out.key("name");
            out.string(argument.name);
            out.key("description");
            out.string(argument.description);
            out.key("required");
            out.bool(true);
            out.end_object();
        }
        out.end_array();
        out.end_object();
    }
    out.end_array();
    out.end_object();
}

/// Why a prompt cannot be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptError {
    /// No prompt has that name.
    Unknown,
    /// An argument is missing, not a string, or longer than [`MAX_ARGUMENT_BYTES`].
    Argument(&'static str),
}

fn argument_text<'b>(
    arguments: Option<Value<'_>>,
    name: &str,
    buffer: &'b mut [u8; MAX_ARGUMENT_BYTES],
) -> Option<&'b str> {
    let value = arguments?.get(name)?.as_str()?;
    let (text, whole) = value.decode_prefix_into(buffer);
    whole.then_some(text)
}

/// Writes `prompts/get`'s result for `prompt` with `arguments`, or says why it cannot.
pub fn write_get(
    prompt: &Prompt,
    arguments: Option<Object<'_>>,
    out: &mut JsonWriter<'_>,
) -> Result<(), PromptError> {
    let arguments = arguments.map(Value::Object);
    let mut values: [[u8; MAX_ARGUMENT_BYTES]; 2] = [[0; MAX_ARGUMENT_BYTES]; 2];
    let mut lengths = [0usize; 2];
    for (index, argument) in prompt.arguments.iter().enumerate() {
        let (Some(buffer), Some(length)) = (values.get_mut(index), lengths.get_mut(index)) else {
            return Err(PromptError::Argument(argument.name));
        };
        let text = argument_text(arguments, argument.name, buffer)
            .ok_or(PromptError::Argument(argument.name))?;
        *length = text.len();
    }
    out.begin_object();
    out.key("description");
    out.string(prompt.description);
    out.key("messages");
    out.begin_array();
    out.begin_object();
    out.key("role");
    out.string("user");
    out.key("content");
    out.begin_object();
    out.key("type");
    out.string("text");
    out.key("text");
    out.begin_string();
    let mut rest = prompt.text;
    while let Some(open) = rest.find('{') {
        out.string_piece(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.string_piece(&rest[open..]);
            rest = "";
            break;
        };
        let name = &after[..close];
        match prompt
            .arguments
            .iter()
            .position(|argument| argument.name == name)
        {
            Some(index) => {
                let value = values
                    .get(index)
                    .zip(lengths.get(index))
                    .and_then(|(value, &length)| value.get(..length))
                    .and_then(|value| core::str::from_utf8(value).ok())
                    .unwrap_or_default();
                out.string_piece(value);
            }
            None => out.string_piece(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    out.string_piece(rest);
    out.end_string();
    out.end_object();
    out.end_object();
    out.end_array();
    out.end_object();
    Ok(())
}

#[cfg(test)]
mod tests;
