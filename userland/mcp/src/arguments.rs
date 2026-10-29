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
use crate::tools::{DEFAULT_IMAGE_WIDTH, Kind, MAX_IMAGE_WIDTH, MIN_IMAGE_WIDTH, Operand, Tool};

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
/// The most bytes `wm_type` types in a call, printable ASCII only: the audit record escapes
/// the logged arguments again, up to four bytes for a `\` or `"`, and its field keeps 512
/// bytes, so the whole text is always logged.
pub const MAX_TYPED_TEXT_BYTES: usize = 96;
/// The most bytes of `wm_press`'s keys.
pub const MAX_KEYS_BYTES: usize = 32;
/// The largest window id (`wm` reads at most nine digits).
pub const MAX_WINDOW_ID: u64 = 999_999_999;
/// The largest coordinate in a window.
pub const MAX_COORDINATE: u64 = 65_535;
/// The most wheel notches one scroll turns, either way (the window server's own limit).
pub const MAX_NOTCHES: u64 = 16;

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
    /// `window` is not a whole number from 0 to [`MAX_WINDOW_ID`].
    BadWindow,
    /// `x` or `y` is not a whole number from 0 to [`MAX_COORDINATE`], or only one is given.
    BadPoint,
    /// `button` is not `left`, `right` or `middle`.
    BadButton,
    /// `text` is empty, longer than [`MAX_TYPED_TEXT_BYTES`] or not printable ASCII.
    BadText,
    /// `keys` is empty, longer than [`MAX_KEYS_BYTES`] or holds more than letters, digits
    /// and `+`.
    BadKeys,
    /// `dy` is 0 or more than [`MAX_NOTCHES`] either way.
    BadNotches,
    /// `max_width` is outside [`MIN_IMAGE_WIDTH`] to [`MAX_IMAGE_WIDTH`].
    BadImageWidth,
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
            ArgumentProblem::Missing("window") => "window is required: an id wm_list shows",
            ArgumentProblem::Missing("x") => "x and y are required",
            ArgumentProblem::Missing("text") => "text is required",
            ArgumentProblem::Missing("keys") => "keys is required",
            ArgumentProblem::Missing("dy") => "dy is required",
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
            ArgumentProblem::BadWindow => {
                "window must be a window id as wm_list shows it: a whole number from 0 to \
                 999999999"
            }
            ArgumentProblem::BadPoint => {
                "x and y are whole numbers of window pixels from 0 to 65535, given together"
            }
            ArgumentProblem::BadButton => "button is left, right or middle",
            ArgumentProblem::BadText => {
                "text must be 1 to 96 printable ASCII characters; press Enter and other keys \
                 with wm_press"
            }
            ArgumentProblem::BadKeys => {
                "keys must be one key after any of ctrl+, shift+, alt+ and super+, such as \
                 ctrl+s: 1 to 32 letters, digits and +"
            }
            ArgumentProblem::BadNotches => {
                "dy must be a whole number of notches from -16 to 16, not 0"
            }
            ArgumentProblem::BadImageWidth => "max_width must be a whole number from 16 to 1568",
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

/// A mouse button `wm_click` presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The left button, the default.
    Left,
    /// The right button.
    Right,
    /// The middle button.
    Middle,
}

impl Button {
    /// Every button.
    pub const ALL: [Button; 3] = [Button::Left, Button::Right, Button::Middle];

    /// Its name, as the tool and `wm click --button` take it.
    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
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
    /// The window's id.
    pub window: Option<u32>,
    /// A point in the window's content, in window pixels (`x`, `y`).
    pub point: Option<(u32, u32)>,
    /// `wm_click`'s button.
    pub button: Option<Button>,
    /// `wm_press`'s keys' length, in the words buffer.
    pub keys: Option<usize>,
    /// `wm_type`'s text's length, in the words buffer.
    pub text: Option<usize>,
    /// `wm_scroll`'s notches (`dy`), up when positive.
    pub notches: Option<i32>,
    /// `wm_capture`'s widest image, in pixels.
    pub max_width: Option<u32>,
}

/// The buffers a call's arguments are decoded into.
#[derive(Debug)]
pub struct Decoded<'s> {
    /// The path or program ([`MAX_PATH_BYTES`] + 1).
    pub path: &'s mut [u8],
    /// `tee`'s content ([`MAX_CONTENT_BYTES`]).
    pub content: &'s mut [u8],
    /// `run`'s arguments, `wm_type`'s text or `wm_press`'s keys ([`WORDS_BYTES`]).
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
    if tool.twin.operand == Operand::Window {
        window_arguments(tool.kind, &get, decoded.words, &mut checked)?;
    }
    Ok(checked)
}

