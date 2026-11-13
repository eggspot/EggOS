//! The shell's item ID lists (`ITEMIDLIST`, D399), as the layer makes them: a run of items,
//! each its size in bytes (a little-endian `u16`, itself included) and its body, ended by an
//! item of size 0. The empty list is the desktop, the namespace's root. Every item the layer
//! makes is one name of an EggOS path: `EG`, a flags byte (bit 0: a folder) and the name in
//! UTF-8; the first item of an absolute list has an empty name and is the file system's root,
//! `/` (Windows' `C:\`). A list relative to a folder holds only the names below it.

/// An item's flag: it is a folder.
pub const FOLDER: u8 = 1;
const MAGIC: [u8; 2] = *b"EG";
const SIZE_BYTES: usize = 2;
const HEADER: usize = SIZE_BYTES + MAGIC.len() + 1;
const TERMINATOR: usize = SIZE_BYTES;

/// One item of a list the layer made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item<'a> {
    /// The name, empty for the file system's root.
    pub name: &'a [u8],
    /// Its flags ([`FOLDER`]).
    pub flags: u8,
}

impl Item<'_> {
    /// Whether the item is a folder.
    pub fn is_folder(&self) -> bool {
        self.flags & FOLDER != 0
    }

    /// Whether the item is the file system's root.
    pub fn is_root(&self) -> bool {
        self.name.is_empty()
    }
}

fn item_size(bytes: &[u8]) -> Option<usize> {
    let size = usize::from(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]));
    (size <= bytes.len()).then_some(size)
}

/// The bytes the list starting at `bytes` takes, its terminator included; `None` when it does
/// not end inside `bytes`.
pub fn length(bytes: &[u8]) -> Option<usize> {
    let mut at = 0;
    loop {
        match item_size(&bytes[at..])? {
            0 => return Some(at + TERMINATOR),
            size if size < SIZE_BYTES => return None,
            size => at += size,
        }
    }
}

/// The items' offsets and sizes of the list in `bytes` (its terminator excluded).
pub fn offsets(bytes: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut at = 0;
    core::iter::from_fn(move || {
        let size = item_size(bytes.get(at..)?)?;
        if size < SIZE_BYTES {
            return None;
        }
        let item = (at, size);
        at += size;
        Some(item)
    })
}

/// The item whose bytes (size included) are `bytes`, if the layer made it.
pub fn item(bytes: &[u8]) -> Option<Item<'_>> {
    if bytes.len() < HEADER || bytes[SIZE_BYTES..SIZE_BYTES + MAGIC.len()] != MAGIC {
        return None;
    }
    Some(Item {
        name: &bytes[HEADER..],
        flags: bytes[HEADER - 1],
    })
}

/// The items of the list in `bytes`; `None` for one holding an item the layer did not make.
pub fn items(bytes: &[u8]) -> impl Iterator<Item = Option<Item<'_>>> + '_ {
    offsets(bytes).map(|(at, size)| item(&bytes[at..at + size]))
}

/// The offset of the list's last item, or `None` for the empty list.
pub fn last_offset(bytes: &[u8]) -> Option<usize> {
    offsets(bytes).last().map(|(at, _)| at)
}

/// How many items the list holds.
pub fn count(bytes: &[u8]) -> usize {
    offsets(bytes).count()
}

fn write_item(name: &[u8], flags: u8, out: &mut [u8], at: usize) -> Option<usize> {
    let size = HEADER + name.len();
    let end = at.checked_add(size)?;
    let size_field = u16::try_from(size).ok()?;
    let target = out.get_mut(at..end)?;
    target[..SIZE_BYTES].copy_from_slice(&size_field.to_le_bytes());
    target[SIZE_BYTES..SIZE_BYTES + MAGIC.len()].copy_from_slice(&MAGIC);
    target[HEADER - 1] = flags;
    target[HEADER..].copy_from_slice(name);
    Some(end)
}

fn terminate(out: &mut [u8], at: usize) -> Option<usize> {
    out.get_mut(at..at + TERMINATOR)?.fill(0);
    Some(at + TERMINATOR)
}

fn names(relative: &[u8]) -> impl Iterator<Item = &[u8]> {
    relative
        .split(|&byte| byte == b'/')
        .filter(|name| !name.is_empty())
}

/// Writes the list of the path `relative` (names joined by `/`) below a folder into `out`,
/// every item but the last a folder and the last one too when `last_is_folder`; the bytes
/// written, terminator included.
pub fn write_relative(relative: &[u8], last_is_folder: bool, out: &mut [u8]) -> Option<usize> {
    let total = names(relative).count();
    let mut at = 0;
    for (index, name) in names(relative).enumerate() {
        let folder = index + 1 < total || last_is_folder;
        at = write_item(name, if folder { FOLDER } else { 0 }, out, at)?;
    }
    terminate(out, at)
}

/// Writes the absolute list of the absolute EggOS path `path` into `out`, its last item a
/// folder when `last_is_folder`; the bytes written, terminator included.
pub fn write_absolute(path: &[u8], last_is_folder: bool, out: &mut [u8]) -> Option<usize> {
    let rest = path.strip_prefix(b"/")?;
    let root_only = names(rest).next().is_none();
    let at = write_item(b"", FOLDER, out, 0)?;
    let written = match root_only {
        true => terminate(out, at)?,
        false => at + write_relative(rest, last_is_folder, out.get_mut(at..)?)?,
    };
    match root_only && !last_is_folder {
        true => None,
        false => Some(written),
    }
}

