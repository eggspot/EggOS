use crate::disk::{ABSOLUTE_CAPACITY, SystemFiles, absolute_path};
use crate::options::{Refused, parse_defined};
use crate::output::{FAILURE, Output};
use crate::page::Page;
use crate::system::{SymbolicLinks, System};
use core::ffi::CStr;
use eggos_app::files::Files;
use eggos_commands::commands::LIST_PAGE_LIMIT;
use eggos_commands::page::{MAX_KEY_BYTES, Smallest};
use eggos_commands::{commands, errors};
use eggos_files::clock::deletion_date;
use eggos_files::names::is_valid;
use eggos_files::trash::{HOME, Record, RestoreError, Trash};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: trash [-f] [--dry-run] [--] <path>... | trash -l [--json] \
    [--limit <count>] [--after <cursor>] | trash -r <name>... | trash -e";
const USAGE_STATUS: i32 = 2;
const PAGE_SLOTS: usize = LIST_PAGE_LIMIT as usize + 1;
const RECORD_JSON_BYTES: usize = 4 * 4096 + 256;

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
    let mut dry_run = false;
    let mut json = false;
    let mut page = Page::default();
    let mut explained = false;
    let parsed = parse_defined(output, arguments, &commands::TRASH, |output, given| {
        match page.take(output, given) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(refused) => {
                explained = true;
                return Err(refused);
            }
        }
        let chosen = match given.option.short {
            Some(b'l') => Action::List,
            Some(b'r') => Action::Restore,
            Some(b'e') => Action::Empty,
            Some(b'f') => {
                force = true;
                return Ok(());
            }
            _ => {
                dry_run |= given.is_long("dry-run");
                json |= given.is_long("json");
                return Ok(());
            }
        };
        if action != Action::Put && action != chosen {
            return Err(Refused::Reported);
        }
        action = chosen;
        Ok(())
    });
    let operands = match parsed {
        Ok(operands) => operands,
        Err(_) if explained => return USAGE_STATUS,
        Err(_) => {
            output.coded(&errors::USAGE, &[USAGE]);
            return USAGE_STATUS;
        }
    };
    let operands = operands.all();
    let wants_operands = matches!(action, Action::Put | Action::Restore);
    let lists = action == Action::List;
    if (wants_operands == operands.is_empty() && !(force && action == Action::Put))
        || (dry_run && action != Action::Put)
        || ((json || page.is_paged()) && !lists)
    {
        output.coded(&errors::USAGE, &[USAGE]);
        return USAGE_STATUS;
    }
    if dry_run {
        return preview(output, operands, force);
    }
    let Some(trash) = the_trash() else {
        return FAILURE;
    };
    match action {
        Action::Put => put(output, &trash, operands, force),
        Action::List => list(output, &trash, &page),
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

fn write_escaped<S: System>(output: &mut Output<S>, bytes: &[u8]) {
    for &byte in bytes {
        match byte {
            b'\\' => output.write(b"\\\\"),
            byte if byte < b' ' || byte == 0x7f => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                output.write(&[
                    b'\\',
                    b'x',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xf)],
                ]);
            }
            byte => output.write(&[byte]),
        }
    }
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

fn preview<S: System>(output: &mut Output<S>, paths: &[&CStr], force: bool) -> i32 {
    let mut status = 0;
    for &path in paths {
        match output.system.status(None, path, SymbolicLinks::DoNotFollow) {
            Ok(_) => {
                output.write(b"would move '");
                write_escaped(output, path.to_bytes());
                output.write(b"' to the Trash\n");
            }
            Err(Errno::ENOENT) if force => {}
            Err(errno) => {
                output.file_error(path, errno);
                status = FAILURE;
            }
        }
    }
    status
}

fn each_entry<S: System>(
    output: &mut Output<S>,
    trash: &Trash,
    mut entry: impl FnMut(&mut Output<S>, &[u8]),
) -> Result<(), i32> {
    let Some(items) = trash.items() else {
        return Ok(());
    };
    let mut name = [0u8; MAX_KEY_BYTES + 1];
    let mut index = 0;
    loop {
        let found = SystemFiles::new(&mut output.system).entry(items.as_bytes(), index, &mut name);
        let length = match found {
            Ok(Some(length)) => length,
            Ok(None) | Err(Errno::ENOENT) => return Ok(()),
            Err(error) => {
                output.error(&[b"cannot read the Trash: ", error.message()]);
                return Err(FAILURE);
            }
        };
        index += 1;
        entry(output, &name[..length]);
    }
}

