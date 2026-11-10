//! The network clock's record (D225, docs/ux/date-time-v0.3.md section 9.1): what the last
//! `date --sync` did, in [`PATH`], as `key=value` lines. `date --sync` writes it (through a
//! temporary file renamed over it), Settings and `settings get time.sync` read it. It is in
//! `/run`, so every boot starts without one. It also keeps the automatic clock's unproven
//! budget: the sum of the steps applied without an HTTPS proof since boot or the last proven
//! step.

/// Where the record is.
pub const PATH: &str = "/run/eggos/clock-sync";
/// The folder the record is in.
pub const FOLDER: &str = "/run/eggos";
/// The start of a temporary file's name, followed by 8 random hex digits.
pub const TEMPORARY_PREFIX: &str = "/run/eggos/clock-sync.";
/// The most bytes of a time server's name a record keeps (`host[:port]`).
pub const SERVER_CAPACITY: usize = 262;
/// The most bytes of a reason a record keeps.
pub const REASON_CAPACITY: usize = 200;
/// The most bytes a record takes; a longer file holds no record.
pub const LARGEST: usize = 1024;

/// What a sync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Running now.
    Syncing,
    /// A step applied without a second source (or none needed).
    Set,
    /// A step applied because HTTPS `Date` headers agreed.
    Confirmed,
    /// A step not applied: no roots, HTTPS unreachable, or HTTPS disagreed.
    Unconfirmed,
    /// No usable answer from the time server.
    NoAnswer,
    /// The network adapter had no address.
    NoAddress,
    /// The kernel refused to set the clock.
    NotAllowed,
}

impl Outcome {
    /// Every outcome, in the order of the design's table.
    pub const ALL: [Self; 7] = [
        Self::Syncing,
        Self::Set,
        Self::Confirmed,
        Self::Unconfirmed,
        Self::NoAnswer,
        Self::NoAddress,
        Self::NotAllowed,
    ];

    /// The outcome's word in the record and in `settings get time.sync`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Syncing => "syncing",
            Self::Set => "set",
            Self::Confirmed => "confirmed",
            Self::Unconfirmed => "unconfirmed",
            Self::NoAnswer => "no-answer",
            Self::NoAddress => "no-address",
            Self::NotAllowed => "not-allowed",
        }
    }

    /// The outcome named `word`.
    pub fn named(word: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.name().as_bytes() == word)
    }
}

/// One record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// What the sync did.
    pub outcome: Outcome,
    /// When it started, in seconds since 1970 (UTC).
    pub started: i64,
    /// When it ended; `None` while it runs.
    pub finished: Option<i64>,
    /// How far the clock moved (`set`, `confirmed`) or was found off (`unconfirmed`), in
    /// milliseconds, positive forward; 0 otherwise.
    pub moved_ms: i64,
    /// The automatic clock's unproven budget used so far, in milliseconds.
    pub unproven_ms: u64,
    server: [u8; SERVER_CAPACITY],
    server_length: usize,
    reason: [u8; REASON_CAPACITY],
    reason_length: usize,
}

fn printable_copy(from: &[u8], into: &mut [u8]) -> usize {
    let length = from.len().min(into.len());
    for (slot, &byte) in into.iter_mut().zip(&from[..length]) {
        *slot = match byte {
            b' '..=b'~' => byte,
            _ => b'?',
        };
    }
    length
}

impl Record {
    /// A record of `outcome` for a sync with `server` that started at `started`.
    pub fn new(outcome: Outcome, started: i64, server: &[u8]) -> Self {
        let mut record = Self {
            outcome,
            started,
            finished: None,
            moved_ms: 0,
            unproven_ms: 0,
            server: [0; SERVER_CAPACITY],
            server_length: 0,
            reason: [0; REASON_CAPACITY],
            reason_length: 0,
        };
        record.server_length = printable_copy(server, &mut record.server);
        record
    }

    /// The time server, as it was given.
    pub fn server(&self) -> &[u8] {
        &self.server[..self.server_length]
    }

    /// `date --sync`'s own one-line text for a failure; empty for none.
    pub fn reason(&self) -> &[u8] {
        &self.reason[..self.reason_length]
    }

    /// Keeps `reason` (cut to [`REASON_CAPACITY`], unprintable bytes as `?`).
    pub fn set_reason(&mut self, reason: &[u8]) {
        let reason = reason.strip_suffix(b"\n").unwrap_or(reason);
        self.reason_length = printable_copy(reason, &mut self.reason);
    }

