//! The `app` command (D134, docs/apps.md): `app install [--name <name>] [--entry <path>]
//! [--arguments <words>] [--environment <variables>] [--window] <path>` (a program, an `.app`
//! or a `.zip`; `--entry` chooses the program inside a `.zip`, D148; `--arguments` gives the
//! words it opens with, D172; `--environment` the `NAME=value` words set for it, D176),
//! `app uninstall [--window] <name>` (`remove` is the same), `app open <name>` (D137; the
//! built-in Terminal, Files, Settings and Text Editor too, D141, D149) and `app list` (name,
//! source, architecture, program and icon file, D148; `--json` writes them as JSON lines, D160;
//! `--limit <count>` and `--after <cursor>` page them by name, D164). `app uninstall --dry-run`
//! says what it would remove (D164). `app show [--json] <name>` gives one app's fields and, for
//! a program translated at install, its translation: sizes, refused count and the first
//! refused sites (D191); `app install` translates such a program right after copying it.
//! `app icon [--size 48|32] [-o <file.png>] <name> [<file.png>]` writes the app's own icon as a
//! PNG (D196).
//! Text, the same with or without a display; `--window` shows the installer's dialogs instead.
//! `--` ends the options, so the next word is the operand even if it starts with `-`.

use crate::messages::Line;
use core::fmt::Write;
use eggos_app_manifest::translation::{self, Record};
use eggos_mcp::json::JsonWriter;

/// What the command was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    Install {
        path: &'a [u8],
        name: Option<&'a [u8]>,
        entry: Option<&'a [u8]>,
        arguments: Option<&'a [u8]>,
        environment: Option<&'a [u8]>,
        window: bool,
    },
    Uninstall {
        name: &'a [u8],
        window: bool,
        dry_run: bool,
    },
    Open {
        name: &'a [u8],
    },
    List {
        json: bool,
        limit: Option<&'a [u8]>,
        after: Option<&'a [u8]>,
    },
    Show {
        name: &'a [u8],
        json: bool,
    },
    Icon {
        name: &'a [u8],
        output: &'a [u8],
        size: Option<&'a [u8]>,
    },
    Help,
}

/// The command's usage, printed for `--help` and with a usage error.
pub const USAGE: &str = "usage: app install [--name <name>] [--entry <path>] \
                         [--arguments <words>] [--environment <variables>] [--window] \
                         [--] <path>\n       \
                         app uninstall [--window] [--dry-run] [--] <name>\n       \
                         app open [--] <name>\n       \
                         app list [--json] [--limit <count>] [--after <cursor>]\n       \
                         app show [--json] [--] <name>\n       \
                         app icon [--size 48|32] [-o <file.png>] [--] <name> [<file.png>]\n";

/// The built-in apps `app open` opens by name, which no installed app of the same name stands
/// in for.
pub const BUILT_IN_APPS: [&[u8]; 4] = [b"Terminal", b"Files", b"Settings", b"Text Editor"];

/// Whether `name` is a built-in app's ([`BUILT_IN_APPS`]), which is never in `/Applications`.
pub fn is_built_in(name: &[u8]) -> bool {
    BUILT_IN_APPS.contains(&name)
}

/// Exit status of a refused or failed command.
pub const FAILED: i32 = 1;
/// Exit status of a usage error.
pub const USAGE_ERROR: i32 = 2;

