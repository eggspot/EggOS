//! HTTP/1.1 for downloads (RFC 9110, RFC 9112): `http://` and `https://` URLs (TLS is the
//! caller's, D159), the `GET` request, the head
//! of a response (its status, how its body is framed, where a redirect points), and a body
//! decoded as it arrives, whether framed by its length, in chunks, or by the end of the
//! connection. Nothing here allocates: the caller keeps the head it read and passes the body's
//! bytes as they come.

/// The port of `http://` URLs that name none.
pub const DEFAULT_PORT: u16 = 80;
/// The port of `https://` URLs that name none.
pub const DEFAULT_SECURE_PORT: u16 = 443;
/// The longest head of a response the client reads.
pub const MAX_HEAD_BYTES: usize = 16 * 1024;
/// The longest host name.
pub const MAX_HOST_BYTES: usize = 253;

/// A parsed `http://` or `https://` URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Url<'a> {
    /// Whether it is `https://`: the exchange goes over TLS.
    pub secure: bool,
    /// The host: a name or an IPv4 address.
    pub host: &'a [u8],
    /// The port.
    pub port: u16,
    /// The path and query, `/` when the URL has none; the fragment is left out.
    pub target: &'a [u8],
}

/// Why a URL is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Its scheme is neither `http` nor `https`.
    NotHttp,
    /// It names no host, or a host with characters a name cannot have.
    BadHost,
    /// Its port is not a number from 1 to 65535.
    BadPort,
    /// It holds a space or a control character.
    BadCharacter,
}

impl UrlError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::NotHttp => "only http:// and https:// URLs are fetched",
            Self::BadHost => "the URL names no valid host",
            Self::BadPort => "the port is not a number from 1 to 65535",
            Self::BadCharacter => "the URL holds a space or a control character",
        }
    }
}

fn starts_with_ignoring_case(text: &[u8], prefix: &[u8]) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// Parses `http://host[:port][/path][?query][#fragment]`, or the same with `https://`.
pub fn parse_url(text: &[u8]) -> Result<Url<'_>, UrlError> {
    if text.iter().any(|&byte| byte <= b' ' || byte == 0x7f) {
        return Err(UrlError::BadCharacter);
    }
    let (secure, rest) = if starts_with_ignoring_case(text, b"https://") {
        (true, &text[b"https://".len()..])
    } else if starts_with_ignoring_case(text, b"http://") {
        (false, &text[b"http://".len()..])
    } else {
        return Err(UrlError::NotHttp);
    };
    let default_port = if secure {
        DEFAULT_SECURE_PORT
    } else {
        DEFAULT_PORT
    };
    let rest = match rest.iter().position(|&byte| byte == b'#') {
        Some(fragment) => &rest[..fragment],
        None => rest,
    };
    let authority_end = rest
        .iter()
        .position(|&byte| byte == b'/')
        .unwrap_or(rest.len());
    let (authority, target) = rest.split_at(authority_end);
    if authority.contains(&b'@') {
        return Err(UrlError::BadHost);
    }
    let (host, port) = match authority.iter().rposition(|&byte| byte == b':') {
        Some(colon) => (
            &authority[..colon],
            parse_port(&authority[colon + 1..], default_port)?,
        ),
        None => (authority, default_port),
    };
    if !is_host(host) {
        return Err(UrlError::BadHost);
    }
    let target: &[u8] = match target {
        [] => b"/",
        target => target,
    };
    Ok(Url {
        secure,
        host,
        port,
        target,
    })
}

fn parse_port(text: &[u8], default_port: u16) -> Result<u16, UrlError> {
    if text.is_empty() {
        return Ok(default_port);
    }
    if text.len() > 5 || !text.iter().all(u8::is_ascii_digit) {
        return Err(UrlError::BadPort);
    }
    let value = text
        .iter()
        .fold(0u32, |value, digit| value * 10 + u32::from(digit - b'0'));
    match u16::try_from(value) {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(UrlError::BadPort),
    }
}

fn is_host(host: &[u8]) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && host.iter().all(|&byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
        })
        && host.first() != Some(&b'.')
        && host.first() != Some(&b'-')
}

