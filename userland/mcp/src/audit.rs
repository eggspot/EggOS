//! The audit log (D136): `/var/log/agent-audit.log`, one record per line, its fields separated
//! by tabs: the time (seconds of the wall clock), the call's number since the server started,
//! the client's name, the tool, the outcome, the arguments and a detail (a size, an exit
//! status or an error, never what a command printed). A call that runs a command writes a
//! `started` record, then its outcome under the same number; one stopped before (an unknown
//! tool, bad arguments, a refused path) writes one record.
//!
//! Every field a client could have chosen is escaped when written, so a record is one line and
//! shows what was sent: a backslash as `\\`, tab, newline and return as `\t`, `\n`, `\r`, escape
//! as `\e`, other control bytes as `\xHH`, invisible and direction-changing characters (C1
//! controls, Unicode format characters such as U+202E, line and paragraph separators) as
//! `\u{202e}`, and invalid UTF-8 as U+FFFD. Each field keeps at most [`FIELD_BYTES`] bytes,
//! ending `...(cut)` when cut. Reading ([`parse_line`], [`display_into`]) escapes again, so a
//! log edited by other hands cannot hide anything from the person reading it either.

/// Where the log is.
pub const LOG_PATH: &str = "/var/log/agent-audit.log";
/// Where the previous log goes once the log passes [`ROTATE_BYTES`].
pub const ROTATED_PATH: &str = "/var/log/agent-audit.log.1";
/// The folder the log is in.
pub const LOG_FOLDER: &str = "/var/log";
/// How large the log grows before it is rotated.
pub const ROTATE_BYTES: u64 = 256 * 1024;
/// The most bytes a free field keeps.
pub const FIELD_BYTES: usize = 512;
/// The most bytes one record takes, its newline included.
pub const RECORD_BYTES: usize = 4 * FIELD_BYTES + 128;

const CUT: &[u8] = b"...(cut)";

/// What a record says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The server started.
    ServerStart,
    /// The server started again after it ended.
    ServerRestart,
    /// A client initialized a session.
    Initialize,
    /// The client's side of the channel closed.
    Disconnected,
    /// A call's command is about to run.
    Started,
    /// A call succeeded.
    Ok,
    /// A call failed: bad arguments, an unknown tool, or a command that failed.
    Error,
    /// A call was refused: a path outside the roots.
    Refused,
}

impl Outcome {
    /// Its word in the log.
    pub fn word(self) -> &'static str {
        match self {
            Outcome::ServerStart => "server-start",
            Outcome::ServerRestart => "server-restart",
            Outcome::Initialize => "initialize",
            Outcome::Disconnected => "disconnected",
            Outcome::Started => "started",
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Refused => "refused",
        }
    }

    /// The outcome its word names.
    pub fn from_word(word: &[u8]) -> Option<Self> {
        [
            Outcome::ServerStart,
            Outcome::ServerRestart,
            Outcome::Initialize,
            Outcome::Disconnected,
            Outcome::Started,
            Outcome::Ok,
            Outcome::Error,
            Outcome::Refused,
        ]
        .into_iter()
        .find(|outcome| outcome.word().as_bytes() == word)
    }

    /// Whether it ends a call ([`Outcome::Ok`], [`Outcome::Error`] or [`Outcome::Refused`]).
    pub fn ends_a_call(self) -> bool {
        matches!(self, Outcome::Ok | Outcome::Error | Outcome::Refused)
    }
}

/// A record's fields before they are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The wall clock's seconds.
    pub time: u64,
    /// The call's number since the server started; 0 for the server's and session's records.
    pub call: u64,
    /// The client's name.
    pub client: &'a [u8],
    /// The tool, or `-`.
    pub tool: &'a [u8],
    /// What happened.
    pub outcome: Outcome,
    /// The arguments.
    pub arguments: &'a [u8],
    /// The detail.
    pub detail: &'a [u8],
}

/// A record read back, its free fields as stored (escaped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// The wall clock's seconds.
    pub time: u64,
    /// The call's number.
    pub call: u64,
    /// The client's name.
    pub client: &'a [u8],
    /// The tool.
    pub tool: &'a [u8],
    /// What happened.
    pub outcome: Outcome,
    /// The arguments.
    pub arguments: &'a [u8],
    /// The detail.
    pub detail: &'a [u8],
}

