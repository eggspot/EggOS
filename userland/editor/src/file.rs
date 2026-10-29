//! Reading a document from a file and saving it (docs/ux/text-editor-v0.3.md sections 5 and
//! 8): what a file must be to open (a regular file of at most the document's room, UTF-8 with
//! no NUL, checked in a pass of its own before the document is touched, so a refused file
//! leaves the open document as it was) and how a save replaces it (a new file beside it, synced,
//! then renamed over the old one, so a failed save never leaves half a file).

use crate::document::{Document, NotText, TEXT_CAPACITY};
use core::fmt::Write;
use eggos_app::files::{self, Files, Handle, Kind, Path};
use eggos_app::messages::Line;
use eggos_runtime::errno::Errno;

const CHECK_CHUNK: usize = 4096;
const FILE_MODE: u32 = 0o644;
const SAVING_PREFIX: &[u8] = b".";
const SAVING_SUFFIX: &[u8] = b".saving";
const SAVING_ATTEMPTS: usize = 16;

/// A streaming check that bytes are UTF-8 with no NUL, fed in pieces of any size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextCheck {
    needed: u8,
    low: u8,
    high: u8,
    valid: bool,
}

impl Default for TextCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl TextCheck {
    /// A check that has seen nothing.
    pub const fn new() -> Self {
        Self {
            needed: 0,
            low: 0x80,
            high: 0xbf,
            valid: true,
        }
    }

    /// Checks the next `bytes`.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if !self.valid {
                return;
            }
            if self.needed > 0 {
                if byte < self.low || byte > self.high {
                    self.valid = false;
                    return;
                }
                self.needed -= 1;
                self.low = 0x80;
                self.high = 0xbf;
                continue;
            }
            let (needed, low, high) = match byte {
                0x01..=0x7f => (0, 0x80, 0xbf),
                0xc2..=0xdf => (1, 0x80, 0xbf),
                0xe0 => (2, 0xa0, 0xbf),
                0xe1..=0xec | 0xee..=0xef => (2, 0x80, 0xbf),
                0xed => (2, 0x80, 0x9f),
                0xf0 => (3, 0x90, 0xbf),
                0xf1..=0xf3 => (3, 0x80, 0xbf),
                0xf4 => (3, 0x80, 0x8f),
                _ => {
                    self.valid = false;
                    return;
                }
            };
            self.needed = needed;
            self.low = low;
            self.high = high;
        }
    }

    /// Whether everything fed was text and ended on a whole character.
    pub fn finish(&self) -> bool {
        self.valid && self.needed == 0
    }
}

/// Why a file did not open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenProblem {
    /// Nothing is at the path.
    Missing,
    /// It is a folder.
    Folder,
    /// It is not UTF-8 text, or holds a NUL, or is not a regular file.
    NotText,
    /// It is larger than the document's room.
    TooLarge,
    /// Reading it failed.
    Failed(Errno),
}

/// Why a save failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveProblem {
    /// The folder to save in does not exist.
    MissingFolder,
    /// The place is read-only or not the user's (`EROFS`, `EACCES`, `EPERM`).
    ReadOnly,
    /// Something else, with its error.
    Failed(Errno),
}

fn errno_text(error: Errno) -> &'static str {
    core::str::from_utf8(error.message()).unwrap_or("error")
}

/// What the editor and `edit` say when the file named `name` does not open.
pub fn open_message(name: &str, problem: OpenProblem) -> Line {
    let mut line = Line::new();
    let _ = match problem {
        OpenProblem::Missing => write!(line, "{name} does not exist."),
        OpenProblem::Folder => write!(line, "{name} is a folder."),
        OpenProblem::NotText => write!(line, "{name} is not a text file."),
        OpenProblem::TooLarge => write!(
            line,
            "{name} is too large to open (over {} MB).",
            TEXT_CAPACITY >> 20
        ),
        OpenProblem::Failed(error) => {
            write!(line, "Could not open {name} ({}).", errno_text(error))
        }
    };
    line
}

