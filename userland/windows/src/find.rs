//! Directory listing as `FindFirstFileW` does it: a search path splits into the directory and
//! the last component's wildcard pattern, matched with DOS's rules ([`matches`]: `*`, `?`,
//! `*.*`, `*.` and the DOS wildcards `<`, `>` and `"`, compared ignoring ASCII case as Windows
//! compares names), and each entry that matches fills a `WIN32_FIND_DATAW` from its `stat`.

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

const _: () = assert!(offset::FILE_NAME + MAX_PATH * 2 == offset::ALTERNATE_FILE_NAME);
const _: () = assert!(offset::ALTERNATE_FILE_NAME + 28 == FIND_DATA_SIZE);

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

/// Where the pattern starts in the program argument `argument` that the C runtime expands
/// (`_dowildcard`, D189): after its last `\` or `/`, or after the drive's `:`, when that last
/// component holds `*` or `?`; `None` for an argument kept as it is (no wildcard there, or a
/// `:` other than the drive's).
pub fn argument_pattern_start(argument: &[u8]) -> Option<usize> {
    let other_colon = argument
        .iter()
        .enumerate()
        .any(|(at, byte)| *byte == b':' && at != 1);
    if other_colon {
        return None;
    }
    let start = match argument
        .iter()
        .rposition(|byte| matches!(byte, b'\\' | b'/'))
    {
        Some(at) => at + 1,
        None if argument.get(1) == Some(&b':') => 2,
        None => 0,
    };
    argument[start..]
        .iter()
        .any(|byte| matches!(byte, b'*' | b'?'))
        .then_some(start)
}

/// Whether `name` matches the wildcard `pattern` (UTF-8 both) as `FindFirstFileW` matches
/// names, ignoring ASCII case: `*` any run of characters, `?` one character or none before a
/// dot or the end, a `.` before a wildcard or at the end a dot or the end (`*.*` everything,
/// `readme.*` `readme` too), `*.` a name without an extension, and the DOS wildcards `<` (any
/// run up to the last dot), `>` (as `?`) and `"` (a dot or the end). A pattern without
/// wildcards names one entry, its trailing dots dropped.
pub fn matches(pattern: &[u8], name: &[u8]) -> bool {
    if pattern == b"*" || pattern == b"*.*" {
        return true;
    }
    if name.len() > MAX_NAME_BYTES {
        return false;
    }
    if !pattern.iter().any(|byte| b"*?<>\"".contains(byte)) {
        let literal = match pattern.iter().rposition(|byte| *byte != b'.') {
            Some(last) => &pattern[..=last],
            None => pattern,
        };
        return literal.eq_ignore_ascii_case(name);
    }
    let last_dot = name.iter().rposition(|byte| *byte == b'.');
    let mut reached = Positions::none();
    reached.add(0);
    for index in 0..pattern.len() {
        reached = reached.step(wildcard(pattern, index), name, last_dot);
    }
    reached.has(name.len())
}

const MAX_NAME_BYTES: usize = 1023;
const POSITION_WORDS: usize = (MAX_NAME_BYTES + 1) / 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wildcard {
    AnyRun,
    RunToLastDot,
    OneOrNone,
    DotOrEnd,
    Byte(u8),
}

fn wildcard(pattern: &[u8], index: usize) -> Wildcard {
    let next = pattern.get(index + 1).copied();
    let last = index + 1 == pattern.len();
    match pattern[index] {
        b'*' if next == Some(b'.') && index + 2 == pattern.len() => Wildcard::RunToLastDot,
        b'*' => Wildcard::AnyRun,
        b'<' => Wildcard::RunToLastDot,
        b'?' | b'>' => Wildcard::OneOrNone,
        b'.' if last || matches!(next, Some(b'*' | b'?')) => Wildcard::DotOrEnd,
        b'"' => Wildcard::DotOrEnd,
        byte => Wildcard::Byte(byte),
    }
}

fn char_length(bytes: &[u8]) -> usize {
    match bytes.first() {
        Some(0xf0..) => 4.min(bytes.len()),
        Some(0xe0..) => 3.min(bytes.len()),
        Some(0xc0..) => 2.min(bytes.len()),
        _ => 1,
    }
}

fn starts_char(name: &[u8], at: usize) -> bool {
    name.get(at).is_none_or(|byte| !(0x80..0xc0).contains(byte))
}

#[derive(Debug, Clone, Copy)]
struct Positions([u64; POSITION_WORDS]);

impl Positions {
    const fn none() -> Self {
        Self([0; POSITION_WORDS])
    }

    fn add(&mut self, at: usize) {
        self.0[at / 64] |= 1 << (at % 64);
    }

    fn has(&self, at: usize) -> bool {
        self.0[at / 64] & (1 << (at % 64)) != 0
    }

    fn add_run(&mut self, name: &[u8], from: usize, to: usize) {
        for at in from..=to {
            if starts_char(name, at) {
                self.add(at);
            }
        }
    }