fn list<S: System>(output: &mut Output<S>, trash: &Trash, page: &Page) -> i32 {
    let after = page.after();
    let comes_after = |name: &[u8]| after.is_none_or(|after| name > after);
    let mut selected = None;
    if let Some(limit) = page.limit {
        let mut smallest = Smallest::<MAX_KEY_BYTES, PAGE_SLOTS>::new(limit as usize);
        let walked = each_entry(output, trash, |_, name| {
            if comes_after(name) {
                smallest.offer(name);
            }
        });
        if let Err(status) = walked {
            return status;
        }
        selected = Some(smallest);
    }
    let walked = each_entry(output, trash, |output, name| {
        let shown = match &selected {
            Some(smallest) => smallest.holds(name),
            None => comes_after(name),
        };
        if shown {
            let record = trash.record(&mut SystemFiles::new(&mut output.system), name);
            match output.is_json() {
                true => write_entry_json(output, name, record.as_ref()),
                false => write_entry(output, name, record.as_ref()),
            }
        }
    });
    if let Err(status) = walked {
        return status;
    }
    let last = selected
        .as_ref()
        .filter(|smallest| smallest.more())
        .and_then(|smallest| smallest.last());
    if let Some(last) = last {
        output.more(last);
    }
    0
}

fn write_entry<S: System>(output: &mut Output<S>, name: &[u8], record: Option<&Record>) {
    let date = record.and_then(|record| record.deleted).map(deletion_date);
    write_escaped(output, name);
    output.write(b"\t");
    output.write(
        date.as_ref()
            .map_or(&b"-"[..], |date| date.as_str().as_bytes()),
    );
    output.write(b"\t");
    write_escaped(
        output,
        record.map_or(&b"?"[..], |record| record.original.as_bytes()),
    );
    output.write(b"\n");
}

