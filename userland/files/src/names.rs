//! File names as Files and the `trash` command make them (docs/ux/desktop-v0.3-gui.md section
//! 8.1 item 8, docs/ux/files-links-v0.3.md): a name that never overwrites (`notes copy.txt`,
//! `notes copy 2.txt`, `New folder 2`, `notes link.txt`), what a name may hold, the text Files
//! shows for a name that is not UTF-8, and where a symbolic link leads.

use core::fmt::Write;
use eggos_app::files::{self, Files, PATH_CAPACITY, Path};
use eggos_runtime::errno::Errno;

/// The longest file name, in bytes.
pub const NAME_CAPACITY: usize = 255;
/// The name Files gives a new folder.
pub const NEW_FOLDER: &[u8] = b"New folder";
/// The most symbolic links one path is followed through, as Linux allows.
pub const MOST_LINKS: usize = 40;
const COPY: &[u8] = b" copy";
const LINK: &[u8] = b" link";
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

/// The `number`th name a link to `name` tries: `<stem> link<.ext>` first, then `<stem> link
/// 2<.ext>` and so on (a folder's whole name is its stem).
pub fn link_name(name: &[u8], folder: bool, number: usize) -> Option<Name> {
    let (stem, extension) = name.split_at(extension_start(name, folder));
    let mut made = Name::from(stem)?;
    if !made.push(LINK) {
        return None;
    }
    if number > 0 {
        write!(made, " {}", number + 1).ok()?;
    }
    made.push(extension).then_some(made)
}

/// Where the target text `target` of a link in `folder` leads, one level and [`normalized`]:
/// relative text from `folder`, absolute text as it is. `None` when it does not fit.
pub fn link_target_path(folder: &[u8], target: &[u8]) -> Option<Path> {
    if target.first() == Some(&b'/') {
        return normalized(target);
    }
    let mut path = Path::from(folder)?;
    if path.as_bytes() != b"/" && !path.append(b"/") {
        return None;
    }
    if !path.append(target) {
        return None;
    }
    normalized(path.as_bytes())
}

/// Where the absolute `path` really is: every symbolic link in it replaced by where it leads,
/// [`normalized`]. `ENOENT` when part of it is missing, `ELOOP` past [`MOST_LINKS`] links.
pub fn resolved(files: &mut impl Files, path: &[u8]) -> Result<Path, Errno> {
    let mut path = normalized(path).ok_or(Errno::ENAMETOOLONG)?;
    let mut target = [0u8; PATH_CAPACITY];
    let mut links = 0;
    let mut end = 0;
    while end < path.as_bytes().len() {
        let bytes = path.as_bytes();
        end = bytes[end + 1..]
            .iter()
            .position(|&byte| byte == b'/')
            .map_or(bytes.len(), |slash| end + 1 + slash);
        let prefix = &bytes[..end];
        if files.status(prefix)?.kind != files::Kind::Link {
            continue;
        }
        links += 1;
        if links > MOST_LINKS {
            return Err(Errno::ELOOP);
        }
        let length = files.read_link(prefix, &mut target)?;
        let mut next =
            link_target_path(parent(prefix), &target[..length]).ok_or(Errno::ENAMETOOLONG)?;
        if !next.append(&bytes[end..]) {
            return Err(Errno::ENAMETOOLONG);
        }
        path = normalized(next.as_bytes()).ok_or(Errno::ENAMETOOLONG)?;
        end = 0;
    }
    Ok(path)
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

/// `path` with `//` collapsed, `.` dropped, `..` taking the component before it away and no
/// `/` at its end, by its text alone as `cd` keeps `$PWD` ([`resolved`] says where it really
/// is). A relative path stays relative (a `..` it cannot resolve is kept); `None` when it does
/// not fit.
pub fn normalized(path: &[u8]) -> Option<Path> {
    let absolute = path.first() == Some(&b'/');
    let mut out = Path::new();
    if absolute && !out.append(b"/") {
        return None;
    }
    let mut kept = 0usize;
    for part in path.split(|&byte| byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." if kept > 0 => {
                let bytes = out.as_bytes();
                let cut = bytes.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
                let cut = if absolute && cut == 0 { 1 } else { cut };
                out = Path::from(&bytes[..cut])?;
                kept -= 1;
            }
            b".." if absolute => {}
            part => {
                let room = out.as_bytes().last().is_some_and(|&byte| byte != b'/');
                if (room && !out.append(b"/")) || !out.append(part) {
                    return None;
                }
                if part != b".." {
                    kept += 1;
                }
            }
        }
    }
    if out.is_empty() && !absolute {
        out.append(b".");
    }
    Some(out)
}

