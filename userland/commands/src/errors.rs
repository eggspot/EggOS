//! EggOS's error codes (D160, D163): the ones every command can end with, each command's own, and
//! the MCP server's refusals. Codes are EggOS's own; the category is shared with EggDB.

use crate::{Category, ErrorDefinition};

const fn error(
    code: &'static str,
    category: Category,
    fix: &'static str,
    fix_command: Option<&'static str>,
) -> ErrorDefinition {
    ErrorDefinition {
        code,
        category,
        fix,
        fix_command,
    }
}

/// The command line was written wrongly.
pub const USAGE: ErrorDefinition = error(
    "usage",
    Category::Usage,
    "Correct the command line; help <command> shows its options and operands.",
    None,
);
/// A path names nothing.
pub const NO_SUCH_FILE: ErrorDefinition = error(
    "no_such_file",
    Category::NotFound,
    "Check the path: ls shows what a folder holds.",
    None,
);
/// The user or the file's mode does not allow it.
pub const PERMISSION_DENIED: ErrorDefinition = error(
    "permission_denied",
    Category::Refused,
    "Use a path the user may reach.",
    None,
);
/// Something is in the way.
pub const ALREADY_EXISTS: ErrorDefinition = error(
    "already_exists",
    Category::Exists,
    "Choose another name, or remove what is there first.",
    None,
);
/// No process has that id (D200).
pub const NO_SUCH_PROCESS: ErrorDefinition = error(
    "no_such_process",
    Category::NotFound,
    "The process has ended, or the id is wrong: ps lists the processes.",
    Some("ps"),
);
/// A folder where a file was wanted.
pub const IS_A_FOLDER: ErrorDefinition = error(
    "is_a_folder",
    Category::Usage,
    "Name a file, not a folder.",
    None,
);
/// A file where a folder was wanted.
pub const NOT_A_FOLDER: ErrorDefinition = error(
    "not_a_folder",
    Category::Usage,
    "Name a folder, or a path whose parents are folders.",
    None,
);
/// The file system can only be read.
pub const READ_ONLY: ErrorDefinition = error(
    "read_only",
    Category::ReadOnly,
    "Write below /home/root, or to a disk mounted for writing.",
    None,
);
/// The disk or a size limit is full.
pub const NO_SPACE: ErrorDefinition = error(
    "no_space",
    Category::TooLarge,
    "Free some space, or write less.",
    None,
);
/// Something else holds what it needs.
pub const BUSY: ErrorDefinition = error("busy", Category::Busy, "Try again in a moment.", None);
/// It took too long.
pub const TIMED_OUT: ErrorDefinition = error(
    "timed_out",
    Category::Timeout,
    "Try again; check that the other end answers.",
    None,
);
/// A device or file could not be read whole.
pub const DAMAGED: ErrorDefinition = error(
    "io_error",
    Category::Damaged,
    "Check the disk: the data may be damaged.",
    None,
);
/// Memory ran short.
pub const OUT_OF_MEMORY: ErrorDefinition = error(
    "out_of_memory",
    Category::TooLarge,
    "Close some programs and try again.",
    None,
);
/// Any other failure.
pub const FAILED: ErrorDefinition = error(
    "failed",
    Category::Failed,
    "Read the message; the command did not finish.",
    None,
);

