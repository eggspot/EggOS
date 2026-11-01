//! The `settings` command's verbs (D132, D163; docs/ux/command-model.md): `settings list`,
//! `settings get <key>` and `settings set <key> <value>`, defined in the command model's
//! registry (`eggos_commands::commands::SETTINGS_LIST`, `SETTINGS_GET`, `SETTINGS_SET`), with
//! `--json` for list and get, `--dry-run` for set (D164) and coded errors. The machine is a [`Machine`], so host tests run
//! the registry's examples against example facts.

use crate::facts::Facts;
use crate::keys::{self, Change, InputChange, Key, KeyboardChoice, KeysChange, Refusal};
use core::fmt::Write;
use eggos_commands::{ErrorDefinition, errors};
use eggos_mcp::json::JsonWriter;
use eggos_mcp::server::write_error;
use eggos_settings_file::{InputPreferences, KeyAccessibility, PATH};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Theme;

/// The command's usage, printed for a usage error.
pub const USAGE: &str = "usage: settings [--pair <id> [--name <name>] | list [--json] | get \
                         [--json] <key> | set [--dry-run] <key> <value>]\n";
/// The exit status of a usage error.
pub const USAGE_STATUS: i32 = 2;
/// The exit status of a value the setting does not take.
pub const INVALID_VALUE_STATUS: i32 = 2;
/// The exit status of every other error.
pub const FAILURE: i32 = 1;
/// The most words the command takes after `settings`.
pub const MAX_ARGUMENTS: usize = 6;

const MAX_WORDS: usize = 3;
const RECORD_BYTES: usize = 512;
const ERROR_BYTES: usize = 1024;

type Message = Text<256>;

/// What the command needs from the machine.
pub trait Machine {
    /// The facts the settings report.
    fn facts(&mut self) -> Facts;
    /// Saves the desktop profile `index` to the settings file; the error's text on failure.
    fn save_profile(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Applies the desktop profile `index` now; the error's text on failure.
    fn apply_profile(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Saves the desktop's theme to the settings file as `appearance.theme` (D218).
    fn save_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]>;
    /// Makes `theme` the desktop's now (`SET_PALETTE`): every window repaints in it.
    fn apply_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]>;
    /// Saves what the keyboard uses (`keyboard.layouts`, `keyboard.layout` and
    /// `keyboard.switch_key`) to the settings file; the error's text on failure.
    fn save_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]>;
    /// Has the keyboard use `choice` now, with or without a desktop (D220); the error's text
    /// on failure.
    fn apply_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]>;
    /// Saves the time zone `index` (of `eggos_calendar::zone::ZONES`) to the settings file;
    /// programs read it each time they show a time.
    fn save_time_zone(&mut self, index: usize) -> Result<(), &'static [u8]>;
    /// Saves the display's scale to the settings file; the kernel shows at it from its next
    /// boot.
    fn save_display_scale(&mut self, scale: u32) -> Result<(), &'static [u8]>;
    /// Saves one mouse or key preference to the settings file; the kernel uses it from its
    /// next boot.
    fn save_input(&mut self, change: InputChange) -> Result<(), &'static [u8]>;
    /// Uses `preferences` from the next mouse and key event on.
    fn apply_input(&mut self, preferences: InputPreferences) -> Result<(), &'static [u8]>;
    /// Saves one key accessibility setting to the settings file (D205).
    fn save_keys(&mut self, change: KeysChange) -> Result<(), &'static [u8]>;
    /// Uses `keys` from the next key on.
    fn apply_keys(&mut self, keys: KeyAccessibility) -> Result<(), &'static [u8]>;
    /// Writes to standard output.
    fn write_output(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn write_error(&mut self, bytes: &[u8]);
}

/// The option that opens the window at the Agents pane's pairing panel, ready to pair an
/// agent (D193): `settings --pair <id> [--name <name>]`, what `agent pair` runs from a terminal.
pub const PAIR_OPTION: &str = "--pair";
/// With [`PAIR_OPTION`], the name the user sees.
pub const NAME_OPTION: &str = "--name";

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
    /// `--pair` or `--name` without what they need.
    Usage,
}

