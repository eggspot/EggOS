//! Reading `.zip` archives (D148), from PKWARE's public APPNOTE: the end of central directory
//! record, the central directory's entries and each entry's local header, and stored (method 0)
//! or deflated (method 8, through our own inflate in `eggos-png`) data whose CRC-32 and size
//! are checked. No ZIP64, encryption, archives split over disks or other methods. An entry's
//! name must be a relative path of `/`-separated parts; one that is absolute or has an empty,
//! `.` or `..` part, a backslash, a colon or a NUL is refused, so extracting never writes
//! outside its folder. macOS's `__MACOSX/` metadata entries are recognised so they can be
//! skipped.

use crate::reader::{RangeBytes, Reader};
use eggos_png::crc32_update;
use eggos_png::inflate::{InflateError, Sink, Window, inflate};

/// The longest entry name read, in bytes.
pub const NAME_CAPACITY: usize = 255;

const END_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const END_BYTES: usize = 22;
const CENTRAL_BYTES: usize = 46;
const LOCAL_BYTES: usize = 30;
const MAX_COMMENT: u64 = 0xffff;
const SEARCH_BYTES: usize = 1024;
const ENCRYPTED: u16 = 1;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
const UNIX: u8 = 3;
const FILE_TYPE: u32 = 0o170_000;
const DIRECTORY_TYPE: u32 = 0o040_000;
const REGULAR_TYPE: u32 = 0o100_000;
const ZIP64_COUNT: u16 = 0xffff;
const ZIP64_SIZE: u32 = 0xffff_ffff;
const MAC_METADATA: &[u8] = b"__MACOSX";
const COPY_BYTES: usize = 4096;

/// Why an archive could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveError {
    /// No end of central directory record: not a `.zip`.
    NotArchive,
    /// ZIP64, encryption, several disks, a compression method other than stored and deflate,
    /// or an entry that is a symbolic link or a device.
    Unsupported,
    /// An entry's name could write outside the folder it is extracted into, or is too long.
    UnsafeName,
    /// A record, the compressed data, its size or its CRC-32 does not match what the archive
    /// says.
    Corrupt,
    /// The archive could not be read.
    Unreadable,
    /// The sink refused the entry's data.
    Stopped,
}

/// The first bytes of a `.zip`: a local header, or the end record of an empty archive.
pub fn is_archive(head: &[u8]) -> bool {
    head.starts_with(&LOCAL_SIGNATURE.to_le_bytes())
        || head.starts_with(&END_SIGNATURE.to_le_bytes())
}

/// Whether `name` (an entry's path without its trailing `/`) stays inside the folder it is
/// extracted into.
pub fn is_safe_name(name: &[u8]) -> bool {
    !name.is_empty()
        && !name.iter().any(|&byte| matches!(byte, b'\\' | b':' | 0))
        && name
            .split(|&byte| byte == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b"..")
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// An archive whose central directory was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archive {
    size: u64,
    directory: u64,
    directory_end: u64,
    entries: u16,
}

/// Where [`Archive::next`] reads the next entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    offset: u64,
    index: u16,
}

/// One entry of the central directory.
#[derive(Clone, Copy)]
pub struct Entry {
    name: [u8; NAME_CAPACITY],
    name_length: usize,
    directory: bool,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    local: u64,
    permissions: Option<u32>,
}

impl core::fmt::Debug for Entry {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Entry")
            .field("name", &self.name().escape_ascii())
            .field("directory", &self.directory)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// Its path inside the archive, without a trailing `/`.
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_length]
    }

    /// Whether it is a folder.
    pub fn is_directory(&self) -> bool {
        self.directory
    }

    /// Its size once extracted.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The permission bits the archive records (from Unix), if any.
    pub fn permissions(&self) -> Option<u32> {
        self.permissions
    }

    /// Whether it is macOS's metadata (`__MACOSX/...`), which is not part of the app.
    pub fn is_metadata(&self) -> bool {
        self.name().split(|&byte| byte == b'/').next() == Some(MAC_METADATA)
    }
}

