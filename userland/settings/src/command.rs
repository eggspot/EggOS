//! The `settings` command's verbs (D132, D163; docs/ux/command-model.md): `settings list`,
//! `settings get <key>` and `settings set <key> <value>`, defined in the command model's
//! registry (`eggos_commands::commands::SETTINGS_LIST`, `SETTINGS_GET`, `SETTINGS_SET`), with
//! `--json` for list and get, `--dry-run` for set (D164) and coded errors. The machine is a [`Machine`], so host tests run
//! the registry's examples against example facts.

use crate::facts::Facts;
use crate::keys::{
    self, Change, DisplayChange, InputChange, Key, KeyboardChoice, KeysChange, Refusal,
};
use core::fmt::Write;
use eggos_commands::commands::LIST_PAGE_LIMIT;
use eggos_commands::page::{self, MAX_CURSOR_BYTES, MAX_KEY_BYTES, MORE_PREFIX};
use eggos_commands::{ErrorDefinition, errors};
use eggos_mcp::json::JsonWriter;
use eggos_mcp::server::{write_error, write_page};
use eggos_settings_file::{DisplayAccessibility, InputPreferences, KeyAccessibility, PATH};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Theme;

/// The command's usage, printed for a usage error.
pub const USAGE: &str = "usage: settings [open <pane> | --pair <id> [--name <name>] | --pane \
                         <pane> | --first-run | list [--json] [--limit <count>] [--after \
                         <cursor>] | get [--json] <key> | set [--dry-run] <key> <value>]\n";
/// The exit status of a usage error.
pub const USAGE_STATUS: i32 = 2;
/// The exit status of a value the setting does not take.
pub const INVALID_VALUE_STATUS: i32 = 2;
/// The exit status of every other error.
pub const FAILURE: i32 = 1;
/// The most words the command takes after `settings`.
pub const MAX_ARGUMENTS: usize = 8;

const MAX_WORDS: usize = 3;
const RECORD_BYTES: usize = 512;
const ERROR_BYTES: usize = 1024;
const PAGE_LINE_BYTES: usize = MAX_CURSOR_BYTES + 128;
const PAGE_OPTIONS: [&str; 2] = ["--limit", "--after"];

type Message = Text<256>;

