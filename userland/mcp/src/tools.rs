//! The MCP tools (D136, D137): each a front-end over an existing command, its CLI twin, which
//! the server runs with the agent's checked arguments (D22). Each tool's name, description,
//! arguments, grant, confirmation and annotations come from the command model's registry
//! (`eggos_commands::tools`, D160); this table adds how the server runs its twin. A tool that
//! changes something needs a grant the user gave on the host ([`crate::grants`]); seeing a
//! window's content or its menus needs `view` (D143, D147).

use crate::grants::Grant;
use crate::json::JsonWriter;
use crate::paths::{ANYWHERE, INSTALL_ROOTS, READ_ROOTS, TRASH, WRITE_ROOTS};
use eggos_commands::tools::{self as registry, Argument, DefaultValue, JsonType, ToolDefinition};

pub use eggos_commands::tools::{CAPTURE_PATH, Confirmation};

/// The largest PNG `wm_capture` returns: in base64 with the answer's envelope it fits the
/// 1 MiB answer (D136); `wm capture` shrinks the image until it fits.
pub const MAX_IMAGE_BYTES: usize = 700 * 1024;
/// The widest image `wm_capture` returns: clients such as Claude scale larger images down to
/// about this long edge, so wider ones would cost bytes and show nothing more.
pub const MAX_IMAGE_WIDTH: u32 = 1568;
/// The narrowest image `wm_capture` may be asked for.
pub const MIN_IMAGE_WIDTH: u32 = 16;
/// The widest image `wm_capture` returns when the agent names no `max_width`.
pub const DEFAULT_IMAGE_WIDTH: u32 = 1024;
/// The exit status of a `wm` command refused because a person is using the machine (D141).
pub const BUSY_STATUS: i32 = 75;

/// What a tool's path argument is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathArgument {
    /// The argument's name.
    pub name: &'static str,
    /// Its value when the agent gives none; `None` when it is required.
    pub default: Option<&'static str>,
    /// The roots the resolved path must be inside.
    pub roots: &'static [&'static str],
    /// Whether it must be strictly inside a root, not a root itself.
    pub strictly: bool,
    /// Places inside the roots it must not be in.
    pub excluded: &'static [&'static str],
    /// What a path outside the roots is told.
    pub outside: &'static str,
}

/// Which tool it is, for what its arguments and result are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `ls`.
    Ls,
    /// `cat`.
    Cat,
    /// `ps`.
    Ps,
    /// `wm_list`.
    WmList,
    /// `app_list`.
    AppList,
    /// `clock_get`: the machine's date and time (D150).
    ClockGet,
    /// `clock_set`: sets the machine's clock (D150).
    ClockSet,
    /// `clock_sync`: sets the machine's clock from a time server (D159).
    ClockSync,
    /// `net_status`: the network's state (D157).
    NetStatus,
    /// `net_ping`: pings a host (D158).
    NetPing,
    /// `net_lookup`: looks up a host name (D158).
    NetLookup,
    /// `fetch`: downloads an `http://` URL into a file (D158).
    Fetch,
    /// `tee`: writes a file.
    Tee,
    /// `trash`: moves a file or folder to the Trash.
    Trash,
    /// `run`: runs a program.
    Run,
    /// `app_install`.
    AppInstall,
    /// `app_open`.
    AppOpen,
    /// `edit_open`: a text file in a Text Editor window (D149).
    EditOpen,
    /// `app_uninstall`.
    AppUninstall,
    /// `wm_capture`: a window's content as a PNG.
    WmCapture,
    /// `wm_click`.
    WmClick,
    /// `wm_type`.
    WmType,
    /// `wm_press`.
    WmPress,
    /// `wm_scroll`.
    WmScroll,
    /// `wm_menu_list`: a window's main menu (D147).
    WmMenuList,
    /// `wm_menu_choose`: chooses an item of a window's main menu (D147).
    WmMenuChoose,
    /// `wm_focus` (D160).
    WmFocus,
    /// `wm_move` (D160).
    WmMove,
    /// `wm_size` (D160).
    WmSize,
    /// `wm_minimise` (D160).
    WmMinimise,
    /// `wm_maximise` (D160).
    WmMaximise,
    /// `wm_restore` (D160).
    WmRestore,
    /// `wm_close` (D160).
    WmClose,
    /// `wm_show`: a window's element tree (D161).
    WmShow,
    /// `wm_act`: an action on one element of a window (D161).
    WmAct,
    /// `wm_drag` (D162).
    WmDrag,
    /// `settings_list`: every setting and its value (D163).
    SettingsList,
    /// `settings_get`: one setting's value (D163).
    SettingsGet,
    /// `settings_set`: sets a setting (D163).
    SettingsSet,
    /// `ln`: makes a symbolic or hard link (D170).
    Ln,
    /// `df`: how full the file system holding a path is (D171).
    Df,
    /// `device_list`: the machine's devices (D184).
    DeviceList,
    /// `getprop`: Android system properties (D188).
    Getprop,
    /// `setprop`: sets an Android system property (D188).
    Setprop,
    /// `logcat`: the Android log (D188).
    Logcat,
    /// `app_show`: one app and its translation kept at install (D191).
    AppShow,
}

