//! The grant prompt's protocol (D193, slice 41c3): the server starts `settings --grant-prompt`
//! on EggOS's own screen, writes one [`Question`] as a JSON line on its stdin and reads one
//! [`PromptAnswer`] word on its stdout. Both sides use this module, so the words and fields
//! are one set.

use crate::grants::Grant;
use crate::json::{self, JsonWriter, Object, Str, Value};

/// The program that shows the prompt.
pub const PROMPT_PROGRAM: &str = "/bin/settings";
/// The word that starts the program as the prompt.
pub const PROMPT_OPTION: &str = "--grant-prompt";
/// How long the prompt waits for the person before it answers [`PromptAnswer::Timeout`].
pub const GRANT_PROMPT_TIMEOUT_S: u64 = 90;
/// The longest question line the server writes, its newline included.
pub const QUESTION_BYTES: usize = 2048;
/// The longest answer line the prompt writes, its newline included.
pub const ANSWER_BYTES: usize = 16;

/// What the person answered on the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Allow this call only.
    Once,
    /// Allow this call and the class's later calls until the client disconnects.
    Session,
    /// Allow this call; the prompt wrote `allow` for the class into the store first.
    Always,
    /// Refuse this call.
    Deny,
    /// Nobody answered within [`GRANT_PROMPT_TIMEOUT_S`].
    Timeout,
}

impl PromptAnswer {
    /// Every answer, in the order the protocol lists them.
    pub const ALL: [PromptAnswer; 5] = [
        PromptAnswer::Once,
        PromptAnswer::Session,
        PromptAnswer::Always,
        PromptAnswer::Deny,
        PromptAnswer::Timeout,
    ];

    /// Its word on the prompt's stdout.
    pub fn word(self) -> &'static str {
        match self {
            PromptAnswer::Once => "once",
            PromptAnswer::Session => "session",
            PromptAnswer::Always => "always",
            PromptAnswer::Deny => "deny",
            PromptAnswer::Timeout => "timeout",
        }
    }

    /// The answer an answer line names (its trailing newline allowed), `None` for anything
    /// else.
    pub fn from_line(line: &[u8]) -> Option<Self> {
        let word = line.strip_suffix(b"\n").unwrap_or(line);
        Self::ALL
            .into_iter()
            .find(|answer| answer.word().as_bytes() == word)
    }
}

/// Whether the prompt offers `Always` for a call of `class`: only `view` and `write` calls
/// that need no yes of their own; `run` and `install` are never granted from a prompt.
pub fn offers_always(class: Option<Grant>, destructive: bool) -> bool {
    !destructive && matches!(class, Some(Grant::View | Grant::Write))
}

/// The question the server writes for the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Question<'q> {
    /// The asking agent's id.
    pub agent: &'q str,
    /// The agent's name, its id when the store names it nothing else.
    pub name: &'q str,
    /// The tool's name.
    pub tool: &'q str,
    /// The tool's title.
    pub title: &'q str,
    /// The grant class the tool needs, `None` for a tool that needs none.
    pub class: Option<Grant>,
    /// Whether the call needs a yes by itself (it removes or replaces something).
    pub destructive: bool,
    /// The call's arguments as the audit record shows them: a JSON object's text.
    pub arguments: &'q [u8],
    /// How long the prompt waits, in seconds.
    pub timeout_s: u64,
}

/// Writes `question` as one JSON line into `out`; returns its length, 0 when it does not fit
/// or `arguments` is not a JSON object.
pub fn write_question(question: &Question<'_>, out: &mut [u8]) -> usize {
    let arguments_are_an_object = matches!(
        json::parse(question.arguments).map(|parsed| parsed.value),
        Ok(Value::Object(_))
    );
    if !arguments_are_an_object {
        return 0;
    }
    let mut writer = JsonWriter::new(out);
    writer.begin_object();
    writer.key("agent");
    writer.string(question.agent);
    writer.key("name");
    writer.string(question.name);
    writer.key("tool");
    writer.string(question.tool);
    writer.key("title");
    writer.string(question.title);
    writer.key("class");
    match question.class {
        Some(class) => writer.string(class.name()),
        None => writer.null(),
    }
    writer.key("destructive");
    writer.bool(question.destructive);
    writer.key("arguments");
    writer.raw(question.arguments);
    writer.key("timeout_s");
    writer.u64(question.timeout_s);
    writer.end_object();
    writer.end_line();
    match writer.overflowed() {
        true => 0,
        false => writer.len(),
    }
}

/// A question as the prompt read it: its strings still escaped, to decode where they are
/// shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionRead<'a> {
    /// The asking agent's id.
    pub agent: Str<'a>,
    /// The agent's name.
    pub name: Str<'a>,
    /// The tool's name.
    pub tool: Str<'a>,
    /// The tool's title.
    pub title: Str<'a>,
    /// The grant class the tool needs.
    pub class: Option<Grant>,
    /// Whether the call needs a yes by itself.
    pub destructive: bool,
    /// The call's arguments, in the order the server wrote them.
    pub arguments: Object<'a>,
    /// How long the prompt waits, in seconds ([`GRANT_PROMPT_TIMEOUT_S`] when absent or 0).
    pub timeout_s: u64,
}

/// Reads a question line (its trailing newline allowed); `None` when it is not one.
pub fn read_question(line: &[u8]) -> Option<QuestionRead<'_>> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let Value::Object(question) = json::parse(line).ok()?.value else {
        return None;
    };
    let text = |key: &str| question.get(key).and_then(Value::as_str);
    let class = match question.get("class")? {
        Value::Null => None,
        Value::Str(name) => Some(Grant::named(name.raw())?),
        _ => return None,
    };
    let destructive = match question.get("destructive") {
        Some(Value::Bool(destructive)) => destructive,
        None => false,
        Some(_) => return None,
    };
    let timeout_s = match question.get("timeout_s") {
        Some(Value::Number(seconds)) => seconds.as_u64()?,
        None => 0,
        Some(_) => return None,
    };
    Some(QuestionRead {
        agent: text("agent")?,
        name: text("name").or(text("agent"))?,
        tool: text("tool")?,
        title: text("title")?,
        class,
        destructive,
        arguments: question.get("arguments")?.as_object()?,
        timeout_s: match timeout_s {
            0 => GRANT_PROMPT_TIMEOUT_S,
            seconds => seconds,
        },
    })
}

#[cfg(test)]
mod tests;