/// The command `arguments` (after the program's name) ask for; `None` for a usage error.
pub fn parse<'a>(arguments: &[&'a [u8]]) -> Option<Command<'a>> {
    let (&verb, rest) = arguments.split_first()?;
    let mut window = false;
    let mut name = None;
    let mut entry = None;
    let mut words = None;
    let mut environment = None;
    let mut json = false;
    let mut dry_run = false;
    let mut limit = None;
    let mut after = None;
    let mut operand = None;
    let mut second = None;
    let mut size = None;
    let mut output = None;
    let icon = verb == b"icon";
    let mut rest = rest.iter();
    let mut options_end = false;
    let removing = verb == b"uninstall" || verb == b"remove";
    while let Some(&argument) = rest.next() {
        match argument {
            _ if options_end && operand.is_none() => operand = Some(argument),
            _ if options_end && icon && second.is_none() => second = Some(argument),
            _ if options_end => return None,
            b"--" => options_end = true,
            b"--window" => window = true,
            b"--dry-run" if removing => dry_run = true,
            b"--json" if verb == b"list" || verb == b"show" => json = true,
            b"--limit" if verb == b"list" => limit = Some(*rest.next()?),
            b"--after" if verb == b"list" => after = Some(*rest.next()?),
            _ if verb == b"list" && argument.starts_with(b"--limit=") => {
                limit = argument.get(b"--limit=".len()..);
            }
            _ if verb == b"list" && argument.starts_with(b"--after=") => {
                after = argument.get(b"--after=".len()..);
            }
            b"--name" if verb == b"install" => name = Some(*rest.next()?),
            b"--entry" if verb == b"install" => entry = Some(*rest.next()?),
            b"--arguments" if verb == b"install" => words = Some(*rest.next()?),
            b"--environment" if verb == b"install" => environment = Some(*rest.next()?),
            b"--size" if icon => size = Some(*rest.next()?),
            b"-o" if icon => output = Some(*rest.next()?),
            _ if argument.starts_with(b"-") && argument.len() > 1 => return None,
            _ if operand.is_none() => operand = Some(argument),
            _ if icon && second.is_none() => second = Some(argument),
            _ => return None,
        }
    }
    match verb {
        b"install" => Some(Command::Install {
            path: operand?,
            name,
            entry,
            arguments: words,
            environment,
            window,
        }),
        b"uninstall" | b"remove" if !(window && dry_run) => Some(Command::Uninstall {
            name: operand?,
            window,
            dry_run,
        }),
        b"open" if !window => Some(Command::Open { name: operand? }),
        b"list" if operand.is_none() && !window => Some(Command::List { json, limit, after }),
        b"show" if !window => Some(Command::Show {
            name: operand?,
            json,
        }),
        b"icon" if !window => Some(Command::Icon {
            name: operand?,
            output: output.xor(second)?,
            size,
        }),
        b"--help" | b"help" if operand.is_none() => Some(Command::Help),
        _ => None,
    }
}

/// The most bytes one app's JSON record takes: its fields are at most a path long each.
pub const APP_RECORD_BYTES: usize = 6 * 4096 + 256;

/// Writes one app as `app list --json`'s record (D160): `name`, `source`, `architecture`
/// (`null` for none), `program` and `icon` (`null` for none), then a newline.
pub fn write_app_record(
    out: &mut JsonWriter<'_>,
    name: &str,
    source: &str,
    architecture: Option<&str>,
    program: &[u8],
    icon: Option<&str>,
) {
    out.begin_object();
    write_app_fields(out, name, source, architecture, program, icon);
    out.end_object();
    out.end_line();
}

/// Writes one app as `app show --json`'s record (D191): `app list --json`'s fields, then
/// `translation`, `null` when the app keeps no translation record, else an object with `key`
/// (64 hex digits), `code_bytes`, `cache_bytes`, `stored`, `refused` and `refused_sites` (the
/// first ones, each `address` in `0x` hex and `reason`), then a newline.
pub fn write_app_show_record(
    out: &mut JsonWriter<'_>,
    name: &str,
    source: &str,
    architecture: Option<&str>,
    program: &[u8],
    icon: Option<&str>,
    translation: Option<&Record<'_>>,
) {
    out.begin_object();
    write_app_fields(out, name, source, architecture, program, icon);
    out.key("translation");
    match translation {
        Some(record) => write_translation(out, record),
        None => out.null(),
    }
    out.end_object();
    out.end_line();
}

fn write_app_fields(
    out: &mut JsonWriter<'_>,
    name: &str,
    source: &str,
    architecture: Option<&str>,
    program: &[u8],
    icon: Option<&str>,
) {
    out.key("name");
    out.string(name);
    out.key("source");
    out.string(source);
    out.key("architecture");
    match architecture {
        Some(architecture) => out.string(architecture),
        None => out.null(),
    }
    out.key("program");
    out.string_lossy(program);
    out.key("icon");
    match icon {
        Some(icon) => out.string(icon),
        None => out.null(),
    }
}

fn write_translation(out: &mut JsonWriter<'_>, record: &Record<'_>) {
    let path = record.cache_path();
    let digits = translation::CACHE_DIRECTORY.len() + 1;
    out.begin_object();
    out.key("key");
    out.string_lossy(&path[digits..digits + 64]);
    out.key("code_bytes");
    out.u64(record.code_bytes);
    out.key("cache_bytes");
    out.u64(record.cache_bytes);
    out.key("stored");
    out.bool(record.stored);
    out.key("refused");
    out.u64(record.refused);
    out.key("refused_sites");
    out.begin_array();
    for site in record.sites() {
        let mut address = Line::new();
        let _ = write!(address, "{:#x}", site.address);
        out.begin_object();
        out.key("address");
        out.string(address.as_str());
        out.key("reason");
        out.string_lossy(site.reason);
        out.end_object();
    }
    out.end_array();
    out.end_object();
}

