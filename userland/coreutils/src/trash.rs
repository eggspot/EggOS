use crate::disk::SystemFiles;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_app::files::Files;
use eggos_files::clock::deletion_date;
use eggos_files::trash::{HOME, RestoreError, Trash};
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: trash [-f] [--] <path>... | trash -l | trash -r <name>... | trash -e";
const USAGE_STATUS: i32 = 2;
const ABSOLUTE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Put,
    List,
    Restore,
    Empty,
}

fn the_trash() -> Option<Trash> {
    Trash::of_home(HOME)
}

pub fn trash<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut action = Action::Put;
    let mut force = false;
    let mut operands = [c""; crate::utilities::MAX_ARGUMENTS];
    let mut count = 0;
    let mut options_end = false;
    for &argument in arguments {
        let chosen = match argument.to_bytes() {
            _ if options_end => None,
            b"--" => {
                options_end = true;
                continue;
            }
            b"-l" | b"--list" => Some(Action::List),
            b"-r" | b"--restore" => Some(Action::Restore),
            b"-e" | b"--empty" => Some(Action::Empty),
            b"-f" | b"--force" => {
                force = true;
                continue;
            }
            bytes if bytes.len() > 1 && bytes[0] == b'-' => {
                output.invalid_option(bytes);
                output.error(&[USAGE]);
                return USAGE_STATUS;
            }
            _ => None,
        };
        match chosen {
            Some(chosen) if action == Action::Put || action == chosen => action = chosen,
            Some(_) => {
                output.error(&[USAGE]);
                return USAGE_STATUS;
            }
            None => {
                if count == operands.len() {
                    output.error(&[b"too many operands"]);
                    return FAILURE;
                }
                operands[count] = argument;
                count += 1;
            }
        }
    }
    let operands = &operands[..count];
    let wants_operands = matches!(action, Action::Put | Action::Restore);
    if wants_operands == operands.is_empty() && !(force && action == Action::Put) {
        output.error(&[USAGE]);
        return USAGE_STATUS;
    }
    let Some(trash) = the_trash() else {
        return FAILURE;
    };
    match action {
        Action::Put => put(output, &trash, operands, force),
        Action::List => list(output, &trash),
        Action::Restore => restore(output, &trash, operands),
        Action::Empty => {
            let emptied = trash.empty(&mut SystemFiles::new(&mut output.system));
            output.system.sync();
            match emptied {
                Ok(_) => 0,
                Err(error) => {
                    output.error(&[b"cannot empty the Trash: ", error.message()]);
                    FAILURE
                }
            }
        }
    }
}

fn absolute_path<S: System>(
    system: &mut S,
    path: &[u8],
    out: &mut [u8; ABSOLUTE_CAPACITY],
) -> Option<usize> {
    if path.first() == Some(&b'/') {
        out.get_mut(..path.len())?.copy_from_slice(path);
        return Some(path.len());
    }
    let mut length = system.working_directory(out).ok()?;
    if out[..length].last() != Some(&b'/') {
        *out.get_mut(length)? = b'/';
        length += 1;
    }
    out.get_mut(length..length + path.len())?
        .copy_from_slice(path);
    Some(length + path.len())
}

fn put<S: System>(output: &mut Output<S>, trash: &Trash, paths: &[&CStr], force: bool) -> i32 {
    let mut status = 0;
    for &path in paths {
        let now = output.system.now();
        let bytes = path.to_bytes();
        let mut absolute = [0u8; ABSOLUTE_CAPACITY];
        let Some(length) = absolute_path(&mut output.system, bytes, &mut absolute) else {
            output.error(&[
                b"cannot move '",
                bytes,
                b"' to the Trash: ",
                Errno::ENAMETOOLONG.message(),
            ]);
            status = FAILURE;
            continue;
        };
        let put = trash.put(
            &mut SystemFiles::new(&mut output.system),
            &absolute[..length],
            now,
        );
        match put {
            Ok(_) => {}
            Err(Errno::ENOENT) if force => {}
            Err(Errno::EXDEV) => {
                output.error(&[
                    b"cannot move '",
                    bytes,
                    b"' to the Trash: it is not on the home folder's file system; use rm",
                ]);
                status = FAILURE;
            }
            Err(error) => {
                output.error(&[
                    b"cannot move '",
                    bytes,
                    b"' to the Trash: ",
                    error.message(),
                ]);
                status = FAILURE;
            }
        }
    }
    output.system.sync();
    status
}

