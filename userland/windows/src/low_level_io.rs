//! The C runtime's low-level I/O (`io.h`, `fcntl.h`, `share.h`; D179): what `_open`'s
//! `_O_*` flags, `_sopen_s`'s sharing mode and `_lseek`'s origin ask of Linux's `openat` and
//! `lseek`, and what a descriptor's translation mode does to the bytes `_read` and `_write`
//! move (D229), kept here, outside the layer's Win32 code, so its host tests run.

/// `_open`'s flags (`_O_*`).
pub mod windows_flag {
    pub const RDONLY: i32 = 0x0;
    pub const WRONLY: i32 = 0x1;
    pub const RDWR: i32 = 0x2;
    pub const ACCESS: i32 = 0x3;
    pub const APPEND: i32 = 0x8;
    pub const RANDOM: i32 = 0x10;
    pub const SEQUENTIAL: i32 = 0x20;
    pub const TEMPORARY: i32 = 0x40;
    pub const NOINHERIT: i32 = 0x80;
    pub const CREAT: i32 = 0x100;
    pub const TRUNC: i32 = 0x200;
    pub const EXCL: i32 = 0x400;
    pub const SHORT_LIVED: i32 = 0x1000;
    pub const OBTAIN_DIR: i32 = 0x2000;
    pub const TEXT: i32 = 0x4000;
    pub const BINARY: i32 = 0x8000;
    pub const WTEXT: i32 = 0x1_0000;
    pub const U16TEXT: i32 = 0x2_0000;
    pub const U8TEXT: i32 = 0x4_0000;
    /// Every flag `_open` knows.
    pub const KNOWN: i32 = ACCESS
        | APPEND
        | RANDOM
        | SEQUENTIAL
        | TEMPORARY
        | NOINHERIT
        | CREAT
        | TRUNC
        | EXCL
        | SHORT_LIVED
        | OBTAIN_DIR
        | TEXT
        | BINARY
        | WTEXT
        | U16TEXT
        | U8TEXT;
}

/// Linux's `openat` flags the low-level functions ask for.
pub mod linux_flag {
    pub const RDONLY: usize = 0;
    pub const WRONLY: usize = 1;
    pub const RDWR: usize = 2;
    pub const CREAT: usize = 0o100;
    pub const EXCL: usize = 0o200;
    pub const TRUNC: usize = 0o1000;
    pub const APPEND: usize = 0o2000;
}

/// `_sopen_s`'s sharing modes (`_SH_*`).
pub mod share {
    pub const DENY_READ_WRITE: i32 = 0x10;
    pub const DENY_WRITE: i32 = 0x20;
    pub const DENY_READ: i32 = 0x30;
    pub const DENY_NONE: i32 = 0x40;
}

/// What `_open`'s `flags` ask of `openat`: its flags, the file to remove once opened
/// (`_O_TEMPORARY`, which Windows removes at the last close and EggOS removes at once, keeping
/// it while it is open). `None` for an access mode Windows has not (both `_O_WRONLY` and
/// `_O_RDWR`) or an unknown flag, which the runtime answers with `EINVAL`.
pub fn open_flags(flags: i32) -> Option<(usize, bool)> {
    if flags & !windows_flag::KNOWN != 0 {
        return None;
    }
    let access = match flags & windows_flag::ACCESS {
        windows_flag::RDONLY => linux_flag::RDONLY,
        windows_flag::WRONLY => linux_flag::WRONLY,
        windows_flag::RDWR => linux_flag::RDWR,
        _ => return None,
    };
    let asked = [
        (windows_flag::APPEND, linux_flag::APPEND),
        (windows_flag::CREAT, linux_flag::CREAT),
        (windows_flag::TRUNC, linux_flag::TRUNC),
        (windows_flag::EXCL, linux_flag::EXCL),
    ]
    .into_iter()
    .filter(|&(windows, _)| flags & windows != 0)
    .fold(access, |linux, (_, flag)| linux | flag);
    Some((asked, flags & windows_flag::TEMPORARY != 0))
}

/// Whether `share` is one of `_sopen_s`'s sharing modes. EggOS's files are always shared, as
/// Linux's are, so a mode is checked and then has no effect.
pub fn is_sharing_mode(share: i32) -> bool {
    matches!(
        share,
        share::DENY_READ_WRITE | share::DENY_WRITE | share::DENY_READ | share::DENY_NONE
    )
}

