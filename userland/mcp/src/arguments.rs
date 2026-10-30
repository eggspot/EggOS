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
/// The longest path inside a `.zip` `app_install`'s `entry` takes (a `.zip` entry's name).
pub const MAX_ARCHIVE_ENTRY_BYTES: usize = 255;
/// The longest `arguments` `app_install` takes (D172): a manifest's `arguments`.
pub const MAX_APP_ARGUMENTS_BYTES: usize = 128;
const APP_ARGUMENTS_AT: usize = MAX_ARCHIVE_ENTRY_BYTES + 1;
/// The longest `environment` `app_install` takes (D176): a manifest's `environment`.
pub const MAX_APP_ENVIRONMENT_BYTES: usize = 128;
const APP_ENVIRONMENT_AT: usize = APP_ARGUMENTS_AT + MAX_APP_ARGUMENTS_BYTES;
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
/// The largest menu number of a main menu (the window server holds 6 menus).
pub const MAX_MENU_INDEX: u64 = 5;
/// The largest entry number in one menu (the window server holds 16 entries a menu).
pub const MAX_ENTRY_INDEX: u64 = 15;
/// The most bytes of `wm_menu_choose`'s `label`, so the audit record keeps it whole as it keeps
/// `wm_type`'s text.
pub const MAX_MENU_LABEL_BYTES: usize = 64;
/// The most bytes of `wm_act`'s `text`: what `ACT_ON_ELEMENT` carries (D161).
pub const MAX_ELEMENT_TEXT_BYTES: usize = 232;
/// `wm_act`'s actions, as `wm act` takes them (D161).
pub const ELEMENT_ACTIONS: [&str; 4] = ["invoke", "focus", "set-text", "select"];

/// The length of `clock_set`'s `time`: `YYYY-MM-DDTHH:MM:SSZ` (D150).
pub const TIME_BYTES: usize = 20;
/// The longest host name `net_ping` and `net_lookup` take (D158).
pub const MAX_HOST_BYTES: usize = 253;
/// The most echo requests `net_ping` sends.
pub const MAX_PING_COUNT: u64 = 5;
/// The echo requests `net_ping` sends when the agent names no `count`.
pub const DEFAULT_PING_COUNT: u32 = 4;
/// The longest URL `fetch` takes (D158).
pub const MAX_URL_BYTES: usize = 2048;
/// The longest key `settings_get` and `settings_set` take (D163).
pub const MAX_SETTING_KEY_BYTES: usize = 64;
/// The longest value `settings_set` takes, kept in the words buffer after the key (D163).
pub const MAX_SETTING_VALUE_BYTES: usize = 64;
/// The longest `after` a list tool takes: a cursor (D164).
pub const MAX_CURSOR_BYTES: usize = eggos_commands::page::MAX_CURSOR_BYTES;
/// The longest `target` `ln` takes (D170).
pub const MAX_LINK_TARGET_BYTES: usize = eggos_commands::tools::MAX_LINK_TARGET_BYTES as usize;