/// What the command needs from the machine.
pub trait Machine {
    /// The facts the settings report.
    fn facts(&mut self) -> Facts;
    /// Saves the desktop profile `index` to the settings file; the error's text on failure.
    fn save_profile(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Saves `shell.form` as the setting `index` of [`crate::facts::FORM_SETTINGS`] (D303).
    fn save_form(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Applies `shell.form` at once; the error's text when it is saved but another form shows.
    fn apply_form(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Saves `keyboard.on_screen` as mode `index` (D314).
    fn save_on_screen_keyboard(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Has the window server use `keyboard.on_screen` mode `index` now.
    fn apply_on_screen_keyboard(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Saves `display.rotation` as value `index` of `DISPLAY_ROTATIONS` (D330).
    fn save_rotation(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Has the window server turn the screen to `display.rotation` value `index` now.
    fn apply_rotation(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let _ = index;
        Ok(())
    }
    /// Applies the desktop profile `index` now; the error's text on failure.
    fn apply_profile(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Saves the desktop's theme to the settings file as `appearance.theme` (D219).
    fn save_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]>;
    /// Makes `theme` the desktop's now (`SET_PALETTE`): every window repaints in it.
    fn apply_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]>;
    /// Saves what the keyboard uses (`keyboard.layouts`, `keyboard.layout` and
    /// `keyboard.switch_key`) to the settings file; the error's text on failure.
    fn save_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]>;
    /// Has the keyboard use `choice` now, with or without a desktop (D221); the error's text
    /// on failure.
    fn apply_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]>;
    /// Saves the time zone `index` (of `eggos_calendar::zone::ZONES`) to the settings file;
    /// programs read it each time they show a time.
    fn save_time_zone(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Saves `time.automatic` (D225): to the settings file, or for this session in
    /// `/run/eggos/settings` without a disk.
    fn save_time_automatic(&mut self, on: bool) -> Result<(), &'static [u8]>;
    /// Starts `date --sync --automatic` in the background, which records its result in
    /// `/run/eggos/clock-sync` (D225).
    fn start_clock_sync(&mut self) -> Result<(), &'static [u8]>;
    /// Saves the display's scale to the settings file; the kernel shows at it from its next
    /// boot.
    fn save_display_scale(&mut self, scale: u32) -> Result<(), &'static [u8]>;
    /// Saves the display's size for the next boot, `None` for the display's own (D287).
    fn save_display_resolution(&mut self, size: Option<(u32, u32)>) -> Result<(), &'static [u8]>;
    /// Saves one mouse or key preference to the settings file; the kernel uses it from its
    /// next boot.
    fn save_input(&mut self, change: InputChange) -> Result<(), &'static [u8]>;
    /// Uses `preferences` from the next mouse and key event on.
    fn apply_input(&mut self, preferences: InputPreferences) -> Result<(), &'static [u8]>;
    /// Saves one key accessibility setting to the settings file (D205).
    fn save_keys(&mut self, change: KeysChange) -> Result<(), &'static [u8]>;
    /// Uses `keys` from the next key on.
    fn apply_keys(&mut self, keys: KeyAccessibility) -> Result<(), &'static [u8]>;
    /// Saves high contrast or reduced motion to the settings file (D224).
    fn save_display(&mut self, change: DisplayChange) -> Result<(), &'static [u8]>;
    /// Shows `display` at once in `theme` (`SET_PALETTE` with the high-contrast index and
    /// `SET_REDUCE_MOTION`): every window repaints.
    fn apply_display(
        &mut self,
        theme: Theme,
        display: DisplayAccessibility,
    ) -> Result<(), &'static [u8]>;
    /// Writes to standard output.
    fn write_output(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn write_error(&mut self, bytes: &[u8]);
    /// Whether the program was started with `EGGOS_CODED_ERRORS=1` (D226): an MCP tool's twin,
    /// which writes each error's `{"error":{...}}` line after its text.
    fn wants_coded_errors(&self) -> bool;
}

#[derive(Debug, Clone, Copy)]
struct Page {
    limit: Option<usize>,
    after: [u8; MAX_KEY_BYTES],
    after_length: Option<usize>,
}

impl Page {
    fn after(&self) -> Option<&[u8]> {
        self.after_length
            .and_then(|length| self.after.get(..length))
    }

    fn is_paged(&self) -> bool {
        self.limit.is_some() || self.after_length.is_some()
    }
}

/// The option that opens the window at the Agents pane's pairing panel, ready to pair an
/// agent (D193): `settings --pair <id> [--name <name>]`, what `agent pair` runs from a terminal.
pub const PAIR_OPTION: &str = "--pair";
/// With [`PAIR_OPTION`], the name the user sees.
pub const NAME_OPTION: &str = "--name";
/// Opens the window on the pane it names (D221): a pane's title in lower case, its spaces as
/// `-` and `&` as `and` (`keyboard`, `date-and-time`).
pub const PANE_OPTION: &str = "--pane";
/// The verb that opens the window on the pane it names (D225, docs/ux/network-v0.3.md section
/// 4.5): `settings open network`, what the bar's network indicator runs; the pane is named as
/// [`PANE_OPTION`] names it.
pub const OPEN_VERB: &str = "open";
/// Opens the first run's window (D224, docs/ux/first-run-accessibility-v0.3.md section 1): the
/// profile, the keyboard and the time zone, each saved as it is chosen.
pub const FIRST_RUN_OPTION: &str = "--first-run";

const WINDOW_OPTIONS: [&str; 4] = [PAIR_OPTION, NAME_OPTION, PANE_OPTION, FIRST_RUN_OPTION];

/// The agent `settings --pair` fills the pairing panel in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingAsked<'a> {
    pub id: &'a str,
    /// Empty for none.
    pub name: &'a str,
}

/// What the words after `settings` ask of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAsked<'a> {
    /// Not the window with a pairing: the other verbs, or the plain window with no words.
    Other,
    /// The window at the pairing panel, filled in.
    Pairing(PairingAsked<'a>),
    /// The window on this pane (an index of `PANES`).
    Pane(usize),
    /// The first run's window (D224).
    FirstRun,
    /// `--pair` or `--name` without what they need.
    Usage,
}

/// Reads `settings --pair <id> [--name <name>]` (the options in either order), `--pane
/// <pane>`, `open <pane>` and `--first-run` from `arguments`, the words after the program's
/// name.
pub fn window_asked<'a>(arguments: &[&'a str]) -> WindowAsked<'a> {
    let asked = |id, name| WindowAsked::Pairing(PairingAsked { id, name });
    match *arguments {
        [PAIR_OPTION, id] => asked(id, ""),
        [PAIR_OPTION, id, NAME_OPTION, name] | [NAME_OPTION, name, PAIR_OPTION, id] => {
            asked(id, name)
        }
        [FIRST_RUN_OPTION] => WindowAsked::FirstRun,
        [PANE_OPTION | OPEN_VERB, name] => match crate::settings_window::pane_named(name) {
            Some(pane) => WindowAsked::Pane(pane),
            None => WindowAsked::Usage,
        },
        [OPEN_VERB, ..] => WindowAsked::Usage,
        [first, ..] if WINDOW_OPTIONS.contains(&first) => WindowAsked::Usage,
        _ => WindowAsked::Other,
    }
}

/// Runs `settings` with `arguments`, the words after its name (at least one: with none the
/// program opens its window); the exit status.
pub fn run(machine: &mut impl Machine, arguments: &[&str]) -> i32 {
    let mut json = arguments
        .iter()
        .take_while(|&&argument| argument != "--")
        .any(|&argument| argument == "--json");
    let mut dry_run = false;
    let mut page = Page {
        limit: None,
        after: [0; MAX_KEY_BYTES],
        after_length: None,
    };
    let mut words = [""; MAX_WORDS];
    let mut count = 0;
    let mut options_end = false;
    let mut index = 0;
    while let Some(&argument) = arguments.get(index) {
        index += 1;
        if !options_end && argument == "--" {
            options_end = true;
            continue;
        }
        if let Some((name, attached)) = page_option(argument).filter(|_| !options_end) {
            let value = match attached {
                Some(value) => value,
                None => {
                    let Some(&value) = arguments.get(index) else {
                        let mut message = Message::new();
                        let _ = write!(message, "option '{name}' requires an argument");
                        report(machine, json, &errors::USAGE, message.as_str());
                        return USAGE_STATUS;
                    };
                    index += 1;
                    value
                }
            };
            if let Err(status) = take_page(machine, json, &mut page, name, value) {
                return status;
            }
            continue;
        }
        if !options_end && argument == "--json" {
            json = true;
            continue;
        }
        if !options_end && argument == "--dry-run" {
            dry_run = true;
            continue;
        }
        if !options_end && argument.len() > 1 && argument.starts_with('-') {
            let mut message = Message::new();
            let _ = write!(message, "unrecognized option '{argument}'");
            report(machine, json, &errors::USAGE, message.as_str());
            return USAGE_STATUS;
        }
        if count == MAX_WORDS {
            return usage(machine, json);
        }
        words[count] = argument;
        count += 1;
    }
    match words[..count] {
        ["list"] if !dry_run => list(machine, json, &page),
        _ if page.is_paged() => usage(machine, json),
        ["get", name] if !dry_run => get(machine, json, name),
        ["set", name, value] if !json => set(machine, name, value, dry_run),
        _ => usage(machine, json),
    }
}

fn page_option(argument: &str) -> Option<(&'static str, Option<&str>)> {
    for name in PAGE_OPTIONS {
        let Some(rest) = argument.strip_prefix(name) else {
            continue;
        };
        if rest.is_empty() {
            return Some((name, None));
        }
        if let Some(value) = rest.strip_prefix('=') {
            return Some((name, Some(value)));
        }
    }
    None
}

fn take_page(
    machine: &mut impl Machine,
    json: bool,
    page: &mut Page,
    name: &str,
    value: &str,
) -> Result<(), i32> {
    let mut message = Message::new();
    if name == "--limit" {
        let most = usize::try_from(LIST_PAGE_LIMIT).unwrap_or(usize::MAX);
        let limit = match value.bytes().all(|byte| byte.is_ascii_digit()) {
            true => value.parse::<usize>().ok(),
            false => None,
        };
        if let Some(limit) = limit.filter(|limit| (1..=most).contains(limit)) {
            page.limit = Some(limit);
            return Ok(());
        }
        let _ = write!(
            message,
            "invalid limit '{value}': a page holds 1 to {most} items"
        );
        report(machine, json, &errors::USAGE, message.as_str());
        return Err(USAGE_STATUS);
    }
    match page::decode(value.as_bytes(), &mut page.after) {
        Some(length) => {
            page.after_length = Some(length);
            Ok(())
        }
        None => {
            let _ = write!(
                message,
                "invalid cursor '{value}': not one a page ended with"
            );
            report(machine, json, &errors::BAD_CURSOR, message.as_str());
            Err(USAGE_STATUS)
        }
    }
}

fn usage(machine: &mut impl Machine, json: bool) -> i32 {
    match json {
        true => report(machine, true, &errors::USAGE, USAGE.trim_end()),
        false => {
            machine.write_error(USAGE.as_bytes());
            if machine.wants_coded_errors() {
                write_json_error(machine, &errors::USAGE, USAGE.trim_end());
            }
        }
    }
    USAGE_STATUS
}

fn write_json_error(machine: &mut impl Machine, error: &ErrorDefinition, message: &str) {
    let mut buffer = [0u8; ERROR_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    write_error(&mut out, error, message);
    out.end_line();
    if !out.overflowed() {
        machine.write_error(out.as_bytes());
    }
}

fn report(machine: &mut impl Machine, json: bool, error: &ErrorDefinition, message: &str) {
    if json {
        write_json_error(machine, error, message);
        return;
    }
    machine.write_error(b"settings: ");
    machine.write_error(message.as_bytes());
    machine.write_error(b"\n");
    if let Some(command) = error.fix_command {
        machine.write_error(b"  try: ");
        machine.write_error(command.as_bytes());
        machine.write_error(b"\n");
    }
    if machine.wants_coded_errors() {
        write_json_error(machine, error, message);
    }
}

fn write_record(machine: &mut impl Machine, key: Key, facts: &Facts) {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("key");
    out.string(key.name());
    out.key("value");
    out.string(key.value(facts).as_str());
    out.key("settable");
    out.bool(key.is_settable());
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        machine.write_output(out.as_bytes());
    }
}

fn key_name(index: usize, out: &mut [u8; MAX_KEY_BYTES]) -> usize {
    let name = Key::ALL.get(index).map_or("", |key| key.name()).as_bytes();
    let length = name.len().min(out.len());
    out[..length].copy_from_slice(&name[..length]);
    length
}

fn more(machine: &mut impl Machine, json: bool, last_key: &str) {
    let mut cursor = [0u8; MAX_CURSOR_BYTES];
    let Some(length) = page::encode(last_key.as_bytes(), &mut cursor) else {
        return;
    };
    let cursor = &cursor[..length];
    if !json {
        machine.write_error(MORE_PREFIX.as_bytes());
        machine.write_error(cursor);
        machine.write_error(b"\n");
        return;
    }
    let mut buffer = [0u8; PAGE_LINE_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("page");
    write_page(&mut out, core::str::from_utf8(cursor).ok());
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        machine.write_error(out.as_bytes());
    }
}

fn list(machine: &mut impl Machine, json: bool, page: &Page) -> i32 {
    let facts = machine.facts();
    let count = Key::ALL.len();
    let after = page.after();
    for (index, key) in Key::ALL.into_iter().enumerate() {
        if page.is_paged() {
            let Some(rank) = page::rank(count, index, after, &key_name) else {
                continue;
            };
            if page.limit.is_some_and(|limit| rank >= limit) {
                continue;
            }
        }
        match json {
            true => write_record(machine, key, &facts),
            false => {
                machine.write_output(key.name().as_bytes());
                machine.write_output(b"=");
                machine.write_output(key.value(&facts).as_str().as_bytes());
                machine.write_output(b"\n");
            }
        }
    }
    let end = page
        .limit
        .and_then(|limit| page::page_end(count, limit, after, &key_name));
    if let Some(key) = end.and_then(|end| Key::ALL.get(end)) {
        more(machine, json, key.name());
    }
    0
}

fn no_such_setting(machine: &mut impl Machine, json: bool, name: &str) -> i32 {
    let mut message = Message::new();
    let _ = write!(message, "{name}: no such setting");
    report(machine, json, &errors::NO_SUCH_SETTING, message.as_str());
    FAILURE
}

fn get(machine: &mut impl Machine, json: bool, name: &str) -> i32 {
    let Some(key) = Key::named(name) else {
        return no_such_setting(machine, json, name);
    };
    let facts = machine.facts();
    match json {
        true => write_record(machine, key, &facts),
        false => {
            machine.write_output(key.value(&facts).as_str().as_bytes());
            machine.write_output(b"\n");
        }
    }
    0
}

fn set(machine: &mut impl Machine, name: &str, value: &str, dry_run: bool) -> i32 {
    let (key, change) = match keys::change(name, value) {
        Ok(changed) => changed,
        Err(Refusal::NoSuchSetting) => return no_such_setting(machine, false, name),
        Err(Refusal::ReadOnly) => {
            let mut message = Message::new();
            let _ = write!(message, "{name}: read-only");
            report(machine, false, &errors::SETTING_READ_ONLY, message.as_str());
            return FAILURE;
        }
        Err(Refusal::InvalidValue) => {
            let choices = Key::named(name).map_or_else(Text::new, Key::choices);
            return invalid_value(machine, name, value, choices.as_str());
        }
        Err(Refusal::InvalidLayouts(refusal)) => {
            let (quoted, reason) = keys::layouts_refusal(value, refusal);
            return invalid_value(machine, name, quoted, reason.as_str());
        }
    };
    if dry_run {
        let facts = machine.facts();
        let mut message = Message::new();
        let _ = writeln!(
            message,
            "would set {name} from {} to {value}",
            key.value(&facts).as_str()
        );
        machine.write_output(message.as_str().as_bytes());
        return 0;
    }
    let keyboard = change.keyboard_choice(&machine.facts());
    let saved = match change {
        Change::Profile(index) => machine.save_profile(index),
        Change::Form(index) => machine.save_form(index),
        Change::OnScreenKeyboard(index) => machine.save_on_screen_keyboard(index),
        Change::DisplayRotation(index) => machine.save_rotation(index),
        Change::Theme(theme) => machine.save_theme(theme),
        Change::KeyboardLayout(_) | Change::KeyboardLayouts(_) | Change::KeyboardSwitchKey(_) => {
            keyboard.map_or(Ok(()), |choice| machine.save_keyboard(choice))
        }
        Change::TimeZone(index) => machine.save_time_zone(index),
        Change::Automatic(on) => machine.save_time_automatic(on),
        Change::DisplayScale(scale) => machine.save_display_scale(scale),
        Change::DisplayResolution(size) => machine.save_display_resolution(size),
        Change::Input(input) => machine.save_input(input),
        Change::Keys(keys) => machine.save_keys(keys),
        Change::Display(display) => machine.save_display(display),
    };
    if let Err(why) = saved {
        let mut message = Message::new();
        let _ = write!(message, "{PATH}: {}", text_of(why));
        report(machine, false, &errors::FAILED, message.as_str());
    }
    let applied = match change {
        Change::Profile(index) => machine.apply_profile(index),
        Change::Form(index) => machine.apply_form(index),
        Change::OnScreenKeyboard(index) => machine.apply_on_screen_keyboard(index),
        Change::DisplayRotation(index) => machine.apply_rotation(index),
        Change::Theme(theme) => machine.apply_theme(theme),
        Change::KeyboardLayout(_) | Change::KeyboardLayouts(_) | Change::KeyboardSwitchKey(_) => {
            keyboard.map_or(Ok(()), |choice| machine.apply_keyboard(choice))
        }
        Change::TimeZone(_) | Change::DisplayScale(_) | Change::DisplayResolution(_) | Change::Automatic(false) => Ok(()),
        Change::Automatic(true) if saved.is_ok() => machine.start_clock_sync(),
        Change::Automatic(true) => Ok(()),
        Change::Input(input) => {
            let preferences = input.applied_to(machine.facts().input);
            machine.apply_input(preferences)
        }
        Change::Keys(keys) => {
            let applied = keys.applied_to(machine.facts().keys);
            machine.apply_keys(applied)
        }
        Change::Display(display) => {
            let facts = machine.facts();
            machine.apply_display(facts.theme, display.applied_to(facts.display))
        }
    };
    if let Err(why) = applied {
        let mut message = Message::new();
        let _ = write!(message, "{name}: not applied: {}", text_of(why));
        report(machine, false, &errors::NOT_APPLIED, message.as_str());
    }
    match saved.is_ok() && applied.is_ok() {
        true => 0,
        false => FAILURE,
    }
}

fn invalid_value(machine: &mut impl Machine, name: &str, value: &str, choices: &str) -> i32 {
    let mut message = Message::new();
    let _ = write!(message, "{name}: invalid value '{value}' ({choices})");
    report(machine, false, &errors::INVALID_VALUE, message.as_str());
    INVALID_VALUE_STATUS
}

fn text_of(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_commands::COMMANDS;

    #[test]
    fn pair_opens_the_window_filled_in_and_anything_else_with_it_is_a_usage_error() {
        let pairing = |id, name| WindowAsked::Pairing(PairingAsked { id, name });
        assert_eq!(window_asked(&["--pair", "phone"]), pairing("phone", ""));
        assert_eq!(
            window_asked(&["--pair", "phone", "--name", "My phone"]),
            pairing("phone", "My phone")
        );
        assert_eq!(
            window_asked(&["--name", "My phone", "--pair", "phone"]),
            pairing("phone", "My phone")
        );
        for wrong in [
            &["--pair"][..],
            &["--name", "x"],
            &["--pair", "a", "--name"],
            &["--pair", "a", "b"],
        ] {
            assert_eq!(window_asked(wrong), WindowAsked::Usage, "{wrong:?}");
        }
        assert_eq!(window_asked(&["list"]), WindowAsked::Other);
        assert_eq!(window_asked(&[]), WindowAsked::Other);
        assert_eq!(window_asked(&["--first-run"]), WindowAsked::FirstRun);
        assert_eq!(window_asked(&["--first-run", "now"]), WindowAsked::Usage);
    }

    #[test]
    fn pane_opens_the_window_on_the_pane_it_names() {
        use crate::settings_window::{DATE_AND_TIME, KEYBOARD};
        assert_eq!(
            window_asked(&["--pane", "keyboard"]),
            WindowAsked::Pane(KEYBOARD)
        );
        assert_eq!(
            window_asked(&["--pane", "date-and-time"]),
            WindowAsked::Pane(DATE_AND_TIME)
        );
        for wrong in [
            &["--pane"][..],
            &["--pane", "nowhere"],
            &["--pane", "Keyboard"],
            &["--pane", "keyboard", "list"],
        ] {
            assert_eq!(window_asked(wrong), WindowAsked::Usage, "{wrong:?}");
        }
    }

    #[test]
    fn open_names_a_pane_as_the_bar_s_network_indicator_does_and_nowhere_is_a_usage_error() {
        use crate::settings_window::{DATE_AND_TIME, NETWORK, SettingsWindow};
        assert_eq!(
            window_asked(&["open", "network"]),
            WindowAsked::Pane(NETWORK)
        );
        assert_eq!(
            window_asked(&["open", "date-and-time"]),
            WindowAsked::Pane(DATE_AND_TIME)
        );
        for wrong in [
            &["open"][..],
            &["open", "nowhere"],
            &["open", "Network"],
            &["open", "network", "now"],
        ] {
            assert_eq!(window_asked(wrong), WindowAsked::Usage, "{wrong:?}");
        }
        assert_eq!(window_asked(&[]), WindowAsked::Other);
        assert!(USAGE.contains("open <pane>"));
        let mut window = SettingsWindow::new(Facts::example());
        assert_eq!(window.pane(), 0);
        window.open_pane(NETWORK);
        assert_eq!(window.pane(), NETWORK);
    }

    #[derive(Debug, Default)]
    struct Fake {
        output: Vec<u8>,
        errors: Vec<u8>,
        saved: Option<usize>,
        applied: Option<usize>,
        layout_saved: Option<KeyboardChoice>,
        layout_applied: Option<KeyboardChoice>,
        zone_saved: Option<usize>,
        automatic_saved: Option<bool>,
        syncs_started: usize,
        scale_saved: Option<u32>,
        input_saved: Option<InputChange>,
        input_applied: Option<InputPreferences>,
        keys_applied: Option<KeyAccessibility>,
        save_fails: bool,
        apply_fails: bool,
        theme_saved: Option<Theme>,
        theme_applied: Option<Theme>,
        display_saved: Option<DisplayChange>,
        display_applied: Option<(Theme, DisplayAccessibility)>,
        coded_errors: bool,
    }

    impl Machine for Fake {
        fn facts(&mut self) -> Facts {
            Facts::example()
        }

        fn save_profile(&mut self, index: usize) -> Result<(), &'static [u8]> {
            if self.save_fails {
                return Err(b"Read-only file system");
            }
            self.saved = Some(index);
            Ok(())
        }

        fn apply_profile(&mut self, index: usize) -> Result<(), &'static [u8]> {
            if self.apply_fails {
                return Err(b"No such device");
            }
            self.applied = Some(index);
            Ok(())
        }