fn list<S: System>(output: &mut Output<S>, trash: &Trash) -> i32 {
    let Some(items) = trash.items() else {
        return 0;
    };
    let mut name = [0u8; 256];
    let mut index = 0;
    loop {
        let found = SystemFiles::new(&mut output.system).entry(items.as_bytes(), index, &mut name);
        let length = match found {
            Ok(Some(length)) => length,
            Ok(None) | Err(Errno::ENOENT) => return 0,
            Err(error) => {
                output.error(&[b"cannot read the Trash: ", error.message()]);
                return FAILURE;
            }
        };
        index += 1;
        let record = trash.record(&mut SystemFiles::new(&mut output.system), &name[..length]);
        let date = record.and_then(|record| record.deleted).map(deletion_date);
        output.write(&name[..length]);
        output.write(b"\t");
        output.write(
            date.as_ref()
                .map_or(&b"-"[..], |date| date.as_str().as_bytes()),
        );
        output.write(b"\t");
        output.write(
            record
                .as_ref()
                .map_or(&b"?"[..], |record| record.original.as_bytes()),
        );
        output.write(b"\n");
    }
}

fn restore<S: System>(output: &mut Output<S>, trash: &Trash, names: &[&CStr]) -> i32 {
    let mut status = 0;
    for &name in names {
        let bytes = name.to_bytes();
        let restored = trash.restore(&mut SystemFiles::new(&mut output.system), bytes);
        match restored {
            Ok(_) => {}
            Err(RestoreError::FolderGone(folder)) => {
                output.error(&[
                    b"cannot restore '",
                    bytes,
                    b"': ",
                    folder.as_bytes(),
                    b" no longer exists",
                ]);
                status = FAILURE;
            }
            Err(RestoreError::NoRecord) => {
                output.error(&[b"cannot restore '", bytes, b"': it is not in the Trash"]);
                status = FAILURE;
            }
            Err(RestoreError::Failed(error)) => {
                output.error(&[b"cannot restore '", bytes, b"': ", error.message()]);
                status = FAILURE;
            }
        }
    }
    output.system.sync();
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn home() -> Fake {
        Fake::default()
            .with_directory("/home", 0)
            .with_directory("/home/root", 0)
            .with_directory("/home/root/Work", 0)
            .with_file("/home/root/Work/notes.txt", 0o644, b"notes", 0)
            .with_directory("/tmp", 0)
            .with_file("/tmp/x", 0o644, b"x", 0)
    }

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        let mut fake = fake;
        fake.standard_error.clear();
        fake.standard_output.clear();
        let mut output = Output::new(fake, b"trash");
        let status = trash(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    #[test]
    fn trash_moves_lists_restores_and_empties() {
        let mut fake = home();
        fake.now = 192;
        let (status, fake) = run(fake, &[c"/home/root/Work/notes.txt"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert!(!fake.nodes.contains_key(&b"/home/root/Work/notes.txt"[..]));
        let (status, fake) = run(fake, &[c"-l"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "notes.txt\t1970-01-01T00:03:12\t/home/root/Work/notes.txt\n"
        );
        let (status, fake) = run(fake, &[c"--restore", c"notes.txt"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert!(fake.nodes.contains_key(&b"/home/root/Work/notes.txt"[..]));
        let (_, fake) = run(fake, &[c"--", c"/home/root/Work"]);
        let (status, fake) = run(fake, &[c"-r", c"notes.txt"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "trash: cannot restore 'notes.txt': it is not in the Trash\n"
        );
        let (status, fake) = run(fake, &[c"--empty"]);
        assert_eq!(status, 0);
        let (_, fake) = run(fake, &[c"-l"]);
        assert_eq!(fake.output(), "");
        assert!(
            !fake
                .nodes
                .keys()
                .any(|path| path.starts_with(b"/home/root/.local/share/Trash/files/"))
        );
    }

    #[test]
    fn trash_refuses_what_it_cannot_take_and_bad_usage() {
        let mut fake = home();
        fake.cross_device = vec![b"/tmp".to_vec()];
        let (status, fake) = run(fake, &[c"/tmp/x"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "trash: cannot move '/tmp/x' to the Trash: it is not on the home folder's file \
             system; use rm\n"
        );
        let (status, fake) = run(fake, &[c"/home/root/none"]);
        assert_eq!(status, 1);
        assert!(fake.errors().ends_with("No such file or directory\n"));
        let (status, fake) = run(fake, &[c"-f", c"/home/root/none"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        let mut fake = home();
        fake.working_directory = "/home/root/Work".to_owned();
        let (status, fake) = run(fake, &[c"notes.txt"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        let (_, fake) = run(fake, &[c"-l"]);
        assert!(fake.output().ends_with("\t/home/root/Work/notes.txt\n"));
        assert_eq!(run(home(), &[]).0, 2);
        assert_eq!(run(home(), &[c"-l", c"x"]).0, 2);
        assert_eq!(run(home(), &[c"-l", c"-e"]).0, 2);
        assert_eq!(run(home(), &[c"-z"]).0, 2);
        let (status, fake) = run(home(), &[c"--", c"-l"]);
        assert_eq!(status, 1);
        assert!(fake.errors().starts_with("trash: cannot move '-l'"));
    }
}
