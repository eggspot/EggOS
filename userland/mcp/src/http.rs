//! MCP for agents on the network (D193): the Streamable HTTP transport over HTTPS, as small as
//! the specification allows. `POST /mcp` carries one JSON-RPC message and gets the server's
//! answer as `application/json` (202 with no body for a notification); there is no event
//! stream, so `GET` and `DELETE` answer 405. Every `/mcp` request carries the agent's token
//! as `Authorization: Bearer`; `POST /pair` trades a one-time code for that token
//! ([`crate::pairing`]). A request whose `Origin` is not the address it was sent to is refused,
//! against DNS rebinding. Requests and answers are HTTP/1.1 (RFC 9112) with `Content-Length`;
//! chunked bodies are refused.

use crate::json::{self, JsonWriter};
use crate::pairing::{Token, token_hash};

/// The most bytes of a request's line and headers.
pub const MAX_HEADER_BYTES: usize = 8192;
/// The path of the MCP endpoint.
pub const MCP_PATH: &[u8] = b"/mcp";
/// The path pairing uses.
pub const PAIR_PATH: &[u8] = b"/pair";
/// The port the server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 8443;
/// The most bytes of an answer's line and headers.
pub const MAX_RESPONSE_HEADER_BYTES: usize = 512;

const BEARER: &[u8] = b"bearer ";
const JSON_TYPE: &[u8] = b"application/json";

/// A request's method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `DELETE`.
    Delete,
    /// Any other.
    Other,
}

/// An answer's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 200.
    Ok,
    /// 202: a notification was taken.
    Accepted,
    /// 400.
    BadRequest,
    /// 401: no token, or one no agent has.
    Unauthorized,
    /// 403: a wrong `Origin`, or a wrong pairing code.
    Forbidden,
    /// 404.
    NotFound,
    /// 405.
    MethodNotAllowed,
    /// 411: a body without `Content-Length`.
    LengthRequired,
    /// 413.
    PayloadTooLarge,
    /// 415: an MCP message not sent as JSON.
    UnsupportedMediaType,
    /// 429: pairing is closed for now after too many wrong codes.
    TooManyRequests,
    /// 431.
    HeaderTooLarge,
    /// 501: a transfer coding the server does not read.
    NotImplemented,
}

impl Status {
    /// The status code.
    pub const fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::Accepted => 202,
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::LengthRequired => 411,
            Self::PayloadTooLarge => 413,
            Self::UnsupportedMediaType => 415,
            Self::TooManyRequests => 429,
            Self::HeaderTooLarge => 431,
            Self::NotImplemented => 501,
        }
    }

    /// The reason phrase.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Accepted => "Accepted",
            Self::BadRequest => "Bad Request",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not Found",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::LengthRequired => "Length Required",
            Self::PayloadTooLarge => "Content Too Large",
            Self::UnsupportedMediaType => "Unsupported Media Type",
            Self::TooManyRequests => "Too Many Requests",
            Self::HeaderTooLarge => "Request Header Fields Too Large",
            Self::NotImplemented => "Not Implemented",
        }
    }
}

/// A request read from the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    /// The method.
    pub method: Method,
    /// The path, without any query.
    pub path: &'a [u8],
    /// The `Authorization` header's value.
    pub authorization: Option<&'a [u8]>,
    /// The `Origin` header's value.
    pub origin: Option<&'a [u8]>,
    /// The `Host` header's value.
    pub host: Option<&'a [u8]>,
    /// The `Content-Type` header's value.
    pub content_type: Option<&'a [u8]>,
    /// The `MCP-Protocol-Version` header's value.
    pub protocol_version: Option<&'a [u8]>,
    /// The `Mcp-Method` header's value.
    pub mcp_method: Option<&'a [u8]>,
    /// The `Mcp-Name` header's value.
    pub mcp_name: Option<&'a [u8]>,
    /// The body.
    pub body: &'a [u8],
    /// Whether the client keeps the connection for another request.
    pub keep_alive: bool,
}

/// What reading the buffer found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed<'a> {
    /// A whole request, which took this many bytes of the buffer.
    Complete(Request<'a>, usize),
    /// More bytes are needed.
    Incomplete,
    /// The request cannot be read: answer this status and close the connection.
    Refused(Status),
}

