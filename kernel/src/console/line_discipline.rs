//! The console's line discipline (D100): what happens to each byte the console receives before
//! a program reads it, as POSIX's general terminal interface describes (termios(3)). In
//! canonical mode ([`local::ICANON`]) input is edited a line at a time and a read returns at
//! most one line; otherwise bytes are readable as they arrive, as `VMIN` and `VTIME` say. Typed
//! bytes are echoed, `VINTR` and `VQUIT` raise signals ([`Received`]), and the settings are a
//! `struct termios` ([`Termios`]) with Linux's flag values, the kernel's native ABI (D1).
//!
//! Pure logic: the console feeds it from its receive interrupt and echoes through a closure, so
//! host tests drive it without hardware.

/// The bytes the discipline holds: unread input and the line being edited, together.
pub const CAPACITY: usize = 2048;

const WORDS: usize = CAPACITY / 64;

const _: () = assert!(CAPACITY.is_power_of_two() && CAPACITY.is_multiple_of(64));

/// How many control characters a `struct termios` holds (`NCCS` of the kernel's structure).
pub const CONTROL_CHARACTERS: usize = 19;

/// The input flags (`c_iflag`) the discipline acts on.
pub mod input {
    /// Clear the eighth bit of each byte.
    pub const ISTRIP: u32 = 0o40;
    /// Read newline as carriage return.
    pub const INLCR: u32 = 0o100;
    /// Ignore carriage return.
    pub const IGNCR: u32 = 0o200;
    /// Read carriage return as newline.
    pub const ICRNL: u32 = 0o400;
    /// Start and stop output with `VSTART` and `VSTOP` (kept, not acted on).
    pub const IXON: u32 = 0o2000;
    /// Input is UTF-8: an erase removes a whole character.
    pub const IUTF8: u32 = 0o40000;
}

/// The output flags (`c_oflag`): kept and reported; the console's driver always writes
/// newline as carriage return and newline.
pub mod output {
    /// Process output.
    pub const OPOST: u32 = 0o1;
    /// Write newline as carriage return and newline.
    pub const ONLCR: u32 = 0o4;
}

/// The control flags (`c_cflag`): kept and reported, as the console's line cannot change.
pub mod control {
    /// 115200 baud.
    pub const B115200: u32 = 0o10002;
    /// Eight-bit characters.
    pub const CS8: u32 = 0o60;
    /// The receiver is on.
    pub const CREAD: u32 = 0o200;
    /// Hang up on the last close.
    pub const HUPCL: u32 = 0o2000;
    /// No modem control lines.
    pub const CLOCAL: u32 = 0o4000;
}

/// The local flags (`c_lflag`) the discipline acts on.
pub mod local {
    /// `VINTR` and `VQUIT` raise their signals.
    pub const ISIG: u32 = 0o1;
    /// Canonical mode: input is edited and read a line at a time.
    pub const ICANON: u32 = 0o2;
    /// Echo input.
    pub const ECHO: u32 = 0o10;
    /// An erase rubs the character out on the screen.
    pub const ECHOE: u32 = 0o20;
    /// A kill echoes a newline after the kill character.
    pub const ECHOK: u32 = 0o40;
    /// Echo newline even without [`ECHO`].
    pub const ECHONL: u32 = 0o100;
    /// A signal character does not flush the input.
    pub const NOFLSH: u32 = 0o200;
    /// Control characters echo as `^X`.
    pub const ECHOCTL: u32 = 0o1000;
    /// A kill rubs the whole line out on the screen.
    pub const ECHOKE: u32 = 0o4000;
    /// `VWERASE` and `VLNEXT` act.
    pub const IEXTEN: u32 = 0o100000;
}

/// Indexes of the control characters (`c_cc`). A character of 0 is disabled
/// (`_POSIX_VDISABLE`).
pub mod index {
    pub const VINTR: usize = 0;
    pub const VQUIT: usize = 1;
    pub const VERASE: usize = 2;
    pub const VKILL: usize = 3;
    pub const VEOF: usize = 4;
    pub const VTIME: usize = 5;
    pub const VMIN: usize = 6;
    pub const VSTART: usize = 8;
    pub const VSTOP: usize = 9;
    pub const VSUSP: usize = 10;
    pub const VEOL: usize = 11;
    pub const VREPRINT: usize = 12;
    pub const VDISCARD: usize = 13;
    pub const VWERASE: usize = 14;
    pub const VLNEXT: usize = 15;
    pub const VEOL2: usize = 16;
}

/// ASCII backspace, `^H`: an erase as well as `VERASE`, as BSD's `VERASE2` is, since terminals
/// send either for the backspace key.
pub const BACKSPACE: u8 = 0x08;