const TIME_FORM: &[u8; TIME_BYTES] = b"0000-00-00T00:00:00Z";

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
    /// `app_install`'s `entry` is not a string of 1 to [`MAX_ARCHIVE_ENTRY_BYTES`] bytes
    /// without NUL.
    BadArchiveEntry,
    /// `app_install`'s `arguments` is not a string of 1 to [`MAX_APP_ARGUMENTS_BYTES`] bytes
    /// without NUL.
    BadAppArguments,
    /// `app_install`'s `environment` is not a string of 1 to [`MAX_APP_ENVIRONMENT_BYTES`]
    /// bytes without NUL.
    BadAppEnvironment,
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
    /// `dy` or `dx` is more than [`MAX_NOTCHES`] either way, or both are 0.
    BadNotches,
    /// `max_width` is outside [`MIN_IMAGE_WIDTH`] to [`MAX_IMAGE_WIDTH`].
    BadImageWidth,
    /// `time` is not `YYYY-MM-DDTHH:MM:SSZ`.
    BadTime,
    /// `menu` is not 0 to [`MAX_MENU_INDEX`] or `entry` not 0 to [`MAX_ENTRY_INDEX`].
    BadMenuItem,
    /// `label` is longer than [`MAX_MENU_LABEL_BYTES`] or holds a control character.
    BadMenuLabel,
    /// `host` is not 1 to [`MAX_HOST_BYTES`] letters, digits, dots, hyphens and underscores,
    /// or starts with a dot or a hyphen.
    BadHost,
    /// `count` is not 1 to [`MAX_PING_COUNT`].
    BadCount,
    /// `url` is not an `http://` URL of at most [`MAX_URL_BYTES`] bytes without spaces or
    /// control characters.
    BadUrl,
    /// `width` or `height` is not a whole number from 1 to [`MAX_COORDINATE`], or only one is
    /// given (D160).
    BadSize,
    /// `element` is not a whole number from 1 to 4294967295 (D161).
    BadElement,
    /// `action` is not invoke, focus, set-text or select, or `text` comes with an action other
    /// than set-text or is missing for it (D161).
    BadAction,
    /// `text` for set-text is longer than [`MAX_ELEMENT_TEXT_BYTES`] or holds a control
    /// character (D161).
    BadElementText,
    /// `key` is not 1 to [`MAX_SETTING_KEY_BYTES`] lowercase letters, digits, dots and
    /// underscores starting with a letter (D163).
    BadSettingKey,
    /// `value` is not 1 to [`MAX_SETTING_VALUE_BYTES`] bytes without control characters (D163).
    BadSettingValue,
    /// `limit` is not a whole number from 1 to the tool's page size (D164).
    BadLimit,
    /// `after` is not a cursor a page ended with (D164).
    BadCursor,
    /// `ln`'s `target` is empty, longer than [`MAX_LINK_TARGET_BYTES`], holds a NUL or starts
    /// with `-` (D170).
    BadLinkTarget,
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
            ArgumentProblem::Missing("to_x") => "to_x and to_y are required: where the drag ends",
            ArgumentProblem::Missing("width") => "width and height are required",
            ArgumentProblem::Missing("text") => "text is required",
            ArgumentProblem::Missing("keys") => "keys is required",
            ArgumentProblem::Missing("dy") => "dy is required",
            ArgumentProblem::Missing("menu") => {
                "menu and entry are required: the numbers wm_menu_list shows"
            }
            ArgumentProblem::Missing("time") => "time is required: YYYY-MM-DDTHH:MM:SSZ, in UTC",
            ArgumentProblem::Missing("host") => "host is required: an IPv4 address or a host name",
            ArgumentProblem::Missing("url") => "url is required: an http:// URL",
            ArgumentProblem::Missing("element") => {
                "element and action are required: an id wm_show shows and what to do"
            }
            ArgumentProblem::Missing("key") => "key is required: a key settings_list shows",
            ArgumentProblem::Missing("link") => "link is required: the path of the link to make",
            ArgumentProblem::Missing("target") => "target is required: what the link points to",
            ArgumentProblem::Missing("value") => "value is required: the new value",
            ArgumentProblem::Missing(_) => "a required argument is missing",
            ArgumentProblem::WrongType("path") => "path must be a string",
            ArgumentProblem::WrongType("arguments") => "arguments must be an array of strings",
            ArgumentProblem::WrongType("append" | "confirm") => "append and confirm are booleans",
            ArgumentProblem::WrongType("dry_run") => "dry_run is a boolean",
            ArgumentProblem::WrongType("symbolic") => "symbolic is a boolean",
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
            ArgumentProblem::BadArchiveEntry => {
                "entry must be a path inside the .zip of 1 to 255 bytes, without NUL"
            }
            ArgumentProblem::BadAppArguments => "arguments must be 1 to 128 bytes, without NUL",
            ArgumentProblem::BadAppEnvironment => "environment must be 1 to 128 bytes, without NUL",
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
            ArgumentProblem::BadSize => {
                "width and height are whole numbers of pixels from 1 to 65535, given together"
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
                "dy must be a whole number of notches from -16 to 16, as dx, and not both 0"
            }
            ArgumentProblem::BadImageWidth => "max_width must be a whole number from 16 to 1568",
            ArgumentProblem::BadTime => {
                "time must be a date and time in UTC as YYYY-MM-DDTHH:MM:SSZ, such as \
                 2026-10-29T14:05:09Z"
            }
            ArgumentProblem::BadMenuItem => {
                "menu (0 to 5) and entry (0 to 15) are whole numbers as wm_menu_list shows them"
            }
            ArgumentProblem::BadMenuLabel => {
                "label is the item's label as wm_menu_list shows it: at most 64 bytes, without \
                 control characters"
            }
            ArgumentProblem::BadHost => {
                "host must be an IPv4 address or a host name: 1 to 253 letters, digits, dots, \
                 hyphens and underscores, not starting with a dot or a hyphen"
            }
            ArgumentProblem::BadCount => "count must be a whole number from 1 to 5",
            ArgumentProblem::BadUrl => {
                "url must be an http:// or https:// URL of at most 2048 bytes without spaces"
            }
            ArgumentProblem::BadElement => {
                "element must be an element id as wm_show shows it: a whole number from 1 to \
                 4294967295"
            }
            ArgumentProblem::BadAction => {
                "action is invoke, focus, set-text or select; text goes with set-text and only \
                 with it"
            }
            ArgumentProblem::BadElementText => {
                "text must be at most 232 bytes of UTF-8 without control characters"
            }
            ArgumentProblem::BadSettingKey => {
                "key must be a key settings_list shows, such as shell.profile: 1 to 64 lowercase \
                 letters, digits, dots and underscores"
            }
            ArgumentProblem::BadSettingValue => {
                "value must be 1 to 64 bytes without control characters"
            }
            ArgumentProblem::BadLimit => {
                "limit must be a whole number from 1 to the most a page holds: 500 for ls, 100 \
                 for the other lists"
            }
            ArgumentProblem::BadCursor => {
                "after must be the next_cursor a page ended with, as it was given: leave it out \
                 to start again"
            }
            ArgumentProblem::BadLinkTarget => {
                "target must be 1 to 4095 bytes without NUL, not starting with - (write ./-name)"
            }
        }
    }
}

