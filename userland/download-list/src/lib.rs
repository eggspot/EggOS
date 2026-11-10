//! Downloads as `fetch` records them and as people read them (D225, docs/ux/downloads-v0.3.md
//! section 4): the record each `fetch` that saves to a file keeps in
//! `/run/eggos/downloads/<pid>`, the judgement of a record whose `fetch` is gone, the order of
//! the list, cancelling and clearing, and the words of the Downloads window's columns, which
//! `download list` and the MCP tools give too (D22). No allocator.

#![cfg_attr(not(test), no_std)]

use core::cmp::Ordering;

/// Where each `fetch` that saves to a file keeps its record, named by its process id.
pub const FOLDER: &str = "/run/eggos/downloads";
/// The folder [`FOLDER`] is made in.
pub const PARENT_FOLDER: &str = "/run/eggos";
/// Where the Downloads window saves what it downloads.
pub const SAVE_FOLDER: &str = "/home/root/Downloads";
/// The most downloads a list holds; the newest are kept.
pub const MAX_DOWNLOADS: usize = 256;
/// The longest record file.
pub const MAX_RECORD_BYTES: usize = 8192;
/// The longest failure reason a record keeps.
pub const MAX_REASON_BYTES: usize = 160;
/// The reason of a `downloading` record whose `fetch` is no longer running.
pub const ENDED_WITHOUT_FINISHING: &str = "fetch ended without finishing";
/// The least time between two writes of a running download's record, in microseconds.
pub const WRITE_INTERVAL_US: u64 = 250_000;
/// The signal that cancels a download: `SIGTERM`.
pub const TERMINATE: u32 = 15;
/// `ESRCH`: no process has the id.
pub const ESRCH: i32 = 3;
/// `EPERM`: the process belongs to another user.
pub const EPERM: i32 = 1;
/// The name of the program that downloads, as `/proc/ps` and a command line's first word show
/// it.
pub const FETCH: &[u8] = b"fetch";
/// The multi-call program `fetch` is a name of, which takes the utility's name as its first
/// argument.
pub const MULTI_CALL: &[u8] = b"coreutils";

/// Where a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Downloading,
    Done,
    Failed,
    Cancelled,
}

impl State {
    /// Every state.
    pub const ALL: [State; 4] = [
        State::Downloading,
        State::Done,
        State::Failed,
        State::Cancelled,
    ];

    /// Its word in a record and in `download list --json`.
    pub const fn word(self) -> &'static str {
        match self {
            State::Downloading => "downloading",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    /// The state whose word is `word`.
    pub fn parse(word: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|state| state.word().as_bytes() == word)
    }

    /// Whether the download is over: done, failed or cancelled.
    pub const fn is_over(self) -> bool {
        !matches!(self, State::Downloading)
    }
}

/// One download's record: one `key=value` line each for `url`, `path`, `received`, `total`
/// (absent without a `Content-Length`), `state`, `reason` (with `failed`), `started` and
/// `ended` (UTC seconds; absent while downloading).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub url: &'a [u8],
    pub path: &'a [u8],
    pub received: u64,
    pub total: Option<u64>,
    pub state: State,
    pub reason: &'a [u8],
    pub started: u64,
    pub ended: Option<u64>,
}

fn number(value: &[u8]) -> Option<u64> {
    if value.is_empty() || value.len() > 20 {
        return None;
    }
    value.iter().try_fold(0u64, |sum, &digit| match digit {
        b'0'..=b'9' => sum.checked_mul(10)?.checked_add(u64::from(digit - b'0')),
        _ => None,
    })
}

