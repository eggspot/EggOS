//! Directory listing as `FindFirstFileW` does it: a search path splits into the directory and
//! the last component's wildcard pattern (`*` any run of characters, `?` one character,
//! compared ignoring ASCII case as Windows compares names, and `*.*` everything), and each
//! entry that matches fills a `WIN32_FIND_DATAW` from its `stat`.

use crate::text;

pub const FIND_DATA_SIZE: usize = 592;
pub const MAX_PATH: usize = 260;
pub const FILE_INFORMATION_SIZE: usize = 52;
pub const ATTRIBUTE_DATA_SIZE: usize = 36;

pub mod attribute {
    pub const READONLY: u32 = 0x1;
    pub const DIRECTORY: u32 = 0x10;
    pub const NORMAL: u32 = 0x80;
}

mod offset {
    pub const CREATION_TIME: usize = 4;
    pub const LAST_ACCESS_TIME: usize = 12;
    pub const LAST_WRITE_TIME: usize = 20;
    pub const SIZE_HIGH: usize = 28;
    pub const SIZE_LOW: usize = 32;
    pub const FILE_NAME: usize = 44;
    pub const ALTERNATE_FILE_NAME: usize = 564;
}

const FILE_TYPE: u32 = 0o170_000;
const DIRECTORY_MODE: u32 = 0o040_000;
const OWNER_WRITE: u32 = 0o200;

/// What a directory entry's `stat` says, in Windows' terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Entry {
    pub attributes: u32,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub size: u64,
    pub volume: u32,
    pub links: u32,
    pub index: u64,
}

/// A file's Windows attributes from its Linux mode: a directory, read-only without the owner's
/// write permission, otherwise `FILE_ATTRIBUTE_NORMAL`.
pub fn attributes(mode: u32) -> u32 {
    let directory = mode & FILE_TYPE == DIRECTORY_MODE;
    let read_only = mode & OWNER_WRITE == 0;
    match (directory, read_only) {
        (true, true) => attribute::DIRECTORY | attribute::READONLY,
        (true, false) => attribute::DIRECTORY,
        (false, true) => attribute::READONLY,
        (false, false) => attribute::NORMAL,
    }
}

/// Splits the EggOS search path `path` (no NUL) into its directory (`.` when it has none) and
/// its last component, the pattern.
pub fn split(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(0) => (b"/", &path[1..]),
        Some(at) => (&path[..at], &path[at + 1..]),
        None => (b".", path),
    }
}

/// Whether `name` matches the wildcard `pattern` (UTF-8 both).
pub fn matches(pattern: &[u8], name: &[u8]) -> bool {
    if pattern == b"*.*" || pattern == b"*" {
        return true;
    }
    if let Some(stem) = pattern.strip_suffix(b".*")
        && matches_exactly(stem, name)
    {
        return true;
    }
    matches_exactly(pattern, name)
}

fn char_length(bytes: &[u8]) -> usize {
    match bytes.first() {
        Some(0xf0..) => 4.min(bytes.len()),
        Some(0xe0..) => 3.min(bytes.len()),
        Some(0xc0..) => 2.min(bytes.len()),
        _ => 1,
    }
}

fn matches_exactly(pattern: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p + 1, n));
                p += 1;
                continue;
            }
            Some(b'?') => {
                p += 1;
                n += char_length(&name[n..]);
                continue;
            }
            Some(byte) if byte.eq_ignore_ascii_case(&name[n]) => {
                p += 1;
                n += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((after, from)) => {
                let next = from + char_length(&name[from..]);
                star = Some((after, next));
                p = after;
                n = next;
            }
            None => return false,
        }
    }
    pattern[p.min(pattern.len())..]
        .iter()
        .all(|byte| *byte == b'*')
}