/// MCP's tool annotations: hints a client shows its user, not guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// `readOnlyHint`.
    pub read_only: bool,
    /// `destructiveHint`.
    pub destructive: bool,
    /// `idempotentHint`.
    pub idempotent: bool,
    /// `openWorldHint`.
    pub open_world: bool,
}

/// The program a twin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// This program, by its absolute path.
    Fixed(&'static str),
    /// The one the arguments name (`run`).
    FromArguments,
}

/// What goes last on a twin's command line, after `--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// Nothing.
    None,
    /// A path, resolved and checked.
    Path(PathArgument),
    /// An app's name.
    Name,
    /// A window's id, then the tool's other positional words (a point, text, keys or a
    /// scroll), each its own word.
    Window,
}

/// What a twin reads on its standard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdin {
    /// Nothing: end of input at once.
    Empty,
    /// The call's content (`tee`).
    Content,
}

/// What becomes of a twin's standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// It is the result, cut at the output limit.
    Captured,
    /// It is read and dropped (`tee` echoes its input).
    Discarded,
}

/// A tool's CLI twin: the program, the words before its operand, the operand, its input and
/// what becomes of its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Twin {
    /// The program.
    pub program: Program,
    /// The words between the program and `--` and the operand (or the end).
    pub words: &'static [&'static str],
    /// The operand, which goes last after `--`.
    pub operand: Operand,
    /// Its standard input.
    pub stdin: Stdin,
    /// Its standard output.
    pub output: Output,
}

/// An MCP tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// Its definition in the registry: name, description, arguments and safety.
    pub definition: &'static ToolDefinition,
    /// Its name.
    pub name: &'static str,
    /// Which tool it is.
    pub kind: Kind,
    /// The grant it needs; `None` for reading.
    pub grant: Option<Grant>,
    /// When it needs a yes.
    pub confirmation: Confirmation,
    /// Its annotations.
    pub hints: Hints,
    /// Its CLI twin.
    pub twin: Twin,
}

impl Tool {
    /// Whether it takes an argument of this name.
    pub fn takes(&self, name: &str) -> bool {
        self.definition.takes(name)
    }
}

const fn grant_of(grant: Option<eggos_commands::Grant>) -> Option<Grant> {
    match grant {
        None => None,
        Some(eggos_commands::Grant::View) => Some(Grant::View),
        Some(eggos_commands::Grant::Write) => Some(Grant::Write),
        Some(eggos_commands::Grant::Install) => Some(Grant::Install),
        Some(eggos_commands::Grant::Run) => Some(Grant::Run),
    }
}

const fn tool(definition: &'static ToolDefinition, kind: Kind, twin: Twin) -> Tool {
    Tool {
        definition,
        name: definition.name,
        kind,
        grant: grant_of(definition.grant),
        confirmation: definition.confirm,
        hints: Hints {
            read_only: definition.read_only(),
            destructive: definition.destructive(),
            idempotent: definition.idempotent,
            open_world: definition.open_world,
        },
        twin,
    }
}