/// Writes `value` in decimal through `out`.
pub fn write_decimal(value: u64, out: &mut dyn FnMut(&[u8])) {
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut rest = value;
    loop {
        start -= 1;
        digits[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    out(&digits[start..]);
}

/// Writes `bytes` as Files writes a size (`512 B`, `3.4 KB`, `12 MB`, `1.5 GB`) through `out`.
pub fn write_size(bytes: u64, out: &mut dyn FnMut(&[u8])) {
    out(eggos_files::listing::size_text(bytes).as_str().as_bytes());
}

struct Writer<'o> {
    out: &'o mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.length.checked_add(bytes.len())?;
        self.out.get_mut(self.length..end)?.copy_from_slice(bytes);
        self.length = end;
        Some(())
    }

    fn text(&mut self, key: &[u8], value: &[u8]) -> Option<()> {
        self.put(key)?;
        self.put(b"=")?;
        for &byte in value {
            self.put(&[if byte < b' ' || byte == 0x7f {
                b'?'
            } else {
                byte
            }])?;
        }
        self.put(b"\n")
    }

    fn number(&mut self, key: &[u8], value: u64) -> Option<()> {
        self.put(key)?;
        self.put(b"=")?;
        let mut fits = true;
        write_decimal(value, &mut |digits| fits &= self.put(digits).is_some());
        fits.then_some(())?;
        self.put(b"\n")
    }
}

impl<'a> Record<'a> {
    /// The record a file holds; `None` when a required line is missing or malformed.
    pub fn parse(text: &'a [u8]) -> Option<Self> {
        let mut url = None;
        let mut path = None;
        let mut received = None;
        let mut total = None;
        let mut state = None;
        let mut reason: &[u8] = b"";
        let mut started = None;
        let mut ended = None;
        for line in text.split(|&byte| byte == b'\n') {
            let Some(equals) = line.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            let (key, value) = (&line[..equals], &line[equals + 1..]);
            match key {
                b"url" => url = Some(value),
                b"path" => path = Some(value),
                b"received" => received = number(value),
                b"total" => total = number(value),
                b"state" => state = State::parse(value),
                b"reason" => reason = value,
                b"started" => started = number(value),
                b"ended" => ended = number(value),
                _ => {}
            }
        }
        Some(Self {
            url: url?,
            path: path?,
            received: received?,
            total,
            state: state?,
            reason,
            started: started?,
            ended,
        })
    }