fn trimmed(mut value: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = value {
        value = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = value {
        value = rest;
    }
    value
}

fn decimal_length(value: &[u8]) -> Option<usize> {
    if value.is_empty() || value.len() > 10 {
        return None;
    }
    value.iter().try_fold(0usize, |total, &digit| {
        digit
            .is_ascii_digit()
            .then(|| total * 10 + usize::from(digit - b'0'))
    })
}

/// Reads one request from the start of `buffer`, whose body may be up to `max_body` bytes.
pub fn parse(buffer: &[u8], max_body: usize) -> Parsed<'_> {
    let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") else {
        return match buffer.len() > MAX_HEADER_BYTES {
            true => Parsed::Refused(Status::HeaderTooLarge),
            false => Parsed::Incomplete,
        };
    };
    if end > MAX_HEADER_BYTES {
        return Parsed::Refused(Status::HeaderTooLarge);
    }
    let mut lines = buffer[..end].split(|&byte| byte == b'\n');
    let request_line = lines.next().unwrap_or_default();
    let request_line = request_line.strip_suffix(b"\r").unwrap_or(request_line);
    let mut words = request_line.split(|&byte| byte == b' ');
    let (Some(method), Some(target), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Parsed::Refused(Status::BadRequest);
    };
    let keep_alive_by_default = match version {
        b"HTTP/1.1" => true,
        b"HTTP/1.0" => false,
        _ => return Parsed::Refused(Status::BadRequest),
    };
    let method = match method {
        b"GET" => Method::Get,
        b"POST" => Method::Post,
        b"DELETE" => Method::Delete,
        _ => Method::Other,
    };
    let path = target
        .iter()
        .position(|&byte| byte == b'?')
        .map_or(target, |query| &target[..query]);
    let mut request = Request {
        method,
        path,
        authorization: None,
        origin: None,
        host: None,
        content_type: None,
        protocol_version: None,
        mcp_method: None,
        mcp_name: None,
        body: &[],
        keep_alive: keep_alive_by_default,
    };
    let mut length = None;
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|&byte| byte == b':') else {
            return Parsed::Refused(Status::BadRequest);
        };
        let (name, value) = (&line[..colon], trimmed(&line[colon + 1..]));
        let is = |wanted: &[u8]| name.eq_ignore_ascii_case(wanted);
        if is(b"content-length") {
            match decimal_length(value) {
                Some(parsed) if length.is_none_or(|known| known == parsed) => length = Some(parsed),
                _ => return Parsed::Refused(Status::BadRequest),
            }
        } else if is(b"transfer-encoding") {
            return Parsed::Refused(Status::NotImplemented);
        } else if is(b"authorization") {
            request.authorization = Some(value);
        } else if is(b"origin") {
            request.origin = Some(value);
        } else if is(b"host") {
            request.host = Some(value);
        } else if is(b"content-type") {
            request.content_type = Some(value);
        } else if is(b"mcp-protocol-version") {
            request.protocol_version = Some(value);
        } else if is(b"mcp-method") {
            request.mcp_method = Some(value);
        } else if is(b"mcp-name") {
            request.mcp_name = Some(value);
        } else if is(b"connection") {
            if value.eq_ignore_ascii_case(b"close") {
                request.keep_alive = false;
            } else if value.eq_ignore_ascii_case(b"keep-alive") {
                request.keep_alive = true;
            }
        }
    }
    let body_length = match (length, method) {
        (Some(length), _) => length,
        (None, Method::Post) => return Parsed::Refused(Status::LengthRequired),
        (None, _) => 0,
    };
    if body_length > max_body {
        return Parsed::Refused(Status::PayloadTooLarge);
    }
    let start = end + 4;
    match buffer.get(start..start + body_length) {
        Some(body) => {
            request.body = body;
            Parsed::Complete(request, start + body_length)
        }
        None => Parsed::Incomplete,
    }
}