/// Writes `entry` as one line into `out` ([`RECORD_BYTES`] or more); returns its length.
pub fn write_record(entry: &Entry<'_>, out: &mut [u8]) -> usize {
    let mut length = 0;
    let mut put = |bytes: &[u8], length: &mut usize| {
        if let Some(place) = out.get_mut(*length..*length + bytes.len()) {
            place.copy_from_slice(bytes);
            *length += bytes.len();
        }
    };
    let mut digits = [0u8; 20];
    put(decimal(entry.time, &mut digits), &mut length);
    put(b"\t", &mut length);
    put(decimal(entry.call, &mut digits), &mut length);
    let mut field = [0u8; FIELD_BYTES];
    for text in [entry.client, entry.tool] {
        put(b"\t", &mut length);
        let count = escape_field(text, &mut field, true);
        put(field.get(..count).unwrap_or(&[]), &mut length);
    }
    put(b"\t", &mut length);
    put(entry.outcome.word().as_bytes(), &mut length);
    for text in [entry.arguments, entry.detail] {
        put(b"\t", &mut length);
        let count = escape_field(text, &mut field, true);
        put(field.get(..count).unwrap_or(&[]), &mut length);
    }
    put(b"\n", &mut length);
    length
}

fn decimal(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &digits[start..];
        }
    }
}

/// Reads one line of the log; `None` for a line that is not a record.
pub fn parse_line(line: &[u8]) -> Option<Record<'_>> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let mut fields = line.split(|&byte| byte == b'\t');
    let number = |field: &[u8]| -> Option<u64> {
        if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
            return None;
        }
        core::str::from_utf8(field).ok()?.parse().ok()
    };
    let time = number(fields.next()?)?;
    let call = number(fields.next()?)?;
    let client = fields.next()?;
    let tool = fields.next()?;
    let outcome = Outcome::from_word(fields.next()?)?;
    let arguments = fields.next()?;
    let detail = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    Some(Record {
        time,
        call,
        client,
        tool,
        outcome,
        arguments,
        detail,
    })
}

/// Writes a stored field for display into `out`: as stored, but any control byte, hidden
/// character or invalid UTF-8 escaped again (its backslashes are kept as they are); returns
/// the length.
pub fn display_into(field: &[u8], out: &mut [u8]) -> usize {
    escape_field(field, out, false)
}