    /// Writes the record's file into `out`, control bytes in its texts as `?`; its length, or
    /// `None` when `out` is too small.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let mut writer = Writer { out, length: 0 };
        writer.text(b"url", self.url)?;
        writer.text(b"path", self.path)?;
        writer.number(b"received", self.received)?;
        if let Some(total) = self.total {
            writer.number(b"total", total)?;
        }
        writer.text(b"state", self.state.word().as_bytes())?;
        if self.state == State::Failed {
            writer.text(b"reason", self.reason)?;
        }
        writer.number(b"started", self.started)?;
        if let Some(ended) = self.ended {
            writer.number(b"ended", ended)?;
        }
        Some(writer.length)
    }

    /// The saved file's name: the last component of its path.
    pub fn name(&self) -> &'a [u8] {
        self.path
            .rsplit(|&byte| byte == b'/')
            .next()
            .unwrap_or(self.path)
    }

    /// Where it came from: the URL's host, port kept.
    pub fn host(&self) -> &'a [u8] {
        host_of(self.url)
    }

    /// Whole percent received, floored, when the total is known.
    pub fn percent(&self) -> Option<u64> {
        self.part_of(100).map(|part| part as u64)
    }

    /// Permille received, floored, when the total is known.
    pub fn permille(&self) -> Option<u16> {
        self.part_of(1000).map(|part| part as u16)
    }

    fn part_of(&self, whole: u128) -> Option<u128> {
        let total = self.total.filter(|&total| total > 0)?;
        Some((u128::from(self.received) * whole / u128::from(total)).min(whole))
    }

    /// The record as people are shown it: a `downloading` record whose `fetch` is not
    /// `running` failed with [`ENDED_WITHOUT_FINISHING`]; `running` is `None` when the
    /// processes could not be read, and the record is taken as it says.
    pub fn judged(self, running: Option<bool>) -> Self {
        match (self.state, running) {
            (State::Downloading, Some(false)) => Self {
                state: State::Failed,
                reason: ENDED_WITHOUT_FINISHING.as_bytes(),
                ..self
            },
            _ => self,
        }
    }

    /// The `Size` column: `3.4 MB of 12 MB` with a total, `3.4 MB` without, and what arrived
    /// once done.
    pub fn write_size(&self, out: &mut dyn FnMut(&[u8])) {
        write_size(self.received, out);
        if let (false, Some(total)) = (self.state == State::Done, self.total) {
            out(b" of ");
            write_size(total, out);
        }
    }

    /// The `State` column: `Downloading, 28 %`, `Downloading`, `Done`, `Failed: <reason>` or
    /// `Cancelled`.
    pub fn write_state(&self, out: &mut dyn FnMut(&[u8])) {
        match self.state {
            State::Downloading => {
                out(b"Downloading");
                if let Some(percent) = self.percent() {
                    out(b", ");
                    write_decimal(percent, out);
                    out(b" %");
                }
            }
            State::Done => out(b"Done"),
            State::Failed if self.reason.is_empty() => out(b"Failed"),
            State::Failed => {
                out(b"Failed: ");
                out(self.reason);
            }
            State::Cancelled => out(b"Cancelled"),
        }
    }

    /// The `State` column of a phone's two: `28 %` (or what arrived without a total), `Done`,
    /// `Failed` or `Cancelled`.
    pub fn write_short_state(&self, out: &mut dyn FnMut(&[u8])) {
        match (self.state, self.percent()) {
            (State::Downloading, Some(percent)) => {
                write_decimal(percent, out);
                out(b" %");
            }
            (State::Downloading, None) => write_size(self.received, out),
            (State::Done, _) => out(b"Done"),
            (State::Failed, _) => out(b"Failed"),
            (State::Cancelled, _) => out(b"Cancelled"),
        }
    }

    /// The detail line: `Saving to <path>`, `Saved to <path>`, `Failed: <reason>` or
    /// `Cancelled: the partial file was removed.`
    pub fn write_detail(&self, out: &mut dyn FnMut(&[u8])) {
        match self.state {
            State::Downloading => {
                out(b"Saving to ");
                out(self.path);
            }
            State::Done => {
                out(b"Saved to ");
                out(self.path);
            }
            State::Failed => self.write_state(out),
            State::Cancelled => out(b"Cancelled: the partial file was removed."),
        }
    }
}

/// A URL's host with its port, without the scheme, the user or the path.
pub fn host_of(url: &[u8]) -> &[u8] {
    let rest = url
        .windows(3)
        .position(|window| window == b"://")
        .map_or(url, |at| &url[at + 3..]);
    let end = rest
        .iter()
        .position(|&byte| matches!(byte, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    authority
        .iter()
        .rposition(|&byte| byte == b'@')
        .map_or(authority, |at| &authority[at + 1..])
}

/// The download id a record's file name is: its `fetch`'s process id, in decimal without
/// leading zeros.
pub fn id_of(file_name: &[u8]) -> Option<u32> {
    if file_name.first() == Some(&b'0') || file_name.len() > 10 {
        return None;
    }
    number(file_name).and_then(|id| u32::try_from(id).ok())
}

/// Whether `/proc/ps`'s text `ps` lists `pid` as a [`FETCH`] that has not ended: a process
/// with that id running another program is not the download's, whose `fetch` is gone.
pub fn fetch_running(ps: &[u8], pid: u32) -> bool {
    ps.split(|&byte| byte == b'\n').any(|line| {
        let mut fields = line.splitn(7, |&byte| byte == b' ');
        let listed = fields.next().and_then(number) == Some(u64::from(pid));
        let state = fields.nth(3);
        let name = fields.nth(1);
        listed && state != Some(b"Z") && name == Some(FETCH)
    })
}

/// Whether `/proc/<pid>/cmdline`'s text `command_line` runs [`FETCH`]: its first word's last
/// component is `fetch`, or [`MULTI_CALL`]'s followed by `fetch`.
pub fn runs_fetch(command_line: &[u8]) -> bool {
    let mut words = command_line.split(|&byte| byte == 0);
    let program = words.next().unwrap_or_default();
    let program = program
        .rsplit(|&byte| byte == b'/')
        .next()
        .unwrap_or(program);
    program == FETCH || (program == MULTI_CALL && words.next() == Some(FETCH))
}

/// The list's order of two downloads, each `(started, id)`: newest first, then the larger id.
pub fn newest_first(first: (u64, u32), second: (u64, u32)) -> Ordering {
    second.cmp(&first)
}

/// A failure's reason as a record keeps it, from the error `fetch` wrote: what follows the
/// message's last `: ` (the place it names comes before), on one line, at most
/// [`MAX_REASON_BYTES`] bytes.
pub fn reason_of(message: &[u8]) -> &[u8] {
    let line = message
        .split(|&byte| byte == b'\n')
        .next()
        .unwrap_or_default();
    let reason = line
        .windows(2)
        .rposition(|window| window == b": ")
        .map_or(line, |at| &line[at + 2..]);
    let mut length = reason.len().min(MAX_REASON_BYTES);
    while length > 0 && length < reason.len() && reason[length] & 0xc0 == 0x80 {
        length -= 1;
    }
    &reason[..length]
}

/// Why a download cannot be cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelError {
    /// No record has the id.
    NoSuchDownload,
    /// The download is over, or its `fetch` is gone.
    AlreadyEnded,
    /// Its `fetch` belongs to another user.
    NotAllowed,
}

impl CancelError {
    /// Its error code in JSON and MCP.
    pub const fn code(self) -> &'static str {
        match self {
            CancelError::NoSuchDownload => "no_such_download",
            CancelError::AlreadyEnded => "already_ended",
            CancelError::NotAllowed => "permission_denied",
        }
    }

    /// What `download cancel` says after the id.
    pub const fn words(self) -> &'static str {
        match self {
            CancelError::NoSuchDownload => "no such download",
            CancelError::AlreadyEnded => "has already ended",
            CancelError::NotAllowed => "not allowed",
        }
    }
}