/// `ls`: a folder holds more entries than `ls` lists at once.
pub const TOO_MANY_ENTRIES: ErrorDefinition = error(
    "too_many_entries",
    Category::TooLarge,
    "List it a page at a time with --limit, then --after with the cursor each page ends with.",
    None,
);
/// A list command's `--after` (or a list tool's `after`) is not a cursor a page ended with
/// (D164).
pub const BAD_CURSOR: ErrorDefinition = error(
    "bad_cursor",
    Category::Usage,
    "Pass the cursor a page ended with, as it was written, or leave --after out to start again.",
    None,
);
/// `wm`: no window has the id.
pub const NO_SUCH_WINDOW: ErrorDefinition = error(
    "no_such_window",
    Category::NotFound,
    "Use an id that wm list shows.",
    Some("wm list"),
);
/// `wm`: a person is using the keyboard or the pointer (exit 75).
pub const PERSON_ACTIVE: ErrorDefinition = error(
    "person_active",
    Category::Busy,
    "Wait a moment and try again: a person always goes first.",
    None,
);
/// `wm act`, `wm type` and `wm menu`: the window's app has left an event unread for 2 s, so
/// the window is not responding (exit 75, D222).
pub const NOT_RESPONDING: ErrorDefinition = error(
    "not_responding",
    Category::Busy,
    "Wait for the app to catch up and try again; wm list shows (not responding) until it does, \
     and wm close ends a window that stays so.",
    Some("wm list"),
);
/// `wm`: there is no window server (`/dev/window` cannot be opened).
pub const NO_WINDOW_SERVER: ErrorDefinition = error(
    "no_window_server",
    Category::NotFound,
    "Start EggOS with its desktop, or with --no-window for windows without a screen.",
    None,
);
/// `wm menu`: the window has no main menu, or no such item.
pub const NO_SUCH_MENU_ITEM: ErrorDefinition = error(
    "no_such_menu_item",
    Category::NotFound,
    "Use the numbers or the label wm menu shows.",
    None,
);
/// `wm click`, `wm drag` and `wm scroll`: the window has a dialog open, so a point in it means
/// nothing; the dialog takes the input instead (D162).
pub const DIALOG_OPEN: ErrorDefinition = error(
    "dialog_open",
    Category::Busy,
    "The window waits for its dialog: see the dialog with wm capture and the id the error \
     names, then aim at the dialog.",
    Some("wm capture"),
);
/// `wm show` and `wm act`: the window's app gives no element tree (D161).
pub const PIXELS_ONLY: ErrorDefinition = error(
    "pixels_only",
    Category::NotFound,
    "The app does not describe its window: see it with wm capture, then use wm click and \
     wm type.",
    Some("wm capture"),
);
/// `wm act`: the window has no element of that id now.
pub const NO_SUCH_ELEMENT: ErrorDefinition = error(
    "no_such_element",
    Category::NotFound,
    "Read the window's elements again: wm show lists them with their ids.",
    Some("wm show"),
);
/// `wm act`: the element does not take that action.
pub const ELEMENT_CANNOT: ErrorDefinition = error(
    "element_cannot",
    Category::Usage,
    "Use an action the element's role takes: invoke buttons, checkboxes, options and items; \
     set-text text fields; select options and items; focus what is focusable.",
    None,
);
/// `wm act`: the element is disabled.
pub const ELEMENT_DISABLED: ErrorDefinition = error(
    "element_disabled",
    Category::Refused,
    "The element is disabled now: do what enables it first, then read the window again.",
    Some("wm show"),
);
/// `app`: no installed app has the name.
pub const NO_SUCH_APP: ErrorDefinition = error(
    "no_such_app",
    Category::NotFound,
    "Use a name app list shows.",
    Some("app list"),
);
/// `app do`: the app offers no action of that name (D292).
pub const NO_SUCH_ACTION: ErrorDefinition = error(
    "no_such_action",
    Category::NotFound,
    "Use an action app actions shows for the app.",
    Some("app actions"),
);
/// `app install`: an app of that name is installed already.
pub const APP_EXISTS: ErrorDefinition = error(
    "app_exists",
    Category::Exists,
    "Uninstall it first, or install under another name with --name.",
    None,
);
/// `app install`: the file is not a program EggOS runs.
pub const NOT_AN_APP: ErrorDefinition = error(
    "not_an_app",
    Category::Usage,
    "Install a Windows .exe, a macOS .app, a Linux program or a .zip holding one.",
    None,
);
/// `trash -r`: nothing of that name is in the Trash.
pub const NOT_IN_TRASH: ErrorDefinition = error(
    "not_in_trash",
    Category::NotFound,
    "Use a name trash -l shows.",
    Some("trash -l"),
);
/// `help`: no command has those words.
pub const NO_SUCH_COMMAND: ErrorDefinition = error(
    "no_such_command",
    Category::NotFound,
    "Use the words help lists.",
    Some("help"),
);

