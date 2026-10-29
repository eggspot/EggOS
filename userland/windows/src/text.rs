//! Windows' UTF-16 strings and EggOS's UTF-8 ones, converted without allocating: invalid input
//! (an unpaired surrogate, a bad UTF-8 sequence) becomes U+FFFD, as Windows' own conversions do.

pub const REPLACEMENT: char = '\u{fffd}';

/// The characters of UTF-16 `units`.
pub fn utf16_chars(units: &[u16]) -> impl Iterator<Item = char> + '_ {
    char::decode_utf16(units.iter().copied()).map(|unit| unit.unwrap_or(REPLACEMENT))
}

/// The characters of UTF-8 `bytes`.
pub fn utf8_chars(bytes: &[u8]) -> impl Iterator<Item = char> + '_ {
    let mut rest = bytes;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let head = &rest[..rest.len().min(4)];
        match core::str::from_utf8(head) {
            Ok(text) => {
                let c = text.chars().next()?;
                rest = &rest[c.len_utf8()..];
                Some(c)
            }
            Err(error) if error.valid_up_to() > 0 => {
                let valid = core::str::from_utf8(&head[..error.valid_up_to()]).ok()?;
                let c = valid.chars().next()?;
                rest = &rest[c.len_utf8()..];
                Some(c)
            }
            Err(error) => {
                let skip = error.error_len().unwrap_or(rest.len());
                rest = &rest[skip..];
                Some(REPLACEMENT)
            }
        }
    })
}

/// UTF-8 decoded a byte at a time, for text that arrives in pieces.
#[derive(Debug, Default, Clone, Copy)]
pub struct Utf8Decoder {
    bytes: [u8; 4],
    length: usize,
    needed: usize,
}

impl Utf8Decoder {
    /// Takes the next byte and returns the characters it completes, in order: U+FFFD first when
    /// the byte cuts a sequence short, then the character the byte ends or is itself (none while
    /// a sequence is still open).
    pub fn push(&mut self, byte: u8) -> [Option<char>; 2] {
        let mut cut_short = None;
        if self.length > 0 && byte & 0xc0 != 0x80 {
            self.length = 0;
            cut_short = Some(REPLACEMENT);
        }
        [cut_short, self.take(byte)]
    }

    fn take(&mut self, byte: u8) -> Option<char> {
        if self.length == 0 {
            self.needed = match byte {
                0x00..=0x7f => return Some(char::from(byte)),
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Some(REPLACEMENT),
            };
        }
        self.bytes[self.length] = byte;
        self.length += 1;
        if self.length < self.needed {
            return None;
        }
        let complete = &self.bytes[..self.length];
        self.length = 0;
        Some(
            core::str::from_utf8(complete)
                .ok()
                .and_then(|text| text.chars().next())
                .unwrap_or(REPLACEMENT),
        )
    }
}

/// How many UTF-16 units `bytes` (UTF-8) takes.
pub fn utf16_length(bytes: &[u8]) -> usize {
    utf8_chars(bytes).map(char::len_utf16).sum()
}

/// Writes `bytes` (UTF-8) as UTF-16 into `out` as far as it fits; returns the units written.
pub fn write_utf16(bytes: &[u8], out: &mut [u16]) -> usize {
    let mut written = 0;
    for c in utf8_chars(bytes) {
        let mut units = [0; 2];
        let units = c.encode_utf16(&mut units);
        if written + units.len() > out.len() {
            break;
        }
        out[written..written + units.len()].copy_from_slice(units);
        written += units.len();
    }
    written
}

/// Writes UTF-16 `units` as UTF-8 into `out` as far as whole characters fit; returns the bytes
/// written and the units consumed.
pub fn write_utf8(units: &[u16], out: &mut [u8]) -> (usize, usize) {
    let mut written = 0;
    let mut consumed = 0;
    for c in utf16_chars(units) {
        let length = c.len_utf8();
        if written + length > out.len() {
            break;
        }
        c.encode_utf8(&mut out[written..]);
        written += length;
        consumed += match c {
            REPLACEMENT => 1,
            c => c.len_utf16(),
        };
    }
    (written, consumed)
}

