//! A folder's entries as Files lists them (docs/ux/desktop-v0.3-gui.md sections 8 and 8.1):
//! what each is, its size and time, folders first and each group by name ignoring case, or the
//! Trash's items newest first; names starting with `.` are hidden. The entries and their names
//! live in fixed arrays, so a folder with more than [`MAX_ENTRIES`] entries or longer names
//! than [`NAMES_CAPACITY`] bytes together shows the first that fit.

use crate::clock::Stamp;
use crate::kind::{self, Kind};
use crate::names::joined;
use crate::trash::Trash;
use core::cmp::Ordering;
use core::fmt::Write;
use eggos_app::files::{Files, Path};
use eggos_runtime::errno::Errno;

/// The most entries a listing holds.
pub const MAX_ENTRIES: usize = 512;
/// The most bytes of names a listing holds.
pub const NAMES_CAPACITY: usize = 32 * 1024;
const KIBIBYTE: u64 = 1024;

/// One listed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    name_start: u16,
    name_length: u8,
    /// What it is.
    pub kind: Kind,
    /// Its size in bytes (0 for a folder).
    pub size: u64,
    /// When it last changed or, in the Trash, when it was trashed: seconds since the epoch.
    pub time: u64,
}

const EMPTY: Entry = Entry {
    name_start: 0,
    name_length: 0,
    kind: Kind::Other,
    size: 0,
    time: 0,
};

/// A folder's listing. See the module documentation.
#[derive(Debug, Clone)]
pub struct Listing {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    names: [u8; NAMES_CAPACITY],
    used: usize,
    truncated: bool,
    error: Option<Errno>,
}

impl Default for Listing {
    fn default() -> Self {
        Self::new()
    }
}

fn ignoring_case(first: &[u8], second: &[u8]) -> Ordering {
    first
        .iter()
        .map(u8::to_ascii_lowercase)
        .cmp(second.iter().map(u8::to_ascii_lowercase))
        .then_with(|| first.cmp(second))
}

impl Listing {
    /// An empty listing.
    pub const fn new() -> Self {
        Self {
            entries: [EMPTY; MAX_ENTRIES],
            count: 0,
            names: [0; NAMES_CAPACITY],
            used: 0,
            truncated: false,
            error: None,
        }
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Entry `index`.
    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries[..self.count].get(index)
    }

    /// Entry `index`'s name.
    pub fn name(&self, index: usize) -> &[u8] {
        self.get(index).map_or(&[], |entry| {
            let start = usize::from(entry.name_start);
            &self.names[start..start + usize::from(entry.name_length)]
        })
    }

    /// The entry named `name`.
    pub fn find(&self, name: &[u8]) -> Option<usize> {
        (0..self.count).find(|&index| self.name(index) == name)
    }

    /// Whether entries were left out for want of room.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Why the folder could not be read, if it could not.
    pub fn error(&self) -> Option<Errno> {
        self.error
    }

    fn clear(&mut self) {
        self.count = 0;
        self.used = 0;
        self.truncated = false;
        self.error = None;
    }

    fn push(&mut self, name: &[u8], kind: Kind, size: u64, time: u64) {
        let end = self.used + name.len();
        if self.count == MAX_ENTRIES || end > NAMES_CAPACITY || name.len() > usize::from(u8::MAX)
        {
            self.truncated = true;
            return;
        }
        self.names[self.used..end].copy_from_slice(name);
        self.entries[self.count] = Entry {
            name_start: self.used as u16,
            name_length: name.len() as u8,
            kind,
            size,
            time,
        };
        self.used = end;
        self.count += 1;
    }