/// `settings`: no setting has the key (D163).
pub const NO_SUCH_SETTING: ErrorDefinition = error(
    "no_such_setting",
    Category::NotFound,
    "Use a key settings list shows.",
    Some("settings list"),
);
/// `settings set`: the setting reports a fact and cannot be set (D163).
pub const SETTING_READ_ONLY: ErrorDefinition = error(
    "setting_read_only",
    Category::ReadOnly,
    "Set a setting settings list marks settable; this one reports a fact.",
    Some("settings list"),
);
/// `settings set`: the setting does not take the value (D163).
pub const INVALID_VALUE: ErrorDefinition = error(
    "invalid_value",
    Category::Usage,
    "Use one of the values the message lists.",
    None,
);
/// `settings set`: the value was saved, but the desktop did not take it now (D163).
pub const NOT_APPLIED: ErrorDefinition = error(
    "not_applied",
    Category::Failed,
    "The value is saved and applies when the desktop starts next; try again to apply it now.",
    None,
);
/// `disk`: no disk of that name is attached (D192).
pub const NO_SUCH_DISK: ErrorDefinition = error(
    "no_such_disk",
    Category::NotFound,
    "Name a disk disk list shows: /dev/vda to /dev/vdd.",
    Some("disk list"),
);
/// `agent`: EggOS does not know the agent (D193).
pub const NO_SUCH_AGENT: ErrorDefinition = error(
    "no_such_agent",
    Category::NotFound,
    "Name an agent agent list shows, or give it a grant first: agent grant <id> <grant>.",
    Some("agent list"),
);
/// `agent`: the agents' grants file holds as many agents as it can (D193).
pub const TOO_MANY_AGENTS: ErrorDefinition = error(
    "too_many_agents",
    Category::TooLarge,
    "Forget an agent no longer used (agent forget <id>), then try again.",
    Some("agent list"),
);
/// `disk`: a file system holds the disk (D192).
pub const DISK_IN_USE: ErrorDefinition = error(
    "disk_in_use",
    Category::Busy,
    "A mounted disk cannot be checked, repaired or formatted in this version.",
    None,
);
/// `disk check` and `disk repair`: the disk holds no EggFS file system (D192).
pub const NOT_EGGFS: ErrorDefinition = error(
    "not_eggfs",
    Category::Refused,
    "Format the disk to make it an EggFS disk.",
    Some("disk format"),
);
/// `disk format`: the label is longer than 64 bytes (D192).
pub const LABEL_TOO_LONG: ErrorDefinition = error(
    "label_too_long",
    Category::Usage,
    "Use a label of at most 64 bytes.",
    None,
);
/// `disk repair`: problems are left that cannot be repaired safely (D192).
pub const NOT_REPAIRABLE: ErrorDefinition = error(
    "not_repairable",
    Category::Damaged,
    "Copy what can still be read from the disk, then format it.",
    None,
);

/// MCP: the path is outside the folders the tool may reach.
pub const PATH_NOT_ALLOWED: ErrorDefinition = error(
    "path_not_allowed",
    Category::Refused,
    "Use a path inside the folders the tool's description names.",
    None,
);
/// MCP: the tool needs a grant this boot does not have.
pub const NEEDS_GRANT: ErrorDefinition = error(
    "needs_grant",
    Category::NeedsGrant,
    "Ask the user to start EggOS with the grant (run-eggos.sh --mcp --mcp-allow <grant>).",
    None,
);
/// MCP: the call needs `confirm: true` from a client that cannot ask the user.
pub const NEEDS_CONFIRMATION: ErrorDefinition = error(
    "needs_confirmation",
    Category::NeedsConfirmation,
    "Ask the user, then call again with confirm: true once they agreed.",
    None,
);
/// MCP: the user said no, or dismissed the question (D121's code).
pub const DECLINED_BY_USER: ErrorDefinition = error(
    "declined_by_user",
    Category::Declined,
    "Do not retry unless the user asks again.",
    None,
);
/// MCP: the client could not ask the user, or its answer was not understood (D121's code).
pub const CONFIRMATION_FAILED: ErrorDefinition = error(
    "confirmation_failed",
    Category::Failed,
    "Call again with confirm: true once the user agreed in the conversation.",
    None,
);
/// MCP: the user did not answer the question in time.
pub const CONFIRMATION_TIMED_OUT: ErrorDefinition = error(
    "confirmation_timed_out",
    Category::Timeout,
    "Call again when the user is there to answer.",
    None,
);
/// MCP: another call waits for the user's answer.
pub const QUESTION_PENDING: ErrorDefinition = error(
    "question_pending",
    Category::Busy,
    "Wait for the user's answer to the other call, then try again.",
    None,
);
/// MCP: what the call would change changed while the user was asked.
pub const TARGET_CHANGED: ErrorDefinition = error(
    "target_changed",
    Category::Busy,
    "Look at the target again, then call again.",
    None,
);
/// MCP: the audit log could not be written, so nothing ran.
pub const AUDIT_FAILED: ErrorDefinition = error(
    "audit_failed",
    Category::Failed,
    "Try again; if it keeps failing the user should restart EggOS.",
    None,
);
/// MCP: this server run wrote as much to the audit log as it may.
pub const AUDIT_FULL: ErrorDefinition = error(
    "audit_full",
    Category::TooLarge,
    "The user should restart EggOS's MCP server.",
    None,
);
/// MCP: the request id is too long to keep while the user is asked.
pub const REQUEST_ID_TOO_LONG: ErrorDefinition = error(
    "request_id_too_long",
    Category::Usage,
    "Use a shorter request id.",
    None,
);
/// `readlink`: the path is not a symbolic link (D170).
pub const NOT_A_LINK: ErrorDefinition = error(
    "not_a_link",
    Category::Usage,
    "Name a symbolic link; readlink -f resolves any path.",
    None,
);
/// MCP: `wm_capture`'s image could not be made, read or returned.
pub const CAPTURE_FAILED: ErrorDefinition = error(
    "capture_failed",
    Category::Failed,
    "Try again, or with a smaller max_width.",
    None,
);

