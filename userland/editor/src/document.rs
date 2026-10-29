//! A document's text (docs/ux/text-editor-v0.3.md section 5): UTF-8 bytes with `\n` line
//! breaks in memory the caller gives, at most [`TEXT_CAPACITY`] bytes, kept in one piece so
//! drawing and finding read plain slices, and the index of where each line starts, kept up to
//! date by every edit. [`Document::load`] turns a file's bytes into a document (the byte-order
//! mark and CRLF line endings are taken out and remembered) and [`Document::encode`] gives them
//! back as they were.

use crate::file::TextCheck;

/// The most bytes a document holds: 4 MiB.
pub const TEXT_CAPACITY: usize = 4 * 1024 * 1024;
/// The most lines a document of [`TEXT_CAPACITY`] bytes can have.
pub const LINE_CAPACITY: usize = TEXT_CAPACITY + 1;
/// UTF-8's byte-order mark.
pub const BYTE_ORDER_MARK: &[u8] = b"\xef\xbb\xbf";
const ENCODE_CHUNK: usize = 4096;
const PLAIN_CHUNK: usize = 64 * 1024;

/// How the file ends its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n`, as Linux and macOS write.
    Lf,
    /// `\r\n`, as Windows writes.
    CrLf,
}

impl LineEnding {
    /// What the status bar says.
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
        }
    }
}

/// An edit would pass the document's room for bytes or lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// Bytes that are not text: not UTF-8, or holding a NUL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotText;

/// A document. See the module documentation.
pub struct Document<'a> {
    text: &'a mut [u8],
    length: usize,
    starts: &'a mut [u32],
    lines: usize,
    ending: LineEnding,
    marked: bool,
}

impl core::fmt::Debug for Document<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Document")
            .field("length", &self.length)
            .field("lines", &self.lines)
            .field("ending", &self.ending)
            .finish_non_exhaustive()
    }
}

/// Whether `byte` continues a UTF-8 character rather than starting one.
pub fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

fn breaks(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte == b'\n').count()
}

impl<'a> Document<'a> {
    /// An empty document in `text` (its length is the room for bytes) with `starts` for the
    /// line index (its length is the room for lines, at least 1).
    pub fn new(text: &'a mut [u8], starts: &'a mut [u32]) -> Self {
        let mut document = Self {
            text,
            length: 0,
            starts,
            lines: 1,
            ending: LineEnding::Lf,
            marked: false,
        };
        document.clear();
        document
    }

    /// Empties it: no text, one empty line, LF, no byte-order mark.
    pub fn clear(&mut self) {
        self.length = 0;
        if let Some(first) = self.starts.first_mut() {
            *first = 0;
        }
        self.lines = 1;
        self.ending = LineEnding::Lf;
        self.marked = false;
    }

    /// The most bytes it holds.
    pub fn capacity(&self) -> usize {
        self.text.len()
    }

    /// Its text.
    pub fn bytes(&self) -> &[u8] {
        &self.text[..self.length]
    }

    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether it holds no text.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// How many lines it has: one more than its line breaks.
    pub fn line_count(&self) -> usize {
        self.lines
    }

