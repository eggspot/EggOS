//! The Trash (D135), laid out as the freedesktop.org Trash specification says, so Linux tools
//! read it too: `<home>/.local/share/Trash/files/<name>` holds a trashed file or folder and
//! `info/<name>.trashinfo` where it came from and when:
//!
//! ```text
//! [Trash Info]
//! Path=/home/root/Work/notes%20old.txt
//! DeletionDate=1970-01-01T00:03:12
//! ```
//!
//! An item is trashed by a rename, so only what is on the home folder's file system can be;
//! the record is made first (claiming its name with an exclusive create), then the item moved,
//! so a cut between the two leaves a record without an item, which the listing skips. The
//! name in the Trash is the item's own, or `<name>.2`, `<name>.3`, ... when taken. Restore
//! moves it back to its folder under a name that is free there (`notes copy.txt`).

use crate::clock::{deletion_date, parse_deletion_date};
use crate::names::{Name, copy_name, free_path, is_within, joined, normalized, parent};
use core::fmt::Write;
use eggos_app::files::{self, Files, Kind, Path};
use eggos_app::installer::remove_tree;
use eggos_runtime::errno::Errno;

/// The user's home folder, whose Trash Files and `trash` use: every process runs as root
/// until there are accounts (D132).
pub const HOME: &[u8] = b"/home/root";
/// Where the Trash is under a home folder.
pub const UNDER_HOME: &[u8] = b".local/share/Trash";
/// The extension of a record.
pub const INFO_EXTENSION: &[u8] = b".trashinfo";
const HEADER: &[u8] = b"[Trash Info]";
const PATH_KEY: &[u8] = b"Path=";
const DATE_KEY: &[u8] = b"DeletionDate=";
const INFO_CAPACITY: usize = 2048;
const INFO_PERMISSIONS: u32 = 0o600;
const MOST_NAMES: usize = 10_000;
const UNESCAPED: &[u8] = b"-_.~/";

/// What a record says about a trashed item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Where it was.
    pub original: Path,
    /// When it was trashed, in seconds since the epoch, if the record says so validly.
    pub deleted: Option<u64>,
}

/// Why an item could not be restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum RestoreError {
    /// Its folder no longer exists: the folder's path.
    FolderGone(Path),
    /// The Trash has no record of where it came from.
    NoRecord,
    /// Its record names a place outside the Trash's home folder, or inside the Trash, or a
    /// path that is not plain (with `.` or `..`): a record nobody but a forger writes.
    OutsideHome,
    /// The file system refused.
    Failed(Errno),
}

/// The Trash of one home folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trash {
    root: Path,
}