/// Whether `path` names `folder` or something inside it, once both are [`normalized`].
pub fn is_inside(path: &[u8], folder: &[u8]) -> bool {
    match (normalized(path), normalized(folder)) {
        (Some(path), Some(folder)) => is_within(path.as_bytes(), folder.as_bytes()),
        _ => false,
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
        for piece in [
            chunk.valid().as_bytes(),
            &b"?"[..chunk.invalid().len().min(1)],
        ] {
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
        assert_eq!(
            text(copy_name(b"archive.tar.gz", false, 3)),
            "archive.tar copy 3.gz"
        );
        assert!(copy_name(&[b'x'; NAME_CAPACITY], false, 1).is_none());
    }

    #[test]
    fn links_are_named_before_the_extension_and_counted_from_two() {
        assert_eq!(text(link_name(b"report.txt", false, 0)), "report link.txt");
        assert_eq!(
            text(link_name(b"report.txt", false, 1)),
            "report link 2.txt"
        );
        assert_eq!(text(link_name(b"Projects", true, 0)), "Projects link");
        assert_eq!(text(link_name(b"Paint.app", true, 2)), "Paint.app link 3");
        assert!(link_name(&[b'x'; NAME_CAPACITY], false, 0).is_none());
    }

    #[test]
    fn a_link_target_leads_from_the_links_folder_or_from_the_root() {
        let path = |folder: &[u8], target: &[u8]| {
            String::from_utf8(
                link_target_path(folder, target)
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            )
            .unwrap()
        };
        assert_eq!(path(b"/home/root", b"../notes.txt"), "/home/notes.txt");
        assert_eq!(path(b"/home/root", b"Projects/"), "/home/root/Projects");
        assert_eq!(path(b"/", b"tmp"), "/tmp");
        assert_eq!(path(b"/home/root", b"/etc/hostname"), "/etc/hostname");
    }

    #[test]
    fn a_path_is_resolved_through_every_link_in_it() {
        let mut files = crate::memory::MemoryFiles::new();
        files
            .file("/home/root/Projects/plan.txt", b"plan", 1)
            .link("/home/root/Desktop/work", "../Projects", 1)
            .link("/home/root/again", "Desktop/work/plan.txt", 1)
            .link("/home/root/loop", "loop", 1);
        let real = |files: &mut crate::memory::MemoryFiles, path: &[u8]| {
            resolved(files, path).map(|path| String::from_utf8(path.as_bytes().to_vec()).unwrap())
        };
        assert_eq!(
            real(&mut files, b"/home/root/again").unwrap(),
            "/home/root/Projects/plan.txt"
        );
        assert_eq!(
            real(&mut files, b"/home/root/Desktop/work/").unwrap(),
            "/home/root/Projects"
        );
        assert_eq!(real(&mut files, b"/").unwrap(), "/");
        assert_eq!(real(&mut files, b"/home/root/loop"), Err(Errno::ELOOP));
        assert_eq!(real(&mut files, b"/home/root/none/x"), Err(Errno::ENOENT));
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
    fn paths_are_normalized_without_touching_the_file_system() {
        let text = |path: &[u8]| String::from_utf8(normalized(path).unwrap().as_bytes().to_vec());
        assert_eq!(text(b"/home/root/./Work").unwrap(), "/home/root/Work");
        assert_eq!(text(b"/home//root/Work/").unwrap(), "/home/root/Work");
        assert_eq!(text(b"/home/root/a/../Work").unwrap(), "/home/root/Work");
        assert_eq!(text(b"/../..").unwrap(), "/");
        assert_eq!(text(b"/").unwrap(), "/");
        assert_eq!(text(b"Work/./x/..").unwrap(), "Work");
        assert_eq!(text(b"../a").unwrap(), "../a");
        assert_eq!(text(b"a/..").unwrap(), ".");
        assert!(is_inside(b"/home/root/./Work/backup", b"/home/root/Work"));
        assert!(is_inside(b"/home/root/Work", b"/home/root/x/../Work/"));
        assert!(!is_inside(b"/home/root/Workshop", b"/home/root/Work"));
    }

    #[test]
    fn names_that_are_not_utf8_show_a_question_mark_per_bad_run() {
        let mut out = [0u8; NAME_CAPACITY * 2];
        assert_eq!(display(b"caf\xc3\xa9", &mut out), "café");
        assert_eq!(display(b"a\xffb\xfe\xfd", &mut out), "a?b??");
    }
}
