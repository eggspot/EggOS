//! A tool call's arguments, checked against what its tool takes (D137): names, types and
//! sizes, decoded into the server's buffers, before anything is resolved or run. A problem is
//! an [`ArgumentProblem`] whose message the agent gets.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::json::{Object, Str, Value};
use crate::paths::MAX_PATH_BYTES;
use crate::tools::{Kind, Operand, Tool};

/// The most bytes `tee` writes in a call (as much as `cat` reads back).
pub const MAX_CONTENT_BYTES: usize = 128 * 1024;
/// The most arguments `run` passes.
pub const MAX_WORDS: usize = 32;
/// The most bytes `run`'s arguments take in all.
pub const WORDS_BYTES: usize = 4096;
/// The longest app name.
pub const MAX_NAME_BYTES: usize = 64;
/// `run`'s time limit when the agent gives none.
pub const DEFAULT_TIME_LIMIT_MS: u64 = 10_000;
/// `run`'s longest time limit, in seconds.
pub const MAX_TIMEOUT_SECONDS: u64 = 60;

/// Why a call's arguments are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentProblem {
    /// `arguments` is not an object.
    NotAnObject,
    /// An argument appears twice.
    Duplicate,
    /// An argument the tool does not take.
    Unknown,
    /// A required argument is missing.
    Missing(&'static str),
    /// An argument has the wrong type.
    WrongType(&'static str),
    /// A path or program is longer than [`MAX_PATH_BYTES`] (a NUL in one is refused once it is
    /// resolved, with the path shown).
    BadPath,
    /// `content` and `content_base64` are both given.
    BothContents,
    /// The content is longer than [`MAX_CONTENT_BYTES`].
    ContentTooLong,
    /// `content_base64` is not base64.
    BadBase64,
    /// An app name is empty, too long, or holds `/`, NUL, or is `.` or `..`.
    BadName,
    /// Too many arguments for `run`, too long, or one holds a NUL.
    BadWords,
    /// `timeout_seconds` is not 1 to 60.
    BadTimeout,
}

impl ArgumentProblem {
    /// What is wrong, for the agent.
    pub fn message(self) -> &'static str {
        match self {
            ArgumentProblem::NotAnObject => "arguments must be an object",
            ArgumentProblem::Duplicate => "an argument appears twice",
            ArgumentProblem::Unknown => "an argument this tool does not take",
            ArgumentProblem::Missing("path") => "path is required",
            ArgumentProblem::Missing("program") => "program is required",
            ArgumentProblem::Missing("name") => "name is required",
            ArgumentProblem::Missing(_) => "a required argument is missing",
            ArgumentProblem::WrongType("path") => "path must be a string",
            ArgumentProblem::WrongType("arguments") => "arguments must be an array of strings",
            ArgumentProblem::WrongType("append" | "confirm") => "append and confirm are booleans",
            ArgumentProblem::WrongType(_) => "an argument has the wrong type",
            ArgumentProblem::BadPath => "the path is longer than 4095 bytes",
            ArgumentProblem::BothContents => "give content or content_base64, not both",
            ArgumentProblem::ContentTooLong => {
                "the content is longer than 128 KiB: write it in pieces with append: true"
            }
            ArgumentProblem::BadBase64 => "content_base64 is not base64",
            ArgumentProblem::BadName => {
                "the name must be 1 to 64 bytes, without / or NUL, and not . or .."
            }
            ArgumentProblem::BadWords => {
                "at most 32 arguments, 4096 bytes in all, none holding a NUL"
            }
            ArgumentProblem::BadTimeout => "timeout_seconds must be a whole number from 1 to 60",
        }
    }
}

/// An app name, kept whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppName {
    bytes: [u8; MAX_NAME_BYTES],
    length: usize,
}

impl AppName {
    /// The name.
    pub fn as_str(&self) -> &str {
        self.bytes
            .get(..self.length)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }

    fn of(text: Str<'_>) -> Result<Self, ArgumentProblem> {
        let mut name = Self {
            bytes: [0; MAX_NAME_BYTES],
            length: 0,
        };
        let decoded = text
            .decode_into(&mut name.bytes)
            .ok_or(ArgumentProblem::BadName)?;
        let valid = !decoded.is_empty()
            && decoded != "."
            && decoded != ".."
            && !decoded.contains(['/', '\0']);
        name.length = decoded.len();
        match valid {
            true => Ok(name),
            false => Err(ArgumentProblem::BadName),
        }
    }
}

/// `run`'s arguments, one after another in a buffer; where each ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    ends: [usize; MAX_WORDS],
    count: usize,
}

impl Default for Words {
    fn default() -> Self {
        Self {
            ends: [0; MAX_WORDS],
            count: 0,
        }
    }
}

impl Words {
    /// How many there are.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Each word, from `buffer`, the one they were decoded into.
    pub fn each<'b>(&self, buffer: &'b [u8]) -> impl Iterator<Item = &'b str> + use<'b, '_> {
        let ends = self.ends.get(..self.count).unwrap_or_default();
        let starts = core::iter::once(0).chain(ends.iter().copied());
        starts.zip(ends.iter().copied()).map(move |(start, end)| {
            buffer
                .get(start..end)
                .and_then(|bytes| core::str::from_utf8(bytes).ok())
                .unwrap_or("")
        })
    }
}

/// A call's checked arguments; texts are in the buffers given to [`check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checked {
    /// The path's (or program's) decoded length, in the path buffer.
    pub path: Option<usize>,
    /// The content's decoded length, in the content buffer.
    pub content: Option<usize>,
    /// The app name.
    pub name: Option<AppName>,
    /// `append`.
    pub append: bool,
    /// `confirm`.
    pub confirm: bool,
    /// `run`'s arguments, in the words buffer.
    pub words: Words,
    /// The time limit, in milliseconds.
    pub time_limit_ms: u64,
}

/// The buffers a call's arguments are decoded into.
#[derive(Debug)]
pub struct Decoded<'s> {
    /// The path or program ([`MAX_PATH_BYTES`] + 1).
    pub path: &'s mut [u8],
    /// `tee`'s content ([`MAX_CONTENT_BYTES`]).
    pub content: &'s mut [u8],
    /// `run`'s arguments ([`WORDS_BYTES`]).
    pub words: &'s mut [u8],
}

/// Checks `arguments` (absent, or anything) for `tool`, decoding them into `decoded`.
pub fn check(
    tool: &Tool,
    arguments: Option<Value<'_>>,
    nested_duplicate: bool,
    decoded: &mut Decoded<'_>,
) -> Result<Checked, ArgumentProblem> {
    if nested_duplicate {
        return Err(ArgumentProblem::Duplicate);
    }
    let arguments = match arguments {
        None => None,
        Some(Value::Object(arguments)) => Some(arguments),
        Some(_) => return Err(ArgumentProblem::NotAnObject),
    };
    if let Some(arguments) = arguments
        && arguments
            .members()
            .any(|(name, _)| !tool.arguments.iter().any(|known| name.equals(known)))
    {
        return Err(ArgumentProblem::Unknown);
    }
    let get = |name: &str| arguments.and_then(|arguments| arguments.get(name));
    let mut checked = Checked {
        time_limit_ms: DEFAULT_TIME_LIMIT_MS,
        ..Checked::default()
    };
    checked.append = flag(get("append"), "append")?;
    checked.confirm = flag(get("confirm"), "confirm")?;
    let path_argument = match (tool.kind, tool.twin.operand) {
        (Kind::Run, _) => Some(("program", None)),
        (_, Operand::Path(path)) => Some((path.name, path.default)),
        _ => None,
    };
    if let Some((name, default)) = path_argument {
        let length = match (get(name), default) {
            (Some(Value::Str(text)), _) => text
                .decode_into(decoded.path)
                .ok_or(ArgumentProblem::BadPath)?
                .len(),
            (Some(_), _) => return Err(ArgumentProblem::WrongType(name)),
            (None, Some(default)) => {
                decoded
                    .path
                    .get_mut(..default.len())
                    .ok_or(ArgumentProblem::BadPath)?
                    .copy_from_slice(default.as_bytes());
                default.len()
            }
            (None, None) => return Err(ArgumentProblem::Missing(name)),
        };
        checked.path = Some(length);
    }
    if tool.kind == Kind::Tee {
        checked.content = Some(content(
            get("content"),
            get("content_base64"),
            decoded.content,
        )?);
    }
    if tool.twin.operand == Operand::Name {
        checked.name = Some(match get("name") {
            Some(Value::Str(text)) => AppName::of(text)?,
            Some(_) => return Err(ArgumentProblem::WrongType("name")),
            None => return Err(ArgumentProblem::Missing("name")),
        });
    }
    if tool.kind == Kind::Run {
        checked.words = words(get("arguments"), decoded.words)?;
        checked.time_limit_ms = match get("timeout_seconds") {
            None => DEFAULT_TIME_LIMIT_MS,
            Some(Value::Number(number)) => number
                .as_u64()
                .filter(|seconds| (1..=MAX_TIMEOUT_SECONDS).contains(seconds))
                .ok_or(ArgumentProblem::BadTimeout)?
                .saturating_mul(1000),
            Some(_) => return Err(ArgumentProblem::BadTimeout),
        };
    }
    Ok(checked)
}