fn encode(path: &[u8], out: &mut Line) {
    for &byte in path {
        if byte.is_ascii_alphanumeric() || UNESCAPED.contains(&byte) {
            out.push(&[byte]);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

fn hex(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

fn decode(text: &[u8]) -> Option<Path> {
    let mut path = Path::new();
    let mut at = 0;
    while at < text.len() {
        let byte = match text[at] {
            b'%' => {
                let high = hex(*text.get(at + 1)?)?;
                let low = hex(*text.get(at + 2)?)?;
                at += 2;
                high << 4 | low
            }
            byte => byte,
        };
        if !path.append(&[byte]) {
            return None;
        }
        at += 1;
    }
    (path.as_bytes().first() == Some(&b'/')).then_some(path)
}

struct Line {
    bytes: [u8; INFO_CAPACITY],
    length: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; INFO_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> bool {
        let end = self.length + bytes.len();
        if end > INFO_CAPACITY {
            return false;
        }
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
        true
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        match self.push(text.as_bytes()) {
            true => Ok(()),
            false => Err(core::fmt::Error),
        }
    }
}

/// Makes the folder `path` and every folder above it that is missing.
pub fn make_folders(files: &mut impl Files, path: &[u8]) -> Result<(), Errno> {
    let mut end = 0;
    while end < path.len() {
        end = path[end + 1..]
            .iter()
            .position(|&byte| byte == b'/')
            .map_or(path.len(), |at| end + 1 + at);
        let prefix = &path[..end];
        match files.make_directory(prefix) {
            Ok(()) => {}
            Err(Errno::EEXIST) => {
                if files.status_following(prefix)?.kind != Kind::Directory {
                    return Err(Errno::ENOTDIR);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn record_of(bytes: &[u8]) -> Option<Record> {
    let mut lines = bytes
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    if lines.next()? != HEADER {
        return None;
    }
    let (mut original, mut deleted) = (None, None);
    for line in lines {
        if let Some(value) = line.strip_prefix(PATH_KEY) {
            original = decode(value);
        } else if let Some(value) = line.strip_prefix(DATE_KEY) {
            deleted = parse_deletion_date(value);
        }
    }
    Some(Record {
        original: original?,
        deleted,
    })
}

impl Trash {
    /// The Trash of the home folder `home`.
    pub fn of_home(home: &[u8]) -> Option<Self> {
        Some(Self {
            root: Path::from(home)?.join(UNDER_HOME)?,
        })
    }

    /// Its folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder holding the trashed items.
    pub fn items(&self) -> Option<Path> {
        self.root.join(b"files")
    }

    fn records(&self) -> Option<Path> {
        self.root.join(b"info")
    }

    /// Where trashed item `name` is.
    pub fn item(&self, name: &[u8]) -> Option<Path> {
        self.items()?.join(name)
    }

    fn record_path(&self, name: &[u8]) -> Option<Path> {
        let mut path = self.records()?.join(name)?;
        path.append(INFO_EXTENSION).then_some(path)
    }

    /// Whether `path` is the Trash or inside it.
    pub fn holds(&self, path: &[u8]) -> bool {
        is_within(path, self.root.as_bytes())
    }

    /// Moves `path` to the Trash at time `now`: the name it has there. `EXDEV` when it is on
    /// another file system than the Trash, `EINVAL` for the Trash itself or `/`.
    pub fn put(&self, files: &mut impl Files, path: &[u8], now: u64) -> Result<Name, Errno> {
        let base = files::file_name(path);
        if self.holds(path)
            || base.is_empty()
            || base == b"/"
            || is_within(self.root.as_bytes(), path)
        {
            return Err(Errno::EINVAL);
        }
        files.status(path)?;
        let items = self.items().ok_or(Errno::ENAMETOOLONG)?;
        let records = self.records().ok_or(Errno::ENAMETOOLONG)?;
        make_folders(files, items.as_bytes())?;
        make_folders(files, records.as_bytes())?;
        let mut record = Line::new();
        record.push(HEADER);
        record.push(b"\n");
        record.push(PATH_KEY);
        encode(path, &mut record);
        let _ = write!(
            record,
            "\n{}{}\n",
            core::str::from_utf8(DATE_KEY).unwrap_or(""),
            deletion_date(now).as_str()
        );
        for number in 0..MOST_NAMES {
            let mut name = Name::from(base).ok_or(Errno::ENAMETOOLONG)?;
            if number > 0 && write!(name, ".{}", number + 1).is_err() {
                return Err(Errno::ENAMETOOLONG);
            }
            let item = joined(&items, name.as_bytes()).ok_or(Errno::ENAMETOOLONG)?;
            if files.status(item.as_bytes()).is_ok() {
                continue;
            }
            let record_path = self
                .record_path(name.as_bytes())
                .ok_or(Errno::ENAMETOOLONG)?;
            let file = match files.create(record_path.as_bytes(), INFO_PERMISSIONS) {
                Ok(file) => file,
                Err(Errno::EEXIST) => continue,
                Err(error) => return Err(error),
            };
            let written = files
                .write(file, record.as_bytes())
                .and_then(|()| files.sync_file(file));
            files.close(file);
            let moved = written.and_then(|()| files.rename(path, item.as_bytes()));
            if let Err(error) = moved {
                let _ = files.remove_file(record_path.as_bytes());
                return Err(error);
            }
            return Ok(name);
        }
        Err(Errno::EEXIST)
    }

    /// What the record of trashed item `name` says.
    pub fn record(&self, files: &mut impl Files, name: &[u8]) -> Option<Record> {
        let path = self.record_path(name)?;
        let file = files.open(path.as_bytes()).ok()?;
        let mut bytes = [0u8; INFO_CAPACITY];
        let mut length = 0;
        while length < bytes.len() {
            match files.read(file, &mut bytes[length..]) {
                Ok(0) | Err(_) => break,
                Ok(read) => length += read,
            }
        }
        files.close(file);
        record_of(&bytes[..length])
    }

    /// Calls `each` with every trashed item's name and record, in the folder's order; an item
    /// whose record is missing or broken gets `None`.
    pub fn for_each(
        &self,
        files: &mut impl Files,
        mut each: impl FnMut(&[u8], Option<Record>),
    ) -> Result<(), Errno> {
        let Some(items) = self.items() else {
            return Ok(());
        };
        let mut name = [0u8; 256];
        let mut index = 0;
        loop {
            let found = match files.entry(items.as_bytes(), index, &mut name) {
                Ok(found) => found,
                Err(Errno::ENOENT) => return Ok(()),
                Err(error) => return Err(error),
            };
            let Some(length) = found else {
                return Ok(());
            };
            index += 1;
            let record = self.record(files, &name[..length]);
            each(&name[..length], record);
        }
    }

    /// The home folder whose Trash this is.
    pub fn home(&self) -> &[u8] {
        self.root
            .as_bytes()
            .strip_suffix(UNDER_HOME)
            .and_then(|home| home.strip_suffix(b"/"))
            .unwrap_or(b"/")
    }

    /// Whether an item may go back to `original`: a plain absolute path (no `.`, `..` or
    /// repeated slashes) strictly inside the home folder and outside the Trash itself.
    pub fn may_restore_to(&self, original: &[u8]) -> bool {
        let plain = normalized(original).is_some_and(|path| path.as_bytes() == original);
        plain
            && original.first() == Some(&b'/')
            && original != self.home()
            && is_within(original, self.home())
            && !is_within(original, self.root.as_bytes())
    }

    /// Moves trashed item `name` back to its folder, under a name free there: where it went;
    /// refused for a record that points outside the home folder ([`Self::may_restore_to`]).
    #[allow(clippy::result_large_err)]
    pub fn restore(&self, files: &mut impl Files, name: &[u8]) -> Result<Path, RestoreError> {
        let record = self.record(files, name).ok_or(RestoreError::NoRecord)?;
        if !self.may_restore_to(record.original.as_bytes()) {
            return Err(RestoreError::OutsideHome);
        }
        let item = self
            .item(name)
            .ok_or(RestoreError::Failed(Errno::ENAMETOOLONG))?;
        let status = files
            .status(item.as_bytes())
            .map_err(RestoreError::Failed)?;
        let folder = Path::from(parent(record.original.as_bytes()))
            .ok_or(RestoreError::Failed(Errno::ENAMETOOLONG))?;
        match files.status_following(folder.as_bytes()) {
            Ok(found) if found.kind == Kind::Directory => {}
            _ => return Err(RestoreError::FolderGone(folder)),
        }
        let original = record.original.file_name();
        let is_folder = status.kind == Kind::Directory;
        let target = free_path(files, folder.as_bytes(), |number| {
            copy_name(original, is_folder, number)
        })
        .map_err(RestoreError::Failed)?;
        files
            .rename(item.as_bytes(), target.as_bytes())
            .map_err(RestoreError::Failed)?;
        self.forget(files, name);
        Ok(target)
    }

    /// Removes the record of trashed item `name`, once the item itself is gone.
    pub fn forget(&self, files: &mut impl Files, name: &[u8]) {
        if let Some(path) = self.record_path(name) {
            let _ = files.remove_file(path.as_bytes());
        }
    }

    /// Deletes trashed item `name` and its record for good.
    pub fn delete(&self, files: &mut impl Files, name: &[u8]) -> Result<(), Errno> {
        let item = self.item(name).ok_or(Errno::ENAMETOOLONG)?;
        remove_tree(files, item.as_bytes())?;
        self.forget(files, name);
        Ok(())
    }

    /// Deletes everything in the Trash for good: how many items were there.
    pub fn empty(&self, files: &mut impl Files) -> Result<usize, Errno> {
        let mut name = [0u8; 256];
        let mut count = 0;
        for (folder, counted) in [(self.items(), true), (self.records(), false)] {
            let Some(folder) = folder else {
                continue;
            };
            loop {
                let found = match files.entry(folder.as_bytes(), 0, &mut name) {
                    Ok(found) => found,
                    Err(Errno::ENOENT) => break,
                    Err(error) => return Err(error),
                };
                let Some(length) = found else {
                    break;
                };
                let path = folder.join(&name[..length]).ok_or(Errno::ENAMETOOLONG)?;
                remove_tree(files, path.as_bytes())?;
                count += usize::from(counted);
            }
        }
        Ok(count)
    }

    /// How many items it holds.
    pub fn count(&self, files: &mut impl Files) -> usize {
        let mut count = 0;
        let _ = self.for_each(files, |_, _| count += 1);
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryFiles, Node};

    const HOME: &str = "/home/root";

    fn home() -> MemoryFiles {
        let mut files = MemoryFiles::new();
        files
            .file("/home/root/Work/notes old.txt", b"notes", 10)
            .file("/home/root/notes old.txt", b"other", 11)
            .folder("/home/root/Work/Deep/er")
            .file("/tmp/scratch", b"s", 1)
            .device("/tmp", 2);
        files.now = 192;
        files
    }

    fn text(path: &Path) -> String {
        String::from_utf8_lossy(path.as_bytes()).into_owned()
    }

    #[test]
    fn a_trashed_file_has_a_record_of_where_it_was_and_when() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        let name = trash
            .put(&mut files, b"/home/root/Work/notes old.txt", 192)
            .unwrap();
        assert_eq!(name.as_bytes(), b"notes old.txt");
        assert_eq!(files.node("/home/root/Work/notes old.txt"), None);
        assert_eq!(
            files.contents("/home/root/.local/share/Trash/files/notes old.txt"),
            Some(&b"notes"[..])
        );
        assert_eq!(
            files.contents("/home/root/.local/share/Trash/info/notes old.txt.trashinfo"),
            Some(
                &b"[Trash Info]\nPath=/home/root/Work/notes%20old.txt\n\
                   DeletionDate=1970-01-01T00:03:12\n"[..]
            )
        );
        let record = trash.record(&mut files, b"notes old.txt").unwrap();
        assert_eq!(text(&record.original), "/home/root/Work/notes old.txt");
        assert_eq!(record.deleted, Some(192));
    }

    #[test]
    fn a_taken_name_in_the_trash_gets_a_number() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        trash
            .put(&mut files, b"/home/root/Work/notes old.txt", 1)
            .unwrap();
        let second = trash
            .put(&mut files, b"/home/root/notes old.txt", 2)
            .unwrap();
        assert_eq!(second.as_bytes(), b"notes old.txt.2");
        let mut seen = Vec::new();
        trash
            .for_each(&mut files, |name, record| {
                seen.push((
                    String::from_utf8_lossy(name).into_owned(),
                    record.map(|record| text(&record.original)),
                ))
            })
            .unwrap();
        assert_eq!(
            seen,
            [
                (
                    "notes old.txt".to_owned(),
                    Some("/home/root/Work/notes old.txt".to_owned())
                ),
                (
                    "notes old.txt.2".to_owned(),
                    Some("/home/root/notes old.txt".to_owned())
                ),
            ]
        );
        assert_eq!(trash.count(&mut files), 2);
    }

    #[test]
    fn other_file_systems_and_the_trash_itself_cannot_be_trashed() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        assert_eq!(trash.put(&mut files, b"/tmp/scratch", 1), Err(Errno::EXDEV));
        assert_eq!(files.contents("/tmp/scratch"), Some(&b"s"[..]));
        assert!(
            files
                .paths_under("/home/root/.local/share/Trash/info")
                .is_empty()
        );
        assert_eq!(
            trash.put(&mut files, b"/home/root/.local/share/Trash", 1),
            Err(Errno::EINVAL)
        );
        assert_eq!(trash.put(&mut files, b"/home/root", 1), Err(Errno::EINVAL));
        assert_eq!(
            trash.put(&mut files, b"/home/root/none", 1),
            Err(Errno::ENOENT)
        );
    }

    #[test]
    fn restore_puts_an_item_back_under_a_free_name_or_says_its_folder_is_gone() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        trash.put(&mut files, b"/home/root/Work/Deep", 1).unwrap();
        files.folder("/home/root/Work/Deep");
        let back = trash.restore(&mut files, b"Deep").unwrap();
        assert_eq!(text(&back), "/home/root/Work/Deep copy");
        assert_eq!(
            files.node("/home/root/Work/Deep copy/er"),
            Some(&Node::Folder)
        );
        assert_eq!(trash.count(&mut files), 0);
        trash
            .put(&mut files, b"/home/root/Work/notes old.txt", 1)
            .unwrap();
        trash.put(&mut files, b"/home/root/Work", 2).unwrap();
        assert_eq!(
            trash.restore(&mut files, b"notes old.txt"),
            Err(RestoreError::FolderGone(
                Path::from(b"/home/root/Work").unwrap()
            ))
        );
        assert_eq!(
            trash.restore(&mut files, b"nothing"),
            Err(RestoreError::NoRecord)
        );
    }

    #[test]
    fn a_forged_record_pointing_outside_the_home_is_not_restored() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        assert_eq!(trash.home(), b"/home/root");
        trash
            .put(&mut files, b"/home/root/notes old.txt", 1)
            .unwrap();
        for forged in [
            "/usr/x",
            "/home/root/../../etc/x",
            "/home/root/./x",
            "/home/other/x",
            "/home/root",
            "/home/root/.local/share/Trash/files/y",
            "/home/rootless/x",
        ] {
            let record = format!("[Trash Info]\nPath={forged}\nDeletionDate=1970-01-01T00:00:01\n");
            files.file(
                "/home/root/.local/share/Trash/info/notes old.txt.trashinfo",
                record.as_bytes(),
                1,
            );
            assert_eq!(
                trash.restore(&mut files, b"notes old.txt"),
                Err(RestoreError::OutsideHome),
                "{forged}"
            );
            assert_eq!(
                files.contents("/home/root/.local/share/Trash/files/notes old.txt"),
                Some(&b"other"[..])
            );
        }
        assert!(trash.may_restore_to(b"/home/root/notes old.txt"));
        assert!(trash.may_restore_to(b"/home/root/Work/x"));
    }

    #[test]
    fn deleting_and_emptying_leave_nothing() {
        let mut files = home();
        let trash = Trash::of_home(HOME.as_bytes()).unwrap();
        trash.put(&mut files, b"/home/root/Work", 1).unwrap();
        trash
            .put(&mut files, b"/home/root/notes old.txt", 1)
            .unwrap();
        trash.delete(&mut files, b"notes old.txt").unwrap();
        assert_eq!(trash.count(&mut files), 1);
        assert_eq!(trash.empty(&mut files), Ok(1));
        assert!(
            files
                .paths_under("/home/root/.local/share/Trash/files")
                .is_empty()
        );
        assert!(
            files
                .paths_under("/home/root/.local/share/Trash/info")
                .is_empty()
        );
        assert_eq!(trash.empty(&mut files), Ok(0));
    }

    #[test]
    fn records_read_back_escapes_and_refuse_garbage() {
        let record = record_of(b"[Trash Info]\r\nPath=/a%2Fb%25c\r\nDeletionDate=bad\r\n").unwrap();
        assert_eq!(record.original.as_bytes(), b"/a/b%c");
        assert_eq!(record.deleted, None);
        assert_eq!(record_of(b"[Other]\nPath=/a\n"), None);
        assert_eq!(record_of(b"[Trash Info]\nPath=relative\n"), None);
        assert_eq!(record_of(b"[Trash Info]\nPath=/a%G0\n"), None);
    }
}