const DELETE: u8 = 0x7f;
const TAB_WIDTH: usize = 8;

/// A terminal's settings, a `struct termios`: four flag words, the line discipline's number
/// and the control characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Termios {
    pub input: u32,
    pub output: u32,
    pub control: u32,
    pub local: u32,
    pub line: u8,
    pub control_characters: [u8; CONTROL_CHARACTERS],
}

impl Termios {
    /// Every flag off and every control character disabled.
    pub const OFF: Self = Self {
        input: 0,
        output: 0,
        control: 0,
        local: 0,
        line: 0,
        control_characters: [0; CONTROL_CHARACTERS],
    };

    /// A Linux serial console's settings after boot (`stty -a`), at 115200 baud: canonical
    /// mode with echo and signals, carriage return read as newline.
    pub const SERIAL_CONSOLE: Self = Self {
        input: input::ICRNL | input::IXON,
        output: output::OPOST | output::ONLCR,
        control: control::B115200
            | control::CS8
            | control::CREAD
            | control::HUPCL
            | control::CLOCAL,
        local: local::ISIG
            | local::ICANON
            | local::ECHO
            | local::ECHOE
            | local::ECHOK
            | local::ECHOCTL
            | local::ECHOKE
            | local::IEXTEN,
        line: 0,
        control_characters: [
            0x03, 0x1c, DELETE, 0x15, 0x04, 0, 1, 0, 0x11, 0x13, 0x1a, 0, 0x12, 0x0f, 0x17, 0x16,
            0, 0, 0,
        ],
    };

    /// The size of the kernel's `struct termios`.
    pub const SIZE: usize = 17 + CONTROL_CHARACTERS;

    /// As the kernel's `struct termios` lays it out, in native byte order.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (word, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([
            self.input,
            self.output,
            self.control,
            self.local,
        ]) {
            word.copy_from_slice(&value.to_ne_bytes());
        }
        bytes[16] = self.line;
        bytes[17..].copy_from_slice(&self.control_characters);
        bytes
    }

    /// From the kernel's `struct termios` laid out in native byte order.
    pub fn from_bytes(bytes: &[u8; Self::SIZE]) -> Self {
        let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut control_characters = [0; CONTROL_CHARACTERS];
        control_characters.copy_from_slice(&bytes[17..]);
        Self {
            input: word(0),
            output: word(4),
            control: word(8),
            local: word(12),
            line: bytes[16],
            control_characters,
        }
    }

    pub fn is_canonical(&self) -> bool {
        self.local & local::ICANON != 0
    }

    /// `VMIN` and `VTIME`: how many bytes a non-canonical read waits for, and how long, in
    /// tenths of a second.
    pub fn minimum_and_time(&self) -> (u8, u8) {
        (
            self.control_characters[index::VMIN],
            self.control_characters[index::VTIME],
        )
    }

    fn has(&self, flags: u32) -> bool {
        self.local & flags == flags
    }

    fn is(&self, byte: u8, index: usize) -> bool {
        let character = self.control_characters[index];
        character != 0 && byte == character
    }
}

/// What a [`LineDiscipline::receive`] did that others must act on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Received {
    /// Input became readable: a line ended, or bytes arrived in non-canonical mode.
    pub readable: bool,
    /// `VINTR` was typed: the foreground program is sent `SIGINT`.
    pub interrupt: bool,
    /// `VQUIT` was typed: the foreground program is sent `SIGQUIT`.
    pub quit: bool,
}

impl core::ops::BitOrAssign for Received {
    fn bitor_assign(&mut self, other: Self) {
        self.readable |= other.readable;
        self.interrupt |= other.interrupt;
        self.quit |= other.quit;
    }
}

/// What a [`LineDiscipline::take`] took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taken {
    /// Bytes copied out.
    pub length: usize,
    /// In canonical mode, whether the line ended with them (an end of file with none).
    pub line_ended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Erase {
    Character,
    Word,
    Line,
}

/// The console's input: unread bytes, oldest first, followed by the line being edited, in
/// [`CAPACITY`] bytes together. A byte may end a line; an end of file typed at the start of a
/// line, or after bytes it pushes to the reader without a newline, is kept as a marker byte
/// that ends a line and is never read.
#[derive(Debug)]
pub struct LineDiscipline {
    settings: Termios,
    bytes: [u8; CAPACITY],
    line_ends: [u64; WORDS],
    markers: [u64; WORDS],
    read: usize,
    committed: usize,
    end: usize,
    lines: usize,
    marker_count: usize,
    literal_next: bool,
    dropped: u64,
}

impl Default for LineDiscipline {
    fn default() -> Self {
        Self::new()
    }
}