impl Archive {
    /// Finds the archive's end record (searching back over a comment of up to 64 KiB) and
    /// checks it.
    pub fn open(reader: &mut impl Reader) -> Result<Self, ArchiveError> {
        let size = reader.size();
        if size < END_BYTES as u64 {
            return Err(ArchiveError::NotArchive);
        }
        let lowest = size.saturating_sub(END_BYTES as u64 + MAX_COMMENT);
        let mut chunk = [0u8; SEARCH_BYTES];
        let mut end = size;
        loop {
            let start = end.saturating_sub(SEARCH_BYTES as u64).max(lowest);
            let length = (end - start) as usize;
            if !reader.read_exact_at(start, &mut chunk[..length]) {
                return Err(ArchiveError::Unreadable);
            }
            for at in (0..length.saturating_sub(3)).rev() {
                let position = start + at as u64;
                if position + END_BYTES as u64 <= size && u32_at(&chunk, at) == END_SIGNATURE {
                    return Self::at_end_record(reader, position, size);
                }
            }
            if start == lowest {
                return Err(ArchiveError::NotArchive);
            }
            end = start + 3;
        }
    }

    fn at_end_record(
        reader: &mut impl Reader,
        position: u64,
        size: u64,
    ) -> Result<Self, ArchiveError> {
        let mut record = [0u8; END_BYTES];
        if !reader.read_exact_at(position, &mut record) {
            return Err(ArchiveError::Unreadable);
        }
        let (disk, directory_disk) = (u16_at(&record, 4), u16_at(&record, 6));
        let (on_disk, entries) = (u16_at(&record, 8), u16_at(&record, 10));
        let (directory_size, directory) = (u32_at(&record, 12), u32_at(&record, 16));
        if entries == ZIP64_COUNT || directory_size == ZIP64_SIZE || directory == ZIP64_SIZE {
            return Err(ArchiveError::Unsupported);
        }
        if disk != 0 || directory_disk != 0 || on_disk != entries {
            return Err(ArchiveError::Unsupported);
        }
        let directory = u64::from(directory);
        let directory_end = directory + u64::from(directory_size);
        if directory_end > position {
            return Err(ArchiveError::Corrupt);
        }
        Ok(Self {
            size,
            directory,
            directory_end,
            entries,
        })
    }

    /// The archive file's size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How many entries the central directory holds.
    pub fn entries(&self) -> u16 {
        self.entries
    }

    /// A cursor at the first entry.
    pub fn first(&self) -> Cursor {
        Cursor {
            offset: self.directory,
            index: 0,
        }
    }