/// `clock_set`'s `time`, kept whole: checked to have the form `YYYY-MM-DDTHH:MM:SSZ`, digits
/// where it has them; whether it is a real moment in range is for `date -s`, its twin, to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeText([u8; TIME_BYTES]);

impl TimeText {
    /// `text` as a time, if it has the form.
    pub fn of(text: &[u8]) -> Result<Self, ArgumentProblem> {
        let shaped = text.len() == TIME_BYTES
            && text.iter().zip(TIME_FORM).all(|(&byte, &form)| match form {
                b'0' => byte.is_ascii_digit(),
                _ => byte == form,
            });
        let mut bytes = [0u8; TIME_BYTES];
        match shaped {
            true => {
                bytes.copy_from_slice(text);
                Ok(Self(bytes))
            }
            false => Err(ArgumentProblem::BadTime),
        }
    }

    /// The time's text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or("")
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
    /// `app_install`'s entry's length, in the words buffer.
    pub archive_entry: Option<usize>,
    /// `app_install`'s arguments' length, in the words buffer after the entry's room.
    pub app_arguments: Option<usize>,
    /// `app_install`'s environment's length, in the words buffer after the arguments' room.
    pub app_environment: Option<usize>,
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
    /// `wm_size`'s width and height (D160).
    pub size: Option<(u32, u32)>,
    /// `wm_click`'s button.
    pub button: Option<Button>,
    /// `wm_press`'s keys' length, in the words buffer.
    pub keys: Option<usize>,
    /// `wm_type`'s text's length, in the words buffer.
    pub text: Option<usize>,
    /// `wm_scroll`'s notches (`dy`), up when positive.
    pub notches: Option<i32>,
    /// `wm_scroll`'s sideways notches (`dx`), right when positive (D162).
    pub sideways: Option<i32>,
    /// `wm_click`'s `double` (D162).
    pub double: bool,
    /// `wm_drag`'s end point, in window pixels (`to_x`, `to_y`) (D162).
    pub to_point: Option<(u32, u32)>,
    /// `wm_capture`'s widest image, in pixels.
    pub max_width: Option<u32>,
    /// `clock_set`'s time.
    pub time: Option<TimeText>,
    /// `wm_menu_choose`'s menu and entry numbers.
    pub menu_item: Option<(u32, u32)>,
    /// `wm_menu_choose`'s label's length, in the words buffer.
    pub menu_label: Option<usize>,
    /// `net_ping`'s and `net_lookup`'s host's length, in the words buffer.
    pub host: Option<usize>,
    /// `net_ping`'s count of echo requests.
    pub count: Option<u32>,
    /// `fetch`'s URL's length, in the content buffer.
    pub url: Option<usize>,
    /// `wm_act`'s element id (D161).
    pub element: Option<u32>,
    /// `wm_act`'s action, as `wm act` takes it (D161).
    pub action: Option<&'static str>,
    /// `settings_get`'s and `settings_set`'s key's length, in the words buffer (D163).
    pub setting_key: Option<usize>,
    /// `settings_set`'s value's length, in the words buffer after
    /// [`MAX_SETTING_KEY_BYTES`] (D163).
    pub setting_value: Option<usize>,
    /// A list tool's `limit` (D164).
    pub limit: Option<u32>,
    /// A list tool's `after`'s length, in the words buffer (D164).
    pub after: Option<usize>,
    /// `dry_run` (D164).
    pub dry_run: bool,
    /// `ln`'s target's length, in the content buffer (D170).
    pub link_target: Option<usize>,
    /// `ln`'s `symbolic`, true unless the agent says false (D170).
    pub symbolic: bool,
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
        && arguments.members().any(|(name, _)| {
            !tool
                .definition
                .arguments
                .iter()
                .any(|known| name.equals(known.name))
        })
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
    checked.dry_run = flag(get("dry_run"), "dry_run")?;
    if let Some(most) = tool.definition.page_limit() {
        checked.limit = match whole(get("limit"), most as u64, ArgumentProblem::BadLimit)? {
            Some(0) => return Err(ArgumentProblem::BadLimit),
            limit => limit.map(|limit| limit as u32),
        };
        if get("after").is_some() {
            let length = short_text(
                get("after"),
                "after",
                decoded.words,
                MAX_CURSOR_BYTES,
                |byte| byte == b'%' || eggos_commands::page::is_cursor_byte(byte),
                ArgumentProblem::BadCursor,
            )?;
            let cursor = decoded.words.get(..length).unwrap_or_default();
            if !eggos_commands::page::is_cursor(cursor) {
                return Err(ArgumentProblem::BadCursor);
            }
            checked.after = Some(length);
        }
    }
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
    if tool.kind == Kind::AppInstall {
        checked.archive_entry = archive_entry(get("entry"), decoded.words)?;
        checked.app_arguments = app_arguments(get("arguments"), decoded.words)?;
        checked.app_environment = app_environment(get("environment"), decoded.words)?;
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
    if tool.kind == Kind::ClockSet {
        checked.time = Some(match get("time") {
            Some(Value::Str(text)) => {
                let mut bytes = [0u8; TIME_BYTES];
                let decoded = text
                    .decode_into(&mut bytes)
                    .ok_or(ArgumentProblem::BadTime)?;
                TimeText::of(decoded.as_bytes())?
            }
            Some(_) => return Err(ArgumentProblem::BadTime),
            None => return Err(ArgumentProblem::Missing("time")),
        });
    }
    if matches!(tool.kind, Kind::NetPing | Kind::NetLookup) {
        let length = short_text(
            get("host"),
            "host",
            decoded.words,
            MAX_HOST_BYTES,
            is_host_byte,
            ArgumentProblem::BadHost,
        )?;
        let first = decoded.words.first().copied();
        if matches!(first, Some(b'-' | b'.')) {
            return Err(ArgumentProblem::BadHost);
        }
        checked.host = Some(length);
    }
    if tool.kind == Kind::NetPing {
        let count = whole(get("count"), MAX_PING_COUNT, ArgumentProblem::BadCount)?;
        checked.count = Some(match count {
            None => DEFAULT_PING_COUNT,
            Some(0) => return Err(ArgumentProblem::BadCount),
            Some(count) => count as u32,
        });
    }
    if matches!(tool.kind, Kind::SettingsGet | Kind::SettingsSet) {
        let length = short_text(
            get("key"),
            "key",
            decoded.words,
            MAX_SETTING_KEY_BYTES,
            is_setting_key_byte,
            ArgumentProblem::BadSettingKey,
        )?;
        if !decoded.words.first().is_some_and(u8::is_ascii_lowercase) {
            return Err(ArgumentProblem::BadSettingKey);
        }
        checked.setting_key = Some(length);
    }
    if tool.kind == Kind::SettingsSet {
        let room = decoded
            .words
            .get_mut(MAX_SETTING_KEY_BYTES..)
            .ok_or(ArgumentProblem::BadSettingValue)?;
        checked.setting_value = Some(short_text(
            get("value"),
            "value",
            room,
            MAX_SETTING_VALUE_BYTES,
            |byte| !byte.is_ascii_control(),
            ArgumentProblem::BadSettingValue,
        )?);
    }
    if tool.kind == Kind::Ln {
        checked.symbolic = match get("symbolic") {
            None => true,
            given => flag(given, "symbolic")?,
        };
        let length = short_text(
            get("target"),
            "target",
            decoded.content,
            MAX_LINK_TARGET_BYTES,
            |byte| byte != 0,
            ArgumentProblem::BadLinkTarget,
        )?;
        if decoded.content.first() == Some(&b'-') {
            return Err(ArgumentProblem::BadLinkTarget);
        }
        checked.link_target = Some(length);
    }
    if tool.kind == Kind::Fetch {
        let length = short_text(
            get("url"),
            "url",
            decoded.content,
            MAX_URL_BYTES,
            |byte| byte > b' ' && byte != 0x7f,
            ArgumentProblem::BadUrl,
        )?;
        let starts = |scheme: &[u8]| {
            decoded
                .content
                .get(..scheme.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
        };
        if !starts(b"http://") && !starts(b"https://") {
            return Err(ArgumentProblem::BadUrl);
        }
        checked.url = Some(length);
    }
    Ok(checked)
}

fn is_setting_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_')
}

/// `settings_get`'s and `settings_set`'s key in `buffer`, the words buffer, that
/// [`Checked::setting_key`] measures (D163).
pub fn setting_key_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.setting_key?)?).ok()
}