/// `app show`'s text lines after the app's fields: the translation's sizes and cache file,
/// the refused count, then one line per listed refused site.
pub fn translation_lines(record: &Record<'_>, mut each: impl FnMut(&str)) {
    let mut line = Line::new();
    let _ = write!(
        line,
        "translation:\t{} bytes of code, a {}-byte cache in ",
        record.code_bytes, record.cache_bytes
    );
    line.push_bytes(&record.cache_path());
    if !record.stored {
        let _ = line.write_str(" (not stored)");
    }
    each(line.as_str());
    let mut line = Line::new();
    let _ = write!(line, "refused:\t{}", record.refused);
    if record.refused > record.sites().len() as u64 {
        let _ = write!(line, " (the first {} below)", record.sites().len());
    }
    each(line.as_str());
    for site in record.sites() {
        let mut line = Line::new();
        let _ = write!(line, "\t{:#x}\t", site.address);
        line.push_bytes(site.reason);
        each(line.as_str());
    }
}

#[cfg(target_env = "musl")]
mod program {
    use super::*;
    use crate::architecture::Architecture;
    use crate::files::Path;
    use crate::icon_export::{self, ExportError, IconSize};
    use crate::installer::{self, APPLICATIONS};
    use crate::linux::{self, LinuxFiles, STANDARD_ERROR, STANDARD_OUTPUT, write_all};
    use crate::messages::{self, Line};
    use core::ffi::CStr;
    use core::fmt::Write;
    use eggos_commands::page::{self, MAX_CURSOR_BYTES, MAX_KEY_BYTES, MORE_PREFIX};
    use eggos_commands::{ErrorDefinition, errors};
    use eggos_png::EncoderWorkspace;
    use eggos_runtime::startup::Startup;

    const COPY_BUFFER: usize = 64 * 1024;
    const MAX_ARGUMENTS: usize = 12;

    static mut BUFFER: [u8; COPY_BUFFER] = [0; COPY_BUFFER];

    fn say(descriptor: usize, line: &str) {
        write_all(descriptor, line.as_bytes());
        write_all(descriptor, b"\n");
    }

    fn complain(line: &str) {
        let mut said = Line::new();
        let _ = said.write_str("app: ");
        match line.strip_prefix("Could not") {
            Some(rest) => {
                let _ = write!(said, "could not{rest}");
            }
            None => {
                let _ = said.write_str(line);
            }
        }
        say(STANDARD_ERROR, said.as_str());
    }