/// Appends the path of the list in `bytes` to the EggOS path `out[..length]` (empty for the
/// desktop): the root item starts over at `/`, every other item adds `/` and its name. The new
/// length; `None` for an item the layer did not make, or when it does not fit.
pub fn append_path(bytes: &[u8], out: &mut [u8], length: usize) -> Option<usize> {
    let mut length = length;
    for found in items(bytes) {
        let found = found?;
        if found.is_root() {
            *out.first_mut()? = b'/';
            length = 1;
            continue;
        }
        if length == 0 {
            return None;
        }
        let separator = usize::from(out[length - 1] != b'/');
        let end = length + separator + found.name.len();
        let target = out.get_mut(length..end)?;
        if separator == 1 {
            target[0] = b'/';
        }
        target[separator..].copy_from_slice(found.name);
        length = end;
    }
    Some(length)
}

/// The EggOS path of the absolute list in `bytes` into `out`: its length, or `None` for the
/// desktop (no path) or a list the layer did not make.
pub fn path(bytes: &[u8], out: &mut [u8]) -> Option<usize> {
    match append_path(bytes, out, 0)? {
        0 => None,
        length => Some(length),
    }
}

/// Orders two items as Explorer lists them: folders first, then by name ignoring ASCII case.
pub fn compare(a: &Item<'_>, b: &Item<'_>) -> core::cmp::Ordering {
    b.is_folder().cmp(&a.is_folder()).then_with(|| {
        let a = a.name.iter().map(u8::to_ascii_lowercase);
        let b = b.name.iter().map(u8::to_ascii_lowercase);
        a.cmp(b)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_list_holds_the_root_then_each_name() {
        let mut out = [0u8; 64];
        let written = write_absolute(b"/home/ana/notes.txt", false, &mut out).unwrap();
        let list = &out[..written];
        assert_eq!(length(list), Some(written));
        assert_eq!(count(list), 4);
        let found: Vec<_> = items(list).map(Option::unwrap).collect();
        assert!(found[0].is_root());
        assert_eq!(found[1].name, b"home");
        assert!(found[2].is_folder());
        assert_eq!(found[3].name, b"notes.txt");
        assert!(!found[3].is_folder());
        let mut path_out = [0u8; 64];
        let path_length = path(list, &mut path_out).unwrap();
        assert_eq!(&path_out[..path_length], b"/home/ana/notes.txt");
    }

    #[test]
    fn the_root_alone_is_a_folder_and_its_path_is_the_slash() {
        let mut out = [0u8; 16];
        let written = write_absolute(b"/", true, &mut out).unwrap();
        assert_eq!(count(&out[..written]), 1);
        let mut path_out = [0u8; 8];
        assert_eq!(path(&out[..written], &mut path_out), Some(1));
        assert_eq!(path_out[0], b'/');
        assert_eq!(write_absolute(b"/", false, &mut out), None);
    }

    #[test]
    fn the_empty_list_is_the_desktop_with_no_path() {
        let desktop = [0u8, 0];
        assert_eq!(length(&desktop), Some(2));
        assert_eq!(count(&desktop), 0);
        assert_eq!(last_offset(&desktop), None);
        let mut out = [0u8; 8];
        assert_eq!(path(&desktop, &mut out), None);
    }

    #[test]
    fn a_relative_list_appends_to_its_folder() {
        let mut out = [0u8; 64];
        let written = write_relative(b"docs/plan", true, &mut out).unwrap();
        let mut path_out = [0u8; 64];
        path_out[..9].copy_from_slice(b"/home/ana");
        let total = append_path(&out[..written], &mut path_out, 9).unwrap();
        assert_eq!(&path_out[..total], b"/home/ana/docs/plan");
        let mut root = [0u8; 64];
        root[0] = b'/';
        let total = append_path(&out[..written], &mut root, 1).unwrap();
        assert_eq!(&root[..total], b"/docs/plan");
    }

    #[test]
    fn foreign_and_truncated_lists_are_refused() {
        let foreign = [4u8, 0, 1, 2, 0, 0];
        assert_eq!(items(&foreign).next(), Some(None));
        let mut out = [0u8; 8];
        assert_eq!(path(&foreign, &mut out), None);
        let truncated = [9u8, 0, b'E', b'G'];
        assert_eq!(length(&truncated), None);
        let looping = [1u8, 0, 0, 0];
        assert_eq!(length(&looping), None);
    }

    #[test]
    fn a_list_that_does_not_fit_is_not_written() {
        let mut out = [0u8; 10];
        assert_eq!(write_absolute(b"/home/ana", true, &mut out), None);
    }

    #[test]
    fn folders_sort_before_files_then_names_ignore_case() {
        let folder = Item {
            name: b"zeta",
            flags: FOLDER,
        };
        let file = Item {
            name: b"Alpha",
            flags: 0,
        };
        let other = Item {
            name: b"alpha",
            flags: 0,
        };
        assert_eq!(compare(&folder, &file), core::cmp::Ordering::Less);
        assert_eq!(compare(&file, &other), core::cmp::Ordering::Equal);
    }
}