        fn save_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]> {
            self.theme_saved = Some(theme);
            Ok(())
        }

        fn apply_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]> {
            self.theme_applied = Some(theme);
            Ok(())
        }

        fn save_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]> {
            self.layout_saved = Some(choice);
            Ok(())
        }

        fn save_time_zone(&mut self, index: usize) -> Result<(), &'static [u8]> {
            self.zone_saved = Some(index);
            Ok(())
        }

        fn save_time_automatic(&mut self, on: bool) -> Result<(), &'static [u8]> {
            if self.save_fails {
                return Err(b"Read-only file system");
            }
            self.automatic_saved = Some(on);
            Ok(())
        }

        fn start_clock_sync(&mut self) -> Result<(), &'static [u8]> {
            self.syncs_started += 1;
            Ok(())
        }

        fn save_display_scale(&mut self, scale: u32) -> Result<(), &'static [u8]> {
            self.scale_saved = Some(scale);
            Ok(())
        }

        fn save_display_resolution(&mut self, _size: Option<(u32, u32)>) -> Result<(), &'static [u8]> {
            Ok(())
        }

        fn save_input(&mut self, change: InputChange) -> Result<(), &'static [u8]> {
            self.input_saved = Some(change);
            Ok(())
        }

        fn apply_input(&mut self, preferences: InputPreferences) -> Result<(), &'static [u8]> {
            self.input_applied = Some(preferences);
            Ok(())
        }

        fn save_keys(&mut self, _change: KeysChange) -> Result<(), &'static [u8]> {
            Ok(())
        }

        fn apply_keys(&mut self, keys: KeyAccessibility) -> Result<(), &'static [u8]> {
            self.keys_applied = Some(keys);
            Ok(())
        }

        fn save_display(&mut self, change: DisplayChange) -> Result<(), &'static [u8]> {
            self.display_saved = Some(change);
            Ok(())
        }

        fn apply_display(
            &mut self,
            theme: Theme,
            display: DisplayAccessibility,
        ) -> Result<(), &'static [u8]> {
            self.display_applied = Some((theme, display));
            Ok(())
        }

        fn apply_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]> {
            if self.apply_fails {
                return Err(b"No such device");
            }
            self.layout_applied = Some(choice);
            Ok(())
        }

        fn write_output(&mut self, bytes: &[u8]) {
            self.output.extend_from_slice(bytes);
        }

        fn write_error(&mut self, bytes: &[u8]) {
            self.errors.extend_from_slice(bytes);
        }

        fn wants_coded_errors(&self) -> bool {
            self.coded_errors
        }
    }

    fn settings(fake: &mut Fake, arguments: &[&str]) -> (i32, String, String) {
        let status = run(fake, arguments);
        let output = String::from_utf8_lossy(&core::mem::take(&mut fake.output)).into_owned();
        let errors = String::from_utf8_lossy(&core::mem::take(&mut fake.errors)).into_owned();
        (status, output, errors)
    }

    #[test]
    fn every_settings_example_writes_what_the_registry_says() {
        let mut ran = 0;
        for command in COMMANDS
            .iter()
            .filter(|command| command.words[0] == "settings")
        {
            for example in command.examples {
                let words: Vec<&str> = example.line.split(' ').skip(1).collect();
                let (status, output, errors) = settings(&mut Fake::default(), &words);
                assert_eq!(
                    (status, output.as_str()),
                    (example.status, example.output),
                    "{}: {errors}",
                    example.line
                );
                ran += 1;
            }
        }
        assert!(ran >= 4, "{ran}");
    }

    #[test]
    fn list_and_get_write_records_with_json() {
        let mut fake = Fake::default();
        let (status, output, _) = settings(&mut fake, &["list", "--json"]);
        assert_eq!(status, 0);
        assert_eq!(output.lines().count(), Key::ALL.len());
        assert_eq!(
            output.lines().next(),
            Some("{\"key\":\"shell.profile\",\"value\":\"macos\",\"settable\":true}")
        );
        let (status, output, _) = settings(&mut fake, &["--json", "get", "--", "system.cpus"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "{\"key\":\"system.cpus\",\"value\":\"4\",\"settable\":false}\n"
        );
    }

    #[test]
    fn time_automatic_is_on_or_off_and_turning_it_on_syncs_at_once() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["get", "time.automatic"]),
            (0, "on\n".to_owned(), String::new())
        );
        assert_eq!(
            settings(&mut fake, &["set", "--dry-run", "time.automatic", "off"]).1,
            "would set time.automatic from on to off\n"
        );
        assert_eq!(fake.automatic_saved, None);
        assert_eq!(settings(&mut fake, &["set", "time.automatic", "off"]).0, 0);
        assert_eq!((fake.automatic_saved, fake.syncs_started), (Some(false), 0));
        assert_eq!(settings(&mut fake, &["set", "time.automatic", "on"]).0, 0);
        assert_eq!((fake.automatic_saved, fake.syncs_started), (Some(true), 1));
        let (status, _, errors) = settings(&mut fake, &["set", "time.automatic", "maybe"]);
        assert_eq!(status, INVALID_VALUE_STATUS);
        assert!(
            errors.starts_with("settings: time.automatic: invalid value 'maybe' (on or off)\n"),
            "{errors}"
        );
        let mut failing = Fake {
            save_fails: true,
            ..Fake::default()
        };
        assert_eq!(
            settings(&mut failing, &["set", "time.automatic", "on"]).0,
            FAILURE
        );
        assert_eq!(failing.syncs_started, 0);
    }

    #[test]
    fn time_sync_prints_the_last_record_and_is_read_only() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["get", "time.sync"]),
            (0, "none\n".to_owned(), String::new())
        );
        let (status, _, errors) = settings(&mut fake, &["set", "time.sync", "x"]);
        assert_eq!(status, FAILURE);
        assert!(
            errors.starts_with("settings: time.sync: read-only\n"),
            "{errors}"
        );
        use eggos_settings_file::clock_sync::{Outcome, Record};
        let mut record = Record::new(Outcome::Set, 1_793_282_700, b"pool.ntp.org");
        record.finished = Some(1_793_282_709);
        record.moved_ms = 12_000;
        let mut value = keys::Value::new();
        keys::write_clock_sync(&mut value, Some(&record)).unwrap();
        assert_eq!(
            value.as_str(),
            "set 2026-10-29T14:05:09Z pool.ntp.org 12000"
        );
        let syncing = Record::new(Outcome::Syncing, 1_793_282_709, b"time.example.com:123");
        let mut value = keys::Value::new();
        keys::write_clock_sync(&mut value, Some(&syncing)).unwrap();
        assert_eq!(
            value.as_str(),
            "syncing 2026-10-29T14:05:09Z time.example.com:123 0"
        );
    }

    #[test]
    fn set_saves_and_applies_the_profile() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["set", "--", "shell.profile", "windows"]).0,
            0
        );
        assert_eq!((fake.saved, fake.applied), (Some(0), Some(0)));
        let mut fake = Fake {
            apply_fails: true,
            ..Fake::default()
        };
        let (status, _, errors) = settings(&mut fake, &["set", "shell.profile", "linux"]);
        assert_eq!(status, 1);
        assert_eq!(fake.saved, Some(2));
        assert_eq!(
            errors,
            "settings: shell.profile: not applied: No such device\n"
        );
        let mut fake = Fake {
            save_fails: true,
            ..Fake::default()
        };
        let (status, _, errors) = settings(&mut fake, &["set", "shell.profile", "linux"]);
        assert_eq!(status, 1);
        assert_eq!(fake.applied, Some(2));
        assert!(errors.ends_with(": Read-only file system\n"), "{errors}");
    }

    #[test]
    fn set_saves_and_applies_the_keyboard_layout_and_lists_the_layouts_when_refused() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["set", "keyboard.layout", "de"]),
            (0, String::new(), String::new())
        );
        let german = eggos_settings_file::keyboard_layout_named(b"de").unwrap();
        let in_force = |choice: Option<KeyboardChoice>| choice.map(|choice| choice.in_force);
        assert_eq!(
            (in_force(fake.layout_saved), in_force(fake.layout_applied)),
            (Some(german), Some(german))
        );
        let mut text = [0u8; eggos_settings_file::KEYBOARD_LAYOUT_SET_CAPACITY];
        assert_eq!(fake.layout_saved.unwrap().layouts.write(&mut text), "us,de");
        assert_eq!(fake.saved, None);
        let (status, _, errors) = settings(&mut fake, &["set", "keyboard.layout", "xx"]);
        assert_eq!(status, 2);
        assert_eq!(
            errors,
            "settings: keyboard.layout: invalid value 'xx' (us, us-intl, us-mac, gb, \
             us-dvorak, us-colemak, dk, fi, fr, be, ca, de, ch, it, no, br, pt, es, latam \
             or se)\n"
        );
        let mut fake = Fake {
            apply_fails: true,
            ..Fake::default()
        };
        let (status, _, errors) = settings(&mut fake, &["set", "keyboard.layout", "fr"]);
        assert_eq!(status, 1);
        assert_eq!(
            in_force(fake.layout_saved),
            eggos_settings_file::keyboard_layout_named(b"fr")
        );
        assert_eq!(
            errors,
            "settings: keyboard.layout: not applied: No such device\n"
        );
    }

    #[test]
    fn a_dry_run_checks_the_key_and_value_and_changes_nothing() {
        let mut fake = Fake::default();
        let (status, output, _) = settings(
            &mut fake,
            &["set", "--dry-run", "--", "shell.profile", "linux"],
        );
        assert_eq!(status, 0);
        assert_eq!(output, "would set shell.profile from macos to linux\n");
        assert_eq!((fake.saved, fake.applied), (None, None));
        let (status, _, errors) =
            settings(&mut fake, &["set", "--dry-run", "shell.profile", "amiga"]);
        assert_eq!(status, 2);
        assert!(errors.contains("invalid value 'amiga'"), "{errors}");
        let (status, _, _) = settings(&mut fake, &["set", "--dry-run", "system.arch", "x"]);
        assert_eq!(status, 1);
        assert_eq!(settings(&mut fake, &["list", "--dry-run"]).0, 2);
        assert_eq!(fake.saved, None);
    }

    #[test]
    fn errors_are_coded_with_their_fix_and_their_exit_status() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["get", "nope"]),
            (
                1,
                String::new(),
                "settings: nope: no such setting\n  try: settings list\n".to_owned()
            )
        );
        let (status, _, errors) = settings(&mut fake, &["get", "--json", "nope"]);
        assert_eq!(status, 1);
        assert!(
            errors
                .starts_with("{\"error\":{\"code\":\"no_such_setting\",\"category\":\"not_found\""),
            "{errors}"
        );
        let (status, _, errors) = settings(&mut fake, &["set", "system.arch", "x"]);
        assert_eq!(status, 1);
        assert!(
            errors.starts_with("settings: system.arch: read-only\n"),
            "{errors}"
        );
        let (status, _, errors) = settings(&mut fake, &["set", "shell.profile", "amiga"]);
        assert_eq!(status, 2);
        assert_eq!(
            errors,
            "settings: shell.profile: invalid value 'amiga' (windows, macos or linux)\n"
        );
        assert_eq!(fake.saved, None);
        assert_eq!(
            settings(&mut fake, &["frobnicate"]),
            (2, String::new(), USAGE.to_owned())
        );
        assert_eq!(settings(&mut fake, &["set", "--json", "a", "b"]).0, 2);
        let (status, _, errors) = settings(&mut fake, &["--json", "--bogus"]);
        assert_eq!(status, 2);
        assert!(errors.contains("\"code\":\"usage\""), "{errors}");
    }

    fn sorted_keys() -> Vec<&'static str> {
        let mut keys: Vec<&str> = Key::ALL.iter().map(|key| key.name()).collect();
        keys.sort_unstable();
        keys
    }

    fn listed_keys(output: &str) -> Vec<&str> {
        output
            .lines()
            .map(|line| line.split_once('=').unwrap().0)
            .collect()
    }

    #[test]
    fn list_pages_by_key_and_keeps_its_order_within_a_page() {
        let sorted = sorted_keys();
        let (status, output, errors) = settings(&mut Fake::default(), &["list", "--limit", "3"]);
        assert_eq!(status, 0);
        let mut first = listed_keys(&output);
        let order: Vec<usize> = first
            .iter()
            .map(|name| Key::ALL.iter().position(|key| key.name() == *name).unwrap())
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{output}");
        first.sort_unstable();
        assert_eq!(first, sorted[..3]);
        assert_eq!(errors, format!("more: --after {}\n", sorted[2]));
        let (status, output, errors) = settings(
            &mut Fake::default(),
            &["list", "--after", sorted[2], "--limit=100"],
        );
        assert_eq!(status, 0);
        let mut rest = listed_keys(&output);
        rest.sort_unstable();
        assert_eq!(rest, sorted[3..]);
        assert_eq!(errors, "");
        let last = sorted[sorted.len() - 1];
        let (status, output, errors) = settings(&mut Fake::default(), &["list", "--after", last]);
        assert_eq!((status, output.as_str(), errors.as_str()), (0, "", ""));
    }

    #[test]
    fn a_json_page_ends_with_its_page_line() {
        let first = sorted_keys()[0];
        let (status, output, errors) =
            settings(&mut Fake::default(), &["list", "--limit", "1", "--json"]);
        assert_eq!(status, 0);
        assert_eq!(output.lines().count(), 1);
        assert!(
            output.starts_with(&format!("{{\"key\":\"{first}\"")),
            "{output}"
        );
        assert_eq!(
            errors,
            format!(
                "{{\"page\":{{\"truncated\":true,\"truncated_by\":\"limit\",\
                 \"next_cursor\":\"{first}\"}}}}\n"
            )
        );
    }

    #[test]
    fn a_bad_page_is_refused_with_its_code() {
        let mut fake = Fake::default();
        let (status, _, errors) = settings(&mut fake, &["list", "--after", "-x"]);
        assert_eq!(status, 2);
        assert_eq!(
            errors,
            "settings: invalid cursor '-x': not one a page ended with\n"
        );
        let (status, _, errors) = settings(&mut fake, &["list", "--after", "a b", "--json"]);
        assert_eq!(status, 2);
        assert!(
            errors.starts_with("{\"error\":{\"code\":\"bad_cursor\""),
            "{errors}"
        );
        for limit in ["0", "101", "+5", "x", ""] {
            let (status, _, errors) = settings(&mut fake, &["list", "--limit", limit]);
            assert_eq!(status, 2, "{limit}");
            assert!(errors.starts_with("settings: invalid limit"), "{errors}");
        }
        let (status, _, errors) = settings(&mut fake, &["list", "--limit"]);
        assert_eq!(status, 2);
        assert_eq!(errors, "settings: option '--limit' requires an argument\n");
        let (status, _, errors) = settings(&mut fake, &["get", "--limit", "1", "shell.profile"]);
        assert_eq!((status, errors.as_str()), (2, USAGE));
    }

    #[test]
    fn a_twin_asked_for_coded_errors_adds_the_json_line_after_the_text() {
        let mut fake = Fake {
            coded_errors: true,
            ..Fake::default()
        };
        let (status, _, errors) = settings(&mut fake, &["get", "nope"]);
        assert_eq!(status, 1);
        let lines: Vec<&str> = errors.lines().collect();
        assert_eq!(
            lines[..2],
            ["settings: nope: no such setting", "  try: settings list"]
        );
        assert!(
            lines[2].starts_with("{\"error\":{\"code\":\"no_such_setting\""),
            "{errors}"
        );
        assert_eq!(lines.len(), 3, "{errors}");
        let (status, _, errors) = settings(&mut fake, &["frobnicate"]);
        assert_eq!(status, 2);
        assert!(errors.starts_with(USAGE), "{errors}");
        assert!(
            errors[USAGE.len()..].starts_with("{\"error\":{\"code\":\"usage\""),
            "{errors}"
        );
        let (status, _, errors) = settings(&mut fake, &["set", "shell.profile", "amiga"]);
        assert_eq!(status, 2);
        assert!(errors.contains("\"code\":\"invalid_value\""), "{errors}");
        let (status, _, errors) = settings(&mut fake, &["get", "--json", "nope"]);
        assert_eq!(status, 1);
        assert_eq!(errors.lines().count(), 1, "{errors}");
        let (status, _, errors) = settings(&mut Fake::default(), &["get", "nope"]);
        assert_eq!(status, 1);
        assert!(!errors.contains("{\"error\""), "{errors}");
    }

    #[test]
    fn a_time_zone_is_saved_by_its_iana_name() {
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &["set", "time.zone", "Europe/Paris"]), 0);
        assert_eq!(fake.zone_saved, crate::facts::zone_index("Europe/Paris"));
        let mut fake = Fake::default();
        assert_ne!(run(&mut fake, &["set", "time.zone", "Mars/Olympus"]), 0);
        assert!(
            String::from_utf8(fake.errors)
                .unwrap()
                .contains("Europe/Paris")
        );
    }

    #[test]
    fn sticky_keys_take_every_boolean_word_and_are_used_at_once() {
        let mut fake = Fake::default();
        assert_eq!(
            run(&mut fake, &["set", "accessibility.sticky_keys", "on"]),
            0
        );
        assert_eq!(fake.keys_applied.map(|keys| keys.sticky_keys), Some(true));
        let mut fake = Fake::default();
        assert_eq!(
            run(&mut fake, &["set", "accessibility.slow_keys_ms", "50"]),
            INVALID_VALUE_STATUS
        );
        assert_eq!(
            run(
                &mut Fake::default(),
                &["set", "accessibility.keyboard_navigation", "off"]
            ),
            FAILURE
        );
    }

    #[test]
    fn high_contrast_and_reduce_motion_are_saved_and_shown_at_once() {
        let mut fake = Fake::default();
        let (status, _, _) = settings(&mut fake, &["set", "accessibility.high_contrast", "on"]);
        assert_eq!(status, 0);
        assert_eq!(fake.display_saved, Some(DisplayChange::HighContrast(true)));
        assert_eq!(
            fake.display_applied,
            Some((
                Theme::Dark,
                DisplayAccessibility {
                    high_contrast: true,
                    reduce_motion: false,
                }
            ))
        );
        let mut fake = Fake::default();
        assert_eq!(
            run(&mut fake, &["set", "accessibility.reduce_motion", "yes"]),
            0
        );
        assert_eq!(
            fake.display_applied
                .map(|(_, display)| display.reduce_motion),
            Some(true)
        );
        assert_eq!(
            run(&mut fake, &["set", "accessibility.reduce_motion", "maybe"]),
            INVALID_VALUE_STATUS
        );
        let mut fake = Fake::default();
        let (_, listed, _) = settings(&mut fake, &["get", "accessibility.high_contrast"]);
        assert_eq!(listed, "false\n");
    }

    #[test]
    fn a_mouse_preference_is_saved_and_used_at_once() {
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &["set", "mouse.buttons", "left"]), 0);
        assert_eq!(fake.input_saved, Some(InputChange::LeftHanded(true)));
        assert_eq!(
            fake.input_applied,
            Some(InputPreferences {
                left_handed: true,
                ..InputPreferences::DEFAULT
            })
        );
        let mut fake = Fake::default();
        let (status, _, errors) = settings(&mut fake, &["set", "mouse.speed", "11"]);
        assert_eq!(status, INVALID_VALUE_STATUS);
        assert!(errors.contains("(1 to 10)"), "{errors}");
        assert_eq!(fake.input_applied, None);
    }

    #[test]
    fn the_layouts_and_the_switch_key_are_saved_and_applied_and_refused_with_their_reasons() {
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["set", "keyboard.layouts", "gb,se"]).0,
            0
        );
        let choice = fake.layout_applied.unwrap();
        assert_eq!(
            Some(choice.in_force),
            eggos_settings_file::keyboard_layout_named(b"gb")
        );
        assert_eq!(fake.layout_saved, Some(choice));
        for (value, line) in [
            (
                "us,de,fr,es,it",
                "settings: keyboard.layouts: invalid value 'us,de,fr,es,it' (at most 4 layouts)\n",
            ),
            (
                "us,us",
                "settings: keyboard.layouts: invalid value 'us,us' ('us' is listed twice)\n",
            ),
        ] {
            assert_eq!(
                settings(&mut Fake::default(), &["set", "keyboard.layouts", value]),
                (INVALID_VALUE_STATUS, String::new(), line.to_owned())
            );
        }
        let (status, _, errors) =
            settings(&mut Fake::default(), &["set", "keyboard.layouts", "us,xx"]);
        assert_eq!(status, INVALID_VALUE_STATUS);
        assert!(
            errors.starts_with("settings: keyboard.layouts: invalid value 'xx' (us, us-intl, "),
            "{errors}"
        );
        let mut fake = Fake::default();
        assert_eq!(
            settings(&mut fake, &["set", "keyboard.switch_key", "alt+shift"]).0,
            0
        );
        assert_eq!(
            fake.layout_applied.map(|choice| choice.switch_key),
            Some(eggos_settings_file::SwitchKey::AltShift)
        );
        assert_eq!(
            settings(&mut Fake::default(), &["set", "keyboard.switch_key", "x"]),
            (
                INVALID_VALUE_STATUS,
                String::new(),
                "settings: keyboard.switch_key: invalid value 'x' (profile, super+space, \
                 ctrl+space, alt+shift, ctrl+shift or none)\n"
                    .to_owned()
            )
        );
        let (_, output, _) = settings(&mut Fake::default(), &["get", "keyboard.switch_key"]);
        assert_eq!(output, "profile\n");
    }

    #[test]
    fn the_theme_is_read_set_and_refused_through_one_change() {
        let mut fake = Fake::default();
        let (_, output, _) = settings(&mut fake, &["get", "appearance.theme"]);
        assert_eq!(output, "dark\n");
        let (status, _, _) = settings(&mut fake, &["set", "appearance.theme", "light"]);
        assert_eq!(status, 0);
        assert_eq!(fake.theme_saved, Some(Theme::Light));
        assert_eq!(fake.theme_applied, Some(Theme::Light));
        let (status, _, errors) = settings(&mut fake, &["set", "appearance.theme", "blue"]);
        assert_eq!(status, INVALID_VALUE_STATUS);
        assert_eq!(
            errors,
            "settings: appearance.theme: invalid value 'blue' (dark or light)\n"
        );
        let (_, output, _) = settings(
            &mut fake,
            &["set", "--dry-run", "appearance.theme", "light"],
        );
        assert_eq!(output, "would set appearance.theme from dark to light\n");
    }
}