/// Where a request goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route<'a> {
    /// An MCP message from the agent holding the token with this hash.
    Mcp([u8; 32], &'a [u8]),
    /// A pairing request, with its JSON body.
    Pair(&'a [u8]),
    /// Refused with this status and message.
    Refused(Status, &'static str),
}

fn origin_matches(origin: &[u8], host: Option<&[u8]>) -> bool {
    let Some(host) = host else {
        return false;
    };
    origin
        .strip_prefix(b"https://")
        .is_some_and(|rest| rest.eq_ignore_ascii_case(host))
}

fn is_json(content_type: Option<&[u8]>) -> bool {
    content_type.is_some_and(|value| {
        let media = value
            .iter()
            .position(|&byte| byte == b';')
            .map_or(value, |semicolon| &value[..semicolon]);
        trimmed(media).eq_ignore_ascii_case(JSON_TYPE)
    })
}

/// Whether the connection stays open after answering `route` with `status`: only after an
/// MCP message whose token checked out, from a client that asked to keep it. Anything else, a
/// refusal, a pairing or a token that is no agent's, closes it, so a stranger cannot hold the
/// server's connection.
pub fn keeps_connection(route: &Route<'_>, status: Status, keep_alive: bool) -> bool {
    keep_alive && matches!(route, Route::Mcp(..)) && status != Status::Unauthorized
}

/// Decides where `request` goes.
pub fn route<'a>(request: &Request<'a>) -> Route<'a> {
    if let Some(origin) = request.origin
        && !origin_matches(origin, request.host)
    {
        return Route::Refused(
            Status::Forbidden,
            "the request comes from a web page of another address",
        );
    }
    match (request.path, request.method) {
        (MCP_PATH, Method::Post) => {
            let token = request.authorization.and_then(|value| {
                let (scheme, token) = value.split_at_checked(BEARER.len())?;
                scheme
                    .eq_ignore_ascii_case(BEARER)
                    .then_some(trimmed(token))
            });
            let Some(token) = token.filter(|token| !token.is_empty()) else {
                return Route::Refused(
                    Status::Unauthorized,
                    "pair this agent with EggOS first: agent pair <id> on EggOS",
                );
            };
            if !is_json(request.content_type) {
                return Route::Refused(
                    Status::UnsupportedMediaType,
                    "MCP messages are sent as application/json",
                );
            }
            Route::Mcp(token_hash(token), request.body)
        }
        (MCP_PATH, _) => Route::Refused(
            Status::MethodNotAllowed,
            "this server answers POST only: it has no event stream",
        ),
        (PAIR_PATH, Method::Post) => Route::Pair(request.body),
        (PAIR_PATH, _) => Route::Refused(Status::MethodNotAllowed, "pair with POST"),
        _ => Route::Refused(Status::NotFound, "no such page: MCP is at /mcp"),
    }
}

/// Whether an MCP message is a stateless (2026-07-28) one, and if so whether its headers say
/// what its body does: `MCP-Protocol-Version` the version in `_meta`, `Mcp-Method` the method,
/// and `Mcp-Name` the tool, prompt or resource for `tools/call`, `prompts/get` and
/// `resources/read` (decoded first when sent as `=?base64?...?=`). `Ok(false)` for a message of
/// the older versions, which have no such headers; `Err` with the mismatch to report.
pub fn check_stateless_headers(request: &Request<'_>) -> Result<bool, &'static str> {
    let Ok(parsed) = json::parse(request.body) else {
        return Ok(false);
    };
    let params = parsed.value.get("params");
    let Some(version) = params
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(crate::session::META_PROTOCOL_VERSION))
        .and_then(|version| version.as_str())
    else {
        return Ok(false);
    };
    let matches = |header: Option<&[u8]>, text: json::Str<'_>| {
        let mut decoded = [0u8; 256];
        header.is_some_and(|header| {
            let header = sentinel_decoded(header, &mut decoded).unwrap_or(header);
            core::str::from_utf8(header).is_ok_and(|header| text.equals(header))
        })
    };
    if !matches(request.protocol_version, version) {
        return Err("MCP-Protocol-Version does not match the version in _meta");
    }
    let Some(method) = parsed
        .value
        .get("method")
        .and_then(|method| method.as_str())
    else {
        return Ok(true);
    };
    if !matches(request.mcp_method, method) {
        return Err("Mcp-Method does not match the method");
    }
    let named = if method.equals("tools/call") || method.equals("prompts/get") {
        "name"
    } else if method.equals("resources/read") {
        "uri"
    } else {
        return Ok(true);
    };
    let Some(name) = params
        .and_then(|params| params.get(named))
        .and_then(|name| name.as_str())
    else {
        return Ok(true);
    };
    match matches(request.mcp_name, name) {
        true => Ok(true),
        false => Err("Mcp-Name does not match the tool, prompt or resource"),
    }
}