/// No download since boot has that id (D225).
pub const NO_SUCH_DOWNLOAD: ErrorDefinition = error(
    "no_such_download",
    Category::NotFound,
    "Check the id: download list shows the downloads since boot.",
    None,
);
/// The download is over, or its fetch is gone, so there is nothing to cancel (D225).
pub const ALREADY_ENDED: ErrorDefinition = error(
    "already_ended",
    Category::Failed,
    "Nothing to do: download list shows how it ended.",
    None,
);
/// The download is still running, so its record stays (D225).
pub const STILL_DOWNLOADING: ErrorDefinition = error(
    "still_downloading",
    Category::Busy,
    "Wait for it to end, or cancel it first with download cancel.",
    None,
);

/// The registry key does not exist (D231).
pub const NO_SUCH_KEY: ErrorDefinition = error(
    "no_such_key",
    Category::NotFound,
    "Check the key: windows registry get shows a key's subkeys.",
    None,
);
/// The registry value does not exist (D231).
pub const NO_SUCH_VALUE: ErrorDefinition = error(
    "no_such_value",
    Category::NotFound,
    "Check the name: windows registry get shows a key's values.",
    None,
);
/// The key is in the machine's hive, which only an administrator changes (D231).
pub const READ_ONLY_KEY: ErrorDefinition = error(
    "read_only_key",
    Category::ReadOnly,
    "Change it under HKEY_CURRENT_USER, or as an administrator.",
    None,
);
/// A predefined key cannot be created, deleted or edited (D231).
pub const ROOT_KEY: ErrorDefinition = error(
    "root_key",
    Category::Refused,
    "Name a key below the root.",
    None,
);
/// The key has subkeys, so it cannot be deleted alone (D231).
pub const HAS_SUBKEYS: ErrorDefinition = error(
    "has_subkeys",
    Category::Refused,
    "Delete its subkeys first, or the whole key with windows registry delete.",
    None,
);
/// A key or value name is empty, too long or holds a backslash (D231).
pub const BAD_NAME: ErrorDefinition = error(
    "bad_name",
    Category::Usage,
    "Use a shorter name without a backslash.",
    None,
);
/// The data cannot be read as the value's type (D231).
pub const BAD_DATA: ErrorDefinition = error(
    "bad_data",
    Category::Usage,
    "Give a number, in decimal or after 0x, or pairs of hex digits for binary.",
    None,
);
/// The value is larger than a registry value may be (D231).
pub const VALUE_TOO_LARGE: ErrorDefinition = error(
    "value_too_large",
    Category::TooLarge,
    "Store large data in a file and its path in the registry.",
    None,
);
/// The key would be deeper than the registry allows (D231).
pub const KEY_TOO_DEEP: ErrorDefinition = error(
    "key_too_deep",
    Category::TooLarge,
    "Make the key higher up.",
    None,
);
/// The registry file has reached its size limit (D231).
pub const HIVE_FULL: ErrorDefinition = error(
    "hive_full",
    Category::TooLarge,
    "Delete keys you no longer need.",
    None,
);
/// The registry file is damaged (D231).
pub const HIVE_DAMAGED: ErrorDefinition = error(
    "hive_damaged",
    Category::Damaged,
    "Replace it with an empty one; the damaged file is kept beside it.",
    Some("windows registry reset"),
);
/// The registry file could not be written (D231).
pub const WRITE_FAILED: ErrorDefinition = error(
    "write_failed",
    Category::Failed,
    "Check the disk and try again.",
    None,
);

/// The definition of the error code `code`, if EggOS defines it.
pub fn by_code(code: &[u8]) -> Option<&'static ErrorDefinition> {
    ALL.iter()
        .copied()
        .find(|error| error.code.as_bytes() == code)
}

