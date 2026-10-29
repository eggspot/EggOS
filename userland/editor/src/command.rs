//! The `edit` command (D149, docs/apps.md): `edit [--] [<path>]` opens the file at `path` (or
//! an untitled document) in a Text Editor window of its own and returns at once, the window
//! left running on its own (a session of its own, its standard streams on a pipe nobody reads
//! from), so a terminal or an agent's command goes on. A file the editor would refuse is
//! refused here with the editor's words; a path with nothing at it opens as a new file. With
//! no desktop there is no editor to open: the command says so and names `cat` and `tee`.

/// What the command was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Open this file, or an untitled document.
    Open(Option<&'a [u8]>),
    /// Print the usage.
    Help,
}

/// The command's usage.
pub const USAGE: &str = "usage: edit [--] [<path>]\n";
/// What the command says with no desktop.
pub const NO_DESKTOP: &str = "edit: there is no desktop to open the Text Editor on; `cat` \
                              shows a file and `tee` writes one\n";
/// The program that shows the window.
pub const EDITOR_PROGRAM: &str = "/bin/text-editor";
/// Exit status of a refused or failed command.
pub const FAILED: i32 = 1;
/// Exit status of a usage error.
pub const USAGE_ERROR: i32 = 2;

/// The command `arguments` (after the program's name) ask for; `None` for a usage error.
pub fn parse<'a>(arguments: &[&'a [u8]]) -> Option<Command<'a>> {
    match arguments {
        [] | [b"--"] => Some(Command::Open(None)),
        [b"--help" | b"-h"] => Some(Command::Help),
        [b"--", path] => Some(Command::Open(Some(*path))),
        [path] if !path.starts_with(b"-") || *path == b"-" => Some(Command::Open(Some(*path))),
        _ => None,
    }
}

#[cfg(all(feature = "window", target_env = "musl"))]
mod program {
    use super::*;
    use crate::document::TEXT_CAPACITY;
    use crate::file::{self, OpenProblem};
    use crate::program::absolute;
    use core::ffi::CStr;
    use eggos_app::files::{self as paths, PATH_CAPACITY, Path};
    use eggos_app::linux::{LinuxFiles, STANDARD_ERROR, STANDARD_OUTPUT, write_all};
    use eggos_app::messages::Line;
    use eggos_runtime::errno::Errno;
    use eggos_runtime::process::{ChildStack, Spawn, spawn};
    use eggos_runtime::startup::Startup;
    use eggos_runtime::syscall::{self, number};
    use eggos_toolkit::window::WindowServer;

    const MAX_ARGUMENTS: usize = 4;
    const MAX_VARIABLES: usize = 32;
    const O_CLOEXEC: usize = 0o2_000_000;
    const EDITOR: &CStr = c"/bin/text-editor";

    fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
        // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as
        // the call reads or writes them, and no call here maps or changes the program's memory.
        Errno::result(unsafe { syscall::call(number, arguments) })
    }

    fn say(descriptor: usize, line: &str) {
        write_all(descriptor, line.as_bytes());
        write_all(descriptor, b"\n");
    }

    fn complain(line: &str) -> i32 {
        write_all(STANDARD_ERROR, b"edit: ");
        say(STANDARD_ERROR, line);
        FAILED
    }

    fn has_desktop() -> bool {
        WindowServer::open().is_ok_and(|server| server.work_area().is_ok())
    }

    fn start(startup: &Startup, path: Option<&[u8]>) -> Result<(), Errno> {
        let mut held = [0u8; PATH_CAPACITY + 1];
        let mut arguments = [core::ptr::null::<u8>(); 3];
        arguments[0] = EDITOR.as_ptr().cast();
        if let Some(path) = path {
            held[..path.len()].copy_from_slice(path);
            arguments[1] = held.as_ptr();
        }
        let mut environment = [core::ptr::null::<u8>(); MAX_VARIABLES + 1];
        for (slot, variable) in environment[..MAX_VARIABLES]
            .iter_mut()
            .zip(startup.environment())
        {
            *slot = variable.as_ptr().cast();
        }
        let mut pipe = [0i32; 2];
        call(
            number::PIPE2,
            [pipe.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0],
        )?;
        let (reading, writing) = (pipe[0] as usize, pipe[1] as usize);
        let ended = match path {
            Some(_) => &arguments[..],
            None => &arguments[..2],
        };
        let request =
            Spawn::new(EDITOR, ended, &environment, [reading, writing, writing]).in_own_session();
        let mut stack = ChildStack::new();
        let started = spawn(&mut stack, &request);
        let _ = call(number::CLOSE, [reading, 0, 0, 0, 0, 0]);
        let _ = call(number::CLOSE, [writing, 0, 0, 0, 0, 0]);
        started.map(|_| ())
    }

    /// The program: see the module documentation.
    pub fn run(startup: &Startup) -> i32 {
        let mut words: [&[u8]; MAX_ARGUMENTS] = [b""; MAX_ARGUMENTS];
        let mut count = 0;
        for argument in startup.arguments().skip(1) {
            if count == MAX_ARGUMENTS {
                write_all(STANDARD_ERROR, USAGE.as_bytes());
                return USAGE_ERROR;
            }
            words[count] = argument.to_bytes();
            count += 1;
        }
        let given = match parse(&words[..count]) {
            Some(Command::Help) => {
                write_all(STANDARD_OUTPUT, USAGE.as_bytes());
                return 0;
            }
            Some(Command::Open(given)) => given,
            None => {
                write_all(STANDARD_ERROR, USAGE.as_bytes());
                return USAGE_ERROR;
            }
        };
        if !has_desktop() {
            write_all(STANDARD_ERROR, NO_DESKTOP.as_bytes());
            return FAILED;
        }
        let path = match given.map(absolute) {
            Some(Some(path)) => Some(path),
            Some(None) => return complain("the path is too long"),
            None => None,
        };
        let mut files = LinuxFiles::new();
        if let Some(path) = &path {
            let name = core::str::from_utf8(paths::file_name(path.as_bytes())).unwrap_or("?");
            match file::check(&mut files, path.as_bytes(), TEXT_CAPACITY) {
                Ok(_) | Err(OpenProblem::Missing) => {}
                Err(problem) => return complain(file::open_message(name, problem).as_str()),
            }
        }
        if let Err(error) = start(startup, path.as_ref().map(Path::as_bytes)) {
            let mut line = Line::new();
            let _ = core::fmt::Write::write_str(&mut line, "cannot start the Text Editor: ");
            line.push_bytes(error.message());
            return complain(line.as_str());
        }
        let mut line = Line::new();
        let _ = core::fmt::Write::write_str(&mut line, "Opened ");
        match &path {
            Some(path) => {
                line.push_bytes(path.as_bytes());
                let _ = core::fmt::Write::write_str(&mut line, " in Text Editor.");
            }
            None => {
                let _ = core::fmt::Write::write_str(&mut line, "Text Editor.");
            }
        }
        say(STANDARD_OUTPUT, line.as_str());
        0
    }
}

#[cfg(all(feature = "window", target_env = "musl"))]
pub use program::run;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_or_nothing_opens_and_options_are_usage_errors() {
        assert_eq!(parse(&[]), Some(Command::Open(None)));
        assert_eq!(
            parse(&[b"notes.txt"]),
            Some(Command::Open(Some(&b"notes.txt"[..])))
        );
        assert_eq!(
            parse(&[b"--", b"-odd.txt"]),
            Some(Command::Open(Some(&b"-odd.txt"[..])))
        );
        assert_eq!(parse(&[b"-"]), Some(Command::Open(Some(&b"-"[..]))));
        assert_eq!(parse(&[b"--help"]), Some(Command::Help));
        assert_eq!(parse(&[b"--"]), Some(Command::Open(None)));
        for wrong in [
            &[&b"-x"[..]][..],
            &[b"a", b"b"],
            &[b"--print", b"a"],
            &[b"--", b"a", b"b"],
        ] {
            assert_eq!(parse(wrong), None, "{wrong:?}");
        }
    }
}