/// Reads `settings --pair <id> [--name <name>]` (the options in either order) from
/// `arguments`, the words after the program's name.
pub fn window_asked<'a>(arguments: &[&'a str]) -> WindowAsked<'a> {
    let asked = |id, name| WindowAsked::Pairing(PairingAsked { id, name });
    match *arguments {
        [PAIR_OPTION, id] => asked(id, ""),
        [PAIR_OPTION, id, NAME_OPTION, name] | [NAME_OPTION, name, PAIR_OPTION, id] => {
            asked(id, name)
        }
        [first, ..] if first == PAIR_OPTION || first == NAME_OPTION => WindowAsked::Usage,
        _ => WindowAsked::Other,
    }
}

/// Runs `settings` with `arguments`, the words after its name (at least one: with none the
/// program opens its window); the exit status.
pub fn run(machine: &mut impl Machine, arguments: &[&str]) -> i32 {
    let mut json = false;
    let mut dry_run = false;
    let mut words = [""; MAX_WORDS];
    let mut count = 0;
    let mut options_end = false;
    for &argument in arguments {
        if !options_end && argument == "--" {
            options_end = true;
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
        ["list"] if !dry_run => list(machine, json),
        ["get", name] if !dry_run => get(machine, json, name),
        ["set", name, value] if !json => set(machine, name, value, dry_run),
        _ => usage(machine, json),
    }
}

fn usage(machine: &mut impl Machine, json: bool) -> i32 {
    match json {
        true => report(machine, true, &errors::USAGE, USAGE.trim_end()),
        false => machine.write_error(USAGE.as_bytes()),
    }
    USAGE_STATUS
}

fn report(machine: &mut impl Machine, json: bool, error: &ErrorDefinition, message: &str) {
    if json {
        let mut buffer = [0u8; ERROR_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        write_error(&mut out, error, message);
        out.end_line();
        if !out.overflowed() {
            machine.write_error(out.as_bytes());
        }
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

fn list(machine: &mut impl Machine, json: bool) -> i32 {
    let facts = machine.facts();
    for key in Key::ALL {
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
        Change::Theme(theme) => machine.save_theme(theme),
        Change::KeyboardLayout(_) | Change::KeyboardLayouts(_) | Change::KeyboardSwitchKey(_) => {
            keyboard.map_or(Ok(()), |choice| machine.save_keyboard(choice))
        }
        Change::TimeZone(index) => machine.save_time_zone(index),
        Change::DisplayScale(scale) => machine.save_display_scale(scale),
        Change::Input(input) => machine.save_input(input),
        Change::Keys(keys) => machine.save_keys(keys),
    };
    if let Err(why) = saved {
        let mut message = Message::new();
        let _ = write!(message, "{PATH}: {}", text_of(why));
        report(machine, false, &errors::FAILED, message.as_str());
    }
    let applied = match change {
        Change::Profile(index) => machine.apply_profile(index),
        Change::Theme(theme) => machine.apply_theme(theme),
        Change::KeyboardLayout(_) | Change::KeyboardLayouts(_) | Change::KeyboardSwitchKey(_) => {
            keyboard.map_or(Ok(()), |choice| machine.apply_keyboard(choice))
        }
        Change::TimeZone(_) | Change::DisplayScale(_) => Ok(()),
        Change::Input(input) => {
            let preferences = input.applied_to(machine.facts().input);
            machine.apply_input(preferences)
        }
        Change::Keys(keys) => {
            let applied = keys.applied_to(machine.facts().keys);
            machine.apply_keys(applied)
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
        scale_saved: Option<u32>,
        input_saved: Option<InputChange>,
        input_applied: Option<InputPreferences>,
        keys_applied: Option<KeyAccessibility>,
        save_fails: bool,
        apply_fails: bool,
        theme_saved: Option<Theme>,
        theme_applied: Option<Theme>,
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

        fn save_display_scale(&mut self, scale: u32) -> Result<(), &'static [u8]> {
            self.scale_saved = Some(scale);
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
        assert_eq!(settings(&mut fake, &["set", "keyboard.layouts", "gb,se"]).0, 0);
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
