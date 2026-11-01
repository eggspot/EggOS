//! Android's log (D188): the datagrams Android programs send `logd` through
//! `/dev/socket/logdw` ([`WRITER_PATH`]), and the records the kernel keeps them as, which
//! `logcat` reads from `/dev/android_log` ([`LOG_PATH`]).
//!
//! A datagram is liblog's: an [`HEADER_SIZE`]-byte header (the log buffer's id, the sending
//! thread's id in 16 bits, and the time it was sent, seconds and nanoseconds of the real-time
//! clock), then, for the text buffers, a priority byte, the tag and its NUL, and the message
//! and its NUL. All numbers are little-endian, as on every machine Android runs on. At most
//! [`MAX_PAYLOAD`] bytes follow the header; the rest is cut off.
//!
//! A record is the kernel's own layout of one entry: its size in two bytes, the buffer, the
//! priority, the process and thread ids, the time, the tag's length in two bytes, the tag and
//! the message, every number little-endian. The log holds whole records, oldest first, in at
//! most [`CAPACITY`] bytes. Nothing here allocates or panics.

#![cfg_attr(not(test), no_std)]

use core::ffi::CStr;

/// Where Android programs send their log datagrams: logd's socket.
pub const WRITER_PATH: &CStr = c"/dev/socket/logdw";
/// The folder of Android's sockets in `/dev`.
pub const SOCKETS_NAME: &[u8] = b"socket";
/// The writer socket's name in [`SOCKETS_NAME`].
pub const WRITER_NAME: &[u8] = b"logdw";
/// Where `logcat` reads the log: a read-only file of records.
pub const LOG_PATH: &CStr = c"/dev/android_log";
/// The log file's name in `/dev`.
pub const LOG_NAME: &[u8] = b"android_log";
/// A datagram's header: buffer id, thread id, seconds, nanoseconds.
pub const HEADER_SIZE: usize = 11;
/// The most bytes of a datagram after its header: liblog's own limit.
pub const MAX_PAYLOAD: usize = 4068;
/// A record's bytes before its tag.
pub const RECORD_HEADER_SIZE: usize = 22;
/// The largest record.
pub const MAX_RECORD: usize = RECORD_HEADER_SIZE + MAX_PAYLOAD;
/// The most bytes of records the log holds; the oldest go first.
pub const CAPACITY: usize = 64 * 1024;
/// The size of [`SNAPSHOT_REQUEST`]'s buffer: a 4-byte length, then up to [`CAPACITY`] bytes
/// of records.
pub const SNAPSHOT_SIZE: usize = 4 + CAPACITY;
/// The `ioctl` request on the log file that copies the whole log at once into a
/// [`SNAPSHOT_SIZE`]-byte buffer, its length first: a read never sees records half replaced.
pub const SNAPSHOT_REQUEST: u32 = 0x4c01;
/// The `ioctl` request on the log file that empties the log (`logcat -c`), for a privileged
/// process.
pub const CLEAR_REQUEST: u32 = 0x4c02;

const SIZE_AT: usize = 0;
const BUFFER_AT: usize = 2;
const PRIORITY_AT: usize = 3;
const PID_AT: usize = 4;
const TID_AT: usize = 8;
const SECONDS_AT: usize = 12;
const NANOSECONDS_AT: usize = 16;
const TAG_LENGTH_AT: usize = 20;

/// The log buffers liblog names by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buffer {
    /// The programs' log (`LOG_ID_MAIN`).
    Main,
    /// The radio's log.
    Radio,
    /// Binary event records.
    Events,
    /// The system's services' log.
    System,
    /// Crash reports.
    Crash,
    /// Binary statistics.
    Stats,
    /// Binary security records.
    Security,
    /// The kernel's messages.
    Kernel,
}