    /// The entry at `cursor`, which moves to the next; `None` after the last.
    pub fn next(
        &self,
        reader: &mut impl Reader,
        cursor: &mut Cursor,
    ) -> Result<Option<Entry>, ArchiveError> {
        if cursor.index >= self.entries {
            return Ok(None);
        }
        let mut header = [0u8; CENTRAL_BYTES];
        if cursor.offset + CENTRAL_BYTES as u64 > self.directory_end
            || !reader.read_exact_at(cursor.offset, &mut header)
            || u32_at(&header, 0) != CENTRAL_SIGNATURE
        {
            return Err(ArchiveError::Corrupt);
        }
        let host = header[5];
        let (flags, method) = (u16_at(&header, 8), u16_at(&header, 10));
        let crc = u32_at(&header, 16);
        let (compressed, size) = (u32_at(&header, 20), u32_at(&header, 24));
        let name_length = usize::from(u16_at(&header, 28));
        let rest = u64::from(u16_at(&header, 30)) + u64::from(u16_at(&header, 32));
        let external = u32_at(&header, 38);
        let local = u32_at(&header, 42);
        let mut name = [0u8; NAME_CAPACITY];
        if name_length > NAME_CAPACITY {
            return Err(ArchiveError::UnsafeName);
        }
        if !reader.read_exact_at(
            cursor.offset + CENTRAL_BYTES as u64,
            &mut name[..name_length],
        ) {
            return Err(ArchiveError::Corrupt);
        }
        cursor.offset += CENTRAL_BYTES as u64 + name_length as u64 + rest;
        cursor.index += 1;
        if cursor.offset > self.directory_end {
            return Err(ArchiveError::Corrupt);
        }
        if [compressed, size, local].contains(&ZIP64_SIZE)
            || flags & ENCRYPTED != 0
            || !matches!(method, STORED | DEFLATED)
        {
            return Err(ArchiveError::Unsupported);
        }
        let mut length = name_length;
        let mut directory = false;
        if length > 0 && name[length - 1] == b'/' {
            length -= 1;
            directory = true;
        }
        if !is_safe_name(&name[..length]) {
            return Err(ArchiveError::UnsafeName);
        }
        let mode = (host == UNIX)
            .then_some(external >> 16)
            .filter(|&mode| mode != 0);
        match mode.map(|mode| mode & FILE_TYPE) {
            None | Some(0) | Some(REGULAR_TYPE) => {}
            Some(DIRECTORY_TYPE) => directory = true,
            Some(_) => return Err(ArchiveError::Unsupported),
        }
        Ok(Some(Entry {
            name,
            name_length: length,
            directory,
            method,
            crc,
            compressed: u64::from(compressed),
            size: u64::from(size),
            local: u64::from(local),
            permissions: mode.map(|mode| mode & 0o777),
        }))
    }
}

fn data_range(reader: &mut impl Reader, entry: &Entry) -> Result<(u64, u64), ArchiveError> {
    let mut header = [0u8; LOCAL_BYTES];
    if !reader.read_exact_at(entry.local, &mut header) || u32_at(&header, 0) != LOCAL_SIGNATURE {
        return Err(ArchiveError::Corrupt);
    }
    let start = entry.local
        + LOCAL_BYTES as u64
        + u64::from(u16_at(&header, 26))
        + u64::from(u16_at(&header, 28));
    let end = start + entry.compressed;
    match end <= reader.size() {
        true => Ok((start, end)),
        false => Err(ArchiveError::Corrupt),
    }
}

struct Checked<'k, K: Sink> {
    sink: &'k mut K,
    crc: u32,
    written: u64,
    limit: u64,
    too_long: bool,
}

impl<K: Sink> Sink for Checked<'_, K> {
    fn write(&mut self, bytes: &[u8]) -> bool {
        self.written += bytes.len() as u64;
        if self.written > self.limit {
            self.too_long = true;
            return false;
        }
        self.crc = crc32_update(self.crc, bytes);
        self.sink.write(bytes)
    }
}

impl<K: Sink> Checked<'_, K> {
    fn refusal(&self) -> ArchiveError {
        match self.too_long {
            true => ArchiveError::Corrupt,
            false => ArchiveError::Stopped,
        }
    }
}

/// Writes `entry`'s data, extracted, to `sink`, inflating through `window`; its size and
/// CRC-32 are checked once it is all written.
pub fn extract<R: Reader, K: Sink>(
    reader: &mut R,
    entry: &Entry,
    window: &mut Window,
    sink: &mut K,
) -> Result<(), ArchiveError> {
    let (start, end) = data_range(reader, entry)?;
    let mut checked = Checked {
        sink,
        crc: 0,
        written: 0,
        limit: entry.size,
        too_long: false,
    };
    match entry.method {
        STORED => {
            if entry.compressed != entry.size {
                return Err(ArchiveError::Corrupt);
            }
            let mut buffer = [0u8; COPY_BYTES];
            let mut at = start;
            while at < end {
                let want = (end - at).min(COPY_BYTES as u64) as usize;
                if !reader.read_exact_at(at, &mut buffer[..want]) {
                    return Err(ArchiveError::Unreadable);
                }
                if !checked.write(&buffer[..want]) {
                    return Err(checked.refusal());
                }
                at += want as u64;
            }
        }
        _ => {
            let mut source = RangeBytes::new(reader, start, end);
            if let Err(error) = inflate(&mut source, window, &mut checked) {
                return Err(match error {
                    InflateError::Stopped => checked.refusal(),
                    _ => ArchiveError::Corrupt,
                });
            }
        }
    }
    match checked.written == entry.size && checked.crc == entry.crc {
        true => Ok(()),
        false => Err(ArchiveError::Corrupt),
    }
}