pub const PATH_CAPACITY: usize = 4096;

/// Writes the Windows path `chars` as a NUL-terminated EggOS path into `out` (`\\` is `/`,
/// and a drive letter or the `\\?\\` prefix goes); its length with the NUL, or `None` if it
/// does not fit.
pub fn eggos_path(
    chars: impl Iterator<Item = char>,
    out: &mut [u8; PATH_CAPACITY],
) -> Option<usize> {
    let mut length = 0;
    for c in chars {
        let c = if c == '\\' { '/' } else { c };
        if length + c.len_utf8() >= out.len() {
            return None;
        }
        c.encode_utf8(&mut out[length..]);
        length += c.len_utf8();
    }
    let mut path = &out[..length];
    if path.starts_with(b"//?/") {
        path = &path[4..];
    }
    if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        path = &path[2..];
    }
    let skipped = length - path.len();
    out.copy_within(skipped..length, 0);
    out[length - skipped] = 0;
    Some(length - skipped + 1)
}

/// How many UTF-8 bytes UTF-16 `units` take (an unpaired surrogate as U+FFFD's three).
pub fn utf8_length(units: &[u16]) -> usize {
    utf16_chars(units).map(char::len_utf8).sum()
}

/// Whether `units` hold an unpaired surrogate, which UTF-8 cannot carry.
pub fn has_unpaired_surrogate(units: &[u16]) -> bool {
    char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err())
}

/// The upper case of UTF-16 unit `unit` when it is one character whose simple upper case is one
/// unit too, as `CharUpperW` maps it; otherwise `unit` itself.
pub fn upper(unit: u16) -> u16 {
    let Some(c) = char::from_u32(u32::from(unit)) else {
        return unit;
    };
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => u16::try_from(u32::from(single)).unwrap_or(unit),
        _ => unit,
    }
}

/// The lower case of UTF-16 unit `unit`, as [`upper`] maps upper case.
pub fn lower(unit: u16) -> u16 {
    let Some(c) = char::from_u32(u32::from(unit)) else {
        return unit;
    };
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => u16::try_from(u32::from(single)).unwrap_or(unit),
        _ => unit,
    }
}

/// Compares UTF-16 strings unit by unit, as `CompareStringOrdinal` does (with `ignore_case`,
/// after [`upper`]).
pub fn compare_ordinal(a: &[u16], b: &[u16], ignore_case: bool) -> core::cmp::Ordering {
    let fold = |unit: &u16| if ignore_case { upper(*unit) } else { *unit };
    a.iter().map(fold).cmp(b.iter().map(fold))
}

/// Writes the EggOS path `path` (UTF-8, no NUL) in Windows' form through `out`: `/` is `\`,
/// and an absolute path starts with the drive `C:`.
pub fn windows_path(path: &[u8], out: &mut impl FnMut(char)) {
    if path.first() == Some(&b'/') {
        out('C');
        out(':');
    }
    for c in utf8_chars(path) {
        out(if c == '/' { '\\' } else { c });
    }
}

/// Writes `path` (an EggOS path, UTF-8, no NUL), taken relative to the absolute directory
/// `directory` unless it is absolute itself, into `out` as an absolute path with `.`, `..` and
/// repeated or trailing slashes resolved; its length (no NUL), or `None` if it does not fit.
pub fn absolute_path(directory: &[u8], path: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut length = 0;
    let base: &[u8] = if path.first() == Some(&b'/') {
        b""
    } else {
        directory
    };
    for component in base
        .split(|byte| *byte == b'/')
        .chain(path.split(|byte| *byte == b'/'))
    {
        match component {
            b"" | b"." => {}
            b".." => {
                length = out[..length]
                    .iter()
                    .rposition(|byte| *byte == b'/')
                    .unwrap_or(0);
            }
            name => {
                let end = length.checked_add(1 + name.len())?;
                if end > out.len() {
                    return None;
                }
                out[length] = b'/';
                out[length + 1..end].copy_from_slice(name);
                length = end;
            }
        }
    }
    if length == 0 {
        *out.first_mut()? = b'/';
        length = 1;
    }
    Some(length)
}

