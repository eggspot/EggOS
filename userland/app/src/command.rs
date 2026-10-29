//! The `app` command (D134, docs/apps.md): `app install [--name <name>] [--entry <path>]
//! [--window] <path>` (a program, an `.app` or a `.zip`; `--entry` chooses the program inside a
//! `.zip`, D148), `app uninstall [--window] <name>` (`remove` is the same), `app open <name>`
//! (D137; the built-in Terminal, Files, Settings and Text Editor too, D141, D149) and `app
//! list` (name, source, architecture, program and icon file, D148). Text only, the same with or
//! without a display; `--window` shows the installer's dialogs instead. `--` ends the options,
//! so the next word is the operand even if it starts with `-`.

/// What the command was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    Install {
        path: &'a [u8],
        name: Option<&'a [u8]>,
        entry: Option<&'a [u8]>,
        window: bool,
    },
    Uninstall {
        name: &'a [u8],
        window: bool,
    },
    Open {
        name: &'a [u8],
    },
    List,
    Help,
}

/// The command's usage, printed for `--help` and with a usage error.
pub const USAGE: &str = "usage: app install [--name <name>] [--entry <path>] [--window] \
                         [--] <path>\n       \
                         app uninstall [--window] [--] <name>\n       \
                         app open [--] <name>\n       app list\n";

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
    let mut operand = None;
    let mut rest = rest.iter();
    let mut options_end = false;
    while let Some(&argument) = rest.next() {
        match argument {
            _ if options_end && operand.is_none() => operand = Some(argument),
            _ if options_end => return None,
            b"--" => options_end = true,
            b"--window" => window = true,
            b"--name" if verb == b"install" => name = Some(*rest.next()?),
            b"--entry" if verb == b"install" => entry = Some(*rest.next()?),
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
            window,
        }),
        b"uninstall" | b"remove" => Some(Command::Uninstall {
            name: operand?,
            window,
        }),
        b"open" if !window => Some(Command::Open { name: operand? }),
        b"list" if operand.is_none() && !window => Some(Command::List),
        b"--help" | b"help" if operand.is_none() => Some(Command::Help),
        _ => None,
    }
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

    fn install(path: &[u8], name: Option<&[u8]>, entry: Option<&[u8]>) -> i32 {
        let mut files = LinuxFiles::new();
        let machine = linux::machine();
        let mut subject = Line::new();
        subject.push_bytes(crate::files::file_name(path));
        let plan = match installer::plan_entry(&mut files, path, name, entry, machine) {
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

    fn list() -> i32 {
        let mut files = LinuxFiles::new();
        let listed = installer::for_each_installed(&mut files, |app| {
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
            Ok(()) => 0,
            Err(error) => {
                let mut line = Line::new();
                let _ = line.write_str("cannot list /Applications: ");
                line.push_bytes(error.message());
                complain(line.as_str());
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
                path, name, entry, ..
            }) => install(path, name, entry),
            Some(Command::Uninstall { name, .. }) => uninstall(name),
            Some(Command::Open { name }) => open(name),
            Some(Command::List) => list(),
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
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--name", b"Paint", b"--window", b"x.exe"]),
            Some(Command::Install {
                path: b"x.exe",
                name: Some(b"Paint"),
                entry: None,
                window: true
            })
        );
        assert_eq!(
            parse(&[b"remove", b"Paint"]),
            Some(Command::Uninstall {
                name: b"Paint",
                window: false
            })
        );
        assert_eq!(
            parse(&[b"uninstall", b"--window", b"Paint"]),
            Some(Command::Uninstall {
                name: b"Paint",
                window: true
            })
        );
        assert_eq!(
            parse(&[b"install", b"--entry", b"bin/a.exe", b"--", b"tools.zip"]),
            Some(Command::Install {
                path: b"tools.zip",
                name: None,
                entry: Some(b"bin/a.exe"),
                window: false
            })
        );
        assert_eq!(parse(&[b"uninstall", b"--entry", b"a", b"b"]), None);
        assert_eq!(parse(&[b"install", b"--entry"]), None);
        assert_eq!(parse(&[b"list"]), Some(Command::List));
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
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"--", b"/media/host/-x.exe"]),
            Some(Command::Install {
                path: b"/media/host/-x.exe",
                name: None,
                entry: None,
                window: false
            })
        );
        assert_eq!(
            parse(&[b"install", b"-"]),
            Some(Command::Install {
                path: b"-",
                name: None,
                entry: None,
                window: false
            })
        );
    }
}
