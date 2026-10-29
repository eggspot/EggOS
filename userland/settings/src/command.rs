//! The `settings` command's verbs (D132, D163; docs/ux/command-model.md): `settings list`,
//! `settings get <key>` and `settings set <key> <value>`, defined in the command model's
//! registry (`eggos_commands::commands::SETTINGS_LIST`, `SETTINGS_GET`, `SETTINGS_SET`), with
//! `--json` for list and get, `--dry-run` for set (D164) and coded errors. The machine is a [`Machine`], so host tests run
//! the registry's examples against example facts.

use crate::facts::Facts;
use crate::keys::{self, Change, Key, PROFILE_CHOICES, Refusal};
use core::fmt::Write;
use eggos_commands::{ErrorDefinition, errors};
use eggos_mcp::json::JsonWriter;
use eggos_mcp::server::write_error;
use eggos_settings_file::{PATH, SHELL_PROFILE};
use eggos_toolkit::text::Text;

/// The command's usage, printed for a usage error.
pub const USAGE: &str =
    "usage: settings [list [--json] | get [--json] <key> | set [--dry-run] <key> <value>]\n";
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
    /// Writes to standard output.
    fn write_output(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn write_error(&mut self, bytes: &[u8]);
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
    let (key, index) = match keys::change(name, value) {
        Ok((key, Change::Profile(index))) => (key, index),
        Err(Refusal::NoSuchSetting) => return no_such_setting(machine, false, name),
        Err(Refusal::ReadOnly) => {
            let mut message = Message::new();
            let _ = write!(message, "{name}: read-only");
            report(machine, false, &errors::SETTING_READ_ONLY, message.as_str());
            return FAILURE;
        }
        Err(Refusal::InvalidValue) => {
            let mut message = Message::new();
            let _ = write!(
                message,
                "{name}: invalid value '{value}' ({PROFILE_CHOICES})"
            );
            report(machine, false, &errors::INVALID_VALUE, message.as_str());
            return INVALID_VALUE_STATUS;
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
    let saved = machine.save_profile(index);
    if let Err(why) = saved {
        let mut message = Message::new();
        let _ = write!(message, "{PATH}: {}", text_of(why));
        report(machine, false, &errors::FAILED, message.as_str());
    }
    let applied = machine.apply_profile(index);
    if let Err(why) = applied {
        let mut message = Message::new();
        let _ = write!(message, "{SHELL_PROFILE}: not applied: {}", text_of(why));
        report(machine, false, &errors::NOT_APPLIED, message.as_str());
    }
    match saved.is_ok() && applied.is_ok() {
        true => 0,
        false => FAILURE,
    }
}

fn text_of(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_commands::COMMANDS;

    #[derive(Debug, Default)]
    struct Fake {
        output: Vec<u8>,
        errors: Vec<u8>,
        saved: Option<usize>,
        applied: Option<usize>,
        save_fails: bool,
        apply_fails: bool,
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
}