impl Buffer {
    /// The buffer with liblog's `id`.
    pub const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Main,
            1 => Self::Radio,
            2 => Self::Events,
            3 => Self::System,
            4 => Self::Crash,
            5 => Self::Stats,
            6 => Self::Security,
            7 => Self::Kernel,
            _ => return None,
        })
    }

    /// Its id.
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Whether its entries are text (priority, tag, message); the others are binary.
    pub const fn is_text(self) -> bool {
        !matches!(self, Self::Events | Self::Stats | Self::Security)
    }
}

/// Android's log priorities (`android_LogPriority`).
pub mod priority {
    /// Verbose.
    pub const VERBOSE: u8 = 2;
    /// Debug.
    pub const DEBUG: u8 = 3;
    /// Information.
    pub const INFO: u8 = 4;
    /// Warning.
    pub const WARN: u8 = 5;
    /// Error.
    pub const ERROR: u8 = 6;
    /// Fatal.
    pub const FATAL: u8 = 7;
    /// Above every other: a filter that shows nothing.
    pub const SILENT: u8 = 8;
}

/// `priority`'s letter in `logcat`'s output: `V`, `D`, `I`, `W`, `E`, `F` or `S`; `?` for
/// another number.
pub const fn priority_letter(priority: u8) -> u8 {
    match priority {
        priority::VERBOSE => b'V',
        priority::DEBUG => b'D',
        priority::INFO => b'I',
        priority::WARN => b'W',
        priority::ERROR => b'E',
        priority::FATAL => b'F',
        priority::SILENT => b'S',
        _ => b'?',
    }
}

/// The priority a filter's letter names, in either case.
pub const fn priority_of_letter(letter: u8) -> Option<u8> {
    Some(match letter.to_ascii_uppercase() {
        b'V' => priority::VERBOSE,
        b'D' => priority::DEBUG,
        b'I' => priority::INFO,
        b'W' => priority::WARN,
        b'E' => priority::ERROR,
        b'F' => priority::FATAL,
        b'S' => priority::SILENT,
        _ => return None,
    })
}

/// One entry of the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The buffer it went to.
    pub buffer: Buffer,
    /// Its priority ([`priority`]).
    pub priority: u8,
    /// The sending process's id.
    pub pid: u32,
    /// The sending thread's id, as the datagram carried it.
    pub tid: u32,
    /// When it was sent: seconds of the real-time clock.
    pub seconds: u32,
    /// And nanoseconds.
    pub nanoseconds: u32,
    /// Its tag, without NUL.
    pub tag: &'a [u8],
    /// Its message, without its last NUL.
    pub message: &'a [u8],
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn until_nul(bytes: &[u8]) -> (&[u8], &[u8]) {
    match bytes.iter().position(|&byte| byte == 0) {
        Some(end) => (&bytes[..end], &bytes[end + 1..]),
        None => (bytes, &[]),
    }
}

/// The entry `datagram` carries, sent by process `pid`: `None` for one shorter than its header
/// and priority, for an unknown buffer, and for the binary buffers.
pub fn parse_datagram(datagram: &[u8], pid: u32) -> Option<Entry<'_>> {
    let buffer = Buffer::from_id(*datagram.first()?)?;
    if !buffer.is_text() {
        return None;
    }
    let payload = datagram.get(HEADER_SIZE..)?;
    let payload = &payload[..payload.len().min(MAX_PAYLOAD)];
    let (&priority, text) = payload.split_first()?;
    let (tag, rest) = until_nul(text);
    let (message, _) = until_nul(rest);
    Some(Entry {
        buffer,
        priority,
        pid,
        tid: u32::from(u16_at(datagram, 1)?),
        seconds: u32_at(datagram, 3)?,
        nanoseconds: u32_at(datagram, 7)?,
        tag,
        message,
    })
}