fn flag(value: Option<Value<'_>>, name: &'static str) -> Result<bool, ArgumentProblem> {
    match value {
        None => Ok(false),
        Some(Value::Bool(set)) => Ok(set),
        Some(_) => Err(ArgumentProblem::WrongType(name)),
    }
}

fn content(
    text: Option<Value<'_>>,
    base64: Option<Value<'_>>,
    out: &mut [u8],
) -> Result<usize, ArgumentProblem> {
    let room = out.len().min(MAX_CONTENT_BYTES);
    let out = out.get_mut(..room).unwrap_or_default();
    match (text, base64) {
        (Some(_), Some(_)) => Err(ArgumentProblem::BothContents),
        (Some(Value::Str(text)), None) => text
            .decode_into(out)
            .map(str::len)
            .ok_or(ArgumentProblem::ContentTooLong),
        (None, Some(Value::Str(encoded))) => decode_base64(encoded, out),
        (None, None) => Ok(0),
        (Some(_), None) => Err(ArgumentProblem::WrongType("content")),
        (None, Some(_)) => Err(ArgumentProblem::WrongType("content_base64")),
    }
}

fn sextet(character: char) -> Option<u32> {
    let value = match character {
        'A'..='Z' => u32::from(character) - u32::from('A'),
        'a'..='z' => u32::from(character) - u32::from('a') + 26,
        '0'..='9' => u32::from(character) - u32::from('0') + 52,
        '+' => 62,
        '/' => 63,
        _ => return None,
    };
    Some(value)
}

/// Decodes the base64 text `encoded` (padded or not; white space refused) into `out`.
pub fn decode_base64(encoded: Str<'_>, out: &mut [u8]) -> Result<usize, ArgumentProblem> {
    let mut length = 0;
    let mut bits = 0u32;
    let mut held = 0;
    let mut padding = 0;
    for character in encoded.chars() {
        if character == '=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return Err(ArgumentProblem::BadBase64);
        }
        bits = (bits << 6) | sextet(character).ok_or(ArgumentProblem::BadBase64)?;
        held += 6;
        if held >= 8 {
            held -= 8;
            let byte = (bits >> held) as u8;
            *out.get_mut(length).ok_or(ArgumentProblem::ContentTooLong)? = byte;
            length += 1;
            bits &= (1 << held) - 1;
        }
    }
    if padding > 2 || held >= 6 || bits != 0 {
        return Err(ArgumentProblem::BadBase64);
    }
    Ok(length)
}