/// Where a redirect's `Location` points, resolved against `base`, the URL that answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redirect<'a> {
    /// A whole URL of its own.
    Absolute(Url<'a>),
    /// A path on `base`'s host and port.
    Path(&'a [u8]),
}

/// Resolves `location`, a redirect's `Location`, against the URL that answered: an absolute
/// `http://` or `https://` URL, or an absolute path on the same host. Other forms are refused.
pub fn redirect(location: &[u8]) -> Result<Redirect<'_>, UrlError> {
    match location.first() {
        Some(b'/') if location.get(1) != Some(&b'/') => {
            if location.iter().any(|&byte| byte <= b' ' || byte == 0x7f) {
                return Err(UrlError::BadCharacter);
            }
            let end = location
                .iter()
                .position(|&byte| byte == b'#')
                .unwrap_or(location.len());
            Ok(Redirect::Path(&location[..end]))
        }
        _ => parse_url(location).map(Redirect::Absolute),
    }
}

/// Writes the `GET` request for `url` to `out`, asking the server to close the connection
/// after its answer: its length, or `None` when `out` is shorter.
pub fn write_request(out: &mut [u8], url: &Url<'_>, user_agent: &[u8]) -> Option<usize> {
    let mut writer = Writer { out, length: 0 };
    writer.put(b"GET ")?;
    writer.put(url.target)?;
    writer.put(b" HTTP/1.1\r\nHost: ")?;
    writer.put(url.host)?;
    let default_port = if url.secure {
        DEFAULT_SECURE_PORT
    } else {
        DEFAULT_PORT
    };
    if url.port != default_port {
        writer.put(b":")?;
        let mut digits = [0u8; 5];
        let mut port = url.port;
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b'0' + (port % 10) as u8;
            port /= 10;
            if port == 0 {
                break;
            }
        }
        writer.put(&digits[start..])?;
    }
    writer.put(b"\r\nUser-Agent: ")?;
    writer.put(user_agent)?;
    writer.put(b"\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n")?;
    Some(writer.length)
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
}

/// Where the head of a response ends in `bytes`, the response so far: the length of the head
/// with the empty line that ends it, or `None` while it has not ended.
pub fn head_end(bytes: &[u8]) -> Option<usize> {
    let mut line_start = 0;
    for (at, &byte) in bytes.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let line = &bytes[line_start..at];
        if line.is_empty() || line == b"\r" {
            return Some(at + 1);
        }
        line_start = at + 1;
    }
    None
}

/// How a response's body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// After this many bytes (`Content-Length`).
    Length(u64),
    /// After its last chunk (`Transfer-Encoding: chunked`).
    Chunked,
    /// When the server closes the connection.
    UntilClose,
}

/// The head of a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head<'a> {
    /// The status code.
    pub status: u16,
    /// The reason phrase.
    pub reason: &'a [u8],
    /// How the body is framed.
    pub framing: Framing,
    /// Where a redirect points (`Location`).
    pub location: Option<&'a [u8]>,
    /// The body's media type (`Content-Type`).
    pub content_type: Option<&'a [u8]>,
}

impl Head<'_> {
    /// Whether the status is a redirect the client follows: 301, 302, 303, 307 or 308.
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    /// Whether the status is an interim answer (1xx) another head follows.
    pub fn is_interim(&self) -> bool {
        (100..200).contains(&self.status)
    }
}

/// Why the head of a response is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadError {
    /// The status line is not `HTTP/1.x <three digits> <reason>`.
    BadStatusLine,
    /// A header line has no name or no colon.
    BadHeader,
    /// `Content-Length` is not a number, or two differ.
    BadLength,
    /// The body is encoded in a way the client does not decode.
    UnsupportedEncoding,
}

impl HeadError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::BadStatusLine => "the server's answer is not HTTP/1.x",
            Self::BadHeader => "the server's answer has a malformed header",
            Self::BadLength => "the server's answer has a malformed Content-Length",
            Self::UnsupportedEncoding => {
                "the server's answer uses a transfer encoding EggOS does not decode"
            }
        }
    }
}