fn whole(
    value: Option<Value<'_>>,
    most: u64,
    problem: ArgumentProblem,
) -> Result<Option<u64>, ArgumentProblem> {
    match value {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|&value| value <= most)
            .map(Some)
            .ok_or(problem),
        Some(_) => Err(problem),
    }
}

fn point(
    x: Option<Value<'_>>,
    y: Option<Value<'_>>,
) -> Result<Option<(u32, u32)>, ArgumentProblem> {
    let x = whole(x, MAX_COORDINATE, ArgumentProblem::BadPoint)?;
    let y = whole(y, MAX_COORDINATE, ArgumentProblem::BadPoint)?;
    match (x, y) {
        (Some(x), Some(y)) => Ok(Some((x as u32, y as u32))),
        (None, None) => Ok(None),
        _ => Err(ArgumentProblem::BadPoint),
    }
}

fn short_text(
    value: Option<Value<'_>>,
    name: &'static str,
    buffer: &mut [u8],
    most: usize,
    allowed: fn(u8) -> bool,
    problem: ArgumentProblem,
) -> Result<usize, ArgumentProblem> {
    let text = match value {
        Some(Value::Str(text)) => text,
        Some(_) => return Err(problem),
        None => return Err(ArgumentProblem::Missing(name)),
    };
    let room = buffer.get_mut(..most).ok_or(problem)?;
    let decoded = text.decode_into(room).ok_or(problem)?;
    match !decoded.is_empty() && decoded.bytes().all(allowed) {
        true => Ok(decoded.len()),
        false => Err(problem),
    }
}

fn window_arguments<'a>(
    kind: Kind,
    get: &impl Fn(&str) -> Option<Value<'a>>,
    words: &mut [u8],
    checked: &mut Checked,
) -> Result<(), ArgumentProblem> {
    let window = whole(get("window"), MAX_WINDOW_ID, ArgumentProblem::BadWindow)?
        .ok_or(ArgumentProblem::Missing("window"))?;
    checked.window = Some(window as u32);
    match kind {
        Kind::WmCapture => {
            let width = whole(
                get("max_width"),
                u64::from(MAX_IMAGE_WIDTH),
                ArgumentProblem::BadImageWidth,
            )?
            .unwrap_or(u64::from(DEFAULT_IMAGE_WIDTH));
            if width < u64::from(MIN_IMAGE_WIDTH) {
                return Err(ArgumentProblem::BadImageWidth);
            }
            checked.max_width = Some(width as u32);
        }
        Kind::WmClick => {
            checked.point = Some(point(get("x"), get("y"))?.ok_or(ArgumentProblem::Missing("x"))?);
            checked.button = Some(match get("button") {
                None => Button::Left,
                Some(Value::Str(name)) => Button::ALL
                    .into_iter()
                    .find(|button| name.equals(button.name()))
                    .ok_or(ArgumentProblem::BadButton)?,
                Some(_) => return Err(ArgumentProblem::BadButton),
            });
        }
        Kind::WmType => {
            checked.text = Some(short_text(
                get("text"),
                "text",
                words,
                MAX_TYPED_TEXT_BYTES,
                |byte| (b' '..=b'~').contains(&byte),
                ArgumentProblem::BadText,
            )?);
        }
        Kind::WmPress => {
            checked.keys = Some(short_text(
                get("keys"),
                "keys",
                words,
                MAX_KEYS_BYTES,
                |byte| byte.is_ascii_alphanumeric() || byte == b'+',
                ArgumentProblem::BadKeys,
            )?);
        }
        Kind::WmScroll => {
            let notches = match get("dy") {
                None => return Err(ArgumentProblem::Missing("dy")),
                Some(Value::Number(number)) => number
                    .as_i64()
                    .filter(|notches| *notches != 0 && notches.unsigned_abs() <= MAX_NOTCHES)
                    .ok_or(ArgumentProblem::BadNotches)?,
                Some(_) => return Err(ArgumentProblem::BadNotches),
            };
            checked.notches = Some(notches as i32);
            checked.point = point(get("x"), get("y"))?;
        }
        _ => {}
    }
    Ok(())
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

/// `wm_type`'s text in `buffer`, the words buffer, that [`Checked::text`] measures.
pub fn typed_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.text?)?).ok()
}