/// `lseek`'s `whence` for `_lseek`'s `origin` (`SEEK_SET`, `SEEK_CUR`, `SEEK_END`: the same
/// numbers); `None` for another.
pub fn whence(origin: i32) -> Option<usize> {
    match origin {
        0..=2 => Some(origin as usize),
        _ => None,
    }
}

/// What a descriptor's translation mode does to the bytes `_read` and `_write` move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Translation {
    /// `_O_BINARY`: bytes as they are.
    Binary,
    /// `_O_TEXT`: `\n` is `\r\n` in the file and Ctrl+Z ends the text.
    Text,
    /// `_O_WTEXT` and `_O_U16TEXT`: the program's UTF-16 is UTF-16 in the file.
    Utf16,
    /// `_O_U8TEXT`: the program's UTF-16 is UTF-8 in the file.
    Utf8,
}

impl Translation {
    /// The translation of the mode `_setmode` and `_open` take (`_O_TEXT`, ...).
    pub fn of_mode(mode: i32) -> Translation {
        match mode {
            windows_flag::BINARY => Translation::Binary,
            windows_flag::WTEXT | windows_flag::U16TEXT => Translation::Utf16,
            windows_flag::U8TEXT => Translation::Utf8,
            _ => Translation::Text,
        }
    }

    /// Whether the program's buffers hold UTF-16, so their sizes must be even.
    pub fn is_wide(self) -> bool {
        matches!(self, Translation::Utf16 | Translation::Utf8)
    }
}

/// The translation modes, in the order `_open` looks for them in its flags.
pub const TRANSLATION_MODES: [i32; 5] = [
    windows_flag::TEXT,
    windows_flag::BINARY,
    windows_flag::WTEXT,
    windows_flag::U16TEXT,
    windows_flag::U8TEXT,
];

/// The translation mode `_open`'s `flags` ask for, else `default` (`_fmode`'s).
pub fn open_mode(flags: i32, default: i32) -> i32 {
    TRANSLATION_MODES
        .into_iter()
        .find(|mode| flags & mode != 0)
        .unwrap_or(default)
}

const CARRIAGE_RETURN: u8 = b'\r';
const LINE_FEED: u8 = b'\n';
const END_OF_TEXT: u8 = 0x1a;
/// The fewest bytes `translate_written`'s `out` may hold: one character's.
pub const SMALLEST_TRANSLATION: usize = 4;
const HIGH_SURROGATES: core::ops::RangeInclusive<u16> = 0xd800..=0xdbff;

fn put(out: &mut [u8], at: &mut usize, bytes: &[u8]) -> bool {
    match out.get_mut(*at..*at + bytes.len()) {
        Some(room) => {
            room.copy_from_slice(bytes);
            *at += bytes.len();
            true
        }
        None => false,
    }
}

/// Translates the start of `input`, the bytes given to `_write`, into `out` as `translation`
/// asks for a file, or with `to_terminal` for a terminal, which takes UTF-8 in every wide
/// mode: text mode writes `\n` as `\r\n`; the wide modes read `input` as UTF-16 and write
/// UTF-16 with `\r\n`, or UTF-8 with `\r\n` (`_O_U8TEXT`, a terminal; an unpaired surrogate
/// as U+FFFD, except a high surrogate that ends `input`, which is left for the next write's
/// low one, see [`translate_write`]). Takes whole characters only; returns how many bytes of
/// `input` it took and how many it put in `out`, which holds at least `SMALLEST_TRANSLATION`
/// bytes.
pub fn translate_written(
    translation: Translation,
    to_terminal: bool,
    input: &[u8],
    out: &mut [u8],
) -> (usize, usize) {
    let mut taken = 0;
    let mut put_at = 0;
    match (translation, to_terminal) {
        (Translation::Binary, _) => {
            taken = input.len().min(out.len());
            out[..taken].copy_from_slice(&input[..taken]);
            put_at = taken;
        }
        (Translation::Text, _) => {
            for &byte in input {
                let bytes: &[u8] = match byte {
                    LINE_FEED => b"\r\n",
                    _ => core::slice::from_ref(&byte),
                };
                if !put(out, &mut put_at, bytes) {
                    break;
                }
                taken += 1;
            }
        }
        (Translation::Utf16, false) => {
            for unit in input.as_chunks::<2>().0 {
                let bytes: &[u8] = match unit {
                    [LINE_FEED, 0] => &[CARRIAGE_RETURN, 0, LINE_FEED, 0],
                    _ => unit,
                };
                if !put(out, &mut put_at, bytes) {
                    break;
                }
                taken += 2;
            }
        }
        _ => {
            let pairs = input.as_chunks::<2>().0;
            let units = pairs.iter().map(|pair| u16::from_le_bytes(*pair));
            for decoded in char::decode_utf16(units) {
                let (c, width) = match decoded {
                    Ok(c) => (c, c.len_utf16() * 2),
                    Err(error)
                        if is_high_surrogate(error.unpaired_surrogate())
                            && taken + 2 == pairs.len() * 2 =>
                    {
                        break;
                    }
                    Err(_) => (char::REPLACEMENT_CHARACTER, 2),
                };
                let mut encoded = [0; 4];
                let bytes: &[u8] = match c {
                    '\n' => b"\r\n",
                    _ => c.encode_utf8(&mut encoded).as_bytes(),
                };
                if !put(out, &mut put_at, bytes) {
                    break;
                }
                taken += width;
            }
        }
    }
    (taken, put_at)
}