/// Writes `entry` as liblog sends it into `out`: the datagram's length, `None` if it does not
/// fit or is longer than a datagram may be.
pub fn encode_datagram(entry: &Entry<'_>, out: &mut [u8]) -> Option<usize> {
    let size = HEADER_SIZE + 1 + entry.tag.len() + 1 + entry.message.len() + 1;
    if size - HEADER_SIZE > MAX_PAYLOAD {
        return None;
    }
    let out = out.get_mut(..size)?;
    out[0] = entry.buffer.id();
    out[1..3].copy_from_slice(&(entry.tid as u16).to_le_bytes());
    out[3..7].copy_from_slice(&entry.seconds.to_le_bytes());
    out[7..11].copy_from_slice(&entry.nanoseconds.to_le_bytes());
    out[HEADER_SIZE] = entry.priority;
    let tag_at = HEADER_SIZE + 1;
    out[tag_at..tag_at + entry.tag.len()].copy_from_slice(entry.tag);
    out[tag_at + entry.tag.len()] = 0;
    let message_at = tag_at + entry.tag.len() + 1;
    out[message_at..message_at + entry.message.len()].copy_from_slice(entry.message);
    out[size - 1] = 0;
    Some(size)
}

/// The size of `entry`'s record.
pub const fn record_size(entry: &Entry<'_>) -> usize {
    RECORD_HEADER_SIZE + entry.tag.len() + entry.message.len()
}

/// Writes `entry`'s record into `out`: its size, `None` if it does not fit or is larger than
/// [`MAX_RECORD`].
pub fn encode_record(entry: &Entry<'_>, out: &mut [u8]) -> Option<usize> {
    let size = record_size(entry);
    if size > MAX_RECORD {
        return None;
    }
    let out = out.get_mut(..size)?;
    out[SIZE_AT..SIZE_AT + 2].copy_from_slice(&(size as u16).to_le_bytes());
    out[BUFFER_AT] = entry.buffer.id();
    out[PRIORITY_AT] = entry.priority;
    out[PID_AT..PID_AT + 4].copy_from_slice(&entry.pid.to_le_bytes());
    out[TID_AT..TID_AT + 4].copy_from_slice(&entry.tid.to_le_bytes());
    out[SECONDS_AT..SECONDS_AT + 4].copy_from_slice(&entry.seconds.to_le_bytes());
    out[NANOSECONDS_AT..NANOSECONDS_AT + 4].copy_from_slice(&entry.nanoseconds.to_le_bytes());
    out[TAG_LENGTH_AT..TAG_LENGTH_AT + 2].copy_from_slice(&(entry.tag.len() as u16).to_le_bytes());
    let message_at = RECORD_HEADER_SIZE + entry.tag.len();
    out[RECORD_HEADER_SIZE..message_at].copy_from_slice(entry.tag);
    out[message_at..size].copy_from_slice(entry.message);
    Some(size)
}

/// The size of the record `bytes` starts with, as its first two bytes give it: `None` for
/// fewer than two bytes.
pub fn record_size_at(bytes: &[u8]) -> Option<usize> {
    u16_at(bytes, SIZE_AT).map(usize::from)
}

/// The entry of the record `bytes` starts with, and the record's size: `None` for a record
/// that is cut off or damaged.
pub fn decode_record(bytes: &[u8]) -> Option<(Entry<'_>, usize)> {
    let size = record_size_at(bytes)?;
    if size < RECORD_HEADER_SIZE {
        return None;
    }
    let record = bytes.get(..size)?;
    let tag_length = usize::from(u16_at(record, TAG_LENGTH_AT)?);
    let message_at = RECORD_HEADER_SIZE.checked_add(tag_length)?;
    let entry = Entry {
        buffer: Buffer::from_id(record[BUFFER_AT])?,
        priority: record[PRIORITY_AT],
        pid: u32_at(record, PID_AT)?,
        tid: u32_at(record, TID_AT)?,
        seconds: u32_at(record, SECONDS_AT)?,
        nanoseconds: u32_at(record, NANOSECONDS_AT)?,
        tag: record.get(RECORD_HEADER_SIZE..message_at)?,
        message: record.get(message_at..)?,
    };
    Some((entry, size))
}