/// `wm_press`'s keys in `buffer`, the words buffer, that [`Checked::keys`] measures.
pub fn keys_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.keys?)?).ok()
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
    fn window_tools_take_a_window_and_their_own_words() {
        let (result, _) = checked("wm_capture", r#"{"window":129}"#);
        let result = result.unwrap();
        assert_eq!(result.window, Some(129));
        assert_eq!(result.max_width, Some(DEFAULT_IMAGE_WIDTH));
        let (result, _) = checked("wm_capture", r#"{"window":0,"max_width":16}"#);
        assert_eq!(result.unwrap().max_width, Some(MIN_IMAGE_WIDTH));
        let (result, _) = checked("wm_click", r#"{"window":5,"x":0,"y":65535}"#);
        let result = result.unwrap();
        assert_eq!(result.point, Some((0, 65535)));
        assert_eq!(result.button, Some(Button::Left));
        let (result, _) = checked("wm_click", r#"{"window":5,"x":1,"y":2,"button":"middle"}"#);
        assert_eq!(result.unwrap().button, Some(Button::Middle));
        let (result, buffers) = checked("wm_type", r#"{"window":5,"text":" ~a\"\\"}"#);
        let result = result.unwrap();
        assert_eq!(typed_text(&result, &buffers.words), Some(" ~a\"\\"));
        assert_eq!(keys_text(&result, &buffers.words), None);
        let (result, buffers) = checked("wm_press", r#"{"window":5,"keys":"ctrl+S"}"#);
        assert_eq!(keys_text(&result.unwrap(), &buffers.words), Some("ctrl+S"));
        let (result, _) = checked("wm_scroll", r#"{"window":5,"dy":-16}"#);
        let result = result.unwrap();
        assert_eq!(result.notches, Some(-16));
        assert_eq!(result.point, None);
        let (result, _) = checked("wm_scroll", r#"{"window":5,"dy":1,"x":3,"y":4}"#);
        assert_eq!(result.unwrap().point, Some((3, 4)));
        for (tool, arguments, problem) in [
            ("wm_capture", r#"{"window":999999999}"#, None),
            (
                "wm_capture",
                r#"{"window":1000000000}"#,
                Some(ArgumentProblem::BadWindow),
            ),
            (
                "wm_capture",
                r#"{"window":1,"max_width":1569}"#,
                Some(ArgumentProblem::BadImageWidth),
            ),
            (
                "wm_click",
                r#"{"window":1,"y":1}"#,
                Some(ArgumentProblem::BadPoint),
            ),
            (
                "wm_click",
                r#"{"window":1,"x":1,"y":1,"button":1}"#,
                Some(ArgumentProblem::BadButton),
            ),
            (
                "wm_type",
                r#"{"window":1}"#,
                Some(ArgumentProblem::Missing("text")),
            ),
            (
                "wm_type",
                r#"{"window":1,"text":"	"}"#,
                Some(ArgumentProblem::BadText),
            ),
            (
                "wm_press",
                r#"{"window":1,"keys":""}"#,
                Some(ArgumentProblem::BadKeys),
            ),
            (
                "wm_scroll",
                r#"{"window":1,"dy":-17}"#,
                Some(ArgumentProblem::BadNotches),
            ),
            (
                "wm_scroll",
                r#"{"window":1,"dy":"1"}"#,
                Some(ArgumentProblem::BadNotches),
            ),
            (
                "wm_scroll",
                r#"{"window":1}"#,
                Some(ArgumentProblem::Missing("dy")),
            ),
        ] {
            assert_eq!(
                checked(tool, arguments).0.err(),
                problem,
                "{tool} {arguments}"
            );
        }
        let exact = format!(
            r#"{{"window":1,"text":"{}"}}"#,
            "x".repeat(MAX_TYPED_TEXT_BYTES)
        );
        assert_eq!(
            checked("wm_type", &exact).0.unwrap().text,
            Some(MAX_TYPED_TEXT_BYTES)
        );
        let exact = format!(r#"{{"window":1,"keys":"{}"}}"#, "x".repeat(MAX_KEYS_BYTES));
        assert_eq!(
            checked("wm_press", &exact).0.unwrap().keys,
            Some(MAX_KEYS_BYTES)
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
