//! MCP's stdio framing: one JSON-RPC message per line, lines ended by `\n` (a `\r` before it
//! is dropped), empty lines skipped. [`LineReader`] gathers the bytes reads bring into whole
//! lines in a fixed buffer; a line longer than the buffer is dropped up to its end and
//! reported once ([`Line::TooLong`]), so the next line is read as it should be.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// What [`LineReader::next_line`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'r> {
    /// A whole line, without its end.
    Message(&'r [u8]),
    /// A line longer than the buffer ended; its bytes were dropped.
    TooLong,
}

/// Gathers bytes into lines in a fixed buffer.
#[derive(Debug)]
pub struct LineReader<'b> {
    buffer: &'b mut [u8],
    start: usize,
    end: usize,
    scanned: usize,
    dropping: bool,
}

impl<'b> LineReader<'b> {
    /// A reader using `buffer`, whose size is the longest line it takes.
    pub fn new(buffer: &'b mut [u8]) -> Self {
        Self {
            buffer,
            start: 0,
            end: 0,
            scanned: 0,
            dropping: false,
        }
    }

    /// The free part of the buffer to read into, after moving what is kept to its start.
    pub fn free_space(&mut self) -> &mut [u8] {
        if self.start > 0 {
            self.buffer.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.scanned -= self.start;
            self.start = 0;
        }
        self.buffer.get_mut(self.end..).unwrap_or_default()
    }

    /// Takes `count` bytes read into [`Self::free_space`].
    pub fn filled(&mut self, count: usize) {
        self.end = (self.end + count).min(self.buffer.len());
    }

    /// The next whole line, if one is there.
    pub fn next_line(&mut self) -> Option<Line<'_>> {
        loop {
            let newline = self
                .buffer
                .get(self.scanned..self.end)
                .and_then(|bytes| bytes.iter().position(|&byte| byte == b'\n'));
            let Some(offset) = newline else {
                self.scanned = self.end;
                if self.dropping || self.end - self.start == self.buffer.len() {
                    self.dropping = true;
                    self.start = 0;
                    self.end = 0;
                    self.scanned = 0;
                }
                return None;
            };
            let line_end = self.scanned + offset;
            let line_start = self.start;
            self.start = line_end + 1;
            self.scanned = self.start;
            if self.dropping {
                self.dropping = false;
                return Some(Line::TooLong);
            }
            let line = self.buffer.get(line_start..line_end).unwrap_or_default();
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\r')) {
                continue;
            }
            return self
                .buffer
                .get(line_start..line_start + line.len())
                .map(Line::Message);
        }
    }

    /// The input ended: drops a line left unfinished, and says whether there was one.
    pub fn end_of_input(&mut self) -> bool {
        let unfinished = self.dropping || self.end > self.start;
        self.start = 0;
        self.end = 0;
        self.scanned = 0;
        self.dropping = false;
        unfinished
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn feed(reader: &mut LineReader<'_>, bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
        let mut lines = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let space = reader.free_space();
            let count = space.len().min(rest.len());
            space[..count].copy_from_slice(&rest[..count]);
            reader.filled(count);
            rest = &rest[count..];
            while let Some(line) = reader.next_line() {
                lines.push(match line {
                    Line::Message(bytes) => Some(bytes.to_vec()),
                    Line::TooLong => None,
                });
            }
        }
        lines
    }

    #[test]
    fn several_lines_in_one_read_come_out_in_order() {
        let mut buffer = [0u8; 64];
        let mut reader = LineReader::new(&mut buffer);
        assert_eq!(
            feed(&mut reader, b"{\"a\":1}\n{\"b\":2}\n"),
            [Some(b"{\"a\":1}".to_vec()), Some(b"{\"b\":2}".to_vec())]
        );
    }

    #[test]
    fn a_line_split_across_reads_is_joined() {
        let mut buffer = [0u8; 64];
        let mut reader = LineReader::new(&mut buffer);
        assert!(feed(&mut reader, b"{\"a\":").is_empty());
        assert!(feed(&mut reader, b"1").is_empty());
        assert_eq!(feed(&mut reader, b"}\n"), [Some(b"{\"a\":1}".to_vec())]);
    }

    #[test]
    fn crlf_ends_and_empty_lines_are_skipped() {
        let mut buffer = [0u8; 64];
        let mut reader = LineReader::new(&mut buffer);
        assert_eq!(
            feed(&mut reader, b"\n\r\n  \nx\r\n\ny\n"),
            [Some(b"x".to_vec()), Some(b"y".to_vec())]
        );
    }

    #[test]
    fn a_line_longer_than_the_buffer_is_dropped_then_a_good_line_is_read() {
        let mut buffer = [0u8; 16];
        let mut reader = LineReader::new(&mut buffer);
        let mut input = vec![b'x'; 40];
        input.extend_from_slice(b"\n{\"ok\":1}\n");
        assert_eq!(
            feed(&mut reader, &input),
            [None, Some(b"{\"ok\":1}".to_vec())]
        );
        let exactly = [vec![b'y'; 15], b"\n".to_vec()].concat();
        assert_eq!(feed(&mut reader, &exactly), [Some(vec![b'y'; 15])]);
        let one_more = [vec![b'z'; 16], b"\nq\n".to_vec()].concat();
        assert_eq!(feed(&mut reader, &one_more), [None, Some(b"q".to_vec())]);
    }

    #[test]
    fn the_end_of_input_drops_an_unfinished_line() {
        let mut buffer = [0u8; 32];
        let mut reader = LineReader::new(&mut buffer);
        assert!(feed(&mut reader, b"{\"half\":").is_empty());
        assert!(reader.end_of_input());
        assert!(!reader.end_of_input());
        assert_eq!(feed(&mut reader, b"{}\n"), [Some(b"{}".to_vec())]);
        assert!(!reader.end_of_input());
    }
}
