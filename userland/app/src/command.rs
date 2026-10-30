//! The `app` command (D134, docs/apps.md): `app install [--name <name>] [--entry <path>]
//! [--arguments <words>] [--window] <path>` (a program, an `.app` or a `.zip`; `--entry`
//! chooses the program inside a `.zip`, D148; `--arguments` gives the words it opens with,
//! D172), `app uninstall [--window] <name>` (`remove` is the same), `app open <name>`
//! (D137; the built-in Terminal, Files, Settings and Text Editor too, D141, D149) and `app
//! list` (name, source, architecture, program and icon file, D148; `--json` writes them as JSON
//! lines, D160; `--limit <count>` and `--after <cursor>` page them by name, D164). `app uninstall
//! --dry-run` says what it would remove (D164). Text, the same with or without a display;
//! `--window` shows the installer's dialogs instead. `--` ends the options, so the next word is the operand even if it starts
//! with `-`.

use eggos_mcp::json::JsonWriter;

/// What the command was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    Install {
        path: &'a [u8],
        name: Option<&'a [u8]>,
        entry: Option<&'a [u8]>,
        arguments: Option<&'a [u8]>,
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
    Help,
}

/// The command's usage, printed for `--help` and with a usage error.
pub const USAGE: &str = "usage: app install [--name <name>] [--entry <path>] \
                         [--arguments <words>] [--window] [--] <path>\n       \
                         app uninstall [--window] [--dry-run] [--] <name>\n       \
                         app open [--] <name>\n       \
                         app list [--json] [--limit <count>] [--after <cursor>]\n";

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
    let mut json = false;
    let mut dry_run = false;
    let mut limit = None;
    let mut after = None;
    let mut operand = None;
    let mut rest = rest.iter();
    let mut options_end = false;
    let removing = verb == b"uninstall" || verb == b"remove";
    while let Some(&argument) = rest.next() {
        match argument {
            _ if options_end && operand.is_none() => operand = Some(argument),
            _ if options_end => return None,
            b"--" => options_end = true,
            b"--window" => window = true,
            b"--dry-run" if removing => dry_run = true,
            b"--json" if verb == b"list" => json = true,
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
            _ if argument.starts_with(b"-") && argument.len() > 1 => return None,
            _ if operand.is_none() => operand = Some(argument),
            _ => return None,
        }
    }
    match verb {
        b"install" => Some(Command::Install {
            path: operand?,
            name,
            entry,
            arguments: words,
            window,
        }),
        b"uninstall" | b"remove" if !(window && dry_run) => Some(Command::Uninstall {
            name: operand?,
            window,
            dry_run,
        }),
        b"open" if !window => Some(Command::Open { name: operand? }),
        b"list" if operand.is_none() && !window => Some(Command::List { json, limit, after }),
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
    out.end_object();
    out.end_line();
}

#[cfg(target_env = "musl")]
mod program {
    use super::*;
    use crate::files::Path;
    use crate::installer::{self, APPLICATIONS};
    use crate::linux::{self, LinuxFiles, STANDARD_ERROR, STANDARD_OUTPUT, write_all};
    use crate::messages::{self, Line};
    use core::ffi::CStr;
    use core::fmt::Write;
    use eggos_commands::page::{self, MAX_CURSOR_BYTES, MAX_KEY_BYTES, MORE_PREFIX};
    use eggos_commands::{ErrorDefinition, errors};
    use eggos_runtime::startup::Startup;

    const COPY_BUFFER: usize = 64 * 1024;
    const MAX_ARGUMENTS: usize = 8;

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

    fn install(
        path: &[u8],
        name: Option<&[u8]>,
        entry: Option<&[u8]>,
        arguments: Option<&[u8]>,
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
                ..
            }) => install(path, name, entry, arguments),
            Some(Command::Uninstall {
                name,
                dry_run: true,
                ..
            }) => preview_uninstall(name),
            Some(Command::Uninstall { name, .. }) => uninstall(name),
            Some(Command::Open { name }) => open(name),
            Some(Command::List { json, limit, after }) => list(json, limit, after),
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
                window: false
            })
        );
        assert_eq!(parse(&[b"open", b"--arguments", b"x", b"a"]), None);
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
}