fn put_u32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Fills `out`, a `WIN32_FIND_DATAW`, for the entry `name` (UTF-8); `false` if the name does
/// not fit `cFileName` (259 units and the NUL).
pub fn fill_find_data(out: &mut [u8; FIND_DATA_SIZE], entry: &Entry, name: &[u8]) -> bool {
    if text::utf16_length(name) >= MAX_PATH {
        return false;
    }
    out.fill(0);
    put_u32(out, 0, entry.attributes);
    put_u64(out, offset::CREATION_TIME, entry.creation_time);
    put_u64(out, offset::LAST_ACCESS_TIME, entry.last_access_time);
    put_u64(out, offset::LAST_WRITE_TIME, entry.last_write_time);
    put_u32(out, offset::SIZE_HIGH, (entry.size >> 32) as u32);
    put_u32(out, offset::SIZE_LOW, entry.size as u32);
    let mut units = [0u16; MAX_PATH];
    let length = text::write_utf16(name, &mut units[..MAX_PATH - 1]);
    for (index, unit) in units[..length].iter().enumerate() {
        let at = offset::FILE_NAME + index * 2;
        out[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    debug_assert!(offset::FILE_NAME + MAX_PATH * 2 == offset::ALTERNATE_FILE_NAME);
    true
}

/// Fills `out`, a `BY_HANDLE_FILE_INFORMATION`.
pub fn fill_file_information(out: &mut [u8; FILE_INFORMATION_SIZE], entry: &Entry) {
    put_u32(out, 0, entry.attributes);
    put_u64(out, 4, entry.creation_time);
    put_u64(out, 12, entry.last_access_time);
    put_u64(out, 20, entry.last_write_time);
    put_u32(out, 28, entry.volume);
    put_u32(out, 32, (entry.size >> 32) as u32);
    put_u32(out, 36, entry.size as u32);
    put_u32(out, 40, entry.links);
    put_u32(out, 44, (entry.index >> 32) as u32);
    put_u32(out, 48, entry.index as u32);
}

/// Fills `out`, a `WIN32_FILE_ATTRIBUTE_DATA` (`GetFileAttributesExW`'s standard level).
pub fn fill_attribute_data(out: &mut [u8; ATTRIBUTE_DATA_SIZE], entry: &Entry) {
    put_u32(out, 0, entry.attributes);
    put_u64(out, 4, entry.creation_time);
    put_u64(out, 12, entry.last_access_time);
    put_u64(out, 20, entry.last_write_time);
    put_u32(out, 28, (entry.size >> 32) as u32);
    put_u32(out, 32, entry.size as u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_as_windows_matches_names() {
        for (pattern, name) in [
            (&b"*"[..], &b"anything"[..]),
            (b"*.*", b"no-dot"),
            (b"*.exe", b"windows-console.exe"),
            (b"WINDOWS-*.EXE", b"windows-tools.exe"),
            (b"windows-?ools.exe", b"windows-tools.exe"),
            (b"host????", b"hostname"),
            (b"a*b*c", b"aXXbYYc"),
            (b"a*b*c", b"abbbc"),
            (b"?ber.txt", "über.txt".as_bytes()),
            (b"*.txt", "grüße.txt".as_bytes()),
            (b"readme.*", b"readme"),
            (b"*", b""),
            (b"**", b"x"),
        ] {
            assert!(
                matches(pattern, name),
                "{} {}",
                String::from_utf8_lossy(pattern),
                String::from_utf8_lossy(name)
            );
        }
        for (pattern, name) in [
            (&b"*.exe"[..], &b"windows.exe.txt"[..]),
            (b"windows-?ools.exe", b"windows-console.exe"),
            (b"host???", b"hostname"),
            (b"a*b*c", b"aXXbYY"),
            (b"?", b""),
            (b"abc", b"abcd"),
            (b"", b"x"),
            (b"??ber.txt", "über.txt".as_bytes()),
        ] {
            assert!(
                !matches(pattern, name),
                "{} {}",
                String::from_utf8_lossy(pattern),
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn search_paths_split_into_directory_and_pattern() {
        assert_eq!(split(b"/bin/*.exe"), (&b"/bin"[..], &b"*.exe"[..]));
        assert_eq!(split(b"/*"), (&b"/"[..], &b"*"[..]));
        assert_eq!(split(b"*.txt"), (&b"."[..], &b"*.txt"[..]));
        assert_eq!(split(b"a/b/"), (&b"a/b"[..], &b""[..]));
    }

    #[test]
    fn modes_become_attributes() {
        assert_eq!(attributes(0o040_755), attribute::DIRECTORY);
        assert_eq!(
            attributes(0o040_555),
            attribute::DIRECTORY | attribute::READONLY
        );
        assert_eq!(attributes(0o100_644), attribute::NORMAL);
        assert_eq!(attributes(0o100_444), attribute::READONLY);
    }

    #[test]
    fn find_data_has_windows_layout_and_refuses_names_too_long() {
        let entry = Entry {
            attributes: attribute::NORMAL,
            creation_time: 0x0102_0304_0506_0708,
            last_access_time: 2,
            last_write_time: 3,
            size: 0x1_0000_0005,
            ..Entry::default()
        };
        let mut out = [0xffu8; FIND_DATA_SIZE];
        assert!(fill_find_data(&mut out, &entry, "über".as_bytes()));
        assert_eq!(&out[0..4], &0x80u32.to_le_bytes());
        assert_eq!(&out[4..12], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(&out[20..28], &3u64.to_le_bytes());
        assert_eq!(&out[28..32], &1u32.to_le_bytes());
        assert_eq!(&out[32..36], &5u32.to_le_bytes());
        let name: Vec<u16> = out[44..44 + 10]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(name, [0xfc, b'b' as u16, b'e' as u16, b'r' as u16, 0]);
        assert!(out[564..].iter().all(|byte| *byte == 0));
        assert!(fill_find_data(&mut out, &entry, &[b'a'; MAX_PATH - 1]));
        assert!(!fill_find_data(&mut out, &entry, &[b'a'; MAX_PATH]));
    }

    #[test]
    fn file_information_has_windows_layout() {
        let entry = Entry {
            attributes: attribute::DIRECTORY,
            size: 7,
            volume: 9,
            links: 2,
            index: 0x5_0000_0006,
            ..Entry::default()
        };
        let mut out = [0u8; FILE_INFORMATION_SIZE];
        fill_file_information(&mut out, &entry);
        assert_eq!(&out[0..4], &0x10u32.to_le_bytes());
        assert_eq!(&out[28..32], &9u32.to_le_bytes());
        assert_eq!(&out[36..40], &7u32.to_le_bytes());
        assert_eq!(&out[40..44], &2u32.to_le_bytes());
        assert_eq!(&out[44..48], &5u32.to_le_bytes());
        assert_eq!(&out[48..52], &6u32.to_le_bytes());
        let mut data = [0u8; ATTRIBUTE_DATA_SIZE];
        fill_attribute_data(&mut data, &entry);
        assert_eq!(&data[32..36], &7u32.to_le_bytes());
    }
}
