//! The C runtime's `FILE` streams (D146), as logic over a [`Backing`] (the descriptor's read,
//! write and seek): a 4 KiB buffer that holds either bytes read ahead or bytes waiting to be
//! written, fully, line or not buffered; text mode, Windows' default, where a written `\n`
//! becomes `\r\n` in the file and a read `\r\n` becomes `\n` and Ctrl+Z ends the input, as
//! Microsoft documents for `fopen`; one byte pushed back by `ungetc`; and positions that count
//! the file's own bytes, so `ftell` and `fseek` agree in either mode.

/// The buffer's size.
pub const BUFFER_SIZE: usize = 4096;

const CARRIAGE_RETURN: u8 = b'\r';
const LINE_FEED: u8 = b'\n';
const END_OF_TEXT: u8 = 0x1a;

/// Where a seek counts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Start,
    Current,
    End,
}

/// How a stream's bytes reach its file; each call fails with a Linux error number.
pub trait Backing {
    /// Reads into `buffer`; 0 at the end of the file.
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, u16>;
    /// Writes all of `bytes`.
    fn write(&mut self, bytes: &[u8]) -> Result<(), u16>;
    /// Moves the file's position and returns the new one.
    fn seek(&mut self, offset: i64, origin: Origin) -> Result<u64, u16>;
}

/// What `fopen`'s mode string asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
    pub exclusive: bool,
    pub text: bool,
}

impl Mode {
    /// The mode `letters` (an `fopen` mode string, without its NUL) ask for: `r`, `w` or `a`,
    /// then any of `+`, `b`, `t` and `x` (`x` only with `w`) and the letters Microsoft adds
    /// for caching hints (`c`, `n`, `S`, `R`, `T`, `D`, `N`), which change nothing here; an
    /// encoding after a comma (`, ccs=UTF-8`) is refused. Without `b` the stream is in text
    /// mode, Windows' default.
    pub fn parse(letters: impl IntoIterator<Item = u32>) -> Option<Mode> {
        let mut letters = letters.into_iter();
        let mut mode = match char::from_u32(letters.next()?)? {
            'r' => Mode::with(true, false),
            'w' => Mode {
                create: true,
                truncate: true,
                ..Mode::with(false, true)
            },
            'a' => Mode {
                create: true,
                append: true,
                ..Mode::with(false, true)
            },
            _ => return None,
        };
        for letter in letters {
            match char::from_u32(letter)? {
                '+' => {
                    mode.read = true;
                    mode.write = true;
                }
                'b' => mode.text = false,
                't' => mode.text = true,
                'x' if mode.truncate => mode.exclusive = true,
                'c' | 'n' | 'S' | 'R' | 'T' | 'D' | 'N' => {}
                _ => return None,
            }
        }
        Some(mode)
    }

    const fn with(read: bool, write: bool) -> Mode {
        Mode {
            read,
            write,
            create: false,
            truncate: false,
            append: false,
            exclusive: false,
            text: true,
        }
    }
}

/// When written bytes leave the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buffering {
    /// When the buffer is full, or on a flush, seek or close.
    Full,
    /// Also after each write that holds a line feed: a terminal's standard output.
    Line,
    /// After every write: standard error.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Idle,
    Reading,
    Writing,
}

/// One open stream.
#[derive(Debug)]
pub struct Stream<B> {
    backing: B,
    mode: Mode,
    buffering: Buffering,
    direction: Direction,
    buffer: [u8; BUFFER_SIZE],
    start: usize,
    end: usize,
    pushed_back: Option<u8>,
    end_of_file: bool,
    error: bool,
}

impl<B: Backing> Stream<B> {
    pub fn new(backing: B, mode: Mode, buffering: Buffering) -> Self {
        Self {
            backing,
            mode,
            buffering,
            direction: Direction::Idle,
            buffer: [0; BUFFER_SIZE],
            start: 0,
            end: 0,
            pushed_back: None,
            end_of_file: false,
            error: false,
        }
    }

    pub fn backing(&self) -> &B {
        &self.backing
    }

    /// Whether a read reached the end of the file (`feof`).
    pub fn end_of_file(&self) -> bool {
        self.end_of_file
    }

    /// Whether a read or write failed (`ferror`).
    pub fn error(&self) -> bool {
        self.error
    }

    /// Clears both indicators (`clearerr`).
    pub fn clear_indicators(&mut self) {
        self.end_of_file = false;
        self.error = false;
    }