fn sentinel_decoded<'o>(header: &[u8], out: &'o mut [u8; 256]) -> Option<&'o [u8]> {
    let encoded = header.strip_prefix(b"=?base64?")?.strip_suffix(b"?=")?;
    let mut length = 0;
    let mut bits = 0u32;
    let mut count = 0;
    for &byte in encoded {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            *out.get_mut(length)? = (bits >> count) as u8;
            length += 1;
        }
    }
    out.get(..length)
}

/// The code in a pairing request's body, `{"code": "ABCD-EFGH"}`, copied into `out`.
pub fn pair_code<'o>(body: &[u8], out: &'o mut [u8; 32]) -> Option<&'o [u8]> {
    let parsed = json::parse(body).ok()?;
    let code = parsed.value.get("code")?.as_str()?;
    let (text, complete) = code.decode_prefix_into(out);
    let length = text.len();
    match complete {
        true => out.get(..length),
        false => None,
    }
}

/// The body answering a pairing: the agent's id and its token.
pub fn pair_answer(agent: &[u8], token: &Token, out: &mut [u8]) -> usize {
    let mut writer = JsonWriter::new(out);
    writer.begin_object();
    writer.key("agent");
    writer.string(core::str::from_utf8(agent).unwrap_or_default());
    writer.key("token");
    writer.string(core::str::from_utf8(token).unwrap_or_default());
    writer.end_object();
    written(&writer)
}

fn written(writer: &JsonWriter<'_>) -> usize {
    match writer.overflowed() {
        true => 0,
        false => writer.len(),
    }
}

/// The body refusing a stateless message whose headers do not match it: a JSON-RPC error
/// with code -32020, as MCP 2026-07-28 says.
pub fn header_mismatch_body(message: &str, out: &mut [u8]) -> usize {
    let mut writer = JsonWriter::new(out);
    crate::rpc::write_error(&mut writer, None, crate::rpc::HEADER_MISMATCH, message);
    written(&writer)
}

/// The HTTP status of a stateless message's answer: 404 for a method the server does not have,
/// 400 for a version it does not speak or parameters it does not take, else 200.
pub fn stateless_status(answer: &[u8]) -> Status {
    let has = |code: &[u8]| answer.windows(code.len()).any(|window| window == code);
    if has(b"\"error\":{\"code\":-32601") {
        Status::NotFound
    } else if has(b"\"error\":{\"code\":-32022") || has(b"\"error\":{\"code\":-32602") {
        Status::BadRequest
    } else {
        Status::Ok
    }
}

/// The body of a refusal: `{"error": message}`.
pub fn error_body(message: &str, out: &mut [u8]) -> usize {
    let mut writer = JsonWriter::new(out);
    writer.begin_object();
    writer.key("error");
    writer.string(message);
    writer.end_object();
    written(&writer)
}

/// Writes an answer's status line and headers into `out` for a JSON body of `body_length`
/// bytes (none when 0); returns their length.
pub fn response_head(
    out: &mut [u8],
    status: Status,
    body_length: usize,
    keep_alive: bool,
) -> usize {
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        if let Some(place) = out.get_mut(length..length + bytes.len()) {
            place.copy_from_slice(bytes);
            length += bytes.len();
        }
    };
    let mut digits = [0u8; 20];
    put(b"HTTP/1.1 ");
    put(number(u64::from(status.code()), &mut digits));
    put(b" ");
    put(status.reason().as_bytes());
    put(b"\r\n");
    if body_length > 0 {
        put(b"Content-Type: application/json\r\n");
    }
    put(b"Content-Length: ");
    put(number(body_length as u64, &mut digits));
    put(b"\r\n");
    match status {
        Status::Unauthorized => put(b"WWW-Authenticate: Bearer realm=\"EggOS\"\r\n"),
        Status::MethodNotAllowed => put(b"Allow: POST\r\n"),
        _ => {}
    }
    put(b"Cache-Control: no-store\r\n");
    match keep_alive {
        true => put(b"Connection: keep-alive\r\n\r\n"),
        false => put(b"Connection: close\r\n\r\n"),
    }
    length
}

fn number(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
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

#[cfg(test)]
mod tests;