/// What the editor says when saving at `path` failed.
pub fn save_message(path: &[u8], problem: SaveProblem) -> Line {
    let folder = core::str::from_utf8(folder_of(path)).unwrap_or("?");
    let name = core::str::from_utf8(files::file_name(path)).unwrap_or("?");
    let mut line = Line::new();
    let _ = match problem {
        SaveProblem::MissingFolder => write!(line, "{folder} does not exist."),
        SaveProblem::ReadOnly => write!(line, "Cannot save here: {folder} is read-only."),
        SaveProblem::Failed(error) => {
            write!(line, "Could not save {name} ({}).", errno_text(error))
        }
    };
    line
}

/// Whether `path` can be opened in a document of `capacity` bytes: a regular file of at most
/// that size whose every byte is text. The file is read once, in pieces; nothing is kept.
pub fn check(files: &mut impl Files, path: &[u8], capacity: usize) -> Result<u64, OpenProblem> {
    let status = match files.status(path) {
        Ok(status) => status,
        Err(Errno::ENOENT) => return Err(OpenProblem::Missing),
        Err(error) => return Err(OpenProblem::Failed(error)),
    };
    match status.kind {
        Kind::Directory => return Err(OpenProblem::Folder),
        Kind::Other => return Err(OpenProblem::NotText),
        Kind::File => {}
    }
    if status.size > capacity as u64 {
        return Err(OpenProblem::TooLarge);
    }
    let file = files.open(path).map_err(OpenProblem::Failed)?;
    let mut check = TextCheck::new();
    let mut chunk = [0u8; CHECK_CHUNK];
    let mut total = 0u64;
    let read = loop {
        match files.read(file, &mut chunk) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                total += count as u64;
                check.feed(&chunk[..count]);
                if total > capacity as u64 {
                    break Err(OpenProblem::TooLarge);
                }
            }
            Err(error) => break Err(OpenProblem::Failed(error)),
        }
    };
    files.close(file);
    read?;
    match check.finish() {
        true => Ok(total),
        false => Err(OpenProblem::NotText),
    }
}

/// Reads the file at `path` into `document`, after [`check`] says it is text that fits.
pub fn load(
    files: &mut impl Files,
    path: &[u8],
    document: &mut Document<'_>,
) -> Result<(), OpenProblem> {
    check(files, path, document.capacity())?;
    read(files, path, document)
}

/// Reads the file at `path` into `document`, which is lost whatever happens: on an error the
/// document is left empty. [`check`] first, so a file that cannot open costs nothing.
pub fn read(
    files: &mut impl Files,
    path: &[u8],
    document: &mut Document<'_>,
) -> Result<(), OpenProblem> {
    let file = match files.open(path) {
        Ok(file) => file,
        Err(error) => {
            document.clear();
            return Err(OpenProblem::Failed(error));
        }
    };
    let room = document.room();
    let mut filled = 0;
    let read = loop {
        if filled == room.len() {
            let mut probe = [0u8; 1];
            break match files.read(file, &mut probe) {
                Ok(0) => Ok(()),
                Ok(_) => Err(OpenProblem::TooLarge),
                Err(error) => Err(OpenProblem::Failed(error)),
            };
        }
        match files.read(file, &mut room[filled..]) {
            Ok(0) => break Ok(()),
            Ok(count) => filled += count,
            Err(error) => break Err(OpenProblem::Failed(error)),
        }
    };
    files.close(file);
    if let Err(problem) = read {
        document.clear();
        return Err(problem);
    }
    document
        .load(filled)
        .map_err(|NotText| OpenProblem::NotText)
}

/// The folder part of `path`: everything before its last `/`, or `/`.
pub fn folder_of(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/",
        Some(slash) => &path[..slash],
    }
}

fn saving_path(path: &[u8], attempt: usize) -> Option<Path> {
    let folder = folder_of(path);
    let mut number = Line::new();
    if attempt > 0 {
        let _ = write!(number, ".{attempt}");
    }
    let mut saving = Path::new();
    let fits = (folder == b"/" || saving.append(folder))
        && saving.append(b"/")
        && saving.append(SAVING_PREFIX)
        && saving.append(files::file_name(path))
        && saving.append(number.as_str().as_bytes())
        && saving.append(SAVING_SUFFIX);
    fits.then_some(saving)
}

fn save_problem(error: Errno) -> SaveProblem {
    match error {
        Errno::EROFS | Errno::EACCES | Errno::EPERM => SaveProblem::ReadOnly,
        Errno::ENOENT => SaveProblem::MissingFolder,
        error => SaveProblem::Failed(error),
    }
}

