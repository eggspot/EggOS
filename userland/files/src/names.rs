//! File names as Files and the `trash` command make them (docs/ux/desktop-v0.3-gui.md section
//! 8.1 item 8): a name that never overwrites (`notes copy.txt`, `notes copy 2.txt`, `New folder
//! 2`), what a name may hold, and the text Files shows for a name that is not UTF-8.

use core::fmt::Write;
use eggos_app::files::{Files, Path};
use eggos_runtime::errno::Errno;

/// The longest file name, in bytes.
pub const NAME_CAPACITY: usize = 255;
/// The name Files gives a new folder.
pub const NEW_FOLDER: &[u8] = b"New folder";
const COPY: &[u8] = b" copy";
const MOST_TRIES: usize = 10_000;

/// A file name of at most [`NAME_CAPACITY`] bytes.
#[derive(Clone, Copy)]
pub struct Name {
    bytes: [u8; NAME_CAPACITY],
    length: usize,
}

impl core::fmt::Debug for Name {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Name {}

impl Default for Name {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Name {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        match self.push(text.as_bytes()) {
            true => Ok(()),
            false => Err(core::fmt::Error),
        }
    }
}

impl Name {
    /// The empty name.
    pub const fn new() -> Self {
        Self {
            bytes: [0; NAME_CAPACITY],
            length: 0,
        }
    }