    /// Reads a record from `file`; `None` when it holds none.
    pub fn parse(file: &[u8]) -> Option<Self> {
        if file.len() > LARGEST {
            return None;
        }
        let mut outcome = None;
        let mut started = None;
        let mut record = Self::new(Outcome::Syncing, 0, b"");
        for line in file.split(|&byte| byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(equals) = line.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            let (key, value) = (&line[..equals], &line[equals + 1..]);
            match key {
                b"result" => outcome = Outcome::named(value),
                b"started" => started = number(value),
                b"finished" => record.finished = number(value),
                b"server" => record.server_length = printable_copy(value, &mut record.server),
                b"moved_ms" | b"offset_ms" => record.moved_ms = number(value).unwrap_or(0),
                b"unproven_ms" => {
                    record.unproven_ms = number(value).map_or(0, i64::unsigned_abs);
                }
                b"reason" => record.set_reason(value),
                _ => {}
            }
        }
        record.outcome = outcome?;
        record.started = started?;
        if record.outcome == Outcome::Syncing {
            record.finished = None;
        }
        Some(record)
    }

    /// Writes the record into `out` as its file; its length, or `None` when `out` is too small.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let mut writer = Lines { out, length: 0 };
        writer.put(&[b"result=", self.outcome.name().as_bytes(), b"\n"])?;
        writer.number(b"started=", self.started)?;
        if let Some(finished) = self.finished {
            writer.number(b"finished=", finished)?;
        }
        writer.put(&[b"server=", self.server(), b"\n"])?;
        match self.outcome {
            Outcome::Set | Outcome::Confirmed => writer.number(b"moved_ms=", self.moved_ms)?,
            Outcome::Unconfirmed => writer.number(b"offset_ms=", self.moved_ms)?,
            _ => {}
        }
        writer.number(b"unproven_ms=", self.unproven_ms.min(i64::MAX as u64) as i64)?;
        if !self.reason().is_empty() {
            writer.put(&[b"reason=", self.reason(), b"\n"])?;
        }
        Some(writer.length)
    }
}

fn number(text: &[u8]) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix(b"-") {
        Some(digits) => (true, digits),
        None => (false, text),
    };
    if digits.is_empty() || digits.len() > 18 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0i64, |value, digit| value * 10 + i64::from(digit - b'0'));
    Some(if negative { -value } else { value })
}

struct Lines<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Lines<'_> {
    fn put(&mut self, parts: &[&[u8]]) -> Option<()> {
        for part in parts {
            let end = self.length.checked_add(part.len())?;
            self.out.get_mut(self.length..end)?.copy_from_slice(part);
            self.length = end;
        }
        Some(())
    }

    fn number(&mut self, key: &[u8], value: i64) -> Option<()> {
        let mut digits = [0u8; 20];
        let mut start = digits.len();
        let mut rest = value.unsigned_abs();
        loop {
            start -= 1;
            digits[start] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        if value < 0 {
            start -= 1;
            digits[start] = b'-';
        }
        self.put(&[key, &digits[start..], b"\n"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(record: &Record) -> Vec<u8> {
        let mut out = [0u8; LARGEST];
        let length = record.write(&mut out).unwrap();
        out[..length].to_vec()
    }

    #[test]
    fn a_record_writes_the_design_s_keys_and_reads_back() {
        let mut record = Record::new(Outcome::Set, 1_793_282_700, b"pool.ntp.org");
        record.finished = Some(1_793_282_709);
        record.moved_ms = 12_000;
        record.unproven_ms = 12_000;
        let file = written(&record);
        assert_eq!(
            file,
            b"result=set\nstarted=1793282700\nfinished=1793282709\nserver=pool.ntp.org\n\
              moved_ms=12000\nunproven_ms=12000\n"
        );
        assert_eq!(Record::parse(&file), Some(record));
        let mut refused = Record::new(Outcome::Unconfirmed, 5, b"time.example.com:123");
        refused.finished = Some(9);
        refused.moved_ms = -10_920_000;
        refused.set_reason(b"pool.ntp.org: unproven\x1b\n");
        let file = written(&refused);
        assert!(
            file.ends_with(b"offset_ms=-10920000\nunproven_ms=0\nreason=pool.ntp.org: unproven?\n")
        );
        assert_eq!(Record::parse(&file), Some(refused));
    }

    #[test]
    fn a_running_sync_has_no_finish_and_a_broken_file_no_record() {
        let syncing = Record::new(Outcome::Syncing, 7, b"pool.ntp.org");
        let file = written(&syncing);
        assert_eq!(
            file,
            b"result=syncing\nstarted=7\nserver=pool.ntp.org\nunproven_ms=0\n"
        );
        assert_eq!(Record::parse(&file), Some(syncing));
        assert_eq!(Record::parse(b""), None);
        assert_eq!(Record::parse(b"result=maybe\nstarted=1\n"), None);
        assert_eq!(Record::parse(b"result=set\n"), None);
        assert_eq!(Record::parse(&[b'x'; LARGEST + 1]), None);
        for outcome in Outcome::ALL {
            assert_eq!(Outcome::named(outcome.name().as_bytes()), Some(outcome));
        }
    }
}