    fn buffer() -> &'static mut [u8] {
        // SAFETY: the program has one thread and calls this once, from `install`, so no other
        // reference to the buffer exists while this one lives.
        unsafe { core::slice::from_raw_parts_mut((&raw mut BUFFER).cast::<u8>(), COPY_BUFFER) }
    }

    #[allow(clippy::result_large_err)]
    fn install(
        path: &[u8],
        name: Option<&[u8]>,
        entry: Option<&[u8]>,
        arguments: Option<&[u8]>,
        environment: Option<&[u8]>,
    ) -> i32 {
        let mut files = LinuxFiles::new();
        let machine = linux::machine();
        let mut subject = Line::new();
        subject.push_bytes(crate::files::file_name(path));
        let planned =
            installer::plan_entry(&mut files, path, name, entry, machine).and_then(|plan| {
                match arguments {
                    Some(arguments) => plan.with_arguments(arguments),
                    None => Ok(plan),
                }
                .and_then(|plan| match environment {
                    Some(environment) => plan.with_environment(environment),
                    None => Ok(plan),
                })
            });
        let plan = match planned {
            Ok(plan) => plan,
            Err(refused) => {
                let line = messages::install_failed(
                    refused.name.as_str(),
                    subject.as_str(),
                    machine,
                    refused.problem,
                );
                complain(line.as_str());
                return FAILED;
            }
        };
        match installer::install(&mut files, plan, buffer()) {
            Ok(()) => {
                linux::apps_changed();
                let mut line = Line::new();
                let _ = write!(line, "Installed {} in ", plan.name());
                line.push_bytes(plan.folder().as_bytes());
                let _ = line.write_str(".");
                say(STANDARD_OUTPUT, line.as_str());
                if let Some(ahead) = crate::translation::ahead_of_launch(&plan, machine) {
                    translate(&mut files, &plan, machine, ahead);
                }
                0
            }
            Err(problem) => {
                let line =
                    messages::install_failed(plan.name(), subject.as_str(), machine, problem);
                complain(line.as_str());
                FAILED
            }
        }
    }

    fn translate(
        files: &mut LinuxFiles,
        plan: &installer::Plan,
        machine: Architecture,
        ahead: crate::translation::AheadOfLaunch,
    ) {
        let built_for = plan.program().architecture;
        say(
            STANDARD_OUTPUT,
            messages::translating(plan.name(), built_for, machine).as_str(),
        );
        let mut printed = [0u8; translation::LARGEST];
        let ran = linux::run_translation(ahead.layer, ahead.program.as_bytes(), &mut printed);
        let printed = match ran {
            Ok((length, true)) => &printed[..length],
            _ => {
                say(STANDARD_OUTPUT, messages::not_translated().as_str());
                return;
            }
        };
        let Some(record) = translation::parse(printed) else {
            say(STANDARD_OUTPUT, messages::not_translated().as_str());
            return;
        };
        say(
            STANDARD_OUTPUT,
            messages::translated(plan.name(), &record).as_str(),
        );
        if crate::translation::keep_record(files, plan.name().as_bytes(), printed).is_err() {
            complain("could not keep the translation record: app show will not list it");
        }
    }

    fn show(name: &[u8], json: bool) -> i32 {
        let mut files = LinuxFiles::new();
        let shown = installer::Name::new(name);
        let mut file = [0u8; eggos_app_manifest::LARGEST + 1];
        let manifest = installer::read_manifest(&mut files, name, &mut file)
            .and_then(|length| eggos_app_manifest::parse(&file[..length], name));
        let Some(app) = manifest else {
            let mut line = Line::new();
            let _ = write!(line, "{} is not installed", shown.as_str());
            report(json, &errors::NO_SUCH_APP, line.as_str());
            return FAILED;
        };
        let mut kept: crate::translation::RecordBuffer = [0; translation::LARGEST + 1];
        let record = crate::translation::read_record(&mut files, name, &mut kept)
            .and_then(|length| translation::parse(&kept[..length]));
        let mut program = Path::new();
        let resolved = match app.run_is_relative() {
            true => Path::from(APPLICATIONS)
                .and_then(|folder| folder.join(app.name.as_bytes()))
                .and_then(|folder| folder.join(app.run.as_bytes())),
            false => Path::from(app.run.as_bytes()),
        };
        if let Some(resolved) = resolved {
            program = resolved;
        }
        if json {
            let mut out_bytes = [0u8; APP_RECORD_BYTES];
            let mut out = JsonWriter::new(&mut out_bytes);
            write_app_show_record(
                &mut out,
                app.name,
                app.source.key(),
                app.architecture,
                program.as_bytes(),
                app.icon,
                record.as_ref(),
            );
            if out.overflowed() {
                report(json, &errors::FAILED, "the app's record is too long");
                return FAILED;
            }
            write_all(STANDARD_OUTPUT, out.as_bytes());
            return 0;
        }
        let mut line = Line::new();
        let _ = write!(line, "name:\t{}", app.name);
        say(STANDARD_OUTPUT, line.as_str());
        let mut line = Line::new();
        let _ = write!(line, "source:\t{}", app.source.key());
        say(STANDARD_OUTPUT, line.as_str());
        let mut line = Line::new();
        let _ = write!(line, "architecture:\t{}", app.architecture.unwrap_or("-"));
        say(STANDARD_OUTPUT, line.as_str());
        let mut line = Line::new();
        let _ = line.write_str("program:\t");
        line.push_bytes(program.as_bytes());
        say(STANDARD_OUTPUT, line.as_str());
        let mut line = Line::new();
        let _ = write!(line, "icon:\t{}", app.icon.unwrap_or("-"));
        say(STANDARD_OUTPUT, line.as_str());
        match record {
            Some(record) => translation_lines(&record, |line| say(STANDARD_OUTPUT, line)),
            None => say(STANDARD_OUTPUT, "translation:\t-"),
        }
        0
    }

    static mut ICON_WORK: EncoderWorkspace = EncoderWorkspace::new();

    fn icon(name: &[u8], output: &[u8], size: Option<&[u8]>) -> i32 {
        let size = match size.map(IconSize::named) {
            None => IconSize::Large,
            Some(Some(size)) => size,
            Some(None) => {
                complain("invalid size: 48 or 32");
                return USAGE_ERROR;
            }
        };
        let shown = installer::Name::new(name);
        // SAFETY: the program has one thread and calls this once, so no other reference to the
        // workspace exists while this one lives.
        let Some(work) = (unsafe { (&raw mut ICON_WORK).as_mut() }) else {
            return 1;
        };
        let mut line = Line::new();
        match icon_export::export(&mut LinuxFiles::new(), name, size, output, work) {
            Ok(()) => {
                let _ = line.write_str("Wrote ");
                line.push_bytes(output);
                let _ = line.write_str(".");
                say(STANDARD_OUTPUT, line.as_str());
                0
            }
            Err(ExportError::NotInstalled) => {
                let _ = write!(line, "{} is not installed", shown.as_str());
                report(false, &errors::NO_SUCH_APP, line.as_str());
                FAILED
            }
            Err(ExportError::NoIcon) => {
                let _ = write!(line, "{} has no icon", shown.as_str());
                complain(line.as_str());
                FAILED
            }
            Err(ExportError::Write(error)) => {
                let _ = line.write_str("could not write ");
                line.push_bytes(output);
                let _ = line.write_str(": ");
                line.push_bytes(error.message());
                complain(line.as_str());
                FAILED
            }
        }
    }

    fn uninstall(name: &[u8]) -> i32 {
        let mut files = LinuxFiles::new();
        let shown = installer::Name::new(name);
        match installer::uninstall(&mut files, name) {
            Ok(()) => {
                linux::apps_changed();
                let mut line = Line::new();
                let _ = write!(line, "Uninstalled {}.", shown.as_str());
                say(STANDARD_OUTPUT, line.as_str());
                0
            }
            Err(problem) => {
                complain(messages::uninstall_failed(shown.as_str(), problem).as_str());
                FAILED
            }
        }
    }

    fn report(json: bool, error: &ErrorDefinition, line: &str) {
        if json {
            let mut record = [0u8; 1024];
            let mut out = JsonWriter::new(&mut record);
            eggos_mcp::server::write_error(&mut out, error, line);
            out.end_line();
            write_all(STANDARD_ERROR, out.as_bytes());
            return;
        }
        complain(line);
        if let Some(command) = error.fix_command {
            write_all(STANDARD_ERROR, b"  try: ");
            say(STANDARD_ERROR, command);
        }
    }

    fn is_installed(files: &mut LinuxFiles, name: &[u8]) -> bool {
        let mut installed = false;
        installer::for_each_installed(files, |app| {
            installed |= app.name.as_bytes() == name;
        })
        .is_ok()
            && installed
    }

    fn preview_uninstall(name: &[u8]) -> i32 {
        let mut files = LinuxFiles::new();
        let shown = installer::Name::new(name);
        let mut line = Line::new();
        if !is_installed(&mut files, name) {
            let _ = write!(line, "{} is not installed", shown.as_str());
            report(false, &errors::NO_SUCH_APP, line.as_str());
            return FAILED;
        }
        let _ = write!(line, "would uninstall {}: removes ", shown.as_str());
        line.push_bytes(APPLICATIONS);
        let _ = write!(line, "/{} and everything in it", shown.as_str());
        let mut kept: crate::translation::RecordBuffer = [0; translation::LARGEST + 1];
        let record = crate::translation::read_record(&mut files, name, &mut kept)
            .and_then(|length| translation::parse(&kept[..length]));
        if let Some(record) = record {
            let _ = line.write_str(", and its translated code ");
            line.push_bytes(&record.cache_path());
        }
        say(STANDARD_OUTPUT, line.as_str());
        0
    }

    fn report_more(json: bool, name: &[u8]) {
        let mut cursor = [0u8; MAX_CURSOR_BYTES];
        let Some(length) = page::encode(name, &mut cursor) else {
            return;
        };
        let cursor = core::str::from_utf8(&cursor[..length]).unwrap_or("");
        if json {
            let mut record = [0u8; MAX_CURSOR_BYTES + 128];
            let mut out = JsonWriter::new(&mut record);
            out.begin_object();
            out.key("page");
            eggos_mcp::server::write_page(&mut out, Some(cursor));
            out.end_object();
            out.end_line();
            write_all(STANDARD_ERROR, out.as_bytes());
            return;
        }
        write_all(STANDARD_ERROR, MORE_PREFIX.as_bytes());
        say(STANDARD_ERROR, cursor);
    }

    fn open(name: &[u8]) -> i32 {
        let mut files = LinuxFiles::new();
        let shown = installer::Name::new(name);
        let mut installed = false;
        let found = is_built_in(name)
            || (installer::for_each_installed(&mut files, |app| {
                installed |= app.name.as_bytes() == name;
            })
            .is_ok()
                && installed);
        if !found {
            let mut line = Line::new();
            let _ = write!(line, "{} is not installed", shown.as_str());
            complain(line.as_str());
            return FAILED;
        }
        match linux::open_app(name) {
            Ok(()) => {
                let mut line = Line::new();
                let _ = write!(line, "Opened {}.", shown.as_str());
                say(STANDARD_OUTPUT, line.as_str());
                0
            }
            Err(error) => {
                let mut line = Line::new();
                let _ = write!(line, "could not open {}: ", shown.as_str());
                line.push_bytes(error.message());
                complain(line.as_str());
                FAILED
            }
        }
    }

    fn page_limit(text: &[u8]) -> Option<usize> {
        let text = core::str::from_utf8(text).ok()?;
        let limit: usize = text.parse().ok()?;
        (1..=installer::PAGE_LIMIT)
            .contains(&limit)
            .then_some(limit)
    }

    fn list(json: bool, limit: Option<&[u8]>, after: Option<&[u8]>) -> i32 {
        let limit = match limit.map(page_limit) {
            None => None,
            Some(Some(limit)) => Some(limit),
            Some(None) => {
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "invalid limit: a page holds 1 to {} apps",
                    installer::PAGE_LIMIT
                );
                report(json, &errors::USAGE, line.as_str());
                return USAGE_ERROR;
            }
        };
        let mut key = [0u8; MAX_KEY_BYTES];
        let after = match after.map(|cursor| page::decode(cursor, &mut key)) {
            None => None,
            Some(Some(length)) => Some(&key[..length]),
            Some(None) => {
                report(
                    json,
                    &errors::BAD_CURSOR,
                    "invalid cursor: not one a page ended with",
                );
                return USAGE_ERROR;
            }
        };
        let mut files = LinuxFiles::new();
        let listed = installer::for_each_installed_page(&mut files, limit, after, |app| {
            let mut program = Path::new();
            let resolved = match app.run_is_relative() {
                true => Path::from(APPLICATIONS)
                    .and_then(|folder| folder.join(app.name.as_bytes()))
                    .and_then(|folder| folder.join(app.run.as_bytes())),
                false => Path::from(app.run.as_bytes()),
            };
            if let Some(resolved) = resolved {
                program = resolved;
            }
            if json {
                let mut record = [0u8; APP_RECORD_BYTES];
                let mut out = JsonWriter::new(&mut record);
                write_app_record(
                    &mut out,
                    app.name,
                    app.source.key(),
                    app.architecture,
                    program.as_bytes(),
                    app.icon,
                );
                if !out.overflowed() {
                    write_all(STANDARD_OUTPUT, out.as_bytes());
                }
                return;
            }
            let mut line = Line::new();
            let _ = write!(
                line,
                "{}\t{}\t{}\t",
                app.name,
                app.source.key(),
                app.architecture.unwrap_or("-")
            );
            line.push_bytes(program.as_bytes());
            let _ = write!(line, "\t{}", app.icon.unwrap_or("-"));
            say(STANDARD_OUTPUT, line.as_str());
        });
        match listed {
            Ok(next) => {
                if let Some(next) = next {
                    report_more(json, next.as_bytes());
                }
                0
            }
            Err(error) => {
                let mut line = Line::new();
                let _ = line.write_str("cannot list /Applications: ");
                line.push_bytes(error.message());
                report(json, &errors::FAILED, line.as_str());
                FAILED
            }
        }
    }

    fn open_window(startup: &Startup, arguments: &[&CStr]) -> i32 {
        let mut kept: [&CStr; MAX_ARGUMENTS] = [c""; MAX_ARGUMENTS];
        let mut count = 0;
        for argument in arguments {
            if argument.to_bytes() == b"--window" {
                continue;
            }
            if count == MAX_ARGUMENTS {
                write_all(STANDARD_ERROR, USAGE.as_bytes());
                return USAGE_ERROR;
            }
            kept[count] = argument;
            count += 1;
        }
        let error = linux::start_window(startup, &kept[..count]);
        let mut line = Line::new();
        let _ = line.write_str("cannot open the installer's window: ");
        line.push_bytes(error.message());
        complain(line.as_str());
        FAILED
    }

    /// The program: see the module documentation.
    pub fn run(startup: &Startup) -> i32 {
        let mut arguments: [&CStr; MAX_ARGUMENTS] = [c""; MAX_ARGUMENTS];
        let mut bytes: [&[u8]; MAX_ARGUMENTS] = [b""; MAX_ARGUMENTS];
        let mut count = 0;
        for argument in startup.arguments().skip(1) {
            if count == MAX_ARGUMENTS {
                write_all(STANDARD_ERROR, USAGE.as_bytes());
                return USAGE_ERROR;
            }
            arguments[count] = argument;
            bytes[count] = argument.to_bytes();
            count += 1;
        }
        match parse(&bytes[..count]) {
            Some(Command::Help) => {
                write_all(STANDARD_OUTPUT, USAGE.as_bytes());
                0
            }
            Some(
                Command::Install { window: true, .. } | Command::Uninstall { window: true, .. },
            ) => open_window(startup, &arguments[..count]),
            Some(Command::Install {
                path,
                name,
                entry,
                arguments,
                environment,
                ..
            }) => install(path, name, entry, arguments, environment),
            Some(Command::Uninstall {
                name,
                dry_run: true,
                ..
            }) => preview_uninstall(name),
            Some(Command::Uninstall { name, .. }) => uninstall(name),
            Some(Command::Open { name }) => open(name),
            Some(Command::List { json, limit, after }) => list(json, limit, after),
            Some(Command::Show { name, json }) => show(name, json),
            Some(Command::Icon { name, output, size }) => icon(name, output, size),
            None => {
                write_all(STANDARD_ERROR, USAGE.as_bytes());
                USAGE_ERROR
            }
        }
    }
}