impl LineDiscipline {
    /// With every setting off (non-canonical, no echo, `VMIN` and `VTIME` 0) until
    /// [`Self::set_settings`], and all zero, so a static one costs no image space.
    pub const fn new() -> Self {
        Self {
            settings: Termios::OFF,
            bytes: [0; CAPACITY],
            line_ends: [0; WORDS],
            markers: [0; WORDS],
            read: 0,
            committed: 0,
            end: 0,
            lines: 0,
            marker_count: 0,
            literal_next: false,
            dropped: 0,
        }
    }

    pub fn settings(&self) -> Termios {
        self.settings
    }

    /// Changes the settings, after discarding the input if `flush`. Leaving canonical mode
    /// makes the line being edited readable; entering it makes what non-canonical mode left
    /// unread one line.
    pub fn set_settings(&mut self, settings: Termios, flush: bool) {
        if flush {
            self.flush();
        }
        let was_canonical = self.settings.is_canonical();
        self.settings = settings;
        match (was_canonical, settings.is_canonical()) {
            (true, false) => {
                self.committed = self.end;
                self.literal_next = false;
            }
            (false, true) if self.committed != self.read => {
                let last = self.committed.wrapping_sub(1);
                if !bit(&self.line_ends, last) {
                    set_bit(&mut self.line_ends, last, true);
                    self.lines += 1;
                }
            }
            _ => {}
        }
    }

    /// Discards the unread input and the line being edited.
    pub fn flush(&mut self) {
        self.read = self.end;
        self.committed = self.end;
        self.lines = 0;
        self.marker_count = 0;
        self.literal_next = false;
    }

    /// Bytes dropped because the discipline was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Whether a read would take input without waiting for more, as `poll`'s `POLLIN` says: a
    /// whole line (or an end of file) in canonical mode; otherwise `VMIN` bytes, at least one,
    /// or one when `VTIME` is set (a read returns once it passes).
    pub fn is_readable(&self) -> bool {
        if self.settings.is_canonical() {
            return self.lines > 0;
        }
        let (minimum, time) = self.settings.minimum_and_time();
        let needed = match time {
            0 => usize::from(minimum).max(1),
            _ => 1,
        };
        self.available() >= needed
    }

    /// The unread bytes a non-canonical read can take.
    pub fn available(&self) -> usize {
        self.committed.wrapping_sub(self.read) - self.marker_count
    }

    /// The bytes a read can take without waiting for more input, as `FIONREAD` reports
    /// them: those of whole lines in canonical mode.
    pub fn pending(&self) -> usize {
        if !self.settings.is_canonical() {
            return self.available();
        }
        let (mut pending, mut counted) = (0, 0);
        let mut position = self.read;
        while position != self.committed {
            if !bit(&self.markers, position) {
                counted += 1;
            }
            if bit(&self.line_ends, position) {
                pending = counted;
            }
            position = position.wrapping_add(1);
        }
        pending
    }

    /// Moves unread bytes into `buffer`: in canonical mode those of the first line, up to its
    /// end, as many as fit (the rest stays for the next take), and nothing but the line's end
    /// for an end of file; otherwise as many as are unread and fit.
    pub fn take(&mut self, buffer: &mut [u8]) -> Taken {
        let canonical = self.settings.is_canonical();
        let mut length = 0;
        let mut line_ended = false;
        while self.read != self.committed && length < buffer.len() {
            let (byte, ended) = self.take_byte();
            if let Some(byte) = byte {
                buffer[length] = byte;
                length += 1;
            }
            if ended && canonical {
                line_ended = true;
                break;
            }
        }
        if canonical
            && !line_ended
            && length > 0
            && self.read != self.committed
            && bit(&self.markers, self.read)
        {
            self.take_byte();
            line_ended = true;
        }
        Taken { length, line_ended }
    }

    fn take_byte(&mut self) -> (Option<u8>, bool) {
        let position = self.read;
        self.read = position.wrapping_add(1);
        let ended = bit(&self.line_ends, position);
        if ended {
            self.lines -= 1;
        }
        if bit(&self.markers, position) {
            self.marker_count -= 1;
            return (None, ended);
        }
        (Some(self.bytes[position % CAPACITY]), ended)
    }