fn is_high_surrogate(unit: u16) -> bool {
    HIGH_SURROGATES.contains(&unit)
}

/// The high surrogate `rest`, what a wide write left untranslated, is alone, if it is one.
pub fn lone_high_surrogate(rest: &[u8]) -> Option<u16> {
    match rest {
        [low, high] => {
            Some(u16::from_le_bytes([*low, *high])).filter(|&unit| is_high_surrogate(unit))
        }
        _ => None,
    }
}

/// Translates all of `bytes`, one `_write`'s, as [`translate_written`] does, through `out`,
/// giving each translated piece to `emit`. `pending` is the high surrogate an earlier wide
/// write ended with: it goes before this write's first unit, so a character split between two
/// writes is written whole, and a high surrogate this write ends with takes its place.
/// Returns how many bytes of `bytes` were taken, or with `emit`'s error how many were taken
/// before it.
pub fn translate_write<E>(
    translation: Translation,
    to_terminal: bool,
    bytes: &[u8],
    pending: &mut Option<u16>,
    out: &mut [u8],
    mut emit: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<usize, (usize, E)> {
    let mut taken = 0;
    if let Some(high) = pending.take()
        && translation.is_wide()
    {
        let mut joined = [0u8; 4];
        joined[..2].copy_from_slice(&high.to_le_bytes());
        let first = bytes.len().min(2);
        joined[2..2 + first].copy_from_slice(&bytes[..first]);
        let (used, put) = translate_written(translation, to_terminal, &joined[..2 + first], out);
        emit(&out[..put]).map_err(|error| (0, error))?;
        if used == 0 {
            *pending = Some(high);
            return Ok(0);
        }
        taken = used - 2;
    }
    while taken < bytes.len() {
        let (used, put) = translate_written(translation, to_terminal, &bytes[taken..], out);
        emit(&out[..put]).map_err(|error| (taken, error))?;
        if used == 0 {
            if let Some(high) = lone_high_surrogate(&bytes[taken..]) {
                *pending = Some(high);
                taken += 2;
            }
            break;
        }
        taken += used;
    }
    Ok(taken)
}

/// How many bytes a translated `_write` that took `taken` bytes reports written: all of them,
/// less the 2 of the high surrogate it ended with (`pending`) when the descriptor has no place
/// to keep it until the next write (`keeps_pending` false), so the program writes those 2
/// bytes again rather than losing them.
pub fn reported_written(taken: usize, pending: Option<u16>, keeps_pending: bool) -> usize {
    match (pending, keeps_pending) {
        (Some(_), false) => taken.saturating_sub(2),
        _ => taken,
    }
}

/// What text mode made of the bytes one `_read` read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRead {
    /// How many bytes the program gets, at the buffer's start.
    pub kept: usize,
    /// The index among the bytes read of the Ctrl+Z that ended the text, if one did.
    pub stopped_at: Option<usize>,
    /// Whether the last byte read was a `\r` left out of `kept`: the byte after it decides
    /// whether it is a line's end (`\n`) or itself.
    pub return_pending: bool,
}