fn trimmed(mut text: &[u8]) -> &[u8] {
    while let [b' ' | b'\t' | b'\r', rest @ ..] = text {
        text = rest;
    }
    while let [rest @ .., b' ' | b'\t' | b'\r'] = text {
        text = rest;
    }
    text
}

fn parse_length(text: &[u8]) -> Result<u64, HeadError> {
    if text.is_empty() || text.len() > 19 || !text.iter().all(u8::is_ascii_digit) {
        return Err(HeadError::BadLength);
    }
    Ok(text
        .iter()
        .fold(0u64, |value, digit| value * 10 + u64::from(digit - b'0')))
}

/// Parses `head`, a response's head as [`head_end`] measured it, for a `GET` request.
pub fn parse_head(head: &[u8]) -> Result<Head<'_>, HeadError> {
    let mut lines = head
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let status_line = lines.next().ok_or(HeadError::BadStatusLine)?;
    let (status, reason) = parse_status_line(status_line)?;
    let mut length = None;
    let mut chunked = false;
    let mut location = None;
    let mut content_type = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let colon = line
            .iter()
            .position(|&byte| byte == b':')
            .ok_or(HeadError::BadHeader)?;
        let name = &line[..colon];
        if name.is_empty() || name.iter().any(|&byte| byte <= b' ') {
            return Err(HeadError::BadHeader);
        }
        let value = trimmed(&line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"content-length") {
            let parsed = parse_length(value)?;
            if length.is_some_and(|known| known != parsed) {
                return Err(HeadError::BadLength);
            }
            length = Some(parsed);
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            let last = value.rsplit(|&byte| byte == b',').next().map(trimmed);
            match last {
                Some(coding) if coding.eq_ignore_ascii_case(b"chunked") => chunked = true,
                Some(coding) if coding.eq_ignore_ascii_case(b"identity") => {}
                _ => return Err(HeadError::UnsupportedEncoding),
            }
        } else if name.eq_ignore_ascii_case(b"location") {
            location = Some(value);
        } else if name.eq_ignore_ascii_case(b"content-type") {
            content_type = Some(value);
        }
    }
    let framing = match (status, chunked, length) {
        (100..=199 | 204 | 304, _, _) => Framing::Length(0),
        (_, true, _) => Framing::Chunked,
        (_, false, Some(length)) => Framing::Length(length),
        (_, false, None) => Framing::UntilClose,
    };
    Ok(Head {
        status,
        reason,
        framing,
        location,
        content_type,
    })
}

fn parse_status_line(line: &[u8]) -> Result<(u16, &[u8]), HeadError> {
    let rest = line
        .strip_prefix(b"HTTP/1.")
        .ok_or(HeadError::BadStatusLine)?;
    let [minor, b' ', a, b, c, rest @ ..] = rest else {
        return Err(HeadError::BadStatusLine);
    };
    if !minor.is_ascii_digit() || ![a, b, c].iter().all(|digit| digit.is_ascii_digit()) {
        return Err(HeadError::BadStatusLine);
    }
    let status = u16::from(a - b'0') * 100 + u16::from(b - b'0') * 10 + u16::from(c - b'0');
    let reason = match rest {
        [] => &[][..],
        [b' ', reason @ ..] => reason,
        _ => return Err(HeadError::BadStatusLine),
    };
    Ok((status, reason))
}

/// Why a body could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// A chunk's size line is malformed or too large.
    BadChunk,
    /// The connection closed before the body ended.
    Truncated,
}

impl BodyError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::BadChunk => "the server sent a malformed chunk",
            Self::Truncated => "the connection closed before the whole file came",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chunk {
    Size {
        value: u64,
        digits: u8,
        extension: bool,
    },
    Data(u64),
    DataEnd {
        seen_cr: bool,
    },
    Trailer {
        line_empty: bool,
    },
    Done,
}

/// A body decoded as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body {
    framing: Framing,
    left: u64,
    chunk: Chunk,
}

/// What one step of decoding found: the bytes of `input` it used, and the body's bytes among
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// How many bytes of the input it used.
    pub used: usize,
    /// The body's bytes among them.
    pub data: &'a [u8],
}