/// `settings_set`'s value in `buffer`, the words buffer, that [`Checked::setting_value`]
/// measures (D163).
pub fn setting_value_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    let end = MAX_SETTING_KEY_BYTES.checked_add(checked.setting_value?)?;
    core::str::from_utf8(buffer.get(MAX_SETTING_KEY_BYTES..end)?).ok()
}

fn is_host_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
}

/// `net_ping`'s and `net_lookup`'s host in `buffer`, the words buffer, that [`Checked::host`]
/// measures.
pub fn host_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.host?)?).ok()
}

/// `fetch`'s URL in `buffer`, the content buffer (which only calls that need a grant fill, so
/// it lasts while the call waits for the user), that [`Checked::url`] measures.
pub fn url_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.url?)?).ok()
}

fn archive_entry(
    value: Option<Value<'_>>,
    words: &mut [u8],
) -> Result<Option<usize>, ArgumentProblem> {
    let text = match value {
        None => return Ok(None),
        Some(Value::Str(text)) => text,
        Some(_) => return Err(ArgumentProblem::BadArchiveEntry),
    };
    let room = words
        .get_mut(..MAX_ARCHIVE_ENTRY_BYTES)
        .ok_or(ArgumentProblem::BadArchiveEntry)?;
    let entry = text
        .decode_into(room)
        .ok_or(ArgumentProblem::BadArchiveEntry)?;
    match !entry.is_empty() && !entry.contains('\0') {
        true => Ok(Some(entry.len())),
        false => Err(ArgumentProblem::BadArchiveEntry),
    }
}