    /// Takes in `input`, as the console received it, echoing through `echo` with
    /// [`local::ECHO`] (control characters as `^X` with [`local::ECHOCTL`]). The input flags
    /// map carriage return and newline first. With [`local::ISIG`], `VINTR` and `VQUIT` discard
    /// the input (unless [`local::NOFLSH`]), echo as themselves (`^C`, `^\`), and are reported.
    /// In canonical mode `VERASE` and [`BACKSPACE`] remove the last character (a UTF-8 one with
    /// [`input::IUTF8`]), rubbing it out with [`local::ECHOE`], `VKILL` the line (rubbed out
    /// with [`local::ECHOE`], [`local::ECHOK`] and [`local::ECHOKE`], else echoed, with a
    /// newline with [`local::ECHOK`]), and with [`local::IEXTEN`] `VWERASE` the last word
    /// (letters, digits and `_`, after anything else before them) and `VLNEXT` takes the next
    /// byte as it is (echoed only with [`local::ECHO`]);
    /// newline, `VEOL` and `VEOL2` end the line and are read with it, `VEOF` ends it without
    /// being read (an empty line reads as an end of file). A line keeps room for the byte
    /// that ends it; bytes that find no room are dropped and counted.
    pub fn receive(&mut self, input: &[u8], echo: &mut impl FnMut(&[u8])) -> Received {
        let mut received = Received::default();
        for &byte in input {
            received |= self.receive_byte(byte, echo);
        }
        received
    }

    fn receive_byte(&mut self, byte: u8, echo: &mut impl FnMut(&[u8])) -> Received {
        let settings = self.settings;
        let canonical = settings.is_canonical();
        let mut byte = byte;
        if settings.input & input::ISTRIP != 0 {
            byte &= 0x7f;
        }
        if core::mem::take(&mut self.literal_next) {
            return self.append(byte, canonical, settings.has(local::ECHO), echo);
        }
        if byte == b'\r' {
            if settings.input & input::IGNCR != 0 {
                return Received::default();
            }
            if settings.input & input::ICRNL != 0 {
                byte = b'\n';
            }
        } else if byte == b'\n' && settings.input & input::INLCR != 0 {
            byte = b'\r';
        }
        if settings.has(local::ISIG) {
            let interrupt = settings.is(byte, index::VINTR);
            let quit = settings.is(byte, index::VQUIT);
            if interrupt || quit {
                if !settings.has(local::NOFLSH) {
                    self.flush();
                }
                if settings.has(local::ECHO) {
                    self.echo_character(byte, echo);
                }
                return Received {
                    readable: false,
                    interrupt,
                    quit,
                };
            }
        }
        if !canonical {
            return self.append(byte, false, settings.has(local::ECHO), echo);
        }
        let extended = settings.has(local::IEXTEN);
        if settings.is(byte, index::VERASE) || byte == BACKSPACE {
            self.erase(Erase::Character, byte, echo);
        } else if settings.is(byte, index::VKILL) {
            self.erase(Erase::Line, byte, echo);
        } else if extended && settings.is(byte, index::VWERASE) {
            self.erase(Erase::Word, byte, echo);
        } else if extended && settings.is(byte, index::VLNEXT) {
            self.literal_next = true;
            if settings.has(local::ECHO | local::ECHOCTL) {
                echo(b"^\x08");
            }
        } else if settings.is(byte, index::VEOF) {
            return self.end_line(None, false, echo);
        } else if byte == b'\n' || settings.is(byte, index::VEOL) || settings.is(byte, index::VEOL2)
        {
            let echoed =
                settings.has(local::ECHO) || (byte == b'\n' && settings.has(local::ECHONL));
            return self.end_line(Some(byte), echoed, echo);
        } else {
            return self.append(byte, true, settings.has(local::ECHO), echo);
        }
        Received::default()
    }

    fn append(
        &mut self,
        byte: u8,
        canonical: bool,
        echoed: bool,
        echo: &mut impl FnMut(&[u8]),
    ) -> Received {
        let reserve = usize::from(canonical);
        if self.end.wrapping_sub(self.read) + reserve >= CAPACITY {
            self.dropped += 1;
            return Received::default();
        }
        if echoed {
            self.echo_character(byte, echo);
        }
        self.push(byte, false, false);
        if canonical {
            return Received::default();
        }
        self.committed = self.end;
        Received {
            readable: true,
            ..Received::default()
        }
    }

    fn end_line(
        &mut self,
        terminator: Option<u8>,
        echoed: bool,
        echo: &mut impl FnMut(&[u8]),
    ) -> Received {
        if self.end.wrapping_sub(self.read) >= CAPACITY {
            self.dropped += 1;
            return Received::default();
        }
        if let (true, Some(byte)) = (echoed, terminator) {
            self.echo_character(byte, echo);
        }
        match terminator {
            Some(byte) => self.push(byte, true, false),
            None => {
                self.push(0, true, true);
                self.marker_count += 1;
            }
        }
        self.committed = self.end;
        self.lines += 1;
        Received {
            readable: true,
            ..Received::default()
        }
    }

    fn push(&mut self, byte: u8, ends_line: bool, marker: bool) {
        let position = self.end;
        self.bytes[position % CAPACITY] = byte;
        set_bit(&mut self.line_ends, position, ends_line);
        set_bit(&mut self.markers, position, marker);
        self.end = position.wrapping_add(1);
    }