#[cfg(target_env = "musl")]
pub use program::run;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_takes_its_file_as_a_second_operand_or_after_o() {
        assert_eq!(
            parse(&[b"icon", b"Paint", b"paint.png"]),
            Some(Command::Icon {
                name: b"Paint",
                output: b"paint.png",
                size: None
            })
        );
        assert_eq!(
            parse(&[
                b"icon",
                b"--size",
                b"32",
                b"-o",
                b"/run/icon.png",
                b"--",
                b"-Paint"
            ]),
            Some(Command::Icon {
                name: b"-Paint",
                output: b"/run/icon.png",
                size: Some(b"32")
            })
        );
        assert_eq!(parse(&[b"icon", b"Paint"]), None);
        assert_eq!(parse(&[b"icon", b"-o", b"a.png", b"Paint", b"b.png"]), None);
        assert_eq!(parse(&[b"show", b"Paint", b"b.png"]), None);
        assert_eq!(parse(&[b"icon", b"--window", b"Paint", b"a.png"]), None);
    }

    #[test]
    fn the_built_in_apps_open_by_their_exact_names() {
        assert!(is_built_in(b"Settings") && is_built_in(b"Files") && is_built_in(b"Terminal"));
        assert!(is_built_in(b"Text Editor") && !is_built_in(b"text editor"));
        assert!(!is_built_in(b"settings") && !is_built_in(b"Paint") && !is_built_in(b""));
        assert_eq!(
            parse(&[b"open", b"Settings"]),
            Some(Command::Open { name: b"Settings" })
        );
    }

    #[test]
    fn arguments_make_commands_and_anything_else_is_a_usage_error() {
        assert_eq!(
            parse(&[b"install", b"/media/host/win-paint.exe"]),
            Some(Command::Install {
                path: b"/media/host/win-paint.exe",
                name: None,
                entry: None,
                arguments: None,
                environment: None,
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--name", b"Paint", b"--window", b"x.exe"]),
            Some(Command::Install {
                path: b"x.exe",
                name: Some(b"Paint"),
                entry: None,
                arguments: None,
                environment: None,
                window: true
            })
        );
        assert_eq!(
            parse(&[b"remove", b"Paint"]),
            Some(Command::Uninstall {
                name: b"Paint",
                window: false,
                dry_run: false
            })
        );
        assert_eq!(
            parse(&[b"uninstall", b"--window", b"Paint"]),
            Some(Command::Uninstall {
                name: b"Paint",
                window: true,
                dry_run: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--entry", b"bin/a.exe", b"--", b"tools.zip"]),
            Some(Command::Install {
                path: b"tools.zip",
                name: None,
                entry: Some(b"bin/a.exe"),
                arguments: None,
                environment: None,
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--arguments", b"-n --size 4", b"brogue.zip"]),
            Some(Command::Install {
                path: b"brogue.zip",
                name: None,
                entry: None,
                arguments: Some(b"-n --size 4"),
                environment: None,
                window: false
            })
        );
        assert_eq!(parse(&[b"open", b"--arguments", b"x", b"a"]), None);
        assert_eq!(
            parse(&[
                b"install",
                b"--environment",
                b"XDG_DATA_DIRS=/Applications/mousepad/share",
                b"mousepad.zip"
            ]),
            Some(Command::Install {
                path: b"mousepad.zip",
                name: None,
                entry: None,
                arguments: None,
                environment: Some(b"XDG_DATA_DIRS=/Applications/mousepad/share"),
                window: false
            })
        );
        assert_eq!(parse(&[b"open", b"--environment", b"A=1", b"a"]), None);
        assert_eq!(parse(&[b"install", b"--environment"]), None);
        assert_eq!(parse(&[b"uninstall", b"--entry", b"a", b"b"]), None);
        assert_eq!(parse(&[b"install", b"--entry"]), None);
        assert_eq!(
            parse(&[b"list"]),
            Some(Command::List {
                json: false,
                limit: None,
                after: None
            })
        );
        assert_eq!(
            parse(&[b"list", b"--json"]),
            Some(Command::List {
                json: true,
                limit: None,
                after: None
            })
        );
        assert_eq!(
            parse(&[b"list", b"--limit", b"2", b"--after=fig"]),
            Some(Command::List {
                json: false,
                limit: Some(b"2"),
                after: Some(b"fig")
            })
        );
        assert_eq!(
            parse(&[b"uninstall", b"--dry-run", b"Paint"]),
            Some(Command::Uninstall {
                name: b"Paint",
                window: false,
                dry_run: true
            })
        );
        for wrong in [
            &[&b"list"[..], b"--limit"][..],
            &[b"list", b"--dry-run"],
            &[b"open", b"--dry-run", b"Paint"],
            &[b"uninstall", b"--dry-run", b"--window", b"Paint"],
            &[b"install", b"--limit", b"2", b"x.exe"],
        ] {
            assert_eq!(parse(wrong), None, "{wrong:?}");
        }
        assert_eq!(parse(&[b"open", b"--json", b"Files"]), None);
        assert_eq!(parse(&[b"--help"]), Some(Command::Help));
        for wrong in [
            &[][..],
            &[&b"install"[..]],
            &[b"install", b"a", b"b"],
            &[b"install", b"--name"],
            &[b"install", b"--force", b"a"],
            &[b"uninstall", b"--name", b"x", b"y"],
            &[b"list", b"extra"],
            &[b"list", b"--window"],
            &[b"upgrade", b"x"],
            &[b"open"],
            &[b"open", b"a", b"b"],
            &[b"open", b"--window", b"a"],
            &[b"uninstall", b"--", b"a", b"b"],
            &[b"open", b"--name", b"a"],
        ] {
            assert_eq!(parse(wrong), None, "{wrong:?}");
        }
        assert_eq!(
            parse(&[b"open", b"win-paint"]),
            Some(Command::Open { name: b"win-paint" })
        );
        assert_eq!(
            parse(&[b"open", b"--", b"-odd"]),
            Some(Command::Open { name: b"-odd" })
        );
        assert_eq!(
            parse(&[b"uninstall", b"--", b"--window"]),
            Some(Command::Uninstall {
                name: b"--window",
                window: false,
                dry_run: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--", b"/media/host/-x.exe"]),
            Some(Command::Install {
                path: b"/media/host/-x.exe",
                name: None,
                entry: None,
                arguments: None,
                environment: None,
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"-"]),
            Some(Command::Install {
                path: b"-",
                name: None,
                entry: None,
                arguments: None,
                environment: None,
                window: false
            })
        );
    }

    #[test]
    fn an_app_s_json_record_has_its_fields_with_null_for_none() {
        let mut buffer = [0u8; APP_RECORD_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        write_app_record(
            &mut out,
            "win-paint",
            "windows",
            Some("x86_64"),
            b"/Applications/win-paint/win-paint.exe",
            None,
        );
        assert_eq!(
            core::str::from_utf8(out.as_bytes()).unwrap(),
            "{\"name\":\"win-paint\",\"source\":\"windows\",\"architecture\":\"x86_64\",\
             \"program\":\"/Applications/win-paint/win-paint.exe\",\"icon\":null}\n"
        );
    }

    #[test]
    fn show_takes_a_name_and_json() {
        assert_eq!(
            parse(&[b"show", b"--json", b"Paint"]),
            Some(Command::Show {
                name: b"Paint",
                json: true
            })
        );
        assert_eq!(parse(&[b"show"]), None);
        assert_eq!(parse(&[b"show", b"--window", b"Paint"]), None);
        assert_eq!(parse(&[b"show", b"a", b"b"]), None);
    }

    fn refused_record() -> Record<'static> {
        let mut record = Record::new([0xab; 32], 4096, 20480, true, 3);
        assert!(record.push_site(translation::RefusedSite {
            address: 0x1_4000_1010,
            reason: b"an SSE3 instruction",
        }));
        record
    }

    #[test]
    fn an_app_s_show_record_adds_its_translation() {
        let mut buffer = [0u8; APP_RECORD_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        let record = refused_record();
        write_app_show_record(
            &mut out,
            "Paint",
            "windows",
            Some("x86_64"),
            b"/Applications/Paint/paint.exe",
            None,
            Some(&record),
        );
        let text = core::str::from_utf8(out.as_bytes()).unwrap();
        let key = "ab".repeat(32);
        assert_eq!(
            text,
            format!(
                "{{\"name\":\"Paint\",\"source\":\"windows\",\"architecture\":\"x86_64\",\
                 \"program\":\"/Applications/Paint/paint.exe\",\"icon\":null,\"translation\":\
                 {{\"key\":\"{key}\",\"code_bytes\":4096,\"cache_bytes\":20480,\"stored\":true,\
                 \"refused\":3,\"refused_sites\":[{{\"address\":\"0x140001010\",\
                 \"reason\":\"an SSE3 instruction\"}}]}}}}\n"
            )
        );
    }

    #[test]
    fn the_text_lists_the_sizes_the_count_and_the_sites() {
        let mut lines = Vec::new();
        translation_lines(&refused_record(), |line| lines.push(line.to_owned()));
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("translation:\t4096 bytes of code, a 20480-byte cache in "));
        assert!(lines[0].ends_with(".tx"));
        assert_eq!(lines[1], "refused:\t3 (the first 1 below)");
        assert_eq!(lines[2], "\t0x140001010\tan SSE3 instruction");
    }
}