fn words(value: Option<Value<'_>>, buffer: &mut [u8]) -> Result<Words, ArgumentProblem> {
    let mut words = Words::default();
    let elements = match value {
        None => return Ok(words),
        Some(Value::Array(array)) => array.elements(),
        Some(_) => return Err(ArgumentProblem::WrongType("arguments")),
    };
    let room = buffer.len().min(WORDS_BYTES);
    let mut used = 0;
    for element in elements {
        let Value::Str(text) = element else {
            return Err(ArgumentProblem::WrongType("arguments"));
        };
        if words.count == MAX_WORDS {
            return Err(ArgumentProblem::BadWords);
        }
        let place = buffer
            .get_mut(used..room)
            .ok_or(ArgumentProblem::BadWords)?;
        let word = text.decode_into(place).ok_or(ArgumentProblem::BadWords)?;
        if word.contains('\0') {
            return Err(ArgumentProblem::BadWords);
        }
        used += word.len();
        *words
            .ends
            .get_mut(words.count)
            .ok_or(ArgumentProblem::BadWords)? = used;
        words.count += 1;
    }
    Ok(words)
}

/// The names of `arguments`' members, as a JSON array, or `"<unparsed N bytes>"` when it is
/// not an object: what the log keeps of a call stopped before its arguments were checked, so
/// no value (a file's content) is ever kept.
pub fn names_only(arguments: Option<Value<'_>>, out: &mut crate::json::JsonWriter<'_>) {
    const MAX_NAME_SHOWN: usize = 32;
    match arguments {
        None => {
            out.begin_array();
            out.end_array();
        }
        Some(Value::Object(object)) => names_of(object, out, MAX_NAME_SHOWN),
        Some(other) => {
            let mut digits = [0u8; 24];
            let mut number = crate::json::JsonWriter::new(&mut digits);
            number.i64(other.raw().len() as i64);
            let length = number.len();
            out.begin_string();
            out.string_piece("<unparsed ");
            out.string_piece(
                digits
                    .get(..length)
                    .and_then(|bytes| core::str::from_utf8(bytes).ok())
                    .unwrap_or(""),
            );
            out.string_piece(" bytes>");
            out.end_string();
        }
    }
}

fn names_of(object: Object<'_>, out: &mut crate::json::JsonWriter<'_>, shown: usize) {
    out.begin_array();
    for (name, _) in object.members() {
        let mut buffer = [0u8; 32];
        let (text, _) = name.decode_prefix_into(buffer.get_mut(..shown).unwrap_or_default());
        out.string(text);
    }
    out.end_array();
}