/// Whether `character` would be invisible or change how text around it shows: a control
/// character, a Unicode format character (general category Cf) or a line or paragraph
/// separator.
pub fn is_hidden(character: char) -> bool {
    matches!(
        character,
        '\0'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

fn escape_character(character: char, escape_backslash: bool, buffer: &mut [u8; 12]) -> &[u8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let simple: Option<&[u8]> = match character {
        '\\' if escape_backslash => Some(b"\\\\"),
        '\t' => Some(b"\\t"),
        '\n' => Some(b"\\n"),
        '\r' => Some(b"\\r"),
        '\u{1b}' => Some(b"\\e"),
        _ => None,
    };
    if let Some(simple) = simple {
        buffer[..simple.len()].copy_from_slice(simple);
        return &buffer[..simple.len()];
    }
    if !is_hidden(character) {
        return character.encode_utf8(buffer).as_bytes();
    }
    let code = u32::from(character);
    if code < 0x80 {
        buffer[..4].copy_from_slice(&[
            b'\\',
            b'x',
            HEX[(code >> 4) as usize & 15],
            HEX[code as usize & 15],
        ]);
        return &buffer[..4];
    }
    buffer[..3].copy_from_slice(b"\\u{");
    let mut length = 3;
    let digits = (32 - code.leading_zeros()).div_ceil(4).max(2);
    for shift in (0..digits).rev() {
        buffer[length] = HEX[(code >> (shift * 4)) as usize & 15];
        length += 1;
    }
    buffer[length] = b'}';
    &buffer[..length + 1]
}

fn escape_field(text: &[u8], out: &mut [u8], escape_backslash: bool) -> usize {
    let capacity = out.len();
    let mut length = 0;
    let mut last_safe = 0;
    let mut piece = [0u8; 12];
    for chunk in text.utf8_chunks() {
        let characters = chunk.valid().chars().map(Some);
        let invalid = (!chunk.invalid().is_empty()).then_some(None);
        for character in characters.chain(invalid) {
            let bytes = match character {
                Some(character) => escape_character(character, escape_backslash, &mut piece),
                None => "\u{fffd}".as_bytes(),
            };
            if length + bytes.len() > capacity {
                let cut = last_safe.min(capacity.saturating_sub(CUT.len()));
                out[cut..cut + CUT.len()].copy_from_slice(CUT);
                return cut + CUT.len();
            }
            out[length..length + bytes.len()].copy_from_slice(bytes);
            length += bytes.len();
            if length + CUT.len() <= capacity {
                last_safe = length;
            }
        }
    }
    length
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(entry: &Entry<'_>) -> String {
        let mut out = [0u8; RECORD_BYTES];
        let length = write_record(entry, &mut out);
        String::from_utf8(out[..length].to_vec()).unwrap()
    }

    fn entry<'a>(client: &'a [u8], arguments: &'a [u8]) -> Entry<'a> {
        Entry {
            time: 12,
            call: 3,
            client,
            tool: b"cat",
            outcome: Outcome::Started,
            arguments,
            detail: b"",
        }
    }

    #[test]
    fn a_record_is_one_line_of_tab_separated_fields_and_reads_back() {
        let line = record(&Entry {
            detail: b"42 bytes",
            outcome: Outcome::Ok,
            ..entry(b"claude-code 2.1", br#"{"path":"/home/a"}"#)
        });
        assert_eq!(
            line,
            "12\t3\tclaude-code 2.1\tcat\tok\t{\"path\":\"/home/a\"}\t42 bytes\n"
        );
        let read = parse_line(line.as_bytes()).unwrap();
        assert_eq!(read.time, 12);
        assert_eq!(read.call, 3);
        assert_eq!(read.client, b"claude-code 2.1");
        assert_eq!(read.outcome, Outcome::Ok);
        assert_eq!(read.detail, b"42 bytes");
        assert!(read.outcome.ends_a_call());
        assert!(!Outcome::Started.ends_a_call());
    }

    #[test]
    fn client_chosen_text_cannot_break_or_disguise_a_record() {
        let client = "evil\u{1b}[2J\u{202e}gnp.exe\ttab\nnew\\".as_bytes();
        let path = "{\"path\":\"/home/\u{202e}txt.exe\u{200b}\"}".as_bytes();
        let line = record(&entry(client, path));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(line.matches('\t').count(), 6);
        assert!(line.contains("evil\\e[2J\\u{202e}gnp.exe\\ttab\\nnew\\\\"));
        assert!(line.contains("/home/\\u{202e}txt.exe\\u{200b}"));
        assert!(
            !line
                .chars()
                .any(|character| character != '\t' && character != '\n' && is_hidden(character))
        );
        let invalid = record(&entry(b"bad\xff\xfe", b"\x01\x7f\xc2\x85"));
        assert!(invalid.contains("bad\u{fffd}\u{fffd}"));
        assert!(invalid.contains("\\x01\\x7f\\u{85}"));
    }

    #[test]
    fn reading_escapes_again_but_keeps_stored_backslashes() {
        let mut out = [0u8; 64];
        let length = display_into("a\\e\u{1b}b\u{202e}c\t".as_bytes(), &mut out);
        assert_eq!(&out[..length], b"a\\e\\eb\\u{202e}c\\t");
        let length = display_into(b"\xff", &mut out);
        assert_eq!(&out[..length], "\u{fffd}".as_bytes());
    }

    #[test]
    fn long_fields_are_cut_with_a_marker_at_a_character_boundary() {
        let long = "é".repeat(FIELD_BYTES);
        let line = record(&entry(b"c", long.as_bytes()));
        let arguments = line.split('\t').nth(5).unwrap();
        assert!(arguments.len() <= FIELD_BYTES);
        assert!(arguments.ends_with("...(cut)"));
        assert!(arguments.starts_with("éé"));
        let exact = "a".repeat(FIELD_BYTES);
        let line = record(&entry(b"c", exact.as_bytes()));
        assert_eq!(line.split('\t').nth(5).unwrap(), exact);
        let worst = "\u{202e}".repeat(FIELD_BYTES);
        let line = record(&entry(worst.as_bytes(), worst.as_bytes()));
        assert!(line.len() <= RECORD_BYTES);
    }

    #[test]
    fn lines_that_are_not_records_are_skipped() {
        for line in [
            &b""[..],
            b"x\t1\tc\tt\tok\ta\td",
            b"1\t1\tc\tt\tmaybe\ta\td",
            b"1\t1\tc\tt\tok\ta",
            b"1\t1\tc\tt\tok\ta\td\textra",
        ] {
            assert_eq!(parse_line(line), None);
        }
        assert!(parse_line(b"1\t0\t-\t-\tserver-start\t\t\n").is_some());
    }
}