    fn erase(&mut self, what: Erase, typed: u8, echo: &mut impl FnMut(&[u8])) {
        let settings = self.settings;
        let visual = match what {
            Erase::Line => settings.has(local::ECHOE | local::ECHOK | local::ECHOKE),
            _ => settings.has(local::ECHOE),
        };
        let echoing = settings.has(local::ECHO);
        if echoing && !visual && self.end != self.committed {
            self.echo_character(typed, echo);
            if what == Erase::Line && settings.has(local::ECHOK) {
                echo(b"\n");
            }
        }
        let mut in_word = false;
        while self.end != self.committed {
            let start = self.character_start();
            let first = self.bytes[start % CAPACITY];
            if what == Erase::Word {
                let word = first.is_ascii_alphanumeric() || first == b'_';
                if !word && in_word {
                    break;
                }
                in_word |= word;
            }
            if echoing && visual {
                self.rub_out(start, echo);
            }
            self.end = start;
            if what == Erase::Character {
                break;
            }
        }
    }

    fn character_start(&self) -> usize {
        let mut start = self.end.wrapping_sub(1);
        if self.settings.input & input::IUTF8 != 0 {
            while start != self.committed && self.bytes[start % CAPACITY] & 0xc0 == 0x80 {
                start = start.wrapping_sub(1);
            }
        }
        start
    }

    fn rub_out(&self, start: usize, echo: &mut impl FnMut(&[u8])) {
        let byte = self.bytes[start % CAPACITY];
        if byte == b'\t' {
            let column = self.column_at(start);
            let width = TAB_WIDTH - column % TAB_WIDTH;
            for _ in 0..width {
                echo(b"\x08");
            }
            return;
        }
        for _ in 0..self.echo_width(byte) {
            echo(b"\x08 \x08");
        }
    }

    fn column_at(&self, position: usize) -> usize {
        let mut column = 0;
        let mut at = self.committed;
        while at != position {
            let byte = self.bytes[at % CAPACITY];
            column += match byte {
                b'\t' => TAB_WIDTH - column % TAB_WIDTH,
                0x80..=0xbf if self.settings.input & input::IUTF8 != 0 => 0,
                _ => self.echo_width(byte),
            };
            at = at.wrapping_add(1);
        }
        column
    }

    fn echo_width(&self, byte: u8) -> usize {
        if !is_control(byte) {
            1
        } else if self.settings.has(local::ECHOCTL) {
            2
        } else {
            0
        }
    }

    fn echo_character(&self, byte: u8, echo: &mut impl FnMut(&[u8])) {
        if is_control(byte) && self.settings.has(local::ECHOCTL) {
            echo(&[b'^', byte ^ 0x40]);
        } else {
            echo(&[byte]);
        }
    }
}

fn is_control(byte: u8) -> bool {
    (byte < 0x20 && byte != b'\t' && byte != b'\n') || byte == DELETE
}

fn bit(bits: &[u64; WORDS], position: usize) -> bool {
    let index = position % CAPACITY;
    bits[index / 64] >> (index % 64) & 1 != 0
}