/// Whether two ASCII names are equal, ignoring case, as Windows compares DLL and environment
/// variable names.
pub fn equal_ignoring_ascii_case(a: impl Iterator<Item = u8>, b: impl Iterator<Item = u8>) -> bool {
    a.map(|byte| byte.to_ascii_lowercase())
        .eq(b.map(|byte| byte.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_utf16_convert_both_ways_with_replacements_for_bad_input() {
        let text = "Egg \u{1f95a} os é";
        let mut units = [0u16; 32];
        let length = write_utf16(text.as_bytes(), &mut units);
        assert_eq!(length, text.encode_utf16().count());
        assert_eq!(utf16_length(text.as_bytes()), length);
        let mut bytes = [0u8; 32];
        assert_eq!(
            write_utf8(&units[..length], &mut bytes),
            (text.len(), length)
        );
        assert_eq!(&bytes[..text.len()], text.as_bytes());
        let bad: String = utf8_chars(b"a\xffb\xe2\x82").collect();
        assert_eq!(bad, "a\u{fffd}b\u{fffd}");
        let unpaired: String = utf16_chars(&[0x61, 0xd800, 0x62]).collect();
        assert_eq!(unpaired, "a\u{fffd}b");
        assert_eq!(write_utf8(&[0x61, 0xdc00], &mut bytes), (4, 2));
    }

    fn decoded(bytes: &[u8]) -> String {
        let mut decoder = Utf8Decoder::default();
        bytes
            .iter()
            .flat_map(|&byte| decoder.push(byte))
            .flatten()
            .collect()
    }

    #[test]
    fn the_byte_decoder_keeps_the_byte_after_a_sequence_cut_short() {
        assert_eq!(decoded(b"caf\xe9!"), "caf\u{fffd}!");
        assert_eq!(decoded(b"\xe2\x82x\xc3"), "\u{fffd}x");
        assert_eq!(decoded(b"a\xffb"), "a\u{fffd}b");
        assert_eq!(decoded(b"\xe0\x80\x80z"), "\u{fffd}z");
        assert_eq!(decoded("\u{e9}\u{1f95a}".as_bytes()), "\u{e9}\u{1f95a}");
    }

    #[test]
    fn conversions_stop_at_the_last_whole_character_that_fits() {
        let mut units = [0u16; 3];
        assert_eq!(write_utf16("ab\u{1f95a}".as_bytes(), &mut units), 2);
        let mut bytes = [0u8; 5];
        let wide: Vec<u16> = "ab\u{1f95a}".encode_utf16().collect();
        assert_eq!(write_utf8(&wide, &mut bytes), (2, 2));
    }

    fn path(windows: &str) -> Option<String> {
        let mut out = [0u8; PATH_CAPACITY];
        let length = eggos_path(windows.chars(), &mut out)?;
        assert_eq!(out[length - 1], 0);
        Some(String::from_utf8(out[..length - 1].to_vec()).unwrap())
    }

    #[test]
    fn windows_paths_become_eggos_paths() {
        assert_eq!(path(r"C:\etc\hostname").as_deref(), Some("/etc/hostname"));
        assert_eq!(path(r"\\?\D:\x\y.txt").as_deref(), Some("/x/y.txt"));
        assert_eq!(path("data.txt").as_deref(), Some("data.txt"));
        assert_eq!(path(r"dir\é.txt").as_deref(), Some("dir/é.txt"));
        assert_eq!(path("C:").as_deref(), Some(""));
        assert!(path(&"a".repeat(PATH_CAPACITY)).is_none());
        assert!(path(&"a".repeat(PATH_CAPACITY - 1)).is_some());
    }

    #[test]
    fn names_compare_ignoring_ascii_case() {
        assert!(equal_ignoring_ascii_case(
            b"KERNEL32.dll".iter().copied(),
            b"kernel32.DLL".iter().copied()
        ));
        assert!(!equal_ignoring_ascii_case(
            b"kernel32".iter().copied(),
            b"kernel32.dll".iter().copied()
        ));
    }

    #[test]
    fn lengths_surrogates_and_upper_case() {
        let wide: Vec<u16> = "aé\u{1f95a}".encode_utf16().collect();
        assert_eq!(utf8_length(&wide), 1 + 2 + 4);
        assert_eq!(utf8_length(&[0xd800]), 3);
        assert!(has_unpaired_surrogate(&[0x61, 0xdc00]));
        assert!(!has_unpaired_surrogate(&wide));
        assert_eq!(upper(u16::from(b'a')), u16::from(b'A'));
        assert_eq!(upper(0xe9), 0xc9);
        assert_eq!(upper(0xdf), 0xdf);
        assert_eq!(upper(0xd800), 0xd800);
        assert_eq!(upper(u16::from(b'1')), u16::from(b'1'));
        assert_eq!(lower(0xc9), 0xe9);
        assert_eq!(lower(0x130), 0x130);
    }

    #[test]
    fn ordinal_comparison_is_by_unit_with_optional_case_folding() {
        use core::cmp::Ordering;
        let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(
            compare_ordinal(&wide("abc"), &wide("abc"), false),
            Ordering::Equal
        );
        assert_eq!(
            compare_ordinal(&wide("ABC"), &wide("abc"), false),
            Ordering::Less
        );
        assert_eq!(
            compare_ordinal(&wide("ABC"), &wide("abc"), true),
            Ordering::Equal
        );
        assert_eq!(
            compare_ordinal(&wide("ab"), &wide("abc"), false),
            Ordering::Less
        );
        assert_eq!(
            compare_ordinal(&wide("Grüße"), &wide("GRÜSSE"), true),
            Ordering::Greater
        );
        assert_eq!(
            compare_ordinal(&wide("é"), &wide("É"), true),
            Ordering::Equal
        );
    }

    fn windows(path: &str) -> String {
        let mut out = String::new();
        windows_path(path.as_bytes(), &mut |c| out.push(c));
        out
    }

    #[test]
    fn eggos_paths_become_windows_paths() {
        assert_eq!(windows("/"), r"C:\");
        assert_eq!(
            windows("/bin/windows-tools.exe"),
            r"C:\bin\windows-tools.exe"
        );
        assert_eq!(windows("data/é.txt"), r"data\é.txt");
    }

    fn absolute(directory: &str, path: &str) -> Option<String> {
        let mut out = [0u8; 64];
        let length = absolute_path(directory.as_bytes(), path.as_bytes(), &mut out)?;
        Some(String::from_utf8(out[..length].to_vec()).unwrap())
    }

    #[test]
    fn paths_resolve_against_the_directory() {
        assert_eq!(
            absolute("/etc", "hostname").as_deref(),
            Some("/etc/hostname")
        );
        assert_eq!(absolute("/etc", "/bin//x/").as_deref(), Some("/bin/x"));
        assert_eq!(
            absolute("/usr/lib", "../share/./a").as_deref(),
            Some("/usr/share/a")
        );
        assert_eq!(absolute("/", "../..").as_deref(), Some("/"));
        assert_eq!(absolute("/etc", "").as_deref(), Some("/etc"));
        assert_eq!(absolute("/", "").as_deref(), Some("/"));
        assert_eq!(
            absolute("/", &"a".repeat(63)).as_deref(),
            Some(&*format!("/{}", "a".repeat(63)))
        );
        assert_eq!(absolute("/", &"a".repeat(64)), None);
    }
}