    /// `bytes` as a name; `None` when it is too long.
    pub fn from(bytes: &[u8]) -> Option<Self> {
        let mut name = Self::new();
        name.push(bytes).then_some(name)
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Adds `bytes` at the end; `false`, and nothing added, when they do not fit.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        let end = self.length + bytes.len();
        if end > NAME_CAPACITY {
            return false;
        }
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
        true
    }
}

/// Whether `name` may name a file: not empty, not `.` or `..`, no `/` or NUL, at most
/// [`NAME_CAPACITY`] bytes.
pub fn is_valid(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= NAME_CAPACITY
        && !matches!(name, b"." | b"..")
        && !name.iter().any(|&byte| byte == b'/' || byte == 0)
}

/// Where `name`'s extension starts: its last `.` when that is not its first byte; its length
/// when it has none. A folder's whole name is its stem (`folder`).
pub fn extension_start(name: &[u8], folder: bool) -> usize {
    match folder {
        true => name.len(),
        false => match name.iter().rposition(|&byte| byte == b'.') {
            Some(0) | None => name.len(),
            Some(dot) => dot,
        },
    }
}

/// The `number`th name a copy of `name` tries: `name` itself first, then `<stem> copy<.ext>`,
/// then `<stem> copy 2<.ext>` and so on.
pub fn copy_name(name: &[u8], folder: bool, number: usize) -> Option<Name> {
    if number == 0 {
        return Name::from(name);
    }
    let (stem, extension) = name.split_at(extension_start(name, folder));
    let mut made = Name::from(stem)?;
    if !made.push(COPY) {
        return None;
    }
    if number > 1 {
        write!(made, " {number}").ok()?;
    }
    made.push(extension).then_some(made)
}

/// The `number`th name a new folder tries: `New folder`, then `New folder 2` and so on.
pub fn new_folder_name(number: usize) -> Name {
    let mut made = Name::from(NEW_FOLDER).unwrap_or_default();
    if number > 0 {
        let _ = write!(made, " {}", number + 1);
    }
    made
}

/// The first of `candidate(0)`, `candidate(1)`, ... that names nothing in `folder`: its path.
pub fn free_path(
    files: &mut impl Files,
    folder: &[u8],
    mut candidate: impl FnMut(usize) -> Option<Name>,
) -> Result<Path, Errno> {
    let folder = Path::from(folder).ok_or(Errno::ENAMETOOLONG)?;
    for number in 0..MOST_TRIES {
        let name = candidate(number).ok_or(Errno::ENAMETOOLONG)?;
        let path = joined(&folder, name.as_bytes()).ok_or(Errno::ENAMETOOLONG)?;
        match files.status(path.as_bytes()) {
            Err(Errno::ENOENT) => return Ok(path),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Err(Errno::EEXIST)
}

/// `folder` and `name` joined by one `/` (`/` and `x` give `/x`).
pub fn joined(folder: &Path, name: &[u8]) -> Option<Path> {
    match folder.as_bytes() {
        b"/" => {
            let mut path = *folder;
            path.append(name).then_some(path)
        }
        _ => folder.join(name),
    }
}

/// The folder that holds `path`: `/` for `/x` and for `/`.
pub fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/",
        Some(slash) => &path[..slash],
    }
}

/// Whether `path` is `folder` or inside it.
pub fn is_within(path: &[u8], folder: &[u8]) -> bool {
    folder == b"/"
        || path == folder
        || (path.starts_with(folder) && path.get(folder.len()) == Some(&b'/'))
}

/// `bytes` as text: UTF-8 as it is, each byte that is not UTF-8 as `?`, into `out`.
pub fn display<'a>(bytes: &[u8], out: &'a mut [u8; NAME_CAPACITY * 2]) -> &'a str {
    let mut length = 0;
    for chunk in bytes.utf8_chunks() {
        for piece in [chunk.valid().as_bytes(), &b"?"[..chunk.invalid().len().min(1)]] {
            let end = (length + piece.len()).min(out.len());
            let taken = end - length;
            out[length..end].copy_from_slice(&piece[..taken]);
            length = end;
        }
    }
    let mut end = length;
    while core::str::from_utf8(&out[..end]).is_err() {
        end -= 1;
    }
    core::str::from_utf8(&out[..end]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: Option<Name>) -> String {
        String::from_utf8(name.unwrap().as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn copies_are_named_before_the_extension_and_counted_from_two() {
        assert_eq!(text(copy_name(b"notes.txt", false, 0)), "notes.txt");
        assert_eq!(text(copy_name(b"notes.txt", false, 1)), "notes copy.txt");
        assert_eq!(text(copy_name(b"notes.txt", false, 2)), "notes copy 2.txt");
        assert_eq!(text(copy_name(b".profile", false, 1)), ".profile copy");
        assert_eq!(text(copy_name(b"Paint.app", true, 1)), "Paint.app copy");
        assert_eq!(text(copy_name(b"archive.tar.gz", false, 3)), "archive.tar copy 3.gz");
        assert!(copy_name(&[b'x'; NAME_CAPACITY], false, 1).is_none());
    }

    #[test]
    fn new_folders_are_counted_from_two() {
        assert_eq!(text(Some(new_folder_name(0))), "New folder");
        assert_eq!(text(Some(new_folder_name(1))), "New folder 2");
        assert_eq!(text(Some(new_folder_name(9))), "New folder 10");
    }

    #[test]
    fn names_refuse_slashes_dots_and_nothing() {
        assert!(is_valid(b"Work"));
        assert!(!is_valid(b""));
        assert!(!is_valid(b".."));
        assert!(!is_valid(b"a/b"));
        assert!(!is_valid(b"a\0b"));
        assert!(!is_valid(&[b'x'; NAME_CAPACITY + 1]));
    }

    #[test]
    fn paths_join_and_split_at_the_root_too() {
        let root = Path::from(b"/").unwrap();
        assert_eq!(joined(&root, b"tmp").unwrap().as_bytes(), b"/tmp");
        let home = Path::from(b"/home/root").unwrap();
        assert_eq!(joined(&home, b"a").unwrap().as_bytes(), b"/home/root/a");
        assert_eq!(parent(b"/home/root/a"), b"/home/root");
        assert_eq!(parent(b"/tmp"), b"/");
        assert_eq!(parent(b"/"), b"/");
        assert!(is_within(b"/home/root/a", b"/home/root"));
        assert!(is_within(b"/home/root", b"/home/root"));
        assert!(!is_within(b"/home/rooted", b"/home/root"));
        assert!(is_within(b"/x", b"/"));
    }

    #[test]
    fn names_that_are_not_utf8_show_a_question_mark_per_bad_run() {
        let mut out = [0u8; NAME_CAPACITY * 2];
        assert_eq!(display(b"caf\xc3\xa9", &mut out), "café");
        assert_eq!(display(b"a\xffb\xfe\xfd", &mut out), "a?b??");
    }
}