const READ_PATH: PathArgument = PathArgument {
    name: "path",
    default: None,
    roots: &READ_ROOTS,
    strictly: false,
    excluded: &[],
    outside: "Refused: the path is outside /home, /media/host and /work, the folders this tool may reach.",
};

const WRITE_PATH: PathArgument = PathArgument {
    name: "path",
    default: None,
    roots: &WRITE_ROOTS,
    strictly: true,
    excluded: &[TRASH],
    outside: "Refused: the path is outside /home/root, the folder this tool may change.",
};

const LINK_PATH: PathArgument = PathArgument {
    name: "link",
    ..WRITE_PATH
};

/// `ln`'s `target` for a hard link: a file the write tools reach, so a file elsewhere is never
/// given a second name an agent could write through (D170).
pub const HARD_LINK_TARGET: PathArgument = PathArgument {
    name: "target",
    outside: "Refused: a hard link's target must be below /home/root, the folder this tool may \
              change.",
    ..WRITE_PATH
};

const INSTALL_PATH: PathArgument = PathArgument {
    name: "path",
    default: None,
    roots: &INSTALL_ROOTS,
    strictly: true,
    excluded: &[],
    outside: "Refused: apps are installed from /media/host, the host's shared folder, only.",
};

/// `run`'s program: any absolute path.
pub const PROGRAM_PATH: PathArgument = PathArgument {
    name: "program",
    default: None,
    roots: &ANYWHERE,
    strictly: false,
    excluded: &[],
    outside: "Refused: the program is outside the machine's folders.",
};

const fn twin(program: &'static str, words: &'static [&'static str], operand: Operand) -> Twin {
    Twin {
        program: Program::Fixed(program),
        words,
        operand,
        stdin: Stdin::Empty,
        output: Output::Captured,
    }
}

const fn window_twin(words: &'static [&'static str]) -> Twin {
    twin("/bin/wm", words, Operand::Window)
}