    /// Reads up to `out.len()` bytes (after text mode's translation) and returns how many.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let mut filled = 0;
        if self.mode.text {
            while filled < out.len() {
                let Some(byte) = self.get_byte() else {
                    break;
                };
                out[filled] = byte;
                filled += 1;
            }
            return filled;
        }
        if out.is_empty() {
            return 0;
        }
        if let Some(byte) = self.pushed_back.take() {
            out[0] = byte;
            filled = 1;
        }
        if filled == out.len() || !self.start_reading() {
            return filled;
        }
        while filled < out.len() {
            if self.start == self.end && out.len() - filled >= BUFFER_SIZE {
                match self.backing.read(&mut out[filled..]) {
                    Ok(0) => {
                        self.end_of_file = true;
                        break;
                    }
                    Ok(read) => filled += read,
                    Err(_) => {
                        self.error = true;
                        break;
                    }
                }
                continue;
            }
            if !self.fill() {
                break;
            }
            let taken = (self.end - self.start).min(out.len() - filled);
            out[filled..filled + taken]
                .copy_from_slice(&self.buffer[self.start..self.start + taken]);
            self.start += taken;
            filled += taken;
        }
        filled
    }

    /// The next byte (`fgetc`), after text mode's translation; `None` at the end of the file or
    /// on an error, which the indicators tell apart.
    pub fn get_byte(&mut self) -> Option<u8> {
        if let Some(byte) = self.pushed_back.take() {
            return Some(byte);
        }
        if !self.start_reading() {
            return None;
        }
        let byte = self.next_raw()?;
        if !self.mode.text {
            return Some(byte);
        }
        match byte {
            END_OF_TEXT => {
                self.start -= 1;
                self.end_of_file = true;
                None
            }
            CARRIAGE_RETURN => {
                let reached_end = self.end_of_file;
                match self.peek_raw() {
                    Some(LINE_FEED) => {
                        self.start += 1;
                        Some(LINE_FEED)
                    }
                    _ => {
                        self.end_of_file = reached_end;
                        Some(CARRIAGE_RETURN)
                    }
                }
            }
            byte => Some(byte),
        }
    }

    /// Pushes `byte` back (`ungetc`), so the next read returns it first; one byte at a time.
    pub fn unget(&mut self, byte: u8) -> bool {
        if self.pushed_back.is_some() || !self.start_reading() {
            return false;
        }
        self.pushed_back = Some(byte);
        self.end_of_file = false;
        true
    }

    /// Writes `bytes` (a `\n` as `\r\n` in text mode) and returns how many of them it took.
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        if !self.start_writing() {
            return 0;
        }
        let mut taken = 0;
        if self.mode.text {
            for piece in bytes.split_inclusive(|&byte| byte == LINE_FEED) {
                let (line, newline) = match piece.split_last() {
                    Some((&LINE_FEED, line)) => (line, true),
                    _ => (piece, false),
                };
                if !self.put(line) {
                    return taken;
                }
                taken += line.len();
                if newline {
                    if !self.put(b"\r\n") {
                        return taken;
                    }
                    taken += 1;
                }
            }
        } else {
            if !self.put(bytes) {
                return 0;
            }
            taken = bytes.len();
        }
        let flush_now = match self.buffering {
            Buffering::Full => false,
            Buffering::Line => bytes.contains(&LINE_FEED),
            Buffering::None => true,
        };
        if flush_now {
            let _ = self.write_pending();
        }
        taken
    }

    /// Writes out what the buffer holds, or gives back what it read ahead (`fflush`).
    pub fn flush(&mut self) -> Result<(), u16> {
        let result = match self.direction {
            Direction::Writing => self.write_pending(),
            Direction::Reading => self.give_back_read_ahead(),
            Direction::Idle => Ok(()),
        };
        self.direction = Direction::Idle;
        result
    }

    /// Moves to `offset` from `origin` (`fseek`), counting the file's own bytes.
    pub fn seek(&mut self, offset: i64, origin: Origin) -> Result<(), u16> {
        let ahead = match self.direction {
            Direction::Reading => self.unread(),
            _ => 0,
        };
        if self.direction == Direction::Writing {
            self.write_pending()?;
        }
        self.start = 0;
        self.end = 0;
        self.pushed_back = None;
        self.direction = Direction::Idle;
        let offset = match origin {
            Origin::Current => offset - ahead,
            _ => offset,
        };
        self.backing.seek(offset, origin)?;
        self.end_of_file = false;
        Ok(())
    }

    /// The position (`ftell`): the file's own bytes before the next one read or written.
    pub fn position(&mut self) -> Result<u64, u16> {
        let file = self.backing.seek(0, Origin::Current)?;
        Ok(match self.direction {
            Direction::Reading => file.saturating_sub(self.unread() as u64),
            Direction::Writing => file + self.end as u64,
            Direction::Idle => file,
        })
    }

    fn unread(&self) -> i64 {
        (self.end - self.start) as i64 + i64::from(self.pushed_back.is_some())
    }

    fn start_reading(&mut self) -> bool {
        if !self.mode.read {
            self.error = true;
            return false;
        }
        if self.direction == Direction::Writing && self.write_pending().is_err() {
            return false;
        }
        self.direction = Direction::Reading;
        true
    }

    fn start_writing(&mut self) -> bool {
        if !self.mode.write {
            self.error = true;
            return false;
        }
        if self.direction == Direction::Reading && self.give_back_read_ahead().is_err() {
            self.error = true;
            return false;
        }
        self.direction = Direction::Writing;
        true
    }

    fn give_back_read_ahead(&mut self) -> Result<(), u16> {
        let ahead = self.unread();
        self.start = 0;
        self.end = 0;
        self.pushed_back = None;
        if ahead > 0 {
            self.backing.seek(-ahead, Origin::Current)?;
        }
        Ok(())
    }

    fn write_pending(&mut self) -> Result<(), u16> {
        if self.end == 0 {
            return Ok(());
        }
        let result = self.backing.write(&self.buffer[..self.end]);
        self.start = 0;
        self.end = 0;
        result.inspect_err(|_| self.error = true)
    }

    fn put(&mut self, bytes: &[u8]) -> bool {
        let mut rest = bytes;
        while !rest.is_empty() {
            if self.end == BUFFER_SIZE && self.write_pending().is_err() {
                return false;
            }
            if self.end == 0 && rest.len() >= BUFFER_SIZE {
                return match self.backing.write(rest) {
                    Ok(()) => true,
                    Err(_) => {
                        self.error = true;
                        false
                    }
                };
            }
            let taken = (BUFFER_SIZE - self.end).min(rest.len());
            self.buffer[self.end..self.end + taken].copy_from_slice(&rest[..taken]);
            self.end += taken;
            rest = &rest[taken..];
        }
        true
    }

    fn fill(&mut self) -> bool {
        if self.start < self.end {
            return true;
        }
        match self.backing.read(&mut self.buffer) {
            Ok(0) => {
                self.start = 0;
                self.end = 0;
                self.end_of_file = true;
                false
            }
            Ok(read) => {
                self.start = 0;
                self.end = read;
                true
            }
            Err(_) => {
                self.error = true;
                false
            }
        }
    }

    fn next_raw(&mut self) -> Option<u8> {
        if !self.fill() {
            return None;
        }
        let byte = self.buffer[self.start];
        self.start += 1;
        Some(byte)
    }

    fn peek_raw(&mut self) -> Option<u8> {
        self.fill().then(|| self.buffer[self.start])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Memory {
        bytes: Vec<u8>,
        at: usize,
        writes: usize,
    }

    impl Backing for Memory {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, u16> {
            let left = self.bytes.len().saturating_sub(self.at);
            let taken = left.min(buffer.len());
            buffer[..taken].copy_from_slice(&self.bytes[self.at..self.at + taken]);
            self.at += taken;
            Ok(taken)
        }

        fn write(&mut self, bytes: &[u8]) -> Result<(), u16> {
            self.writes += 1;
            let end = self.at + bytes.len();
            if self.bytes.len() < end {
                self.bytes.resize(end, 0);
            }
            self.bytes[self.at..end].copy_from_slice(bytes);
            self.at = end;
            Ok(())
        }

        fn seek(&mut self, offset: i64, origin: Origin) -> Result<u64, u16> {
            let base = match origin {
                Origin::Start => 0,
                Origin::Current => self.at as i64,
                Origin::End => self.bytes.len() as i64,
            };
            let at = base + offset;
            if at < 0 {
                return Err(22);
            }
            self.at = at as usize;
            Ok(self.at as u64)
        }
    }

    fn mode(letters: &str) -> Mode {
        Mode::parse(letters.chars().map(u32::from)).unwrap()
    }

    fn stream(bytes: &[u8], letters: &str) -> Stream<Memory> {
        let memory = Memory {
            bytes: bytes.to_vec(),
            ..Memory::default()
        };
        Stream::new(memory, mode(letters), Buffering::Full)
    }

    #[test]
    fn mode_strings_are_read_as_fopen_reads_them() {
        let read = mode("r");
        assert!(read.read && !read.write && read.text && !read.create);
        let write = mode("wb");
        assert!(write.write && !write.read && write.truncate && write.create && !write.text);
        let append = mode("a+t");
        assert!(append.append && append.read && append.write && append.text);
        assert!(mode("wx").exclusive);
        assert!(mode("rbS").read);
        for refused in ["", "x", "rx", "q", "r?", "w, ccs=UTF-8"] {
            assert_eq!(
                Mode::parse(refused.chars().map(u32::from)),
                None,
                "{refused}"
            );
        }
    }

    #[test]
    fn binary_streams_keep_every_byte_and_count_positions() {
        let mut out = stream(b"", "w+b");
        assert_eq!(out.write(b"one\r\ntwo\n"), 9);
        assert_eq!(out.position(), Ok(9));
        assert_eq!(out.backing().writes, 0);
        out.seek(0, Origin::Start).unwrap();
        let mut read = [0u8; 16];
        assert_eq!(out.read(&mut read), 9);
        assert_eq!(&read[..9], b"one\r\ntwo\n");
        assert_eq!(out.read(&mut read), 0);
        assert!(out.end_of_file() && !out.error());
    }

    #[test]
    fn text_mode_writes_crlf_and_reads_lf() {
        let mut out = stream(b"", "w");
        assert_eq!(out.write(b"a\nb\n\nc"), 6);
        out.flush().unwrap();
        assert_eq!(out.backing().bytes, b"a\r\nb\r\n\r\nc");
        let mut input = stream(b"a\r\nb\rc\r\n\x1a after", "r");
        let mut read = [0u8; 32];
        let count = input.read(&mut read);
        assert_eq!(&read[..count], b"a\nb\rc\n");
        assert!(input.end_of_file());
        assert_eq!(input.get_byte(), None);
    }

    #[test]
    fn a_carriage_return_at_the_buffer_s_end_still_pairs() {
        let mut file = vec![b'x'; BUFFER_SIZE - 1];
        file.extend_from_slice(b"\r\nz\r");
        let mut input = stream(&file, "rt");
        let mut read = vec![0u8; BUFFER_SIZE + 8];
        let count = input.read(&mut read);
        assert_eq!(count, BUFFER_SIZE + 2);
        assert_eq!(&read[BUFFER_SIZE - 1..count], b"\nz\r");
        assert!(input.end_of_file());
    }

    #[test]
    fn positions_count_the_file_s_bytes_while_reading_in_text_mode() {
        let mut input = stream(b"ab\r\ncd\r\n", "r");
        assert_eq!(input.get_byte(), Some(b'a'));
        assert_eq!(input.get_byte(), Some(b'b'));
        assert_eq!(input.get_byte(), Some(b'\n'));
        assert_eq!(input.position(), Ok(4));
        input.seek(1, Origin::Current).unwrap();
        assert_eq!(input.get_byte(), Some(b'd'));
        input.seek(-2, Origin::End).unwrap();
        assert_eq!(input.get_byte(), Some(b'\n'));
        assert_eq!(input.get_byte(), None);
    }

    #[test]
    fn writing_after_reading_starts_where_reading_stopped() {
        let mut both = stream(b"12345", "r+b");
        assert_eq!(both.get_byte(), Some(b'1'));
        assert_eq!(both.get_byte(), Some(b'2'));
        assert_eq!(both.write(b"ab"), 2);
        both.flush().unwrap();
        assert_eq!(both.backing().bytes, b"12ab5");
        assert_eq!(both.position(), Ok(4));
    }

    #[test]
    fn one_byte_can_be_pushed_back() {
        let mut input = stream(b"xy", "rb");
        assert_eq!(input.get_byte(), Some(b'x'));
        assert!(input.unget(b'q'));
        assert!(!input.unget(b'r'));
        assert_eq!(input.position(), Ok(0));
        let mut read = [0u8; 4];
        assert_eq!(input.read(&mut read), 2);
        assert_eq!(&read[..2], b"qy");
    }

    #[test]
    fn buffering_decides_when_bytes_reach_the_file() {
        let mut full = stream(b"", "wb");
        full.write(b"line\n");
        assert_eq!(full.backing().writes, 0);
        let mut line = Stream::new(Memory::default(), mode("wb"), Buffering::Line);
        line.write(b"part");
        assert_eq!(line.backing().writes, 0);
        line.write(b" end\n");
        assert_eq!(line.backing().writes, 1);
        assert_eq!(line.backing().bytes, b"part end\n");
        let mut none = Stream::new(Memory::default(), mode("wb"), Buffering::None);
        none.write(b"x");
        assert_eq!(none.backing().bytes, b"x");
        let mut large = stream(b"", "wb");
        assert_eq!(large.write(&vec![7u8; BUFFER_SIZE * 2]), BUFFER_SIZE * 2);
        assert_eq!(large.backing().bytes.len(), BUFFER_SIZE * 2);
    }

    #[test]
    fn a_stream_refuses_the_direction_its_mode_lacks() {
        let mut input = stream(b"abc", "r");
        assert_eq!(input.write(b"x"), 0);
        assert!(input.error());
        input.clear_indicators();
        assert!(!input.error());
        let mut output = stream(b"", "w");
        assert_eq!(output.get_byte(), None);
        assert!(output.error() && !output.end_of_file());
    }
}