fn app_arguments(
    value: Option<Value<'_>>,
    words: &mut [u8],
) -> Result<Option<usize>, ArgumentProblem> {
    let text = match value {
        None => return Ok(None),
        Some(Value::Str(text)) => text,
        Some(_) => return Err(ArgumentProblem::BadAppArguments),
    };
    let room = words
        .get_mut(APP_ARGUMENTS_AT..APP_ARGUMENTS_AT + MAX_APP_ARGUMENTS_BYTES)
        .ok_or(ArgumentProblem::BadAppArguments)?;
    let arguments = text
        .decode_into(room)
        .ok_or(ArgumentProblem::BadAppArguments)?;
    match !arguments.is_empty() && !arguments.contains('\0') {
        true => Ok(Some(arguments.len())),
        false => Err(ArgumentProblem::BadAppArguments),
    }
}

fn app_environment(
    value: Option<Value<'_>>,
    words: &mut [u8],
) -> Result<Option<usize>, ArgumentProblem> {
    let text = match value {
        None => return Ok(None),
        Some(Value::Str(text)) => text,
        Some(_) => return Err(ArgumentProblem::BadAppEnvironment),
    };
    let room = words
        .get_mut(APP_ENVIRONMENT_AT..APP_ENVIRONMENT_AT + MAX_APP_ENVIRONMENT_BYTES)
        .ok_or(ArgumentProblem::BadAppEnvironment)?;
    let environment = text
        .decode_into(room)
        .ok_or(ArgumentProblem::BadAppEnvironment)?;
    match !environment.is_empty() && !environment.contains('\0') {
        true => Ok(Some(environment.len())),
        false => Err(ArgumentProblem::BadAppEnvironment),
    }
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
        Kind::WmClick | Kind::WmDrag => {
            checked.point = Some(point(get("x"), get("y"))?.ok_or(ArgumentProblem::Missing("x"))?);
            checked.button = Some(match get("button") {
                None => Button::Left,
                Some(Value::Str(name)) => Button::ALL
                    .into_iter()
                    .find(|button| name.equals(button.name()))
                    .ok_or(ArgumentProblem::BadButton)?,
                Some(_) => return Err(ArgumentProblem::BadButton),
            });
            match kind {
                Kind::WmDrag => {
                    checked.to_point = Some(
                        point(get("to_x"), get("to_y"))?.ok_or(ArgumentProblem::Missing("to_x"))?,
                    );
                }
                _ => checked.double = flag(get("double"), "double")?,
            }
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
            let notches = |value: Option<Value<'a>>| match value {
                None => Ok(0),
                Some(Value::Number(number)) => number
                    .as_i64()
                    .filter(|notches| notches.unsigned_abs() <= MAX_NOTCHES)
                    .ok_or(ArgumentProblem::BadNotches),
                Some(_) => Err(ArgumentProblem::BadNotches),
            };
            let dy = get("dy").ok_or(ArgumentProblem::Missing("dy"))?;
            let (dy, dx) = (notches(Some(dy))?, notches(get("dx"))?);
            if dy == 0 && dx == 0 {
                return Err(ArgumentProblem::BadNotches);
            }
            checked.notches = Some(dy as i32);
            checked.sideways = Some(dx as i32);
            checked.point = point(get("x"), get("y"))?;
        }
        Kind::WmMove => {
            checked.point = Some(point(get("x"), get("y"))?.ok_or(ArgumentProblem::Missing("x"))?);
        }
        Kind::WmSize => {
            let width = whole(get("width"), MAX_COORDINATE, ArgumentProblem::BadSize)?;
            let height = whole(get("height"), MAX_COORDINATE, ArgumentProblem::BadSize)?;
            checked.size = match (width, height) {
                (Some(0), _) | (_, Some(0)) => return Err(ArgumentProblem::BadSize),
                (Some(width), Some(height)) => Some((width as u32, height as u32)),
                (None, None) => return Err(ArgumentProblem::Missing("width")),
                _ => return Err(ArgumentProblem::BadSize),
            };
        }
        Kind::WmMenuChoose => {
            let menu = whole(get("menu"), MAX_MENU_INDEX, ArgumentProblem::BadMenuItem)?;
            let entry = whole(get("entry"), MAX_ENTRY_INDEX, ArgumentProblem::BadMenuItem)?;
            let (Some(menu), Some(entry)) = (menu, entry) else {
                return Err(ArgumentProblem::Missing("menu"));
            };
            checked.menu_item = Some((menu as u32, entry as u32));
            checked.menu_label = menu_label(get("label"), words)?;
        }
        Kind::WmAct => {
            let element = whole(
                get("element"),
                u64::from(u32::MAX),
                ArgumentProblem::BadElement,
            )?
            .ok_or(ArgumentProblem::Missing("element"))?;
            if element == 0 {
                return Err(ArgumentProblem::BadElement);
            }
            checked.element = Some(element as u32);
            let action = match get("action") {
                None => return Err(ArgumentProblem::Missing("element")),
                Some(Value::Str(name)) => ELEMENT_ACTIONS
                    .into_iter()
                    .find(|action| name.equals(action))
                    .ok_or(ArgumentProblem::BadAction)?,
                Some(_) => return Err(ArgumentProblem::BadAction),
            };
            checked.action = Some(action);
            checked.text = element_text(get("text"), action == "set-text", words)?;
        }
        _ => {}
    }
    Ok(())
}