fn create_saving_file(
    files: &mut impl Files,
    path: &[u8],
    permissions: u32,
) -> Result<(Path, Handle), SaveProblem> {
    for attempt in 0..SAVING_ATTEMPTS {
        let saving = saving_path(path, attempt).ok_or(SaveProblem::Failed(Errno::ENAMETOOLONG))?;
        match files.create(saving.as_bytes(), permissions) {
            Ok(file) => return Ok((saving, file)),
            Err(Errno::EEXIST) => {}
            Err(error) => return Err(save_problem(error)),
        }
    }
    Err(SaveProblem::Failed(Errno::EEXIST))
}

/// Saves `document` at `path`: written to `.<name>.saving` beside it (`.<name>.<n>.saving`
/// when a file of that name is already there, which is left alone), synced, then renamed over
/// `path`, keeping an existing file's permissions (else `0o644`).
pub fn save(
    files: &mut impl Files,
    path: &[u8],
    document: &Document<'_>,
) -> Result<(), SaveProblem> {
    let folder = folder_of(path);
    match files.status(folder) {
        Ok(status) if status.kind == Kind::Directory => {}
        Ok(_) | Err(Errno::ENOENT | Errno::ENOTDIR) => return Err(SaveProblem::MissingFolder),
        Err(error) => return Err(save_problem(error)),
    }
    let permissions = match files.status(path) {
        Ok(status) if status.kind == Kind::File => status.permissions,
        Ok(_) => return Err(SaveProblem::Failed(Errno::EISDIR)),
        Err(_) => FILE_MODE,
    };
    let (saving, file) = create_saving_file(files, path, permissions)?;
    let written = document
        .encode(|piece| files.write(file, piece))
        .and_then(|()| files.sync_file(file));
    files.close(file);
    let renamed = written.and_then(|()| files.rename(saving.as_bytes(), path));
    if let Err(error) = renamed {
        let _ = files.remove_file(saving.as_bytes());
        return Err(save_problem(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::TEXT_CAPACITY;
    use eggos_files::memory::MemoryFiles;

    fn disk() -> MemoryFiles {
        let mut disk = MemoryFiles::new();
        disk.folder("/home")
            .folder("/home/root")
            .folder("/media/host");
        disk.read_only("/media/host");
        disk
    }

    fn room(bytes: usize) -> (Vec<u8>, Vec<u32>) {
        (vec![0; bytes], vec![0; bytes + 1])
    }

    #[test]
    fn the_check_takes_utf_8_in_any_pieces_and_refuses_the_rest() {
        let text = "aé€😀".as_bytes();
        for split in 0..=text.len() {
            let mut check = TextCheck::new();
            check.feed(&text[..split]);
            check.feed(&text[split..]);
            assert!(check.finish(), "{split}");
        }
        for wrong in [
            &b"\0"[..],
            b"\xc0\x80",
            b"\xe0\x80\x80",
            b"\xf4\x90\x80\x80",
            b"\xf5",
            b"\x80",
            b"\xe2\x82",
        ] {
            let mut check = TextCheck::new();
            check.feed(wrong);
            assert!(!check.finish(), "{wrong:?}");
        }
    }

    #[test]
    fn files_open_by_what_they_hold() {
        let mut disk = disk();
        disk.file("/home/root/notes.txt", b"hello\n", 0)
            .file("/home/root/photo.png", b"\x89PNG\0", 0)
            .folder("/home/root/work")
            .file("/home/root/marked.txt", b"\xef\xbb\xbfhi", 0);
        let (mut text, mut starts) = room(64);
        let mut document = Document::new(&mut text, &mut starts);
        assert_eq!(
            load(&mut disk, b"/home/root/notes.txt", &mut document),
            Ok(())
        );
        assert_eq!(document.bytes(), b"hello\n");
        assert_eq!(
            load(&mut disk, b"/home/root/photo.png", &mut document),
            Err(OpenProblem::NotText)
        );
        assert_eq!(document.bytes(), b"hello\n");
        assert_eq!(
            load(&mut disk, b"/home/root/work", &mut document),
            Err(OpenProblem::Folder)
        );
        assert_eq!(
            load(&mut disk, b"/home/root/gone.txt", &mut document),
            Err(OpenProblem::Missing)
        );
        assert_eq!(
            load(&mut disk, b"/home/root/marked.txt", &mut document),
            Ok(())
        );
        assert_eq!(document.bytes(), b"hi");
        assert!(document.has_byte_order_mark());
        assert_eq!(disk.open_count(), 0);
    }

    #[test]
    fn four_mebibytes_open_and_one_byte_more_does_not() {
        let mut disk = disk();
        disk.file("/home/root/full.txt", &vec![b'a'; TEXT_CAPACITY], 0)
            .file("/home/root/over.txt", &vec![b'a'; TEXT_CAPACITY + 1], 0);
        let (mut text, mut starts) = room(TEXT_CAPACITY);
        let mut document = Document::new(&mut text, &mut starts);
        assert_eq!(
            load(&mut disk, b"/home/root/full.txt", &mut document),
            Ok(())
        );
        assert_eq!(document.len(), TEXT_CAPACITY);
        assert_eq!(
            load(&mut disk, b"/home/root/over.txt", &mut document),
            Err(OpenProblem::TooLarge)
        );
        assert_eq!(document.len(), TEXT_CAPACITY);
    }

    #[test]
    fn a_save_replaces_the_file_and_keeps_its_permissions() {
        let mut disk = disk();
        disk.file("/home/root/notes.txt", b"old\r\n", 0);
        let (mut text, mut starts) = room(64);
        let mut document = Document::new(&mut text, &mut starts);
        load(&mut disk, b"/home/root/notes.txt", &mut document).unwrap();
        document.insert(0, b"new\n").unwrap();
        assert_eq!(save(&mut disk, b"/home/root/notes.txt", &document), Ok(()));
        assert_eq!(
            disk.contents("/home/root/notes.txt"),
            Some(&b"new\r\nold\r\n"[..])
        );
        assert_eq!(disk.contents("/home/root/.notes.txt.saving"), None);
        assert_eq!(save(&mut disk, b"/home/root/new.txt", &document), Ok(()));
        assert_eq!(
            disk.contents("/home/root/new.txt"),
            Some(&b"new\r\nold\r\n"[..])
        );
        assert_eq!(disk.open_count(), 0);
    }

    #[test]
    fn a_save_leaves_a_file_named_like_its_temporary_one_alone() {
        let mut disk = disk();
        disk.file("/home/root/.notes.txt.saving", b"mine", 0);
        let (mut text, mut starts) = room(64);
        let mut document = Document::new(&mut text, &mut starts);
        document.insert(0, b"new\n").unwrap();
        assert_eq!(save(&mut disk, b"/home/root/notes.txt", &document), Ok(()));
        assert_eq!(disk.contents("/home/root/notes.txt"), Some(&b"new\n"[..]));
        assert_eq!(
            disk.contents("/home/root/.notes.txt.saving"),
            Some(&b"mine"[..])
        );
        assert_eq!(disk.contents("/home/root/.notes.txt.1.saving"), None);
        assert_eq!(disk.open_count(), 0);
    }

    #[test]
    fn a_save_says_why_it_failed() {
        let mut disk = disk();
        let (mut text, mut starts) = room(64);
        let document = Document::new(&mut text, &mut starts);
        assert_eq!(
            save(&mut disk, b"/media/host/notes.txt", &document),
            Err(SaveProblem::ReadOnly)
        );
        assert_eq!(
            save(&mut disk, b"/home/root/missing/notes.txt", &document),
            Err(SaveProblem::MissingFolder)
        );
        disk.space = Some(0);
        let (mut text, mut starts) = room(64);
        let mut full = Document::new(&mut text, &mut starts);
        full.insert(0, b"text").unwrap();
        assert_eq!(
            save(&mut disk, b"/home/root/notes.txt", &full),
            Err(SaveProblem::Failed(Errno::ENOSPC))
        );
        assert_eq!(disk.contents("/home/root/.notes.txt.saving"), None);
        assert_eq!(disk.contents("/home/root/notes.txt"), None);
    }

    #[test]
    fn the_folder_of_a_path_is_before_its_last_slash() {
        assert_eq!(folder_of(b"/home/root/a.txt"), b"/home/root");
        assert_eq!(folder_of(b"/a.txt"), b"/");
        assert_eq!(folder_of(b"a.txt"), b"/");
    }
}