    /// Reads `folder` again, hiding names that start with `.`.
    pub fn read_folder(&mut self, files: &mut impl Files, folder: &Path) {
        self.clear();
        let mut name = [0u8; 256];
        let mut index = 0;
        loop {
            let length = match files.entry(folder.as_bytes(), index, &mut name) {
                Ok(Some(length)) => length,
                Ok(None) => break,
                Err(error) => {
                    self.error = Some(error);
                    break;
                }
            };
            index += 1;
            let name = &name[..length];
            if name.first() == Some(&b'.') {
                continue;
            }
            let Some(path) = joined(folder, name) else {
                continue;
            };
            let Ok(status) = files.status(path.as_bytes()) else {
                continue;
            };
            let kind = kind::of_entry(files, folder, name, status.kind);
            let size = if kind.is_folder() { 0 } else { status.size };
            self.push(name, kind, size, status.modified);
            if self.count == MAX_ENTRIES && self.truncated {
                break;
            }
        }
        let names = &self.names;
        self.entries[..self.count].sort_unstable_by(|first, second| {
            let name = |entry: &Entry| {
                let start = usize::from(entry.name_start);
                &names[start..start + usize::from(entry.name_length)]
            };
            second
                .kind
                .is_folder()
                .cmp(&first.kind.is_folder())
                .then_with(|| ignoring_case(name(first), name(second)))
        });
    }

    /// Reads `trash`'s items: their names, what they are, their sizes and when they were
    /// trashed, newest first.
    pub fn read_trash(&mut self, files: &mut impl Files, trash: &Trash) {
        self.clear();
        let Some(items) = trash.items() else {
            return;
        };
        let result = trash.for_each(files, |name, record| {
            let deleted = record.and_then(|record| record.deleted).unwrap_or(0);
            self.push(name, Kind::Other, 0, deleted);
        });
        if let Err(error) = result {
            self.error = Some(error);
        }
        let mut name = [0u8; 256];
        for index in 0..self.count {
            let length = self.name(index).len();
            name[..length].copy_from_slice(self.name(index));
            let name = &name[..length];
            let Some(path) = items.join(name) else {
                continue;
            };
            let Ok(status) = files.status(path.as_bytes()) else {
                continue;
            };
            let kind = kind::of_entry(files, &items, name, status.kind);
            self.entries[index].kind = kind;
            self.entries[index].size = if kind.is_folder() { 0 } else { status.size };
        }
        let names = &self.names;
        self.entries[..self.count].sort_unstable_by(|first, second| {
            let name = |entry: &Entry| {
                let start = usize::from(entry.name_start);
                &names[start..start + usize::from(entry.name_length)]
            };
            second
                .time
                .cmp(&first.time)
                .then_with(|| ignoring_case(name(first), name(second)))
        });
    }
}

/// A size as the list shows it: `512 B`, `4.2 KB`, `12 MB`, `1.5 GB`.
pub fn size_text(size: u64) -> Stamp {
    let mut stamp = Stamp::new();
    if size < KIBIBYTE {
        let _ = write!(stamp, "{size} B");
        return stamp;
    }
    let mut unit = KIBIBYTE;
    for name in ["KB", "MB", "GB", "TB"] {
        let next = unit.saturating_mul(KIBIBYTE);
        if size < next || name == "TB" {
            let tenths = size.saturating_mul(10) / unit;
            let _ = match tenths < 100 {
                true => write!(stamp, "{}.{} {name}", tenths / 10, tenths % 10),
                false => write!(stamp, "{} {name}", size / unit),
            };
            return stamp;
        }
        unit = next;
    }
    stamp
}