    fn step(&self, wildcard: Wildcard, name: &[u8], last_dot: Option<usize>) -> Self {
        let mut next = Self::none();
        for from in (0..=name.len()).filter(|at| self.has(*at)) {
            match wildcard {
                Wildcard::AnyRun => {
                    next.add_run(name, from, name.len());
                    break;
                }
                Wildcard::RunToLastDot => {
                    let end = last_dot.filter(|dot| *dot >= from).unwrap_or(name.len());
                    next.add_run(name, from, end);
                }
                Wildcard::OneOrNone => match name.get(from) {
                    None | Some(b'.') => next.add(from),
                    Some(_) => next.add(from + char_length(&name[from..])),
                },
                Wildcard::DotOrEnd => match name.get(from) {
                    None => next.add(from),
                    Some(b'.') => next.add(from + 1),
                    Some(_) => {}
                },
                Wildcard::Byte(byte) => {
                    if name
                        .get(from)
                        .is_some_and(|own| own.eq_ignore_ascii_case(&byte))
                    {
                        next.add(from + 1);
                    }
                }
            }
        }
        next
    }
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

/// The size of `FILE_BASIC_INFO`.
pub const BASIC_INFORMATION_SIZE: usize = 40;
/// The size of `FILE_STANDARD_INFO`.
pub const STANDARD_INFORMATION_SIZE: usize = 24;
/// The size of `FILE_ATTRIBUTE_TAG_INFO`.
pub const ATTRIBUTE_TAG_INFORMATION_SIZE: usize = 8;

/// Fills `out`, a `FILE_BASIC_INFO` (`GetFileInformationByHandleEx`'s `FileBasicInfo`): the
/// creation, access, write and change times and the attributes.
pub fn fill_basic_information(out: &mut [u8; BASIC_INFORMATION_SIZE], entry: &Entry) {
    out.fill(0);
    put_u64(out, 0, entry.creation_time);
    put_u64(out, 8, entry.last_access_time);
    put_u64(out, 16, entry.last_write_time);
    put_u64(out, 24, entry.creation_time);
    put_u32(out, 32, entry.attributes);
}

/// Fills `out`, a `FILE_STANDARD_INFO`: the size as both the allocation and the end of the
/// file, the links, never a pending delete, and whether it is a directory.
pub fn fill_standard_information(out: &mut [u8; STANDARD_INFORMATION_SIZE], entry: &Entry) {
    out.fill(0);
    put_u64(out, 0, entry.size);
    put_u64(out, 8, entry.size);
    put_u32(out, 16, entry.links);
    out[21] = u8::from(entry.attributes & attribute::DIRECTORY != 0);
}

/// Fills `out`, a `FILE_ATTRIBUTE_TAG_INFO`: the attributes and no reparse tag.
pub fn fill_attribute_tag_information(
    out: &mut [u8; ATTRIBUTE_TAG_INFORMATION_SIZE],
    entry: &Entry,
) {
    put_u32(out, 0, entry.attributes);
    put_u32(out, 4, 0);
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
            (b"a??", b"a"),
            (b"a??", b"abc"),
            (b"a?.txt", b"a.txt"),
            (b"*.", b"README"),
            (b"readme.", b"readme"),
            (b"FILE.TXT", b"file.txt"),
            (b"..", b".."),
            (b"<.txt", b"a.b.txt"),
            (b"a\"txt", b"a.txt"),
            (b"a\"", b"a"),
            (b">>>.c", b"ab.c"),
            (b"a*.*", b"abc"),
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
            (b"abc", b"abcd"),
            (b"", b"x"),
            (b"??ber.txt", "über.txt".as_bytes()),
            (b"a??", b"abcd"),
            (b"a?", b"a.b"),
            (b"*.", b"a.txt"),
            (b"..", b"."),
            (b"<.txt", b"a.txt.bak"),
            (b"a\"txt", b"abtxt"),
            (b"?", b"ab"),
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

    #[test]
    fn the_handle_information_classes_have_windows_layout() {
        let entry = Entry {
            attributes: attribute::DIRECTORY,
            creation_time: 3,
            last_access_time: 4,
            last_write_time: 5,
            size: 7,
            links: 2,
            ..Entry::default()
        };
        let mut basic = [0xffu8; BASIC_INFORMATION_SIZE];
        fill_basic_information(&mut basic, &entry);
        assert_eq!(&basic[0..8], &3u64.to_le_bytes());
        assert_eq!(&basic[8..16], &4u64.to_le_bytes());
        assert_eq!(&basic[16..24], &5u64.to_le_bytes());
        assert_eq!(&basic[24..32], &3u64.to_le_bytes());
        assert_eq!(&basic[32..40], &[0x10, 0, 0, 0, 0, 0, 0, 0]);
        let mut standard = [0xffu8; STANDARD_INFORMATION_SIZE];
        fill_standard_information(&mut standard, &entry);
        assert_eq!(&standard[0..8], &7u64.to_le_bytes());
        assert_eq!(&standard[8..16], &7u64.to_le_bytes());
        assert_eq!(&standard[16..20], &2u32.to_le_bytes());
        assert_eq!(&standard[20..24], &[0, 1, 0, 0]);
        let mut tag = [0xffu8; ATTRIBUTE_TAG_INFORMATION_SIZE];
        fill_attribute_tag_information(&mut tag, &entry);
        assert_eq!(tag, [0x10, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn arguments_expand_only_a_wildcard_in_their_last_component() {
        assert_eq!(argument_pattern_start(b"*.txt"), Some(0));
        assert_eq!(argument_pattern_start(b"C:\\tmp\\x\\a?.c"), Some(9));
        assert_eq!(argument_pattern_start(b"dir/*"), Some(4));
        assert_eq!(argument_pattern_start(b"C:*.txt"), Some(2));
        assert_eq!(argument_pattern_start(b"plain.txt"), None);
        assert_eq!(argument_pattern_start(b"a*\\b.txt"), None);
        assert_eq!(argument_pattern_start(b"x:y:*.c"), None);
        assert_eq!(argument_pattern_start(b"http://*"), None);
    }
}