    /// Where line `line` starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.starts[line.min(self.lines - 1)] as usize
    }

    /// Where line `line` ends, before its line break.
    pub fn line_end(&self, line: usize) -> usize {
        match line + 1 < self.lines {
            true => self.starts[line + 1] as usize - 1,
            false => self.length,
        }
    }

    /// Line `line`'s bytes, without its line break.
    pub fn line(&self, line: usize) -> &[u8] {
        &self.text[self.line_start(line)..self.line_end(line)]
    }

    /// The line byte `offset` is on.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts[..self.lines].partition_point(|&start| start as usize <= offset) - 1
    }

    /// How the file it came from ends its lines, which saving keeps.
    pub fn ending(&self) -> LineEnding {
        self.ending
    }

    /// Whether the file it came from began with a byte-order mark, which saving keeps.
    pub fn has_byte_order_mark(&self) -> bool {
        self.marked
    }

    /// The previous character boundary before `at`, or 0.
    pub fn previous_boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.length);
        while at > 0 {
            at -= 1;
            if !is_continuation(self.text[at]) {
                break;
            }
        }
        at
    }

    /// The next character boundary after `at`, or the end.
    pub fn next_boundary(&self, at: usize) -> usize {
        let mut at = at;
        if at >= self.length {
            return self.length;
        }
        at += 1;
        while at < self.length && is_continuation(self.text[at]) {
            at += 1;
        }
        at
    }

    /// Whether replacing `removed` bytes with `inserted` fits its room for bytes and lines.
    pub fn fits(&self, removed: &[u8], inserted: &[u8]) -> bool {
        let length = self.length - removed.len() + inserted.len();
        let lines = self.lines - breaks(removed) + breaks(inserted);
        length <= self.text.len() && lines <= self.starts.len()
    }

    /// Puts `bytes` (whole characters) at byte `at` (a character boundary).
    pub fn insert(&mut self, at: usize, bytes: &[u8]) -> Result<(), Full> {
        let at = at.min(self.length);
        let end = self
            .length
            .checked_add(bytes.len())
            .filter(|&end| end <= self.text.len())
            .ok_or(Full)?;
        let added = breaks(bytes);
        let lines = self.lines + added;
        if lines > self.starts.len() {
            return Err(Full);
        }
        let line = self.line_of(at);
        self.text.copy_within(at..self.length, at + bytes.len());
        self.text[at..at + bytes.len()].copy_from_slice(bytes);
        self.length = end;
        self.starts
            .copy_within(line + 1..self.lines, line + 1 + added);
        let shift = bytes.len() as u32;
        for start in &mut self.starts[line + 1 + added..lines] {
            *start += shift;
        }
        let mut slot = line + 1;
        for (index, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' {
                self.starts[slot] = (at + index + 1) as u32;
                slot += 1;
            }
        }
        self.lines = lines;
        Ok(())
    }

    /// Takes out the bytes from `start` to `end` (character boundaries).
    pub fn remove(&mut self, start: usize, end: usize) {
        let end = end.min(self.length);
        if start >= end {
            return;
        }
        let first = self.line_of(start);
        let last = self.line_of(end);
        self.text.copy_within(end..self.length, start);
        self.length -= end - start;
        self.starts.copy_within(last + 1..self.lines, first + 1);
        self.lines -= last - first;
        let shift = (end - start) as u32;
        for line_start in &mut self.starts[first + 1..self.lines] {
            *line_start -= shift;
        }
    }

    /// The memory a file is read into before [`Self::load`]: all of the document's room.
    pub fn room(&mut self) -> &mut [u8] {
        &mut self.text[..]
    }

    /// Makes the document the file whose `length` bytes [`Self::room`] holds: a leading
    /// byte-order mark is taken out and remembered, and when the text holds `\r\n` anywhere
    /// the document is CRLF and every `\r\n` becomes `\n`; a lone `\r` stays. Bytes that are
    /// not UTF-8 or hold a NUL leave it empty.
    pub fn load(&mut self, length: usize) -> Result<(), NotText> {
        let mut length = length.min(self.text.len());
        let marked = self.text[..length].starts_with(BYTE_ORDER_MARK);
        if marked {
            self.text.copy_within(BYTE_ORDER_MARK.len()..length, 0);
            length -= BYTE_ORDER_MARK.len();
        }
        let mut check = TextCheck::new();
        check.feed(&self.text[..length]);
        if !check.finish() {
            self.clear();
            return Err(NotText);
        }
        let crlf = self.text[..length].windows(2).any(|pair| pair == b"\r\n");
        if crlf {
            let mut kept = 0;
            let mut read = 0;
            while read < length {
                let byte = self.text[read];
                read += 1;
                if byte == b'\r' && read < length && self.text[read] == b'\n' {
                    continue;
                }
                self.text[kept] = byte;
                kept += 1;
            }
            length = kept;
        }
        if breaks(&self.text[..length]) + 1 > self.starts.len() {
            self.clear();
            return Err(NotText);
        }
        self.length = length;
        self.starts[0] = 0;
        let mut lines = 1;
        for (at, &byte) in self.text[..length].iter().enumerate() {
            if byte == b'\n' {
                self.starts[lines] = (at + 1) as u32;
                lines += 1;
            }
        }
        self.lines = lines;
        self.ending = match crlf {
            true => LineEnding::CrLf,
            false => LineEnding::Lf,
        };
        self.marked = marked;
        Ok(())
    }

    /// Hands `write` the file's bytes in pieces: the byte-order mark if it had one, then the
    /// text, each `\n` written `\r\n` in a CRLF document; stops at `write`'s first error.
    pub fn encode<E>(&self, mut write: impl FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        if self.marked {
            write(BYTE_ORDER_MARK)?;
        }
        if self.ending == LineEnding::Lf {
            for piece in self.bytes().chunks(PLAIN_CHUNK) {
                write(piece)?;
            }
            return Ok(());
        }
        let mut chunk = [0u8; ENCODE_CHUNK];
        let mut used = 0;
        for &byte in self.bytes() {
            if used + 2 > ENCODE_CHUNK {
                write(&chunk[..used])?;
                used = 0;
            }
            if byte == b'\n' {
                chunk[used] = b'\r';
                used += 1;
            }
            chunk[used] = byte;
            used += 1;
        }
        if used > 0 {
            write(&chunk[..used])?;
        }
        Ok(())
    }

    /// Forgets the file's byte-order mark and line ending: a new document's are LF and none.
    pub fn reset_format(&mut self) {
        self.ending = LineEnding::Lf;
        self.marked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Room {
        text: Vec<u8>,
        starts: Vec<u32>,
    }

    impl Room {
        fn new(bytes: usize, lines: usize) -> Self {
            Self {
                text: vec![0; bytes],
                starts: vec![0; lines],
            }
        }

        fn document(&mut self) -> Document<'_> {
            Document::new(&mut self.text, &mut self.starts)
        }
    }

    fn loaded<'a>(room: &'a mut Room, bytes: &[u8]) -> Document<'a> {
        let mut document = room.document();
        document.room()[..bytes.len()].copy_from_slice(bytes);
        document.load(bytes.len()).unwrap();
        document
    }

    fn lines(document: &Document<'_>) -> Vec<String> {
        (0..document.line_count())
            .map(|line| String::from_utf8(document.line(line).to_vec()).unwrap())
            .collect()
    }

    fn encoded(document: &Document<'_>) -> Vec<u8> {
        let mut out = Vec::new();
        document
            .encode(|piece| {
                out.extend_from_slice(piece);
                Ok::<(), ()>(())
            })
            .unwrap();
        out
    }

    #[test]
    fn an_empty_document_has_one_empty_line() {
        let mut room = Room::new(16, 4);
        let document = room.document();
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.line(0), b"");
        assert_eq!(document.line_of(0), 0);
        assert!(document.is_empty());
    }

    #[test]
    fn inserting_and_removing_keep_the_line_index() {
        let mut room = Room::new(64, 16);
        let mut document = room.document();
        document.insert(0, b"one\nthree").unwrap();
        assert_eq!(lines(&document), ["one", "three"]);
        document.insert(4, b"two\n").unwrap();
        assert_eq!(lines(&document), ["one", "two", "three"]);
        assert_eq!(document.line_start(2), 8);
        assert_eq!(document.line_of(8), 2);
        assert_eq!(document.line_of(7), 1);
        document.insert(document.len(), b"\n").unwrap();
        assert_eq!(lines(&document), ["one", "two", "three", ""]);
        document.remove(2, 9);
        assert_eq!(lines(&document), ["onhree", ""]);
        document.remove(0, document.len());
        assert_eq!(lines(&document), [""]);
        document.insert(0, b"\n\n").unwrap();
        assert_eq!(lines(&document), ["", "", ""]);
        document.remove(1, 2);
        assert_eq!(lines(&document), ["", ""]);
    }

    #[test]
    fn an_edit_past_the_room_is_refused_and_changes_nothing() {
        let mut room = Room::new(8, 3);
        let mut document = room.document();
        document.insert(0, b"abcdef").unwrap();
        assert_eq!(document.insert(0, b"xyz"), Err(Full));
        assert_eq!(document.insert(0, b"\n\n\n"), Err(Full));
        assert!(!document.fits(b"", b"\n\n\n"));
        assert!(document.fits(b"abc", b"x\ny"));
        assert_eq!(document.bytes(), b"abcdef");
        assert_eq!(document.line_count(), 1);
    }

    #[test]
    fn boundaries_step_over_whole_characters() {
        let mut room = Room::new(32, 4);
        let document = loaded(&mut room, "aé€b".as_bytes());
        assert_eq!(document.next_boundary(0), 1);
        assert_eq!(document.next_boundary(1), 3);
        assert_eq!(document.next_boundary(3), 6);
        assert_eq!(document.previous_boundary(6), 3);
        assert_eq!(document.previous_boundary(3), 1);
        assert_eq!(document.previous_boundary(0), 0);
        assert_eq!(document.next_boundary(7), 7);
    }

    #[test]
    fn a_crlf_file_loads_as_lf_and_saves_as_crlf_with_its_mark() {
        let file = b"\xef\xbb\xbfone\r\ntwo\rthree\r\n";
        let mut room = Room::new(64, 8);
        let document = loaded(&mut room, file);
        assert_eq!(document.ending(), LineEnding::CrLf);
        assert!(document.has_byte_order_mark());
        assert_eq!(document.bytes(), b"one\ntwo\rthree\n");
        assert_eq!(lines(&document), ["one", "two\rthree", ""]);
        assert_eq!(encoded(&document), file);
        assert_eq!(document.ending().label(), "CRLF");
    }

    #[test]
    fn an_lf_file_saves_as_it_was() {
        let mut room = Room::new(64, 8);
        let document = loaded(&mut room, b"a\nb\r");
        assert_eq!(document.ending(), LineEnding::Lf);
        assert!(!document.has_byte_order_mark());
        assert_eq!(encoded(&document), b"a\nb\r");
    }

    #[test]
    fn a_long_crlf_text_is_written_in_pieces_that_never_split_a_pair() {
        let mut room = Room::new(64 * 1024, 64 * 1024);
        let mut file = Vec::new();
        for _ in 0..5000 {
            file.extend_from_slice(b"line\r\n");
        }
        let document = loaded(&mut room, &file);
        let mut pieces = Vec::new();
        document
            .encode(|piece| {
                pieces.push(piece.to_vec());
                Ok::<(), ()>(())
            })
            .unwrap();
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|piece| piece.len() <= 4096));
        assert_eq!(pieces.concat(), file);
    }

    #[test]
    fn bytes_that_are_not_text_are_refused() {
        for bytes in [&b"a\0b"[..], b"\xff", b"\xc3", b"\xed\xa0\x80"] {
            let mut room = Room::new(16, 4);
            let mut document = room.document();
            document.room()[..bytes.len()].copy_from_slice(bytes);
            assert_eq!(document.load(bytes.len()), Err(NotText), "{bytes:?}");
            assert!(document.is_empty());
        }
    }
}