/// Cancels download `id`, whose judged record is `record`: sends its `fetch` `SIGTERM` through
/// `terminate`, which answers an errno on failure; `fetch` then removes its partial file and
/// records `cancelled`.
pub fn cancel(
    record: Option<&Record<'_>>,
    id: u32,
    terminate: impl FnOnce(u32, u32) -> Result<(), i32>,
) -> Result<(), CancelError> {
    let record = record.ok_or(CancelError::NoSuchDownload)?;
    if record.state.is_over() {
        return Err(CancelError::AlreadyEnded);
    }
    terminate(id, TERMINATE).map_err(|errno| match errno {
        EPERM => CancelError::NotAllowed,
        _ => CancelError::AlreadyEnded,
    })
}

/// Why a download's record cannot be cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearError {
    /// No record has the id.
    NoSuchDownload,
    /// It is still downloading.
    StillDownloading,
}

impl ClearError {
    /// Its error code in JSON and MCP.
    pub const fn code(self) -> &'static str {
        match self {
            ClearError::NoSuchDownload => "no_such_download",
            ClearError::StillDownloading => "still_downloading",
        }
    }

    /// What `download clear` says after the id.
    pub const fn words(self) -> &'static str {
        match self {
            ClearError::NoSuchDownload => "no such download",
            ClearError::StillDownloading => "still downloading",
        }
    }
}

/// Whether the judged `record` may be cleared: only a download that is over.
pub fn clear_refusal(record: Option<&Record<'_>>) -> Option<ClearError> {
    match record {
        None => Some(ClearError::NoSuchDownload),
        Some(record) if !record.state.is_over() => Some(ClearError::StillDownloading),
        Some(_) => None,
    }
}

/// What clearing `count` records says: `Cleared 3 downloads.`, `Cleared 1 download.`, or
/// `Nothing to clear.`
pub fn write_cleared(count: usize, out: &mut dyn FnMut(&[u8])) {
    match count {
        0 => out(b"Nothing to clear."),
        1 => out(b"Cleared 1 download."),
        count => {
            out(b"Cleared ");
            write_decimal(count as u64, out);
            out(b" downloads.");
        }
    }
}

#[cfg(test)]
mod tests;