/// Turns the bytes `_read` read, all of `buffer`, into text mode's in place: `\r\n` becomes
/// `\n`, and a Ctrl+Z ends the text.
pub fn take_text(buffer: &mut [u8]) -> TextRead {
    let mut kept = 0;
    let mut index = 0;
    while index < buffer.len() {
        let byte = buffer[index];
        match byte {
            END_OF_TEXT => {
                return TextRead {
                    kept,
                    stopped_at: Some(index),
                    return_pending: false,
                };
            }
            CARRIAGE_RETURN if index + 1 == buffer.len() => {
                return TextRead {
                    kept,
                    stopped_at: None,
                    return_pending: true,
                };
            }
            CARRIAGE_RETURN if buffer[index + 1] == LINE_FEED => {
                buffer[kept] = LINE_FEED;
                index += 2;
            }
            _ => {
                buffer[kept] = byte;
                index += 1;
            }
        }
        kept += 1;
    }
    TextRead {
        kept,
        stopped_at: None,
        return_pending: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_flags_become_linux_s_and_unknown_ones_are_refused() {
        assert_eq!(open_flags(windows_flag::RDONLY), Some((0, false)));
        assert_eq!(
            open_flags(
                windows_flag::WRONLY
                    | windows_flag::CREAT
                    | windows_flag::TRUNC
                    | windows_flag::BINARY
            ),
            Some((
                linux_flag::WRONLY | linux_flag::CREAT | linux_flag::TRUNC,
                false
            ))
        );
        assert_eq!(
            open_flags(
                windows_flag::RDWR
                    | windows_flag::APPEND
                    | windows_flag::EXCL
                    | windows_flag::CREAT
                    | windows_flag::TEMPORARY
                    | windows_flag::NOINHERIT
            ),
            Some((
                linux_flag::RDWR | linux_flag::APPEND | linux_flag::EXCL | linux_flag::CREAT,
                true
            ))
        );
        assert_eq!(open_flags(windows_flag::WRONLY | windows_flag::RDWR), None);
        assert_eq!(open_flags(0x0800), None);
        assert_eq!(open_flags(0x0010_0000), None);
    }

    #[test]
    fn sharing_modes_and_origins_are_checked() {
        assert!(is_sharing_mode(share::DENY_NONE));
        assert!(is_sharing_mode(share::DENY_READ_WRITE));
        assert!(!is_sharing_mode(0));
        assert!(!is_sharing_mode(0x50));
        assert_eq!(whence(0), Some(0));
        assert_eq!(whence(2), Some(2));
        assert_eq!(whence(3), None);
        assert_eq!(whence(-1), None);
    }

    fn written(translation: Translation, to_terminal: bool, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = input;
        while !rest.is_empty() {
            let mut chunk = [0u8; 5];
            let (taken, put) = translate_written(translation, to_terminal, rest, &mut chunk);
            assert!(taken > 0, "{translation:?} stalled");
            out.extend_from_slice(&chunk[..put]);
            rest = &rest[taken..];
        }
        out
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn translation_modes_come_from_open_s_flags_or_the_default() {
        assert_eq!(
            open_mode(windows_flag::RDONLY, windows_flag::TEXT),
            windows_flag::TEXT
        );
        assert_eq!(
            open_mode(windows_flag::RDONLY, windows_flag::BINARY),
            windows_flag::BINARY
        );
        assert_eq!(
            open_mode(
                windows_flag::WRONLY | windows_flag::U16TEXT,
                windows_flag::TEXT
            ),
            windows_flag::U16TEXT
        );
        assert_eq!(
            Translation::of_mode(windows_flag::WTEXT),
            Translation::Utf16
        );
        assert_eq!(
            Translation::of_mode(windows_flag::U8TEXT),
            Translation::Utf8
        );
        assert_eq!(
            Translation::of_mode(windows_flag::BINARY),
            Translation::Binary
        );
        assert_eq!(Translation::of_mode(windows_flag::TEXT), Translation::Text);
        assert!(Translation::Utf8.is_wide() && !Translation::Text.is_wide());
    }

    #[test]
    fn writing_translates_line_ends_and_wide_text_in_whole_characters() {
        assert_eq!(written(Translation::Binary, false, b"a\nb"), b"a\nb");
        assert_eq!(
            written(Translation::Text, false, b"a\n\nb\n"),
            b"a\r\n\r\nb\r\n"
        );
        assert_eq!(
            written(Translation::Utf16, false, &utf16("é\n")),
            utf16("é\r\n")
        );
        assert_eq!(
            written(Translation::Utf8, false, &utf16("é😀\n")),
            "é😀\r\n".as_bytes()
        );
        assert_eq!(written(Translation::Utf16, true, &utf16("x\n")), b"x\r\n");
        assert_eq!(
            written(Translation::Utf8, false, &[0x00, 0xd8, b'a', 0]),
            "\u{fffd}a".as_bytes()
        );
        let mut small = [0u8; SMALLEST_TRANSLATION];
        assert_eq!(
            translate_written(Translation::Utf8, false, &utf16("😀"), &mut small),
            (4, 4)
        );
        let mut one = [0u8; 1];
        assert_eq!(
            translate_written(Translation::Text, false, b"\n", &mut one),
            (0, 0)
        );
    }

    fn written_in_writes(translation: Translation, writes: &[&[u8]]) -> (Vec<u8>, Option<u16>) {
        let mut file = Vec::new();
        let mut pending = None;
        for bytes in writes {
            let mut out = [0u8; SMALLEST_TRANSLATION];
            let taken =
                translate_write(translation, false, bytes, &mut pending, &mut out, |piece| {
                    file.extend_from_slice(piece);
                    Ok::<(), ()>(())
                });
            assert_eq!(taken, Ok(bytes.len()));
        }
        (file, pending)
    }

    #[test]
    fn a_character_split_between_two_wide_writes_is_written_whole() {
        let text = utf16("a😀\n");
        let (first, second) = text.split_at(4);
        assert_eq!(lone_high_surrogate(&first[2..]), Some(0xd83d));
        assert_eq!(
            translate_written(Translation::Utf8, false, first, &mut [0u8; 8]),
            (2, 1)
        );
        let (file, pending) = written_in_writes(Translation::Utf8, &[first, second]);
        assert_eq!(file, "a😀\r\n".as_bytes());
        assert_eq!(pending, None);
        let (file, pending) = written_in_writes(Translation::Utf8, &[first, &[][..], second]);
        assert_eq!(file, "a😀\r\n".as_bytes());
        assert_eq!(pending, None);
        let (file, pending) = written_in_writes(Translation::Utf8, &[first, &utf16("b")[..]]);
        assert_eq!(file, "a\u{fffd}b".as_bytes());
        assert_eq!(pending, None);
        let (file, pending) = written_in_writes(Translation::Utf8, &[first]);
        assert_eq!(file, b"a");
        assert_eq!(pending, Some(0xd83d));
        let (file, _) = written_in_writes(Translation::Utf16, &[first, second]);
        assert_eq!(file, utf16("a😀\r\n"));
    }

    #[test]
    fn a_high_surrogate_a_descriptor_cannot_keep_is_reported_unwritten() {
        let text = utf16("a😀");
        let first = &text[..4];
        let (file, pending) = written_in_writes(Translation::Utf8, &[first]);
        assert_eq!(file, b"a");
        assert_eq!(reported_written(first.len(), pending, true), 4);
        assert_eq!(reported_written(first.len(), pending, false), 2);
        let (_, pending) = written_in_writes(Translation::Utf8, &[&text[..]]);
        assert_eq!(reported_written(text.len(), pending, false), 6);
        let (_, pending) = written_in_writes(Translation::Utf8, &[&text[2..4]]);
        assert_eq!(reported_written(2, pending, false), 0);
    }

    #[test]
    fn reading_text_joins_line_ends_and_stops_at_ctrl_z() {
        let mut lines = *b"a\r\nb\rc\r\n";
        assert_eq!(
            take_text(&mut lines),
            TextRead {
                kept: 6,
                stopped_at: None,
                return_pending: false
            }
        );
        assert_eq!(&lines[..6], b"a\nb\rc\n");
        let mut ended = *b"ab\x1acd";
        let read = take_text(&mut ended);
        assert_eq!((read.kept, read.stopped_at), (2, Some(2)));
        let mut split = *b"ab\r";
        let read = take_text(&mut split);
        assert_eq!((read.kept, read.return_pending), (2, true));
        let mut only_return = *b"\r";
        assert_eq!(take_text(&mut only_return).kept, 0);
    }
}