/// Every tool, in `tools/list`'s order (the registry's).
pub const TOOLS: [Tool; 46] = [
    tool(
        &registry::LS,
        Kind::Ls,
        twin(
            "/bin/ls",
            &["-l"],
            Operand::Path(PathArgument {
                default: Some("/home"),
                ..READ_PATH
            }),
        ),
    ),
    tool(
        &registry::CAT,
        Kind::Cat,
        twin("/bin/cat", &[], Operand::Path(READ_PATH)),
    ),
    tool(&registry::PS, Kind::Ps, twin("/bin/ps", &[], Operand::None)),
    tool(
        &registry::WM_LIST,
        Kind::WmList,
        twin("/bin/wm", &["list"], Operand::None),
    ),
    tool(
        &registry::APP_LIST,
        Kind::AppList,
        twin("/bin/app", &["list"], Operand::None),
    ),
    tool(
        &registry::CLOCK_GET,
        Kind::ClockGet,
        twin("/bin/date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"], Operand::None),
    ),
    tool(
        &registry::NET_STATUS,
        Kind::NetStatus,
        twin("/bin/ip", &["-j", "addr"], Operand::None),
    ),
    tool(
        &registry::NET_PING,
        Kind::NetPing,
        twin("/bin/ping", &["-W", "1", "-c"], Operand::None),
    ),
    tool(
        &registry::NET_LOOKUP,
        Kind::NetLookup,
        twin("/bin/nslookup", &[], Operand::None),
    ),
    tool(
        &registry::TEE,
        Kind::Tee,
        Twin {
            program: Program::Fixed("/bin/tee"),
            words: &[],
            operand: Operand::Path(WRITE_PATH),
            stdin: Stdin::Content,
            output: Output::Discarded,
        },
    ),
    tool(
        &registry::TRASH,
        Kind::Trash,
        twin("/bin/trash", &[], Operand::Path(WRITE_PATH)),
    ),
    tool(
        &registry::FETCH,
        Kind::Fetch,
        twin("/bin/fetch", &[], Operand::Path(WRITE_PATH)),
    ),
    tool(
        &registry::RUN,
        Kind::Run,
        Twin {
            program: Program::FromArguments,
            words: &[],
            operand: Operand::None,
            stdin: Stdin::Empty,
            output: Output::Captured,
        },
    ),
    tool(
        &registry::CLOCK_SET,
        Kind::ClockSet,
        twin("/bin/date", &["-u", "-s"], Operand::None),
    ),
    tool(
        &registry::CLOCK_SYNC,
        Kind::ClockSync,
        twin("/bin/date", &["-u", "--sync"], Operand::None),
    ),
    tool(
        &registry::APP_INSTALL,
        Kind::AppInstall,
        twin("/bin/app", &["install"], Operand::Path(INSTALL_PATH)),
    ),
    tool(
        &registry::APP_OPEN,
        Kind::AppOpen,
        twin("/bin/app", &["open"], Operand::Name),
    ),
    tool(
        &registry::EDIT_OPEN,
        Kind::EditOpen,
        twin("/bin/edit", &[], Operand::Path(READ_PATH)),
    ),
    tool(
        &registry::APP_UNINSTALL,
        Kind::AppUninstall,
        twin("/bin/app", &["uninstall"], Operand::Name),
    ),
    tool(
        &registry::WM_CAPTURE,
        Kind::WmCapture,
        window_twin(&["capture", "-o", CAPTURE_PATH]),
    ),
    tool(&registry::WM_CLICK, Kind::WmClick, window_twin(&["click"])),
    tool(&registry::WM_TYPE, Kind::WmType, window_twin(&["type"])),
    tool(&registry::WM_PRESS, Kind::WmPress, window_twin(&["press"])),
    tool(
        &registry::WM_SCROLL,
        Kind::WmScroll,
        window_twin(&["scroll"]),
    ),
    tool(
        &registry::WM_MENU_LIST,
        Kind::WmMenuList,
        window_twin(&["menu"]),
    ),
    tool(
        &registry::WM_MENU_CHOOSE,
        Kind::WmMenuChoose,
        window_twin(&["menu", "--choose"]),
    ),
    tool(&registry::WM_FOCUS, Kind::WmFocus, window_twin(&["focus"])),
    tool(&registry::WM_MOVE, Kind::WmMove, window_twin(&["move"])),
    tool(&registry::WM_SIZE, Kind::WmSize, window_twin(&["size"])),
    tool(
        &registry::WM_MINIMISE,
        Kind::WmMinimise,
        window_twin(&["minimise"]),
    ),
    tool(
        &registry::WM_MAXIMISE,
        Kind::WmMaximise,
        window_twin(&["maximise"]),
    ),
    tool(
        &registry::WM_RESTORE,
        Kind::WmRestore,
        window_twin(&["restore"]),
    ),
    tool(&registry::WM_CLOSE, Kind::WmClose, window_twin(&["close"])),
    tool(&registry::WM_SHOW, Kind::WmShow, window_twin(&["show"])),
    tool(&registry::WM_ACT, Kind::WmAct, window_twin(&["act"])),
    tool(&registry::WM_DRAG, Kind::WmDrag, window_twin(&["drag"])),
    tool(
        &registry::SETTINGS_LIST,
        Kind::SettingsList,
        twin("/bin/settings", &["list"], Operand::None),
    ),
    tool(
        &registry::SETTINGS_GET,
        Kind::SettingsGet,
        twin("/bin/settings", &["get"], Operand::None),
    ),
    tool(
        &registry::SETTINGS_SET,
        Kind::SettingsSet,
        twin("/bin/settings", &["set"], Operand::None),
    ),
    tool(
        &registry::LN,
        Kind::Ln,
        twin("/bin/ln", &["-T"], Operand::Path(LINK_PATH)),
    ),
    tool(
        &registry::DF,
        Kind::Df,
        twin(
            "/bin/df",
            &["--json"],
            Operand::Path(PathArgument {
                default: Some("/home"),
                ..READ_PATH
            }),
        ),
    ),
    tool(
        &registry::DEVICE_LIST,
        Kind::DeviceList,
        twin("/bin/device", &["list", "--json"], Operand::None),
    ),
    tool(
        &registry::GETPROP,
        Kind::Getprop,
        twin("/bin/getprop", &[], Operand::None),
    ),
    tool(
        &registry::SETPROP,
        Kind::Setprop,
        twin("/bin/setprop", &[], Operand::None),
    ),
    tool(
        &registry::LOGCAT,
        Kind::Logcat,
        twin("/bin/logcat", &["-d", "-v", "threadtime"], Operand::None),
    ),
    tool(
        &registry::APP_SHOW,
        Kind::AppShow,
        twin("/bin/app", &["show"], Operand::Name),
    ),
];