/// A count of bytes with thousands separated: `4,321 bytes`, `1 byte`.
pub fn bytes_text(size: u64) -> Stamp {
    let mut digits = [0u8; 20];
    let mut length = 0;
    let mut rest = size;
    loop {
        digits[length] = b'0' + (rest % 10) as u8;
        length += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let mut stamp = Stamp::new();
    for at in (0..length).rev() {
        let _ = stamp.write_char(char::from(digits[at]));
        if at > 0 && at % 3 == 0 {
            let _ = stamp.write_char(',');
        }
    }
    let _ = stamp.write_str(if size == 1 { " byte" } else { " bytes" });
    stamp
}

/// `N items`, or `1 item`.
pub fn items_text(count: usize) -> Stamp {
    let mut stamp = Stamp::new();
    let _ = match count {
        1 => write!(stamp, "1 item"),
        count => write!(stamp, "{count} items"),
    };
    stamp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryFiles;
    use eggos_app_manifest::Source;

    fn listed(listing: &Listing) -> Vec<(String, Kind)> {
        (0..listing.len())
            .map(|index| {
                (
                    String::from_utf8_lossy(listing.name(index)).into_owned(),
                    listing.get(index).unwrap().kind,
                )
            })
            .collect()
    }

    #[test]
    fn folders_come_first_then_files_each_by_name_ignoring_case() {
        let mut files = MemoryFiles::new();
        files
            .file("/home/root/zeta.txt", b"z", 3)
            .file("/home/root/Alpha.bin", b"\0\x01", 4)
            .file("/home/root/tool", b"\x7fELF\x02", 5)
            .file("/home/root/.hidden", b"", 5)
            .folder("/home/root/beta")
            .folder("/home/root/Paint.app")
            .folder("/home/root/Apps");
        let mut listing = Listing::new();
        listing.read_folder(&mut files, &Path::from(b"/home/root").unwrap());
        assert_eq!(
            listed(&listing),
            [
                ("Apps".to_owned(), Kind::Folder),
                ("beta".to_owned(), Kind::Folder),
                ("Paint.app".to_owned(), Kind::Bundle),
                ("Alpha.bin".to_owned(), Kind::File),
                ("tool".to_owned(), Kind::Program(Source::Linux)),
                ("zeta.txt".to_owned(), Kind::Text),
            ]
        );
        assert_eq!(listing.get(5).unwrap().time, 3);
        assert_eq!(listing.find(b"tool"), Some(4));
        assert_eq!(listing.error(), None);
        listing.read_folder(&mut files, &Path::from(b"/none").unwrap());
        assert_eq!(listing.error(), Some(Errno::ENOENT));
        assert!(listing.is_empty());
    }

    #[test]
    fn apps_are_their_folders_in_applications() {
        let mut files = MemoryFiles::new();
        files
            .file(
                "/Applications/Paint/app.manifest",
                b"eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\n",
                1,
            )
            .folder("/Applications/Broken")
            .folder("/Applications/Old.app");
        let mut listing = Listing::new();
        listing.read_folder(&mut files, &Path::from(b"/Applications").unwrap());
        assert_eq!(
            listed(&listing),
            [
                ("Broken".to_owned(), Kind::Folder),
                ("Old.app".to_owned(), Kind::Folder),
                ("Paint".to_owned(), Kind::App(Source::Windows)),
            ]
        );
    }

    #[test]
    fn a_full_listing_keeps_the_first_entries() {
        let mut files = MemoryFiles::new();
        for index in 0..MAX_ENTRIES + 3 {
            files.file(&format!("/many/f{index:04}"), b"", 0);
        }
        let mut listing = Listing::new();
        listing.read_folder(&mut files, &Path::from(b"/many").unwrap());
        assert_eq!(listing.len(), MAX_ENTRIES);
        assert!(listing.is_truncated());
    }

    #[test]
    fn the_trash_lists_its_items_newest_first() {
        let mut files = MemoryFiles::new();
        files
            .file("/home/root/old.txt", b"old", 1)
            .file("/home/root/new.txt", b"new!", 1);
        let trash = Trash::of_home(b"/home/root").unwrap();
        trash.put(&mut files, b"/home/root/old.txt", 100).unwrap();
        trash.put(&mut files, b"/home/root/new.txt", 200).unwrap();
        let mut listing = Listing::new();
        listing.read_trash(&mut files, &trash);
        assert_eq!(
            listed(&listing),
            [
                ("new.txt".to_owned(), Kind::Text),
                ("old.txt".to_owned(), Kind::Text)
            ]
        );
        assert_eq!(listing.get(0).unwrap().time, 200);
        assert_eq!(listing.get(0).unwrap().size, 4);
    }

    #[test]
    fn sizes_and_counts_read_as_people_say_them() {
        assert_eq!(size_text(512).as_str(), "512 B");
        assert_eq!(size_text(4321).as_str(), "4.2 KB");
        assert_eq!(size_text(12 * 1024 * 1024 + 5).as_str(), "12 MB");
        assert_eq!(size_text(1536 * 1024 * 1024).as_str(), "1.5 GB");
        assert_eq!(bytes_text(4321).as_str(), "4,321 bytes");
        assert_eq!(bytes_text(1).as_str(), "1 byte");
        assert_eq!(bytes_text(1_234_567).as_str(), "1,234,567 bytes");
        assert_eq!(items_text(1).as_str(), "1 item");
        assert_eq!(items_text(0).as_str(), "0 items");
    }
}