fn set_bit(bits: &mut [u64; WORDS], position: usize, value: bool) {
    let index = position % CAPACITY;
    let mask = 1 << (index % 64);
    match value {
        true => bits[index / 64] |= mask,
        false => bits[index / 64] &= !mask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    struct Console {
        discipline: Box<LineDiscipline>,
        echoed: Vec<u8>,
    }

    impl Console {
        fn new() -> Self {
            Self::with(Termios::SERIAL_CONSOLE)
        }

        fn with(settings: Termios) -> Self {
            let mut discipline = Box::new(LineDiscipline::new());
            discipline.set_settings(settings, false);
            Self {
                discipline,
                echoed: Vec::new(),
            }
        }

        fn types(&mut self, input: &[u8]) -> Received {
            let echoed = &mut self.echoed;
            self.discipline
                .receive(input, &mut |bytes| echoed.extend_from_slice(bytes))
        }

        fn echo(&mut self) -> Vec<u8> {
            core::mem::take(&mut self.echoed)
        }

        fn read(&mut self, count: usize) -> Vec<u8> {
            let mut buffer = alloc::vec![0; count];
            let taken = self.discipline.take(&mut buffer);
            buffer.truncate(taken.length);
            buffer
        }
    }

    fn raw(minimum: u8, time: u8) -> Termios {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !(local::ICANON | local::ECHO);
        settings.control_characters[index::VMIN] = minimum;
        settings.control_characters[index::VTIME] = time;
        settings
    }

    #[test]
    fn a_typed_line_is_echoed_and_readable_only_once_enter_ends_it() {
        let mut console = Console::new();
        assert_eq!(console.types(b"hello"), Received::default());
        assert!(!console.discipline.is_readable());
        assert_eq!(console.echo(), b"hello");
        let received = console.types(b"\r");
        assert!(received.readable);
        assert_eq!(console.echo(), b"\n");
        assert!(console.discipline.is_readable());
        assert_eq!(console.discipline.pending(), 6);
        assert_eq!(console.read(64), b"hello\n");
        assert!(!console.discipline.is_readable());
    }

    #[test]
    fn a_read_returns_one_line_at_a_time_and_the_rest_of_a_line_next() {
        let mut console = Console::new();
        console.types(b"first\nsecond\n");
        assert_eq!(console.read(3), b"fir");
        assert_eq!(console.read(64), b"st\n");
        assert_eq!(console.read(64), b"second\n");
        assert_eq!(console.read(64), b"");
    }

    #[test]
    fn delete_and_backspace_erase_the_last_character_and_rub_it_out() {
        let mut console = Console::new();
        console.types(b"ab\x7fc\x08d\n");
        assert_eq!(console.read(64), b"ad\n");
        assert_eq!(console.echo(), b"ab\x08 \x08c\x08 \x08d\n");
    }

    #[test]
    fn an_erase_on_an_empty_line_does_nothing() {
        let mut console = Console::new();
        console.types(b"\x7f\x08\n");
        assert_eq!(console.echo(), b"\n");
        assert_eq!(console.read(64), b"\n");
    }

    #[test]
    fn without_echoe_an_erase_echoes_the_erase_character() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ECHOE;
        let mut console = Console::with(settings);
        console.types(b"ab\x7f\n");
        assert_eq!(console.echo(), b"ab^?\n");
        assert_eq!(console.read(64), b"a\n");
    }

    #[test]
    fn kill_discards_the_line_and_rubs_it_out() {
        let mut console = Console::new();
        console.types(b"discard me\x15kept\r");
        assert_eq!(console.read(64), b"kept\n");
        let rub_out = b"\x08 \x08".repeat(10);
        let expected = [&b"discard me"[..], &rub_out, b"kept\n"].concat();
        assert_eq!(console.echo(), expected);
    }

    #[test]
    fn kill_without_echoke_echoes_the_kill_character_and_a_newline() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ECHOKE;
        let mut console = Console::with(settings);
        console.types(b"gone\x15ok\n");
        assert_eq!(console.echo(), b"gone^U\nok\n");
        assert_eq!(console.read(64), b"ok\n");
    }

    #[test]
    fn kill_without_echok_echoes_only_the_kill_character() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ECHOK;
        let mut console = Console::with(settings);
        console.types(b"gone\x15ok\n");
        assert_eq!(console.echo(), b"gone^Uok\n");
        assert_eq!(console.read(64), b"ok\n");
    }

    #[test]
    fn word_erase_removes_the_last_word_and_the_blanks_after_it() {
        let mut console = Console::new();
        console.types(b"two words \x17\x17one\n");
        assert_eq!(console.read(64), b"one\n");
        console.types(b"keep this\x17that\n");
        assert_eq!(console.read(64), b"keep that\n");
    }

    #[test]
    fn word_erase_takes_letters_digits_and_underscores_after_anything_else() {
        let mut console = Console::new();
        console.types(b"cd /usr/my_dir2/ \x17\n");
        assert_eq!(console.read(64), b"cd /usr/\n");
        console.types(b"a.b\x17\x17\n");
        assert_eq!(console.read(64), b"\n");
    }

    #[test]
    fn end_of_file_at_the_start_of_a_line_reads_as_zero_bytes() {
        let mut console = Console::new();
        let received = console.types(b"\x04");
        assert!(received.readable);
        assert!(console.discipline.is_readable());
        assert_eq!(console.discipline.pending(), 0);
        let mut buffer = [0; 8];
        assert_eq!(
            console.discipline.take(&mut buffer),
            Taken {
                length: 0,
                line_ended: true
            }
        );
        assert!(!console.discipline.is_readable());
        assert!(console.echo().is_empty());
    }

    #[test]
    fn end_of_file_after_bytes_pushes_them_without_a_newline() {
        let mut console = Console::new();
        console.types(b"partial\x04\x04after\n");
        assert_eq!(console.read(64), b"partial");
        assert_eq!(console.read(64), b"");
        assert_eq!(console.read(64), b"after\n");
    }

    #[test]
    fn a_read_that_fills_its_buffer_with_a_pushed_line_leaves_no_spurious_end_of_file() {
        let mut console = Console::new();
        console.types(b"seven..\x04next\n");
        assert_eq!(console.read(7), b"seven..");
        assert_eq!(console.read(64), b"next\n");
    }

    #[test]
    fn an_empty_buffer_takes_nothing_not_even_an_end_of_file() {
        let mut console = Console::new();
        console.types(b"\x04");
        assert_eq!(
            console.discipline.take(&mut []),
            Taken {
                length: 0,
                line_ended: false
            }
        );
        assert!(console.discipline.is_readable());
    }

    #[test]
    fn interrupt_discards_the_input_echoes_it_and_raises_sigint() {
        let mut console = Console::new();
        console.types(b"unread\n");
        let received = console.types(b"doomed\x03");
        assert_eq!(
            received,
            Received {
                readable: false,
                interrupt: true,
                quit: false
            }
        );
        assert_eq!(console.echo(), b"unread\ndoomed^C");
        assert!(!console.discipline.is_readable());
        console.types(b"after\r");
        assert_eq!(console.read(64), b"after\n");
    }

    #[test]
    fn quit_raises_sigquit_and_noflsh_keeps_the_input() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local |= local::NOFLSH;
        let mut console = Console::with(settings);
        console.types(b"kept\n");
        assert!(console.types(b"\x1c").quit);
        assert_eq!(console.read(64), b"kept\n");
    }

    #[test]
    fn signal_characters_are_plain_input_without_isig() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ISIG;
        let mut console = Console::with(settings);
        assert_eq!(
            console.types(b"\x03\n"),
            Received {
                readable: true,
                ..Received::default()
            }
        );
        assert_eq!(console.read(64), b"\x03\n");
        assert_eq!(console.echo(), b"^C\n");
    }

    #[test]
    fn a_disabled_control_character_matches_no_byte() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.control_characters[index::VINTR] = 0;
        let mut console = Console::with(settings);
        assert!(!console.types(b"\x00\x03\n").interrupt);
        assert_eq!(console.read(64), b"\x00\x03\n");
    }

    #[test]
    fn carriage_return_and_newline_are_mapped_as_the_input_flags_say() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.input = input::IGNCR;
        let mut console = Console::with(settings);
        console.types(b"a\r\n");
        assert_eq!(console.read(64), b"a\n");
        settings.input = input::INLCR;
        console.discipline.set_settings(settings, false);
        settings.control_characters[index::VEOL] = b'\r';
        console.discipline.set_settings(settings, false);
        console.types(b"b\n");
        assert_eq!(console.read(64), b"b\r");
    }

    #[test]
    fn istrip_clears_the_eighth_bit() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.input |= input::ISTRIP;
        let mut console = Console::with(settings);
        console.types(b"\xc1\n");
        assert_eq!(console.read(64), b"A\n");
    }

    #[test]
    fn literal_next_takes_the_next_byte_as_it_is() {
        let mut console = Console::new();
        console.types(b"\x16\x03\x16\x7f\n");
        assert_eq!(console.read(64), b"\x03\x7f\n");
        assert_eq!(console.echo(), b"^\x08^C^\x08^?\n");
    }

    #[test]
    fn a_literal_byte_is_not_echoed_without_echo_and_is_stripped_with_istrip() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ECHO;
        settings.input |= input::ISTRIP;
        let mut console = Console::with(settings);
        console.types(b"\x16\xf3ecret\n");
        assert!(console.echo().is_empty());
        assert_eq!(console.read(64), b"secret\n");
    }

    #[test]
    fn control_characters_echo_as_caret_letters_and_rub_out_two_columns() {
        let mut console = Console::new();
        console.types(b"\x01\x7f");
        assert_eq!(console.echo(), b"^A\x08 \x08\x08 \x08");
        console.types(b"\n");
        assert_eq!(console.read(64), b"\n");
    }

    #[test]
    fn erasing_a_tab_backs_up_to_where_it_began() {
        let mut console = Console::new();
        console.types(b"abc\t\x7f");
        assert_eq!(console.echo(), b"abc\t\x08\x08\x08\x08\x08");
    }

    #[test]
    fn with_iutf8_an_erase_removes_a_whole_character() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.input |= input::IUTF8;
        let mut console = Console::with(settings);
        console.types("aé€\x7f\x7f\n".as_bytes());
        assert_eq!(console.read(64), b"a\n");
        let mut console = Console::new();
        console.types("é\x7f\n".as_bytes());
        assert_eq!(console.read(64), b"\xc3\n");
    }

    #[test]
    fn echo_off_echoes_nothing_but_echonl_echoes_newlines() {
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ECHO;
        let mut console = Console::with(settings);
        console.types(b"secret\x7f\n");
        assert!(console.echo().is_empty());
        assert_eq!(console.read(64), b"secre\n");
        settings.local |= local::ECHONL;
        console.discipline.set_settings(settings, false);
        console.types(b"x\n");
        assert_eq!(console.echo(), b"\n");
    }

    #[test]
    fn non_canonical_bytes_are_readable_as_they_arrive_unedited() {
        let mut console = Console::with(raw(1, 0));
        let received = console.types(b"xy\x7fz\r");
        assert!(received.readable);
        assert!(console.echo().is_empty());
        assert_eq!(console.discipline.available(), 5);
        assert_eq!(console.discipline.pending(), 5);
        assert_eq!(console.read(2), b"xy");
        assert_eq!(console.read(64), b"\x7fz\n");
        assert!(!console.discipline.is_readable());
    }

    #[test]
    fn non_canonical_input_is_readable_once_vmin_bytes_arrived_or_one_with_vtime() {
        let mut console = Console::with(raw(3, 0));
        console.types(b"ab");
        assert!(!console.discipline.is_readable());
        console.types(b"c");
        assert!(console.discipline.is_readable());
        let mut console = Console::with(raw(3, 5));
        console.types(b"a");
        assert!(console.discipline.is_readable());
        let console = Console::with(raw(0, 0));
        assert!(!console.discipline.is_readable());
    }

    #[test]
    fn leaving_canonical_mode_makes_the_line_being_edited_readable() {
        let mut console = Console::new();
        console.types(b"done\npart");
        console.discipline.set_settings(raw(1, 0), false);
        assert_eq!(console.discipline.available(), 9);
        assert_eq!(console.read(64), b"done\npart");
    }

    #[test]
    fn entering_canonical_mode_makes_unread_bytes_one_line() {
        let mut console = Console::with(raw(1, 0));
        console.types(b"abc");
        console
            .discipline
            .set_settings(Termios::SERIAL_CONSOLE, false);
        assert!(console.discipline.is_readable());
        assert_eq!(console.read(64), b"abc");
        console.types(b"x\n");
        assert_eq!(console.read(64), b"x\n");
    }

    #[test]
    fn an_end_of_file_left_unread_is_skipped_by_a_non_canonical_read() {
        let mut console = Console::new();
        console.types(b"a\x04");
        console.discipline.set_settings(raw(1, 0), false);
        assert_eq!(console.discipline.available(), 1);
        assert_eq!(console.read(64), b"a");
        assert_eq!(console.discipline.available(), 0);
    }

    #[test]
    fn flushing_settings_discard_the_input() {
        let mut console = Console::new();
        console.types(b"old\nedit");
        console
            .discipline
            .set_settings(Termios::SERIAL_CONSOLE, true);
        assert!(!console.discipline.is_readable());
        console.types(b"new\n");
        assert_eq!(console.read(64), b"new\n");
    }

    #[test]
    fn a_full_line_drops_and_never_echoes_what_does_not_fit_but_still_ends() {
        let mut console = Console::new();
        let long = alloc::vec![b'x'; CAPACITY + 10];
        console.types(&long);
        assert_eq!(console.discipline.dropped(), 11);
        assert_eq!(console.echo(), alloc::vec![b'x'; CAPACITY - 1]);
        console.types(b"\n");
        assert_eq!(console.echo(), b"\n");
        let mut buffer = alloc::vec![0; CAPACITY + 10];
        let taken = console.discipline.take(&mut buffer);
        assert_eq!(taken.length, CAPACITY);
        assert!(taken.line_ended);
        assert_eq!(buffer[CAPACITY - 1], b'\n');
    }

    #[test]
    fn unread_lines_wrap_around_the_buffer() {
        let mut console = Console::new();
        let line = [b'w'; 100];
        for round in 0..3 * CAPACITY / 101 {
            console.types(&line);
            console.types(b"\n");
            if round % 2 == 1 {
                assert_eq!(console.read(200).len(), 101);
                assert_eq!(console.read(200).len(), 101);
            }
        }
        while console.discipline.is_readable() {
            assert_eq!(console.read(200), [&line[..], b"\n"].concat());
        }
        assert_eq!(console.discipline.dropped(), 0);
    }

    #[test]
    fn the_settings_round_trip_through_the_kernel_s_termios_layout() {
        let settings = Termios::SERIAL_CONSOLE;
        let bytes = settings.to_bytes();
        let word = |i: usize| u32::from_ne_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!(
            [word(0), word(1), word(2), word(3)],
            [0x500, 0x5, 0x1cb2, 0x8a3b]
        );
        assert_eq!(bytes[16], 0);
        assert_eq!(&bytes[17..22], b"\x03\x1c\x7f\x15\x04");
        assert_eq!((bytes[17 + index::VMIN], bytes[17 + index::VTIME]), (1, 0));
        assert_eq!(Termios::from_bytes(&bytes), settings);
    }
}