impl Body {
    /// A body framed by `framing`.
    pub fn new(framing: Framing) -> Self {
        let left = match framing {
            Framing::Length(length) => length,
            _ => 0,
        };
        Self {
            framing,
            left,
            chunk: Chunk::Size {
                value: 0,
                digits: 0,
                extension: false,
            },
        }
    }

    /// Whether the whole body came.
    pub fn is_done(&self) -> bool {
        match self.framing {
            Framing::Length(_) => self.left == 0,
            Framing::Chunked => self.chunk == Chunk::Done,
            Framing::UntilClose => false,
        }
    }

    /// Decodes from the start of `input`, bytes as they came: the bytes it used and the
    /// body's bytes among them, which come at most one run at a time, so call it again with
    /// the rest of the input until it used all of it. Bytes after the body's end are not used.
    pub fn decode<'a>(&mut self, input: &'a [u8]) -> Result<Decoded<'a>, BodyError> {
        match self.framing {
            Framing::UntilClose => Ok(Decoded {
                used: input.len(),
                data: input,
            }),
            Framing::Length(_) => {
                let count = input
                    .len()
                    .min(usize::try_from(self.left).unwrap_or(usize::MAX));
                self.left -= count as u64;
                Ok(Decoded {
                    used: count,
                    data: &input[..count],
                })
            }
            Framing::Chunked => self.decode_chunked(input),
        }
    }

    fn decode_chunked<'a>(&mut self, input: &'a [u8]) -> Result<Decoded<'a>, BodyError> {
        let mut used = 0;
        while used < input.len() {
            if let Chunk::Data(left) = self.chunk {
                let rest = &input[used..];
                let count = rest.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                self.chunk = match left - count as u64 {
                    0 => Chunk::DataEnd { seen_cr: false },
                    left => Chunk::Data(left),
                };
                return Ok(Decoded {
                    used: used + count,
                    data: &rest[..count],
                });
            }
            let byte = input[used];
            used += 1;
            self.chunk = match self.chunk {
                Chunk::Size {
                    value,
                    digits,
                    extension,
                } => match byte {
                    b'\n' if digits == 0 => return Err(BodyError::BadChunk),
                    b'\n' if value == 0 => Chunk::Trailer { line_empty: true },
                    b'\n' => Chunk::Data(value),
                    _ if extension || byte == b'\r' => Chunk::Size {
                        value,
                        digits,
                        extension,
                    },
                    b';' | b' ' | b'\t' => Chunk::Size {
                        value,
                        digits,
                        extension: true,
                    },
                    _ => {
                        let digit = (byte as char).to_digit(16).ok_or(BodyError::BadChunk)?;
                        if digits >= 15 {
                            return Err(BodyError::BadChunk);
                        }
                        Chunk::Size {
                            value: value * 16 + u64::from(digit),
                            digits: digits + 1,
                            extension,
                        }
                    }
                },
                Chunk::DataEnd { seen_cr } => match (byte, seen_cr) {
                    (b'\r', false) => Chunk::DataEnd { seen_cr: true },
                    (b'\n', _) => Chunk::Size {
                        value: 0,
                        digits: 0,
                        extension: false,
                    },
                    _ => return Err(BodyError::BadChunk),
                },
                Chunk::Trailer { line_empty } => match byte {
                    b'\n' if line_empty => {
                        self.chunk = Chunk::Done;
                        return Ok(Decoded { used, data: &[] });
                    }
                    b'\n' => Chunk::Trailer { line_empty: true },
                    b'\r' => Chunk::Trailer { line_empty },
                    _ => Chunk::Trailer { line_empty: false },
                },
                Chunk::Done => {
                    return Ok(Decoded {
                        used: used - 1,
                        data: &[],
                    });
                }
                Chunk::Data(_) => unreachable!(),
            };
        }
        Ok(Decoded { used, data: &[] })
    }

    /// Says the connection closed: an error unless the body is whole, or framed by the close.
    pub fn finish(&self) -> Result<(), BodyError> {
        match self.is_done() || self.framing == Framing::UntilClose {
            true => Ok(()),
            false => Err(BodyError::Truncated),
        }
    }
}

#[cfg(test)]
mod tests;