fn element_text(
    value: Option<Value<'_>>,
    wanted: bool,
    words: &mut [u8],
) -> Result<Option<usize>, ArgumentProblem> {
    let text = match (value, wanted) {
        (None, false) => return Ok(None),
        (Some(Value::Str(text)), true) => text,
        _ => return Err(ArgumentProblem::BadAction),
    };
    let room = words
        .get_mut(..MAX_ELEMENT_TEXT_BYTES)
        .ok_or(ArgumentProblem::BadElementText)?;
    let text = text
        .decode_into(room)
        .ok_or(ArgumentProblem::BadElementText)?;
    match text.chars().any(char::is_control) {
        true => Err(ArgumentProblem::BadElementText),
        false => Ok(Some(text.len())),
    }
}

fn menu_label(
    value: Option<Value<'_>>,
    words: &mut [u8],
) -> Result<Option<usize>, ArgumentProblem> {
    let text = match value {
        None => return Ok(None),
        Some(Value::Str(text)) => text,
        Some(_) => return Err(ArgumentProblem::BadMenuLabel),
    };
    let room = words
        .get_mut(..MAX_MENU_LABEL_BYTES)
        .ok_or(ArgumentProblem::BadMenuLabel)?;
    let label = text
        .decode_into(room)
        .ok_or(ArgumentProblem::BadMenuLabel)?;
    match label.chars().any(char::is_control) {
        true => Err(ArgumentProblem::BadMenuLabel),
        false => Ok(Some(label.len())),
    }
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

/// `wm_menu_choose`'s label in `buffer`, the words buffer, that [`Checked::menu_label`]
/// measures.
pub fn menu_label_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.menu_label?)?).ok()
}

/// `wm_press`'s keys in `buffer`, the words buffer, that [`Checked::keys`] measures.
pub fn keys_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.keys?)?).ok()
}

/// `app_install`'s entry in `buffer`, the words buffer, that [`Checked::archive_entry`]
/// measures.
pub fn archive_entry_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.archive_entry?)?).ok()
}

/// `app_install`'s arguments in `buffer`, the words buffer, that [`Checked::app_arguments`]
/// measures (D172).
pub fn app_arguments_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    let end = APP_ARGUMENTS_AT.checked_add(checked.app_arguments?)?;
    core::str::from_utf8(buffer.get(APP_ARGUMENTS_AT..end)?).ok()
}

/// `app_install`'s environment in `buffer`, the words buffer, that
/// [`Checked::app_environment`] measures (D176).
pub fn app_environment_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    let end = APP_ENVIRONMENT_AT.checked_add(checked.app_environment?)?;
    core::str::from_utf8(buffer.get(APP_ENVIRONMENT_AT..end)?).ok()
}

/// A list tool's `after` in `buffer`, the words buffer, that [`Checked::after`] measures (D164).
pub fn after_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.after?)?).ok()
}