/// The entries of `bytes`, records one after another, oldest first.
pub fn records(bytes: &[u8]) -> Records<'_> {
    Records { rest: bytes }
}

/// The entries of a run of records ([`records`]); it ends at the first record that is cut off
/// or damaged.
#[derive(Debug, Clone)]
pub struct Records<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Records<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        let Some((entry, size)) = decode_record(self.rest) else {
            self.rest = &[];
            return None;
        };
        self.rest = &self.rest[size..];
        Some(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry<'static> {
        Entry {
            buffer: Buffer::Main,
            priority: priority::INFO,
            pid: 42,
            tid: 43,
            seconds: 1_700_000_000,
            nanoseconds: 123_456_789,
            tag: b"eggos-test",
            message: b"hello from android-console",
        }
    }

    #[test]
    fn a_datagram_is_liblog_s_header_then_priority_tag_and_message() {
        let mut datagram = [0u8; 64];
        let length = encode_datagram(&entry(), &mut datagram).unwrap();
        assert_eq!(length, HEADER_SIZE + 1 + 11 + 27);
        assert_eq!(&datagram[..3], &[0, 43, 0]);
        assert_eq!(&datagram[3..7], &1_700_000_000u32.to_le_bytes());
        assert_eq!(datagram[HEADER_SIZE], priority::INFO);
        assert_eq!(&datagram[12..23], b"eggos-test\0");
        assert_eq!(parse_datagram(&datagram[..length], 42), Some(entry()));
    }

    #[test]
    fn short_binary_and_unknown_datagrams_carry_no_entry() {
        let mut datagram = [0u8; 64];
        let length = encode_datagram(&entry(), &mut datagram).unwrap();
        assert!(parse_datagram(&datagram[..HEADER_SIZE], 1).is_none());
        datagram[0] = Buffer::Events.id();
        assert!(parse_datagram(&datagram[..length], 1).is_none());
        datagram[0] = 8;
        assert!(parse_datagram(&datagram[..length], 1).is_none());
    }

    #[test]
    fn a_tag_without_nul_has_no_message_and_a_long_payload_is_cut() {
        let mut datagram = [b'x'; HEADER_SIZE + MAX_PAYLOAD + 10];
        datagram[0] = Buffer::System.id();
        datagram[HEADER_SIZE] = priority::WARN;
        let parsed = parse_datagram(&datagram, 7).unwrap();
        assert_eq!(parsed.buffer, Buffer::System);
        assert_eq!(parsed.tag.len(), MAX_PAYLOAD - 1);
        assert!(parsed.message.is_empty());
        datagram[HEADER_SIZE + 2] = 0;
        let parsed = parse_datagram(&datagram, 7).unwrap();
        assert_eq!(parsed.tag, b"x");
        assert_eq!(parsed.message.len(), MAX_PAYLOAD - 3);
    }

    #[test]
    fn records_hold_entries_one_after_another() {
        let mut log = [0u8; 256];
        let first = encode_record(&entry(), &mut log).unwrap();
        assert_eq!(first, record_size(&entry()));
        let second = Entry {
            priority: priority::ERROR,
            tag: b"other",
            message: b"line one\nline two",
            ..entry()
        };
        let size = encode_record(&second, &mut log[first..]).unwrap();
        let entries: Vec<_> = records(&log[..first + size]).collect();
        assert_eq!(entries, [entry(), second]);
        assert_eq!(records(&log[..first + size - 1]).count(), 1);
        assert_eq!(record_size_at(&log), Some(first));
    }

    #[test]
    fn letters_name_priorities_both_ways() {
        for priority in priority::VERBOSE..=priority::SILENT {
            assert_eq!(
                priority_of_letter(priority_letter(priority)),
                Some(priority)
            );
        }
        assert_eq!(priority_of_letter(b'w'), Some(priority::WARN));
        assert_eq!(priority_letter(0), b'?');
        assert_eq!(priority_of_letter(b'X'), None);
    }
}