/// `component`: no component store is attached (D235).
pub const NO_STORE: ErrorDefinition = error(
    "no_store",
    Category::NotFound,
    "Attach the component store disk, or put its .img in the host share.",
    None,
);
/// `component`: neither the store nor the installed components know the name (D235).
pub const NO_SUCH_COMPONENT: ErrorDefinition = error(
    "no_such_component",
    Category::NotFound,
    "Check the name: component list shows the components.",
    Some("component list"),
);
/// `component`: another install or remove is running (D235).
pub const COMPONENT_BUSY: ErrorDefinition = error(
    "component_busy",
    Category::Busy,
    "Wait for the other install or remove, then try again.",
    None,
);
/// `component`: an installed component needs it (D235).
pub const NEEDED_BY: ErrorDefinition = error(
    "needed_by",
    Category::Refused,
    "Remove the components that need it first.",
    None,
);
/// `component`: a running program needs it (D235).
pub const IN_USE: ErrorDefinition = error(
    "in_use",
    Category::Busy,
    "Quit the programs that use it, then remove it.",
    None,
);
/// `component`: a file in the store does not match its SHA-256; nothing was installed (D235).
pub const STORE_DAMAGED: ErrorDefinition = error(
    "store_damaged",
    Category::Damaged,
    "Get the component store again; nothing was installed.",
    None,
);
/// `component`: the store could not be read before the copy finished (D235).
pub const STORE_DETACHED: ErrorDefinition = error(
    "store_detached",
    Category::Failed,
    "Attach the component store again, then install again.",
    None,
);
/// `component`: an install failed; nothing was installed (D235).
pub const INSTALL_FAILED: ErrorDefinition = error(
    "install_failed",
    Category::Failed,
    "Read the message; nothing was installed.",
    None,
);
/// `component`: a remove failed; the component stays installed (D235).
pub const REMOVE_FAILED: ErrorDefinition = error(
    "remove_failed",
    Category::Failed,
    "Read the message; the component stays installed.",
    None,
);
/// Every error code EggOS defines, for the registry test and `help --json`.
pub const ALL: [&ErrorDefinition; 80] = [
    &NO_SUCH_ACTION,
    &USAGE,
    &NO_SUCH_FILE,
    &PERMISSION_DENIED,
    &ALREADY_EXISTS,
    &IS_A_FOLDER,
    &NOT_A_FOLDER,
    &READ_ONLY,
    &NO_SPACE,
    &BUSY,
    &TIMED_OUT,
    &DAMAGED,
    &OUT_OF_MEMORY,
    &FAILED,
    &TOO_MANY_ENTRIES,
    &BAD_CURSOR,
    &NO_SUCH_WINDOW,
    &PERSON_ACTIVE,
    &NOT_RESPONDING,
    &NO_WINDOW_SERVER,
    &NO_SUCH_MENU_ITEM,
    &DIALOG_OPEN,
    &PIXELS_ONLY,
    &NO_SUCH_ELEMENT,
    &ELEMENT_CANNOT,
    &ELEMENT_DISABLED,
    &NO_SUCH_APP,
    &APP_EXISTS,
    &NOT_AN_APP,
    &NOT_IN_TRASH,
    &NOT_A_LINK,
    &NO_SUCH_COMMAND,
    &NO_SUCH_SETTING,
    &SETTING_READ_ONLY,
    &INVALID_VALUE,
    &NO_SUCH_PROCESS,
    &NOT_APPLIED,
    &NO_SUCH_DISK,
    &NO_SUCH_AGENT,
    &TOO_MANY_AGENTS,
    &DISK_IN_USE,
    &NOT_EGGFS,
    &LABEL_TOO_LONG,
    &NOT_REPAIRABLE,
    &PATH_NOT_ALLOWED,
    &NEEDS_GRANT,
    &NEEDS_CONFIRMATION,
    &DECLINED_BY_USER,
    &CONFIRMATION_FAILED,
    &CONFIRMATION_TIMED_OUT,
    &QUESTION_PENDING,
    &TARGET_CHANGED,
    &AUDIT_FAILED,
    &AUDIT_FULL,
    &REQUEST_ID_TOO_LONG,
    &CAPTURE_FAILED,
    &NO_SUCH_DOWNLOAD,
    &ALREADY_ENDED,
    &STILL_DOWNLOADING,
    &NO_SUCH_KEY,
    &NO_SUCH_VALUE,
    &READ_ONLY_KEY,
    &ROOT_KEY,
    &HAS_SUBKEYS,
    &BAD_NAME,
    &BAD_DATA,
    &VALUE_TOO_LARGE,
    &KEY_TOO_DEEP,
    &HIVE_FULL,
    &HIVE_DAMAGED,
    &WRITE_FAILED,
    &NO_STORE,
    &NO_SUCH_COMPONENT,
    &COMPONENT_BUSY,
    &NEEDED_BY,
    &IN_USE,
    &STORE_DAMAGED,
    &STORE_DETACHED,
    &INSTALL_FAILED,
    &REMOVE_FAILED,
];