/// `ln`'s target in `buffer`, the content buffer, that [`Checked::link_target`] measures
/// (D170).
pub fn link_target_text<'b>(checked: &Checked, buffer: &'b [u8]) -> Option<&'b str> {
    core::str::from_utf8(buffer.get(..checked.link_target?)?).ok()
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
        let (result, buffers) = checked(
            "app_install",
            r#"{"path":"/media/host/tools.zip","entry":"bin/a.exe"}"#,
        );
        let result = result.unwrap();
        assert_eq!(
            archive_entry_text(&result, &buffers.words),
            Some("bin/a.exe")
        );
        let (result, _) = checked("app_install", r#"{"path":"/media/host/x.exe"}"#);
        assert_eq!(result.unwrap().archive_entry, None);
        let (result, buffers) = checked(
            "app_install",
            r#"{"path":"/media/host/brogue.zip","entry":"brogue","arguments":"-n --size 4"}"#,
        );
        let result = result.unwrap();
        assert_eq!(archive_entry_text(&result, &buffers.words), Some("brogue"));
        assert_eq!(
            app_arguments_text(&result, &buffers.words),
            Some("-n --size 4")
        );
        let (result, buffers) = checked(
            "app_install",
            r#"{"path":"/media/host/m.zip","arguments":"-n","environment":"A=1 B=2"}"#,
        );
        let result = result.unwrap();
        assert_eq!(app_arguments_text(&result, &buffers.words), Some("-n"));
        assert_eq!(
            app_environment_text(&result, &buffers.words),
            Some("A=1 B=2")
        );
        for environment in [
            r#"{"path":"/media/host/t.zip","environment":""}"#,
            r#"{"path":"/media/host/t.zip","environment":{"A":"1"}}"#,
        ] {
            assert_eq!(
                checked("app_install", environment).0,
                Err(ArgumentProblem::BadAppEnvironment),
                "{environment}"
            );
        }
        for arguments in [
            r#"{"path":"/media/host/t.zip","arguments":""}"#,
            r#"{"path":"/media/host/t.zip","arguments":["-n"]}"#,
        ] {
            assert_eq!(
                checked("app_install", arguments).0,
                Err(ArgumentProblem::BadAppArguments),
                "{arguments}"
            );
        }
        let long = format!(
            r#"{{"path":"/media/host/t.zip","entry":"{}"}}"#,
            "e".repeat(MAX_ARCHIVE_ENTRY_BYTES + 1)
        );
        for arguments in [
            r#"{"path":"/media/host/t.zip","entry":""}"#,
            r#"{"path":"/media/host/t.zip","entry":1}"#,
            r#"{"path":"/media/host/t.zip","entry":"a\u0000b"}"#,
            long.as_str(),
        ] {
            assert_eq!(
                checked("app_install", arguments).0,
                Err(ArgumentProblem::BadArchiveEntry),
                "{arguments}"
            );
        }
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
        let (result, _) = checked("wm_scroll", r#"{"window":5,"dy":0,"dx":-3}"#);
        let result = result.unwrap();
        assert_eq!((result.notches, result.sideways), (Some(0), Some(-3)));
        let (result, _) = checked("wm_click", r#"{"window":5,"x":1,"y":2,"double":true}"#);
        assert!(result.unwrap().double);
        let (result, _) = checked(
            "wm_drag",
            r#"{"window":5,"x":1,"y":2,"to_x":30,"to_y":40,"button":"right"}"#,
        );
        let result = result.unwrap();
        assert_eq!(
            (result.point, result.to_point, result.button),
            (Some((1, 2)), Some((30, 40)), Some(Button::Right))
        );
        for (tool, arguments, problem) in [
            (
                "wm_scroll",
                r#"{"window":1,"dy":0}"#,
                ArgumentProblem::BadNotches,
            ),
            (
                "wm_scroll",
                r#"{"window":1,"dy":0,"dx":17}"#,
                ArgumentProblem::BadNotches,
            ),
            (
                "wm_click",
                r#"{"window":1,"x":1,"y":1,"double":1}"#,
                ArgumentProblem::WrongType("double"),
            ),
            (
                "wm_drag",
                r#"{"window":1,"x":1,"y":1}"#,
                ArgumentProblem::Missing("to_x"),
            ),
            (
                "wm_drag",
                r#"{"window":1,"x":1,"y":1,"to_x":2}"#,
                ArgumentProblem::BadPoint,
            ),
            (
                "wm_drag",
                r#"{"window":1,"x":1,"y":1,"to_x":2,"to_y":3,"double":true}"#,
                ArgumentProblem::Unknown,
            ),
        ] {
            assert_eq!(
                checked(tool, arguments).0.err(),
                Some(problem),
                "{tool} {arguments}"
            );
        }
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
                r#"{"window":1,"text":"\t"}"#,
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
        let (result, _) = checked("wm_menu_choose", r#"{"window":5,"menu":5,"entry":15}"#);
        let result = result.unwrap();
        assert_eq!((result.menu_item, result.menu_label), (Some((5, 15)), None));
        let (result, buffers) = checked(
            "wm_menu_choose",
            r#"{"window":5,"menu":1,"entry":0,"label":"Save\u2026"}"#,
        );
        assert_eq!(
            menu_label_text(&result.unwrap(), &buffers.words),
            Some("Save\u{2026}")
        );
        let (result, _) = checked("wm_menu_list", r#"{"window":5}"#);
        assert_eq!(result.unwrap().menu_item, None);
        let longest = format!(
            r#"{{"window":1,"menu":0,"entry":0,"label":"{}"}}"#,
            "x".repeat(MAX_MENU_LABEL_BYTES)
        );
        assert_eq!(
            checked("wm_menu_choose", &longest).0.unwrap().menu_label,
            Some(MAX_MENU_LABEL_BYTES)
        );
        let too_long = format!(
            r#"{{"window":1,"menu":0,"entry":0,"label":"{}"}}"#,
            "x".repeat(MAX_MENU_LABEL_BYTES + 1)
        );
        assert_eq!(
            checked("wm_menu_choose", &too_long).0,
            Err(ArgumentProblem::BadMenuLabel)
        );
        for (arguments, problem) in [
            (
                r#"{"window":1,"menu":6,"entry":0}"#,
                ArgumentProblem::BadMenuItem,
            ),
            (
                r#"{"window":1,"menu":0,"entry":16}"#,
                ArgumentProblem::BadMenuItem,
            ),
            (
                r#"{"window":1,"menu":-1,"entry":0}"#,
                ArgumentProblem::BadMenuItem,
            ),
            (
                r#"{"window":1,"menu":"File","entry":0}"#,
                ArgumentProblem::BadMenuItem,
            ),
            (r#"{"window":1,"menu":0}"#, ArgumentProblem::Missing("menu")),
            (
                r#"{"window":1,"entry":0}"#,
                ArgumentProblem::Missing("menu"),
            ),
            (
                r#"{"window":1,"menu":0,"entry":0,"label":"a\tb"}"#,
                ArgumentProblem::BadMenuLabel,
            ),
            (
                r#"{"window":1,"menu":0,"entry":0,"label":7}"#,
                ArgumentProblem::BadMenuLabel,
            ),
        ] {
            assert_eq!(
                checked("wm_menu_choose", arguments).0,
                Err(problem),
                "{arguments}"
            );
        }
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

    #[test]
    fn list_tools_take_a_limit_up_to_their_page_and_a_cursor() {
        let (result, buffers) =
            checked("ls", r#"{"path":"/home","limit":500,"after":"my%20notes"}"#);
        let result = result.unwrap();
        assert_eq!(result.limit, Some(500));
        assert_eq!(after_text(&result, &buffers.words), Some("my%20notes"));
        let (result, _) = checked("ps", "{}");
        let result = result.unwrap();
        assert_eq!((result.limit, result.after), (None, None));
        let (result, buffers) = checked("wm_list", r#"{"after":"129"}"#);
        assert_eq!(after_text(&result.unwrap(), &buffers.words), Some("129"));
        for (tool, arguments, problem) in [
            ("ps", r#"{"limit":101}"#, ArgumentProblem::BadLimit),
            ("app_list", r#"{"limit":0}"#, ArgumentProblem::BadLimit),
            ("ls", r#"{"limit":501}"#, ArgumentProblem::BadLimit),
            ("ls", r#"{"limit":"5"}"#, ArgumentProblem::BadLimit),
            ("ls", r#"{"after":""}"#, ArgumentProblem::BadCursor),
            ("ls", r#"{"after":"-rf"}"#, ArgumentProblem::BadCursor),
            ("ls", r#"{"after":"a b"}"#, ArgumentProblem::BadCursor),
            ("ls", r#"{"after":"a\nb"}"#, ArgumentProblem::BadCursor),
            ("ls", r#"{"after":7}"#, ArgumentProblem::BadCursor),
            (
                "cat",
                r#"{"path":"/home/a","limit":1}"#,
                ArgumentProblem::Unknown,
            ),
            ("clock_get", r#"{"after":"x"}"#, ArgumentProblem::Unknown),
        ] {
            assert_eq!(
                checked(tool, arguments).0,
                Err(problem),
                "{tool} {arguments}"
            );
        }
        let long = format!(r#"{{"after":"{}"}}"#, "a".repeat(MAX_CURSOR_BYTES + 1));
        assert_eq!(checked("ls", &long).0, Err(ArgumentProblem::BadCursor));
    }

    #[test]
    fn dry_run_is_a_boolean_of_the_tools_that_change_something() {
        let (result, _) = checked("trash", r#"{"path":"/home/root/a","dry_run":true}"#);
        let result = result.unwrap();
        assert!(result.dry_run && !result.confirm);
        let (result, _) = checked("settings_set", r#"{"key":"shell.profile","value":"linux"}"#);
        assert!(!result.unwrap().dry_run);
        assert_eq!(
            checked("tee", r#"{"path":"/home/root/a","dry_run":"yes"}"#).0,
            Err(ArgumentProblem::WrongType("dry_run"))
        );
        for tool in ["run", "fetch", "clock_set", "wm_click", "ls"] {
            assert_eq!(
                checked(tool, r#"{"dry_run":true}"#).0,
                Err(ArgumentProblem::Unknown),
                "{tool}"
            );
        }
    }

    #[test]
    fn ln_takes_a_link_path_a_target_text_and_symbolic_true_by_default() {
        let (result, buffers) = checked("ln", r#"{"target":"notes.txt","link":"/home/root/l"}"#);
        let result = result.unwrap();
        assert_eq!(path_text(&result, &buffers.path), Some("/home/root/l"));
        assert_eq!(
            link_target_text(&result, &buffers.content),
            Some("notes.txt")
        );
        assert!(result.symbolic);
        let arguments = r#"{"target":"/home/root/a","link":"/home/root/b","symbolic":false}"#;
        assert!(!checked("ln", arguments).0.unwrap().symbolic);
        let long = format!(
            r#"{{"target":"{}","link":"/home/root/l"}}"#,
            "a".repeat(MAX_LINK_TARGET_BYTES + 1)
        );
        assert_eq!(checked("ln", &long).0, Err(ArgumentProblem::BadLinkTarget));
        for (arguments, problem) in [
            (r#"{"target":"a"}"#, ArgumentProblem::Missing("link")),
            (
                r#"{"link":"/home/root/l"}"#,
                ArgumentProblem::Missing("target"),
            ),
            (
                r#"{"target":"","link":"/home/root/l"}"#,
                ArgumentProblem::BadLinkTarget,
            ),
            (
                r#"{"target":"-rf","link":"/home/root/l"}"#,
                ArgumentProblem::BadLinkTarget,
            ),
            (
                r#"{"target":"a\u0000b","link":"/home/root/l"}"#,
                ArgumentProblem::BadLinkTarget,
            ),
            (
                r#"{"target":7,"link":"/home/root/l"}"#,
                ArgumentProblem::BadLinkTarget,
            ),
            (
                r#"{"target":"a","link":"/home/root/l","symbolic":"no"}"#,
                ArgumentProblem::WrongType("symbolic"),
            ),
            (
                r#"{"target":"a","link":"/home/root/l","force":true}"#,
                ArgumentProblem::Unknown,
            ),
        ] {
            assert_eq!(checked("ln", arguments).0, Err(problem), "{arguments}");
        }
    }
}