/// The tool named `name`.
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Writes a tool's `inputSchema`, a JSON Schema object, from its registry arguments.
pub fn write_input_schema(out: &mut JsonWriter<'_>, definition: &ToolDefinition) {
    out.begin_object();
    out.key("type");
    out.string("object");
    out.key("properties");
    out.begin_object();
    for argument in definition.arguments {
        out.key(argument.name);
        write_argument_schema(out, argument);
    }
    out.end_object();
    if definition
        .arguments
        .iter()
        .any(|argument| argument.required)
    {
        out.key("required");
        out.begin_array();
        for argument in definition
            .arguments
            .iter()
            .filter(|argument| argument.required)
        {
            out.string(argument.name);
        }
        out.end_array();
    }
    out.key("additionalProperties");
    out.bool(false);
    out.end_object();
}

fn write_argument_schema(out: &mut JsonWriter<'_>, argument: &Argument) {
    out.begin_object();
    out.key("type");
    out.string(argument.json.name());
    if argument.json == JsonType::StringArray {
        out.key("items");
        out.begin_object();
        out.key("type");
        out.string(JsonType::String.name());
        out.end_object();
    }
    if !argument.choices.is_empty() {
        out.key("enum");
        out.begin_array();
        for choice in argument.choices {
            out.string(choice);
        }
        out.end_array();
    }
    if let Some(pattern) = argument.pattern {
        out.key("pattern");
        out.string(pattern);
    }
    for (key, value) in [
        ("minLength", argument.min_length),
        ("maxLength", argument.max_length),
    ] {
        if let Some(value) = value {
            out.key(key);
            out.u64(u64::from(value));
        }
    }
    for (key, value) in [("minimum", argument.minimum), ("maximum", argument.maximum)] {
        if let Some(value) = value {
            out.key(key);
            out.i64(value);
        }
    }
    let number = argument.json == JsonType::Integer;
    if number {
        write_default(out, argument.default);
    }
    if let Some(description) = argument.description {
        out.key("description");
        out.string(description);
    }
    if !number {
        write_default(out, argument.default);
    }
    out.end_object();
}

fn write_default(out: &mut JsonWriter<'_>, default: DefaultValue) {
    match default {
        DefaultValue::None => {}
        DefaultValue::Text(text) => {
            out.key("default");
            out.string(text);
        }
        DefaultValue::Integer(value) => {
            out.key("default");
            out.i64(value);
        }
        DefaultValue::Boolean(value) => {
            out.key("default");
            out.bool(value);
        }
        DefaultValue::EmptyList => {
            out.key("default");
            out.begin_array();
            out.end_array();
        }
    }
}

/// Writes `tools/list`'s result: every tool with its schema and annotations.
pub fn write_list(out: &mut JsonWriter<'_>) {
    out.begin_object();
    out.key("tools");
    out.begin_array();
    for tool in &TOOLS {
        let definition = tool.definition;
        out.begin_object();
        out.key("name");
        out.string(tool.name);
        out.key("title");
        out.string(definition.title);
        out.key("description");
        out.string(definition.description);
        out.key("inputSchema");
        write_input_schema(out, definition);
        out.key("annotations");
        out.begin_object();
        out.key("title");
        out.string(definition.title);
        out.key("readOnlyHint");
        out.bool(tool.hints.read_only);
        out.key("destructiveHint");
        out.bool(tool.hints.destructive);
        out.key("idempotentHint");
        out.bool(tool.hints.idempotent);
        out.key("openWorldHint");
        out.bool(tool.hints.open_world);
        out.end_object();
        out.end_object();
    }
    out.end_array();
    out.end_object();
}

#[cfg(test)]
mod tests;