/// The path or program in `buffer` that [`Checked::path`] measures.
pub fn path_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    let length = checked.path?;
    let bytes = buffer.get(..length.min(MAX_PATH_BYTES + 1))?;
    core::str::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::json::{self, JsonWriter};
    use crate::tools;

    struct Buffers {
        path: Vec<u8>,
        content: Vec<u8>,
        words: Vec<u8>,
    }

    fn buffers() -> Buffers {
        Buffers {
            path: vec![0; MAX_PATH_BYTES + 1],
            content: vec![0; MAX_CONTENT_BYTES],
            words: vec![0; WORDS_BYTES],
        }
    }

    fn checked(tool: &str, arguments: &str) -> (Result<Checked, ArgumentProblem>, Buffers) {
        let mut buffers = buffers();
        let parsed = json::parse(arguments.as_bytes()).unwrap();
        let mut decoded = Decoded {
            path: &mut buffers.path,
            content: &mut buffers.content,
            words: &mut buffers.words,
        };
        let result = check(
            tools::find(tool).unwrap(),
            Some(parsed.value),
            parsed.nested_duplicate,
            &mut decoded,
        );
        (result, buffers)
    }

    #[test]
    fn tee_takes_a_path_text_or_base64_and_two_flags() {
        let (result, buffers) = checked(
            "tee",
            r#"{"path":"/home/root/a","content":"hi\n","append":true}"#,
        );
        let result = result.unwrap();
        assert_eq!(path_text(&result, &buffers.path), Some("/home/root/a"));
        assert_eq!(result.content, Some(3));
        assert_eq!(&buffers.content[..3], b"hi\n");
        assert!(result.append && !result.confirm);
        let (result, buffers) =
            checked("tee", r#"{"path":"/home/root/b","content_base64":"AP/+"}"#);
        assert_eq!(result.unwrap().content, Some(3));
        assert_eq!(&buffers.content[..3], &[0, 0xff, 0xfe]);
        let (result, _) = checked("tee", r#"{"path":"/home/root/c"}"#);
        assert_eq!(result.unwrap().content, Some(0));
        for (arguments, problem) in [
            (r#"{"content":"x"}"#, ArgumentProblem::Missing("path")),
            (
                r#"{"path":"/a","content":"x","content_base64":"eA=="}"#,
                ArgumentProblem::BothContents,
            ),
            (
                r#"{"path":"/a","content_base64":"e A=="}"#,
                ArgumentProblem::BadBase64,
            ),
            (
                r#"{"path":"/a","content_base64":"eA=x"}"#,
                ArgumentProblem::BadBase64,
            ),
            (
                r#"{"path":"/a","content":1}"#,
                ArgumentProblem::WrongType("content"),
            ),
            (
                r#"{"path":"/a","append":"yes"}"#,
                ArgumentProblem::WrongType("append"),
            ),
            (r#"{"path":"/a","mode":"0777"}"#, ArgumentProblem::Unknown),
        ] {
            assert_eq!(checked("tee", arguments).0, Err(problem), "{arguments}");
        }
        let long = format!(
            r#"{{"path":"/a","content":"{}"}}"#,
            "x".repeat(MAX_CONTENT_BYTES + 1)
        );
        assert_eq!(
            checked("tee", &long).0,
            Err(ArgumentProblem::ContentTooLong)
        );
        let exact = format!(
            r#"{{"path":"/a","content":"{}"}}"#,
            "x".repeat(MAX_CONTENT_BYTES)
        );
        assert_eq!(
            checked("tee", &exact).0.unwrap().content,
            Some(MAX_CONTENT_BYTES)
        );
    }

    #[test]
    fn base64_decodes_padded_and_unpadded_and_refuses_the_rest() {
        let decode = |text: &str| {
            let quoted = format!("\"{text}\"");
            let value = json::parse(quoted.as_bytes()).unwrap().value;
            let mut out = [0u8; 16];
            decode_base64(value.as_str().unwrap(), &mut out).map(|length| out[..length].to_vec())
        };
        assert_eq!(decode("aGk="), Ok(b"hi".to_vec()));
        assert_eq!(decode("aGk"), Ok(b"hi".to_vec()));
        assert_eq!(decode("aGVsbG8="), Ok(b"hello".to_vec()));
        assert_eq!(decode(""), Ok(Vec::new()));
        assert_eq!(decode("a"), Err(ArgumentProblem::BadBase64));
        assert_eq!(decode("aGl="), Err(ArgumentProblem::BadBase64));
        assert_eq!(decode("aG=k"), Err(ArgumentProblem::BadBase64));
        assert_eq!(decode("aGk*"), Err(ArgumentProblem::BadBase64));
        assert_eq!(
            decode(&"QUFB".repeat(8)),
            Err(ArgumentProblem::ContentTooLong)
        );
    }

    #[test]
    fn run_takes_a_program_words_and_a_time_limit() {
        let (result, buffers) = checked(
            "run",
            r#"{"program":"/bin/uname","arguments":["-a","x y"],"timeout_seconds":1}"#,
        );
        let result = result.unwrap();
        assert_eq!(path_text(&result, &buffers.path), Some("/bin/uname"));
        let words: Vec<&str> = result.words.each(&buffers.words).collect();
        assert_eq!(words, ["-a", "x y"]);
        assert_eq!(result.time_limit_ms, 1000);
        let (result, _) = checked("run", r#"{"program":"/bin/ps"}"#);
        let result = result.unwrap();
        assert!(result.words.is_empty());
        assert_eq!(result.time_limit_ms, DEFAULT_TIME_LIMIT_MS);
        let many = format!(
            r#"{{"program":"/bin/echo","arguments":[{}]}}"#,
            vec!["\"a\""; 33].join(",")
        );
        let big = format!(
            r#"{{"program":"/bin/echo","arguments":["{}"]}}"#,
            "a".repeat(WORDS_BYTES + 1)
        );
        for (arguments, problem) in [
            (r#"{"arguments":[]}"#, ArgumentProblem::Missing("program")),
            (
                r#"{"program":"/bin/x","arguments":"-a"}"#,
                ArgumentProblem::WrongType("arguments"),
            ),
            (
                r#"{"program":"/bin/x","arguments":[1]}"#,
                ArgumentProblem::WrongType("arguments"),
            ),
            (
                r#"{"program":"/bin/x","arguments":["a\u0000"]}"#,
                ArgumentProblem::BadWords,
            ),
            (
                r#"{"program":"/bin/x","timeout_seconds":0}"#,
                ArgumentProblem::BadTimeout,
            ),
            (
                r#"{"program":"/bin/x","timeout_seconds":61}"#,
                ArgumentProblem::BadTimeout,
            ),
            (
                r#"{"program":"/bin/x","timeout_seconds":1.5}"#,
                ArgumentProblem::BadTimeout,
            ),
            (many.as_str(), ArgumentProblem::BadWords),
            (big.as_str(), ArgumentProblem::BadWords),
        ] {
            assert_eq!(checked("run", arguments).0, Err(problem), "{arguments}");
        }
        let fill = format!(
            r#"{{"program":"/bin/echo","arguments":[{}]}}"#,
            vec!["\"aa\""; 32].join(",")
        );
        assert_eq!(checked("run", &fill).0.unwrap().words.len(), 32);
    }

    #[test]
    fn app_tools_take_a_plain_name() {
        let (result, _) = checked("app_uninstall", r#"{"name":"win-paint","confirm":true}"#);
        let result = result.unwrap();
        assert_eq!(result.name.unwrap().as_str(), "win-paint");
        assert!(result.confirm);
        for name in [r#""""#, r#"".""#, r#""..""#, r#""a/b""#, r#""a\u0000""#] {
            let arguments = format!(r#"{{"name":{name}}}"#);
            assert_eq!(
                checked("app_open", &arguments).0,
                Err(ArgumentProblem::BadName),
                "{name}"
            );
        }
        let long = format!(r#"{{"name":"{}"}}"#, "n".repeat(MAX_NAME_BYTES + 1));
        assert_eq!(checked("app_open", &long).0, Err(ArgumentProblem::BadName));
        assert_eq!(
            checked("app_open", "{}").0,
            Err(ArgumentProblem::Missing("name"))
        );
        assert_eq!(
            checked("app_install", r#"{"path":"/media/host/x.exe"}"#)
                .0
                .unwrap()
                .path,
            Some(17)
        );
    }

    #[test]
    fn only_names_are_kept_of_unchecked_arguments() {
        let shown = |arguments: &str| {
            let value = json::parse(arguments.as_bytes()).unwrap().value;
            let mut buffer = [0u8; 256];
            let mut out = JsonWriter::new(&mut buffer);
            names_only(Some(value), &mut out);
            String::from_utf8(out.as_bytes().to_vec()).unwrap()
        };
        assert_eq!(
            shown(r#"{"path":"/home/x","content":"secret text"}"#),
            r#"["path","content"]"#
        );
        assert_eq!(shown(r#""secret""#), r#""<unparsed 8 bytes>""#);
        let long = format!(r#"{{"{}":1}}"#, "k".repeat(100));
        assert_eq!(shown(&long), format!(r#"["{}"]"#, "k".repeat(32)));
    }
}