fn write_entry_json<S: System>(output: &mut Output<S>, name: &[u8], record: Option<&Record>) {
    let mut buffer = [0u8; RECORD_JSON_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("name");
    out.string_lossy(name);
    out.key("deleted_at");
    let deleted = record
        .and_then(|record| record.deleted)
        .and_then(|seconds| i64::try_from(seconds).ok())
        .and_then(eggos_calendar::rfc3339);
    match deleted {
        Some(stamp) => out.string_lossy(&stamp),
        None => out.null(),
    }
    out.key("original_path");
    match record {
        Some(record) => out.string_lossy(record.original.as_bytes()),
        None => out.null(),
    }
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

fn restore<S: System>(output: &mut Output<S>, trash: &Trash, names: &[&CStr]) -> i32 {
    let mut status = 0;
    for &name in names {
        let bytes = name.to_bytes();
        if !is_valid(bytes) {
            output.error(&[b"cannot restore '", bytes, b"': it is not in the Trash"]);
            status = FAILURE;
            continue;
        }
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
            Err(RestoreError::OutsideHome) => {
                output.error(&[
                    b"cannot restore '",
                    bytes,
                    b"': its record points outside the home folder",
                ]);
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
    fn dry_run_says_what_it_would_move_and_moves_nothing() {
        let (status, fake) = run(
            home(),
            &[
                c"--dry-run",
                c"/home/root/Work/notes.txt",
                c"/home/root/Work",
            ],
        );
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(
            fake.output(),
            "would move '/home/root/Work/notes.txt' to the Trash\n\
             would move '/home/root/Work' to the Trash\n"
        );
        assert_eq!(fake.contents("/home/root/Work/notes.txt"), b"notes");
        let (status, fake) = run(home(), &[c"--dry-run", c"/home/root/gone"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "trash: /home/root/gone: No such file or directory\n"
        );
        let (status, _) = run(home(), &[c"-f", c"--dry-run", c"/home/root/gone"]);
        assert_eq!(status, 0);
        let (status, _) = run(home(), &[c"-l", c"--dry-run"]);
        assert_eq!(status, 2);
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
        let (status, fake) = run(fake, &[c"-r", c"../../etc"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "trash: cannot restore '../../etc': it is not in the Trash\n"
        );
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

    fn three_in_the_trash() -> Fake {
        let mut fake = home()
            .with_file("/home/root/Work/c.txt", 0o644, b"c", 0)
            .with_file("/home/root/Work/a.txt", 0o644, b"a", 0)
            .with_file("/home/root/Work/b.txt", 0o644, b"b", 0);
        fake.now = 192;
        let paths = [
            c"/home/root/Work/c.txt",
            c"/home/root/Work/a.txt",
            c"/home/root/Work/b.txt",
        ];
        let (status, fake) = run(fake, &paths);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        fake
    }

    fn sorted_lines(text: &str) -> Vec<&str> {
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort();
        lines
    }

    #[test]
    fn the_trash_lists_a_page_at_a_time_by_name() {
        let (status, fake) = run(three_in_the_trash(), &[c"-l", c"--limit", c"2"]);
        assert_eq!(status, 0);
        assert_eq!(
            sorted_lines(&fake.output()),
            [
                "a.txt\t1970-01-01T00:03:12\t/home/root/Work/a.txt",
                "b.txt\t1970-01-01T00:03:12\t/home/root/Work/b.txt",
            ]
        );
        assert_eq!(fake.errors(), "more: --after b.txt\n");
        let (status, fake) = run(fake, &[c"-l", c"--limit=2", c"--after", c"b.txt"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "c.txt\t1970-01-01T00:03:12\t/home/root/Work/c.txt\n"
        );
        assert_eq!(fake.errors(), "");
        let (status, fake) = run(fake, &[c"--list", c"--after", c"a.txt"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output().lines().count(), 2);
        assert_eq!(fake.errors(), "");
        let (status, fake) = run(fake, &[c"-l", c"--after", c"c.txt"]);
        assert_eq!((status, fake.output().as_str()), (0, ""));
    }

    #[test]
    fn the_trash_lists_json_records_and_names_the_next_page() {
        let (status, fake) = run(three_in_the_trash(), &[c"-l", c"--json", c"--limit", c"1"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "{\"name\":\"a.txt\",\"deleted_at\":\"1970-01-01T00:03:12Z\",\
             \"original_path\":\"/home/root/Work/a.txt\"}\n"
        );
        assert_eq!(
            fake.errors(),
            "{\"page\":{\"truncated\":true,\"truncated_by\":\"limit\",\
             \"next_cursor\":\"a.txt\"}}\n"
        );
        let fake = fake.with_file(
            "/home/root/.local/share/Trash/info/b.txt.trashinfo",
            0o600,
            b"[Trash Info]\nPath=/home/root/Work/b.txt\nDeletionDate=x\n",
            0,
        );
        let (_, fake) = run(
            fake,
            &[c"-l", c"--json", c"--after", c"a.txt", c"--limit", c"1"],
        );
        assert_eq!(
            fake.output(),
            "{\"name\":\"b.txt\",\"deleted_at\":null,\
             \"original_path\":\"/home/root/Work/b.txt\"}\n"
        );
    }

    #[test]
    fn a_bad_page_or_a_page_of_another_action_is_refused() {
        let (status, fake) = run(three_in_the_trash(), &[c"-l", c"--after", c"-x"]);
        assert_eq!(status, 2);
        assert_eq!(
            fake.errors(),
            "trash: invalid cursor '-x': not one a page ended with\n"
        );
        let (status, fake) = run(fake, &[c"-l", c"--json", c"--after", c"a b"]);
        assert_eq!(status, 2);
        assert!(
            fake.errors().contains("\"code\":\"bad_cursor\""),
            "{}",
            fake.errors()
        );
        assert!(
            !fake.errors().contains("\"code\":\"usage\""),
            "{}",
            fake.errors()
        );
        let (status, fake) = run(fake, &[c"-l", c"--limit", c"101"]);
        assert_eq!(status, 2);
        assert!(fake.errors().starts_with("trash: invalid limit '101'"));
        for arguments in [
            &[c"--limit", c"1", c"/home/root/Work"][..],
            &[c"--json", c"-r", c"a.txt"],
            &[c"-e", c"--after", c"a.txt"],
        ] {
            let (status, fake) = run(home(), arguments);
            assert_eq!(status, 2, "{arguments:?}");
            assert!(fake.errors().contains("usage: trash"), "{}", fake.errors());
        }
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
        let (_, mut fake) = run(fake, &[c"-l"]);
        assert!(fake.output().ends_with("\t/home/root/Work/notes.txt\n"));
        fake = fake.with_file(
            "/home/root/.local/share/Trash/info/notes.txt.trashinfo",
            0o600,
            b"[Trash Info]\nPath=/home/root/a%1B%5Bb\\c\nDeletionDate=x\n",
            0,
        );
        let (_, fake) = run(fake, &[c"-l"]);
        assert_eq!(fake.output(), "notes.txt\t-\t/home/root/a\\x1b[b\\\\c\n");
        assert_eq!(run(home(), &[]).0, 2);
        assert_eq!(run(home(), &[c"-l", c"x"]).0, 2);
        assert_eq!(run(home(), &[c"-l", c"-e"]).0, 2);
        assert_eq!(run(home(), &[c"-le"]).0, 2);
        let (status, fake) = run(home(), &[c"-z"]);
        assert_eq!(status, 2);
        assert!(
            fake.errors()
                .starts_with("trash: invalid option -- 'z'\ntrash: usage: trash [-f]"),
            "{}",
            fake.errors()
        );
        let (status, fake) = run(home(), &[c"--", c"-l"]);
        assert_eq!(status, 1);
        assert!(fake.errors().starts_with("trash: cannot move '-l'"));
    }
}