struct Head<'h> {
    head: &'h mut [u8],
    filled: usize,
}

impl Sink for Head<'_> {
    fn write(&mut self, bytes: &[u8]) -> bool {
        let take = (self.head.len() - self.filled).min(bytes.len());
        self.head[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
        self.filled += take;
        self.filled < self.head.len()
    }
}

/// Reads the first bytes of `entry`, extracted, into `head`: how many (fewer when the entry is
/// shorter).
pub fn head<R: Reader>(
    reader: &mut R,
    entry: &Entry,
    window: &mut Window,
    head: &mut [u8],
) -> Result<usize, ArchiveError> {
    let (start, end) = data_range(reader, entry)?;
    if entry.method == STORED {
        let want = (end - start).min(head.len() as u64) as usize;
        return match reader.read_exact_at(start, &mut head[..want]) {
            true => Ok(want),
            false => Err(ArchiveError::Unreadable),
        };
    }
    let mut filler = Head { head, filled: 0 };
    let mut source = RangeBytes::new(reader, start, end);
    match inflate(&mut source, window, &mut filler) {
        Ok(_) | Err(InflateError::Stopped) => Ok(filler.filled),
        Err(_) => Err(ArchiveError::Corrupt),
    }
}

#[cfg(test)]
pub(crate) mod samples {
    pub struct Member<'a> {
        pub name: &'a str,
        pub data: &'a [u8],
        pub deflated: Option<Vec<u8>>,
        pub mode: Option<u32>,
    }

    pub fn stored(name: &str, data: &[u8]) -> Member<'static> {
        Member {
            name: Box::leak(name.to_owned().into_boxed_str()),
            data: Box::leak(data.to_vec().into_boxed_slice()),
            deflated: None,
            mode: None,
        }
    }

    pub fn deflate_stored_block(data: &[u8]) -> Vec<u8> {
        let mut stream = vec![1];
        let length = data.len() as u16;
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(data);
        stream
    }

    pub fn deflate_fixed_literals(data: &[u8]) -> Vec<u8> {
        let (mut bytes, mut bits, mut count) = (Vec::new(), 0u64, 0u32);
        let mut put = |value: u32, length: u32| {
            bits |= u64::from(value) << count;
            count += length;
            while count >= 8 {
                bytes.push(bits as u8);
                bits >>= 8;
                count -= 8;
            }
        };
        put(1, 1);
        put(1, 2);
        for &byte in data {
            let (code, length) = match byte {
                0..=143 => (0x30 + u32::from(byte), 8),
                _ => (0x190 + u32::from(byte) - 144, 9),
            };
            put(code.reverse_bits() >> (32 - length), length);
        }
        put(0, 7);
        put(0, 7);
        bytes
    }

    pub fn deflated(name: &str, data: &[u8], stream: Vec<u8>) -> Member<'static> {
        Member {
            deflated: Some(stream),
            ..stored(name, data)
        }
    }

    pub fn zip(members: &[Member<'_>]) -> Vec<u8> {
        let mut file = Vec::new();
        let mut central = Vec::new();
        for member in members {
            let local = file.len() as u32;
            let crc = eggos_png::crc32(member.data);
            let (method, body): (u16, &[u8]) = match &member.deflated {
                Some(stream) => (8, stream.as_slice()),
                None => (0, member.data),
            };
            let name = member.name.as_bytes();
            let mut fields = Vec::new();
            fields.extend_from_slice(&20u16.to_le_bytes());
            fields.extend_from_slice(&0u16.to_le_bytes());
            fields.extend_from_slice(&method.to_le_bytes());
            fields.extend_from_slice(&[0, 0, 0, 0]);
            fields.extend_from_slice(&crc.to_le_bytes());
            fields.extend_from_slice(&(body.len() as u32).to_le_bytes());
            fields.extend_from_slice(&(member.data.len() as u32).to_le_bytes());
            fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
            fields.extend_from_slice(&0u16.to_le_bytes());
            file.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            file.extend_from_slice(&fields);
            file.extend_from_slice(name);
            file.extend_from_slice(body);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            let made_by: u16 = match member.mode {
                Some(_) => 3 << 8 | 20,
                None => 20,
            };
            central.extend_from_slice(&made_by.to_le_bytes());
            central.extend_from_slice(&fields);
            central.extend_from_slice(&[0; 6]);
            central.extend_from_slice(&(member.mode.unwrap_or(0) << 16).to_le_bytes());
            central.extend_from_slice(&local.to_le_bytes());
            central.extend_from_slice(name);
        }
        let directory = file.len() as u32;
        file.extend_from_slice(&central);
        file.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        file.extend_from_slice(&[0, 0, 0, 0]);
        file.extend_from_slice(&(members.len() as u16).to_le_bytes());
        file.extend_from_slice(&(members.len() as u16).to_le_bytes());
        file.extend_from_slice(&(central.len() as u32).to_le_bytes());
        file.extend_from_slice(&directory.to_le_bytes());
        file.extend_from_slice(&0u16.to_le_bytes());
        file
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;
    use crate::reader::Bytes;

    struct Collect(Vec<u8>);

    impl Sink for Collect {
        fn write(&mut self, bytes: &[u8]) -> bool {
            self.0.extend_from_slice(bytes);
            true
        }
    }

    fn entries(zip: &[u8]) -> Result<Vec<Entry>, ArchiveError> {
        let mut reader = Bytes(zip);
        let archive = Archive::open(&mut reader)?;
        let mut cursor = archive.first();
        let mut found = Vec::new();
        while let Some(entry) = archive.next(&mut reader, &mut cursor)? {
            found.push(entry);
        }
        assert_eq!(found.len(), usize::from(archive.entries()));
        Ok(found)
    }

    fn extracted(zip: &[u8], entry: &Entry) -> Result<Vec<u8>, ArchiveError> {
        let mut window = Box::new(Window::new());
        let mut sink = Collect(Vec::new());
        extract(&mut Bytes(zip), entry, &mut window, &mut sink)?;
        Ok(sink.0)
    }

    #[test]
    fn stored_and_deflated_entries_extract_with_their_names_and_modes() {
        let text = b"hello from inside the archive";
        let zip = zip(&[
            Member {
                mode: Some(0o040_755),
                ..stored("Paint.app/", b"")
            },
            Member {
                mode: Some(0o100_755),
                ..stored(
                    "Paint.app/Contents/MacOS/Paint",
                    b"\xcf\xfa\xed\xfe program",
                )
            },
            deflated("notes.txt", text, deflate_fixed_literals(text)),
            deflated(
                "raw.bin",
                b"\x00\xff\x10",
                deflate_stored_block(b"\x00\xff\x10"),
            ),
        ]);
        assert!(is_archive(&zip));
        let found = entries(&zip).unwrap();
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].name(), b"Paint.app");
        assert!(found[0].is_directory());
        assert_eq!(found[1].name(), b"Paint.app/Contents/MacOS/Paint");
        assert_eq!(found[1].permissions(), Some(0o755));
        assert_eq!(found[2].permissions(), None);
        assert_eq!(
            extracted(&zip, &found[1]).unwrap(),
            b"\xcf\xfa\xed\xfe program"
        );
        assert_eq!(extracted(&zip, &found[2]).unwrap(), text);
        assert_eq!(found[2].size(), text.len() as u64);
        assert_eq!(extracted(&zip, &found[3]).unwrap(), b"\x00\xff\x10");
        let mut window = Box::new(Window::new());
        let mut first = [0u8; 5];
        assert_eq!(
            head(&mut Bytes(&zip), &found[2], &mut window, &mut first),
            Ok(5)
        );
        assert_eq!(&first, b"hello");
        let mut more = [0u8; 64];
        assert_eq!(
            head(&mut Bytes(&zip), &found[3], &mut window, &mut more),
            Ok(3)
        );
    }

    #[test]
    fn names_that_leave_the_folder_are_refused() {
        for name in [
            "/etc/passwd",
            "../up",
            "a/../../b",
            "a//b",
            "./a",
            "C:/x",
            "a\\b",
            "",
        ] {
            assert!(!is_safe_name(name.as_bytes()), "{name}");
            assert_eq!(
                entries(&zip(&[stored(name, b"x")])).err(),
                Some(ArchiveError::UnsafeName),
                "{name}"
            );
        }
        assert!(is_safe_name(b"a/b.c/d"));
        assert!(is_safe_name(b"..x"));
        let link = Member {
            mode: Some(0o120_777),
            ..stored("link", b"target")
        };
        assert_eq!(
            entries(&zip(&[link])).err(),
            Some(ArchiveError::Unsupported)
        );
    }

    #[test]
    fn a_comment_after_the_end_record_and_metadata_entries_are_handled() {
        let mut archive = zip(&[stored("__MACOSX/._a", b"meta"), stored("a", b"data")]);
        let comment_length = 2000u16;
        let at = archive.len() - 2;
        archive[at..].copy_from_slice(&comment_length.to_le_bytes());
        archive.extend(std::iter::repeat_n(b'c', usize::from(comment_length)));
        let found = entries(&archive).unwrap();
        assert!(found[0].is_metadata());
        assert!(!found[1].is_metadata());
        assert_eq!(extracted(&archive, &found[1]).unwrap(), b"data");
    }

    #[test]
    fn damaged_archives_are_errors() {
        assert_eq!(
            entries(b"not a zip at all, just text"),
            Err(ArchiveError::NotArchive)
        );
        assert_eq!(entries(b""), Err(ArchiveError::NotArchive));
        let good = zip(&[stored("a", b"abcdef")]);
        let found = entries(&good).unwrap();
        let mut wrong_crc = good.clone();
        wrong_crc[30 + 1 + 2] ^= 1;
        assert_eq!(extracted(&wrong_crc, &found[0]), Err(ArchiveError::Corrupt));
        let lying = zip(&[deflated("a", b"abc", deflate_fixed_literals(b"abcd"))]);
        let found = entries(&lying).unwrap();
        assert_eq!(extracted(&lying, &found[0]), Err(ArchiveError::Corrupt));
        let mut encrypted = zip(&[stored("a", b"x")]);
        let central = encrypted.len() - 22 - 47;
        encrypted[central + 8] = 1;
        assert_eq!(entries(&encrypted), Err(ArchiveError::Unsupported));
        let mut other_method = zip(&[stored("a", b"x")]);
        other_method[central + 10] = 12;
        assert_eq!(entries(&other_method), Err(ArchiveError::Unsupported));
        let mut cut = good.clone();
        cut.truncate(good.len() - 1);
        assert!(entries(&cut).is_err());
        let mut state = 0x1234_5678u32;
        for _ in 0..500 {
            let mut noisy = good.clone();
            for _ in 0..3 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let at = state as usize % noisy.len();
                noisy[at] ^= (state >> 24) as u8 | 1;
            }
            if let Ok(found) = entries(&noisy) {
                for entry in &found {
                    let _ = extracted(&noisy, entry);
                }
            }
        }
    }
}
